//! Shared test support: a mock Harness² Workbench server (wiremock) serving contract-faithful
//! fixtures, and MCP client helpers.
//!
//! Every fixture is passed through its `h2w_api` type before it is served, so a fixture that
//! drifts from the LOCKED wire types fails loudly instead of testing a shape the real server
//! never sends.

#![allow(
    dead_code,
    unreachable_pub,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::needless_pass_by_value
)]

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use h2w_api::{
    AuditSession, BenchmarkStatus, Comparison, CostEstimate, DoctorReport, ErrorBody, Experiment,
    ExportBundle, FileContent, FileEntry, HarnessDiff, HarnessTree, Job, JobSpec, LibraryDomain,
    LogChunk, Manifest, ModelProfile, Page, Principal, ScoreReport, Scores, SplitInfo, SystemInfo,
    TaskDetail, TaskRow, ValidationReport,
};
use h2w_mcp::{ApiClient, ClientOptions, Credentials, H2sMcp, Token, WaitLimiter};
use rmcp::{
    ClientHandler, RoleClient, ServiceExt,
    model::{CallToolRequestParams, CallToolResult, ProgressNotificationParam},
    service::{NotificationContext, RunningService},
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use wiremock::{
    Mock, MockBuilder, MockServer, Request, Respond, ResponseTemplate,
    matchers::{bearer_token, header, method, path},
};

/// Tokens used by the tests (shaped like real ones, but not secrets).
pub const TOKEN: &str = "h2w_alice_s3cret";
pub const OTHER_TOKEN: &str = "h2w_bobby_s3cret";
pub const REVOKED_TOKEN: &str = "h2w_mallory_s3cret";

pub const JOB_ID: &str = "0192f0c4-7a2b-7000-8000-00000000a001";
pub const EXP: &str = "demo";
pub const SWEEP: &str = "demo-oc-agg2";
pub const TASK: &str = "t1";
pub const SESSION: &str = "inv1.a1.3f9c0d2e11aa7b40";
pub const NOW: &str = "2026-10-23T12:00:00Z";

pub const AUDIT_TEXT: &str = "<notes>Tightened the system prompt.</notes>";

// ------------------------------------------------------------------------------- fixtures

/// `value`, after a round trip through `T`.
pub fn typed<T: DeserializeOwned + Serialize>(value: Value) -> Value {
    let parsed: T = serde_json::from_value(value)
        .unwrap_or_else(|e| panic!("fixture does not match {}: {e}", std::any::type_name::<T>()));
    serde_json::to_value(parsed).unwrap()
}

pub fn principal(name: &str, role: &str) -> Value {
    typed::<Principal>(
        json!({"id": format!("tok-{name}"), "name": name, "role": role, "kind": "token"}),
    )
}

pub fn census() -> Value {
    json!({"always_on_chars": 42, "always_on_max": 24000, "n_files": 1, "skills": 0,
           "sub_agents": 0, "scripts": 0, "plugins": 0, "hooks": 0})
}

pub fn system_info() -> Value {
    typed::<SystemInfo>(json!({
        "studio_version": "0.1.0",
        "harness2": {
            "installed": true, "managed": true,
            "commit": "1cfe254000000000000000000000000000000000",
            "tested_commit": "1cfe254000000000000000000000000000000000",
            "version": "0.3.0", "python": "3.12.14", "extras": ["render"]
        },
        "isolation": "host",
        "auth_mode": "token",
        "server_time": NOW,
        "jobs_running": 0,
        "jobs_max_concurrent": 2,
        "features": ["container"]
    }))
}

pub fn doctor() -> Value {
    typed::<DoctorReport>(json!({
        "checks": [
            {"id": "uv", "label": "uv", "level": "ok", "detail": "uv 0.9.2", "needed_for": ["demo"]},
            {"id": "docker", "label": "Docker", "level": "warn", "detail": "not found",
             "remedy": "install Docker Engine", "needed_for": ["wb"]}
        ],
        "ok": true,
        "generated_at": NOW
    }))
}

pub fn benchmarks() -> Value {
    typed::<Vec<BenchmarkStatus>>(json!([{
        "bench": "lab", "display_name": "LAB", "installed": true, "at_pinned_commit": true,
        "data_present": true, "domains": [{"domain": "contracts", "tasks": 12}],
        "requirements": ["pandoc", "pdftoppm"],
        "upstream_url": "https://github.com/example/harvey-labs",
        "license_note": "MIT", "description": "Legal tasks graded against rubrics."
    }]))
}

pub fn profiles() -> Value {
    typed::<Vec<ModelProfile>>(json!([{
        "name": "local", "kind": "openai_compatible", "solver_model": "glm-5",
        "endpoint": {"base_url": "http://127.0.0.1:4000/v1", "api_key_secret": "GATEWAY_KEY"},
        "secrets": [{"name": "GATEWAY_KEY", "set": true, "chars": 51}]
    }]))
}

pub fn experiment(name: &str, archived: bool) -> Value {
    typed::<Experiment>(json!({
        "name": name, "kind": "demo", "description": "harness2 demo", "tags": ["demo"],
        "created_at": NOW, "created_by": "alice", "archived": archived,
        "sweeps": [{
            "id": SWEEP, "bench_name": "demo", "substrate": "oc", "k": 2,
            "modes": ["parallel", "sequential"], "tasks": 4, "ok": 4, "partial": 0, "failed": 0,
            "usd": 0.0,
            "headline": [{"mode": "parallel", "k": 2, "graded": 4, "score": 62.5, "all_pass": 25.0}],
            "updated_at": NOW
        }, {
            "id": "demo-oc-agg1", "bench_name": "demo", "substrate": "oc", "k": 1,
            "modes": ["parallel"], "tasks": 4, "ok": 4, "partial": 0, "failed": 0, "usd": 0.0
        }]
    }))
}

pub fn estimate() -> Value {
    typed::<CostEstimate>(json!({
        "tasks": 10, "modes": 1, "k": 2, "rollouts": 80, "improver_sessions": 60,
        "usd_low": 3.0, "usd_high": 9.0, "basis": "heuristic",
        "notes": ["Judging is not included."]
    }))
}

pub fn splits() -> Value {
    typed::<Vec<SplitInfo>>(json!([{
        "name": "lab-half", "bench": "lab", "seed": 42, "train_fraction": 0.5,
        "train": 6, "test": 6, "created_at": NOW
    }]))
}

/// The parsed result a finished job of `spec`'s kind would carry.
fn result_for(spec: &JobSpec) -> Value {
    match spec {
        JobSpec::InstallHarness2(_) => json!({"kind": "install",
            "commit": "1cfe254000000000000000000000000000000000", "version": "0.3.0",
            "python": "3.12.14", "extras": ["render"]}),
        JobSpec::Demo(_) => json!({"kind": "demo", "done": 4, "failed": 0,
            "execution_count": 24, "editor_calls": 12}),
        JobSpec::Setup(p) => json!({"kind": "setup", "dry_run": p.dry_run,
            "plan": ["git clone ..."], "notes": []}),
        JobSpec::Check(_) => json!({"kind": "check", "ready": true, "tasks": 12, "failures": []}),
        JobSpec::Preview(_) => json!({"kind": "preview",
            "header": "# lab split=all seed=None k=1 modes=parallel",
            "namespaces": [{"mode": "parallel", "namespace": "lab-oc-agg1"}],
            "summary_namespace": "lab-oc-agg1", "workers": 1,
            "stream": [{"index": 1, "domain": "contracts", "task": "t1"}],
            "domains": [{"domain": "contracts", "tasks": 1}],
            "total_tasks": 1, "total_domains": 1, "estimate": estimate()}),
        JobSpec::Run(_) => json!({"kind": "run", "namespace": "lab-oc-agg1", "done": 1,
            "failed": 0, "dead_domains": []}),
        JobSpec::Judge(_) => json!({"kind": "judge", "bases": false, "rows": 1, "graded": 1,
            "ungraded": 0}),
        JobSpec::Report(_) => json!({"kind": "report", "text": "lab parallel k=1 62.5"}),
        JobSpec::Split(p) => json!({"kind": "split", "name": p.name, "train": 6, "test": 6}),
        JobSpec::Bridge(_) => json!({"kind": "bridge", "base_url": "http://127.0.0.1:4100/v1",
            "model": "gemini-3"}),
        JobSpec::Pipeline(_) => json!({"kind": "pipeline", "steps": [
            {"kind": "run", "namespace": "lab-oc-agg1", "done": 1, "failed": 0, "dead_domains": []}
        ]}),
    }
}

/// A job for `spec` in `status` (with a result when finished successfully).
pub fn job(id: &str, spec: &JobSpec, status: &str, progress: Option<Value>) -> Value {
    let finished = matches!(status, "succeeded" | "partial" | "failed" | "cancelled");
    let mut job = json!({
        "id": id, "kind": spec.kind().as_str(), "spec": spec, "status": status,
        "created_by": "alice", "created_at": NOW, "isolation": "host", "steps": [],
        "log_lines": 2
    });
    if let Some(progress) = progress {
        job["progress"] = progress;
    }
    if finished {
        job["finished_at"] = json!(NOW);
        job["exit_code"] = json!(0);
    }
    if status == "succeeded" {
        job["result"] = result_for(spec);
    }
    typed::<Job>(job)
}

pub fn demo_spec() -> JobSpec {
    serde_json::from_value(json!({"kind": "demo", "params": {"experiment": EXP}})).unwrap()
}

pub fn job_page() -> Value {
    typed::<Page<Job>>(
        json!({"items": [job(JOB_ID, &demo_spec(), "succeeded", None)],
                              "next_cursor": "c2"}),
    )
}

pub fn log_chunk(from: u64) -> Value {
    typed::<LogChunk>(json!({
        "lines": [
            {"n": from, "ts": NOW, "stream": "system", "text": "starting harness2-demo"},
            {"n": from + 1, "ts": NOW, "stream": "stdout", "text": "[contracts] 1/4 ok t1"}
        ],
        "next": from + 2,
        "complete": true
    }))
}

pub fn task_page() -> Value {
    typed::<Page<TaskRow>>(json!({"items": [{
        "task_id": TASK, "slug": TASK, "domain": "contracts", "status": "ok",
        "modes": [{"mode": "parallel", "ok": true, "chosen_label": "agg1", "chosen_is_base": false,
                   "fell_back": 0, "usd": 0.0, "score": 0.75, "all_pass": false}],
        "errors": 0
    }]}))
}

pub fn task_detail() -> Value {
    typed::<TaskDetail>(json!({
        "task_id": TASK, "slug": TASK, "domain": "contracts", "k": 2, "status": "ok",
        "base": [{"label": "base1", "run_id": "r-b1", "available": true}],
        "modes": [{
            "mode": "parallel", "ok": true, "finals": ["agg1", "agg2"],
            "stages": [{"label": "inv1", "dim": "INV", "fell_back": false, "kept_base": false,
                        "attempts": 1, "usd": 0.0, "cached": false,
                        "notes": "Tightened the system prompt.", "error": ""}],
            "selection": {"ok": true, "chosen_label": "agg1", "chosen_run_id": "r-a1",
                          "chosen_is_base": false, "id_map": [{"id": "A", "label": "agg1"}],
                          "n_candidates": 2, "why": "complete deliverable", "trivial": false,
                          "fallback": false, "dropped": [], "usd": 0.0, "error": "",
                          "evidence": "deliverable"},
            "errors": [],
            "harnesses": [{"label": "base", "role": "base", "chosen": false},
                          {"label": "agg1", "role": "final", "chosen": true}]
        }],
        "errors": []
    }))
}

pub fn harness_tree() -> Value {
    typed::<HarnessTree>(json!({
        "label": "agg1", "role": "final", "surface": "opencode",
        "files": [{"path": "systemprompt.md", "component": "system_prompt", "bytes": 42,
                   "content": "You are careful.\n"}],
        "census": census(), "fingerprint": "0123456789abcdef"
    }))
}

pub fn harness_diff() -> Value {
    typed::<HarnessDiff>(json!({
        "from": "base", "to": "agg1",
        "files": [{"path": "systemprompt.md", "component": "system_prompt", "change": "modified",
                   "unified": "@@ -1 +1 @@\n-You are helpful.\n+You are careful.\n",
                   "lines_added": 1, "lines_removed": 1}],
        "always_on_char_delta": 3
    }))
}

pub fn export_bundle() -> Value {
    typed::<ExportBundle>(json!({
        "target": "opencode", "label": "agg1",
        "files": [{"path": "AGENTS.md", "content": "You are careful.\n"}],
        "notes": ["Plugins need OpenCode 1.14 or newer."]
    }))
}

pub fn validation() -> Value {
    typed::<ValidationReport>(
        json!({"ok": true, "errors": [], "off_surface": [], "census": census()}),
    )
}

pub fn file_entries() -> Value {
    typed::<Vec<FileEntry>>(json!([
        {"name": "out", "path": "out", "kind": "dir", "bytes": 0},
        {"name": "report.md", "path": "out/report.md", "kind": "file", "bytes": 9}
    ]))
}

pub fn file_content() -> Value {
    typed::<FileContent>(
        json!({"path": "out/report.md", "bytes": 9, "text": "# Report\n",
                                "truncated": false, "mime": "text/markdown"}),
    )
}

pub fn scores() -> Value {
    typed::<Scores>(json!({
        "rows": [{"task_id": TASK, "mode": "parallel", "k": 2, "label": "agg1", "run_id": "r-a1",
                  "chosen_is_base": false, "n_passed": 3, "n_criteria": 4, "all_pass": false,
                  "frac": 0.75, "graded": true, "error": "", "metric": "criteria"}],
        "base_rows": []
    }))
}

pub fn score_report() -> Value {
    typed::<ScoreReport>(json!({
        "title": "demo k=2", "bench_name": "demo",
        "summaries": [{"mode": "parallel", "k": 2, "n": 4, "n_ungraded": 0, "score": 62.5,
                       "all_pass": 25.0, "kept_base": 1, "chose_final": 3, "chose_candidate": 0,
                       "fell_back": 0, "usd": 0.0, "metrics": ["criteria"]}],
        "notes": ["NOTE: k=1 sits at the noise floor."],
        "base_score": 50.0,
        "text": "demo parallel k=2 62.5"
    }))
}

pub fn library() -> Value {
    typed::<Vec<LibraryDomain>>(json!([{
        "mode": "parallel", "domain": "contracts",
        "entries": [{"task_slug": TASK, "adopted_label": "agg1", "run_id": "r-a1",
                     "components": "1 files; 42 always-on chars", "why": "complete deliverable"}]
    }]))
}

pub fn audit_sessions() -> Value {
    typed::<Vec<AuditSession>>(json!([{
        "id": SESSION, "label": "inv1", "ok": true, "channel": "editor", "model": "glm-5",
        "effort": "medium", "attempts": 1, "wall_s": 3.5, "cost_usd": 0.0, "error": "",
        "parts": ["mission", "system", "response"]
    }]))
}

pub fn comparison() -> Value {
    typed::<Comparison>(json!({
        "a": format!("{EXP}/{SWEEP}/parallel"), "b": format!("{EXP}/{SWEEP}/sequential"),
        "tasks": [{"task_id": TASK, "domain": "contracts", "a": 0.75, "b": 0.5, "delta": -0.25}],
        "n_common": 1, "mean_a": 0.75, "mean_b": 0.5, "mean_delta": -0.25,
        "wins": 0, "losses": 1, "ties": 0, "sign_test_p": 1.0
    }))
}

pub fn manifest() -> Value {
    typed::<Manifest>(json!({
        "schema_version": 1, "generated_at": NOW, "studio_version": "0.1.0", "experiment": EXP,
        "harness2_commit": "1cfe254000000000000000000000000000000000",
        "dependency_pins": [{"key": "opencode", "value": "1.14.0"}],
        "models": [{"key": "solver", "value": "glm-5"}],
        "env_names": ["HARNESS2_RUNS_DIR"],
        "isolation": "host", "network_policy": "host network",
        "jobs": [{"id": JOB_ID, "kind": "demo", "status": "succeeded",
                  "argv": ["harness2-demo", "--output", "E"], "created_at": NOW,
                  "finished_at": NOW}],
        "sweep_identities": [{"sweep": SWEEP, "identity_json": "{}"}],
        "results_digest": "sha256:00"
    }))
}

pub fn error_body(code: &str, message: &str, details: Value) -> Value {
    typed::<ErrorBody>(
        json!({"error": {"code": code, "message": message, "details": details,
                                        "request_id": "req-1"}}),
    )
}

// --------------------------------------------------------------------------- mock Workbench

/// `POST /api/v1/jobs`: answers 201 with a job built from the posted spec (400 when the body
/// is not a valid `JobSpec`).
pub struct JobFactory {
    pub status: &'static str,
}

impl Respond for JobFactory {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        match request.body_json::<JobSpec>() {
            Ok(spec) => {
                ResponseTemplate::new(201).set_body_json(job(JOB_ID, &spec, self.status, None))
            }
            Err(e) => ResponseTemplate::new(400).set_body_json(error_body(
                "bad_request",
                &format!("not a JobSpec: {e}"),
                json!([]),
            )),
        }
    }
}

