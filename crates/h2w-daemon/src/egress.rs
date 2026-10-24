//! `h2workbench egress-proxy`: the allowlisting forward proxy container jobs reach the network
//! through (ARCHITECTURE.md §9).
//!
//! - `CONNECT host:port` tunnels and absolute-form `http://` requests; nothing else.
//! - Hostname allowlist: exact names or `*.suffix` (subdomains only), each on its own
//!   ports (443 and 80 unless the entry lists others). IP literals never match.
//! - Resolve, check every address, then connect only to addresses that were checked (no
//!   second lookup an attacker could race), trying the next one when one is unreachable.
//! - Loopback, private, link-local (cloud metadata), CGNAT and unique-local addresses are
//!   refused unless the matching entry has `allow_private`; unspecified, multicast,
//!   broadcast, documentation and other reserved addresses are always refused.
//! - Every decision is logged (client, host, port, decision, reason), never a body.
//! - Header-read, DNS, connect and idle timeouts; a cap on concurrent connections.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http::{HeaderValue, Method, Request, Response, StatusCode, Uri, header};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::Incoming;
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, watch};

use crate::config::{AllowRule, canonical_ip};

type ProxyBody = BoxBody<Bytes, Box<dyn std::error::Error + Send + Sync>>;

/// Limits of the proxy.
#[derive(Debug, Clone)]
pub struct ProxySettings {
    pub rules: Vec<AllowRule>,
    pub max_connections: usize,
    pub header_timeout: Duration,
    pub dns_timeout: Duration,
    pub connect_timeout: Duration,
    pub idle_timeout: Duration,
}

impl ProxySettings {
    #[must_use]
    pub fn new(rules: Vec<AllowRule>) -> ProxySettings {
        ProxySettings {
            rules,
            max_connections: 512,
            header_timeout: Duration::from_secs(10),
            dns_timeout: Duration::from_secs(5),
            connect_timeout: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(300),
        }
    }
}

/// The address classes the proxy distinguishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpClass {
    Public,
    /// Reachable only with `allow_private` (the reason names the range).
    Private(&'static str),
    /// Never reachable.
    Forbidden(&'static str),
}

fn v4_class(ip: Ipv4Addr) -> IpClass {
    let o = ip.octets();
    let rfc1918 = o[0] == 10 || (o[0] == 172 && (16..=31).contains(&o[1])) || o[..2] == [192, 168];
    if rfc1918 {
        return IpClass::Private("private address");
    }
    match o {
        [0, ..] => IpClass::Forbidden("unspecified or this-network address"),
        [127, ..] => IpClass::Private("loopback address"),
        [169, 254, ..] => IpClass::Private("link-local or cloud metadata address"),
        [100, b, ..] if (64..=127).contains(&b) => IpClass::Private("CGNAT address"),
        [255, 255, 255, 255] => IpClass::Forbidden("broadcast address"),
        [a, ..] if a >= 224 => IpClass::Forbidden("multicast or reserved address"),
        [192, 0, 0, _] => IpClass::Forbidden("IETF protocol address"),
        [192, 0, 2, _] | [198, 51, 100, _] | [203, 0, 113, _] => {
            IpClass::Forbidden("documentation address")
        }
        [198, b, ..] if b == 18 || b == 19 => IpClass::Forbidden("benchmarking address"),
        _ => IpClass::Public,
    }
}

/// Classify an address (IPv4-mapped, NAT64 and 6to4 forms are judged by their IPv4).
#[must_use]
pub fn ip_class(ip: IpAddr) -> IpClass {
    match canonical_ip(ip) {
        IpAddr::V4(v4) => v4_class(v4),
        IpAddr::V6(v6) => v6_class(v6),
    }
}

fn v6_class(ip: Ipv6Addr) -> IpClass {
    let s = ip.segments();
    if ip.is_unspecified() {
        return IpClass::Forbidden("unspecified address");
    }
    if ip.is_loopback() {
        return IpClass::Private("loopback address");
    }
    // NAT64 (64:ff9b::/96) embeds an IPv4 address in the low 32 bits.
    let o = ip.octets();
    if s[0] == 0x64 && s[1] == 0xff9b && s[2..6].iter().all(|x| *x == 0) {
        return v4_class(Ipv4Addr::new(o[12], o[13], o[14], o[15]));
    }
    // 6to4 (2002::/16) embeds an IPv4 address in bits 16..48.
    if s[0] == 0x2002 {
        return v4_class(Ipv4Addr::new(o[2], o[3], o[4], o[5]));
    }
    if s[0..6].iter().all(|x| *x == 0) {
        return IpClass::Forbidden("IPv4-compatible address");
    }
    match s[0] {
        x if x & 0xfe00 == 0xfc00 => IpClass::Private("unique local address"),
        x if x & 0xffc0 == 0xfe80 => IpClass::Private("link-local address"),
        x if x & 0xffc0 == 0xfec0 => IpClass::Private("site-local address"),
        x if x & 0xff00 == 0xff00 => IpClass::Forbidden("multicast address"),
        0x2001 if s[1] == 0x0db8 => IpClass::Forbidden("documentation address"),
        0x2001 if s[1] == 0 => IpClass::Forbidden("Teredo address"),
        0x0100 if s[1..4].iter().all(|x| *x == 0) => IpClass::Forbidden("discard address"),
        _ => IpClass::Public,
    }
}

/// `Ok` when `ip` may be contacted under a rule with `allow_private`.
///
/// # Errors
/// The reason it is refused.
pub fn check_ip(ip: IpAddr, allow_private: bool) -> Result<(), &'static str> {
    match ip_class(ip) {
        IpClass::Public => Ok(()),
        IpClass::Private(_) if allow_private => Ok(()),
        IpClass::Private(why) | IpClass::Forbidden(why) => Err(why),
    }
}

