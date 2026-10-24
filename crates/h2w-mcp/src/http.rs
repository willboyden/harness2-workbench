//! Streamable HTTP transport (`--transport http`).
//!
//! The MCP endpoint is `POST /mcp` (rmcp's streamable HTTP service, stateless: no
//! `Mcp-Session-Id`, every request stands alone). In front of it a guard checks, in order:
//!
//! 1. `Host` is an allowed authority (loopback forms of the listen address plus
//!    `--allowed-host`), else **421** — the DNS-rebinding defence;
//! 2. `Origin`, when present, is an allowed origin, else **403**;
//! 3. `Authorization: Bearer h2w_...` is present and well formed, else **401**;
//! 4. fewer than `--max-concurrent-requests` requests are being handled, else **503** with
//!    `Retry-After: 1`.
//!
//! Each API token may also run only `--max-waits-per-token` waits at once (a tool error
//! `rate_limited: ...` beyond it, see [`crate::limits`]).
//! The caller's token is then forwarded to the Workbench server for every API call made on behalf
//! of that request, so the server's roles and audit apply per client. Tokens are never logged.

use std::{net::SocketAddr, sync::Arc};

use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::Response,
};
use h2w_api::{ApiError as WireError, ErrorBody, ErrorCode};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
};
use tokio::{
    net::TcpListener,
    sync::{OwnedSemaphorePermit, Semaphore},
};
use tokio_util::sync::CancellationToken;

use crate::{
    client::ApiClient,
    limits::{DEFAULT_WAITS_PER_TOKEN, WaitLimiter},
    server::{Credentials, H2sMcp},
    token::Token,
};

/// Path of the MCP endpoint.
pub const MCP_PATH: &str = "/mcp";

/// HTTP transport settings.
#[derive(Debug, Clone)]
pub struct HttpOptions {
    /// Extra accepted `Host` values (`host` or `host:port`), beyond the loopback forms of the
    /// listen address.
    pub allowed_hosts: Vec<String>,
    /// Extra accepted browser origins (`scheme://host[:port]`).
    pub allowed_origins: Vec<String>,
    /// Largest request body accepted (413 beyond it).
    pub max_body_bytes: usize,
    /// Most MCP requests handled at once; beyond it the answer is 503 with `Retry-After: 1`.
    pub max_concurrent_requests: usize,
    /// Most concurrent waits (`wait_for_job`, or job tools with `wait_secs`) per API token.
    pub max_waits_per_token: usize,
    /// Pass `acknowledge_risks=true` from MCP clients on to the server (off by default).
    pub allow_risk_acknowledgement: bool,
}

/// Default for [`HttpOptions::max_concurrent_requests`].
pub const DEFAULT_MAX_CONCURRENT_REQUESTS: usize = 32;

impl Default for HttpOptions {
    fn default() -> Self {
        Self {
            allowed_hosts: Vec::new(),
            allowed_origins: Vec::new(),
            max_body_bytes: 1024 * 1024,
            max_concurrent_requests: DEFAULT_MAX_CONCURRENT_REQUESTS,
            max_waits_per_token: DEFAULT_WAITS_PER_TOKEN,
            allow_risk_acknowledgement: false,
        }
    }
}

/// The caller's token from `Authorization: Bearer <token>`, if present and well formed.
#[must_use]
pub fn bearer_token(headers: &HeaderMap) -> Option<Token> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.trim().split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    Token::parse(token).ok()
}

/// Serve MCP on `listener` until `shutdown` is cancelled.
///
/// # Errors
/// The listener fails.
pub async fn serve(
    listener: TcpListener,
    api: ApiClient,
    options: &HttpOptions,
    shutdown: CancellationToken,
) -> std::io::Result<()> {
    let local = listener.local_addr()?;
    let app = router(api, options, local, shutdown.child_token());
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await
}

