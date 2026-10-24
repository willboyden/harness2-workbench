//! `POST /auth/login`, `POST /auth/logout`, `GET /auth/me`.

use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use cookie::{Cookie, SameSite};
use h2w_api::{AuditOutcome, ErrorBody, LoginRequest, Principal, PrincipalKind};
use http::{HeaderMap, HeaderValue, StatusCode, header};

use crate::error::ApiError;
use crate::http::AppState;
use crate::http::extract::{
    AnyRole, ApiJson, Auth, RequestMeta, session_cookie, session_cookie_name, verify_token,
};
use crate::util::now_unix;

/// `Set-Cookie` for the session: HttpOnly, SameSite=Strict, Path=/, never a Domain; over
/// HTTPS `Secure` and named `__Host-h2w_session`.
fn session_cookie_header(state: &AppState, value: &str, max_age: i64) -> Option<HeaderValue> {
    let https = state.cfg.public_https;
    let c = Cookie::build((session_cookie_name(https), value.to_owned()))
        .http_only(true)
        .same_site(SameSite::Strict)
        .path("/")
        .secure(https)
        .max_age(time::Duration::seconds(max_age))
        .build();
    HeaderValue::from_str(&c.to_string()).ok()
}

/// Exchange a token for a session cookie (rate-limited per client address).
#[utoipa::path(post, path = "/api/v1/auth/login", tag = "auth", security(()),
    request_body = LoginRequest,
    responses((status = 200, body = Principal, description = "Signed in; sets the session cookie (h2w_session, or __Host-h2w_session when the server's public URL is https)"),
        (status = "default", description = "Error", body = ErrorBody)))]
pub async fn login(
    State(state): State<AppState>,
    meta: RequestMeta,
    body: Result<ApiJson<LoginRequest>, ApiError>,
) -> Result<Response, ApiError> {
    if let Some(ip) = meta.client_ip
        && let Err(wait) = state.login_limiter.check(ip)
    {
        state.metrics.auth_failure("rate_limited");
        let secs = wait.as_secs().max(1);
        let entry = meta
            .audit("anonymous", "auth.login", "-", AuditOutcome::Denied)
            .detail("rate limited");
        state.audit_failure(meta.client_ip, entry).await;
        return Err(ApiError::rate_limited(secs));
    }
    let ApiJson(req) = body?;
    let rec = match verify_token(&state, req.token.trim()).await {
        Ok(rec) => rec,
        Err(why) => {
            state.metrics.auth_failure(why.reason());
            let target = crate::auth::token_id_hint(req.token.trim()).unwrap_or("-");
            let entry = meta
                .audit("anonymous", "auth.login", target, AuditOutcome::Denied)
                .detail(why.reason());
            state.audit_failure(meta.client_ip, entry).await;
            return Err(ApiError::unauthorized("invalid token"));
        }
    };
    let (sid, hash) = crate::auth::new_session().map_err(|e| {
        tracing::error!(error = %e, "no randomness for a session id");
        ApiError::internal("could not create a session")
    })?;
    let now = now_unix();
    let ttl = i64::try_from(state.cfg.session_ttl.as_secs()).unwrap_or(43_200);
    let expires = rec.expires_at.map_or(now + ttl, |e| e.min(now + ttl));
    let token_id = rec.id.clone();
    let entry = meta.audit(
        &format!("token:{}", rec.name),
        "auth.login",
        &rec.id,
        AuditOutcome::Success,
    );
    state
        .db
        .call(move |c| {
            crate::db::write_tx(c, |tx| {
                crate::db::sessions::insert(tx, &hash, &token_id, now, expires)?;
                crate::db::audit::append(tx, &entry)?;
                Ok(())
            })
        })
        .await?;
    let principal = Principal {
        id: rec.id.clone(),
        name: rec.name.clone(),
        role: rec.role,
        kind: PrincipalKind::Session,
    };
    let mut resp = Json(principal).into_response();
    if let Some(h) = session_cookie_header(&state, &sid, (expires - now).max(1)) {
        resp.headers_mut().insert(header::SET_COOKIE, h);
    }
    Ok(resp)
}

/// End the browser session (the cookie is cleared either way).
#[utoipa::path(post, path = "/api/v1/auth/logout", tag = "auth",
    responses((status = 204, description = "Signed out"),
        (status = "default", description = "Error", body = ErrorBody)))]
pub async fn logout(
    auth: Auth<AnyRole>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if auth.principal.kind == PrincipalKind::Session
        && let Some(v) = session_cookie(&headers, session_cookie_name(state.cfg.public_https))
        && let Some(hash) = crate::auth::session_hash(&v)
    {
        let entry = auth.audit("auth.logout", &auth.principal.id, AuditOutcome::Success);
        state
            .db
            .call(move |c| {
                crate::db::write_tx(c, |tx| {
                    crate::db::sessions::delete(tx, &hash)?;
                    crate::db::audit::append(tx, &entry)?;
                    Ok(())
                })
            })
            .await?;
    }
    let mut resp = StatusCode::NO_CONTENT.into_response();
    if let Some(h) = session_cookie_header(&state, "", 0) {
        resp.headers_mut().insert(header::SET_COOKIE, h);
    }
    Ok(resp)
}

/// The authenticated caller.
#[utoipa::path(get, path = "/api/v1/auth/me", tag = "auth",
    responses((status = 200, body = Principal), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn me(auth: Auth<AnyRole>) -> Json<Principal> {
    Json(auth.principal)
}
