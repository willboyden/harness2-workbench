//! Route handlers. Every handler takes its `Auth<...>` extractor first (or is public by
//! design: health, readiness, login), so the role check runs before any other extraction.

pub mod admin;
pub mod auth;
pub mod experiments;
pub mod jobs;
pub mod results;
pub mod root;
pub mod system;
pub mod tools;

use axum::response::{IntoResponse, Response};
use http::{HeaderValue, header};

use crate::error::ApiError;
use crate::http::AppState;
use crate::reads::ReadCancel;

/// Run a blocking read of experiment results under one of the server's read permits (see
/// [`crate::reads`]).
pub(crate) async fn results_read<T, F>(state: &AppState, f: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce(&ReadCancel) -> Result<T, ApiError> + Send + 'static,
{
    state.reads.run(f).await
}

/// A results read answered as JSON: serialized inside the read permit, and 413 when the
/// body would exceed [`crate::reads::MAX_RESULTS_RESPONSE`].
pub(crate) async fn results_json<T, F>(state: &AppState, f: F) -> Result<Response, ApiError>
where
    T: serde::Serialize,
    F: FnOnce(&ReadCancel) -> Result<T, ApiError> + Send + 'static,
{
    let body = state
        .reads
        .run(move |cancel| crate::reads::json_capped(&f(cancel)?))
        .await?;
    Ok(json_body(body))
}

/// A JSON response from already serialized bytes.
pub(crate) fn json_body(body: Vec<u8>) -> Response {
    let mut r = body.into_response();
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    r
}

/// Run a blocking closure (filesystem, results parsing) off the async runtime.
pub(crate) async fn blocking<T, F>(f: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, ApiError> + Send + 'static,
{
    tokio::task::spawn_blocking(f).await?
}

/// Validate a name with an `h2w_core::names` rule, as a 422 on `field`.
pub(crate) fn check_name(field: &str, r: h2w_core::Result<()>) -> Result<(), ApiError> {
    match r {
        Ok(()) => Ok(()),
        Err(h2w_core::CoreError::Invalid { problem, .. }) => Err(ApiError::invalid(field, problem)),
        Err(e) => Err(e.into()),
    }
}

/// Clamp a page size.
pub(crate) fn clamp_limit(limit: Option<u32>, default: u32, max: u32) -> u32 {
    limit.unwrap_or(default).clamp(1, max)
}
