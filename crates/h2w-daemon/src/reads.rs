//! Bounds on the server's blocking reads of experiment results and job logs.
//!
//! What a job writes into an experiment directory or its log is not trusted to be small, so
//! every blocking read of it:
//! - runs under one of [`RESULTS_READS`] permits (more wait), and the permit moves into the
//!   blocking task, so a read keeps counting until it really ends;
//! - gets the request's [`ReadCancel`] flag, which is set when the request is gone (it timed
//!   out, or its client left). It is the cancellation flag of the request's
//!   [`ReadBudget`]s ([`ReadCancel::budget`]), so h2w-core stops at its next file or sweep
//!   and the permit comes back; the server's own loops (the sweep listing) check it too;
//! - reads results files within a per-request budget of bytes and sweeps (the constants
//!   below; past them h2w-core answers `TooLarge`, which is 413);
//! - when it produces a response, serializes it inside the permit and refuses one over
//!   [`MAX_RESULTS_RESPONSE`] bytes with 413 ([`json_capped`]).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use h2w_api::{ErrorCode, ExperimentKind};
use h2w_core::CoreError;
use h2w_core::isolation::EXPERIMENT_MOUNT;
use h2w_core::results::{ExperimentDir, ReadBudget};
use http::StatusCode;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::error::ApiError;
use crate::sweeps::SweepCache;

/// Blocking reads of experiment results and job logs that run at once; more wait for a
/// permit (each read is bounded by h2w-core's caps, so this bounds the memory they take
/// together).
pub const RESULTS_READS: usize = 4;

/// Largest response body a results route builds (413 past it). harness2's own trees and
/// diffs are a few MiB; a hostile one of control characters would serialize to six times
/// its size.
pub const MAX_RESULTS_RESPONSE: usize = 64 * 1024 * 1024;

const MIB: u64 = 1024 * 1024;

/// Read budget of `GET /experiments` (all experiments together): bytes of results files.
/// Sweeps the cache already holds cost nothing.
pub const LIST_BYTES: u64 = 512 * MIB;
/// ... and sweeps.
pub const LIST_SWEEPS: u32 = 1_000;
/// One experiment's view (`GET /experiments/{exp}`, create, update) and an estimate's
/// history: bytes (its sweeps: at most `sweeps::MAX_SWEEPS_LISTED`).
pub const VIEW_BYTES: u64 = 256 * MIB;
/// The task rows of one sweep (its summaries and judged file).
pub const TASKS_BYTES: u64 = 256 * MIB;
/// Scores and the report of one sweep (its two judged files, 32 MiB each at most).
pub const SCORES_BYTES: u64 = 64 * MIB;
/// A comparison: the scores and task rows of two sweeps (four sweep reads).
pub const COMPARE_BYTES: u64 = 2 * (TASKS_BYTES + SCORES_BYTES);
/// The manifest's results digest: every summary and judged file, and namespaces.
pub const DIGEST_BYTES: u64 = 1024 * MIB;
/// ... and namespaces.
pub const DIGEST_SWEEPS: u32 = 1_000;
/// One poll of the budget guard, and one background refresh of a cached sweep.
pub const SWEEP_BYTES: u64 = 256 * MIB;

/// Set when the request that started a read is gone: stop at the next opportunity (the
/// result would be thrown away).
#[derive(Debug, Clone, Default)]
pub struct ReadCancel(Arc<AtomicBool>);

impl ReadCancel {
    /// A flag for one request and the guard that sets it when the request's future is
    /// dropped (keep the guard in the async handler).
    #[must_use]
    pub fn for_request() -> (ReadCancel, CancelOnDrop) {
        let c = ReadCancel::default();
        (c.clone(), CancelOnDrop(c))
    }

    /// The request has gone away.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// Mark the read as abandoned.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// A read budget of `bytes` and `sweeps` that stops when this flag is set.
    #[must_use]
    pub fn budget(&self, bytes: u64, sweeps: u32) -> ReadBudget {
        ReadBudget::new(bytes, sweeps).with_cancel(Arc::clone(&self.0))
    }
}

