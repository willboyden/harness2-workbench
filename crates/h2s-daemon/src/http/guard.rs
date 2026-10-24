//! The outer middleware every request passes (ARCHITECTURE.md §9), in this order:
//!
//! 1. request id (honoured from trusted proxies only) and client address;
//! 2. `Host` allowlist -> 421 (DNS-rebinding defence: loopback is not a trust boundary
//!    for a browser);
//! 3. unsafe methods need `x-h2s-request: 1` and, when an `Origin` is sent, a same-origin
//!    one -> 403 (CSRF defence; no CORS headers are ever sent);
//! 4. body limit (declared length -> 413 at once; chunked bodies are cut while read);
//! 5. request timeout -> 503;
//!
//! and on the way out, for every response including errors and static files: the
//! request id in error bodies and the `x-request-id` header, `ErrorBody` for any error
//! response no handler produced (404/405/...), the security headers, `Cache-Control:
//! no-store` on the API, metrics and one log line (method, route, status, request id;
//! never headers or bodies).

use std::net::{IpAddr, SocketAddr};
use std::time::Instant;

use axum::body::Body;
use axum::extract::{ConnectInfo, MatchedPath, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http::{HeaderValue, Method, StatusCode, header};
use tracing::Instrument;

use super::AppState;
use super::extract::{BodyLimit, RequestMeta};
use crate::config::split_host_port;
use crate::error::{ApiError, PendingError};

/// Headers on every response.
pub const SECURITY_HEADERS: &[(&str, &str)] = &[
    (
        "content-security-policy",
        "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; img-src 'self' data:; font-src 'self'; connect-src 'self'; manifest-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'",
    ),
    ("x-content-type-options", "nosniff"),
    ("referrer-policy", "no-referrer"),
    ("x-frame-options", "DENY"),
    (
        "permissions-policy",
        "camera=(), microphone=(), geolocation=()",
    ),
    ("cross-origin-opener-policy", "same-origin"),
    ("cross-origin-resource-policy", "same-origin"),
];

/// The matched route template, recorded for metrics by [`route_label`].
#[derive(Debug, Clone)]
pub struct RouteLabel(pub String);

/// `route_layer` middleware: copy the matched route template into the response.
pub async fn route_label(req: Request, next: Next) -> Response {
    let label = req
        .extensions()
        .get::<MatchedPath>()
        .map(|m| m.as_str().to_owned());
    let mut resp = next.run(req).await;
    if let Some(l) = label {
        resp.extensions_mut().insert(RouteLabel(l));
    }
    resp
}

fn is_unsafe(m: &Method) -> bool {
    !matches!(*m, Method::GET | Method::HEAD | Method::OPTIONS)
}

fn valid_request_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b':'))
}

/// The client behind a trusted proxy (see [`forwarded_client_from`]).
fn forwarded_client(state: &AppState, headers: &http::HeaderMap) -> Option<IpAddr> {
    let values: Vec<&[u8]> = headers
        .get_all("x-forwarded-for")
        .iter()
        .map(HeaderValue::as_bytes)
        .collect();
    forwarded_client_from(&values, |ip| state.cfg.is_trusted_proxy(ip))
}

/// Walk `X-Forwarded-For` from the right, where our proxies appended what they saw, and
/// return the first address that is not itself a trusted proxy. An entry that is not an
/// address ends the walk: everything left of it was written by the client, so the left-most
/// trusted hop seen so far is the answer (one malformed entry must not discard the real
/// client address and put every client behind the proxy into one rate-limit bucket).
fn forwarded_client_from(values: &[&[u8]], is_trusted: impl Fn(IpAddr) -> bool) -> Option<IpAddr> {
    let mut last_trusted = None;
    for value in values.iter().rev() {
        for part in value.rsplit(|b| *b == b',') {
            let ip = std::str::from_utf8(part).ok().map(str::trim).and_then(|p| {
                p.parse::<IpAddr>()
                    .ok()
                    .or_else(|| p.parse::<SocketAddr>().ok().map(|a| a.ip()))
            });
            let Some(ip) = ip else { return last_trusted };
            if !is_trusted(ip) {
                return Some(ip);
            }
            last_trusted = Some(ip);
        }
    }
    last_trusted
}

fn authority_allowed(state: &AppState, raw: &str) -> bool {
    let Some((host, port)) = split_host_port(raw) else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    state.hosts.iter().any(|e| e.matches(&host, port))
}