/// The axum application: the guard in front of the MCP service at [`MCP_PATH`].
pub fn router(
    api: ApiClient,
    options: &HttpOptions,
    local: SocketAddr,
    shutdown: CancellationToken,
) -> Router {
    let guard = Arc::new(Guard::new(local, options));
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_allowed_hosts(guard.host_strings())
        .with_max_request_body_bytes(options.max_body_bytes)
        .with_cancellation_token(shutdown);
    // One limiter for every per-request handler, so the cap holds across requests.
    let waits = WaitLimiter::new(options.max_waits_per_token);
    let allow_risk_acknowledgement = options.allow_risk_acknowledgement;
    let service = StreamableHttpService::new(
        move || {
            Ok(H2sMcp::new(api.clone(), Credentials::PerRequest)
                .with_wait_limiter(waits.clone())
                .with_risk_acknowledgement(allow_risk_acknowledgement))
        },
        Arc::new(NeverSessionManager::default()),
        config,
    );
    Router::new()
        .nest_service(MCP_PATH, service)
        .layer(middleware::from_fn_with_state(guard, guard_requests))
}

/// One accepted `Host` authority. `port: None` accepts any port.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HostRule {
    host: String,
    port: Option<u16>,
}

impl HostRule {
    fn parse(authority: &str) -> Option<Self> {
        // A Host header never carries userinfo; `Authority` would accept `evil@127.0.0.1:8723`.
        if authority.contains('@') {
            return None;
        }
        let authority: axum::http::uri::Authority = authority.trim().parse().ok()?;
        Some(Self {
            host: normalize_host(authority.host()),
            port: authority.port_u16(),
        })
    }

    fn matches(&self, other: &HostRule) -> bool {
        self.host == other.host && self.port.is_none_or(|port| other.port == Some(port))
    }

    fn display(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        match self.port {
            Some(port) => format!("{host}:{port}"),
            None => host,
        }
    }
}