/// Lowercase, without a trailing dot; `None` for anything that is not a DNS name
/// (including IP literals).
#[must_use]
pub fn normalize_host(h: &str) -> Option<String> {
    let h = h.strip_suffix('.').unwrap_or(h).to_ascii_lowercase();
    if h.is_empty() || h.len() > 253 || h.parse::<IpAddr>().is_ok() {
        return None;
    }
    let labels_ok = h.split('.').all(|l| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    });
    labels_ok.then_some(h)
}

fn host_matches(r: &AllowRule, host: &str) -> bool {
    if r.wildcard {
        host.len() > r.host.len() + 1
            && host.ends_with(&r.host)
            && host.as_bytes()[host.len() - r.host.len() - 1] == b'.'
    } else {
        host == r.host
    }
}

/// The first rule matching `host` (already normalized).
#[must_use]
pub fn matching_rule<'a>(rules: &'a [AllowRule], host: &str) -> Option<&'a AllowRule> {
    rules.iter().find(|r| host_matches(r, host))
}

/// The first rule matching `host` that also allows `port`.
///
/// # Errors
/// Why no rule applies: the host is not listed, or not on that port.
pub fn rule_for<'a>(
    rules: &'a [AllowRule],
    host: &str,
    port: u16,
) -> Result<&'a AllowRule, &'static str> {
    let mut listed = false;
    for r in rules.iter().filter(|r| host_matches(r, host)) {
        if r.ports.contains(&port) {
            return Ok(r);
        }
        listed = true;
    }
    Err(if listed {
        "port not allowed for this host"
    } else {
        "not on the allowlist"
    })
}

/// Split `host:port` (port required, IPv6 literals bracketed).
fn authority_parts(a: &str) -> Option<(String, u16)> {
    let (h, p) = crate::config::split_host_port(a)?;
    Some((h, p?))
}