/// The `Host` header (or, without one, an absolute-form target's authority) is on the
/// allowlist. When the target is absolute-form, its authority must be allowed too: RFC 9112
/// lets a server route by it instead of `Host`.
pub(crate) fn host_allowed(state: &AppState, req: &Request) -> bool {
    let header = req.headers().get(header::HOST);
    let target = req.uri().authority().map(http::uri::Authority::as_str);
    if let Some(t) = target
        && !authority_allowed(state, t)
    {
        return false;
    }
    match header {
        Some(v) => v.to_str().is_ok_and(|h| authority_allowed(state, h)),
        None => target.is_some(),
    }
}

pub(crate) fn origin_allowed(state: &AppState, req: &Request, origin: &HeaderValue) -> bool {
    let Ok(o) = origin.to_str() else { return false };
    let o = o.trim().trim_end_matches('/').to_ascii_lowercase();
    if o == state.cfg.public_origin.to_ascii_lowercase() {
        return true;
    }
    // A same-origin request for the Host it was sent to (already on the allowlist).
    let Some(host) = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    o == format!("http://{host}") || o == format!("https://{host}")
}

fn content_length(req: &Request) -> Option<u64> {
    req.headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse().ok())
}

async fn check_and_run(state: &AppState, req: Request, next: Next) -> Response {
    // RFC 9112 §3.2: more than one Host field line is a 400.
    if req.headers().get_all(header::HOST).iter().nth(1).is_some() {
        state.metrics.auth_failure("host");
        return ApiError::bad_request("the request has more than one Host header").into_response();
    }
    if !host_allowed(state, &req) {
        state.metrics.auth_failure("host");
        return ApiError::misdirected().into_response();
    }
    if is_unsafe(req.method()) {
        let ok = req
            .headers()
            .get(h2s_api::CSRF_HEADER)
            .is_some_and(|v| v.as_bytes() == b"1");
        if !ok {
            state.metrics.auth_failure("csrf");
            return ApiError::forbidden(format!(
                "state-changing requests need the header {}: 1",
                h2s_api::CSRF_HEADER
            ))
            .into_response();
        }
        if let Some(origin) = req.headers().get(header::ORIGIN)
            && !origin_allowed(state, &req, origin)
        {
            state.metrics.auth_failure("origin");
            return ApiError::forbidden("cross-origin requests are refused").into_response();
        }
    }
    let max = state.cfg.file.server.max_body_bytes;
    if content_length(&req).is_some_and(|n| n > max) {
        return ApiError::too_large(max).into_response();
    }
    let limit = usize::try_from(max).unwrap_or(usize::MAX);
    let req = req.map(|b| Body::new(http_body_util::Limited::new(b, limit)));
    match tokio::time::timeout(state.cfg.request_timeout, next.run(req)).await {
        Ok(r) => r,
        Err(_) => ApiError::unavailable("the request took too long").into_response(),
    }
}

/// The outer middleware (see the module docs).
pub async fn guard(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let started = Instant::now();
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0);
    let trusted = peer.is_some_and(|p| state.cfg.is_trusted_proxy(p.ip()));
    let request_id = req
        .headers()
        .get(h2s_api::REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .filter(|s| trusted && valid_request_id(s))
        .map_or_else(|| uuid::Uuid::new_v4().simple().to_string(), str::to_owned);
    let client_ip = if trusted {
        forwarded_client(&state, req.headers()).or(peer.map(|p| p.ip()))
    } else {
        peer.map(|p| p.ip())
    };
    let meta = RequestMeta {
        request_id,
        client_ip,
        peer,
        peer_is_trusted_proxy: trusted,
    };
    req.extensions_mut().insert(meta.clone());
    req.extensions_mut()
        .insert(BodyLimit(state.cfg.file.server.max_body_bytes));

    let span = tracing::info_span!(
        "http",
        method = %method,
        route = tracing::field::Empty,
        status = tracing::field::Empty,
        request_id = %meta.request_id,
    );
    let resp = check_and_run(&state, req, next)
        .instrument(span.clone())
        .await;
    finalize(&state, resp, &meta, &method, &path, started, &span)
}

