//! Jobs: create, list, read, cancel, page the log, stream events (SSE).
//!
//! Event frames are unnamed (no `event:` field, contract addendum v1.1): each `data:` is
//! exactly one [`JobEvent`] JSON, so `EventSource.onmessage` sees every frame. `log`
//! events carry `id: <line number>`; a reconnecting client's `Last-Event-ID` replays the
//! lines it missed. The first frame is always a `snapshot`; a `heartbeat` follows at least
//! every 15 s; the stream ends after the terminal `status` event, after an idle period,
//! after a maximum lifetime, or at server shutdown.

use std::convert::Infallible;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use h2s_api::{ErrorBody, Job, JobEvent, JobKind, JobSpec, JobStatus, LogChunk, LogLine, Page};
use http::{HeaderMap, HeaderValue, StatusCode};
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;

use super::clamp_limit;
use crate::db::jobs::{JobFilter, parse_enum};
use crate::error::ApiError;
use crate::http::AppState;
use crate::http::extract::{ApiJson, ApiQuery, Auth, OperatorRole, ViewerRole};

/// Heartbeat interval of event streams.
pub const HEARTBEAT: Duration = Duration::from_secs(15);
/// A stream with no events but heartbeats for this long is closed (clients reconnect).
pub const IDLE_CLOSE: Duration = Duration::from_secs(600);
/// Longest life of one stream.
pub const MAX_STREAM: Duration = Duration::from_secs(3600);

