//! Job rows. The in-memory [`h2s_api::Job`] is the source of truth while a job is live;
//! this module persists it and reads it back.

use h2s_api::{Isolation, Job, JobKind, JobSpec, JobStatus, JobStep, Page, Progress};
use rusqlite::{Connection, OptionalExtension, params};
use serde::de::DeserializeOwned;

use super::{DbError, DbResult};
use crate::util::{parse_rfc3339, rfc3339};

/// A stored job plus the bookkeeping the API does not show.
#[derive(Debug, Clone, PartialEq)]
pub struct JobRow {
    pub seq: i64,
    pub job: Job,
    pub experiment: Option<String>,
    /// Leader pid of the step running now (host process or container CLI).
    pub pid: Option<i64>,
    /// Its start time in clock ticks since boot (`/proc/<pid>/stat` field 22).
    pub pid_start: Option<i64>,
    pub in_container: bool,
    /// The main command line (sanitized), for reproducibility manifests.
    pub argv: Option<Vec<String>>,
    pub log_purged: bool,
}

/// Parse a snake_case enum through serde.
pub(crate) fn parse_enum<T: DeserializeOwned>(s: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(s.to_owned())).ok()
}

pub(crate) fn isolation_str(i: Isolation) -> &'static str {
    match i {
        Isolation::Host => "host",
        Isolation::Container => "container",
    }
}

const COLUMNS: &str = "seq, id, kind, spec, experiment, status, isolation, created_by, created_at, \
     started_at, finished_at, exit_code, steps, progress, result, error, log_lines, spent_usd, \
     pid, pid_start, in_container, argv, log_purged";

fn ts(v: Option<i64>) -> Option<String> {
    v.map(rfc3339)
}

fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<DbResult<JobRow>> {
    let kind: String = r.get(2)?;
    let spec: String = r.get(3)?;
    let status: String = r.get(5)?;
    let isolation: String = r.get(6)?;
    let created_at: i64 = r.get(8)?;
    let steps: String = r.get(12)?;
    let progress: Option<String> = r.get(13)?;
    let result: Option<String> = r.get(14)?;
    let log_lines: i64 = r.get(16)?;
    let in_container: i64 = r.get(20)?;
    let argv: Option<String> = r.get(21)?;
    let log_purged: i64 = r.get(22)?;
    let seq: i64 = r.get(0)?;
    let id: String = r.get(1)?;
    let experiment: Option<String> = r.get(4)?;
    let created_by: String = r.get(7)?;
    let started_at: Option<i64> = r.get(9)?;
    let finished_at: Option<i64> = r.get(10)?;
    let exit_code: Option<i64> = r.get(11)?;
    let error: Option<String> = r.get(15)?;
    let spent_usd: Option<f64> = r.get(17)?;
    let pid: Option<i64> = r.get(18)?;
    let pid_start: Option<i64> = r.get(19)?;
    Ok((|| {
        let kind: JobKind =
            parse_enum(&kind).ok_or_else(|| DbError::Invalid(format!("job kind {kind}")))?;
        let status: JobStatus =
            parse_enum(&status).ok_or_else(|| DbError::Invalid(format!("job status {status}")))?;
        let isolation: Isolation = parse_enum(&isolation)
            .ok_or_else(|| DbError::Invalid(format!("isolation {isolation}")))?;
        let spec: JobSpec = serde_json::from_str(&spec)?;
        let steps: Vec<JobStep> = serde_json::from_str(&steps)?;
        let progress: Option<Progress> = progress.map(|p| serde_json::from_str(&p)).transpose()?;
        let result = result.map(|p| serde_json::from_str(&p)).transpose()?;
        let argv: Option<Vec<String>> = argv.map(|a| serde_json::from_str(&a)).transpose()?;
        Ok(JobRow {
            seq,
            job: Job {
                id,
                kind,
                spec,
                status,
                created_by,
                created_at: rfc3339(created_at),
                started_at: ts(started_at),
                finished_at: ts(finished_at),
                exit_code: exit_code.and_then(|c| i32::try_from(c).ok()),
                isolation,
                steps,
                progress,
                result,
                error,
                log_lines: u64::try_from(log_lines).unwrap_or(0),
                spent_usd,
            },
            experiment,
            pid,
            pid_start,
            in_container: in_container != 0,
            argv,
            log_purged: log_purged != 0,
        })
    })())
}

