//! Request extractors: request metadata, JSON bodies and queries with `ErrorBody`
//! rejections, and authentication + role checks.
//!
//! Role checks are route-level extractors (ARCHITECTURE.md §9): every handler takes an
//! [`Auth`] parameter first, so authentication and authorization run before anything else
//! the handler extracts or does. `tests/rbac.rs` enumerates the router's route table and
//! proves each route enforces its documented role.

use std::marker::PhantomData;
use std::net::{IpAddr, SocketAddr};

use axum::extract::{FromRequest, FromRequestParts, Request};
use h2s_api::{AuditOutcome, Principal, PrincipalKind, Role};
use http::request::Parts;
use serde::de::DeserializeOwned;

use super::AppState;
use crate::db::audit::AuditEntry;
use crate::db::tokens::TokenRecord;
use crate::error::{ApiError, scrub_quoted};
use crate::jobs::Actor;
use crate::util::now_unix;

/// Per-request facts set by the outer middleware.
#[derive(Debug, Clone)]
pub struct RequestMeta {
    pub request_id: String,
    /// The client: the peer, or the forwarding client when the peer is a trusted proxy.
    pub client_ip: Option<IpAddr>,
    pub peer: Option<SocketAddr>,
    pub peer_is_trusted_proxy: bool,
}

impl RequestMeta {
    #[must_use]
    pub fn ip(&self) -> Option<String> {
        self.client_ip.map(|i| i.to_string())
    }

    /// An audit entry carrying this request's context.
    #[must_use]
    pub fn audit(
        &self,
        actor: &str,
        action: &str,
        target: &str,
        outcome: AuditOutcome,
    ) -> AuditEntry {
        AuditEntry {
            actor: actor.to_owned(),
            action: action.to_owned(),
            target: target.to_owned(),
            outcome,
            ip: self.ip(),
            request_id: Some(self.request_id.clone()),
            detail: None,
        }
    }
}

impl<S: Send + Sync> FromRequestParts<S> for RequestMeta {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(parts
            .extensions
            .get::<RequestMeta>()
            .cloned()
            .unwrap_or_else(|| RequestMeta {
                request_id: "unknown".into(),
                client_ip: None,
                peer: None,
                peer_is_trusted_proxy: false,
            }))
    }
}

/// `true` when the error chain contains a body-length-limit error.
pub(crate) fn is_length_limit(err: &(dyn std::error::Error + 'static)) -> bool {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = cur {
        if e.is::<http_body_util::LengthLimitError>() {
            return true;
        }
        cur = e.source();
    }
    false
}

/// A JSON request body. Rejections: 400 (not JSON, malformed, wrong types: messages never
/// quote input values), 413 (over the body limit, chunked or not).
#[derive(Debug, Clone)]
pub struct ApiJson<T>(pub T);

impl<T: DeserializeOwned, S: Send + Sync> FromRequest<S> for ApiJson<T> {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let _ = state;
        let ct = req
            .headers()
            .get(http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let essence = ct
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if essence != "application/json" && !essence.ends_with("+json") {
            return Err(ApiError::bad_request(
                "expected a JSON body with Content-Type: application/json",
            ));
        }
        let limit = req
            .extensions()
            .get::<BodyLimit>()
            .map_or(1_048_576, |l| l.0);
        let bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
            .await
            .map_err(|e| {
                let err: &(dyn std::error::Error + 'static) = &e;
                if is_length_limit(err) {
                    ApiError::too_large(limit)
                } else {
                    ApiError::bad_request("could not read the request body")
                }
            })?;
        serde_json::from_slice(&bytes).map(ApiJson).map_err(|e| {
            ApiError::bad_request(format!(
                "invalid JSON body: {}",
                scrub_quoted(&e.to_string())
            ))
        })
    }
}

/// The body limit in force (set by the middleware, read by [`ApiJson`] for its message).
#[derive(Debug, Clone, Copy)]
pub struct BodyLimit(pub u64);

/// A query string. Query structs deny unknown parameters; any problem (unknown parameter,
/// unknown enum value, bad number) is a 422 on the field `query`.
#[derive(Debug, Clone)]
pub struct ApiQuery<T>(pub T);

impl<T: DeserializeOwned, S: Send + Sync> FromRequestParts<S> for ApiQuery<T> {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        let q = parts.uri.query().unwrap_or("");
        serde_urlencoded_lite::from_str(q)
            .map(ApiQuery)
            .map_err(|e| ApiError::invalid("query", scrub_quoted(&e)))
    }
}