fn text_response(status: StatusCode, msg: &str) -> Response<ProxyBody> {
    let mut r = Response::new(
        Full::new(Bytes::from(format!("{msg}\n")))
            .map_err(|never| match never {})
            .boxed(),
    );
    *r.status_mut() = status;
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    // A refused client gets its answer and the connection ends.
    r.headers_mut()
        .insert(header::CONNECTION, HeaderValue::from_static("close"));
    r
}

/// Resolve `host`, check every address, then connect to the checked addresses in order
/// (the next one when one is unreachable, e.g. IPv6 without a route), all within
/// `connect_timeout`. Only checked addresses are ever dialled.
async fn resolve_and_connect(
    s: &ProxySettings,
    rule: &AllowRule,
    host: &str,
    port: u16,
) -> Result<TcpStream, (StatusCode, String)> {
    let mut addrs: Vec<SocketAddr> =
        match tokio::time::timeout(s.dns_timeout, tokio::net::lookup_host((host, port))).await {
            Ok(Ok(a)) => a.collect(),
            Ok(Err(e)) => return Err((StatusCode::BAD_GATEWAY, format!("cannot resolve: {e}"))),
            Err(_) => return Err((StatusCode::GATEWAY_TIMEOUT, "DNS timeout".to_owned())),
        };
    if addrs.is_empty() {
        return Err((StatusCode::BAD_GATEWAY, "no address".to_owned()));
    }
    for a in &addrs {
        if let Err(why) = check_ip(a.ip(), rule.allow_private) {
            return Err((StatusCode::FORBIDDEN, format!("resolves to a {why}")));
        }
    }
    let mut seen = std::collections::HashSet::new();
    addrs.retain(|a| seen.insert(*a));
    connect_any(addrs, s.connect_timeout).await
}

/// Connect to `addrs` in order, moving on when one fails, all within `timeout`.
async fn connect_any(
    addrs: Vec<SocketAddr>,
    timeout: Duration,
) -> Result<TcpStream, (StatusCode, String)> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut last = String::from("no address");
    for a in addrs {
        match tokio::time::timeout_at(deadline, TcpStream::connect(a)).await {
            Ok(Ok(stream)) => {
                let _ = stream.set_nodelay(true);
                return Ok(stream);
            }
            Ok(Err(e)) => last = format!("connect failed: {e}"),
            Err(_) => return Err((StatusCode::GATEWAY_TIMEOUT, "connect timeout".to_owned())),
        }
    }
    Err((StatusCode::BAD_GATEWAY, last))
}

async fn copy_half<R, W>(r: &mut R, w: &mut W, start: Instant, last: &AtomicU64)
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        match r.read(&mut buf).await {
            Ok(0) | Err(_) => {
                let _ = w.shutdown().await;
                return;
            }
            Ok(n) => {
                if w.write_all(&buf[..n]).await.is_err() {
                    return;
                }
                let ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
                last.store(ms, Ordering::Relaxed);
            }
        }
    }
}

/// Copy both ways until both sides close or nothing moves for `idle`.
pub async fn tunnel<A, B>(a: A, b: B, idle: Duration)
where
    A: AsyncRead + AsyncWrite + Unpin,
    B: AsyncRead + AsyncWrite + Unpin,
{
    let start = Instant::now();
    let last = AtomicU64::new(0);
    let (mut ar, mut aw) = tokio::io::split(a);
    let (mut br, mut bw) = tokio::io::split(b);
    let both = async {
        tokio::join!(
            copy_half(&mut ar, &mut bw, start, &last),
            copy_half(&mut br, &mut aw, start, &last)
        );
    };
    let watchdog = async {
        let tick = (idle / 4).max(Duration::from_millis(50));
        let idle_ms = u64::try_from(idle.as_millis()).unwrap_or(u64::MAX);
        loop {
            tokio::time::sleep(tick).await;
            let now = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
            if now.saturating_sub(last.load(Ordering::Relaxed)) >= idle_ms {
                return;
            }
        }
    };
    tokio::select! {
        () = both => {}
        () = watchdog => {}
    }
}

const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

