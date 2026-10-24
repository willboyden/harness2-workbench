//! Waiting for a job: follow the server's job event stream (falling back to polling
//! `GET /jobs/{id}`), and report task progress as MCP progress notifications.

use std::time::Duration;

use h2w_api::{Job, JobEvent, JobStatus};
use rmcp::{
    Peer, RoleServer,
    model::{ProgressNotificationParam, ProgressToken},
};
use tokio::time::{Instant, sleep_until};
use tokio_util::sync::CancellationToken;

use crate::{
    client::{ApiClient, ApiError, is_transient},
    sse::SseParser,
    token::Token,
};

/// No bytes at all (not even a keep-alive) for this long means the event stream is dead.
const STREAM_IDLE: Duration = Duration::from_secs(45);
/// A progress notification is sent at least this often while waiting, so clients that reset
/// their request timeout on progress keep the call alive.
const HEARTBEAT: Duration = Duration::from_secs(10);

/// How a wait ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WaitEnd {
    /// The job reached a terminal status.
    Finished,
    /// The time limit passed first; the job keeps running.
    TimedOut,
    /// The MCP client cancelled the call.
    Cancelled,
}

#[derive(Debug)]
pub(crate) struct WaitOutcome {
    pub(crate) job: Job,
    pub(crate) end: WaitEnd,
    pub(crate) waited: Duration,
}

/// Wait for `job_id` for at most `limit`.
///
/// `initial` is the job as already known (the start tools have it), saving one request.
///
/// # Errors
/// Only a failure to read the job at all, or a non-transient error while polling (the job
/// vanished, the token lost access). Transient errors are retried until the deadline.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn wait_for_job(
    api: &ApiClient,
    token: &Token,
    job_id: &str,
    initial: Option<Job>,
    limit: Duration,
    poll: Duration,
    reporter: &mut ProgressReporter,
    ct: &CancellationToken,
) -> Result<WaitOutcome, ApiError> {
    let started = Instant::now();
    let deadline = started + limit;
    let done = |job: Job, end: WaitEnd| WaitOutcome {
        job,
        end,
        waited: started.elapsed(),
    };

    let mut job = match initial {
        Some(job) => job,
        None => api.get::<Job>(token, &["jobs", job_id], &[]).await?,
    };
    reporter.update(&job).await;
    if job.status.is_terminal() {
        return Ok(done(job, WaitEnd::Finished));
    }

    let timing = StreamTiming {
        idle: STREAM_IDLE,
        heartbeat: HEARTBEAT,
    };
    match follow_events(api, token, job_id, &mut job, deadline, timing, reporter, ct).await {
        Follow::Finished => {
            // Events carry status and progress but not the parsed result: fetch the full job.
            let job = refresh(api, token, job_id, job).await;
            return Ok(done(job, WaitEnd::Finished));
        }
        Follow::Deadline => {
            let job = refresh(api, token, job_id, job).await;
            let end = if job.status.is_terminal() {
                WaitEnd::Finished
            } else {
                WaitEnd::TimedOut
            };
            return Ok(done(job, end));
        }
        Follow::Cancelled => return Ok(done(job, WaitEnd::Cancelled)),
        Follow::Unavailable(reason) => {
            tracing::debug!(job_id, %reason, "job event stream unavailable, polling instead");
        }
    }

    loop {
        if ct.is_cancelled() {
            return Ok(done(job, WaitEnd::Cancelled));
        }
        let now = Instant::now();
        if now >= deadline {
            return Ok(done(job, WaitEnd::TimedOut));
        }
        let wake = (now + poll).min(deadline);
        tokio::select! {
            () = ct.cancelled() => return Ok(done(job, WaitEnd::Cancelled)),
            () = sleep_until(wake) => {}
        }
        match api.get::<Job>(token, &["jobs", job_id], &[]).await {
            Ok(fresh) => {
                job = fresh;
                reporter.update(&job).await;
                if job.status.is_terminal() {
                    return Ok(done(job, WaitEnd::Finished));
                }
            }
            Err(e) if is_transient(&e) => {
                tracing::debug!(job_id, error = %e, "transient error while polling a job");
                reporter.update(&job).await;
            }
            Err(e) => return Err(e),
        }
    }
}

/// The full job, or the event-derived state if the server cannot be read right now.
async fn refresh(api: &ApiClient, token: &Token, job_id: &str, fallback: Job) -> Job {
    match api.get::<Job>(token, &["jobs", job_id], &[]).await {
        Ok(job) => job,
        Err(e) => {
            tracing::debug!(job_id, error = %e, "could not refresh the job after its events");
            fallback
        }
    }
}