/// Query parsing through `axum::extract::Query`'s deserializer, without its rejection type.
mod serde_urlencoded_lite {
    use serde::de::DeserializeOwned;

    pub(super) fn from_str<T: DeserializeOwned>(q: &str) -> Result<T, String> {
        let uri: http::Uri = format!("/?{q}")
            .parse()
            .map_err(|_| "malformed query string".to_owned())?;
        axum::extract::Query::<T>::try_from_uri(&uri)
            .map(|axum::extract::Query(v)| v)
            .map_err(|e| e.body_text())
    }
}

// ---------------------------------------------------------------------------------------
// Authentication
// ---------------------------------------------------------------------------------------

/// Failed authentications with credentials (malformed, unknown or wrong tokens, bad proxy
/// headers) per client address and minute; further failures are answered 429.
pub const AUTH_FAILURES_PER_MINUTE: u32 = 60;
/// Audit records of failed authentication (`auth.failed`, denied `auth.login`) per client
/// address and minute; further failures are only counted in `h2s_auth_failures_total`.
pub const AUDITED_FAILURES_PER_MINUTE: u32 = 10;

/// Why a request was not authenticated (also the metric label).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthFailure {
    Missing,
    Malformed,
    UnknownToken,
    BadSecret,
    Expired,
    Revoked,
    SessionInvalid,
    ProxyUntrusted,
    ProxySecret,
    ProxyUser,
    /// The user, groups or secret header appears more than once.
    ProxyDuplicate,
    ProxyNoRole,
    Unavailable,
}

impl AuthFailure {
    #[must_use]
    pub fn reason(self) -> &'static str {
        match self {
            AuthFailure::Missing => "missing",
            AuthFailure::Malformed => "malformed",
            AuthFailure::UnknownToken => "unknown_token",
            AuthFailure::BadSecret => "bad_secret",
            AuthFailure::Expired => "expired",
            AuthFailure::Revoked => "revoked",
            AuthFailure::SessionInvalid => "session_invalid",
            AuthFailure::ProxyUntrusted => "proxy_untrusted",
            AuthFailure::ProxySecret => "proxy_secret",
            AuthFailure::ProxyUser => "proxy_user",
            AuthFailure::ProxyDuplicate => "proxy_duplicate_header",
            AuthFailure::ProxyNoRole => "proxy_no_role",
            AuthFailure::Unavailable => "unavailable",
        }
    }

    /// Credentials were presented (worth an audit record); requests with none are only
    /// counted, so anonymous probes cannot flood the audit log.
    #[must_use]
    pub fn presented(self) -> bool {
        !matches!(self, AuthFailure::Missing | AuthFailure::Unavailable)
    }

    fn into_error(self) -> ApiError {
        match self {
            AuthFailure::Missing => ApiError::unauthorized("authentication required"),
            AuthFailure::Expired => ApiError::unauthorized("the token has expired"),
            AuthFailure::Revoked => ApiError::unauthorized("the token has been revoked"),
            AuthFailure::SessionInvalid => {
                ApiError::unauthorized("the session has ended; sign in again")
            }
            AuthFailure::ProxyNoRole => {
                ApiError::forbidden("your groups do not grant a role on this server")
            }
            AuthFailure::Unavailable => {
                ApiError::unavailable("authentication is temporarily unavailable")
            }
            _ => ApiError::unauthorized("invalid credentials"),
        }
    }
}

