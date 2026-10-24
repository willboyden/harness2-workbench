//! Job lifecycle with a fake harness2 home: real subprocesses, process groups, logs,
//! cancellation, timeouts, writer locks, budget guard, restart recovery.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::time::Duration;

use common::*;
use h2s_api::{ErrorBody, ErrorCode, Job, JobResult, JobStatus, LogChunk, LogStream, Page, Role};
use serde_json::json;

fn mt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_job_succeeds_with_result_progress_and_masked_log() {
    let t = TestServer::start().await;
    let r = t.create_job(&t.operator, demo_spec("ok-demo")).await;
    assert_eq!(r.status(), 201);
    let job: Job = r.json().await.unwrap();
    assert_eq!(job.status, JobStatus::Queued);
    assert_eq!(job.created_by, "operator-t");
    let done = t.wait_terminal(&job.id, 30).await;
    assert_terminal(&done, JobStatus::Succeeded);
    assert_eq!(done.exit_code, Some(0));
    assert!(done.started_at.is_some() && done.finished_at.is_some());
    match done.result {
        Some(JobResult::Demo(d)) => {
            assert_eq!(
                (d.done, d.failed, d.execution_count, d.editor_calls),
                (2, 0, 28, 32)
            );
        }
        other => panic!("unexpected result {other:?}"),
    }
    let p = done.progress.unwrap();
    assert_eq!((p.done, p.failed), (2, 0));
    assert_eq!(p.current.as_deref(), Some("accounting / ledger-2"));
    assert_eq!(done.steps.len(), 1);
    assert_eq!(done.steps[0].status, JobStatus::Succeeded);

    // The demo experiment was created on the fly.
    let exps: serde_json::Value = t
        .get("/api/v1/experiments", Some(&t.viewer))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let e = exps
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "ok-demo")
        .unwrap();
    assert_eq!(e["kind"], "demo");

    // Log: stdout lines in order, system lines around them, host paths masked.
    let log = full_log(&t, &job.id).await;
    assert_eq!(log.len() as u64, done.log_lines);
    for (i, l) in log.iter().enumerate() {
        assert_eq!(l.n, i as u64);
    }
    let text: Vec<&str> = log.iter().map(|l| l.text.as_str()).collect();
    assert!(
        text.iter()
            .any(|l| l.contains("Local demo: 2 tasks completed; 0 failed. <experiment>/demo.json")),
        "{text:#?}"
    );
    let root = t.root.path().display().to_string();
    assert!(
        !text.iter().any(|l| l.contains(&root)),
        "an absolute path leaked: {text:#?}"
    );
    assert!(
        log.iter()
            .any(|l| l.stream == LogStream::System && l.text.starts_with("▶ demo: harness2-demo"))
    );
    assert!(log.iter().any(|l| l.stream == LogStream::Stdout));
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exit_2_is_partial_and_exit_1_is_failed_with_stderr_reason() {
    let t = TestServer::start().await;
    let a: Job = t
        .create_job(&t.operator, demo_spec("partial-a"))
        .await
        .json()
        .await
        .unwrap();
    let b: Job = t
        .create_job(&t.operator, demo_spec("fail-b"))
        .await
        .json()
        .await
        .unwrap();
    let a = t.wait_terminal(&a.id, 30).await;
    assert_terminal(&a, JobStatus::Partial);
    assert_eq!(a.exit_code, Some(2));
    assert_eq!(a.progress.as_ref().unwrap().failed, 1);
    assert!(matches!(a.result, Some(JobResult::Demo(ref d)) if d.done == 1 && d.failed == 1));
    let b = t.wait_terminal(&b.id, 30).await;
    assert_terminal(&b, JobStatus::Failed);
    assert_eq!(b.exit_code, Some(1));
    let err = b.error.unwrap();
    assert!(err.contains("exited with code 1"), "{err}");
    assert!(err.contains("the deterministic editor crashed"), "{err}");
    let log = full_log(&t, &b.id).await;
    assert!(
        log.iter()
            .any(|l| l.stream == LogStream::Stderr && l.text.contains("crashed"))
    );
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_kills_the_whole_process_group() {
    let t = TestServer::start().await;
    let job: Job = t
        .create_job(&t.operator, demo_spec("sleep-c"))
        .await
        .json()
        .await
        .unwrap();
    t.wait_job(&job.id, 30, |j| j.status == JobStatus::Running)
        .await;
    let child: i32 = wait_file(&t.experiment_dir("sleep-c").join("child.pid"), 30)
        .await
        .trim()
        .parse()
        .unwrap();
    let id = job.id.clone();
    let row = t
        .srv()
        .state
        .db
        .call(move |c| h2s_daemon::db::jobs::get(c, &id))
        .await
        .unwrap()
        .unwrap();
    let leader = i32::try_from(row.pid.expect("pid recorded")).unwrap();
    assert!(row.pid_start.is_some());
    assert!(!process_gone(child) && !process_gone(leader));

    // Empty body, no content type (how the MCP client and the UI cancel).
    let r = t
        .post(
            &format!("/api/v1/jobs/{}/cancel", job.id),
            Some(&t.operator),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let done = t.wait_terminal(&job.id, 30).await;
    assert_terminal(&done, JobStatus::Cancelled);
    assert_eq!(done.error.as_deref(), Some("cancelled by operator-t"));
    assert!(wait_gone(child, 10).await, "the grandchild survived");
    assert!(wait_gone(leader, 10).await, "the leader survived");

    let again = t
        .post(
            &format!("/api/v1/jobs/{}/cancel", job.id),
            Some(&t.operator),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), 409);
    let body: ErrorBody = again.json().await.unwrap();
    assert!(body.error.request_id.is_some());
    // The job directory is gone and pid bookkeeping cleared.
    assert!(!t.root.path().join("data/jobs").join(&job.id).exists());
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sigterm_ignorers_get_sigkill_after_the_grace_period() {
    let t = TestServer::start_with(|s| s.kill_grace = Duration::from_secs(1)).await;
    let job: Job = t
        .create_job(&t.operator, demo_spec("stubborn-s"))
        .await
        .json()
        .await
        .unwrap();
    t.wait_job(&job.id, 30, |j| j.status == JobStatus::Running)
        .await;
    let child: i32 = wait_file(&t.experiment_dir("stubborn-s").join("child.pid"), 30)
        .await
        .trim()
        .parse()
        .unwrap();
    let started = std::time::Instant::now();
    t.post(&format!("/api/v1/jobs/{}/cancel", job.id), Some(&t.admin))
        .send()
        .await
        .unwrap();
    let done = t.wait_terminal(&job.id, 30).await;
    assert_terminal(&done, JobStatus::Cancelled);
    assert!(started.elapsed() >= Duration::from_millis(900));
    assert!(wait_gone(child, 10).await);
    let log = full_log(&t, &job.id).await;
    assert!(
        log.iter().any(|l| l.text.contains("sending SIGKILL")),
        "{log:#?}"
    );
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wall_clock_timeout_fails_the_job() {
    let t = TestServer::start_with(|s| s.file.jobs.timeout_hours = 1.5 / 3600.0).await;
    let job: Job = t
        .create_job(&t.operator, demo_spec("sleep-t"))
        .await
        .json()
        .await
        .unwrap();
    let child: i32 = wait_file(&t.experiment_dir("sleep-t").join("child.pid"), 30)
        .await
        .trim()
        .parse()
        .unwrap();
    let done = t.wait_terminal(&job.id, 30).await;
    assert_terminal(&done, JobStatus::Failed);
    assert_eq!(done.error.as_deref(), Some("timed out after 1s"));
    assert!(wait_gone(child, 10).await);
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn queued_jobs_cancel_at_once_and_the_queue_is_capped() {
    let t = TestServer::start_with(|s| {
        s.file.jobs.max_concurrent = 1;
        s.file.jobs.max_queued = 1;
    })
    .await;
    let a: Job = t
        .create_job(&t.operator, demo_spec("sleep-qa"))
        .await
        .json()
        .await
        .unwrap();
    t.wait_job(&a.id, 30, |j| j.status == JobStatus::Running)
        .await;
    let b: Job = t
        .create_job(&t.operator, demo_spec("ok-qb"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(t.job(&b.id).await.status, JobStatus::Queued);
    let c = t.create_job(&t.operator, demo_spec("ok-qc")).await;
    assert_eq!(c.status(), 503);
    let body: ErrorBody = c.json().await.unwrap();
    assert!(
        body.error.message.contains("queue is full"),
        "{}",
        body.error.message
    );

    let r = t
        .post(&format!("/api/v1/jobs/{}/cancel", b.id), Some(&t.operator))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let cancelled: Job = r.json().await.unwrap();
    assert_eq!(cancelled.status, JobStatus::Cancelled);
    assert!(cancelled.started_at.is_none());
    t.post(&format!("/api/v1/jobs/{}/cancel", a.id), Some(&t.operator))
        .send()
        .await
        .unwrap();
    assert_terminal(&t.wait_terminal(&a.id, 30).await, JobStatus::Cancelled);
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_writer_per_experiment_others_run_alongside() {
    let t = TestServer::start_with(|s| {
        s.file.jobs.max_concurrent = 3;
        s.sleep_secs = 1.5;
    })
    .await;
    let a: Job = t
        .create_job(&t.operator, demo_spec("slow-x"))
        .await
        .json()
        .await
        .unwrap();
    let b: Job = t
        .create_job(&t.operator, demo_spec("slow-x"))
        .await
        .json()
        .await
        .unwrap();
    let c: Job = t
        .create_job(&t.operator, demo_spec("slow-y"))
        .await
        .json()
        .await
        .unwrap();
    t.wait_job(&a.id, 30, |j| j.status == JobStatus::Running)
        .await;
    t.wait_job(&c.id, 30, |j| j.status == JobStatus::Running)
        .await;
    let b_now = t.job(&b.id).await;
    let a_now = t.job(&a.id).await;
    if a_now.status == JobStatus::Running {
        assert_eq!(
            b_now.status,
            JobStatus::Queued,
            "a second writer started on the same experiment"
        );
    }
    let (a, b, c) = (
        t.wait_terminal(&a.id, 60).await,
        t.wait_terminal(&b.id, 60).await,
        t.wait_terminal(&c.id, 60).await,
    );
    for j in [&a, &b, &c] {
        assert_terminal(j, JobStatus::Succeeded);
    }
    let ts = |s: &Option<String>| h2s_daemon::util::parse_rfc3339(s.as_deref().unwrap()).unwrap();
    assert!(
        ts(&b.started_at) >= ts(&a.finished_at),
        "b started before a finished"
    );
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn log_pages_lines_and_reports_completion() {
    let t = TestServer::start().await;
    let job: Job = t
        .create_job(&t.operator, demo_spec("chatty-l"))
        .await
        .json()
        .await
        .unwrap();
    let done = t.wait_terminal(&job.id, 60).await;
    assert_terminal(&done, JobStatus::Succeeded);
    assert!(done.log_lines > 1500);
    let page = |from: u64, limit: u64| {
        let t = &t;
        let id = job.id.clone();
        async move {
            let r = t
                .get(
                    &format!("/api/v1/jobs/{id}/log?from={from}&limit={limit}"),
                    Some(&t.viewer),
                )
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 200);
            r.json::<LogChunk>().await.unwrap()
        }
    };
    let p1 = page(0, 500).await;
    assert_eq!(p1.lines.len(), 500);
    assert_eq!(p1.next, 500);
    assert!(!p1.complete);
    assert_eq!(p1.lines[0].n, 0);
    let p2 = page(500, 500).await;
    assert_eq!(p2.lines[0].n, 500);
    let last = page(done.log_lines - 3, 500).await;
    assert_eq!(last.lines.len(), 3);
    assert_eq!(last.next, done.log_lines);
    assert!(last.complete);
    // limit is clamped to 5000; out-of-range pages are empty and complete.
    let all = page(0, 99_999).await;
    assert_eq!(all.lines.len() as u64, done.log_lines.min(5000));
    let beyond = page(done.log_lines + 10, 10).await;
    assert!(beyond.lines.is_empty() && beyond.complete);
    // Bad query values are 422, unknown ids 404.
    let bad = t
        .get(
            &format!("/api/v1/jobs/{}/log?from=x", job.id),
            Some(&t.viewer),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 422);
    let missing = t
        .get(
            &format!("/api/v1/jobs/{}/log", uuid::Uuid::now_v7()),
            Some(&t.viewer),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn long_lines_are_truncated_at_64_kib() {
    let t = TestServer::start().await;
    let job: Job = t
        .create_job(&t.operator, demo_spec("longline-z"))
        .await
        .json()
        .await
        .unwrap();
    assert_terminal(&t.wait_terminal(&job.id, 30).await, JobStatus::Succeeded);
    let log = full_log(&t, &job.id).await;
    let long = log
        .iter()
        .find(|l| l.text.starts_with("xxxx"))
        .expect("the long line");
    assert!(long.text.ends_with(h2s_daemon::jobs::logs::TRUNCATED_MARK));
    assert!(long.text.len() <= h2s_daemon::jobs::logs::MAX_LINE_BYTES + 64);
    assert!(log.iter().any(|l| l.text == "after the long line"));
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_jobs_preview_first_and_report_progress_against_the_total() {
    let t = TestServer::start().await;
    t.put_profile(json!({"name": "p1", "kind": "custom", "solver_model": "m/x"}))
        .await;
    t.put_experiment("ok-run", None).await;
    let r = t
        .create_job(&t.operator, run_spec("ok-run", "p1", None))
        .await;
    assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
    let job: Job = r.json().await.unwrap();
    let done = t.wait_terminal(&job.id, 30).await;
    assert_terminal(&done, JobStatus::Partial);
    let names: Vec<&str> = done.steps.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["preview", "run"]);
    assert_eq!(done.steps[0].status, JobStatus::Succeeded);
    assert_eq!(done.steps[1].status, JobStatus::Partial);
    let p = done.progress.unwrap();
    assert_eq!((p.done, p.failed, p.total), (2, 1, Some(3)));
    assert!(matches!(done.result, Some(JobResult::Run(ref r)) if r.done == 2 && r.failed == 1));
    // The first writing job pinned the profile to the experiment.
    let e: serde_json::Value = t
        .get("/api/v1/experiments", Some(&t.viewer))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pinned = e
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == "ok-run")
        .unwrap();
    assert_eq!(pinned["profile"], "p1");
    // A run with another profile is refused (409).
    t.put_profile(json!({"name": "p2", "kind": "custom", "solver_model": "m/y"}))
        .await;
    let r = t
        .create_job(&t.operator, run_spec("ok-run", "p2", None))
        .await;
    assert_eq!(r.status(), 409);
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn budget_guard_cancels_when_spend_reaches_the_cap() {
    let t = TestServer::start().await;
    t.put_profile(json!({"name": "p1", "kind": "custom", "solver_model": "m/x"}))
        .await;
    t.put_experiment("budget-b", None).await;
    let job: Job = t
        .create_job(&t.operator, run_spec("budget-b", "p1", Some(0.35)))
        .await
        .json()
        .await
        .unwrap();
    let done = t.wait_terminal(&job.id, 60).await;
    assert_terminal(&done, JobStatus::Cancelled);
    assert_eq!(done.error.as_deref(), Some("budget reached"));
    assert!(done.spent_usd.unwrap() >= 0.35, "{done:?}");
    let metrics = t
        .get("/metrics", Some(&t.viewer))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        metrics.contains("h2s_spend_usd_total{experiment=\"budget-b\"}"),
        "{metrics}"
    );
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn creation_is_validated_before_anything_runs() {
    let t = TestServer::start().await;
    // Structural rules -> 422 with field details.
    let r = t
        .create_job(
            &t.operator,
            json!({"kind": "demo", "params": {"experiment": "ok-v", "k": 99}}),
        )
        .await;
    assert_eq!(r.status(), 422);
    let e: ErrorBody = r.json().await.unwrap();
    assert_eq!(e.error.details[0].field, "params.k");
    // Host isolation needs the acknowledgement.
    t.put_profile(json!({"name": "p1", "kind": "custom", "solver_model": "m/x"}))
        .await;
    let mut spec = run_spec("ok-ack", "p1", None);
    spec["params"]["acknowledge_risks"] = json!(false);
    let r = t.create_job(&t.operator, spec).await;
    assert_eq!(r.status(), 422);
    let e: ErrorBody = r.json().await.unwrap();
    assert_eq!(e.error.details[0].field, "params.acknowledge_risks");
    // Unknown experiment.
    let r = t
        .create_job(&t.operator, run_spec("ok-missing", "p1", None))
        .await;
    assert_eq!(r.status(), 422);
    assert_eq!(
        r.json::<ErrorBody>().await.unwrap().error.details[0].field,
        "params.experiment"
    );
    // A profile secret that is not set is named.
    t.put_profile(
        json!({"name": "p3", "kind": "openai_compatible", "solver_model": "glm",
        "endpoint": {"base_url": "http://127.0.0.1:9/v1", "api_key_secret": "LOCAL_KEY"}}),
    )
    .await;
    t.put_experiment("ok-sec", None).await;
    let r = t
        .create_job(&t.operator, run_spec("ok-sec", "p3", None))
        .await;
    assert_eq!(r.status(), 422);
    let e: ErrorBody = r.json().await.unwrap();
    assert!(e.error.message.contains("LOCAL_KEY"), "{}", e.error.message);
    // Malformed JSON and unknown kinds are 400.
    let r = t
        .post("/api/v1/jobs", Some(&t.operator))
        .header("content-type", "application/json")
        .body("{\"kind\": \"nope\"}")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    // Role per kind: operators cannot install, viewers cannot start anything.
    let r = t
        .create_job(
            &t.operator,
            json!({"kind": "install_harness2", "params": {}}),
        )
        .await;
    assert_eq!(r.status(), 403);
    assert_eq!(
        r.json::<ErrorBody>().await.unwrap().error.message,
        "starting a job of kind install_harness2 requires the admin role"
    );
    let r = t.create_job(&t.viewer, demo_spec("ok-v2")).await;
    assert_eq!(r.status(), 403);
    // A demo on a standard experiment is a conflict.
    t.put_experiment("ok-std", None).await;
    let r = t.create_job(&t.operator, demo_spec("ok-std")).await;
    assert_eq!(r.status(), 409);
    // Nothing above was queued.
    let page: Page<Job> = t
        .get("/api/v1/jobs", Some(&t.viewer))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(page.items.is_empty(), "{:?}", page.items);
    t.stop().await;
}

/// A job printing far more than `jobs.max_log_bytes` keeps running to the end (its output
/// is still read and parsed, so it never blocks on a full pipe) while the stored log stops at
/// the limit with one marker line.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn output_beyond_the_log_limit_is_read_but_not_stored() {
    let cap: u64 = 1024 * 1024;
    let t = TestServer::start_with(|s| s.file.jobs.max_log_bytes = cap).await;
    let job: Job = t
        .create_job(&t.operator, demo_spec("flood-x"))
        .await
        .json()
        .await
        .unwrap();
    let done = t.wait_terminal(&job.id, 60).await;
    assert_terminal(&done, JobStatus::Succeeded);
    // The demo's progress lines came after the flood: parsed although not stored.
    assert_eq!(done.progress.as_ref().map(|p| p.done), Some(2), "{done:?}");
    let file = t.root.path().join(format!("data/logs/jobs/{}.log", job.id));
    let size = std::fs::metadata(&file).unwrap().len();
    assert!(size <= cap + 64 * 1024, "the log is {size} bytes");
    assert!(size > cap / 2, "the log is {size} bytes");
    let log = full_log(&t, &job.id).await;
    let markers = log
        .iter()
        .filter(|l| l.text.contains("log limit of 1 MiB reached"))
        .count();
    assert_eq!(markers, 1);
    assert!(
        log.iter()
            .any(|l| l.stream == LogStream::System && l.text.contains("of output were not stored")),
        "{:?}",
        log.iter().rev().take(5).collect::<Vec<_>>()
    );
    assert!(log.last().unwrap().text.contains("job succeeded"));
    assert_eq!(done.log_lines, u64::try_from(log.len()).unwrap());
    t.stop().await;
}

/// The data directory's filesystem running out of space stops jobs instead of filling it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn jobs_stop_when_the_data_directory_is_almost_full() {
    // A floor above any real disk's free space.
    let t = TestServer::start_with(|s| s.file.jobs.min_free_disk_mb = 1 << 40).await;
    let job: Job = t
        .create_job(&t.operator, demo_spec("chatty-disk"))
        .await
        .json()
        .await
        .unwrap();
    let done = t.wait_terminal(&job.id, 60).await;
    assert_terminal(&done, JobStatus::Failed);
    let error = done.error.unwrap_or_default();
    assert!(error.contains("MiB free on the data directory"), "{error}");
    t.stop().await;
}

/// `install_harness2` checks out only the tested commit unless the configuration allows
/// others (a fork can make any commit fetchable from the upstream repository).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn installing_an_untested_harness2_commit_needs_the_config_switch() {
    let other = "0123456789abcdef0123456789abcdef01234567";
    let tested = h2s_core::TESTED_HARNESS2_COMMIT;
    let install = |rev: Option<&str>| match rev {
        Some(r) => json!({"kind": "install_harness2", "params": {"rev": r}}),
        None => json!({"kind": "install_harness2", "params": {}}),
    };

    let t = TestServer::start_with(|s| s.file.paths.harness2_managed = true).await;
    let r = t.create_job(&t.admin, install(Some(other))).await;
    assert_eq!(r.status(), 422);
    let e: ErrorBody = r.json().await.unwrap();
    assert_eq!(e.error.code, ErrorCode::Validation);
    assert!(
        e.error.details.iter().any(|d| d.field == "params.rev"),
        "{e:?}"
    );
    assert!(
        e.error.message.contains("allow_untested_harness2_rev"),
        "{e:?}"
    );
    for rev in [Some(tested), None] {
        let r = t.create_job(&t.admin, install(rev)).await;
        assert_eq!(r.status(), 201, "{rev:?}: {}", r.text().await.unwrap());
        let j: Job = r.json().await.unwrap();
        t.wait_terminal(&j.id, 30).await;
    }
    t.stop().await;

    let t = TestServer::start_with(|s| {
        s.file.paths.harness2_managed = true;
        s.file.paths.allow_untested_harness2_rev = true;
    })
    .await;
    let r = t.create_job(&t.admin, install(Some(other))).await;
    assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
    let j: Job = r.json().await.unwrap();
    t.wait_terminal(&j.id, 30).await;
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn jobs_need_an_installed_harness2() {
    let t = TestServer::start_with(|s| s.install_harness2 = false).await;
    let r = t.create_job(&t.operator, demo_spec("ok-n")).await;
    assert_eq!(r.status(), 503);
    let e: ErrorBody = r.json().await.unwrap();
    assert!(e.error.message.contains("not installed"));
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn listing_filters_and_pages_newest_first() {
    let t = TestServer::start().await;
    let mut ids = Vec::new();
    for i in 0..3 {
        let j: Job = t
            .create_job(&t.operator, demo_spec(&format!("ok-l{i}")))
            .await
            .json()
            .await
            .unwrap();
        ids.push(j.id);
    }
    for id in &ids {
        t.wait_terminal(id, 30).await;
    }
    let p1: Page<Job> = t
        .get("/api/v1/jobs?limit=2", Some(&t.viewer))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(p1.items.len(), 2);
    assert_eq!(p1.items[0].id, ids[2]);
    let cur = p1.next_cursor.unwrap();
    let p2: Page<Job> = t
        .get(
            &format!("/api/v1/jobs?limit=2&cursor={cur}"),
            Some(&t.viewer),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(p2.items.len(), 1);
    assert_eq!(p2.items[0].id, ids[0]);
    let f: Page<Job> = t
        .get("/api/v1/jobs?status=succeeded&kind=demo", Some(&t.viewer))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(f.items.len(), 3);
    let none: Page<Job> = t
        .get("/api/v1/jobs?status=failed", Some(&t.viewer))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(none.items.is_empty());
    for bad in ["status=done", "kind=nope", "limit=x", "colour=red"] {
        let r = t
            .get(&format!("/api/v1/jobs?{bad}"), Some(&t.viewer))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 422, "{bad}");
    }
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn listing_filters_by_experiment() {
    // The experiment filter is validated with h2s_core::names::experiment_name.
    let t = TestServer::start().await;
    for i in 0..2 {
        let j: Job = t
            .create_job(&t.operator, demo_spec(&format!("ok-f{i}")))
            .await
            .json()
            .await
            .unwrap();
        t.wait_terminal(&j.id, 30).await;
    }
    let f: Page<Job> = t
        .get("/api/v1/jobs?experiment=ok-f1", Some(&t.viewer))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(f.items.len(), 1);
    let bad = t
        .get("/api/v1/jobs?experiment=Bad%20Name", Some(&t.viewer))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 422);
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bridge_key_is_captured_as_a_secret_and_masked_in_the_log() {
    let t = TestServer::start().await;
    t.put_profile(
        json!({"name": "p1", "kind": "custom", "solver_model": "m/x", "codex_model": "gpt-test"}),
    )
    .await;
    let spec = json!({"kind": "bridge", "params": {"profile": "p1"}});
    let r = t.create_job(&t.operator, spec.clone()).await;
    assert_eq!(r.status(), 403, "the bridge is admin-only");
    let job: Job = t.create_job(&t.admin, spec).await.json().await.unwrap();
    let running = t
        .wait_job(&job.id, 30, |j| {
            matches!(j.result, Some(JobResult::Bridge(_)))
        })
        .await;
    match running.result {
        Some(JobResult::Bridge(b)) => {
            assert_eq!(b.base_url, "http://127.0.0.1:4100/v1");
            assert_eq!(b.model, "gpt-test");
        }
        other => panic!("{other:?}"),
    }
    let secrets: Vec<h2s_api::SecretInfo> = t
        .get("/api/v1/secrets", Some(&t.admin))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let s = secrets
        .iter()
        .find(|s| s.name == "HARNESS2_CODEX_API_KEY")
        .expect("captured");
    assert!(s.set);
    assert_eq!(s.chars, Some(u32::try_from(t.bridge_key.len()).unwrap()));
    let log = full_log(&t, &job.id).await;
    let key_line = log
        .iter()
        .find(|l| l.text.starts_with("HARNESS2_CODEX_API_KEY="))
        .unwrap();
    assert_eq!(
        key_line.text,
        "HARNESS2_CODEX_API_KEY=[REDACTED:HARNESS2_CODEX_API_KEY]"
    );
    t.post(&format!("/api/v1/jobs/{}/cancel", job.id), Some(&t.admin))
        .send()
        .await
        .unwrap();
    let done = t.wait_terminal(&job.id, 30).await;
    assert_terminal(&done, JobStatus::Cancelled);
    assert!(matches!(done.result, Some(JobResult::Bridge(_))));
    let audit: Page<h2s_api::AuditEvent> = t
        .get("/api/v1/audit?limit=500", Some(&t.admin))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(audit.items.iter().any(|e| e.action == "secret.capture"));
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn secrets_printed_by_a_job_are_redacted_everywhere() {
    // Redaction is `h2s_core::redact::Redactor`, a stub until core lands.
    let t = TestServer::start().await;
    let job: Job = t
        .create_job(&t.operator, demo_spec("secret-r"))
        .await
        .json()
        .await
        .unwrap();
    assert_terminal(&t.wait_terminal(&job.id, 30).await, JobStatus::Succeeded);
    let log = full_log(&t, &job.id).await;
    let all: String = log
        .iter()
        .map(|l| l.text.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!all.contains(&t.secret_value), "the secret reached the log");
    assert!(all.contains("[REDACTED:FAKE_SECRET]"));
    let raw = std::fs::read_to_string(
        t.root
            .path()
            .join("data/logs/jobs")
            .join(format!("{}.log", job.id)),
    )
    .unwrap();
    assert!(!raw.contains(&t.secret_value));
    // And the bridge key never appears, on any line.
    t.put_profile(
        json!({"name": "p1", "kind": "custom", "solver_model": "m/x", "codex_model": "gpt-test"}),
    )
    .await;
    let b: Job = t
        .create_job(
            &t.admin,
            json!({"kind": "bridge", "params": {"profile": "p1"}}),
        )
        .await
        .json()
        .await
        .unwrap();
    t.wait_job(&b.id, 30, |j| j.log_lines >= 6).await;
    let log = full_log(&t, &b.id).await;
    assert!(!log.iter().any(|l| l.text.contains(&t.bridge_key)));
    t.post(&format!("/api/v1/jobs/{}/cancel", b.id), Some(&t.admin))
        .send()
        .await
        .unwrap();
    t.wait_terminal(&b.id, 30).await;
    t.stop().await;
}

/// Run a server in its own runtime, start a `sleep-*` demo job on `experiment`, then crash
/// the server (no graceful shutdown): the job stays `running` in the database and its
/// processes keep running. Returns the job id, the stub's background child and the leader.
fn crash_with_a_running_job(path: &std::path::Path, experiment: &str) -> (String, i32, i32) {
    let path = path.to_path_buf();
    let experiment = experiment.to_owned();
    std::thread::spawn(move || {
        mt().block_on(async move {
            fake_harness2_home(&path.join("harness2"));
            let setup = Setup::new(&path);
            let srv = start_server(&path, &setup, "", "").await;
            let id = h2s_daemon::auth::new_token_id().unwrap();
            let minted = h2s_daemon::auth::mint(&id).unwrap();
            let rec = h2s_daemon::db::tokens::TokenRecord {
                id: minted.id.clone(),
                secret_hash: minted.secret_hash,
                name: "op".into(),
                role: Role::Operator,
                created_at: 1,
                created_by: "t".into(),
                expires_at: None,
                revoked_at: None,
                last_used_at: None,
            };
            srv.state
                .db
                .call(move |c| h2s_daemon::db::tokens::insert(c, &rec))
                .await
                .unwrap();
            let c = client();
            let job: Job = c
                .post(format!("{}/api/v1/jobs", srv.base_url()))
                .bearer_auth(minted.token.as_str())
                .header("x-h2s-request", "1")
                .json(&demo_spec(&experiment))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            let child: i32 = wait_file(
                &path.join(format!("data/experiments/{experiment}/child.pid")),
                30,
            )
            .await
            .trim()
            .parse()
            .unwrap();
            let jid = job.id.clone();
            let leader = loop {
                let j = jid.clone();
                let row = srv
                    .state
                    .db
                    .call(move |c| h2s_daemon::db::jobs::get(c, &j))
                    .await
                    .unwrap()
                    .unwrap();
                if let Some(p) = row.pid {
                    assert!(row.pid_start.is_some() && row.pid_boot.is_some(), "{row:?}");
                    break i32::try_from(p).unwrap();
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            };
            // Crash: leak the handle (dropping it would trigger a graceful shutdown) and
            // let the runtime, with every task of this server, be dropped below. The
            // processes keep running.
            std::mem::forget(srv);
            (job.id, child, leader)
        })
    })
    .join()
    .unwrap()
}

/// A crash (no graceful shutdown) leaves a job `running` and its processes alive; the
/// next server start marks it failed and terminates its process group, identified by the
/// job marker in the leader's environment.
#[test]
fn restart_recovers_running_jobs_and_kills_their_processes() {
    let root = tempfile::tempdir().unwrap();
    let (job_id, child, leader) = crash_with_a_running_job(root.path(), "sleep-crash");
    assert!(
        !process_gone(child),
        "the job's processes should survive the crash"
    );
    assert!(h2s_daemon::jobs::procfs::carries_marker(leader, &job_id));

    mt().block_on(async move {
        let setup = Setup::new(root.path());
        let t = TestServer::start_in(root, setup).await;
        let j = t.job(&job_id).await;
        assert_terminal(&j, JobStatus::Failed);
        assert_eq!(j.error.as_deref(), Some("server restarted"));
        assert!(
            wait_gone(child, 15).await,
            "the grandchild survived the restart"
        );
        assert!(
            wait_gone(leader, 15).await,
            "the leader survived the restart"
        );
        let log = full_log(&t, &job_id).await;
        let last = &log.last().unwrap().text;
        assert!(last.contains("server restarted"), "{last}");
        assert!(last.contains("terminated its process group"), "{last}");
        t.stop().await;
    });
}

/// Start times are clock ticks since boot: a record from another boot identifies nothing,
/// so recovery signals nothing. (The processes here are alive and carry the marker, so it
/// is the boot check that holds.)
#[test]
fn restart_recovery_signals_nothing_recorded_on_another_boot() {
    let root = tempfile::tempdir().unwrap();
    let (job_id, child, leader) = crash_with_a_running_job(root.path(), "sleep-boot");
    let db = h2s_daemon::db::open_connection(&root.path().join("data/studio.db")).unwrap();
    let n = db
        .execute(
            "UPDATE jobs SET pid_boot = '00000000-0000-0000-0000-000000000000' WHERE id = ?1",
            [&job_id],
        )
        .unwrap();
    assert_eq!(n, 1);
    drop(db);

    mt().block_on(async move {
        let setup = Setup::new(root.path());
        let t = TestServer::start_in(root, setup).await;
        let j = t.job(&job_id).await;
        assert_terminal(&j, JobStatus::Failed);
        assert_eq!(j.error.as_deref(), Some("server restarted"));
        let log = full_log(&t, &job_id).await;
        let last = &log.last().unwrap().text;
        assert!(last.contains("before a reboot"), "{last}");
        tokio::time::sleep(Duration::from_millis(500)).await;
        let alive = !process_gone(child) && !process_gone(leader);
        // Clean up whatever happens below.
        let _ = h2s_daemon::jobs::procfs::signal_group(leader, nix::sys::signal::Signal::SIGKILL);
        assert!(alive, "a process recorded on another boot was signalled");
        assert!(wait_gone(child, 15).await && wait_gone(leader, 15).await);
        t.stop().await;
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn graceful_shutdown_stops_running_jobs_and_keeps_queued_ones() {
    let t = TestServer::start_with(|s| s.file.jobs.max_concurrent = 1).await;
    let running: Job = t
        .create_job(&t.operator, demo_spec("sleep-sd"))
        .await
        .json()
        .await
        .unwrap();
    t.wait_job(&running.id, 30, |j| j.status == JobStatus::Running)
        .await;
    let child: i32 = wait_file(&t.experiment_dir("sleep-sd").join("child.pid"), 30)
        .await
        .trim()
        .parse()
        .unwrap();
    let queued: Job = t
        .create_job(&t.operator, demo_spec("ok-sd"))
        .await
        .json()
        .await
        .unwrap();
    let (rid, qid) = (running.id.clone(), queued.id.clone());
    let root = t.stop().await;
    assert!(
        wait_gone(child, 15).await,
        "the job's process outlived the server"
    );
    let conn = h2s_daemon::db::open_connection(&root.path().join("data/studio.db")).unwrap();
    let r = h2s_daemon::db::jobs::get(&conn, &rid).unwrap().unwrap();
    assert_eq!(r.job.status, JobStatus::Failed);
    assert_eq!(r.job.error.as_deref(), Some("server stopped"));
    let q = h2s_daemon::db::jobs::get(&conn, &qid).unwrap().unwrap();
    assert_eq!(
        q.job.status,
        JobStatus::Queued,
        "queued jobs wait for the next start"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn old_job_logs_are_deleted_by_retention() {
    let t = TestServer::start().await;
    let job: Job = t
        .create_job(&t.operator, demo_spec("ok-ret"))
        .await
        .json()
        .await
        .unwrap();
    t.wait_terminal(&job.id, 30).await;
    let root = t.stop().await;
    let log = root
        .path()
        .join("data/logs/jobs")
        .join(format!("{}.log", job.id));
    assert!(log.exists());
    // Age the job by 2 days, then start with a 1-day retention.
    let conn = h2s_daemon::db::open_connection(&root.path().join("data/studio.db")).unwrap();
    conn.execute(
        "UPDATE jobs SET finished_at = finished_at - 172800 WHERE id = ?1",
        [&job.id],
    )
    .unwrap();
    drop(conn);
    let mut setup = Setup::new(root.path());
    setup.file.jobs.log_retention_days = 1;
    let t = TestServer::start_in(root, setup).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while log.exists() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the old log was not deleted"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let chunk: LogChunk = t
        .get(&format!("/api/v1/jobs/{}/log", job.id), Some(&t.viewer))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(chunk.lines.is_empty() && chunk.complete);
    assert_eq!(t.job(&job.id).await.status, JobStatus::Succeeded);
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_secret_in_argv_refuses_the_step() {
    let t = TestServer::start().await;
    let job: Job = t
        .create_job(&t.operator, demo_spec("argvleak-a"))
        .await
        .json()
        .await
        .unwrap();
    let done = t.wait_terminal(&job.id, 30).await;
    assert_terminal(&done, JobStatus::Failed);
    assert_eq!(
        done.error.as_deref(),
        Some("refused: a secret value would appear on the command line")
    );
    assert!(
        !t.experiment_dir("argvleak-a").join("demo.json").exists(),
        "the step ran"
    );
    let log = full_log(&t, &job.id).await;
    assert!(!log.iter().any(|l| l.text.contains(&t.secret_value)));
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workbuddy_and_the_bridge_refuse_container_isolation() {
    let t = TestServer::start_with(|s| {
        s.file.isolation.mode = h2s_api::Isolation::Container;
        // An explicit user: no runtime is asked anything.
        s.file.isolation.container.user = "1000:1000".into();
    })
    .await;
    t.put_profile(
        json!({"name": "p1", "kind": "custom", "solver_model": "m/x", "codex_model": "gpt-test"}),
    )
    .await;
    let wb = json!({"kind": "check", "params": {"cell": {"bench": "wb", "subset": "office"}}});
    let r = t.create_job(&t.operator, wb).await;
    assert_eq!(r.status(), 409);
    assert!(
        r.json::<ErrorBody>()
            .await
            .unwrap()
            .error
            .message
            .contains("host isolation")
    );
    let r = t
        .create_job(
            &t.admin,
            json!({"kind": "bridge", "params": {"profile": "p1"}}),
        )
        .await;
    assert_eq!(r.status(), 409);
    // Runs need no acknowledgement in container mode.
    t.put_experiment("ok-cont", None).await;
    let mut spec = run_spec("ok-cont", "p1", None);
    spec["params"]["acknowledge_risks"] = json!(false);
    let r = t.create_job(&t.operator, spec).await;
    assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
    let info: h2s_api::SystemInfo = t
        .get("/api/v1/system/info", Some(&t.viewer))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info.isolation, h2s_api::Isolation::Container);
    assert!(info.features.contains(&"container".to_owned()));
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preacknowledged_host_risks_need_no_flag() {
    let t = TestServer::start_with(|s| s.file.isolation.preacknowledge_host_risks = true).await;
    t.put_profile(json!({"name": "p1", "kind": "custom", "solver_model": "m/x"}))
        .await;
    t.put_experiment("ok-pre", None).await;
    let mut spec = run_spec("ok-pre", "p1", None);
    spec["params"]["acknowledge_risks"] = json!(false);
    let r = t.create_job(&t.operator, spec).await;
    assert_eq!(r.status(), 201);
    let j: Job = r.json().await.unwrap();
    t.wait_terminal(&j.id, 30).await;
    // Every role can see it (the UI then skips asking for the acknowledgement).
    let info: h2s_api::SystemInfo = t
        .get("/api/v1/system/info", Some(&t.viewer))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(info.host_risks_preacknowledged);
    t.stop().await;
    let t = TestServer::start().await;
    let info: h2s_api::SystemInfo = t
        .get("/api/v1/system/info", Some(&t.viewer))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!info.host_risks_preacknowledged);
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_panicking_runner_fails_its_job_and_frees_the_slot() {
    let t = TestServer::start_with(|s| s.file.jobs.max_concurrent = 1).await;
    let bad: Job = t
        .create_job(&t.operator, demo_spec("panic-p"))
        .await
        .json()
        .await
        .unwrap();
    let after: Job = t
        .create_job(&t.operator, demo_spec("ok-after"))
        .await
        .json()
        .await
        .unwrap();
    let bad = t.wait_terminal(&bad.id, 30).await;
    assert_terminal(&bad, JobStatus::Failed);
    assert!(
        bad.error
            .as_deref()
            .unwrap()
            .starts_with("internal error in the job runner")
    );
    assert_terminal(&t.wait_terminal(&after.id, 30).await, JobStatus::Succeeded);
    t.stop().await;
}
