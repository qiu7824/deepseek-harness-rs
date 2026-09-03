//! Anonymous, credential-free HTTP(S) retrieval restricted to public networks.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use dsh_web::{
    Cancelled, WebError, WebFetchBody, WebFetchProvider, WebFetchRequest, WebFetchResult,
};
use futures::StreamExt;
use reqwest::Client;
use reqwest::header::{ACCEPT, CONTENT_LENGTH, CONTENT_TYPE, LOCATION, USER_AGENT};
use url::{Host, Url};

pub const LOCAL_FETCH_PROVIDER_ID: &str = "http";

#[derive(Debug, Clone)]
pub struct HttpFetchLimits {
    pub max_response_bytes: usize,
    pub max_body_chars: usize,
    pub timeout_ms: u64,
    pub max_redirects: usize,
    pub user_agent: String,
}

impl Default for HttpFetchLimits {
    fn default() -> Self {
        Self {
            max_response_bytes: 5_000_000,
            max_body_chars: 100_000,
            timeout_ms: 30_000,
            max_redirects: 5,
            user_agent: "DeepSeek-Harness-WebFetch/0.1".into(),
        }
    }
}

pub type AddressResolver = Arc<
    dyn Fn(&str, u16) -> futures::future::BoxFuture<'static, Result<Vec<SocketAddr>, WebError>>
        + Send
        + Sync,
>;

pub struct HttpFetchProvider {
    limits: HttpFetchLimits,
    resolver: AddressResolver,
}

impl HttpFetchProvider {
    pub fn new(limits: HttpFetchLimits) -> Self {
        Self::with_resolver(
            limits,
            Arc::new(|host, port| {
                let host = host.to_string();
                Box::pin(async move {
                    tokio::net::lookup_host((host.as_str(), port))
                        .await
                        .map(|addresses| addresses.collect())
                        .map_err(|error| {
                            WebError::new(
                                "WEB_PROVIDER_ERROR",
                                format!("DNS resolution failed: {error}"),
                            )
                        })
                })
            }),
        )
    }

    pub fn with_resolver(limits: HttpFetchLimits, resolver: AddressResolver) -> Self {
        Self { limits, resolver }
    }

    async fn resolve_public(
        &self,
        url: &Url,
        cancelled: &Cancelled,
    ) -> Result<Vec<SocketAddr>, WebError> {
        if cancelled() {
            return Err(WebError::new("WEB_ABORTED", "web fetch aborted"));
        }
        let host = url
            .host_str()
            .ok_or_else(|| WebError::new("WEB_INVALID_URL", "web fetch URL requires a host"))?;
        let port = url
            .port_or_known_default()
            .ok_or_else(|| WebError::new("WEB_INVALID_URL", "web fetch URL has no usable port"))?;
        let addresses = (self.resolver)(host, port).await?;
        if addresses.is_empty() {
            return Err(WebError::new(
                "WEB_PROVIDER_ERROR",
                "DNS returned no addresses",
            ));
        }
        let nat64_prefixes = if addresses
            .iter()
            .any(|address| matches!(address.ip(), IpAddr::V6(_)))
        {
            match (self.resolver)("ipv4only.arpa", 80).await {
                Ok(answers) => discover_nat64_prefixes(&answers),
                Err(_) => Vec::new(),
            }
        } else {
            Vec::new()
        };
        if let Some(blocked) = addresses
            .iter()
            .find(|address| !is_public_with_nat64(address.ip(), &nat64_prefixes))
        {
            return Err(WebError::new(
                "WEB_BLOCKED_ADDRESS",
                format!(
                    "web fetch destination resolved to blocked address {}",
                    blocked.ip()
                ),
            ));
        }
        Ok(addresses)
    }