/// What authenticated a request, kept so long-lived responses (event streams) can check
/// that it is still valid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Credential {
    /// An API token (by id).
    Token(String),
    /// A browser session (by the hash of its id).
    Session([u8; 32]),
    /// Reverse-proxy headers: nothing the server can revoke.
    Proxy,
}

/// Whether `credential` still authenticates: the token is neither revoked nor expired, the
/// session still exists (logout, revocation and expiry end it). Database errors count as
/// invalid (the client reconnects and authenticates again).
pub(crate) async fn credential_valid(state: &AppState, credential: &Credential) -> bool {
    let now = now_unix();
    match credential.clone() {
        Credential::Proxy => true,
        Credential::Token(id) => state
            .db
            .call(move |c| crate::db::tokens::get(c, &id))
            .await
            .ok()
            .flatten()
            .is_some_and(|r| r.is_active(now)),
        Credential::Session(hash) => state
            .db
            .call(move |c| crate::db::sessions::lookup(c, &hash, now))
            .await
            .ok()
            .flatten()
            .is_some_and(|r| r.is_active(now)),
    }
}

/// Verify a presented token string.
pub(crate) async fn verify_token(
    state: &AppState,
    token: &str,
) -> Result<TokenRecord, AuthFailure> {
    let parsed = crate::auth::parse(token).ok_or(AuthFailure::Malformed)?;
    let id = parsed.id.to_owned();
    let rec = state
        .db
        .call(move |c| crate::db::tokens::get(c, &id))
        .await
        .map_err(|_| AuthFailure::Unavailable)?;
    let Some(rec) = rec else {
        // Spend the same hashing work as a real comparison.
        let _ = crate::auth::secret_matches(&[0; 32], parsed.secret);
        return Err(AuthFailure::UnknownToken);
    };
    if !crate::auth::secret_matches(&rec.secret_hash, parsed.secret) {
        return Err(AuthFailure::BadSecret);
    }
    let now = now_unix();
    if rec.revoked_at.is_some() {
        return Err(AuthFailure::Revoked);
    }
    if rec.expires_at.is_some_and(|e| e <= now) {
        return Err(AuthFailure::Expired);
    }
    let id = rec.id.clone();
    let _ = state
        .db
        .call(move |c| crate::db::tokens::touch(c, &id, now))
        .await;
    Ok(rec)
}

fn token_principal(rec: &TokenRecord, kind: PrincipalKind) -> Principal {
    Principal {
        id: rec.id.clone(),
        name: rec.name.clone(),
        role: rec.role,
        kind,
    }
}

/// Name of the session cookie over HTTPS. The `__Host-` prefix makes browsers refuse it
/// unless it is `Secure`, has `Path=/` and no `Domain`, so a sibling subdomain cannot plant
/// ("toss") a session cookie for this host.
pub const SECURE_SESSION_COOKIE: &str = "__Host-h2s_session";

/// The session cookie's name: [`SECURE_SESSION_COOKIE`] when `public_url` is `https`, else
/// `h2s_session` (`__Host-` cookies need `Secure`, which browsers drop over plain HTTP).
#[must_use]
pub fn session_cookie_name(public_https: bool) -> &'static str {
    if public_https {
        SECURE_SESSION_COOKIE
    } else {
        h2s_api::SESSION_COOKIE
    }
}

/// The value of the session cookie named `name`, when present (a cookie under any other
/// name, the plain `h2s_session` included when the server uses the `__Host-` name, is
/// ignored).
pub(crate) fn session_cookie(headers: &http::HeaderMap, name: &str) -> Option<String> {
    for v in headers.get_all(http::header::COOKIE) {
        let Ok(s) = v.to_str() else { continue };
        for c in cookie::Cookie::split_parse(s).flatten() {
            if c.name() == name {
                return Some(c.value().to_owned());
            }
        }
    }
    None
}