/// Answers with the given bodies in turn, repeating the last one.
pub struct Sequence {
    pub bodies: Vec<Value>,
    pub calls: AtomicUsize,
}

impl Sequence {
    pub fn new(bodies: Vec<Value>) -> Self {
        Self {
            bodies,
            calls: AtomicUsize::new(0),
        }
    }
}

impl Respond for Sequence {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        let body = &self.bodies[n.min(self.bodies.len() - 1)];
        ResponseTemplate::new(200).set_body_json(body.clone())
    }
}

/// A GET route that requires `token`.
pub fn get_route(route: &str, token: &str) -> MockBuilder {
    Mock::given(method("GET"))
        .and(path(route.to_owned()))
        .and(bearer_token(token))
}

/// An unsafe-method route that requires `token` and the CSRF header.
pub fn unsafe_route(verb: &str, route: &str, token: &str) -> MockBuilder {
    Mock::given(method(verb))
        .and(path(route.to_owned()))
        .and(bearer_token(token))
        .and(header("x-h2w-request", "1"))
}

pub fn api(rest: &str) -> String {
    format!("/api/v1{rest}")
}

pub fn sweep_route(rest: &str) -> String {
    api(&format!("/experiments/{EXP}/sweeps/{SWEEP}{rest}"))
}

pub fn task_route(rest: &str) -> String {
    sweep_route(&format!("/tasks/{TASK}{rest}"))
}

