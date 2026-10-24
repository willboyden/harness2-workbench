//! `/healthz`, `/readyz`, `/metrics`.

use std::time::Duration;

use axum::Json;
use axum::extract::{FromRequestParts, State};
use axum::response::{IntoResponse, Response};
use h2w_api::{ErrorBody, Health, Readiness, Role};
use http::request::Parts;
use http::{HeaderValue, StatusCode, header};

use crate::error::ApiError;
use crate::http::AppState;
use crate::http::extract::{RequestMeta, authenticate, bearer, deny};

/// Liveness: the process answers.
#[utoipa::path(get, path = "/healthz", tag = "system", security(()),
    responses((status = 200, body = Health)))]
pub async fn healthz() -> Json<Health> {
    Json(Health {
        status: "ok".to_owned(),
    })
}

/// Readiness: the database answers and the job runner runs.
#[utoipa::path(get, path = "/readyz", tag = "system", security(()),
    responses((status = 200, body = Readiness), (status = 503, body = Readiness)))]
pub async fn readyz(State(state): State<AppState>) -> Response {
    let mut failing = Vec::new();
    if !state.db.ping(Duration::from_secs(2)).await {
        failing.push("database".to_owned());
    }
    if !state.jobs.is_alive() {
        failing.push("job_runner".to_owned());
    }
    let ready = failing.is_empty();
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(Readiness { ready, failing })).into_response()
}

/// A viewer, or a caller presenting the configured metrics token.
pub struct MetricsAccess;

impl FromRequestParts<AppState> for MetricsAccess {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let secret_name = state.cfg.file.telemetry.metrics_token_secret.clone();
        if !secret_name.is_empty()
            && let Some(Ok(value)) = bearer(&parts.headers)
            && !value.starts_with(h2w_api::TOKEN_PREFIX)
        {
            let expected = state.secrets.get(&secret_name).await?;
            if let Some(exp) = expected
                && !exp.is_empty()
                && crate::auth::ct_eq(value.as_bytes(), exp.as_bytes())
            {
                return Ok(MetricsAccess);
            }
        }
        let principal = authenticate(parts, state).await?;
        if !principal.role.allows(Role::Viewer) {
            let meta = RequestMeta::from_request_parts(parts, state).await?;
            return Err(deny(state, &principal, &meta, "GET /metrics", Role::Viewer).await);
        }
        Ok(MetricsAccess)
    }
}

/// Prometheus metrics (OpenMetrics text).
#[utoipa::path(get, path = "/metrics", tag = "system",
    responses((status = 200, description = "OpenMetrics text", content_type = "application/openmetrics-text"),
        (status = "default", description = "Error", body = ErrorBody)))]
pub async fn metrics(_access: MetricsAccess, State(state): State<AppState>) -> Response {
    state
        .metrics
        .sse_connections
        .set(i64::try_from(state.sse_open.load(std::sync::atomic::Ordering::SeqCst)).unwrap_or(0));
    let mut r = state.metrics.encode().into_response();
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/openmetrics-text; version=1.0.0; charset=utf-8"),
    );
    r
}
