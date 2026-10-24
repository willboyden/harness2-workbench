//! Shared integration-test harness: a fake harness2 home whose `.venv/bin/harness2*` are
//! stub scripts printing upstream's output formats, a test [`JobBackend`] that plans
//! those stubs, and a real server on `127.0.0.1:0` with tokens for each role.
//!
//! Scenarios are selected by the experiment name's first `-` segment
//! (`ok-x`, `slow-x`, `partial-x`, `fail-x`, `sleep-x`, `stubborn-x`, `chatty-x`,
//! `flood-x`, `secret-x`, `budget-x`, `longline-x`).

#![allow(
    dead_code,
    unreachable_pub,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    clippy::missing_panics_doc
)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use h2s_api::{
    DemoResult, ExperimentKind, FieldProblem, Job, JobResult, JobSpec, JobStatus, ModeNamespace,
    PreviewResult, Role, RunResult,
};
use h2s_core::parse::{BridgeSettings, TaskProgress};
use h2s_daemon::config::{Config, FileConfig};
use h2s_daemon::http::server::{RunningServer, Server, ServerOptions};
use h2s_daemon::jobs::backend::{
    ExitSemantics, JobBackend, JobPlan, ParseKind, PlanInput, StepOutput, StepPlan,
};
use serde_json::{Value, json};
use zeroize::Zeroizing;

pub const DEMO_STUB: &str = r##"#!/bin/sh
# Fake harness2-demo (test stub). Output format of harness2 1cfe254 cli/demo.py and
# core/schedule.py; behaviour from $FAKE_SCENARIO.
out=""
while [ $# -gt 0 ]; do
  case "$1" in
    --output) out="$2"; shift 2 ;;
    *) shift ;;
  esac
done
# The production backend sets no FAKE_SCENARIO: these come from the experiment's name.
if [ -z "$FAKE_SCENARIO" ]; then
  case "$out" in
    */fifo-*) FAKE_SCENARIO=fifo ;;
    */symlink-*) FAKE_SCENARIO=symlink ;;
  esac
fi
case "$FAKE_SCENARIO" in
  fifo)
    mkfifo "$out/demo.json"
    exit 0 ;;
  symlink)
    printf '{"domain":"accounting","n_tasks":2,"seed":null,"done":2,"failed":0,"tasks":[],"execution_count":28,"editor_calls":32}\n' > "$out/real.json"
    ln -s "$out/real.json" "$out/demo.json"
    exit 0 ;;
  fail)
    echo "starting"
    echo "error: the deterministic editor crashed" >&2
    exit 1 ;;
  sleep)
    sleep 300 &
    echo $! > "$out/child.pid"
    echo "waiting for the editor"
    wait
    exit 0 ;;
  stubborn)
    trap '' TERM
    sleep 300 &
    echo $! > "$out/child.pid"
    echo "ignoring SIGTERM"
    wait
    exit 0 ;;
  longline)
    head -c 200000 /dev/zero | tr '\0' 'x'
    echo
    echo "after the long line" ;;
  chatty)
    i=0
    while [ $i -lt "${FAKE_LINES:-1500}" ]; do echo "line $i"; i=$((i+1)); done ;;
  flood)
    # About 5 MB of output: 5000 lines of 1000 characters.
    yes "$(printf '%01000d' 0)" | head -n 5000 ;;
  secret)
    echo "the key is $FAKE_SECRET_VALUE" ;;
  slow)
    sleep "${FAKE_SLEEP:-1}" ;;
esac
echo "  [accounting] 1/2 ok           ledger-1"
if [ "$FAKE_SCENARIO" = partial ]; then
  echo "  [accounting] 2/2 partial      ledger-2  :: base-failed"
  done=1; failed=1
else
  echo "  [accounting] 2/2 ok           ledger-2"
  done=2; failed=0
