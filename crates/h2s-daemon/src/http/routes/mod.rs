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

use crate::error::ApiError;

/// Run a blocking closure (filesystem, results parsing) off the async runtime.
pub(crate) async fn blocking<T, F>(f: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, ApiError> + Send + 'static,
{
    tokio::task::spawn_blocking(f).await?
}

/// Validate a name with an `h2s_core::names` rule, as a 422 on `field`.
pub(crate) fn check_name(field: &str, r: h2s_core::Result<()>) -> Result<(), ApiError> {
    match r {
        Ok(()) => Ok(()),
        Err(h2s_core::CoreError::Invalid { problem, .. }) => Err(ApiError::invalid(field, problem)),
        Err(e) => Err(e.into()),
    }
}

/// Clamp a page size.
pub(crate) fn clamp_limit(limit: Option<u32>, default: u32, max: u32) -> u32 {
    limit.unwrap_or(default).clamp(1, max)
}
