//! End to end: the real server runs the real harness2 demo, and every results endpoint reads
//! what harness2 actually wrote. Run with `cargo xtask e2e`.

// Test-only crate: a panic with a clear message IS how a failed expectation is reported here.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::time::Duration;

use common::{CSRF_HEADER, Studio, http_client, succeeded};
use futures_util_lite::next_sse_events;
use h2s_api::{
    ChangeKind, DemoMode, DemoParams, Experiment, ExportBundle, HarnessDiff, HarnessTree, JobEvent,
    JobResult, JobSpec, JobStatus, LibraryDomain, LogChunk, Manifest, Mode, Page, SystemInfo,
    TaskDetail, TaskRow,
};

/// Minimal SSE reader over a reqwest byte stream (no extra dependencies).
mod futures_util_lite {
    use h2s_api::JobEvent;

    /// Read `data:` frames until `stop` returns true or `max` events arrive.
    pub(crate) async fn next_sse_events(
        mut resp: reqwest::Response,
        max: usize,
        stop: impl Fn(&JobEvent) -> bool,
    ) -> Vec<JobEvent> {
        let mut buf = String::new();
        let mut events = Vec::new();
        while let Ok(Some(chunk)) = resp.chunk().await {
            buf.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(end) = buf.find("\n\n") {
                let frame: String = buf.drain(..end + 2).collect();
                assert!(
                    !frame.lines().any(|l| l.starts_with("event:")),
                    "SSE frames must be unnamed (contract v1.1), got: {frame}"
                );
                let data: Vec<&str> = frame
                    .lines()
                    .filter_map(|l| l.strip_prefix("data:"))
                    .map(str::trim_start)
                    .collect();
                if data.is_empty() {
                    continue;
                }
                let ev: JobEvent = serde_json::from_str(&data.join("\n"))
                    .unwrap_or_else(|e| panic!("bad JobEvent JSON ({e}): {frame}"));
                let done = stop(&ev);
                events.push(ev);
                if done || events.len() >= max {
                    return events;
                }
            }
        }
        events
    }
}

#[tokio::test]
#[ignore = "end-to-end: run with `cargo xtask e2e`"]
async fn system_info_reports_the_harness2_checkout() {
    let studio = Studio::start();
    let info: SystemInfo = studio.admin().get("/system/info").await;
    assert!(
        info.harness2.installed,
        "harness2 should be detected at {:?}",
        studio.env.harness2_home
    );
    assert!(!info.harness2.managed, "the e2e checkout is unmanaged");
    assert_eq!(info.harness2.tested_commit.len(), 40);
    assert!(info.harness2.home.is_some(), "admins see the checkout path");
    let viewer = studio.token("e2e-viewer", "viewer");
    let info_v: SystemInfo = studio.api(&viewer).get("/system/info").await;
    assert!(
        info_v.harness2.home.is_none(),
        "viewers must not see host paths"
    );
    assert!(info_v.data_dir.is_none(), "viewers must not see host paths");
}