fi
printf '{"domain":"accounting","n_tasks":2,"seed":null,"done":%s,"failed":%s,"tasks":[],"execution_count":28,"editor_calls":32}\n' "$done" "$failed" > "$out/demo.json"
echo "Local demo: $done tasks completed; $failed failed. $out/demo.json"
[ "$FAKE_SCENARIO" = partial ] && exit 2
exit 0
"##;

pub const HARNESS2_STUB: &str = r##"#!/bin/sh
# Fake harness2 (refinement sweep; --dry-run = preview). Formats of cli/run_sweep.py.
dry=0
for a in "$@"; do [ "$a" = "--dry-run" ] && dry=1; done
if [ "$dry" = 1 ]; then
  echo "# lab-oc split=all seed=None k=1 modes=parallel"
  echo "#   namespace[parallel]: lab-oc-agg1"
  echo "#   summaries under: $HARNESS2_RUNS_DIR/lab/adapt/lab-oc-agg1"
  echo "#   library root:    $HARNESS2_RUNS_DIR/lab/library/lab/k1-oc"
  echo "#   domains parallel: 1"
  echo "   1  contracts                                task-a"
  echo "   2  contracts                                task-b"
  echo "   3  litigation                               task-c"
  echo "# domains of this split, with task counts (the legal --only-domains names):"
  echo "   2  contracts"
  echo "   1  litigation"
  echo "# 3 tasks over 2 domains (domains parallel, tasks within a domain sequential)"
  exit 0
fi
echo "sweep lab-oc-agg1: 1 domain(s) in parallel, tasks within a domain in stream order"
if [ "$FAKE_SCENARIO" = budget ]; then
  i=0
  while [ $i -lt 600 ]; do
    i=$((i+1))
    printf '{"usd": %d.%d}\n' $((i/10)) $((i%10)) > "$HARNESS2_RUNS_DIR/spend.json.tmp"
    mv "$HARNESS2_RUNS_DIR/spend.json.tmp" "$HARNESS2_RUNS_DIR/spend.json"
    echo "  [contracts] $i/600 ok           task-$i"
    sleep 0.2
  done
fi
echo "  [contracts] 1/2 ok           task-a"
echo "  [contracts] 2/2 partial      task-b  :: judge failed"
echo "  [litigation] 1/1 skipped      task-c"
echo ""
echo "sweep lab-oc-agg1: 2 tasks ok, 1 failed, 0 domain(s) died"
echo "record: $HARNESS2_RUNS_DIR/lab/adapt/_sweeps/lab-oc-agg1-20261023T000000Z.json"
echo "next:   python3 -m harness2.cli.judge --bench lab --substrate oc --k 1 --modes parallel"
exit 2
"##;

pub const BRIDGE_STUB: &str = r##"#!/bin/sh
# Fake harness2-bridge: prints its settings like cli/bridge.py, then serves (sleeps).
echo "Add these to .env, then run harness2-setup <bench> --substrate cx:"
echo "HARNESS2_CODEX_BASE_URL=http://127.0.0.1:4100/v1"
echo "HARNESS2_CODEX_API_KEY=$FAKE_BRIDGE_KEY"
echo "HARNESS2_CODEX_MODEL=gpt-test"
echo "litellm: master key loaded ($FAKE_BRIDGE_KEY)"
exec sleep 300
"##;

pub const CHECK_STUB: &str = r##"#!/bin/sh
# Fake harness2-check (cli/check.py output); fails for experiments named *failcheck*.
case "$HARNESS2_RUNS_DIR" in
  *failcheck*)
    echo "FAIL: dependencies/harvey-labs is missing; run harness2-setup lab"
    echo "Setup incomplete: 1 issue(s). See docs/BENCHMARKS.md."
    exit 1 ;;
esac
echo "Ready: lab/oc; 3 tasks. Model access is not exercised; the first real run is the first model call."
exit 0
"##;