/// The bearer token in `Authorization`, `Err` when the header is present but malformed.
pub(crate) fn bearer(headers: &http::HeaderMap) -> Option<Result<String, AuthFailure>> {
    let v = headers.get(http::header::AUTHORIZATION)?;
    let Ok(s) = v.to_str() else {
        return Some(Err(AuthFailure::Malformed));
    };
    let mut parts = s.trim().splitn(2, ' ');
    let scheme = parts.next().unwrap_or("");
    let value = parts.next().unwrap_or("").trim();
    if !scheme.eq_ignore_ascii_case("bearer") || value.is_empty() {
        return Some(Err(AuthFailure::Malformed));
    }
    Some(Ok(value.to_owned()))
}

/// A failed authentication, with the token id named by the attempt (for the audit log).
struct Failed {
    why: AuthFailure,
    target: Option<String>,
}

impl From<AuthFailure> for Failed {
    fn from(why: AuthFailure) -> Failed {
        Failed { why, target: None }
    }
}

async fn resolve(
    state: &AppState,
    headers: &http::HeaderMap,
    meta: &RequestMeta,
) -> Result<(Principal, Credential), Failed> {
    if let Some(b) = bearer(headers) {
        let token = b?;
        return match verify_token(state, &token).await {
            Ok(rec) => Ok((
                token_principal(&rec, PrincipalKind::Token),
                Credential::Token(rec.id),
            )),
            Err(why) => Err(Failed {
                why,
                target: crate::auth::token_id_hint(&token).map(str::to_owned),
            }),
        };
    }
    if let Some(value) = session_cookie(headers, session_cookie_name(state.cfg.public_https)) {
        let hash = crate::auth::session_hash(&value).ok_or(AuthFailure::SessionInvalid)?;
        let now = now_unix();
        let rec = state
            .db
            .call(move |c| crate::db::sessions::lookup(c, &hash, now))
            .await
            .map_err(|_| AuthFailure::Unavailable)?
            .ok_or(AuthFailure::SessionInvalid)?;
        if !rec.is_active(now) {
            return Err(Failed {
                why: AuthFailure::SessionInvalid,
                target: Some(rec.id),
            });
        }
        return Ok((
            token_principal(&rec, PrincipalKind::Session),
            Credential::Session(hash),
        ));
    }
    if state.cfg.file.auth.mode == h2s_api::AuthMode::Proxy {
        let p = &state.cfg.file.auth.proxy;
        if let Some(user) = headers.get(p.user_header.as_str()) {
            return proxy_principal(state, headers, meta, user)
                .map(|principal| (principal, Credential::Proxy))
                .map_err(|why| Failed {
                    why,
                    target: user
                        .to_str()
                        .ok()
                        .map(|u| crate::util::truncate_chars(u, 100)),
                });
        }
    }
    Err(AuthFailure::Missing.into())
}

fn proxy_principal(
    state: &AppState,
    headers: &http::HeaderMap,
    meta: &RequestMeta,
    user: &http::HeaderValue,
) -> Result<Principal, AuthFailure> {
    let p = &state.cfg.file.auth.proxy;
    if !meta.peer_is_trusted_proxy {
        return Err(AuthFailure::ProxyUntrusted);
    }
    // The proxy sets each of these once and strips what the client sent. A repeated header
    // means a client value got through: taking the first user or the union of the groups
    // would let it choose its identity or add itself to an admin group.
    let repeated = [&p.user_header, &p.groups_header, &p.secret_header]
        .into_iter()
        .any(|name| headers.get_all(name.as_str()).iter().nth(1).is_some());
    if repeated {
        return Err(AuthFailure::ProxyDuplicate);
    }
    let presented = headers
        .get(p.secret_header.as_str())
        .map(http::HeaderValue::as_bytes)
        .unwrap_or_default();
    let expected = state.proxy_secret.as_ref().map(|s| s.as_bytes());
    match expected {
        Some(exp) if !exp.is_empty() && crate::auth::ct_eq(presented, exp) => {}
        _ => return Err(AuthFailure::ProxySecret),
    }
    let user = user.to_str().map_err(|_| AuthFailure::ProxyUser)?.trim();
    if user.is_empty() || user.len() > 256 || user.chars().any(char::is_control) {
        return Err(AuthFailure::ProxyUser);
    }
    let groups: Vec<String> = headers
        .get_all(p.groups_header.as_str())
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|g| g.trim().to_owned())
        .filter(|g| !g.is_empty())
        .collect();
    let any = |list: &[String]| groups.iter().any(|g| list.contains(g));
    let role = if any(&p.admin_groups) {
        Role::Admin
    } else if any(&p.operator_groups) {
        Role::Operator
    } else if p.viewer_groups.is_empty() || any(&p.viewer_groups) {
        Role::Viewer
    } else {
        return Err(AuthFailure::ProxyNoRole);
    };
    Ok(Principal {
        id: user.to_owned(),
        name: user.to_owned(),
        role,
        kind: PrincipalKind::Proxy,
    })
}