fn normalize_host(host: &str) -> String {
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

/// A normalized browser origin.
#[derive(Debug, Clone, PartialEq, Eq)]
struct OriginValue {
    scheme: String,
    host: String,
    port: u16,
}

impl OriginValue {
    fn parse(value: &str) -> Option<Self> {
        let url = url::Url::parse(value.trim()).ok()?;
        let bare = url.path().is_empty() || url.path() == "/";
        if !matches!(url.scheme(), "http" | "https") || !bare || url.query().is_some() {
            return None;
        }
        Some(Self {
            scheme: url.scheme().to_owned(),
            host: normalize_host(url.host_str()?),
            port: url.port_or_known_default()?,
        })
    }
}

#[derive(Debug)]
struct Guard {
    hosts: Vec<HostRule>,
    origins: Vec<OriginValue>,
    /// Concurrent-request slots.
    capacity: Arc<Semaphore>,
    max_concurrent: usize,
}

/// One of the server's concurrent-request slots. It rides in the request's extensions, which
/// rmcp hands on to the MCP request it serves (`http::request::Parts`), so the slot stays taken
/// until that request has been handled — event-stream responses included — and is freed when
/// rmcp drops it (also when it refuses the request early).
#[derive(Clone)]
struct InFlight {
    _slot: Arc<OwnedSemaphorePermit>,
}

impl Guard {
    fn new(local: SocketAddr, options: &HttpOptions) -> Self {
        let port = Some(local.port());
        let mut hosts = vec![
            HostRule {
                host: "localhost".into(),
                port,
            },
            HostRule {
                host: "127.0.0.1".into(),
                port,
            },
            HostRule {
                host: "::1".into(),
                port,
            },
        ];
        let ip = local.ip();
        if !ip.is_loopback() && !ip.is_unspecified() {
            hosts.push(HostRule {
                host: normalize_host(&ip.to_string()),
                port,
            });
        }
        for extra in &options.allowed_hosts {
            match HostRule::parse(extra) {
                Some(rule) => hosts.push(rule),
                None => tracing::warn!(value = %extra, "ignoring an unparsable --allowed-host"),
            }
        }
        let mut origins = Vec::new();
        for extra in &options.allowed_origins {
            match OriginValue::parse(extra) {
                Some(origin) => origins.push(origin),
                None => tracing::warn!(value = %extra, "ignoring an unparsable --allowed-origin"),
            }
        }
        let max_concurrent = options.max_concurrent_requests.max(1);
        Self {
            hosts,
            origins,
            capacity: Arc::new(Semaphore::new(max_concurrent)),
            max_concurrent,
        }
    }

    /// The same allowlist in rmcp's format (its own check runs behind this guard).
    fn host_strings(&self) -> Vec<String> {
        self.hosts.iter().map(HostRule::display).collect()
    }

    fn host_allowed(&self, value: &str) -> bool {
        HostRule::parse(value).is_some_and(|got| self.hosts.iter().any(|rule| rule.matches(&got)))
    }

    /// An origin is allowed when it names an allowed host (any scheme of http/https) or is
    /// listed with `--allowed-origin`.
    fn origin_allowed(&self, value: &str) -> bool {
        let Some(origin) = OriginValue::parse(value) else {
            return false;
        };
        let as_host = HostRule {
            host: origin.host.clone(),
            port: Some(origin.port),
        };
        self.hosts.iter().any(|rule| rule.matches(&as_host)) || self.origins.contains(&origin)
    }
}

async fn guard_requests(
    State(guard): State<Arc<Guard>>,
    mut request: Request,
    next: Next,
) -> Response {
    let mut response = if let Some(refusal) = check(&guard, &request) {
        refusal
    } else if let Ok(slot) = Arc::clone(&guard.capacity).try_acquire_owned() {
        request.extensions_mut().insert(InFlight {
            _slot: Arc::new(slot),
        });
        next.run(request).await
    } else {
        busy(guard.max_concurrent)
    };
    harden(response.headers_mut());
    response
}

/// 503 for a request over the concurrency cap.
fn busy(max: usize) -> Response {
    tracing::warn!(max, "refused a request: too many concurrent requests");
    let mut response = refuse(
        StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::Unavailable,
        &format!(
            "this MCP server is already handling {max} requests, its maximum \
             (--max-concurrent-requests); retry in a moment"
        ),
    );
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    response
}

/// Security headers on every response, refusals included (the Workbench server's policy, §9).
/// MCP answers are JSON or event streams: nothing here is meant to render or be cached.
fn harden(headers: &mut HeaderMap) {
    let set = [
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::REFERRER_POLICY, "no-referrer"),
        (header::X_FRAME_OPTIONS, "DENY"),
        (
            header::CONTENT_SECURITY_POLICY,
            "default-src 'none'; frame-ancestors 'none'",
        ),
        (header::CACHE_CONTROL, "no-store"),
    ];
    for (name, value) in set {
        headers.insert(name, HeaderValue::from_static(value));
    }
    headers.insert(
        header::HeaderName::from_static("cross-origin-resource-policy"),
        HeaderValue::from_static("same-origin"),
    );
}

/// The refusal for a request the guard does not let through, if any.
fn check(guard: &Guard, request: &Request) -> Option<Response> {
    let headers = request.headers();
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    if !host.is_some_and(|h| guard.host_allowed(h)) {
        tracing::warn!(
            host = ?host.map(|h| h.chars().take(100).collect::<String>()),
            "refused a request for an unknown Host (possible DNS rebinding)"
        );
        return Some(refuse(
            StatusCode::MISDIRECTED_REQUEST,
            ErrorCode::MisdirectedRequest,
            "this MCP server does not answer to that Host; add it with --allowed-host if it is \
             legitimate",
        ));
    }
    if let Some(origin) = headers.get(header::ORIGIN) {
        let origin = origin.to_str().unwrap_or("");
        if !guard.origin_allowed(origin) {
            tracing::warn!(
                origin = ?origin.chars().take(100).collect::<String>(),
                "refused a cross-origin request"
            );
            return Some(refuse(
                StatusCode::FORBIDDEN,
                ErrorCode::Forbidden,
                "cross-origin requests are not allowed; add the origin with --allowed-origin if \
                 it is legitimate",
            ));
        }
    }
    if bearer_token(headers).is_none() {
        let mut response = refuse(
            StatusCode::UNAUTHORIZED,
            ErrorCode::Unauthorized,
            "send `Authorization: Bearer h2w_...` with a Harness² Workbench API token; it is \
             forwarded to the Workbench server, which checks it",
        );
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Bearer realm=\"h2workbench-mcp\""),
        );
        return Some(response);
    }
    None
}