pub const JUDGE_STUB: &str = r##"#!/bin/sh
# Fake harness2-judge (cli/judge.py output, captured in tests/fixtures/cli/std).
for a in "$@"; do
  if [ "$a" = "--bases" ]; then
    echo "  contracts/msa-redline                                base1 1/4"
    echo "  litigation/motion-draft                              base1 -"
    echo "wrote $HARNESS2_RUNS_DIR/lab/adapt/lab-oc-agg1/base_judged.json: 2 row(s), 1 graded"
    exit 2
  fi
done
echo "judging 3 selection(s) under lab-oc-agg1 ..."
echo "judged.json: 3 row(s), 2 graded, 1 ungraded (judge failure or no pick — NOT zeros)"
echo "wrote $HARNESS2_RUNS_DIR/lab/adapt/lab-oc-agg1/judged.json"
exit 2
"##;

pub const REPORT_STUB: &str = r##"#!/bin/sh
# Fake harness2-report (cli/report.py output).
echo "lab-oc modes=parallel"
echo "========================================"
echo "  parallel k=1: n=  2 | partial 100.00% | pass 100.0% | picked output: final 2 / candidate 0 / base 0"
exit 0
"##;

/// Write the fake harness2 home: `<dir>/.venv/bin/harness2*`, mode 0755.
pub fn fake_harness2_home(dir: &Path) {
    let bin = dir.join(".venv/bin");
    std::fs::create_dir_all(&bin).unwrap();
    for (name, body) in [
        ("harness2-demo", DEMO_STUB),
        ("harness2", HARNESS2_STUB),
        ("harness2-bridge", BRIDGE_STUB),
        ("harness2-check", CHECK_STUB),
        ("harness2-judge", JUDGE_STUB),
        ("harness2-report", REPORT_STUB),
        ("harness2-setup", CHECK_STUB),
    ] {
        let p = bin.join(name);
        std::fs::write(&p, body).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// The scenario of an experiment-bound spec.
fn scenario(spec: &JobSpec) -> String {
    spec.experiment()
        .and_then(|e| e.split('-').next())
        .unwrap_or("ok")
        .to_owned()
}

/// A backend that plans the stub scripts (no `h2s-core` needed).
pub struct FakeBackend {
    pub home: PathBuf,
    pub secret_value: String,
    pub bridge_key: String,
    pub sleep_secs: f64,
}

impl FakeBackend {
    fn step(
        &self,
        name: &str,
        bin: &str,
        args: Vec<String>,
        env: Vec<(String, String)>,
    ) -> StepPlan {
        let program = self.home.join(".venv/bin").join(bin).display().to_string();
        let mut display = vec![bin.to_owned()];
        display.extend(args.iter().cloned());
        StepPlan {
            name: name.to_owned(),
            program,
            args,
            cwd: self.home.clone(),
            env,
            runtime_env: false,
            container: None,
            exit: ExitSemantics::Harness2,
            parse: ParseKind::None,
            progress: false,
            budget: false,
            bridge: false,
            display,
        }
    }
}

impl JobBackend for FakeBackend {
    fn check_names(&self, spec: &JobSpec) -> Vec<FieldProblem> {
        let ok = |s: &str| {
            !s.is_empty()
                && s.len() <= 63
                && s.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        };
        match spec.experiment() {
            Some(e) if !ok(e) => vec![FieldProblem {
                field: "params.experiment".into(),
                problem: "lowercase letters, digits and -".into(),
            }],
            _ => Vec::new(),
        }
    }

    fn plan(&self, input: &PlanInput<'_>) -> Result<JobPlan, String> {
        let sc = scenario(input.spec);
        let runs = input
            .experiment
            .map(|(p, _)| p.display().to_string())
            .unwrap_or_else(|| input.job_dir.join("runs").display().to_string());
        let env = vec![
            ("FAKE_SCENARIO".to_owned(), sc),
            ("HARNESS2_RUNS_DIR".to_owned(), runs.clone()),
            ("FAKE_SECRET_VALUE".to_owned(), self.secret_value.clone()),
            ("FAKE_BRIDGE_KEY".to_owned(), self.bridge_key.clone()),
            ("FAKE_SLEEP".to_owned(), self.sleep_secs.to_string()),
        ];
        let mut plan = JobPlan::default();
        if !self.secret_value.is_empty() {
            plan.secrets.push((
                "FAKE_SECRET".to_owned(),
                Zeroizing::new(self.secret_value.clone()),
            ));
        }
        match input.spec {
            JobSpec::Demo(p) => {
                let mut args = vec![
                    "--output".into(),
                    runs,
                    "--k".into(),
                    p.k.to_string(),
                    "--mode".into(),
                    p.mode.as_cli().into(),
                ];
                if scenario(input.spec) == "argvleak" {
                    args.push(format!("--token={}", self.secret_value));
                }
                let mut s = self.step("demo", "harness2-demo", args, env);
                s.parse = ParseKind::Demo;
                s.progress = true;
                plan.steps.push(s);
            }
            JobSpec::Run(p) => {
                let args = vec![
                    "--bench".into(),
                    p.cell.bench.as_cli().into(),
                    "--k".into(),
                    p.options.k.to_string(),
                ];
                let mut pre_args = args.clone();
                pre_args.push("--dry-run".into());
                let mut pre = self.step("preview", "harness2", pre_args, env.clone());
                pre.parse = ParseKind::Preview;
                plan.steps.push(pre);
                let mut run = self.step("run", "harness2", args, env);
                run.parse = ParseKind::Run;
                run.progress = true;
                run.budget = true;
                plan.steps.push(run);
            }
            JobSpec::Check(_) => {
                let mut s = self.step(
                    "check",
                    "harness2-check",
                    vec!["--bench".into(), "lab".into()],
                    env,
                );
                s.parse = ParseKind::Check;
                plan.steps.push(s);
            }
            JobSpec::Bridge(_) => {
                let mut s = self.step(
                    "bridge",
                    "harness2-bridge",
                    vec!["--model".into(), "gpt-test".into()],
                    env,
                );
                s.parse = ParseKind::Bridge;
                s.bridge = true;
                plan.steps.push(s);
            }
            other => {
                return Err(format!(
                    "the fake backend does not run {}",
                    other.kind().as_str()
                ));
            }
        }
        Ok(plan)
    }

    fn parse_step(
        &self,
        step: &StepPlan,
        out: &StepOutput<'_>,
    ) -> Result<Option<JobResult>, String> {
        if step
            .env
            .iter()
            .any(|(k, v)| k == "FAKE_SCENARIO" && v == "panic")
        {
            panic!("simulated parser bug");
        }
        match step.parse {
            ParseKind::Demo => {
                let Some((dir, _)) = out.experiment else {
                    return Ok(None);
                };
                match std::fs::read_to_string(dir.join("demo.json")) {
                    Ok(t) => {
                        let v: Value = serde_json::from_str(&t).map_err(|e| e.to_string())?;
                        let n = |k: &str| u32::try_from(v[k].as_u64().unwrap_or(0)).unwrap();
                        Ok(Some(JobResult::Demo(DemoResult {
                            done: n("done"),
                            failed: n("failed"),
                            execution_count: n("execution_count"),
                            editor_calls: n("editor_calls"),
                        })))
                    }
                    Err(_) => Ok(None),
                }
            }
            ParseKind::Preview => {
                let total = out
                    .stdout
                    .lines()
                    .find_map(|l| {
                        l.strip_prefix("# ")
                            .and_then(|r| r.split(" tasks over").next()?.parse::<u32>().ok())
                    })
                    .ok_or("no task total")?;
                Ok(Some(JobResult::Preview(PreviewResult {
                    header: out.stdout.lines().next().unwrap_or_default().to_owned(),
                    namespaces: vec![ModeNamespace {
                        mode: h2s_api::Mode::Parallel,
                        namespace: "lab-oc-agg1".into(),
                    }],
                    summary_namespace: "lab-oc-agg1".into(),
                    workers: 1,
                    stream: vec![],
                    domains: vec![],
                    total_tasks: total,
                    total_domains: 2,
                    estimate: h2s_core::estimate::estimate(
                        total,
                        1,
                        &[h2s_api::Mode::Parallel],
                        None,
                        None,
                    ),
                })))
            }
            ParseKind::Run => Ok(out.stdout.lines().find_map(|l| {
                let rest = l.strip_prefix("sweep lab-oc-agg1: ")?;
                let done = rest.split(" tasks ok").next()?.parse().ok()?;
                let failed = rest.split(", ").nth(1)?.split(' ').next()?.parse().ok()?;
                Some(JobResult::Run(RunResult {
                    namespace: "lab-oc-agg1".into(),
                    done,
                    failed,
                    dead_domains: vec![],
                    record: Some("lab-oc-agg1-20261023T000000Z.json".into()),
                    next: None,
                }))
            })),
            _ => Ok(None),
        }
    }

    fn progress_line(&self, line: &str) -> Option<TaskProgress> {
        let rest = line.trim_start().strip_prefix('[')?;
        let (domain, rest) = rest.split_once("] ")?;
        let mut parts = rest.split_whitespace();
        let (i, n) = parts.next()?.split_once('/')?;
        let status = parts.next()?.to_owned();
        let task = parts.next()?.to_owned();
        Some(TaskProgress {
            domain: domain.to_owned(),
            index: i.parse().ok()?,
            total: n.parse().ok()?,
            status,
            task,
            error: None,
        })
    }

    fn bridge_line(&self, line: &str, acc: &mut BridgeSettings) -> bool {
        // Deliberately does not capture the key: the daemon's own safety net must.
        if let Some(v) = line.strip_prefix("HARNESS2_CODEX_BASE_URL=") {
            acc.base_url = v.to_owned();
            return true;
        }
        if let Some(v) = line.strip_prefix("HARNESS2_CODEX_MODEL=") {
            acc.model = v.to_owned();
            return true;
        }
        line.starts_with("HARNESS2_CODEX_")
    }

    fn spend_usd(&self, dir: &Path, _kind: ExperimentKind, _ns: &str) -> Option<f64> {
        let t = std::fs::read_to_string(dir.join("spend.json")).ok()?;
        serde_json::from_str::<Value>(&t).ok()?["usd"].as_f64()
    }
}

/// A random alphanumeric string.
pub fn random_text(n: usize) -> String {
    let bytes: [u8; 48] = h2s_daemon::util::random_bytes().unwrap();
    let s: String = h2s_daemon::util::b64url(&bytes)
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    s[..n.min(s.len())].to_owned()
}

/// Test configuration under `root`.
pub fn test_file_config(root: &Path) -> FileConfig {
    let mut f = FileConfig::default();
    f.server.listen = "127.0.0.1:0".into();
    f.server.public_url = "http://127.0.0.1:1".into();
    f.paths.data_dir = root.join("data").display().to_string();
    f.paths.harness2_home = root.join("harness2").display().to_string();
    f.paths.harness2_managed = false;
    f.jobs.budget_poll_secs = 1;
    f.telemetry.log_level = "debug".into();
    f
}

pub struct TestServer {
    pub root: tempfile::TempDir,
    pub srv: Option<RunningServer>,
    pub client: reqwest::Client,
    pub admin: String,
    pub operator: String,
    pub viewer: String,
    pub secret_value: String,
    pub bridge_key: String,
}

pub struct Setup {
    pub file: FileConfig,
    pub kill_grace: Duration,
    pub secret_env: Vec<(String, String)>,
    pub proxy_secret: Option<String>,
    pub install_harness2: bool,
    pub sleep_secs: f64,
    /// Plan jobs with the production backend (`h2s-core`) instead of [`FakeBackend`].
    pub core_backend: bool,
    /// Event-stream heartbeat (the server's default when `None`).
    pub sse_heartbeat: Option<Duration>,
}

impl Setup {
    pub fn new(root: &Path) -> Setup {
        Setup {
            file: test_file_config(root),
            kill_grace: Duration::from_secs(2),
            secret_env: Vec::new(),
            proxy_secret: None,
            install_harness2: true,
            sleep_secs: 1.0,
            core_backend: false,
            sse_heartbeat: None,
        }
    }
}

pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap()
}

impl TestServer {
    pub async fn start() -> TestServer {
        Self::start_with(|_| {}).await
    }

    pub async fn start_with(f: impl FnOnce(&mut Setup)) -> TestServer {
        let root = tempfile::tempdir().unwrap();
        let mut setup = Setup::new(root.path());
        f(&mut setup);
        Self::start_in(root, setup).await
    }

    pub async fn start_in(root: tempfile::TempDir, setup: Setup) -> TestServer {
        if setup.install_harness2 {
            fake_harness2_home(&root.path().join("harness2"));
        } else {
            std::fs::create_dir_all(root.path().join("harness2")).unwrap();
        }
        let secret_value = format!("sv-{}", random_text(32));
        let bridge_key = format!("bk-{}", random_text(32));
        let srv = start_server(root.path(), &setup, &secret_value, &bridge_key).await;
        let mut t = TestServer {
            root,
            srv: Some(srv),
            client: client(),
            admin: String::new(),
            operator: String::new(),
            viewer: String::new(),
            secret_value,
            bridge_key,
        };
        t.admin = t.token("admin-t", Role::Admin, None).await;
        t.operator = t.token("operator-t", Role::Operator, None).await;
        t.viewer = t.token("viewer-t", Role::Viewer, None).await;
        t
    }

    pub fn srv(&self) -> &RunningServer {
        self.srv.as_ref().unwrap()
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.srv().base_url())
    }

    pub fn port(&self) -> u16 {
        self.srv().addr.port()
    }

    /// Mint a token directly in the database.
    pub async fn token(&self, name: &str, role: Role, expires_at: Option<i64>) -> String {
        let id = h2s_daemon::auth::new_token_id().unwrap();
        let minted = h2s_daemon::auth::mint(&id).unwrap();
        let rec = h2s_daemon::db::tokens::TokenRecord {
            id: minted.id.clone(),
            secret_hash: minted.secret_hash,
            name: name.to_owned(),
            role,
            created_at: h2s_daemon::util::now_unix(),
            created_by: "test".into(),
            expires_at,
            revoked_at: None,
            last_used_at: None,
        };
        self.srv()
            .state
            .db
            .call(move |c| h2s_daemon::db::tokens::insert(c, &rec))
            .await
            .unwrap();
        minted.token.as_str().to_owned()
    }

    pub fn get(&self, path: &str, token: Option<&str>) -> reqwest::RequestBuilder {
        let mut r = self.client.get(self.url(path));
        if let Some(t) = token {
            r = r.bearer_auth(t);
        }
        r
    }

    pub fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        token: Option<&str>,
    ) -> reqwest::RequestBuilder {
        let mut r = self
            .client
            .request(method, self.url(path))
            .header("x-h2s-request", "1");
        if let Some(t) = token {
            r = r.bearer_auth(t);
        }
        r
    }

    pub fn post(&self, path: &str, token: Option<&str>) -> reqwest::RequestBuilder {
        self.send(reqwest::Method::POST, path, token)
    }

    pub async fn create_job(&self, token: &str, spec: Value) -> reqwest::Response {
        self.post("/api/v1/jobs", Some(token))
            .json(&spec)
            .send()
            .await
            .unwrap()
    }

    pub async fn job(&self, id: &str) -> Job {
        let r = self
            .get(&format!("/api/v1/jobs/{id}"), Some(&self.viewer))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
        r.json().await.unwrap()
    }

    /// Poll until the job satisfies `pred` (panics after `secs`).
    pub async fn wait_job(&self, id: &str, secs: u64, pred: impl Fn(&Job) -> bool) -> Job {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
        loop {
            let j = self.job(id).await;
            if pred(&j) {
                return j;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting; job is {j:?}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    pub async fn wait_terminal(&self, id: &str, secs: u64) -> Job {
        self.wait_job(id, secs, |j| j.status.is_terminal()).await
    }

    /// Insert a profile directly (PUT /profiles validates through h2s-core).
    pub async fn put_profile(&self, profile: Value) {
        let p: h2s_api::ModelProfile = serde_json::from_value(profile).unwrap();
        self.srv()
            .state
            .db
            .call(move |c| h2s_daemon::db::profiles::put(c, &p, 1))
            .await
            .unwrap();
    }

    /// Insert a secret directly.
    pub async fn put_secret(&self, name: &str, value: &str) {
        let (n, v) = (name.to_owned(), value.to_owned());
        self.srv()
            .state
            .db
            .call(move |c| h2s_daemon::db::secrets::set(c, &n, &v, 1))
            .await
            .unwrap();
    }

    /// Insert a standard experiment directly (POST /experiments validates through h2s-core).
    pub async fn put_experiment(&self, name: &str, profile: Option<&str>) {
        let rec = h2s_daemon::db::experiments::ExperimentRecord {
            name: name.to_owned(),
            kind: ExperimentKind::Standard,
            description: String::new(),
            tags: vec![],
            profile: profile.map(str::to_owned),
            created_at: 1,
            created_by: "test".into(),
            archived: false,
        };
        let root = self.root.path().join("data/experiments");
        h2s_daemon::expdirs::create_or_adopt(&root, name).unwrap();
        self.srv()
            .state
            .db
            .call(move |c| h2s_daemon::db::experiments::insert(c, &rec))
            .await
            .unwrap();
    }

    pub fn experiment_dir(&self, name: &str) -> PathBuf {
        self.root.path().join("data/experiments").join(name)
    }

    pub async fn stop(mut self) -> tempfile::TempDir {
        if let Some(s) = self.srv.take() {
            s.stop().await;
        }
        self.root
    }
}

pub async fn start_server(
    root: &Path,
    setup: &Setup,
    secret_value: &str,
    bridge_key: &str,
) -> RunningServer {
    let cfg = Config::from_file(setup.file.clone(), None, &|_| None).unwrap();
    let backend: Arc<dyn JobBackend> = if setup.core_backend {
        Arc::new(h2s_daemon::jobs::core_backend::CoreBackend)
    } else {
        Arc::new(FakeBackend {
            home: root.join("harness2"),
            secret_value: secret_value.to_owned(),
            bridge_key: bridge_key.to_owned(),
            sleep_secs: setup.sleep_secs,
        })
    };
    let mut opts = ServerOptions::new(backend);
    opts.kill_grace = Some(setup.kill_grace);
    opts.sse_heartbeat = setup.sse_heartbeat;
    opts.proxy_secret.clone_from(&setup.proxy_secret);
    let vars = setup.secret_env.clone();
    let names: Vec<String> = vars
        .iter()
        .filter_map(|(k, _)| k.strip_prefix("H2S_SECRET_").map(str::to_owned))
        .collect();
    opts.secret_env = Some((
        Arc::new(move |n: &str| vars.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone())),
        names,
    ));
    Server::bind(cfg, opts).await.unwrap().spawn()
}

pub fn demo_spec(experiment: &str) -> Value {
    json!({"kind": "demo", "params": {"experiment": experiment, "k": 2, "mode": "both"}})
}

pub fn run_spec(experiment: &str, profile: &str, budget: Option<f64>) -> Value {
    let mut p = json!({
        "experiment": experiment,
        "cell": {"bench": "lab", "substrate": "oc"},
        "options": {"k": 1, "modes": ["parallel"]},
        "profile": profile,
        "acknowledge_risks": true,
    });
    if let Some(b) = budget {
        p["budget_usd"] = json!(b);
    }
    json!({"kind": "run", "params": p})
}

/// One parsed server-sent event.
#[derive(Debug, Clone, Default)]
pub struct SseFrame {
    pub event: Option<String>,
    pub id: Option<String>,
    pub data: String,
    pub raw: String,
}

/// Split an SSE byte stream into frames.
pub fn parse_sse(text: &str) -> Vec<SseFrame> {
    let mut out = Vec::new();
    for block in text.split("\n\n") {
        if block.trim().is_empty() {
            continue;
        }
        let mut f = SseFrame {
            raw: block.to_owned(),
            ..SseFrame::default()
        };
        let mut data = Vec::new();
        for line in block.lines() {
            if let Some(v) = line.strip_prefix("data:") {
                data.push(v.strip_prefix(' ').unwrap_or(v).to_owned());
            } else if let Some(v) = line.strip_prefix("event:") {
                f.event = Some(v.trim().to_owned());
            } else if let Some(v) = line.strip_prefix("id:") {
                f.id = Some(v.trim().to_owned());
            }
        }
        f.data = data.join("\n");
        out.push(f);
    }
    out
}

/// Send raw bytes and read the whole response (connection: close).
pub async fn raw_http(addr: std::net::SocketAddr, request: &[u8]) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(request).await.unwrap();
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut buf)).await;
    String::from_utf8_lossy(&buf).into_owned()
}