/// How the event stream is watched.
#[derive(Debug, Clone, Copy)]
struct StreamTiming {
    /// No bytes at all for this long means the stream is dead.
    idle: Duration,
    /// How often a progress heartbeat is considered while the stream is quiet.
    heartbeat: Duration,
}

#[derive(Debug, PartialEq, Eq)]
enum Follow {
    Finished,
    Deadline,
    Cancelled,
    Unavailable(String),
}

#[allow(clippy::too_many_arguments)]
async fn follow_events(
    api: &ApiClient,
    token: &Token,
    job_id: &str,
    job: &mut Job,
    deadline: Instant,
    timing: StreamTiming,
    reporter: &mut ProgressReporter,
    ct: &CancellationToken,
) -> Follow {
    let StreamTiming { idle, heartbeat } = timing;
    let mut response = match api.job_events(token, job_id).await {
        Ok(response) => response,
        Err(e) => return Follow::Unavailable(e.to_string()),
    };
    let mut parser = SseParser::new();
    let mut heartbeat = tokio::time::interval_at(Instant::now() + heartbeat, heartbeat);
    // Only bytes from the server move this forward (not our own heartbeat ticks).
    let mut silent_until = Instant::now() + idle;
    loop {
        let chunk = tokio::select! {
            () = ct.cancelled() => return Follow::Cancelled,
            () = sleep_until(deadline) => return Follow::Deadline,
            () = sleep_until(silent_until) => {
                return Follow::Unavailable(format!(
                    "no event or keep-alive for {} s",
                    idle.as_secs()
                ));
            }
            _ = heartbeat.tick() => {
                reporter.update(job).await;
                continue;
            }
            chunk = response.chunk() => chunk,
        };
        silent_until = Instant::now() + idle;
        let bytes = match chunk {
            Err(e) => return Follow::Unavailable(format!("event stream failed: {e}")),
            Ok(None) => {
                return Follow::Unavailable("the server closed the event stream".into());
            }
            Ok(Some(bytes)) => bytes,
        };
        let frames = match parser.push(&bytes) {
            Ok(frames) => frames,
            Err(e) => return Follow::Unavailable(e.to_string()),
        };
        for data in frames {
            match serde_json::from_str::<JobEvent>(&data) {
                Ok(event) => {
                    if apply(job, event) {
                        reporter.update(job).await;
                    }
                    if job.status.is_terminal() {
                        return Follow::Finished;
                    }
                }
                Err(e) => tracing::debug!(job_id, error = %e, "ignoring an undecodable job event"),
            }
        }
    }
}

/// Fold one event into the job. Returns true when something worth reporting changed.
pub(crate) fn apply(job: &mut Job, event: JobEvent) -> bool {
    match event {
        JobEvent::Snapshot { job: snapshot } => {
            *job = *snapshot;
            true
        }
        JobEvent::Status {
            status,
            exit_code,
            error,
        } => {
            job.status = status;
            job.exit_code = exit_code;
            if error.is_some() {
                job.error = error;
            }
            true
        }
        JobEvent::Progress { progress } => {
            job.progress = Some(progress);
            true
        }
        JobEvent::Step { step } => {
            match job.steps.iter_mut().find(|s| s.name == step.name) {
                Some(existing) => *existing = step,
                None => job.steps.push(step),
            }
            true
        }
        JobEvent::Log { line } => {
            job.log_lines = job.log_lines.max(line.n + 1);
            false
        }
        JobEvent::Heartbeat { .. } => false,
    }
}

/// Sends MCP progress notifications for one tool call.
pub(crate) struct ProgressReporter {
    peer: Option<Peer<RoleServer>>,
    state: ProgressState,
}

impl ProgressReporter {
    /// Report to `peer` under `token`; `None` (the client asked for no progress) disables it.
    pub(crate) fn new(sink: Option<(Peer<RoleServer>, ProgressToken)>) -> Self {
        match sink {
            Some((peer, token)) => Self {
                peer: Some(peer),
                state: ProgressState::new(Some(token)),
            },
            None => Self {
                peer: None,
                state: ProgressState::new(None),
            },
        }
    }

    /// Report `job` if it changed, or if nothing was sent for a while.
    pub(crate) async fn update(&mut self, job: &Job) {
        let Some(param) = self.state.next(job) else {
            return;
        };
        if let Some(peer) = &self.peer
            && let Err(e) = peer.notify_progress(param).await
        {
            tracing::debug!(error = %e, "could not send a progress notification");
        }
    }
}

/// What to report next.
///
/// The MCP spec requires `progress` to increase with every notification. The value is the
/// number of finished tasks (done + failed) plus an offset that keeps it rising when a
/// multi-step job restarts its count (a pipeline's judge step after its run step); status
/// changes and heartbeats without task progress add 0.001.
pub(crate) struct ProgressState {
    token: Option<ProgressToken>,
    offset: f64,
    last_tasks: u32,
    last_value: Option<f64>,
    last_message: Option<String>,
    last_sent: Option<Instant>,
}