fn finalize(
    state: &AppState,
    mut resp: Response,
    meta: &RequestMeta,
    method: &Method,
    path: &str,
    started: Instant,
    span: &tracing::Span,
) -> Response {
    let status = resp.status();
    if let Some(PendingError(mut body)) = resp.extensions_mut().remove::<PendingError>() {
        body.error.request_id = Some(meta.request_id.clone());
        set_json_body(&mut resp, &body);
    } else if status.is_client_error() || status.is_server_error() {
        let is_json = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.starts_with("application/json"));
        if !is_json {
            let mut body = ApiError::from_status(status).body();
            body.error.request_id = Some(meta.request_id.clone());
            set_json_body(&mut resp, &body);
        }
    }
    let headers = resp.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&meta.request_id) {
        headers.insert(h2s_api::REQUEST_ID_HEADER, v);
    }
    for (k, v) in SECURITY_HEADERS {
        headers.insert(*k, HeaderValue::from_static(v));
    }
    let cors: Vec<http::HeaderName> = headers
        .keys()
        .filter(|k| k.as_str().starts_with("access-control-"))
        .cloned()
        .collect();
    for k in cors {
        headers.remove(k);
    }
    let api = path.starts_with("/api/") || matches!(path, "/healthz" | "/readyz" | "/metrics");
    if api {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }

    let route = resp
        .extensions()
        .get::<super::guard::RouteLabel>()
        .map_or_else(
            || {
                if path.starts_with("/api/") {
                    "unmatched".to_owned()
                } else {
                    "static".to_owned()
                }
            },
            |r| r.0.clone(),
        );
    let secs = started.elapsed().as_secs_f64();
    state
        .metrics
        .observe_http(method, &route, status.as_u16(), secs);
    span.record("route", route.as_str());
    span.record("status", status.as_u16());
    let quiet = matches!(path, "/healthz" | "/readyz" | "/metrics");
    let ms = (secs * 1000.0).round();
    if quiet {
        tracing::debug!(parent: span, latency_ms = ms, "request");
    } else if status == StatusCode::INTERNAL_SERVER_ERROR {
        tracing::warn!(parent: span, latency_ms = ms, "request failed");
    } else {
        tracing::info!(parent: span, latency_ms = ms, "request");
    }
    resp
}

fn set_json_body(resp: &mut Response, body: &h2s_api::ErrorBody) {
    let json = serde_json::to_vec(body).unwrap_or_else(|_| b"{}".to_vec());
    resp.headers_mut().remove(header::CONTENT_LENGTH);
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    *resp.body_mut() = Body::from(json);
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn request_ids() {
        assert!(valid_request_id("abc-123_x.y:z"));
        assert!(!valid_request_id(""));
        assert!(!valid_request_id("a b"));
        assert!(!valid_request_id(&"a".repeat(129)));
        assert!(!valid_request_id("a\nb"));
    }

    #[test]
    fn forwarded_for_takes_the_right_most_untrusted_address() {
        let trusted = |ip: IpAddr| matches!(ip, IpAddr::V4(v4) if v4.octets()[0] == 10);
        let client = |values: &[&str]| {
            let raw: Vec<&[u8]> = values.iter().map(|v| v.as_bytes()).collect();
            forwarded_client_from(&raw, trusted).map(|ip| ip.to_string())
        };
        let ip = |s: &str| Some(s.to_owned());
        assert_eq!(client(&["203.0.113.5"]), ip("203.0.113.5"));
        // Client-supplied junk left of what the proxy appended is ignored.
        assert_eq!(client(&["garbage, 203.0.113.5"]), ip("203.0.113.5"));
        assert_eq!(client(&["198.51.100.1, 203.0.113.5"]), ip("203.0.113.5"));
        assert_eq!(
            client(&["203.0.113.9, 203.0.113.5, 10.0.0.2"]),
            ip("203.0.113.5")
        );
        assert_eq!(client(&["198.51.100.7", "203.0.113.5"]), ip("203.0.113.5"));
        assert_eq!(client(&["[2001:db8::1]:443"]), ip("2001:db8::1"));
        // Junk between trusted hops ends the walk at the last trusted hop.
        assert_eq!(client(&["203.0.113.5, junk, 10.0.0.2"]), ip("10.0.0.2"));
        assert_eq!(client(&["10.0.0.3, 10.0.0.2"]), ip("10.0.0.3"));
        assert_eq!(client(&[]), None);
        assert_eq!(client(&["junk"]), None);
        let raw: Vec<&[u8]> = vec![b"\xff\xfe, 203.0.113.5"];
        assert_eq!(
            forwarded_client_from(&raw, trusted),
            Some("203.0.113.5".parse().unwrap())
        );
    }

    #[test]
    fn unsafe_methods() {
        assert!(!is_unsafe(&Method::GET));
        assert!(!is_unsafe(&Method::HEAD));
        assert!(!is_unsafe(&Method::OPTIONS));
        for m in [
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::TRACE,
        ] {
            assert!(is_unsafe(&m));
        }
    }

    proptest! {
        #[test]
        fn request_id_check_never_panics(s in ".{0,200}") {
            let _ = valid_request_id(&s);
        }
    }
}