    async fn request_once(
        &self,
        url: &Url,
        addresses: &[SocketAddr],
        cancelled: &Cancelled,
    ) -> Result<reqwest::Response, WebError> {
        let host = url.host_str().expect("validated host");
        let mut builder = Client::builder()
            // Anonymous public fetch must not inherit HTTP(S)_PROXY. A proxy
            // would perform its own DNS/connect after our public-address
            // validation and could therefore bypass the SSRF boundary.
            .no_proxy()
            // Keep the transport byte stream encoded. Automatic content
            // decoding can expand a small compressed response beyond the
            // configured memory cap before `read_body` can reject it.
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_millis(self.limits.timeout_ms));
        for address in addresses {
            builder = builder.resolve(host, *address);
        }
        let client = builder.build().map_err(|error| {
            WebError::new(
                "WEB_PROVIDER_ERROR",
                format!("web fetch client failed: {error}"),
            )
        })?;
        let request = client
            .get(url.clone())
            .header(USER_AGENT, &self.limits.user_agent)
            .header(
                ACCEPT,
                "text/html,application/xhtml+xml,text/*;q=0.9,application/json;q=0.8",
            )
            .send();
        tokio::pin!(request);
        loop {
            tokio::select! {
                result = &mut request => {
                    return result.map_err(|error| {
                        if error.is_timeout() {
                            WebError::new("WEB_FETCH_TIMEOUT", format!("web fetch timed out after {}ms", self.limits.timeout_ms))
                        } else {
                            WebError::new("WEB_PROVIDER_ERROR", format!("web fetch request failed: {error}"))
                        }
                    });
                }
                _ = tokio::time::sleep(Duration::from_millis(10)) => {
                    if cancelled() {
                        return Err(WebError::new("WEB_ABORTED", "web fetch aborted"));
                    }
                }
            }
        }
    }

    async fn read_body(
        &self,
        response: reqwest::Response,
        cancelled: &Cancelled,
    ) -> Result<Vec<u8>, WebError> {
        if let Some(length) = response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok())
            && length > self.limits.max_response_bytes
        {
            return Err(WebError::new(
                "WEB_FETCH_TOO_LARGE",
                format!(
                    "web response exceeds {} bytes",
                    self.limits.max_response_bytes
                ),
            ));
        }
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        loop {
            let next = tokio::select! {
                chunk = stream.next() => chunk,
                _ = tokio::time::sleep(Duration::from_millis(10)) => {
                    if cancelled() { return Err(WebError::new("WEB_ABORTED", "web fetch aborted")); }
                    continue;
                }
            };
            let Some(chunk) = next else { break };
            let chunk = chunk.map_err(|error| {
                WebError::new(
                    "WEB_PROVIDER_ERROR",
                    format!("web response body failed: {error}"),
                )
            })?;
            if bytes.len().saturating_add(chunk.len()) > self.limits.max_response_bytes {
                return Err(WebError::new(
                    "WEB_FETCH_TOO_LARGE",
                    format!(
                        "web response exceeds {} bytes",
                        self.limits.max_response_bytes
                    ),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    async fn fetch_inner(
        &self,
        request: WebFetchRequest,
        cancelled: Cancelled,
    ) -> Result<WebFetchResult, WebError> {
        let mut url = validate_fetch_url(&request.url)?;
        for hop in 0..=self.limits.max_redirects {
            let addresses = self.resolve_public(&url, &cancelled).await?;
            let response = self.request_once(&url, &addresses, &cancelled).await?;
            let status = response.status();
            if status.is_redirection() {
                if hop == self.limits.max_redirects {
                    return Err(WebError::new(
                        "WEB_REDIRECT_LIMIT",
                        "web fetch redirect limit exceeded",
                    ));
                }
                let location = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| {
                        WebError::new(
                            "WEB_REDIRECT_INVALID",
                            "redirect omitted a valid Location header",
                        )
                    })?;
                url = validate_fetch_url(
                    url.join(location)
                        .map_err(|error| {
                            WebError::new(
                                "WEB_REDIRECT_INVALID",
                                format!("invalid redirect: {error}"),
                            )
                        })?
                        .as_str(),
                )?;
                continue;
            }
            let content_type = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("text/plain")
                .to_ascii_lowercase();
            let kind = classify_content_type(&content_type)?;
            let bytes = self.read_body(response, &cancelled).await?;
            let mut content = String::from_utf8_lossy(&bytes).into_owned();
            let truncated = content.chars().count() > self.limits.max_body_chars;
            if truncated {
                content = content.chars().take(self.limits.max_body_chars).collect();
            }
            let body = match kind {
                ContentKind::Html => WebFetchBody::Html { content },
                ContentKind::Text => WebFetchBody::Text { content },
            };
            return Ok(WebFetchResult {
                url: url.to_string(),
                status_code: status.as_u16(),
                body,
                truncated,
            });
        }
        unreachable!("redirect loop is bounded")
    }
}

#[async_trait]
impl WebFetchProvider for HttpFetchProvider {
    fn id(&self) -> &str {
        LOCAL_FETCH_PROVIDER_ID
    }

    fn available(&self) -> bool {
        true
    }

    async fn fetch(
        &self,
        request: WebFetchRequest,
        cancelled: Cancelled,
    ) -> Result<WebFetchResult, WebError> {
        self.fetch_inner(request, cancelled).await
    }
}

#[derive(Debug, Clone, Copy)]
enum ContentKind {
    Html,
    Text,
}

fn classify_content_type(value: &str) -> Result<ContentKind, WebError> {
    let mime = value.split(';').next().unwrap_or("").trim();
    if matches!(mime, "text/html" | "application/xhtml+xml") {
        return Ok(ContentKind::Html);
    }
    if mime.starts_with("text/")
        || matches!(
            mime,
            "application/json"
                | "application/ld+json"
                | "application/xml"
                | "application/javascript"
                | "application/x-javascript"
                | "application/sql"
        )
        || mime.ends_with("+json")
        || mime.ends_with("+xml")
    {
        return Ok(ContentKind::Text);
    }
    Err(WebError::new(
        "WEB_UNSUPPORTED_CONTENT_TYPE",
        format!("unsupported web content type {mime:?}"),
    ))
}

pub fn validate_fetch_url(raw: &str) -> Result<Url, WebError> {
    let url = Url::parse(raw).map_err(|error| {
        WebError::new("WEB_INVALID_URL", format!("invalid web fetch URL: {error}"))
    })?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host().is_none()
    {
        return Err(WebError::new(
            "WEB_INVALID_URL",
            "web fetch accepts anonymous HTTP(S) URLs only",
        ));
    }
    if let Some(host) = url.host()
        && match host {
            Host::Ipv4(ip) => !is_public_ip_address(IpAddr::V4(ip)),
            Host::Ipv6(ip) => !is_public_ip_address(IpAddr::V6(ip)),
            Host::Domain(domain) => is_metadata_hostname(domain),
        }
    {
        return Err(WebError::new(
            "WEB_INVALID_URL",
            "web fetch URL targets a blocked address",
        ));
    }
    Ok(url)
}

fn is_metadata_hostname(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    matches!(
        host.as_str(),
        "localhost"
            | "localhost.localdomain"
            | "metadata"
            | "metadata.google.internal"
            | "instance-data"
            | "instance-data.ec2.internal"
    ) || host.ends_with(".localhost")
}

pub fn is_public_ip_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_ipv4(ip),
        IpAddr::V6(ip) => is_public_ipv6(ip),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Nat64Prefix {
    bytes: [u8; 16],
    length: u8,
}

fn extract_rfc6052_ipv4(ip: Ipv6Addr, prefix_length: u8) -> Option<Ipv4Addr> {
    let bytes = ip.octets();
    if prefix_length <= 64 && bytes[8] != 0 {
        return None;
    }
    let embedded = match prefix_length {
        32 => [bytes[4], bytes[5], bytes[6], bytes[7]],
        40 => [bytes[5], bytes[6], bytes[7], bytes[9]],
        48 => [bytes[6], bytes[7], bytes[9], bytes[10]],
        56 => [bytes[7], bytes[9], bytes[10], bytes[11]],
        64 => [bytes[9], bytes[10], bytes[11], bytes[12]],
        96 => [bytes[12], bytes[13], bytes[14], bytes[15]],
        _ => return None,
    };
    Some(Ipv4Addr::from(embedded))
}

fn discover_nat64_prefixes(addresses: &[SocketAddr]) -> Vec<Nat64Prefix> {
    let mut prefixes = Vec::new();
    for address in addresses {
        let IpAddr::V6(ip) = address.ip() else {
            continue;
        };
        for length in [32_u8, 40, 48, 56, 64, 96] {
            let Some(embedded) = extract_rfc6052_ipv4(ip, length) else {
                continue;
            };
            if !matches!(embedded.octets(), [192, 0, 0, 170 | 171]) {
                continue;
            }
            let mut bytes = [0_u8; 16];
            let prefix_bytes = usize::from(length / 8);
            bytes[..prefix_bytes].copy_from_slice(&ip.octets()[..prefix_bytes]);
            let prefix = Nat64Prefix { bytes, length };
            if !prefixes.contains(&prefix) {
                prefixes.push(prefix);
            }
        }
    }
    prefixes
}

fn matches_nat64_prefix(ip: Ipv6Addr, prefix: &Nat64Prefix) -> bool {
    let prefix_bytes = usize::from(prefix.length / 8);
    ip.octets()[..prefix_bytes] == prefix.bytes[..prefix_bytes]
}

fn is_public_with_nat64(ip: IpAddr, prefixes: &[Nat64Prefix]) -> bool {
    if !is_public_ip_address(ip) {
        return false;
    }
    let IpAddr::V6(ip) = ip else {
        return true;
    };
    !prefixes.iter().any(|prefix| {
        matches_nat64_prefix(ip, prefix)
            && extract_rfc6052_ipv4(ip, prefix.length)
                .is_some_and(|embedded| !is_public_ipv4(embedded))
    })
}

fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _d] = ip.octets();
    !(a == 0
        || a == 10
        || a == 127
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2)
        || (a == 198 && b == 18 || a == 198 && b == 19)
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224)
}

