use http_body_util::{BodyExt, Full};
use hyper::{HeaderMap, StatusCode};
use hyper_util::rt::TokioIo;

enum ResponseBody {
    Reqwest(reqwest::Response),
    Hyper {
        body: hyper::body::Incoming,
        driver: tokio::task::JoinHandle<()>,
    },
}

pub(crate) struct CancelableResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    response: ResponseBody,
}

impl Drop for CancelableResponse {
    fn drop(&mut self) {
        if let ResponseBody::Hyper { driver, .. } = &self.response {
            driver.abort();
        }
    }
}

impl CancelableResponse {
    pub async fn next_data(&mut self) -> Result<Option<bytes::Bytes>, String> {
        match &mut self.response {
            ResponseBody::Reqwest(response) => response
                .chunk()
                .await
                .map_err(|error| format!("HTTP response body failed: {error}")),
            ResponseBody::Hyper { body, .. } => loop {
                match body.frame().await {
                    Some(Ok(frame)) => {
                        if let Ok(data) = frame.into_data() {
                            return Ok(Some(data));
                        }
                    }
                    Some(Err(error)) => return Err(format!("HTTP response body failed: {error}")),
                    None => return Ok(None),
                }
            },
        }
    }

    pub async fn collect_limited(mut self, limit: usize) -> Result<Vec<u8>, String> {
        let mut bytes = Vec::new();
        while let Some(chunk) = self.next_data().await? {
            if bytes.len().saturating_add(chunk.len()) > limit {
                return Err(format!("HTTP response body exceeded {limit} bytes"));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .tcp_nodelay(true)
        .http1_only()
        .pool_idle_timeout(std::time::Duration::ZERO)
        .pool_max_idle_per_host(0)
        .build()
        .map_err(|error| format!("provider HTTP client build failed: {error}"))
}

pub(crate) async fn post(
    url: &str,
    api_key: &str,
    body: Vec<u8>,
    attribution: &[(String, String)],
    cancelled: Option<std::sync::Arc<dyn Fn() -> bool + Send + Sync>>,
) -> Result<CancelableResponse, String> {
    let url = reqwest::Url::parse(url).map_err(|error| format!("invalid provider URL: {error}"))?;
    let host = url
        .host_str()
        .ok_or_else(|| "provider URL has no host".to_string())?;
    match url.scheme() {
        "https" => {}
        "http" if is_loopback_host(host) => {}
        "http" => return Err(
            "plain HTTP provider URLs are restricted to loopback; remote providers require HTTPS"
                .to_string(),
        ),
        scheme => return Err(format!("unsupported provider URL scheme {scheme:?}")),
    }
    if url.scheme() == "http" {
        let port = url.port_or_known_default().unwrap_or(80);
        let stream = tokio::net::TcpStream::connect((host, port))
            .await
            .map_err(|error| format!("provider TCP connect failed: {error}"))?;
        let io = TokioIo::new(stream);
        let (mut sender, connection) = hyper::client::conn::http1::handshake(io)
            .await
            .map_err(|error| format!("provider HTTP handshake failed: {error}"))?;
        let driver = tokio::spawn(async move {
            let _ = connection.await;
        });
        let mut builder = hyper::Request::builder()
            .method(hyper::Method::POST)
            .uri({
                let mut path = url.path().to_string();
                if let Some(query) = url.query() {
                    path.push('?');
                    path.push_str(query);
                }
                path
            })
            .header(hyper::header::AUTHORIZATION, format!("Bearer {api_key}"))
            .header(hyper::header::ACCEPT, "text/event-stream")
            .header(hyper::header::ACCEPT_ENCODING, "identity")
            .header(hyper::header::CONTENT_TYPE, "application/json")
            .header(hyper::header::CONNECTION, "close");
        for (name, value) in attribution {
            builder = builder.header(name, value);
        }
        let request = builder
            .body(Full::new(bytes::Bytes::from(body)))
            .map_err(|error| format!("provider HTTP request build failed: {error}"))?;
        let response = tokio::select! {
            result = sender.send_request(request) => result
                .map_err(|error| format!("provider HTTP request failed: {error}"))?,
            _ = async {
                loop {
                    if cancelled.as_ref().is_some_and(|is_cancelled| is_cancelled()) { break; }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            } => {
                driver.abort();
                return Err("provider request cancelled before response headers".to_string());
            }
        };
        return Ok(CancelableResponse {
            status: response.status(),
            headers: response.headers().clone(),
            response: ResponseBody::Hyper {
                body: response.into_body(),
                driver,
            },
        });
    }
    let client = client()?;
    let mut request = client
        .post(url)
        .bearer_auth(api_key)
        .header(reqwest::header::ACCEPT, "text/event-stream")
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(reqwest::header::CONNECTION, "close");
    for (name, value) in attribution {
        request = request.header(name, value);
    }
    let send = request.body(body).send();
    tokio::pin!(send);
    let response = if let Some(cancelled) = cancelled {
        let cancel_wait = async move {
            loop {
                if cancelled() {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        };
        tokio::pin!(cancel_wait);
        tokio::select! {
            result = &mut send => result
                .map_err(|error| format!("provider HTTP request failed: {error}"))?,
            _ = &mut cancel_wait => {
                return Err("provider request cancelled before response headers".to_string());
            }
        }
    } else {
        send.await
            .map_err(|error| format!("provider HTTP request failed: {error}"))?
    };
    let status = response.status();
    let headers = response.headers().clone();
    Ok(CancelableResponse {
        status,
        headers,
        response: ResponseBody::Reqwest(response),
    })
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn streaming_requests_disable_content_coding() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback bind");
        let address = listener.local_addr().expect("loopback address");
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("request connection");
            let mut buffer = vec![0u8; 8192];
            let mut used = 0usize;
            loop {
                let read = socket
                    .read(&mut buffer[used..])
                    .await
                    .expect("request read");
                assert!(read > 0, "request closed before headers");
                used += read;
                if buffer[..used]
                    .windows(4)
                    .any(|window| window == b"\r\n\r\n")
                {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&buffer[..used]).to_ascii_lowercase();
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await
                .expect("response write");
            request
        });
        let response = post(
            &format!("http://{address}/chat/completions"),
            "test-only-secret",
            b"{}".to_vec(),
            &[],
            None,
        )
        .await
        .expect("loopback request");
        drop(response);
        let request = server.await.expect("server task");
        assert!(
            request.contains("accept-encoding: identity\r\n"),
            "SSE transport must not decode a compressed long-lived body: {request}"
        );
    }

    #[tokio::test]
    async fn rejects_remote_plaintext_before_sending_authorization() {
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            post(
                "http://192.0.2.1:9",
                "test-only-secret",
                b"{}".to_vec(),
                &[],
                None,
            ),
        )
        .await
        .expect("plaintext rejection is immediate");
        let error = match result {
            Ok(_) => panic!("remote plaintext must fail before I/O"),
            Err(error) => error,
        };
        assert!(error.contains("remote providers require HTTPS"));
    }
}

#[allow(dead_code)]
fn _status(_: StatusCode, _: HeaderMap) {}

#[allow(dead_code)]
fn _response(_: CancelableResponse) {}

#[allow(dead_code)]
fn _client(_: reqwest::Client) {}

#[allow(dead_code)]
fn _url(_: reqwest::Url) {}

#[allow(dead_code)]
fn _body(_: Vec<u8>) {}

#[allow(dead_code)]
fn _headers(_: HeaderMap) {}

#[allow(dead_code)]
fn _status_code(_: StatusCode) {}

#[allow(dead_code)]
fn _host(_: &str) {}

#[allow(dead_code)]
fn _key(_: &str) {}

#[allow(dead_code)]
fn _attribution(_: &[(String, String)]) {}

#[allow(dead_code)]
fn _limit(_: usize) {}

#[allow(dead_code)]
fn _bytes(_: bytes::Bytes) {}

#[allow(dead_code)]
fn _result(_: Result<Option<bytes::Bytes>, String>) {}

#[allow(dead_code)]
fn _unit(_: ()) {}

#[allow(dead_code)]
fn _bool(_: bool) {}

#[allow(dead_code)]
fn _empty() {}
