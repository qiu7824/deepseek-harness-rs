//! One outbound policy for Host-managed HTTP clients. CDP remains direct.

use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Clone)]
pub struct ProxyPolicy {
    http: Option<String>,
    https: Option<String>,
    bypass: String,
}

fn selected(values: &HashMap<String, String>, name: &str) -> Option<String> {
    values
        .get(&name.to_ascii_lowercase())
        .or_else(|| values.get(name))
        .cloned()
}

impl ProxyPolicy {
    pub fn from_values(values: &HashMap<String, String>) -> Result<Self, String> {
        let fallback = selected(values, "ALL_PROXY");
        let parse = |name: &str| -> Result<Option<String>, String> {
            let raw = selected(values, name).or_else(|| fallback.clone());
            let Some(raw) = raw.filter(|value| !value.trim().is_empty()) else {
                return Ok(None);
            };
            let url = reqwest::Url::parse(raw.trim())
                .map_err(|_| format!("{name} must contain a valid HTTP(S) proxy URL"))?;
            if !matches!(url.scheme(), "http" | "https")
                || url.host_str().is_none()
                || url.query().is_some()
                || url.fragment().is_some()
                || !matches!(url.path(), "" | "/")
            {
                return Err(format!(
                    "{name} requires an HTTP(S) proxy origin; SOCKS and URL paths are unsupported"
                ));
            }
            Ok(Some(url.to_string()))
        };
        let bypass = selected(values, "NO_PROXY").unwrap_or_default();
        Ok(Self {
            http: parse("HTTP_PROXY")?,
            https: parse("HTTPS_PROXY")?,
            bypass: format!("localhost,.localhost,127.0.0.0/8,::1,{bypass}"),
        })
    }

    pub fn configure(
        &self,
        mut builder: reqwest::ClientBuilder,
    ) -> Result<reqwest::ClientBuilder, String> {
        // Never inherit a second implicit/OS policy after resolving the launch policy.
        builder = builder.no_proxy();
        let no_proxy = reqwest::NoProxy::from_string(&self.bypass);
        if let Some(url) = &self.http {
            builder = builder.proxy(
                reqwest::Proxy::http(url)
                    .map_err(|_| "invalid HTTP proxy")?
                    .no_proxy(no_proxy.clone()),
            );
        }
        if let Some(url) = &self.https {
            builder = builder.proxy(
                reqwest::Proxy::https(url)
                    .map_err(|_| "invalid HTTPS proxy")?
                    .no_proxy(no_proxy),
            );
        }
        Ok(builder)
    }

    pub fn configure_blocking(
        &self,
        mut builder: reqwest::blocking::ClientBuilder,
    ) -> Result<reqwest::blocking::ClientBuilder, String> {
        builder = builder.no_proxy();
        let no_proxy = reqwest::NoProxy::from_string(&self.bypass);
        if let Some(url) = &self.http {
            builder = builder.proxy(
                reqwest::Proxy::http(url)
                    .map_err(|_| "invalid HTTP proxy")?
                    .no_proxy(no_proxy.clone()),
            );
        }
        if let Some(url) = &self.https {
            builder = builder.proxy(
                reqwest::Proxy::https(url)
                    .map_err(|_| "invalid HTTPS proxy")?
                    .no_proxy(no_proxy),
            );
        }
        Ok(builder)
    }
}