pub struct Workbench {
    pub server: MockServer,
}

impl Workbench {
    pub async fn start() -> Self {
        Self {
            server: MockServer::start().await,
        }
    }

    pub fn url(&self) -> String {
        self.server.uri()
    }

    async fn json(&self, mock: MockBuilder, body: Value) {
        mock.respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&self.server)
            .await;
    }

    /// Every route the tools call, answering for `token` (as user `name` with `role`).
    pub async fn mount_happy(&self, token: &str, name: &str, role: &str) {
        self.json(get_route(&api("/auth/me"), token), principal(name, role))
            .await;
        self.json(get_route(&api("/system/info"), token), system_info())
            .await;
        self.json(get_route(&api("/system/doctor"), token), doctor())
            .await;
        self.json(get_route(&api("/benchmarks"), token), benchmarks())
            .await;
        self.json(get_route(&api("/profiles"), token), profiles())
            .await;
        self.json(
            get_route(&api("/profiles/local/check"), token),
            json!({"warnings": [], "missing_secrets": []}),
        )
        .await;
        self.json(get_route(&api("/splits"), token), splits()).await;
        self.json(
            get_route(&api("/experiments"), token),
            json!([experiment(EXP, false)]),
        )
        .await;
        unsafe_route("POST", &api("/experiments"), token)
            .respond_with(ResponseTemplate::new(201).set_body_json(experiment("exp1", false)))
            .mount(&self.server)
            .await;
        self.json(
            get_route(&api(&format!("/experiments/{EXP}")), token),
            experiment(EXP, false),
        )
        .await;
        self.json(
            unsafe_route("PATCH", &api(&format!("/experiments/{EXP}")), token),
            experiment(EXP, true),
        )
        .await;
        self.json(
            get_route(&api(&format!("/experiments/{EXP}/manifest")), token),
            manifest(),
        )
        .await;
        self.json(unsafe_route("POST", &api("/estimate"), token), estimate())
            .await;
        self.json(
            unsafe_route("POST", &api("/harness/validate"), token),
            validation(),
        )
        .await;
        unsafe_route("POST", &api("/jobs"), token)
            .respond_with(JobFactory {
                status: "succeeded",
            })
            .mount(&self.server)
            .await;
        self.json(get_route(&api("/jobs"), token), job_page()).await;
        self.json(
            get_route(&api(&format!("/jobs/{JOB_ID}")), token),
            job(JOB_ID, &demo_spec(), "succeeded", None),
        )
        .await;
        self.json(
            unsafe_route("POST", &api(&format!("/jobs/{JOB_ID}/cancel")), token),
            job(JOB_ID, &demo_spec(), "cancelled", None),
        )
        .await;
        self.json(
            get_route(&api(&format!("/jobs/{JOB_ID}/log")), token),
            log_chunk(0),
        )
        .await;
        self.json(get_route(&sweep_route("/tasks"), token), task_page())
            .await;
        self.json(get_route(&task_route(""), token), task_detail())
            .await;
        self.json(
            get_route(&task_route("/modes/parallel/harnesses/agg1"), token),
            harness_tree(),
        )
        .await;
        self.json(
            get_route(&task_route("/modes/parallel/diff"), token),
            harness_diff(),
        )
        .await;
        self.json(
            get_route(&task_route("/modes/parallel/harnesses/agg1/export"), token),
            export_bundle(),
        )
        .await;
        self.json(
            get_route(&task_route("/modes/parallel/rollouts/agg1/files"), token),
            file_entries(),
        )
        .await;
        self.json(
            get_route(&task_route("/modes/parallel/rollouts/agg1/file"), token),
            file_content(),
        )
        .await;
        self.json(get_route(&sweep_route("/scores"), token), scores())
            .await;
        self.json(get_route(&sweep_route("/report"), token), score_report())
            .await;
        self.json(get_route(&sweep_route("/library"), token), library())
            .await;
        self.json(get_route(&sweep_route("/audit"), token), audit_sessions())
            .await;
        get_route(&sweep_route(&format!("/audit/{SESSION}/response")), token)
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(AUDIT_TEXT, "text/plain; charset=utf-8"),
            )
            .mount(&self.server)
            .await;
        self.json(get_route(&api("/compare"), token), comparison())
            .await;
    }

    /// Every request answers `status` with an `ErrorBody` of `code` and `message`.
    pub async fn mount_refusing(&self, status: u16, code: &str, message: &str) {
        Mock::given(wiremock::matchers::any())
            .respond_with(ResponseTemplate::new(status).set_body_json(error_body(
                code,
                message,
                json!([]),
            )))
            .mount(&self.server)
            .await;
    }

    pub async fn received(&self) -> Vec<Request> {
        self.server.received_requests().await.unwrap_or_default()
    }

    /// Every request carried `token`, and every unsafe one the CSRF header.
    pub async fn assert_well_formed(&self, token: &str) {
        let requests = self.received().await;
        assert!(!requests.is_empty(), "no request reached the mock server");
        for r in &requests {
            let auth = r.headers.get("authorization").and_then(|v| v.to_str().ok());
            assert_eq!(
                auth,
                Some(format!("Bearer {token}").as_str()),
                "{} {} without the caller's bearer token",
                r.method,
                r.url
            );
            if !matches!(r.method.as_str(), "GET" | "HEAD" | "OPTIONS") {
                assert_eq!(
                    r.headers.get("x-h2w-request").and_then(|v| v.to_str().ok()),
                    Some("1"),
                    "{} {} without x-h2w-request: 1",
                    r.method,
                    r.url
                );
            }
        }
    }
}