/// Authenticate the request (cached per request).
///
/// # Errors
/// 401 (or 403 for a proxy user without a role); every failure is counted. Failures with
/// credentials are audited up to [`AUDITED_FAILURES_PER_MINUTE`] per client, and past
/// [`AUTH_FAILURES_PER_MINUTE`] per client they are answered 429.
pub async fn authenticate(parts: &mut Parts, state: &AppState) -> Result<Principal, ApiError> {
    if let Some(p) = parts.extensions.get::<Principal>() {
        return Ok(p.clone());
    }
    let meta = RequestMeta::from_request_parts(parts, state).await?;
    match resolve(state, &parts.headers, &meta).await {
        Ok((p, credential)) => {
            parts.extensions.insert(p.clone());
            parts.extensions.insert(credential);
            Ok(p)
        }
        Err(Failed { why, target }) => {
            if !why.presented() {
                state.metrics.auth_failure(why.reason());
                return Err(why.into_error());
            }
            let key = meta
                .client_ip
                .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
            if let Err(wait) = state.auth_failures.check(key) {
                state.metrics.auth_failure("rate_limited");
                return Err(ApiError::rate_limited(wait.as_secs().max(1)));
            }
            state.metrics.auth_failure(why.reason());
            let entry = meta
                .audit(
                    "anonymous",
                    "auth.failed",
                    target.as_deref().unwrap_or("-"),
                    AuditOutcome::Denied,
                )
                .detail(format!("{} on {}", why.reason(), parts.uri.path()));
            state.audit_failure(meta.client_ip, entry).await;
            Err(why.into_error())
        }
    }
}

/// Requirement of an [`Auth`] extractor.
pub trait Need: Send + Sync + 'static {
    /// `None` = any authenticated principal.
    const ROLE: Option<Role>;
}

/// Any authenticated principal.
#[derive(Debug)]
pub enum AnyRole {}
/// At least `viewer`.
#[derive(Debug)]
pub enum ViewerRole {}
/// At least `operator`.
#[derive(Debug)]
pub enum OperatorRole {}
/// `admin`.
#[derive(Debug)]
pub enum AdminRole {}

impl Need for AnyRole {
    const ROLE: Option<Role> = None;
}
impl Need for ViewerRole {
    const ROLE: Option<Role> = Some(Role::Viewer);
}
impl Need for OperatorRole {
    const ROLE: Option<Role> = Some(Role::Operator);
}
impl Need for AdminRole {
    const ROLE: Option<Role> = Some(Role::Admin);
}

/// The authenticated caller, holding at least `R`'s role.
#[derive(Debug, Clone)]
pub struct Auth<R: Need> {
    pub principal: Principal,
    /// What authenticated the request (re-checked by event streams).
    pub credential: Credential,
    pub meta: RequestMeta,
    _need: PhantomData<fn() -> R>,
}

impl<R: Need> Auth<R> {
    /// The caller as a job-engine actor.
    #[must_use]
    pub fn actor(&self) -> Actor {
        Actor {
            principal: self.principal.clone(),
            ip: self.meta.ip(),
            request_id: Some(self.meta.request_id.clone()),
        }
    }

    /// An audit entry by this caller.
    #[must_use]
    pub fn audit(&self, action: &str, target: &str, outcome: AuditOutcome) -> AuditEntry {
        self.meta
            .audit(&self.principal.name, action, target, outcome)
    }

