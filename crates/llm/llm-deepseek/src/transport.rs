use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{HeaderMap, Request, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

trait TransportIo: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T> TransportIo for T where T: AsyncRead + AsyncWrite + Unpin + Send {}

struct ConnectionGuard(tokio::task::JoinHandle<()>);

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(crate) struct CancelableResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    body: hyper::body::Incoming,
    _connection: ConnectionGuard,
}

impl CancelableResponse {
    pub async fn next_data(&mut self) -> Result<Option<Bytes>, String> {
        loop {
            let Some(frame) = self.body.frame().await else {
                return Ok(None);
            };
            let frame = frame.map_err(|error| format!("HTTP response body failed: {error}"))?;
            if let Ok(data) = frame.into_data() {
                return Ok(Some(data));
            }
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

pub(crate) async fn post(
    url: &str,
    api_key: &str,
    body: Vec<u8>,
    attribution: &[(String, String)],
) -> Result<CancelableResponse, String> {
    let url = reqwest::Url::parse(url).map_err(|error| format!("invalid provider URL: {error}"))?;
    let host = url
        .host_str()
        .ok_or_else(|| "provider URL has no host".to_string())?;
    match url.scheme() {
        "https" => {}
        "http" if is_loopback_host(host) => {}
        "http" => {
            return Err(
                "plain HTTP provider URLs are restricted to loopback; remote providers require HTTPS"
                    .to_string(),
            );
        }
        scheme => return Err(format!("unsupported provider URL scheme {scheme:?}")),
    }
    let port = url
        .port_or_known_default()
        .ok_or_else(|| "provider URL has no port".to_string())?;
    let proxy = resolve_proxy(url.scheme(), host)?;
    let (connect_host, connect_port) = proxy
        .as_ref()
        .map(|proxy| (proxy.host.as_str(), proxy.port))
        .unwrap_or((host, port));
    let mut tcp = TcpStream::connect((connect_host, connect_port))
        .await
        .map_err(|error| format!("provider TCP connect failed: {error}"))?;
    tcp.set_nodelay(true)
        .map_err(|error| format!("provider TCP setup failed: {error}"))?;
    if url.scheme() == "https" && proxy.is_some() {
        establish_connect_tunnel(&mut tcp, host, port).await?;
    }
    let io: Box<dyn TransportIo> = match url.scheme() {
        "http" => Box::new(tcp),
        "https" => Box::new(connect_tls(tcp, host).await?),
        scheme => return Err(format!("unsupported provider URL scheme {scheme:?}")),
    };
    let (mut sender, connection) =
        hyper::client::conn::http1::handshake::<_, Full<Bytes>>(TokioIo::new(io))
            .await
            .map_err(|error| format!("provider HTTP handshake failed: {error}"))?;
    let guard = ConnectionGuard(tokio::spawn(async move {
        let _ = connection.await;
    }));
    sender
        .ready()
        .await
        .map_err(|error| format!("provider HTTP connection failed: {error}"))?;
    let target = if url.scheme() == "http" && proxy.is_some() {
        url.as_str().to_string()
    } else {
        let mut target = url.path().to_string();
        if let Some(query) = url.query() {
            target.push('?');
            target.push_str(query);
        }
        target
    };
    let mut request = Request::builder()
        .method("POST")
        .uri(target)
        .header("host", authority(host, port, url.scheme()))
        .header("authorization", format!("Bearer {api_key}"))
        .header("accept", "text/event-stream")
        .header("content-type", "application/json")
        .header("connection", "close");
    for (name, value) in attribution {
        request = request.header(name, value);
    }
    let response = sender
        .send_request(
            request
                .body(Full::new(Bytes::from(body)))
                .map_err(|error| error.to_string())?,
        )
        .await
        .map_err(|error| format!("provider HTTP request failed: {error}"))?;
    let (parts, body) = response.into_parts();
    Ok(CancelableResponse {
        status: parts.status,
        headers: parts.headers,
        body,
        _connection: guard,
    })
}

struct ProxyEndpoint {
    host: String,
    port: u16,
}

fn resolve_proxy(scheme: &str, host: &str) -> Result<Option<ProxyEndpoint>, String> {
    if host.eq_ignore_ascii_case("localhost")
        || host == "127.0.0.1"
        || host == "::1"
        || bypassed_by_no_proxy(host)
    {
        return Ok(None);
    }
    let names: &[&str] = if scheme == "https" {
        &["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"]
    } else {
        &["HTTP_PROXY", "http_proxy"]
    };
    let Some(raw) = names.iter().find_map(|name| std::env::var(name).ok()) else {
        return Ok(None);
    };
    let url = reqwest::Url::parse(&raw).map_err(|error| format!("invalid proxy URL: {error}"))?;
    if url.scheme() != "http" {
        return Err("only http:// provider proxies are supported".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("authenticated provider proxies are not supported".to_string());
    }
    Ok(Some(ProxyEndpoint {
        host: url
            .host_str()
            .ok_or_else(|| "proxy URL has no host".to_string())?
            .to_string(),
        port: url.port_or_known_default().unwrap_or(80),
    }))
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn bypassed_by_no_proxy(host: &str) -> bool {
    let raw = std::env::var("NO_PROXY")
        .or_else(|_| std::env::var("no_proxy"))
        .unwrap_or_default();
    raw.split(',').any(|entry| {
        let entry = entry.trim();
        if entry == "*" {
            return true;
        }
        let entry = entry
            .split_once(':')
            .map(|(name, _)| name)
            .unwrap_or(entry)
            .trim_start_matches('.');
        !entry.is_empty()
            && (host.eq_ignore_ascii_case(entry)
                || host
                    .to_ascii_lowercase()
                    .ends_with(&format!(".{}", entry.to_ascii_lowercase())))
    })
}

async fn establish_connect_tunnel(
    stream: &mut TcpStream,
    host: &str,
    port: u16,
) -> Result<(), String> {
    let authority = authority(host, port, "https");
    stream
        .write_all(
            format!(
                "CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\nProxy-Connection: Keep-Alive\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .map_err(|error| format!("proxy CONNECT write failed: {error}"))?;
    stream
        .flush()
        .await
        .map_err(|error| format!("proxy CONNECT flush failed: {error}"))?;
    let mut response = Vec::new();
    while !response.windows(4).any(|window| window == b"\r\n\r\n") {
        if response.len() >= 64 * 1024 {
            return Err("proxy CONNECT response headers exceeded 64 KiB".to_string());
        }
        let mut buffer = [0_u8; 1024];
        let read = stream
            .read(&mut buffer)
            .await
            .map_err(|error| format!("proxy CONNECT read failed: {error}"))?;
        if read == 0 {
            return Err("proxy closed during CONNECT".to_string());
        }
        response.extend_from_slice(&buffer[..read]);
    }
    let head = std::str::from_utf8(&response)
        .map_err(|_| "proxy CONNECT response was not UTF-8".to_string())?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or_default();
    if status != "200" {
        return Err(format!("proxy CONNECT failed with status {status}"));
    }
    Ok(())
}

async fn connect_tls(
    stream: TcpStream,
    host: &str,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>, String> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let roots = rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let server_name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|error| format!("invalid TLS server name: {error}"))?;
    tokio_rustls::TlsConnector::from(Arc::new(config))
        .connect(server_name, stream)
        .await
        .map_err(|error| format!("provider TLS handshake failed: {error}"))
}

fn authority(host: &str, port: u16, scheme: &str) -> String {
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    let default = (scheme == "http" && port == 80) || (scheme == "https" && port == 443);
    if default {
        host
    } else {
        format!("{host}:{port}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_remote_plaintext_before_sending_authorization() {
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            post(
                "http://192.0.2.1:9/chat/completions",
                "test-only-secret",
                b"{}".to_vec(),
                &[],
            ),
        )
        .await
        .expect("remote plaintext must reject before network I/O");
        let failure = match result {
            Ok(_) => panic!("remote plaintext provider must fail closed"),
            Err(failure) => failure,
        };
        assert!(failure.contains("HTTPS"), "{failure}");
    }
}
