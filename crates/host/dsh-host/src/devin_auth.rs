//! One-use loopback callback for Devin's PKCE authorization flow.
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(crate) struct Login {
    pub verifier: String,
    pub authorization_url: String,
    pub expires: u64,
    code: Arc<Mutex<Option<String>>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Login {
    pub async fn start(
        port: u16,
        state: String,
        verifier: String,
        challenge: String,
        now: u64,
    ) -> Result<Self, String> {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .map_err(|_| {
                "Devin 登录回调端口不可用，请结束其他进行中的 Devin 登录后重试".to_string()
            })?;
        let port = listener
            .local_addr()
            .map_err(|_| "无法读取 Devin 登录回调地址")?
            .port();
        let callback = format!("http://127.0.0.1:{port}/callback");
        let mut url = reqwest::Url::parse("https://app.devin.ai/auth/cli/continue").unwrap();
        url.query_pairs_mut()
            .append_pair("redirect_uri", &callback)
            .append_pair("state", &state)
            .append_pair("prompt", "select_account")
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256");
        let code = Arc::new(Mutex::new(None));
        let received = code.clone();
        let task = tokio::spawn(async move {
            let receive = async move {
                loop {
                    let Ok((mut socket, _)) = listener.accept().await else {
                        return;
                    };
                    let mut bytes = Vec::new();
                    let request = tokio::time::timeout(Duration::from_secs(5), async {
                        let mut chunk = [0u8; 1024];
                        loop {
                            let count = socket.read(&mut chunk).await.ok()?;
                            if count == 0 {
                                return None;
                            }
                            bytes.extend_from_slice(&chunk[..count]);
                            if bytes.len() > 16 * 1024 {
                                return None;
                            }
                            if bytes.windows(4).any(|part| part == b"\r\n\r\n") {
                                break;
                            }
                        }
                        std::str::from_utf8(&bytes).ok().map(str::to_owned)
                    })
                    .await
                    .ok()
                    .flatten();
                    let parsed = request
                        .as_deref()
                        .and_then(|request| parse_callback(request, &state));
                    let (status, body) = if parsed.is_some() {
                        ("200 OK", "Devin 授权已收到，请返回 DeepSeek Harness。")
                    } else {
                        ("400 Bad Request", "无效的登录回调，请返回原授权页面继续。")
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    if let Some(value) = parsed {
                        *received.lock() = Some(value);
                        return;
                    }
                }
            };
            let _ = tokio::time::timeout(Duration::from_secs(900), receive).await;
        });
        Ok(Self {
            verifier,
            authorization_url: url.into(),
            expires: now + 900,
            code,
            task: Some(task),
        })
    }
    pub fn code(&self) -> Option<String> {
        self.code.lock().clone()
    }
    pub async fn close(mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
    }
}
impl Drop for Login {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

fn parse_callback(request: &str, expected_state: &str) -> Option<String> {
    let mut line = request.lines().next()?.split_whitespace();
    if line.next()? != "GET" {
        return None;
    }
    let path = line.next()?;
    if !path.starts_with("/callback?") {
        return None;
    }
    let url = reqwest::Url::parse(&format!("http://127.0.0.1{path}")).ok()?;
    if url.path() != "/callback" {
        return None;
    }
    let states: Vec<_> = url
        .query_pairs()
        .filter(|(name, _)| name == "state")
        .collect();
    let codes: Vec<_> = url
        .query_pairs()
        .filter(|(name, _)| name == "code")
        .collect();
    if states.len() != 1 || states[0].1 != expected_state || codes.len() != 1 {
        return None;
    }
    let code = codes[0].1.as_ref();
    if code.is_empty() || code.len() > 4096 || code.chars().any(char::is_control) {
        return None;
    }
    Some(code.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn callback_rejects_foreign_state_duplicates_and_non_callback_paths() {
        for request in [
            "GET /callback?code=x&state=other HTTP/1.1\r\n\r\n",
            "GET /callback?code=x&state=s&state=s HTTP/1.1\r\n\r\n",
            "GET /other?code=x&state=s HTTP/1.1\r\n\r\n",
            "POST /callback?code=x&state=s HTTP/1.1\r\n\r\n",
        ] {
            assert!(parse_callback(request, "s").is_none())
        }
        assert_eq!(
            parse_callback("GET /callback?code=x%2By&state=s HTTP/1.1\r\n\r\n", "s").as_deref(),
            Some("x+y")
        );
    }
    #[tokio::test]
    async fn callback_is_one_use_and_cancellation_releases_its_port() {
        let login = Login::start(0, "state".into(), "verifier".into(), "challenge".into(), 1)
            .await
            .unwrap();
        let url = reqwest::Url::parse(&login.authorization_url).unwrap();
        let callback = url
            .query_pairs()
            .find(|(key, _)| key == "redirect_uri")
            .unwrap()
            .1
            .into_owned();
        let mut address = reqwest::Url::parse(&callback).unwrap();
        let port = address.port().unwrap();
        address
            .query_pairs_mut()
            .append_pair("state", "wrong")
            .append_pair("code", "ignored");
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        assert_eq!(
            client
                .get(address.clone())
                .send()
                .await
                .unwrap()
                .status()
                .as_u16(),
            400
        );
        assert!(login.code().is_none());
        address.set_query(Some("state=state&code=accepted"));
        assert!(
            client
                .get(address)
                .send()
                .await
                .unwrap()
                .status()
                .is_success()
        );
        tokio::task::yield_now().await;
        assert_eq!(login.code().as_deref(), Some("accepted"));
        login.close().await;
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap();
        drop(listener);
        let login = Login::start(
            port,
            "new-state".into(),
            "verifier".into(),
            "challenge".into(),
            1,
        )
        .await
        .unwrap();
        login.close().await;
        assert!(
            tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
                .await
                .is_ok()
        );
    }
}