fn collect(stmt: &mut rusqlite::Statement<'_>, p: impl rusqlite::Params) -> DbResult<Vec<JobRow>> {
    let rows = stmt.query_map(p, from_row)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r??);
    }
    Ok(out)
}

fn unix(s: &Option<String>) -> Option<i64> {
    s.as_deref().and_then(parse_rfc3339)
}

/// Insert a new job; returns its sequence number (FIFO order).
///
/// # Errors
/// SQLite or JSON errors.
pub fn insert(conn: &Connection, job: &Job) -> DbResult<i64> {
    conn.execute(
        "INSERT INTO jobs (id, kind, spec, experiment, status, isolation, created_by, created_at, steps)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            job.id,
            job.kind.as_str(),
            serde_json::to_string(&job.spec)?,
            job.spec.experiment(),
            job.status.as_str(),
            isolation_str(job.isolation),
            job.created_by,
            parse_rfc3339(&job.created_at).unwrap_or_else(crate::util::now_unix),
            serde_json::to_string(&job.steps)?,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// One job by id.
///
/// # Errors
/// SQLite or JSON errors.
pub fn get(conn: &Connection, id: &str) -> DbResult<Option<JobRow>> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM jobs WHERE id = ?1"),
        [id],
        from_row,
    )
    .optional()?
    .transpose()
}

/// Filters for [`list`].
#[derive(Debug, Clone, Default)]
pub struct JobFilter {
    pub status: Option<JobStatus>,
    pub kind: Option<JobKind>,
    pub experiment: Option<String>,
}

/// Newest first; `before` is the exclusive upper bound on `seq`.
///
/// # Errors
/// SQLite or JSON errors.
pub fn list(
    conn: &Connection,
    f: &JobFilter,
    limit: u32,
    before: Option<i64>,
) -> DbResult<Page<JobRow>> {
    let limit = limit.clamp(1, 500);
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM jobs
         WHERE seq < ?1
           AND (?2 IS NULL OR status = ?2)
           AND (?3 IS NULL OR kind = ?3)
           AND (?4 IS NULL OR experiment = ?4)
         ORDER BY seq DESC LIMIT ?5"
    ))?;
    let mut items = collect(
        &mut stmt,
        params![
            before.unwrap_or(i64::MAX),
            f.status.map(JobStatus::as_str),
            f.kind.map(JobKind::as_str),
            f.experiment,
            i64::from(limit) + 1
        ],
    )?;
    let lim = usize::try_from(limit).unwrap_or(usize::MAX);
    let more = items.len() > lim;
    items.truncate(lim);
    let next_cursor = if more {
        items.last().map(|r| super::cursor::encode(r.seq))
    } else {
        None
    };
    Ok(Page { items, next_cursor })
}

/// Jobs in one status, oldest first.
///
/// # Errors
/// SQLite or JSON errors.
pub fn with_status(conn: &Connection, status: JobStatus) -> DbResult<Vec<JobRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM jobs WHERE status = ?1 ORDER BY seq ASC"
    ))?;
    collect(&mut stmt, [status.as_str()])
}

/// Every job of an experiment, oldest first (reproducibility manifest).
///
/// # Errors
/// SQLite or JSON errors.
pub fn for_experiment(conn: &Connection, experiment: &str) -> DbResult<Vec<JobRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM jobs WHERE experiment = ?1 ORDER BY seq ASC"
    ))?;
    collect(&mut stmt, [experiment])
}