/// Requests to `route` (path only) with `verb`.
pub fn requests_to<'a>(requests: &'a [Request], verb: &str, route: &str) -> Vec<&'a Request> {
    requests
        .iter()
        .filter(|r| r.method.as_str() == verb && r.url.path() == route)
        .collect()
}

/// Wait (up to 5 s) until the Workbench has received at least `n` requests for `route`.
pub async fn until_requests(workbench: &Workbench, verb: &str, route: &str, n: usize) {
    for _ in 0..500 {
        if requests_to(&workbench.received().await, verb, route).len() >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the Workbench never saw {n} {verb} {route}");
}

/// The query of a request as sorted `key=value` pairs.
pub fn query(request: &Request) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = request
        .url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    pairs.sort();
    pairs
}

// ---------------------------------------------------------------------------- MCP client

/// An MCP client that records progress notifications.
#[derive(Clone, Default)]
pub struct TestClient {
    pub progress: Arc<Mutex<Vec<ProgressNotificationParam>>>,
}

impl ClientHandler for TestClient {
    async fn on_progress(
        &self,
        params: ProgressNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        self.progress.lock().unwrap().push(params);
    }
}

pub type Client = RunningService<RoleClient, TestClient>;

/// An in-process MCP session (stdio credentials) against the Workbench at `url`.
pub async fn connect(url: &str, token: &str) -> Client {
    connect_with(url, token, WaitLimiter::default()).await
}