fn is_sensitive_embedded_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, _c, _d] = ip.octets();
    a == 10
        || a == 127
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 100 && (64..=127).contains(&b))
}

fn is_public_ipv6(ip: Ipv6Addr) -> bool {
    // IPv4-compatible and IPv4-mapped literals must inherit the embedded
    // IPv4 policy; accepting only mapped addresses leaves ::10.0.0.1 open.
    if let Some(v4) = ip.to_ipv4() {
        return is_public_ipv4(v4);
    }
    let segments = ip.segments();
    // Only IANA's 2000::/3 global-unicast space is eligible. Then remove
    // special-purpose allocations inside that aggregate.
    if segments[0] & 0xe000 != 0x2000 {
        return false;
    }
    let ietf_special = segments[0] == 0x2001 && segments[1] <= 0x01ff;
    let documentation_2001 = segments[0] == 0x2001 && segments[1] == 0x0db8;
    let documentation_3fff = segments[0] == 0x3fff && segments[1] & 0xf000 == 0;
    if ietf_special || documentation_2001 || documentation_3fff {
        return false;
    }
    if [32_u8, 40, 48, 56, 64, 96]
        .into_iter()
        .filter_map(|length| extract_rfc6052_ipv4(ip, length))
        .any(is_sensitive_embedded_ipv4)
    {
        return false;
    }
    // Translation/transition prefixes can ultimately target an IPv4 address
    // different from the IPv6 socket we pinned. Block them unless a future
    // resolver validates the translated destination explicitly.
    let nat64_well_known = segments[0] == 0x0064
        && segments[1] == 0xff9b
        && segments[2..6].iter().all(|segment| *segment == 0);
    let nat64_local = segments[0] == 0x0064 && segments[1] == 0xff9b && segments[2] == 1;
    let six_to_four = segments[0] == 0x2002;
    let isatap = matches!(segments[4], 0x0000 | 0x0200) && segments[5] == 0x5efe;
    !(nat64_well_known || nat64_local || six_to_four || isatap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn rejects_non_public_ip_ranges() {
        for ip in [
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            "10.0.0.1".parse().unwrap(),
            "172.16.0.1".parse().unwrap(),
            "192.168.0.1".parse().unwrap(),
            "169.254.169.254".parse().unwrap(),
            "100.64.0.1".parse().unwrap(),
            "224.0.0.1".parse().unwrap(),
            "0.0.0.0".parse().unwrap(),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            "fe80::1".parse().unwrap(),
            "fc00::1".parse().unwrap(),
            "ff02::1".parse().unwrap(),
            "::".parse().unwrap(),
            "::192.168.0.1".parse().unwrap(),
            "64:ff9b::c0a8:1".parse().unwrap(),
            "64:ff9b:1::c0a8:1".parse().unwrap(),
            "100::1".parse().unwrap(),
            "2001::1".parse().unwrap(),
            "2001:2::1".parse().unwrap(),
            "2001:20::1".parse().unwrap(),
            "2002:c0a8:1::1".parse().unwrap(),
            "2606:4700:4700::5efe:a00:1".parse().unwrap(),
            "2600:abcd:64::a9fe:a9fe".parse().unwrap(),
            "fec0::1".parse().unwrap(),
            "3fff::1".parse().unwrap(),
            "4000::1".parse().unwrap(),
        ] {
            assert!(!is_public_ip_address(ip), "{ip} must be blocked");
        }
        assert!(is_public_ip_address("1.1.1.1".parse().unwrap()));
        assert!(is_public_ip_address(
            "2606:4700:4700::1111".parse().unwrap()
        ));
    }

    #[test]
    fn blocks_private_ipv4_embedded_in_discovered_network_nat64_prefix() {
        let answers = [
            "[2600:abcd:64::c000:aa]:80".parse().unwrap(),
            "[2600:abcd:64::c000:ab]:80".parse().unwrap(),
        ];
        let prefixes = discover_nat64_prefixes(&answers);
        assert_eq!(prefixes.len(), 1);
        assert!(!is_public_with_nat64(
            "2600:abcd:64::a9fe:a9fe".parse().unwrap(),
            &prefixes,
        ));
        assert!(is_public_with_nat64(
            "2600:abcd:64::808:808".parse().unwrap(),
            &prefixes,
        ));
    }

    #[test]
    fn validates_only_anonymous_http_urls() {
        for raw in [
            "ftp://example.com/file",
            "file:///etc/passwd",
            "http://user:pass@example.com/",
            "http://127.0.0.1/",
            "http://[::1]/",
            "http://169.254.169.254/latest/meta-data/",
        ] {
            let error = validate_fetch_url(raw).unwrap_err();
            assert_eq!(error.code(), "WEB_INVALID_URL", "{raw}");
        }
        assert_eq!(
            validate_fetch_url("https://example.com/path")
                .unwrap()
                .as_str(),
            "https://example.com/path"
        );
    }

    #[tokio::test]
    async fn cancellation_preempts_a_pending_fetch() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancelled);
        let provider = HttpFetchProvider::new(HttpFetchLimits {
            timeout_ms: 30_000,
            ..Default::default()
        });
        let task = tokio::spawn(async move {
            provider
                .fetch(
                    WebFetchRequest {
                        url: "https://1.1.1.1:81/".into(),
                    },
                    Arc::new(move || flag.load(Ordering::SeqCst)),
                )
                .await
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancelled.store(true, Ordering::SeqCst);
        let error = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("cancelled fetch should settle")
            .unwrap()
            .unwrap_err();
        assert_eq!(error.code(), "WEB_ABORTED");
    }

    #[tokio::test]
    async fn blocks_a_redirect_that_resolves_private_before_second_request() {
        let resolver: AddressResolver = Arc::new(move |host, requested_port| {
            let host = host.to_string();
            Box::pin(async move {
                let ip = if host == "public.test" {
                    "93.184.216.34".parse().unwrap()
                } else {
                    "127.0.0.1".parse().unwrap()
                };
                Ok(vec![SocketAddr::new(ip, requested_port)])
            })
        });
        let provider = HttpFetchProvider::with_resolver(HttpFetchLimits::default(), resolver);
        let redirected = validate_fetch_url("http://private.test/secret").unwrap();
        let cancelled: Cancelled = Arc::new(|| false);
        let error = provider
            .resolve_public(&redirected, &cancelled)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "WEB_BLOCKED_ADDRESS");
    }

    #[test]
    fn request_client_disables_ambient_proxies_before_using_pinned_dns() {
        let source = include_str!("lib.rs");
        let no_proxy = source
            .find(".no_proxy()")
            .expect("web fetch client must disable ambient proxies");
        let pinned_dns = source
            .find("builder = builder.resolve(host, *address)")
            .expect("web fetch client must pin validated addresses");
        assert!(no_proxy < pinned_dns);
    }

    #[test]
    fn compressed_decoding_is_disabled_so_the_byte_cap_applies_on_the_wire() {
        let source = include_str!("lib.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("production source");
        for marker in [".no_gzip()", ".no_brotli()", ".no_deflate()", ".no_zstd()"] {
            assert!(source.contains(marker), "missing {marker}");
        }
    }

    #[test]
    fn classifies_readable_and_binary_content_types() {
        assert!(matches!(
            classify_content_type("text/html; charset=utf-8"),
            Ok(ContentKind::Html)
        ));
        assert!(matches!(
            classify_content_type("application/json"),
            Ok(ContentKind::Text)
        ));
        assert_eq!(
            classify_content_type("image/png").unwrap_err().code(),
            "WEB_UNSUPPORTED_CONTENT_TYPE"
        );
    }
}