fn job_id(id: &str) -> Result<String, ApiError> {
    uuid::Uuid::parse_str(id)
        .map(|u| u.hyphenated().to_string())
        .map_err(|_| ApiError::not_found(format!("no job {id}")))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct ListQuery {
    /// `queued`, `running`, `succeeded`, `partial`, `failed`, `cancelled`.
    pub status: Option<String>,
    /// A `JobKind` (`run`, `demo`, ...).
    pub kind: Option<String>,
    pub experiment: Option<String>,
    /// Page size (1..=500, default 50).
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

/// Jobs, newest first.
#[utoipa::path(get, path = "/api/v1/jobs", tag = "jobs", params(ListQuery),
    responses((status = 200, body = ref("#/components/schemas/JobPage")), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn list(
    _auth: Auth<ViewerRole>,
    State(state): State<AppState>,
    ApiQuery(q): ApiQuery<ListQuery>,
) -> Result<Json<Page<Job>>, ApiError> {
    let status: Option<JobStatus> = match &q.status {
        Some(s) => Some(parse_enum(s).ok_or_else(|| {
            ApiError::invalid(
                "status",
                "must be queued, running, succeeded, partial, failed or cancelled",
            )
        })?),
        None => None,
    };
    let kind: Option<JobKind> = match &q.kind {
        Some(k) => Some(
            parse_enum(k)
                .ok_or_else(|| ApiError::invalid("kind", format!("unknown job kind {k}")))?,
        ),
        None => None,
    };
    if let Some(e) = &q.experiment {
        super::check_name("experiment", h2s_core::names::experiment_name(e))?;
    }
    let filter = JobFilter {
        status,
        kind,
        experiment: q.experiment.clone(),
    };
    let limit = clamp_limit(q.limit, 50, 500);
    let page = state
        .jobs
        .list(filter, limit, q.cursor.as_deref())
        .await
        .map_err(|e| {
            if e.message == "invalid cursor" {
                ApiError::invalid("cursor", "invalid cursor")
            } else {
                e
            }
        })?;
    Ok(Json(page))
}

/// Start a job. Needs at least operator, and the role the job kind requires.
#[utoipa::path(post, path = "/api/v1/jobs", tag = "jobs", request_body = JobSpec,
    responses((status = 201, body = ref("#/components/schemas/Job")), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn create(
    auth: Auth<OperatorRole>,
    State(state): State<AppState>,
    ApiJson(spec): ApiJson<JobSpec>,
) -> Result<Response, ApiError> {
    let job = state.jobs.create(spec, &auth.actor()).await?;
    Ok((StatusCode::CREATED, Json(job)).into_response())
}

/// One job.
#[utoipa::path(get, path = "/api/v1/jobs/{id}", tag = "jobs",
    params(("id" = String, Path, description = "Job id")),
    responses((status = 200, body = ref("#/components/schemas/Job")), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn get_one(
    _auth: Auth<ViewerRole>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Job>, ApiError> {
    Ok(Json(state.jobs.get(&job_id(&id)?).await?))
}

/// Cancel a queued or running job (409 when it already finished). No request body.
#[utoipa::path(post, path = "/api/v1/jobs/{id}/cancel", tag = "jobs",
    params(("id" = String, Path, description = "Job id")),
    responses((status = 200, body = ref("#/components/schemas/Job")), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn cancel(
    auth: Auth<OperatorRole>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Job>, ApiError> {
    let id = job_id(&id)?;
    Ok(Json(state.jobs.cancel(&id, &auth.actor()).await?))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct LogQuery {
    /// First line number (0-based).
    pub from: Option<u64>,
    /// Lines to return (1..=5000, default 500).
    pub limit: Option<u64>,
}

/// A page of the job's log.
#[utoipa::path(get, path = "/api/v1/jobs/{id}/log", tag = "jobs",
    params(("id" = String, Path, description = "Job id"), LogQuery),
    responses((status = 200, body = LogChunk), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn log(
    _auth: Auth<ViewerRole>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiQuery(q): ApiQuery<LogQuery>,
) -> Result<Json<LogChunk>, ApiError> {
    let limit = q.limit.unwrap_or(500).clamp(1, crate::jobs::LOG_PAGE_MAX);
    Ok(Json(
        state
            .jobs
            .log(&job_id(&id)?, q.from.unwrap_or(0), limit)
            .await?,
    ))
}

/// Holds one of the server's event-stream slots.
struct SseSlot {
    state: AppState,
}

impl SseSlot {
    fn acquire(state: &AppState) -> Result<SseSlot, ApiError> {
        let max = usize::try_from(state.cfg.file.server.sse_max_connections).unwrap_or(64);
        let prev = state.sse_open.fetch_add(1, Ordering::SeqCst);
        if prev >= max {
            state.sse_open.fetch_sub(1, Ordering::SeqCst);
            return Err(ApiError::unavailable(
                "too many open event streams; try again later",
            ));
        }
        state.metrics.sse_connections.inc();
        Ok(SseSlot {
            state: state.clone(),
        })
    }
}

impl Drop for SseSlot {
    fn drop(&mut self) {
        self.state.sse_open.fetch_sub(1, Ordering::SeqCst);
        self.state.metrics.sse_connections.dec();
    }
}

fn frame(e: &JobEvent) -> Event {
    let ev = Event::default()
        .json_data(e)
        .unwrap_or_else(|_| Event::default().data("{}"));
    match e {
        JobEvent::Log { line } => ev.id(line.n.to_string()),
        _ => ev,
    }
}

fn heartbeat() -> Event {
    frame(&JobEvent::Heartbeat {
        ts: crate::util::now_rfc3339(),
    })
}

/// Live job events as server-sent events.
#[utoipa::path(get, path = "/api/v1/jobs/{id}/events", tag = "jobs",
    params(("id" = String, Path, description = "Job id"),
        ("last-event-id" = Option<String>, Header, description = "Replay log lines after this line number")),
    responses((status = 200, description = "text/event-stream; every data frame is one JobEvent", content_type = "text/event-stream"),
        (status = "default", description = "Error", body = ErrorBody)))]
pub async fn events(
    _auth: Auth<ViewerRole>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let id = job_id(&id)?;
    let slot = SseSlot::acquire(&state)?;
    let last_seen: Option<u64> = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse().ok());
    let sub = state.jobs.subscribe(&id).await?;
    let snapshot_lines = sub.snapshot.log_lines;
    let replay: Vec<LogLine> = match last_seen {
        Some(last) if last + 1 < snapshot_lines => {
            let from = last + 1;
            let ring_ok = sub.recent.first().is_some_and(|l| l.n <= from);
            if ring_ok {
                sub.recent
                    .iter()
                    .filter(|l| l.n >= from && l.n < snapshot_lines)
                    .cloned()
                    .collect()
            } else {
                state.jobs.replay(&id, from, snapshot_lines).await
            }
        }
        _ => Vec::new(),
    };
    let mut shutdown = state.shutdown.clone();
    let jobs = state.jobs.clone();
    let stream = async_stream::stream! {
        let _slot = slot;
        let terminal = sub.snapshot.status.is_terminal();
        yield Ok::<Event, Infallible>(frame(&JobEvent::Snapshot { job: Box::new(sub.snapshot) }));
        for line in replay {
            yield Ok(frame(&JobEvent::Log { line }));
        }
        let Some(mut rx) = sub.events.filter(|_| !terminal) else {
            return;
        };
        let mut beat = tokio::time::interval_at(tokio::time::Instant::now() + HEARTBEAT, HEARTBEAT);
        let end = tokio::time::Instant::now() + MAX_STREAM;
        let mut idle = tokio::time::Instant::now() + IDLE_CLOSE;
        loop {
            tokio::select! {
                ev = rx.recv() => match ev {
                    Ok(e) => {
                        idle = tokio::time::Instant::now() + IDLE_CLOSE;
                        let done = matches!(&e, JobEvent::Status { status, .. } if status.is_terminal());
                        yield Ok(frame(&e));
                        if done {
                            break;
                        }
                    }
                    Err(RecvError::Lagged(_)) => {
                        // Too slow: resynchronise with a fresh snapshot (the client pages
                        // the log from its `log_lines`).
                        match jobs.get(&id).await {
                            Ok(job) => {
                                let done = job.status.is_terminal();
                                yield Ok(frame(&JobEvent::Snapshot { job: Box::new(job) }));
                                if done {
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    Err(RecvError::Closed) => break,
                },
                _ = beat.tick() => yield Ok(heartbeat()),
                () = tokio::time::sleep_until(idle) => break,
                () = tokio::time::sleep_until(end) => break,
                _ = shutdown.changed() => break,
            }
        }
    };
    let mut resp = Sse::new(stream).into_response();
    resp.headers_mut()
        .insert("x-accel-buffering", HeaderValue::from_static("no"));
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_ids_are_uuids() {
        let id = uuid::Uuid::now_v7().to_string();
        assert_eq!(job_id(&id).unwrap(), id);
        assert_eq!(job_id(&id.to_uppercase()).unwrap(), id);
        assert!(job_id("../etc/passwd").is_err());
        assert!(job_id("").is_err());
    }
}