#[tokio::test]
#[ignore = "end-to-end: run with `cargo xtask e2e`"]
async fn demo_runs_and_every_results_view_reads_it() {
    let studio = Studio::start();
    let api = studio.admin();

    // Start the demo and follow it live over SSE.
    let job = api
        .create_job(&JobSpec::Demo(DemoParams {
            experiment: "demo".into(),
            k: 2,
            mode: DemoMode::Both,
        }))
        .await;
    let resp = api.get_raw(&format!("/jobs/{}/events", job.id)).await;
    assert_eq!(resp.status(), 200);
    assert!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream")),
        "events endpoint must be text/event-stream"
    );
    let events = next_sse_events(
        resp,
        10_000,
        |ev| matches!(ev, JobEvent::Status { status, .. } if status.is_terminal()),
    )
    .await;
    assert!(
        matches!(events.first(), Some(JobEvent::Snapshot { .. })),
        "first SSE event must be a snapshot"
    );
    assert!(
        events.iter().any(|e| matches!(e, JobEvent::Log { .. })),
        "expected live log lines"
    );

    let job = api.wait_job(&job.id, Duration::from_secs(120)).await;
    assert!(
        succeeded(&job),
        "demo job ended {:?}: {:?}",
        job.status,
        job.error
    );
    match &job.result {
        Some(JobResult::Demo(d)) => {
            assert_eq!((d.done, d.failed), (2, 0), "demo result {d:?}");
            assert!(d.execution_count > 0 && d.editor_calls > 0);
        }
        other => panic!("expected a demo result, got {other:?}"),
    }

    // The full log is pageable and contains harness2's own summary line.
    let log: LogChunk = api
        .get(&format!("/jobs/{}/log?from=0&limit=5000", job.id))
        .await;
    assert!(log.complete);
    assert!(
        log.lines
            .iter()
            .any(|l| l.text.contains("Local demo: 2 tasks completed")),
        "harness2's summary line is missing from the job log"
    );

    // Experiment and sweep summaries.
    let exps: Vec<Experiment> = api.get("/experiments").await;
    let exp = exps
        .iter()
        .find(|e| e.name == "demo")
        .expect("demo experiment listed");
    assert_eq!(exp.kind, h2s_api::ExperimentKind::Demo);
    let sweep = exp
        .sweeps
        .iter()
        .find(|s| s.id == "demo-oc-agg2")
        .expect("summary namespace demo-oc-agg2");
    assert_eq!(sweep.k, 2);
    assert_eq!((sweep.tasks, sweep.ok), (2, 2));
    assert!(sweep.modes.contains(&Mode::Parallel) && sweep.modes.contains(&Mode::Sequential));

    // Tasks and the full task record.
    let page: Page<TaskRow> = api.get("/experiments/demo/sweeps/demo-oc-agg2/tasks").await;
    let slugs: Vec<&str> = page.items.iter().map(|t| t.slug.as_str()).collect();
    assert!(
        slugs.contains(&"ledger-1") && slugs.contains(&"ledger-2"),
        "tasks: {slugs:?}"
    );
    let task: TaskDetail = api
        .get("/experiments/demo/sweeps/demo-oc-agg2/tasks/ledger-1")
        .await;
    assert_eq!(task.domain, "accounting");
    assert_eq!(task.base.len(), 2, "k=2 means two shared base runs");
    let par = task
        .modes
        .iter()
        .find(|m| m.mode == Mode::Parallel)
        .expect("parallel record");
    assert_eq!(par.finals, vec!["agg1".to_owned(), "agg2".to_owned()]);
    let sel = par.selection.as_ref().expect("selection recorded");
    assert!(sel.ok && !sel.chosen_label.is_empty());
    assert!(
        sel.experience
            .as_ref()
            .is_some_and(|x| x.lesson.contains("both signs"))
    );
    let ledger = par.ledger.as_ref().expect("ledger");
    assert_eq!(
        (ledger.r, ledger.expected_r, ledger.p, ledger.expected_p),
        (8, 8, 6, 6)
    );

    // Harness trees and the diff that captures the demo's one real edit.
    let base: HarnessTree = api
        .get("/experiments/demo/sweeps/demo-oc-agg2/tasks/ledger-1/modes/parallel/harnesses/base")
        .await;
    assert!(base.files.iter().any(|f| f.path == "scripts/summarize.py"));
    let diff: HarnessDiff = api
        .get("/experiments/demo/sweeps/demo-oc-agg2/tasks/ledger-1/modes/parallel/diff?from=base&to=agg1")
        .await;
    let script = diff
        .files
        .iter()
        .find(|f| f.path == "scripts/summarize.py")
        .expect("script in diff");
    assert_eq!(script.change, ChangeKind::Modified);
    assert!(
        script.unified.contains("if amount > 0"),
        "the removed positive-only filter shows in the diff"
    );

    // Export the adopted harness for an OpenCode project.
    let bundle: ExportBundle = api
        .get("/experiments/demo/sweeps/demo-oc-agg2/tasks/ledger-1/modes/parallel/harnesses/agg1/export?target=opencode&format=json")
        .await;
    assert!(
        bundle.files.iter().any(|f| f.path == "AGENTS.md"),
        "OpenCode export has AGENTS.md"
    );
    assert!(
        bundle
            .files
            .iter()
            .any(|f| f.path == "scripts/summarize.py")
    );
    let gz = api
        .get_raw("/experiments/demo/sweeps/demo-oc-agg2/tasks/ledger-1/modes/parallel/harnesses/agg1/export?target=opencode&format=tar.gz")
        .await;
    assert_eq!(gz.status(), 200);
    let bytes = gz.bytes().await.expect("tar.gz body");
    assert_eq!(&bytes[..2], &[0x1f, 0x8b], "gzip magic");

    // The experience library holds the distilled lesson.
    let lib: Vec<LibraryDomain> = api
        .get("/experiments/demo/sweeps/demo-oc-agg2/library")
        .await;
    assert!(
        lib.iter().any(|d| d.domain == "accounting"
            && d.entries.iter().any(|e| e
                .experience
                .as_ref()
                .is_some_and(|x| x.cue.contains("credits and debits")))),
        "library lesson missing: {lib:?}"
    );

    // Reproducibility manifest.
    let manifest: Manifest = api.get("/experiments/demo/manifest").await;
    assert_eq!(manifest.experiment, "demo");
    assert!(
        manifest
            .harness2_commit
            .as_ref()
            .is_some_and(|c| c.len() == 40)
    );
    assert!(manifest.jobs.iter().any(|j| j.id == job.id));
    assert_eq!(manifest.results_digest.len(), 64);
}