impl ProgressState {
    pub(crate) fn new(token: Option<ProgressToken>) -> Self {
        Self {
            token,
            offset: 0.0,
            last_tasks: 0,
            last_value: None,
            last_message: None,
            last_sent: None,
        }
    }

    /// The notification to send for `job` now, if any (updates the state).
    pub(crate) fn next(&mut self, job: &Job) -> Option<ProgressNotificationParam> {
        let token = self.token.clone()?;
        let (tasks, total) = match &job.progress {
            Some(p) => (p.done.saturating_add(p.failed), p.total),
            None => (0, None),
        };
        if tasks < self.last_tasks {
            self.offset += f64::from(self.last_tasks);
        }
        self.last_tasks = tasks;

        let message = describe(job);
        let changed = self.last_message.as_deref() != Some(message.as_str());
        let due = self.last_sent.is_none_or(|at| at.elapsed() >= HEARTBEAT);
        if !changed && !due {
            return None;
        }
        let base = self.offset + f64::from(tasks);
        // A finished job needs no "status changed" notification: the tool result follows at
        // once, and the value would have to creep past the task total.
        if job.status.is_terminal() && self.last_value.is_some_and(|last| base <= last) {
            return None;
        }
        let value = match self.last_value {
            Some(last) if base <= last => ((last + 0.001) * 1000.0).round() / 1000.0,
            _ => base,
        };
        let mut param = ProgressNotificationParam::new(token, value).with_message(message.clone());
        if let Some(total) = total.map(|t| self.offset + f64::from(t))
            && value <= total
        {
            param = param.with_total(total);
        }
        self.last_value = Some(value);
        self.last_message = Some(message);
        self.last_sent = Some(Instant::now());
        Some(param)
    }
}