fn strip_hop_by_hop(h: &mut http::HeaderMap) {
    let named: Vec<String> = h
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    for n in HOP_BY_HOP.iter().copied().map(str::to_owned).chain(named) {
        h.remove(n.as_str());
    }
}

fn log_decision(peer: SocketAddr, host: &str, port: u16, allowed: bool, reason: &str) {
    if allowed {
        tracing::info!(client = %peer.ip(), host, port, decision = "allow", reason, "egress");
    } else {
        tracing::warn!(client = %peer.ip(), host, port, decision = "deny", reason, "egress");
    }
}

async fn handle(
    s: Arc<ProxySettings>,
    peer: SocketAddr,
    req: Request<Incoming>,
) -> Result<Response<ProxyBody>, std::convert::Infallible> {
    let is_connect = req.method() == Method::CONNECT;
    let target = if is_connect {
        req.uri().authority().map(|a| a.as_str().to_owned())
    } else if req.uri().scheme_str() == Some("http") {
        req.uri().authority().map(|a| match a.port_u16() {
            Some(_) => a.as_str().to_owned(),
            None => format!("{}:80", a.host()),
        })
    } else {
        None
    };
    let Some(target) = target else {
        log_decision(peer, "-", 0, false, "not a proxy request");
        return Ok(text_response(
            StatusCode::BAD_REQUEST,
            "only CONNECT and absolute http:// requests are proxied",
        ));
    };
    let Some((raw_host, port)) = authority_parts(&target) else {
        log_decision(peer, "-", 0, false, "bad authority");
        return Ok(text_response(StatusCode::BAD_REQUEST, "bad host:port"));
    };
    let Some(host) = normalize_host(&raw_host) else {
        log_decision(peer, &raw_host, port, false, "not a host name");
        return Ok(text_response(
            StatusCode::FORBIDDEN,
            "denied: only allowlisted host names are reachable",
        ));
    };
    let rule = match rule_for(&s.rules, &host, port) {
        Ok(r) => r.clone(),
        Err(why) => {
            log_decision(peer, &host, port, false, why);
            return Ok(text_response(
                StatusCode::FORBIDDEN,
                &format!("denied: {why}"),
            ));
        }
    };
    let upstream = match resolve_and_connect(&s, &rule, &host, port).await {
        Ok(u) => u,
        Err((status, why)) => {
            log_decision(peer, &host, port, false, &why);
            return Ok(text_response(status, &format!("denied: {why}")));
        }
    };
    log_decision(peer, &host, port, true, "allowlisted");

    if is_connect {
        let idle = s.idle_timeout;
        tokio::spawn(async move {
            match hyper::upgrade::on(req).await {
                Ok(upgraded) => tunnel(TokioIo::new(upgraded), upstream, idle).await,
                Err(e) => tracing::debug!(error = %e, "CONNECT upgrade failed"),
            }
        });
        return Ok(Response::new(
            Empty::<Bytes>::new()
                .map_err(|never| match never {})
                .boxed(),
        ));
    }

    // Absolute-form HTTP: forward in origin form over the pinned connection.
    let (mut parts, body) = req.into_parts();
    let path = parts
        .uri
        .path_and_query()
        .map_or("/", http::uri::PathAndQuery::as_str)
        .to_owned();
    parts.uri = match path.parse::<Uri>() {
        Ok(u) => u,
        Err(_) => return Ok(text_response(StatusCode::BAD_REQUEST, "bad path")),
    };
    strip_hop_by_hop(&mut parts.headers);
    let host_header = if port == 80 {
        host.clone()
    } else {
        format!("{host}:{port}")
    };
    if let Ok(v) = HeaderValue::from_str(&host_header) {
        parts.headers.insert(header::HOST, v);
    }
    let (mut sender, conn) =
        match hyper::client::conn::http1::handshake(TokioIo::new(upstream)).await {
            Ok(x) => x,
            Err(e) => {
                return Ok(text_response(
                    StatusCode::BAD_GATEWAY,
                    &format!("upstream handshake failed: {e}"),
                ));
            }
        };
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let out = Request::from_parts(parts, body);
    match tokio::time::timeout(s.idle_timeout, sender.send_request(out)).await {
        Ok(Ok(resp)) => {
            let (mut rp, rb) = resp.into_parts();
            strip_hop_by_hop(&mut rp.headers);
            Ok(Response::from_parts(
                rp,
                rb.map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                    .boxed(),
            ))
        }
        Ok(Err(e)) => Ok(text_response(
            StatusCode::BAD_GATEWAY,
            &format!("upstream error: {e}"),
        )),
        Err(_) => Ok(text_response(
            StatusCode::GATEWAY_TIMEOUT,
            "upstream timeout",
        )),
    }
}

