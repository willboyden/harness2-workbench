//! The real `h2w-core` behind the API: identifier rules, the CoreBackend's argv and parsers
//! (against the stub harness2 home), experiments, profiles, secrets and tokens through the
//! API, and every results endpoint on real harness2 output (tests/fixtures).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::*;
use h2w_api::{
    AuditSession, Comparison, ErrorBody, Experiment, ExportBundle, FileContent, FileEntry,
    HarnessDiff, HarnessTree, Job, JobResult, JobStatus, LibraryDomain, Manifest, ModelProfile,
    Page, ScoreReport, Scores, SecretInfo, TaskDetail, TaskRow, TokenCreated,
};
use serde_json::json;

async fn core_server() -> TestServer {
    TestServer::start_with(|s| s.core_backend = true).await
}

async fn get_json<T: serde::de::DeserializeOwned>(t: &TestServer, path: &str) -> T {
    let r = t.get(path, Some(&t.viewer)).send().await.unwrap();
    let status = r.status();
    let text = r.text().await.unwrap();
    assert_eq!(status, 200, "{path}: {text}");
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}: {text}"))
}

async fn status_of(t: &TestServer, path: &str) -> u16 {
    t.get(path, Some(&t.viewer))
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_runs_through_the_core_backend() {
    let t = core_server().await;
    let job: Job = t
        .create_job(&t.operator, demo_spec("core-demo"))
        .await
        .json()
        .await
        .unwrap();
    let done = t.wait_terminal(&job.id, 30).await;
    assert_terminal(&done, JobStatus::Succeeded);
    match done.result {
        Some(JobResult::Demo(d)) => assert_eq!((d.done, d.failed), (2, 0)),
        other => panic!("{other:?}"),
    }
    let p = done.progress.expect("the demo reports progress");
    assert_eq!((p.done, p.failed, p.total), (2, 0, None));
    let log = full_log(&t, &job.id).await;
    assert!(
        log.iter()
            .any(|l| l.text == "▶ demo: harness2-demo --output <experiment> --k 2 --mode both"),
        "{:#?}",
        log.iter().map(|l| &l.text).collect::<Vec<_>>()
    );
    // Core's names rule now guards experiment names (422 with the field).
    let r = t.create_job(&t.operator, demo_spec("Bad Name")).await;
    assert_eq!(r.status(), 422);
    let e: ErrorBody = r.json().await.unwrap();
    assert_eq!(e.error.details[0].field, "params.experiment");
    t.stop().await;
}

/// The result files a job leaves are read without following a symlink and without blocking
/// on a FIFO: a demo that plants either fails with a clear message instead of hanging the
/// runner or reading another file.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn planted_result_files_fail_the_job_instead_of_hanging_it() {
    let t = core_server().await;
    for (exp, why) in [
        ("fifo-x", "demo.json is not a regular file"),
        ("symlink-x", "demo.json is a symbolic link"),
    ] {
        let r = t.create_job(&t.operator, demo_spec(exp)).await;
        assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
        let j: Job = r.json().await.unwrap();
        let done = t.wait_terminal(&j.id, 30).await;
        assert_terminal(&done, JobStatus::Failed);
        let error = done.error.unwrap_or_default();
        assert!(error.contains(why), "{exp}: {error}");
    }
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn check_jobs_parse_harness2_check_output() {
    let t = core_server().await;
    let job: Job = t
        .create_job(
            &t.operator,
            json!({"kind": "check", "params": {"cell": {"bench": "lab"}}}),
        )
        .await
        .json()
        .await
        .unwrap();
    let done = t.wait_terminal(&job.id, 30).await;
    assert_terminal(&done, JobStatus::Succeeded);
    match done.result {
        Some(JobResult::Check(c)) => {
            assert!(c.ready);
            assert_eq!(c.tasks, Some(3));
            assert!(c.failures.is_empty());
        }
        other => panic!("{other:?}"),
    }
    t.stop().await;
}

/// Results carry text from the job's output: host paths in it are masked for viewers as in
/// the log, `file://` URLs included.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn check_failures_and_logs_do_not_show_host_paths() {
    let t = core_server().await;
    let job: Job = t
        .create_job(
            &t.operator,
            json!({"kind": "check", "params": {"cell": {"bench": "jb"}}}),
        )
        .await
        .json()
        .await
        .unwrap();
    let done = t.wait_terminal(&job.id, 30).await;
    assert_terminal(&done, JobStatus::Failed);
    let data = t.root.path().join("data").display().to_string();
    match &done.result {
        Some(JobResult::Check(c)) => {
            assert_eq!(
                c.failures,
                vec![
                    "<job>/runs is not writable".to_owned(),
                    "see file://<job>/runs/check.html".to_owned()
                ]
            );
        }
        other => panic!("{other:?}"),
    }
    let log = full_log(&t, &job.id).await;
    let text: Vec<&str> = log.iter().map(|l| l.text.as_str()).collect();
    assert!(
        text.contains(&"FAIL: see file://<job>/runs/check.html"),
        "{text:#?}"
    );
    assert!(!text.iter().any(|l| l.contains(&data)), "{text:#?}");
    let api = t
        .get(&format!("/api/v1/jobs/{}", job.id), Some(&t.viewer))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!api.contains(&data), "{api}");
    t.stop().await;
}