#[tokio::test]
#[ignore = "end-to-end: run with `cargo xtask e2e`"]
async fn security_boundaries_hold_on_the_real_server() {
    let studio = Studio::start();
    let http = http_client();
    let base = &studio.base;

    // Unauthenticated.
    let r = http
        .get(format!("{base}/api/v1/experiments"))
        .send()
        .await
        .expect("GET");
    assert_eq!(r.status(), 401);

    // Viewer cannot start jobs.
    let viewer = studio.token("e2e-viewer", "viewer");
    let spec = JobSpec::Demo(DemoParams {
        experiment: "x".into(),
        k: 1,
        mode: DemoMode::Parallel,
    });
    let r = studio.api(&viewer).post_raw("/jobs", &spec).await;
    assert_eq!(r.status(), 403);

    // Unsafe method without the CSRF header.
    let r = http
        .post(format!("{base}/api/v1/jobs"))
        .bearer_auth(&studio.admin_token)
        .json(&spec)
        .send()
        .await
        .expect("POST");
    assert_eq!(
        r.status(),
        403,
        "POST without {CSRF_HEADER} must be refused"
    );

    // Foreign Origin on an unsafe method.
    let r = http
        .post(format!("{base}/api/v1/jobs"))
        .bearer_auth(&studio.admin_token)
        .header(CSRF_HEADER, "1")
        .header("origin", "https://evil.example")
        .json(&spec)
        .send()
        .await
        .expect("POST");
    assert_eq!(r.status(), 403, "cross-origin POST must be refused");

    // DNS rebinding: a Host the server does not answer to.
    let r = http
        .get(format!("{base}/api/v1/system/info"))
        .bearer_auth(&studio.admin_token)
        .header("host", "attacker.example")
        .send()
        .await
        .expect("GET");
    assert_eq!(r.status(), 421);

    // Security headers on a 404, an API response and the UI shell.
    for path in ["/api/v1/does-not-exist", "/healthz", "/"] {
        let r = http.get(format!("{base}{path}")).send().await.expect("GET");
        let h = r.headers();
        let csp = h
            .get("content-security-policy")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            csp.contains("default-src 'none'") && csp.contains("frame-ancestors 'none'"),
            "{path}: CSP {csp:?}"
        );
        assert_eq!(
            h.get("x-content-type-options")
                .and_then(|v| v.to_str().ok()),
            Some("nosniff"),
            "{path}"
        );
        assert!(
            h.get("access-control-allow-origin").is_none(),
            "{path}: no CORS, ever"
        );
    }

    // Secrets are write-only and never appear in responses or logs.
    let planted = "e2e-planted-secret-value-7f3c9a1d2b";
    let r = studio
        .admin()
        .put_raw(
            "/secrets/E2E_SECRET",
            &h2s_api::SetSecretRequest {
                value: planted.into(),
            },
        )
        .await;
    assert!(r.status().is_success(), "set secret: {}", r.status());
    let list = studio
        .admin()
        .get_raw("/secrets")
        .await
        .text()
        .await
        .expect("body");
    assert!(
        list.contains("E2E_SECRET") && !list.contains(planted),
        "secret value leaked in /secrets"
    );
    let audit = studio
        .admin()
        .get_raw("/audit")
        .await
        .text()
        .await
        .expect("body");
    assert!(
        audit.contains("E2E_SECRET") && !audit.contains(planted),
        "secret value leaked in the audit log"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    let logs = studio.stderr_so_far().join("\n");
    assert!(
        !logs.contains(planted),
        "secret value leaked in server logs"
    );
    assert!(
        !logs.contains(&studio.admin_token),
        "a bearer token leaked in server logs"
    );

    // The audit chain verifies.
    let v: h2s_api::AuditVerification = studio.admin().get("/audit/verify").await;
    assert!(v.ok && v.entries > 0, "audit chain: {v:?}");
}

#[tokio::test]
#[ignore = "end-to-end: run with `cargo xtask e2e`"]
async fn demo_twice_in_one_experiment_is_serialised_and_resumes() {
    let studio = Studio::start();
    let api = studio.admin();
    let spec = JobSpec::Demo(DemoParams {
        experiment: "twice".into(),
        k: 1,
        mode: DemoMode::Parallel,
    });
    let a = api.create_job(&spec).await;
    let b = api.create_job(&spec).await;
    let a = api.wait_job(&a.id, Duration::from_secs(120)).await;
    let b = api.wait_job(&b.id, Duration::from_secs(120)).await;
    assert!(
        succeeded(&a) && succeeded(&b),
        "{:?} / {:?}",
        a.status,
        b.status
    );
    // One writer per experiment: the second started only after the first finished.
    let parse = |s: &str| {
        time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|e| panic!("timestamp {s:?} is not RFC 3339: {e}"))
    };
    let a_end = parse(a.finished_at.as_deref().expect("a finished"));
    let b_start = parse(b.started_at.as_deref().expect("b started"));
    assert!(
        b_start >= a_end,
        "jobs on one experiment overlapped: {a_end} > {b_start}"
    );
    assert_ne!(b.status, JobStatus::Failed);
}