/// [`connect`] with a given wait limiter.
pub async fn connect_with(url: &str, token: &str, waits: WaitLimiter) -> Client {
    connect_configured(url, token, waits, false).await
}

/// [`connect`] to a server that passes `acknowledge_risks=true` on (as if a human had started
/// it with `--allow-risk-acknowledgement`).
pub async fn connect_allowing_risk_acknowledgement(url: &str, token: &str) -> Client {
    connect_configured(url, token, WaitLimiter::default(), true).await
}

async fn connect_configured(
    url: &str,
    token: &str,
    waits: WaitLimiter,
    allow_risk_acknowledgement: bool,
) -> Client {
    let api = ApiClient::new(url, &ClientOptions::default()).unwrap();
    let server = H2sMcp::new(api, Credentials::Fixed(Token::parse(token).unwrap()))
        .with_wait_limiter(waits)
        .with_risk_acknowledgement(allow_risk_acknowledgement);
    let (server_io, client_io) = tokio::io::duplex(1 << 20);
    tokio::spawn(async move {
        if let Ok(running) = server.serve(server_io).await {
            let _ = running.waiting().await;
        }
    });
    TestClient::default().serve(client_io).await.unwrap()
}

pub async fn call(client: &Client, tool: &str, args: Value) -> CallToolResult {
    let mut params = CallToolRequestParams::new(tool.to_owned());
    if let Value::Object(map) = args {
        params = params.with_arguments(map);
    }
    client
        .call_tool(params)
        .await
        .unwrap_or_else(|e| panic!("{tool}: protocol error {e}"))
}

/// The first text block of a result.
pub fn text(result: &CallToolResult) -> String {
    result
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.clone())
        .unwrap_or_default()
}

/// The first text block parsed as JSON.
pub fn payload(result: &CallToolResult) -> Value {
    serde_json::from_str(&text(result))
        .unwrap_or_else(|e| panic!("first text block is not JSON ({e}): {}", text(result)))
}

/// Wait until `client` has recorded at least `n` progress notifications (they are handled
/// asynchronously), or a second passes.
pub async fn progress_after(client: &Client, n: usize) -> Vec<ProgressNotificationParam> {
    for _ in 0..100 {
        let seen = client.service().progress.lock().unwrap().clone();
        if seen.len() >= n {
            return seen;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    client.service().progress.lock().unwrap().clone()
}