fn env_file(path: &std::path::Path) -> HashMap<String, String> {
    let Ok(metadata) = std::fs::metadata(path) else {
        return HashMap::new();
    };
    if metadata.len() > 1024 * 1024 {
        return HashMap::new();
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    text.lines()
        .filter_map(|line| {
            let line = line.trim().strip_prefix("export ").unwrap_or(line.trim());
            let (key, value) = line.split_once('=')?;
            let key = key.trim();
            if !["http_proxy", "https_proxy", "all_proxy", "no_proxy"]
                .contains(&key.to_ascii_lowercase().as_str())
            {
                return None;
            }
            let value = value.trim();
            let value = if value.len() >= 2
                && ((value.starts_with('"') && value.ends_with('"'))
                    || (value.starts_with('\'') && value.ends_with('\'')))
            {
                &value[1..value.len() - 1]
            } else {
                value.split(" #").next().unwrap_or(value).trim_end()
            };
            Some((key.to_owned(), value.to_owned()))
        })
        .collect()
}

fn launch_policy(configured_home: Option<&std::path::Path>) -> Result<ProxyPolicy, String> {
    let env = |name: &str| std::env::var(name).ok();
    let home = configured_home
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| dsh_home_paths::resolve_dsh_home(None, &env));
    let mut values = HashMap::new();
    let mut layers = vec![env_file(&home.join(".env"))];
    if let Ok(cwd) = std::env::current_dir() {
        layers.push(env_file(&cwd.join(".env")));
    }
    for layer in layers {
        for name in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY"] {
            if let Some(value) = selected(&layer, name) {
                values.insert(name.to_ascii_lowercase(), value);
            }
        }
    }
    // A process value, including an empty value, overrides file values in both cases.
    for name in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY"] {
        let lower = name.to_ascii_lowercase();
        if let Some(value) = env(&lower).or_else(|| env(name)) {
            values.remove(name);
            values.insert(lower, value);
        }
    }
    ProxyPolicy::from_values(&values)
}

static POLICY: OnceLock<Result<ProxyPolicy, String>> = OnceLock::new();
pub fn initialize(home: Option<&std::path::Path>) -> Result<&'static ProxyPolicy, String> {
    POLICY
        .get_or_init(|| launch_policy(home))
        .as_ref()
        .map_err(Clone::clone)
}
pub fn policy() -> Result<&'static ProxyPolicy, String> {
    initialize(None)
}
pub fn blocking_builder() -> Result<reqwest::blocking::ClientBuilder, String> {
    policy()?.configure_blocking(reqwest::blocking::Client::builder())
}

pub fn builder() -> Result<reqwest::ClientBuilder, String> {
    policy()?.configure(reqwest::Client::builder())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    #[test]
    fn precedence_empty_values_and_errors_never_expose_credentials() {
        let map = HashMap::from([
            ("HTTPS_PROXY".into(), "http://upper:80".into()),
            ("https_proxy".into(), "".into()),
            ("ALL_PROXY".into(), "http://all:80".into()),
        ]);
        let policy = ProxyPolicy::from_values(&map).unwrap();
        assert!(policy.https.is_none());
        assert!(policy.http.is_some());
        for url in [
            "socks5://user:secret@proxy:1080",
            "http://user:secret@proxy/path",
        ] {
            let error =
                ProxyPolicy::from_values(&HashMap::from([("ALL_PROXY".into(), url.into())]))
                    .err()
                    .unwrap();
            assert!(!error.contains("secret"));
        }
    }
    #[tokio::test]
    async fn actual_http_uses_proxy_and_loopback_always_bypasses_it() {
        let proxy = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let policy = ProxyPolicy::from_values(&HashMap::from([(
            "HTTP_PROXY".into(),
            format!("http://{}", proxy.local_addr().unwrap()),
        )]))
        .unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = proxy.accept().await.unwrap();
            let mut bytes = [0; 4096];
            let n = stream.read(&mut bytes).await.unwrap();
            assert!(
                String::from_utf8_lossy(&bytes[..n])
                    .starts_with("GET http://proxy-fixture.invalid/check ")
            );
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await
                .unwrap();
        });
        let client = policy
            .configure(reqwest::Client::builder())
            .unwrap()
            .timeout(std::time::Duration::from_secs(3))
            .build()
            .unwrap();
        assert_eq!(
            client
                .get("http://proxy-fixture.invalid/check")
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap(),
            "ok"
        );
        server.await.unwrap();
        let direct = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", direct.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut stream, _) = direct.accept().await.unwrap();
            let mut bytes = [0; 1024];
            stream.read(&mut bytes).await.unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await
                .unwrap();
        });
        assert_eq!(
            client.get(url).send().await.unwrap().text().await.unwrap(),
            "ok"
        );
        server.await.unwrap();
    }
}