/// Serve proxy connections on `listener` until `shutdown` turns true.
pub async fn serve(
    listener: TcpListener,
    settings: ProxySettings,
    mut shutdown: watch::Receiver<bool>,
) {
    let s = Arc::new(settings);
    let limit = Arc::new(Semaphore::new(s.max_connections));
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, peer) = match accepted {
                    Ok(x) => x,
                    Err(e) => {
                        tracing::warn!(error = %e, "egress accept failed");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                };
                let Ok(permit) = Arc::clone(&limit).try_acquire_owned() else {
                    tracing::warn!(client = %peer.ip(), "egress connection limit reached");
                    drop(stream);
                    continue;
                };
                let s = Arc::clone(&s);
                tokio::spawn(async move {
                    let _permit = permit;
                    let svc = hyper::service::service_fn(move |req| handle(Arc::clone(&s), peer, req));
                    let mut b = hyper::server::conn::http1::Builder::new();
                    b.timer(TokioTimer::new())
                        .header_read_timeout(Duration::from_secs(10))
                        .max_buf_size(64 * 1024);
                    if let Err(e) = b.serve_connection(TokioIo::new(stream), svc).with_upgrades().await {
                        tracing::debug!(error = %e, "egress connection error");
                    }
                });
            }
            _ = shutdown.changed() => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn rule(host: &str, wildcard: bool, allow_private: bool) -> AllowRule {
        AllowRule {
            host: host.into(),
            wildcard,
            allow_private,
            ports: crate::config::DEFAULT_EGRESS_PORTS.to_vec(),
        }
    }

    #[test]
    fn classifies_addresses() {
        let p = |s: &str| ip_class(s.parse().unwrap());
        assert_eq!(p("8.8.8.8"), IpClass::Public);
        assert_eq!(p("2606:4700::1111"), IpClass::Public);
        for private in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "::1",
            "fd00:ec2::254",
            "fe80::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "64:ff9b::a00:1",
            "2002:c0a8:0101::1",
        ] {
            assert!(matches!(p(private), IpClass::Private(_)), "{private}");
        }
        for forbidden in [
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
            "240.0.0.1",
            "192.0.2.1",
            "::",
            "ff02::1",
            "2001:db8::1",
            "::1.2.3.4",
        ] {
            assert!(matches!(p(forbidden), IpClass::Forbidden(_)), "{forbidden}");
        }
        assert_eq!(p("172.32.0.1"), IpClass::Public);
        assert_eq!(p("100.128.0.1"), IpClass::Public);
        assert!(check_ip("10.0.0.1".parse().unwrap(), true).is_ok());
        assert!(check_ip("10.0.0.1".parse().unwrap(), false).is_err());
        assert!(check_ip("0.0.0.0".parse().unwrap(), true).is_err());
    }

    #[test]
    fn host_rules() {
        let rules = vec![
            rule("aiplatform.googleapis.com", false, false),
            rule("googleapis.com", true, false),
            rule("litellm", false, true),
        ];
        assert!(matching_rule(&rules, "aiplatform.googleapis.com").is_some());
        assert!(matching_rule(&rules, "us-central1-aiplatform.googleapis.com").is_some());
        assert!(matching_rule(&rules, "googleapis.com").is_none());
        assert!(matching_rule(&rules, "evilgoogleapis.com").is_none());
        assert!(matching_rule(&rules, "litellm").unwrap().allow_private);
        assert!(matching_rule(&rules, "github.com").is_none());
        assert_eq!(
            normalize_host("Example.COM.").as_deref(),
            Some("example.com")
        );
        assert!(normalize_host("10.0.0.1").is_none());
        assert!(normalize_host("[::1]").is_none());
        assert!(normalize_host("a b").is_none());
        assert!(normalize_host("-a.com").is_none());
    }

    #[tokio::test]
    async fn connect_falls_back_to_the_next_checked_address() {
        let good = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let good_addr = good.local_addr().unwrap();
        // Bound but never listening: connections are refused, and the port stays ours for
        // the whole test (a released port can be taken by a parallel test).
        let closed = tokio::net::TcpSocket::new_v4().unwrap();
        closed.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let closed_addr = closed.local_addr().unwrap();
        let s = connect_any(vec![closed_addr, good_addr], Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(s.peer_addr().unwrap(), good_addr);
        let (status, why) = connect_any(vec![closed_addr], Duration::from_secs(5))
            .await
            .unwrap_err();
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert!(why.starts_with("connect failed"), "{why}");
        let (status, _) = connect_any(Vec::new(), Duration::from_secs(1))
            .await
            .unwrap_err();
        assert_eq!(status, StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn ports_are_part_of_a_rule() {
        let mut litellm = rule("litellm", false, true);
        litellm.ports = vec![4000];
        let rules = vec![rule("github.com", false, false), litellm.clone()];
        assert!(rule_for(&rules, "github.com", 443).is_ok());
        assert!(rule_for(&rules, "github.com", 80).is_ok());
        assert_eq!(
            rule_for(&rules, "github.com", 22),
            Err("port not allowed for this host")
        );
        assert_eq!(rule_for(&rules, "litellm", 4000), Ok(&litellm));
        assert!(rule_for(&rules, "litellm", 443).is_err());
        assert_eq!(
            rule_for(&rules, "example.com", 443),
            Err("not on the allowlist")
        );
        // A later entry for the same host can open another port.
        let mut ssh = rule("github.com", false, false);
        ssh.ports = vec![22];
        let rules = vec![rule("github.com", false, false), ssh.clone()];
        assert_eq!(rule_for(&rules, "github.com", 22), Ok(&ssh));
    }

    #[test]
    fn hop_by_hop_headers_are_removed() {
        let mut h = http::HeaderMap::new();
        h.insert("connection", "close, x-secret".parse().unwrap());
        h.insert("x-secret", "1".parse().unwrap());
        h.insert("proxy-authorization", "Basic x".parse().unwrap());
        h.insert("accept", "*/*".parse().unwrap());
        strip_hop_by_hop(&mut h);
        assert_eq!(h.len(), 1);
        assert!(h.contains_key("accept"));
    }

    proptest! {
        #[test]
        fn classification_never_panics(v4 in any::<u32>(), v6 in any::<u128>()) {
            let _ = ip_class(IpAddr::V4(Ipv4Addr::from(v4)));
            let _ = ip_class(IpAddr::V6(Ipv6Addr::from(v6)));
        }

        #[test]
        fn wildcard_needs_a_label_boundary(prefix in "[a-z]{1,10}", suffix in "[a-z]{2,10}\\.com") {
            let rules = vec![rule(&suffix, true, false)];
            let sub = format!("{prefix}.{suffix}");
            let glued = format!("{prefix}{suffix}");
            prop_assert!(matching_rule(&rules, &sub).is_some());
            prop_assert!(matching_rule(&rules, &glued).is_none());
            prop_assert!(matching_rule(&rules, &suffix).is_none());
        }

        #[test]
        fn normalize_host_never_panics(s in ".{0,80}") {
            let _ = normalize_host(&s);
        }
    }
}