/// Cancels its request's reads when dropped: the request's future holds it, so a request
/// dropped by its timeout (or a closed connection) cancels the blocking reads it started.
#[derive(Debug)]
pub struct CancelOnDrop(ReadCancel);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// `e` says the request's budget ran out or the request was cancelled (not that the data is
/// bad): such a result must not be cached as the sweep's state. The texts are h2w-core's
/// budget messages; a test pins them.
#[must_use]
pub fn spent(e: &CoreError) -> bool {
    match e {
        CoreError::Cancelled => true,
        CoreError::TooLarge(m) => m.starts_with("this request may read at most"),
        _ => false,
    }
}

/// Permits for blocking reads, and the sweep cache that bounds experiment listings.
#[derive(Debug)]
pub struct Reads {
    /// [`RESULTS_READS`] permits.
    pub permits: Arc<Semaphore>,
    /// Sweep summaries for the experiment views and estimates.
    pub sweeps: SweepCache,
}

impl Reads {
    #[must_use]
    pub fn new() -> Arc<Reads> {
        Arc::new(Reads {
            permits: Arc::new(Semaphore::new(RESULTS_READS)),
            sweeps: SweepCache::default(),
        })
    }

    /// Wait for a permit (for work that is not one blocking closure).
    ///
    /// # Errors
    /// 503 when the server is shutting down.
    pub async fn permit(&self) -> Result<OwnedSemaphorePermit, ApiError> {
        Arc::clone(&self.permits)
            .acquire_owned()
            .await
            .map_err(|_| ApiError::unavailable("the server is shutting down"))
    }

    /// A permit if one is free now (background work never waits for one).
    #[must_use]
    pub fn try_permit(&self) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.permits).try_acquire_owned().ok()
    }

    /// Run a blocking read under one permit. `f` gets a [`ReadCancel`] that is set when the
    /// caller's future is dropped.
    ///
    /// # Errors
    /// What `f` returns; 503 when the server is shutting down.
    pub async fn run<T, F>(&self, f: F) -> Result<T, ApiError>
    where
        T: Send + 'static,
        F: FnOnce(&ReadCancel) -> Result<T, ApiError> + Send + 'static,
    {
        let (cancel, _guard) = ReadCancel::for_request();
        self.run_in(&cancel, f).await
    }

    /// [`Reads::run`] with the request's own flag (for a request that runs several reads:
    /// it holds the [`CancelOnDrop`] guard itself).
    ///
    /// # Errors
    /// What `f` returns; 503 when the server is shutting down or the request is gone.
    pub async fn run_in<T, F>(&self, cancel: &ReadCancel, f: F) -> Result<T, ApiError>
    where
        T: Send + 'static,
        F: FnOnce(&ReadCancel) -> Result<T, ApiError> + Send + 'static,
    {
        let permit = self.permit().await?;
        let cancel = cancel.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            if cancel.is_cancelled() {
                return Err(ApiError::unavailable("the request was abandoned"));
            }
            f(&cancel)
        })
        .await?
    }

    /// The sweeps of an experiment, through the cache (see [`crate::sweeps`]); misses are
    /// read within `budget`. Blocking.
    #[must_use]
    pub fn sweeps_of(
        self: &Arc<Self>,
        experiment: &str,
        path: &Path,
        kind: ExperimentKind,
        dir: &ExperimentDir,
        cancel: &ReadCancel,
        budget: &mut ReadBudget,
    ) -> Vec<h2w_api::SweepSummary> {
        self.sweeps
            .list(self, experiment, path, kind, dir, cancel, budget)
    }
}

/// Open an experiment's results read-only (the host path and the container mount point as
/// roots).
///
/// # Errors
/// What h2w-core refuses (missing, symlinked, unreadable root).
pub fn open_experiment(path: &Path, kind: ExperimentKind) -> Result<ExperimentDir, ApiError> {
    ExperimentDir::open(path, kind, &[EXPERIMENT_MOUNT.to_owned()]).map_err(ApiError::from)
}

/// 413 for a response over [`MAX_RESULTS_RESPONSE`].
fn response_too_large() -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        ErrorCode::PayloadTooLarge,
        format!(
            "too large: the response would be larger than {} MiB",
            MAX_RESULTS_RESPONSE / (1024 * 1024)
        ),
    )
}

/// A writer that refuses to grow past a cap (serialization stops there).
struct CappedWriter {
    buf: Vec<u8>,
    cap: usize,
    over: bool,
}