/// One line describing where a job is, e.g.
/// `run running · step run · 12/40 tasks done, 1 failed · current: legal / nda-03`.
pub(crate) fn describe(job: &Job) -> String {
    let mut parts = vec![format!("{} {}", job.kind.as_str(), job.status.as_str())];
    if let Some(step) = job
        .steps
        .iter()
        .rev()
        .find(|s| s.status == JobStatus::Running)
    {
        parts.push(format!("step {}", step.name));
    }
    if let Some(p) = &job.progress {
        let mut tasks = match p.total {
            Some(total) => format!("{}/{total} tasks done", p.done),
            None => format!("{} tasks done", p.done),
        };
        if p.failed > 0 {
            tasks.push_str(&format!(", {} failed", p.failed));
        }
        parts.push(tasks);
        if let Some(current) = &p.current {
            parts.push(format!("current: {current}"));
        }
    }
    if let Some(usd) = job.spent_usd {
        parts.push(format!("${usd:.2} recorded spend"));
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use h2w_api::{
        Bench, Cell, CheckParams, Isolation, JobKind, JobSpec, JobStep, LogLine, LogStream,
        Progress, Substrate,
    };
    use rmcp::model::NumberOrString;

    use super::*;

    fn job(status: JobStatus, progress: Option<Progress>) -> Job {
        Job {
            id: "j1".into(),
            kind: JobKind::Check,
            spec: JobSpec::Check(CheckParams {
                cell: Cell {
                    bench: Bench::Lab,
                    subset: None,
                    substrate: Substrate::Oc,
                },
                profile: None,
            }),
            status,
            created_by: "alice".into(),
            created_at: "2026-10-23T00:00:00Z".into(),
            started_at: None,
            finished_at: None,
            exit_code: None,
            isolation: Isolation::Host,
            steps: vec![],
            progress,
            result: None,
            error: None,
            log_lines: 0,
            spent_usd: None,
        }
    }

    fn progress(done: u32, failed: u32, total: Option<u32>, current: &str) -> Option<Progress> {
        Some(Progress {
            done,
            failed,
            total,
            current: Some(current.into()),
        })
    }

    fn state() -> ProgressState {
        ProgressState::new(Some(ProgressToken(NumberOrString::String("t".into()))))
    }

    /// A server that answers the event stream with headers, then never sends a byte.
    async fn silent_event_stream() -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let _ = socket.read(&mut buf).await;
                    let _ = socket
                        .write_all(
                            b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
                              transfer-encoding: chunked\r\n\r\n",
                        )
                        .await;
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    drop(socket);
                });
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn a_silent_stream_is_abandoned_despite_heartbeats() {
        let url = silent_event_stream().await;
        let api = ApiClient::new(&url, &crate::client::ClientOptions::default()).unwrap();
        let token = Token::parse("h2w_alice_s3cret").unwrap();
        let mut j = job(JobStatus::Running, None);
        let mut reporter = ProgressReporter::new(None);
        let started = Instant::now();
        let follow = follow_events(
            &api,
            &token,
            "j1",
            &mut j,
            started + Duration::from_secs(10),
            StreamTiming {
                idle: Duration::from_millis(400),
                heartbeat: Duration::from_millis(100),
            },
            &mut reporter,
            &CancellationToken::new(),
        )
        .await;
        assert!(
            matches!(&follow, Follow::Unavailable(why) if why.contains("no event")),
            "{follow:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn progress_values_always_increase() {
        let mut r = state();
        let mut values = Vec::new();
        let mut push = |r: &mut ProgressState, j: &Job| {
            if let Some(p) = r.next(j) {
                values.push((p.progress, p.total));
            }
        };
        push(&mut r, &job(JobStatus::Queued, None));
        push(&mut r, &job(JobStatus::Running, None)); // status change, no tasks yet
        push(
            &mut r,
            &job(JobStatus::Running, progress(1, 0, Some(4), "a")),
        );
        push(
            &mut r,
            &job(JobStatus::Running, progress(1, 0, Some(4), "a")),
        ); // unchanged
        push(
            &mut r,
            &job(JobStatus::Running, progress(2, 1, Some(4), "b")),
        );
        // A new step restarts the count: values keep rising.
        push(
            &mut r,
            &job(JobStatus::Running, progress(0, 0, Some(4), "judge")),
        );
        push(
            &mut r,
            &job(JobStatus::Running, progress(2, 0, Some(4), "judge")),
        );
        push(
            &mut r,
            &job(JobStatus::Succeeded, progress(4, 0, Some(4), "judge")),
        );
        // Finishing without new tasks sends nothing more.
        push(
            &mut r,
            &job(JobStatus::Succeeded, progress(4, 0, Some(4), "judge")),
        );
        let progress: Vec<f64> = values.iter().map(|(p, _)| *p).collect();
        assert_eq!(progress, vec![0.0, 0.001, 1.0, 3.0, 3.001, 5.0, 7.0]);
        for pair in progress.windows(2) {
            assert!(pair[1] > pair[0], "{progress:?}");
        }
        // Totals follow the offset and never sit below the value.
        assert_eq!(values[2].1, Some(4.0));
        assert_eq!(values[5].1, Some(7.0));
        assert_eq!(values[6].1, Some(7.0));
    }

    #[test]
    fn no_sink_means_no_notifications() {
        let mut r = ProgressState::new(None);
        assert!(r.next(&job(JobStatus::Running, None)).is_none());
    }

    #[test]
    fn describes_the_job() {
        let mut j = job(
            JobStatus::Running,
            progress(12, 1, Some(40), "legal / nda-03"),
        );
        j.kind = JobKind::Pipeline;
        j.steps = vec![
            JobStep {
                name: "check".into(),
                status: JobStatus::Succeeded,
                exit_code: Some(0),
                started_at: None,
                finished_at: None,
            },
            JobStep {
                name: "run".into(),
                status: JobStatus::Running,
                exit_code: None,
                started_at: None,
                finished_at: None,
            },
        ];
        j.spent_usd = Some(1.234);
        assert_eq!(
            describe(&j),
            "pipeline running · step run · 12/40 tasks done, 1 failed · current: legal / nda-03 · \
             $1.23 recorded spend"
        );
    }

    #[test]
    fn events_update_the_job() {
        let mut j = job(JobStatus::Queued, None);
        assert!(apply(
            &mut j,
            JobEvent::Status {
                status: JobStatus::Running,
                exit_code: None,
                error: None,
            }
        ));
        assert_eq!(j.status, JobStatus::Running);
        assert!(apply(
            &mut j,
            JobEvent::Progress {
                progress: Progress {
                    done: 1,
                    failed: 0,
                    total: Some(2),
                    current: None,
                },
            }
        ));
        assert!(!apply(
            &mut j,
            JobEvent::Log {
                line: LogLine {
                    n: 41,
                    ts: "t".into(),
                    stream: LogStream::Stdout,
                    text: "x".into(),
                },
            }
        ));
        assert_eq!(j.log_lines, 42);
        let step = JobStep {
            name: "run".into(),
            status: JobStatus::Running,
            exit_code: None,
            started_at: None,
            finished_at: None,
        };
        assert!(apply(&mut j, JobEvent::Step { step: step.clone() }));
        assert!(apply(
            &mut j,
            JobEvent::Step {
                step: JobStep {
                    status: JobStatus::Succeeded,
                    ..step
                },
            }
        ));
        assert_eq!(j.steps.len(), 1);
        assert_eq!(j.steps[0].status, JobStatus::Succeeded);
    }
}