    #[must_use]
    pub fn is_admin(&self) -> bool {
        self.principal.role == Role::Admin
    }
}

/// Refuse with 403 and record the denial.
pub(crate) async fn deny(
    state: &AppState,
    principal: &Principal,
    meta: &RequestMeta,
    route: &str,
    needed: Role,
) -> ApiError {
    state.metrics.auth_failure("forbidden");
    let entry = meta
        .audit(
            &principal.name,
            "access.denied",
            route,
            AuditOutcome::Denied,
        )
        .detail(format!(
            "{} holds {}; this needs {}",
            principal.name,
            principal.role.as_str(),
            needed.as_str()
        ));
    state.audit(entry).await;
    ApiError::forbidden(format!("this requires the {} role", needed.as_str()))
}

impl<R: Need> FromRequestParts<AppState> for Auth<R> {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let principal = authenticate(parts, state).await?;
        let meta = RequestMeta::from_request_parts(parts, state).await?;
        if let Some(role) = R::ROLE
            && !principal.role.allows(role)
        {
            let route = format!("{} {}", parts.method, parts.uri.path());
            return Err(deny(state, &principal, &meta, &route, role).await);
        }
        let credential = parts
            .extensions
            .get::<Credential>()
            .cloned()
            .ok_or_else(|| ApiError::internal("the request lost its credential"))?;
        Ok(Auth {
            principal,
            credential,
            meta,
            _need: PhantomData,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_parsing() {
        let mut h = http::HeaderMap::new();
        assert!(bearer(&h).is_none());
        h.insert(http::header::AUTHORIZATION, "Bearer abc".parse().unwrap());
        assert_eq!(bearer(&h).unwrap().unwrap(), "abc");
        h.insert(
            http::header::AUTHORIZATION,
            "bearer   xyz ".parse().unwrap(),
        );
        assert_eq!(bearer(&h).unwrap().unwrap(), "xyz");
        h.insert(http::header::AUTHORIZATION, "Basic abc".parse().unwrap());
        assert_eq!(bearer(&h).unwrap(), Err(AuthFailure::Malformed));
        h.insert(http::header::AUTHORIZATION, "Bearer".parse().unwrap());
        assert_eq!(bearer(&h).unwrap(), Err(AuthFailure::Malformed));
    }

    #[test]
    fn session_cookie_parsing() {
        let mut h = http::HeaderMap::new();
        h.append(
            http::header::COOKIE,
            "a=1; h2s_session=abc; b=2".parse().unwrap(),
        );
        assert_eq!(session_cookie(&h, "h2s_session").as_deref(), Some("abc"));
        let mut h = http::HeaderMap::new();
        h.append(http::header::COOKIE, "a=1".parse().unwrap());
        h.append(http::header::COOKIE, "h2s_session=zz".parse().unwrap());
        assert_eq!(session_cookie(&h, "h2s_session").as_deref(), Some("zz"));
        // Over HTTPS only the __Host- name counts.
        assert_eq!(session_cookie(&h, SECURE_SESSION_COOKIE), None);
        h.append(
            http::header::COOKIE,
            "__Host-h2s_session=hh".parse().unwrap(),
        );
        assert_eq!(
            session_cookie(&h, SECURE_SESSION_COOKIE).as_deref(),
            Some("hh")
        );
        assert_eq!(session_cookie_name(true), "__Host-h2s_session");
        assert_eq!(session_cookie_name(false), "h2s_session");
    }

    #[tokio::test]
    async fn length_limit_detection() {
        use http_body_util::BodyExt;
        let body = http_body_util::Limited::new(
            http_body_util::Full::new(bytes::Bytes::from(vec![0u8; 64])),
            8,
        );
        let e = body.collect().await.unwrap_err();
        assert!(is_length_limit(&*e));
        let other = std::io::Error::other("x");
        let err: &(dyn std::error::Error + 'static) = &other;
        assert!(!is_length_limit(err));
    }
}