fn refuse(status: StatusCode, code: ErrorCode, message: &str) -> Response {
    let body = ErrorBody {
        error: WireError {
            code,
            message: message.to_owned(),
            details: Vec::new(),
            request_id: None,
        },
    };
    let bytes = serde_json::to_vec(&body).unwrap_or_default();
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard(extra_hosts: &[&str], extra_origins: &[&str]) -> Guard {
        Guard::new(
            "127.0.0.1:8723".parse().unwrap(),
            &HttpOptions {
                allowed_hosts: extra_hosts.iter().map(|s| (*s).to_owned()).collect(),
                allowed_origins: extra_origins.iter().map(|s| (*s).to_owned()).collect(),
                ..HttpOptions::default()
            },
        )
    }

    #[test]
    fn loopback_forms_of_the_listen_port_are_allowed() {
        let g = guard(&[], &[]);
        for ok in [
            "127.0.0.1:8723",
            "localhost:8723",
            "LOCALHOST:8723",
            "[::1]:8723",
        ] {
            assert!(g.host_allowed(ok), "{ok}");
        }
        for bad in [
            "127.0.0.1:8724",
            "localhost",
            "evil.example:8723",
            "attacker.localhost.evil:8723",
            "",
            "127.0.0.1:8723 extra",
            "evil@127.0.0.1:8723",
            "user:pass@localhost:8723",
        ] {
            assert!(!g.host_allowed(bad), "{bad}");
        }
    }

    #[test]
    fn extra_hosts_without_a_port_match_any_port() {
        let g = guard(&["mcp.example.com", "proxy.internal:9000"], &[]);
        assert!(g.host_allowed("mcp.example.com"));
        assert!(g.host_allowed("mcp.example.com:443"));
        assert!(g.host_allowed("proxy.internal:9000"));
        assert!(!g.host_allowed("proxy.internal:9001"));
    }

    #[test]
    fn origins() {
        let g = guard(&[], &["https://inspector.example.com"]);
        assert!(g.origin_allowed("http://127.0.0.1:8723"));
        assert!(g.origin_allowed("http://localhost:8723"));
        assert!(g.origin_allowed("https://inspector.example.com"));
        assert!(g.origin_allowed("https://inspector.example.com:443"));
        for bad in [
            "http://evil.example",
            "http://localhost:6274",
            "null",
            "https://inspector.example.com:8443",
            "file:///x",
            "",
        ] {
            assert!(!g.origin_allowed(bad), "{bad}");
        }
    }

    #[test]
    fn bearer_parsing() {
        let mut h = HeaderMap::new();
        assert!(bearer_token(&h).is_none());
        h.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer h2w_alice_s3cret"),
        );
        assert!(bearer_token(&h).is_some());
        h.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("bearer h2w_alice_s3cret"),
        );
        assert!(bearer_token(&h).is_some());
        for bad in [
            "Basic h2w_alice_s3cret",
            "Bearer",
            "Bearer nope",
            "h2w_alice_s3cret",
        ] {
            h.insert(header::AUTHORIZATION, HeaderValue::from_static(bad));
            assert!(bearer_token(&h).is_none(), "{bad}");
        }
    }

    #[test]
    fn rmcp_gets_the_same_hosts() {
        let g = guard(&["mcp.example.com"], &[]);
        assert_eq!(
            g.host_strings(),
            vec![
                "localhost:8723",
                "127.0.0.1:8723",
                "[::1]:8723",
                "mcp.example.com"
            ]
        );
    }
}