fn pipeline_spec(experiment: &str, judge_bases: bool) -> serde_json::Value {
    json!({"kind": "pipeline", "params": {
        "experiment": experiment,
        "cell": {"bench": "lab", "substrate": "oc"},
        "options": {"k": 1, "modes": ["parallel"]},
        "profile": "p1",
        "judge_bases": judge_bases,
        "acknowledge_risks": true
    }})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipelines_run_every_step_through_the_core_parsers() {
    let t = core_server().await;
    t.put_profile(json!({"name": "p1", "kind": "custom", "solver_model": "m/x"}))
        .await;
    t.put_experiment("pipe-ok", None).await;
    let job: Job = t
        .create_job(&t.operator, pipeline_spec("pipe-ok", true))
        .await
        .json()
        .await
        .unwrap();
    let done = t.wait_terminal(&job.id, 60).await;
    assert_terminal(&done, JobStatus::Partial);
    let steps: Vec<(&str, JobStatus)> = done
        .steps
        .iter()
        .map(|s| (s.name.as_str(), s.status))
        .collect();
    assert_eq!(
        steps,
        vec![
            ("preview", JobStatus::Succeeded),
            ("check", JobStatus::Succeeded),
            ("run", JobStatus::Partial),
            ("judge", JobStatus::Partial),
            ("judge-bases", JobStatus::Partial),
            ("report", JobStatus::Succeeded),
        ]
    );
    let p = done.progress.unwrap();
    assert_eq!((p.done, p.failed, p.total), (2, 1, Some(3)));
    let Some(JobResult::Pipeline(r)) = done.result else {
        panic!("{:?}", done.result)
    };
    assert_eq!(r.steps.len(), 6);
    assert!(
        matches!(&r.steps[0], JobResult::Preview(pv) if pv.total_tasks == 3 && pv.summary_namespace == "lab-oc-agg1")
    );
    assert!(matches!(&r.steps[1], JobResult::Check(c) if c.ready));
    assert!(matches!(&r.steps[2], JobResult::Run(x) if x.done == 2 && x.failed == 1));
    assert!(
        matches!(&r.steps[3], JobResult::Judge(j) if !j.bases && j.rows == 3 && j.graded == 2 && j.ungraded == 1)
    );
    assert!(matches!(&r.steps[4], JobResult::Judge(j) if j.bases && j.rows == 2));
    assert!(
        matches!(&r.steps[5], JobResult::Report(rep) if rep.text.starts_with("lab-oc modes=parallel"))
    );

    // The first step that neither succeeds nor is partial stops the pipeline.
    t.put_experiment("pipe-failcheck", None).await;
    let job: Job = t
        .create_job(&t.operator, pipeline_spec("pipe-failcheck", false))
        .await
        .json()
        .await
        .unwrap();
    let done = t.wait_terminal(&job.id, 60).await;
    assert_terminal(&done, JobStatus::Failed);
    let steps: Vec<(&str, JobStatus)> = done
        .steps
        .iter()
        .map(|s| (s.name.as_str(), s.status))
        .collect();
    assert_eq!(
        steps,
        vec![
            ("preview", JobStatus::Succeeded),
            ("check", JobStatus::Failed),
            ("run", JobStatus::Cancelled),
            ("judge", JobStatus::Cancelled),
            ("report", JobStatus::Cancelled),
        ]
    );
    assert!(
        done.error
            .as_deref()
            .unwrap()
            .starts_with("step check failed"),
        "{:?}",
        done.error
    );
    let Some(JobResult::Pipeline(r)) = done.result else {
        panic!()
    };
    assert!(matches!(&r.steps[1], JobResult::Check(c) if !c.ready && c.failures.len() == 1));
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn experiments_through_the_api() {
    let t = core_server().await;
    let r = t
        .post("/api/v1/experiments", Some(&t.operator))
        .json(&json!({"name": "exp-one", "description": "first", "tags": ["a", "b"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    let e: Experiment = r.json().await.unwrap();
    assert_eq!(
        (e.name.as_str(), e.created_by.as_str()),
        ("exp-one", "operator-t")
    );
    assert_eq!(e.tags, vec!["a", "b"]);
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(t.experiment_dir("exp-one"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o700);
    // Duplicate, invalid name, invalid tag, unknown profile, viewer.
    for (body, status) in [
        (json!({"name": "exp-one"}), 409),
        (json!({"name": "Exp One"}), 422),
        (json!({"name": "exp-two", "tags": ["Bad Tag"]}), 422),
        (json!({"name": "exp-two", "profile": "nope"}), 422),
    ] {
        let r = t
            .post("/api/v1/experiments", Some(&t.operator))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), status, "{body}");
    }
    let r = t
        .post("/api/v1/experiments", Some(&t.viewer))
        .json(&json!({"name": "exp-v"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    // Update and archive.
    let r = t
        .send(
            reqwest::Method::PATCH,
            "/api/v1/experiments/exp-one",
            Some(&t.operator),
        )
        .json(&json!({"description": "changed", "archived": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let e: Experiment = r.json().await.unwrap();
    assert!(e.archived && e.description == "changed");
    let list: Vec<Experiment> = get_json(&t, "/api/v1/experiments").await;
    assert!(list.iter().all(|x| x.name != "exp-one"));
    let all: Vec<Experiment> = get_json(&t, "/api/v1/experiments?archived=true").await;
    assert!(all.iter().any(|x| x.name == "exp-one"));
    let one: Experiment = get_json(&t, "/api/v1/experiments/exp-one").await;
    assert!(one.size_bytes.is_some());
    assert_eq!(status_of(&t, "/api/v1/experiments/nope").await, 404);
    assert_eq!(status_of(&t, "/api/v1/experiments/NOPE").await, 422);
    // Writing jobs refuse archived experiments.
    t.put_profile(json!({"name": "p1", "kind": "custom", "solver_model": "m/x"}))
        .await;
    let r = t
        .create_job(&t.operator, run_spec("exp-one", "p1", None))
        .await;
    assert_eq!(r.status(), 409);
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn profiles_and_secrets_through_the_api() {
    let t = core_server().await;
    let profile = json!({
        "name": "local", "kind": "openai_compatible", "solver_model": "glm-5",
        "endpoint": {"base_url": "http://127.0.0.1:4000/v1", "api_key_secret": "LOCAL_KEY"}
    });
    let r = t
        .send(
            reqwest::Method::PUT,
            "/api/v1/profiles/local",
            Some(&t.admin),
        )
        .json(&profile)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let r = t
        .send(
            reqwest::Method::PUT,
            "/api/v1/profiles/local",
            Some(&t.admin),
        )
        .json(&profile)
        .send()
        .await
        .unwrap();
    let p: ModelProfile = r.json().await.unwrap();
    assert_eq!(p.secrets.len(), 1);
    assert!(!p.secrets[0].set);
    // Name mismatch, invalid URL, operator.
    let r = t
        .send(
            reqwest::Method::PUT,
            "/api/v1/profiles/other",
            Some(&t.admin),
        )
        .json(&profile)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 422);
    let mut bad = profile.clone();
    bad["endpoint"]["base_url"] = json!("ftp://x");
    let r = t
        .send(
            reqwest::Method::PUT,
            "/api/v1/profiles/local",
            Some(&t.admin),
        )
        .json(&bad)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 422);
    let r = t
        .send(
            reqwest::Method::PUT,
            "/api/v1/profiles/local",
            Some(&t.operator),
        )
        .json(&profile)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);

    // A planted secret value is write-only.
    let planted = format!("planted-{}", random_text(30));
    let r = t
        .send(
            reqwest::Method::PUT,
            "/api/v1/secrets/LOCAL_KEY",
            Some(&t.admin),
        )
        .json(&json!({"value": planted}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body = r.text().await.unwrap();
    assert!(!body.contains(&planted));
    let info: SecretInfo = serde_json::from_str(&body).unwrap();
    assert!(info.set && info.chars == Some(u32::try_from(planted.len()).unwrap()));
    let p: Vec<ModelProfile> = get_json(&t, "/api/v1/profiles").await;
    assert!(p[0].secrets[0].set);
    for (name, body, status) in [
        ("lower_case", json!({"value": "x"}), 422),
        ("LOCAL_KEY", json!({"value": ""}), 422),
        ("LOCAL_KEY", json!({"valu": planted}), 400),
    ] {
        let r = t
            .send(
                reqwest::Method::PUT,
                &format!("/api/v1/secrets/{name}"),
                Some(&t.admin),
            )
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), status, "{name} {body}");
        assert!(!r.text().await.unwrap().contains(&planted));
    }
    let audit = t
        .get("/api/v1/audit?limit=100", Some(&t.admin))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!audit.contains(&planted));
    assert!(audit.contains("secret.set"));
    // A pinned profile cannot be deleted; an unpinned one can.
    t.put_experiment("pinned-exp", Some("local")).await;
    let r = t
        .send(
            reqwest::Method::DELETE,
            "/api/v1/profiles/local",
            Some(&t.admin),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 409);
    let r = t
        .send(
            reqwest::Method::DELETE,
            "/api/v1/secrets/LOCAL_KEY",
            Some(&t.admin),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    let r = t
        .send(
            reqwest::Method::DELETE,
            "/api/v1/secrets/LOCAL_KEY",
            Some(&t.admin),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tokens_through_the_api() {
    let t = core_server().await;
    let r = t
        .post("/api/v1/tokens", Some(&t.admin))
        .json(&json!({"name": "ci bot", "role": "operator", "expires_in_days": 30}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    let c: TokenCreated = r.json().await.unwrap();
    assert!(c.token.starts_with("h2w_"));
    assert_eq!(c.info.role, h2w_api::Role::Operator);
    assert!(c.info.expires_at.is_some());
    let me = t
        .get("/api/v1/auth/me", Some(&c.token))
        .send()
        .await
        .unwrap();
    assert_eq!(me.status(), 200);
    let list = t
        .get("/api/v1/tokens", Some(&t.admin))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!list.contains(&c.token[13..]), "a token secret was listed");
    for body in [
        json!({"name": "", "role": "viewer"}),
        json!({"name": "x", "role": "viewer", "expires_in_days": 0}),
        json!({"name": "x", "role": "root"}),
    ] {
        let r = t
            .post("/api/v1/tokens", Some(&t.admin))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert!(
            r.status() == 422 || r.status() == 400,
            "{body}: {}",
            r.status()
        );
    }
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metrics_accept_the_configured_metrics_token() {
    let t = TestServer::start_with(|s| {
        s.core_backend = true;
        s.file.telemetry.metrics_token_secret = "METRICS_TOKEN".into();
        s.secret_env = vec![(
            "H2W_SECRET_METRICS_TOKEN".into(),
            "scrape-me-0123456789".into(),
        )];
    })
    .await;
    t.client.get(t.url("/healthz")).send().await.unwrap();
    let r = t
        .client
        .get(t.url("/metrics"))
        .bearer_auth("scrape-me-0123456789")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.text().await.unwrap().contains("h2w_http_requests_total"));
    let r = t
        .client
        .get(t.url("/metrics"))
        .bearer_auth("scrape-me-wrong")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let r = t
        .client
        .get(t.url("/metrics"))
        .bearer_auth(&t.viewer)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tools_and_system_endpoints() {
    let t = core_server().await;
    let r = t
        .post("/api/v1/estimate", Some(&t.viewer))
        .json(&json!({"tasks": 10, "k": 2, "modes": ["parallel", "sequential"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let e: h2w_api::CostEstimate = r.json().await.unwrap();
    assert_eq!((e.tasks, e.k, e.modes), (10, 2, 2));
    assert!(e.rollouts > 0 && e.improver_sessions > 0);
    let r = t
        .post("/api/v1/estimate", Some(&t.viewer))
        .json(&json!({"tasks": 0, "k": 99, "modes": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 422);
    assert_eq!(r.json::<ErrorBody>().await.unwrap().error.details.len(), 3);

    let r = t
        .post("/api/v1/harness/validate", Some(&t.viewer))
        .json(&json!({"surface": "opencode", "files": [
            {"path": "systemprompt.md", "content": "Be careful."},
            {"path": "skills/x/SKILL.md", "content": "no frontmatter"}
        ]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let v: h2w_api::ValidationReport = r.json().await.unwrap();
    assert!(!v.ok && !v.errors.is_empty(), "{v:?}");
    let r = t
        .post("/api/v1/harness/validate", Some(&t.viewer))
        .json(&json!({"surface": "opencode", "files": [{"path": "../escape.md", "content": ""}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 422);

    let b: Vec<h2w_api::BenchmarkStatus> = get_json(&t, "/api/v1/benchmarks").await;
    assert!(b.len() >= 3, "{b:?}");
    let info: h2w_api::SystemInfo = get_json(&t, "/api/v1/system/info").await;
    assert_eq!(
        info.harness2.tested_commit,
        h2w_core::TESTED_HARNESS2_COMMIT
    );
    assert!(
        info.harness2.home.is_none() && info.data_dir.is_none(),
        "admin-only fields leaked"
    );
    let admin_info: h2w_api::SystemInfo = serde_json::from_str(
        &t.get("/api/v1/system/info", Some(&t.admin))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(admin_info.data_dir.is_some() && admin_info.harness2.home.is_some());
    let d: h2w_api::DoctorReport = get_json(&t, "/api/v1/system/doctor").await;
    assert!(!d.checks.is_empty());
    let r = t
        .get("/api/v1/system/doctor?refresh=true", Some(&t.viewer))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    let r = t
        .get("/api/v1/system/doctor?refresh=true", Some(&t.operator))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    t.stop().await;
}

async fn adopt(t: &TestServer, fixture: &str, name: &str, kind: &str) -> Experiment {
    copy_fixture(fixture, &t.experiment_dir(name));
    let r = t
        .post("/api/v1/experiments", Some(&t.operator))
        .json(&json!({"name": name, "kind": kind}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
    r.json().await.unwrap()
}

/// Tasks whose `summary.json` cannot be used are listed as `too-large` (over 4 MiB, never
/// read) or `unreadable`, and the status filter accepts both.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tasks_filter_by_the_unreadable_summary_statuses() {
    let t = core_server().await;
    copy_fixture("demo-k2", &t.experiment_dir("demo-st"));
    let sweep = t.experiment_dir("demo-st").join("runs/demo-oc-agg2");
    std::fs::write(sweep.join("ledger-1/summary.json"), "not json").unwrap();
    let big = format!("{{\"pad\": \"{}\"}}", "x".repeat(5 * 1024 * 1024));
    std::fs::write(sweep.join("ledger-2/summary.json"), big).unwrap();
    let r = t
        .post("/api/v1/experiments", Some(&t.operator))
        .json(&json!({"name": "demo-st", "kind": "demo"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
    let base = "/api/v1/experiments/demo-st/sweeps/demo-oc-agg2";
    let all: Page<TaskRow> = get_json(&t, &format!("{base}/tasks")).await;
    let mut statuses: Vec<&str> = all.items.iter().map(|r| r.status.as_str()).collect();
    statuses.sort_unstable();
    assert_eq!(statuses, vec!["too-large", "unreadable"]);
    for (status, task) in [("too-large", "ledger-2"), ("unreadable", "ledger-1")] {
        let rows: Page<TaskRow> = get_json(&t, &format!("{base}/tasks?status={status}")).await;
        let ids: Vec<&str> = rows.items.iter().map(|r| r.task_id.as_str()).collect();
        assert_eq!(ids, vec![task], "{status}");
    }
    let r = t
        .get(&format!("{base}/tasks?status=weird"), Some(&t.viewer))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 422);
    let e: h2w_api::ErrorBody = r.json().await.unwrap();
    assert!(e.error.message.contains("too-large or unreadable"), "{e:?}");
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn results_endpoints_on_real_demo_output() {
    let t = core_server().await;
    let e = adopt(&t, "demo-k2", "demo-fx", "demo").await;
    assert_eq!(e.sweeps.len(), 1);
    assert_eq!(e.sweeps[0].id, "demo-oc-agg2");
    let base = "/api/v1/experiments/demo-fx/sweeps/demo-oc-agg2";

    let rows: Page<TaskRow> = get_json(&t, &format!("{base}/tasks")).await;
    let ids: Vec<&str> = rows.items.iter().map(|r| r.task_id.as_str()).collect();
    assert_eq!(ids, vec!["ledger-1", "ledger-2"]);
    let p1: Page<TaskRow> = get_json(&t, &format!("{base}/tasks?limit=1")).await;
    assert_eq!(p1.items.len(), 1);
    let p2: Page<TaskRow> = get_json(
        &t,
        &format!("{base}/tasks?limit=1&cursor={}", p1.next_cursor.unwrap()),
    )
    .await;
    assert_eq!(p2.items[0].task_id, "ledger-2");
    assert!(p2.next_cursor.is_none());
    let none: Page<TaskRow> = get_json(&t, &format!("{base}/tasks?status=partial")).await;
    assert!(none.items.is_empty());
    assert_eq!(
        status_of(&t, &format!("{base}/tasks?status=weird")).await,
        422
    );

    let d: TaskDetail = get_json(&t, &format!("{base}/tasks/ledger-1")).await;
    assert_eq!(d.modes[0].finals, vec!["agg1", "agg2"]);
    assert_eq!(d.base.len(), 2);

    for mode in ["parallel", "agg"] {
        let tree: HarnessTree = get_json(
            &t,
            &format!("{base}/tasks/ledger-1/modes/{mode}/harnesses/agg1"),
        )
        .await;
        assert_eq!(tree.label, "agg1");
        assert!(!tree.files.is_empty() && tree.fingerprint.len() == 16);
    }
    assert_eq!(
        status_of(
            &t,
            &format!("{base}/tasks/ledger-1/modes/both/harnesses/agg1")
        )
        .await,
        422
    );
    assert_eq!(
        status_of(
            &t,
            &format!("{base}/tasks/ledger-1/modes/parallel/harnesses/nope")
        )
        .await,
        422
    );
    assert_eq!(
        status_of(
            &t,
            &format!("{base}/tasks/ledger-9/modes/parallel/harnesses/agg1")
        )
        .await,
        404
    );

    let diff: HarnessDiff = get_json(
        &t,
        &format!("{base}/tasks/ledger-1/modes/parallel/diff?from=base&to=agg1"),
    )
    .await;
    assert_eq!((diff.from.as_str(), diff.to.as_str()), ("base", "agg1"));
    assert!(
        diff.files
            .iter()
            .any(|f| f.change != h2w_api::ChangeKind::Unchanged)
    );
    assert_eq!(
        status_of(
            &t,
            &format!("{base}/tasks/ledger-1/modes/parallel/diff?from=base")
        )
        .await,
        422
    );

    let bundle: ExportBundle = get_json(
        &t,
        &format!(
            "{base}/tasks/ledger-1/modes/parallel/harnesses/agg1/export?target=opencode&format=json"
        ),
    )
    .await;
    assert!(!bundle.files.is_empty());
    let r = t
        .get(&format!("{base}/tasks/ledger-1/modes/parallel/harnesses/agg1/export?target=raw&format=tar.gz"), Some(&t.viewer))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["content-type"], "application/gzip");
    assert!(
        r.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("attachment; filename=\"harness-agg1-raw.tar.gz\"")
    );
    let bytes = r.bytes().await.unwrap();
    assert_eq!(&bytes[..2], &[0x1f, 0x8b]);
    assert_eq!(
        status_of(
            &t,
            &format!(
                "{base}/tasks/ledger-1/modes/parallel/harnesses/agg1/export?target=zip&format=json"
            )
        )
        .await,
        422
    );

    let files: Vec<FileEntry> = get_json(
        &t,
        &format!("{base}/tasks/ledger-1/modes/parallel/rollouts/base1/files"),
    )
    .await;
    assert!(files.iter().any(|f| f.name == "answer.txt"), "{files:?}");
    let f: FileContent = get_json(
        &t,
        &format!("{base}/tasks/ledger-1/modes/parallel/rollouts/base1/file?path=answer.txt"),
    )
    .await;
    assert!(f.text.unwrap().contains("Total"));
    for bad in ["../x", "/etc/passwd", "a/../../b"] {
        let s = status_of(
            &t,
            &format!("{base}/tasks/ledger-1/modes/parallel/rollouts/base1/file?path={bad}"),
        )
        .await;
        assert!(s == 422 || s == 400, "{bad}: {s}");
    }
    assert_eq!(
        status_of(
            &t,
            &format!("{base}/tasks/ledger-1/modes/parallel/rollouts/base1/file")
        )
        .await,
        422
    );

    let scores: Scores = get_json(&t, &format!("{base}/scores")).await;
    assert!(scores.rows.is_empty());
    let _report: ScoreReport = get_json(&t, &format!("{base}/report")).await;
    let lib: Vec<LibraryDomain> = get_json(&t, &format!("{base}/library")).await;
    assert!(lib.iter().any(|l| l.domain == "accounting"));
    let _audit: Vec<AuditSession> = get_json(&t, &format!("{base}/audit")).await;
    let cmp: Comparison = get_json(
        &t,
        "/api/v1/compare?a=demo-fx/demo-oc-agg2/parallel&b=demo-fx/demo-oc-agg2/seq",
    )
    .await;
    assert_eq!(cmp.a, "demo-fx/demo-oc-agg2/parallel");
    assert_eq!(
        status_of(&t, "/api/v1/compare?a=demo-fx/demo-oc-agg2&b=x/y/z").await,
        422
    );
    let m: Manifest = get_json(&t, "/api/v1/experiments/demo-fx/manifest").await;
    assert_eq!(m.experiment, "demo-fx");
    assert_eq!(m.results_digest.len(), 64);
    assert!(!m.sweep_identities.is_empty() || m.jobs.is_empty());
    assert_eq!(
        status_of(&t, "/api/v1/experiments/demo-fx/sweeps/not-a-sweep/tasks").await,
        404
    );
    t.stop().await;
}

/// One sweep that cannot be read (here a judged.json over the reader's row cap) is left out
/// of the experiment's sweep list, and the others are still listed; its own views say why
/// (413).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sweep_that_cannot_be_read_does_not_hide_the_others() {
    let t = core_server().await;
    copy_fixture("std", &t.experiment_dir("std-bad"));
    let row = json!({"task_id": "x", "mode": "agg", "k": 2});
    let rows: Vec<&serde_json::Value> = std::iter::repeat_n(&row, 100_001).collect();
    std::fs::write(
        t.experiment_dir("std-bad")
            .join("lab/adapt/lab-oc-agg2/judged.json"),
        serde_json::to_vec(&rows).unwrap(),
    )
    .unwrap();
    // Sorted: sweeps are listed newest first, and each copy's files have their own times, so
    // the two experiments can list the same sweeps in different orders.
    let ids = |e: &Experiment| -> Vec<String> {
        let mut ids: Vec<String> = e.sweeps.iter().map(|s| s.id.clone()).collect();
        ids.sort();
        ids
    };
    let created = adopt(&t, "std", "std-ok", "standard").await;
    assert!(
        ids(&created).contains(&"lab-oc-agg2".to_owned()),
        "{:?}",
        ids(&created)
    );
    let r = t
        .post("/api/v1/experiments", Some(&t.operator))
        .json(&json!({"name": "std-bad", "kind": "standard"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
    let bad: Experiment = r.json().await.unwrap();
    let mut expected = ids(&created);
    expected.retain(|id| id != "lab-oc-agg2");
    assert_eq!(ids(&bad), expected);
    assert!(expected.contains(&"lab-oc-agg1".to_owned()));
    let one: Experiment = get_json(&t, "/api/v1/experiments/std-bad").await;
    assert_eq!(ids(&one), expected);
    let all: Vec<Experiment> = get_json(&t, "/api/v1/experiments").await;
    let listed = all.iter().find(|e| e.name == "std-bad").unwrap();
    assert_eq!(ids(listed), expected);
    let r = t
        .get(
            "/api/v1/experiments/std-bad/sweeps/lab-oc-agg2/scores",
            Some(&t.viewer),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 413);
    let e: h2w_api::ErrorBody = r.json().await.unwrap();
    assert_eq!(e.error.code, h2w_api::ErrorCode::PayloadTooLarge);
    assert!(e.error.message.contains("100000 rows"), "{e:?}");
    t.stop().await;
}

/// A results response is built inside its read permit and refused with 413 past 64 MiB: a
/// harness of control characters (12 MiB on disk, within h2w-core's tree caps) would be a
/// 72 MiB JSON body. Its tar.gz export, much smaller, still works.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oversized_results_responses_are_refused() {
    let t = core_server().await;
    copy_fixture("demo-k2", &t.experiment_dir("demo-big"));
    let scripts = t
        .experiment_dir("demo-big")
        .join("runs/demo-oc-agg2/ledger-1/harnesses/agg1/scripts");
    for i in 0..3 {
        std::fs::write(
            scripts.join(format!("big{i}.py")),
            vec![1_u8; 4 * 1024 * 1024],
        )
        .unwrap();
    }
    let r = t
        .post("/api/v1/experiments", Some(&t.operator))
        .json(&json!({"name": "demo-big", "kind": "demo"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
    let base = "/api/v1/experiments/demo-big/sweeps/demo-oc-agg2/tasks/ledger-1/modes/parallel";
    for path in [
        format!("{base}/harnesses/agg1"),
        format!("{base}/harnesses/agg1/export?target=raw&format=json"),
    ] {
        let r = t.get(&path, Some(&t.viewer)).send().await.unwrap();
        assert_eq!(r.status(), 413, "{path}");
        let e: h2w_api::ErrorBody = r.json().await.unwrap();
        assert_eq!(e.error.code, h2w_api::ErrorCode::PayloadTooLarge, "{path}");
        assert!(e.error.message.contains("64 MiB"), "{e:?}");
    }
    let r = t
        .get(
            &format!("{base}/harnesses/agg1/export?target=raw&format=tar.gz"),
            Some(&t.viewer),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(status_of(&t, &format!("{base}/harnesses/agg2")).await, 200);
    t.stop().await;
}

/// Results reads take one of the server's `RESULTS_READS` permits: with all of them held,
/// a read waits; once they are free it is answered.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn results_reads_wait_for_a_permit() {
    let t = core_server().await;
    adopt(&t, "std", "std-sem", "standard").await;
    let path = "/api/v1/experiments/std-sem/sweeps/lab-oc-agg1/scores";
    let n = u32::try_from(h2w_daemon::reads::RESULTS_READS).unwrap();
    let permits = std::sync::Arc::clone(&t.srv().state.reads.permits)
        .acquire_many_owned(n)
        .await
        .unwrap();
    let err = t
        .get(path, Some(&t.viewer))
        .timeout(std::time::Duration::from_millis(700))
        .send()
        .await
        .unwrap_err();
    assert!(err.is_timeout(), "{err}");
    drop(permits);
    assert_eq!(status_of(&t, path).await, 200);
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn results_endpoints_on_a_standard_experiment() {
    let t = core_server().await;
    let e = adopt(&t, "std", "std-fx", "standard").await;
    let ids: Vec<&str> = e.sweeps.iter().map(|s| s.id.as_str()).collect();
    assert!(ids.contains(&"lab-oc-agg1"), "{ids:?}");
    let base = "/api/v1/experiments/std-fx/sweeps/lab-oc-agg1";
    let scores: Scores = get_json(&t, &format!("{base}/scores")).await;
    assert!(!scores.rows.is_empty() && !scores.base_rows.is_empty());
    let report: ScoreReport = get_json(&t, &format!("{base}/report")).await;
    assert!(!report.summaries.is_empty() && !report.text.is_empty());
    let sessions: Vec<AuditSession> = get_json(&t, &format!("{base}/audit")).await;
    assert!(!sessions.is_empty());
    let s = &sessions[0];
    let part = s.parts.first().unwrap();
    let r = t
        .get(&format!("{base}/audit/{}/{part}", s.id), Some(&t.viewer))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["content-type"], "text/plain; charset=utf-8");
    assert_eq!(
        status_of(&t, &format!("{base}/audit/{}/secrets", s.id)).await,
        422
    );
    assert_eq!(
        status_of(&t, &format!("{base}/audit/..%2Fx/mission")).await,
        422
    );
    let cmp: Comparison = get_json(
        &t,
        "/api/v1/compare?a=std-fx/lab-oc-agg1/parallel&b=std-fx/lab-oc-agg1/sequential",
    )
    .await;
    assert!(cmp.n_common > 0, "{cmp:?}");
    let vs_base: Comparison = get_json(
        &t,
        "/api/v1/compare?a=std-fx/lab-oc-agg1/base&b=std-fx/lab-oc-agg1/parallel",
    )
    .await;
    assert_eq!(vs_base.a, "std-fx/lab-oc-agg1/base");
    assert_eq!((vs_base.n_common, vs_base.wins), (2, 2), "{vs_base:?}");
    assert_eq!(
        status_of(
            &t,
            "/api/v1/compare?a=std-fx/lab-oc-agg1/bases&b=std-fx/lab-oc-agg1/parallel"
        )
        .await,
        422
    );
    // A preview estimate can use this experiment's measured spend.
    let r = t
        .post("/api/v1/estimate", Some(&t.viewer))
        .json(&json!({"tasks": 3, "k": 1, "modes": ["parallel"], "experiment": "std-fx"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    t.stop().await;
}

#[test]
fn container_plans_use_fixed_hardening_and_numeric_users() {
    use h2w_daemon::jobs::backend::{ContainerSettings, JobBackend, PlanInput};
    let d = tempfile::tempdir().unwrap();
    let home = d.path().join("harness2");
    let job_dir = d.path().join("job");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&job_dir).unwrap();
    let snapshot = h2w_daemon::secrets::SecretSnapshot::default();
    let container = ContainerSettings {
        runtime: h2w_daemon::config::RuntimeKind::Docker,
        image: "harness2-workbench-runner:0.1.0".into(),
        network: "none".into(),
        cpus: 2.0,
        memory_mb: 4096,
        pids_limit: 512,
        user: "0:0".into(),
        proxy: None,
    };
    let check = h2w_api::JobSpec::Check(h2w_api::CheckParams {
        cell: serde_json::from_value(json!({"bench": "lab"})).unwrap(),
        profile: None,
    });
    let split = h2w_api::JobSpec::Split(h2w_api::SplitParams {
        bench: h2w_api::Bench::Lab,
        subset: None,
        seed: 42,
        train_fraction: 0.5,
        name: "half".into(),
    });
    for spec in [&check, &split] {
        let input = PlanInput {
            job_id: "0190c5f0-0000-7000-8000-000000000000",
            spec,
            isolation: h2w_api::Isolation::Container,
            harness2_home: &home,
            harness2_managed: true,
            experiment: None,
            job_dir: &job_dir,
            split_file: None,
            profile: None,
            secrets: &snapshot,
            container: Some(&container),
        };
        let plan = h2w_daemon::jobs::core_backend::CoreBackend
            .plan(&input)
            .unwrap();
        let step = &plan.steps[0];
        assert_eq!(step.program, "docker");
        let argv = step.args.join(" ");
        for must in [
            "run",
            "--rm",
            "--cap-drop ALL",
            "--security-opt no-new-privileges",
            "--pids-limit 512",
            "--network none",
            "--user 0:0",
            "harness2-workbench-runner:0.1.0",
        ] {
            assert!(argv.contains(must), "{must} missing in {argv}");
        }
        assert!(
            argv.contains(&format!("{}:/h2w/job:ro", job_dir.display())),
            "{argv}"
        );
        assert!(step.container.is_some() && step.runtime_env);
        assert!(
            !argv.to_ascii_uppercase().contains("PROXY"),
            "no proxy configured: {argv}"
        );
        // The job's variables go through a private env file in the job directory: neither
        // the runtime CLI's environment nor its argv hold them.
        assert!(step.env.is_empty(), "{:?}", step.env);
        assert!(
            argv.contains(&format!(
                "--env-file {}",
                job_dir.join("container.env").display()
            )),
            "{argv}"
        );
        assert!(!argv.contains("HARNESS2_RUNS_DIR"), "{argv}");
        let (_, env_file) = plan
            .files
            .iter()
            .find(|(n, _)| n == "container.env")
            .expect("the env file is planned");
        assert!(
            env_file
                .lines()
                .any(|l| l.starts_with("HARNESS2_RUNS_DIR=/h2w/")),
            "{}",
            env_file.as_str()
        );
        if matches!(spec, h2w_api::JobSpec::Split(_)) {
            assert!(
                argv.contains(&format!(
                    "{}:/h2w/experiment",
                    job_dir.join("out").display()
                )),
                "{argv}"
            );
            assert!(
                argv.contains("--output /h2w/experiment/split.json"),
                "{argv}"
            );
        }
    }
}

#[test]
fn container_installs_plan_on_the_host_and_run_under_the_mount_point() {
    use h2w_daemon::jobs::backend::{ContainerSettings, JobBackend, PlanInput};
    let d = tempfile::tempdir().unwrap();
    let home = d.path().join("data/harness2");
    let job_dir = d.path().join("job");
    std::fs::create_dir_all(&job_dir).unwrap();
    let snapshot = h2w_daemon::secrets::SecretSnapshot::default();
    let container = ContainerSettings {
        runtime: h2w_daemon::config::RuntimeKind::Podman,
        image: "harness2-workbench-runner:0.1.0".into(),
        network: "h2w-egress".into(),
        cpus: 2.0,
        memory_mb: 4096,
        pids_limit: 512,
        user: "1000:1000".into(),
        proxy: Some("http://egress:3128".into()),
    };
    let spec = h2w_api::JobSpec::InstallHarness2(h2w_api::InstallParams {
        rev: None,
        extras: vec![h2w_api::InstallExtra::Bridge],
        reinstall: false,
    });
    let input = PlanInput {
        job_id: "0190c5f0-0000-7000-8000-000000000001",
        spec: &spec,
        isolation: h2w_api::Isolation::Container,
        harness2_home: &home,
        harness2_managed: true,
        experiment: None,
        job_dir: &job_dir,
        split_file: None,
        profile: None,
        secrets: &snapshot,
        container: Some(&container),
    };
    let plan = h2w_daemon::jobs::core_backend::CoreBackend
        .plan(&input)
        .unwrap();
    let names: Vec<&str> = plan.steps.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names.first(), Some(&"init"));
    assert_eq!(names.last(), Some(&"import"));
    let host = home.display().to_string();
    for s in &plan.steps {
        assert_eq!(s.program, "podman");
        let argv = s.args.join(" ");
        assert!(argv.contains(&format!("{host}:/h2w/harness2")), "{argv}");
        assert!(
            !argv.contains(&format!("{host}:/h2w/harness2:ro")),
            "install needs a writable checkout: {argv}"
        );
        // Everything after the image runs in the container: no host path there.
        let after_image = argv
            .split("harness2-workbench-runner:0.1.0")
            .nth(1)
            .unwrap();
        assert!(!after_image.contains(&host), "{after_image}");
        assert!(argv.contains("--network h2w-egress") && argv.contains("--user 1000:1000"));
        // git, uv and npm reach the network only through the configured proxy.
        for must in [
            "-e HTTPS_PROXY=http://egress:3128",
            "-e https_proxy=http://egress:3128",
            "-e HTTP_PROXY=http://egress:3128",
            "-e NO_PROXY=localhost,127.0.0.1,::1",
        ] {
            assert!(argv.contains(must), "{must} missing in {argv}");
        }
        // ... and the runtime CLI does not get it (it may talk to its daemon over TCP), nor
        // any other job variable: those are in the env file.
        assert!(s.env.is_empty(), "{:?}", s.env);
        assert!(
            argv.contains(&format!(
                "--env-file {}",
                job_dir.join("container.env").display()
            )),
            "{argv}"
        );
    }
    let init = plan.steps[0].args.join(" ");
    assert!(
        init.contains("-w /h2w ") && init.ends_with("git init -q /h2w/harness2"),
        "{init}"
    );
    let import = plan.steps.last().unwrap().args.join(" ");
    assert!(
        import.contains("/h2w/harness2/.venv/bin/python -I -c"),
        "{import}"
    );
    assert!(
        import.contains("litellm"),
        "the bridge extra is checked: {import}"
    );
}