/// Persist the mutable state of `job`.
///
/// # Errors
/// SQLite or JSON errors.
pub fn save_state(conn: &Connection, job: &Job) -> DbResult<()> {
    conn.execute(
        "UPDATE jobs SET status = ?2, started_at = ?3, finished_at = ?4, exit_code = ?5,
             steps = ?6, progress = ?7, result = ?8, error = ?9, log_lines = ?10, spent_usd = ?11
         WHERE id = ?1",
        params![
            job.id,
            job.status.as_str(),
            unix(&job.started_at),
            unix(&job.finished_at),
            job.exit_code,
            serde_json::to_string(&job.steps)?,
            job.progress
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?,
            job.result.as_ref().map(serde_json::to_string).transpose()?,
            job.error,
            i64::try_from(job.log_lines).unwrap_or(i64::MAX),
            job.spent_usd,
        ],
    )?;
    Ok(())
}

/// Atomically move a job from `from` to `to` (e.g. queued -> running). `false` when the
/// job was not in `from`.
///
/// # Errors
/// SQLite errors.
pub fn transition(
    conn: &Connection,
    id: &str,
    from: JobStatus,
    to: JobStatus,
    now: i64,
) -> DbResult<bool> {
    let (started, finished) = if to == JobStatus::Running {
        (Some(now), None)
    } else if to.is_terminal() {
        (None, Some(now))
    } else {
        (None, None)
    };
    let n = conn.execute(
        "UPDATE jobs SET status = ?3,
             started_at = COALESCE(?4, started_at),
             finished_at = COALESCE(?5, finished_at)
         WHERE id = ?1 AND status = ?2",
        params![id, from.as_str(), to.as_str(), started, finished],
    )?;
    Ok(n == 1)
}

/// Record the process the job's current step runs as (`None` clears it).
///
/// # Errors
/// SQLite errors.
pub fn set_process(
    conn: &Connection,
    id: &str,
    pid: Option<i64>,
    pid_start: Option<i64>,
    in_container: bool,
) -> DbResult<()> {
    conn.execute(
        "UPDATE jobs SET pid = ?2, pid_start = ?3, in_container = ?4 WHERE id = ?1",
        params![id, pid, pid_start, i64::from(in_container)],
    )?;
    Ok(())
}

/// Record the sanitized main command line.
///
/// # Errors
/// SQLite or JSON errors.
pub fn set_argv(conn: &Connection, id: &str, argv: &[String]) -> DbResult<()> {
    conn.execute(
        "UPDATE jobs SET argv = ?2 WHERE id = ?1",
        params![id, serde_json::to_string(argv)?],
    )?;
    Ok(())
}

/// Count jobs in a status.
///
/// # Errors
/// SQLite errors.
pub fn count_status(conn: &Connection, status: JobStatus) -> DbResult<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM jobs WHERE status = ?1",
        [status.as_str()],
        |r| r.get(0),
    )?)
}