impl std::io::Write for CappedWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if self.buf.len().saturating_add(data.len()) > self.cap {
            self.over = true;
            return Err(std::io::Error::other("response cap reached"));
        }
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// `value` as JSON, or 413 when it would exceed [`MAX_RESULTS_RESPONSE`] (the
/// serialization stops at the cap, so no more is ever held).
///
/// # Errors
/// 413 over the cap; 500 when serialization fails otherwise.
pub fn json_capped<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, ApiError> {
    json_capped_at(value, MAX_RESULTS_RESPONSE)
}

fn json_capped_at<T: serde::Serialize>(value: &T, cap: usize) -> Result<Vec<u8>, ApiError> {
    let mut w = CappedWriter {
        buf: Vec::new(),
        cap,
        over: false,
    };
    match serde_json::to_writer(&mut w, value) {
        Ok(()) => Ok(w.buf),
        Err(_) if w.over => Err(response_too_large()),
        Err(e) => {
            tracing::error!(error = %e, "could not serialize a results response");
            Err(ApiError::internal("could not serialize the response"))
        }
    }
}

/// `bytes` unless they exceed [`MAX_RESULTS_RESPONSE`] (for non-JSON responses).
///
/// # Errors
/// 413 over the cap.
pub fn capped_bytes(bytes: Vec<u8>) -> Result<Vec<u8>, ApiError> {
    if bytes.len() > MAX_RESULTS_RESPONSE {
        return Err(response_too_large());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_stops_at_the_cap() {
        let small = vec!["x"; 10];
        assert_eq!(
            json_capped_at(&small, 1024).unwrap(),
            serde_json::to_vec(&small).unwrap()
        );
        let big = vec!["\u{1}".repeat(1000); 10];
        let e = json_capped_at(&big, 1024).unwrap_err();
        assert_eq!(
            (e.status, e.code),
            (StatusCode::PAYLOAD_TOO_LARGE, ErrorCode::PayloadTooLarge)
        );
        assert!(capped_bytes(vec![0; 16]).is_ok());
    }

    /// The budget messages [`spent`] recognises are h2w-core's.
    #[test]
    fn spent_budgets_are_recognised() {
        let root = tempfile::tempdir().unwrap();
        let sweep = root.path().join("lab/adapt/lab-oc-agg1/t1");
        std::fs::create_dir_all(&sweep).unwrap();
        std::fs::write(
            sweep.join("summary.json"),
            r#"{"task_id": "t1", "status": "ok"}"#,
        )
        .unwrap();
        let dir = ExperimentDir::open(root.path(), ExperimentKind::Standard, &[]).unwrap();
        let ns = h2w_core::namespace::Namespace::parse("lab-oc-agg1").unwrap();
        let cancel = ReadCancel::default();
        let bytes = dir.sweep_with(&ns, &mut cancel.budget(1, 10)).unwrap_err();
        assert!(spent(&bytes), "{bytes}");
        let sweeps = dir
            .sweep_with(&ns, &mut cancel.budget(1 << 20, 0))
            .unwrap_err();
        assert!(spent(&sweeps), "{sweeps}");
        cancel.cancel();
        let cancelled = dir
            .sweep_with(&ns, &mut cancel.budget(1 << 20, 10))
            .unwrap_err();
        assert!(spent(&cancelled), "{cancelled}");
        assert!(!spent(&CoreError::TooLarge(
            "judged.json has more than 9 rows".into()
        )));
    }

    #[tokio::test]
    async fn a_dropped_request_cancels_its_read() {
        let reads = Reads::new();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (seen_tx, seen_rx) = std::sync::mpsc::channel();
        let r = Arc::clone(&reads);
        let request = tokio::spawn(async move {
            r.run(move |cancel| {
                let _ = started_tx.send(());
                // A long read that checks its flag.
                while !cancel.is_cancelled() {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                let _ = seen_tx.send(());
                Ok(())
            })
            .await
        });
        tokio::task::spawn_blocking(move || started_rx.recv().unwrap())
            .await
            .unwrap();
        assert_eq!(reads.permits.available_permits(), RESULTS_READS - 1);
        // The request times out: its future is dropped.
        request.abort();
        tokio::task::spawn_blocking(move || {
            seen_rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("the read was not cancelled");
        })
        .await
        .unwrap();
        // The permit comes back once the read has really ended.
        for _ in 0..200 {
            if reads.permits.available_permits() == RESULTS_READS {
                return;
            }
            tokio::task::yield_now().await;
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the permit was not released");
    }
}
