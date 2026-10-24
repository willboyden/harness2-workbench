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

use std::sync::Arc;

use crate::error::ApiError;
use crate::http::AppState;

/// Blocking reads of experiment results that run at once; more wait for a permit (each
/// read is bounded by h2w-core's caps, so this bounds the memory they take together).
pub const RESULTS_READS: usize = 4;

/// Run a blocking read of experiment results under one of the server's
/// [`RESULTS_READS`] permits. The permit moves into the blocking task, so a request that
/// times out still holds it until its read has finished.
pub(crate) async fn results_read<T, F>(state: &AppState, f: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, ApiError> + Send + 'static,
{
    let permit = Arc::clone(&state.results_reads)
        .acquire_owned()
        .await
        .map_err(|_| ApiError::unavailable("the server is shutting down"))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        f()
    })
    .await?
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