/// Terminal jobs finished before `cutoff` whose logs are still on disk.
///
/// # Errors
/// SQLite errors.
pub fn logs_to_purge(conn: &Connection, cutoff: i64) -> DbResult<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM jobs WHERE log_purged = 0 AND finished_at IS NOT NULL AND finished_at < ?1",
    )?;
    let rows = stmt.query_map([cutoff], |r| r.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Mark a job's log as deleted by retention.
///
/// # Errors
/// SQLite errors.
pub fn mark_log_purged(conn: &Connection, id: &str) -> DbResult<()> {
    conn.execute("UPDATE jobs SET log_purged = 1 WHERE id = ?1", [id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testutil::conn;
    use h2s_api::{CheckParams, DemoMode, DemoParams};

    fn job(id: &str, spec: JobSpec) -> Job {
        Job {
            id: id.into(),
            kind: spec.kind(),
            spec,
            status: JobStatus::Queued,
            created_by: "op".into(),
            created_at: "2026-10-23T00:00:00Z".into(),
            started_at: None,
            finished_at: None,
            exit_code: None,
            isolation: Isolation::Host,
            steps: vec![],
            progress: None,
            result: None,
            error: None,
            log_lines: 0,
            spent_usd: None,
        }
    }

    fn demo(exp: &str) -> JobSpec {
        JobSpec::Demo(DemoParams {
            experiment: exp.into(),
            k: 2,
            mode: DemoMode::Both,
        })
    }

    #[test]
    fn insert_list_transition() {
        let (_d, c) = conn();
        let s1 = insert(&c, &job("j1", demo("e1"))).unwrap();
        let s2 = insert(
            &c,
            &job(
                "j2",
                JobSpec::Check(CheckParams {
                    cell: serde_json::from_str(r#"{"bench":"lab"}"#).unwrap(),
                    profile: None,
                }),
            ),
        )
        .unwrap();
        assert!(s2 > s1);
        let r = get(&c, "j1").unwrap().unwrap();
        assert_eq!(r.experiment.as_deref(), Some("e1"));
        assert_eq!(r.job.created_at, "2026-10-23T00:00:00Z");

        assert!(transition(&c, "j1", JobStatus::Queued, JobStatus::Running, 100).unwrap());
        assert!(!transition(&c, "j1", JobStatus::Queued, JobStatus::Running, 100).unwrap());
        let r = get(&c, "j1").unwrap().unwrap();
        assert_eq!(r.job.status, JobStatus::Running);
        assert!(r.job.started_at.is_some());

        let page = list(&c, &JobFilter::default(), 1, None).unwrap();
        assert_eq!(page.items[0].job.id, "j2");
        let before = crate::db::cursor::decode(page.next_cursor.as_deref().unwrap()).unwrap();
        let page = list(&c, &JobFilter::default(), 1, Some(before)).unwrap();
        assert_eq!(page.items[0].job.id, "j1");
        assert!(page.next_cursor.is_none());

        let f = JobFilter {
            status: Some(JobStatus::Running),
            ..JobFilter::default()
        };
        assert_eq!(list(&c, &f, 10, None).unwrap().items.len(), 1);
        let f = JobFilter {
            experiment: Some("e1".into()),
            kind: Some(JobKind::Demo),
            ..JobFilter::default()
        };
        assert_eq!(list(&c, &f, 10, None).unwrap().items.len(), 1);
        assert_eq!(with_status(&c, JobStatus::Queued).unwrap().len(), 1);
        assert_eq!(count_status(&c, JobStatus::Running).unwrap(), 1);
    }

    #[test]
    fn save_state_round_trips() {
        let (_d, c) = conn();
        let mut j = job("j1", demo("e1"));
        insert(&c, &j).unwrap();
        j.status = JobStatus::Partial;
        j.started_at = Some("2026-10-23T00:00:01Z".into());
        j.finished_at = Some("2026-10-23T00:00:09Z".into());
        j.exit_code = Some(2);
        j.steps.push(JobStep {
            name: "demo".into(),
            status: JobStatus::Partial,
            exit_code: Some(2),
            started_at: None,
            finished_at: None,
        });
        j.progress = Some(Progress {
            done: 1,
            failed: 1,
            total: Some(2),
            current: None,
        });
        j.error = Some("some tasks failed".into());
        j.log_lines = 12;
        j.spent_usd = Some(0.5);
        save_state(&c, &j).unwrap();
        set_process(&c, "j1", Some(42), Some(7), true).unwrap();
        set_argv(&c, "j1", &["harness2-demo".to_owned()]).unwrap();
        let r = get(&c, "j1").unwrap().unwrap();
        assert_eq!(r.job, j);
        assert_eq!(r.pid, Some(42));
        assert!(r.in_container);
        assert_eq!(r.argv.unwrap(), vec!["harness2-demo".to_owned()]);
        assert_eq!(for_experiment(&c, "e1").unwrap().len(), 1);
        assert_eq!(logs_to_purge(&c, i64::MAX).unwrap(), vec!["j1".to_owned()]);
        mark_log_purged(&c, "j1").unwrap();
        assert!(logs_to_purge(&c, i64::MAX).unwrap().is_empty());
    }
}