/// `true` when the process is gone (or a zombie awaiting its reaper).
pub fn process_gone(pid: i32) -> bool {
    h2s_daemon::jobs::procfs::stat(pid).is_none_or(|s| s.state == 'Z')
}

pub async fn wait_gone(pid: i32, secs: u64) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    while tokio::time::Instant::now() < deadline {
        if process_gone(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    process_gone(pid)
}

pub async fn wait_file(path: &Path, secs: u64) -> String {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        if let Ok(t) = std::fs::read_to_string(path)
            && !t.trim().is_empty()
        {
            return t;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{} never appeared",
            path.display()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// All lines of a job log through the API.
pub async fn full_log(t: &TestServer, id: &str) -> Vec<h2s_api::LogLine> {
    let mut from = 0;
    let mut out = Vec::new();
    loop {
        let r = t
            .get(
                &format!("/api/v1/jobs/{id}/log?from={from}&limit=5000"),
                Some(&t.viewer),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        let chunk: h2s_api::LogChunk = r.json().await.unwrap();
        let n = chunk.lines.len();
        out.extend(chunk.lines);
        from = chunk.next;
        if chunk.complete || n == 0 {
            return out;
        }
    }
}

/// Copy a fixture experiment (tests/fixtures/<name>) to `to`, rewriting the neutral
/// `/fixture/<name>` prefix to the copy's real path, as harness2 would have recorded it.
pub fn copy_fixture(name: &str, to: &Path) {
    let from = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name);
    let prefix = format!("/fixture/{name}");
    let real = to.display().to_string();
    fn walk(from: &Path, to: &Path, prefix: &str, real: &str) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            let target = to.join(e.file_name());
            let ft = e.file_type().unwrap();
            if ft.is_dir() {
                walk(&e.path(), &target, prefix, real);
            } else if ft.is_file() {
                let bytes = std::fs::read(e.path()).unwrap();
                match String::from_utf8(bytes) {
                    Ok(text) => std::fs::write(&target, text.replace(prefix, real)).unwrap(),
                    Err(e) => std::fs::write(&target, e.into_bytes()).unwrap(),
                }
            }
        }
    }
    walk(&from, to, &prefix, &real);
}

pub fn assert_terminal(j: &Job, status: JobStatus) {
    assert_eq!(j.status, status, "job: {j:#?}");
}
