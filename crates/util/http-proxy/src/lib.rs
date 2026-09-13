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
    apply_system_fallback(&mut values, system_proxy_values());
    ProxyPolicy::from_values(&values)
}

fn apply_system_fallback(values: &mut HashMap<String, String>, system: HashMap<String, String>) {
    if ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"]
        .iter()
        .any(|key| selected(values, key).is_some())
    {
        return;
    }
    for (key, value) in system {
        values.entry(key).or_insert(value);
    }
}
fn parse_system_proxy(server: &str, bypass: &str) -> HashMap<String, String> {
    let mut values = HashMap::new();
    let origin = |v: &str| {
        if v.contains("://") {
            v.to_string()
        } else {
            format!("http://{v}")
        }
    };
    if server.contains('=') {
        for pair in server.split(';') {
            if let Some((scheme, address)) = pair.split_once('=') {
                if ["http", "https"].contains(&scheme.trim()) && !address.trim().is_empty() {
                    values.insert(format!("{}_proxy", scheme.trim()), origin(address.trim()));
                }
            }
        }
    } else if !server.trim().is_empty() {
        values.insert("all_proxy".into(), origin(server.trim()));
    }
    if !bypass.is_empty() {
        values.insert(
            "no_proxy".into(),
            bypass.replace(';', ",").replace("<local>", "localhost"),
        );
    }
    values
}
#[cfg(not(windows))]
fn system_proxy_values() -> HashMap<String, String> {
    HashMap::new()
}
#[cfg(windows)]
fn system_proxy_values() -> HashMap<String, String> {
    use windows_sys::Win32::System::Registry::*;
    fn read(name: &str, flags: u32) -> Option<Vec<u8>> {
        let sub: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings\0"
            .encode_utf16()
            .collect();
        let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let mut bytes = vec![0u8; 16384];
        let mut len = bytes.len() as u32;
        let result = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                sub.as_ptr(),
                name.as_ptr(),
                flags,
                std::ptr::null_mut(),
                bytes.as_mut_ptr().cast(),
                &mut len,
            )
        };
        if result != 0 {
            return None;
        }
        bytes.truncate(len as usize);
        Some(bytes)
    }
    if read("ProxyEnable", RRF_RT_REG_DWORD).and_then(|b| {
        b.get(..4)
            .map(|v| u32::from_le_bytes(v.try_into().unwrap()))
    }) != Some(1)
    {
        return HashMap::new();
    }
    let string = |name| {
        read(name, RRF_RT_REG_SZ)
            .map(|b| {
                String::from_utf16_lossy(
                    &b.chunks_exact(2)
                        .map(|v| u16::from_le_bytes([v[0], v[1]]))
                        .take_while(|v| *v != 0)
                        .collect::<Vec<_>>(),
                )
            })
            .unwrap_or_default()
    };
    parse_system_proxy(&string("ProxyServer"), &string("ProxyOverride"))
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
    #[test]
    fn system_proxy_is_only_a_fallback() {
        let system = super::parse_system_proxy(
            "http=127.0.0.1:8080;https=127.0.0.1:8081",
            "localhost;*.local",
        );
        let mut values = std::collections::HashMap::new();
        super::apply_system_fallback(&mut values, system.clone());
        assert_eq!(values["https_proxy"], "http://127.0.0.1:8081");
        let mut explicit = std::collections::HashMap::from([("all_proxy".into(), String::new())]);
        super::apply_system_fallback(&mut explicit, system);
        assert_eq!(explicit.len(), 1);
    }
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
