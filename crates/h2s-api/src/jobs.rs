//! Jobs: every harness2 operation runs as a job with a live log, a status and a parsed result.
//!
//! `POST /api/v1/jobs` takes a [`JobSpec`] (`{"kind": "...", "params": {...}}`) and returns
//! the created [`Job`]. Progress streams over `GET /api/v1/jobs/{id}/events` (SSE carrying
//! [`JobEvent`] JSON) and the full log pages through `GET /api/v1/jobs/{id}/log`.

use serde::{Deserialize, Serialize};

use crate::{Bench, Cell, DemoMode, Mode, Role, RunOptions, Substrate, WbSubset};

/// What to run. Each variant maps to one harness2 command (or a managed sequence of them).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", content = "params", rename_all = "snake_case")]
pub enum JobSpec {
    /// Fetch harness2 at a pinned commit, create its Python 3.12 venv and install it.
    InstallHarness2(InstallParams),
    /// `harness2-demo`: the real recursion and library workflow with deterministic local
    /// editors. No model credentials, no network, about a second.
    Demo(DemoParams),
    /// `harness2-setup <bench>`: clone pinned upstream benchmark code, apply patches, build
    /// environments, download task data. Network-heavy; admin only.
    Setup(SetupParams),
    /// `harness2-check`: offline readiness checks (no model calls).
    Check(CheckParams),
    /// `harness2 --dry-run`: the exact task stream, namespaces and domains, plus a cost
    /// estimate. No model calls, nothing written.
    Preview(PreviewParams),
    /// `harness2`: the refinement sweep (costs money with cloud models).
    Run(RunParams),
    /// `harness2-judge`: score the selected (or with `bases`, the reference) executions.
    Judge(JudgeParams),
    /// `harness2-report`: the benchmark's figures from cached judge rows.
    Report(ReportParams),
    /// `scripts/split_tasks.py`: a reproducible domain-stratified train/test partition.
    Split(SplitParams),
    /// `harness2-bridge`: a local Responses-API endpoint on Vertex AI for the Codex
    /// executor. Long-running until cancelled; admin only.
    Bridge(BridgeParams),
    /// check -> run -> judge (-> judge --bases) -> report, stopping at the first failure.
    Pipeline(PipelineParams),
}

impl JobSpec {
    /// Minimum role allowed to start this job.
    #[must_use]
    pub fn required_role(&self) -> Role {
        match self {
            JobSpec::InstallHarness2(_) | JobSpec::Setup(_) | JobSpec::Bridge(_) => Role::Admin,
            JobSpec::Demo(_)
            | JobSpec::Check(_)
            | JobSpec::Preview(_)
            | JobSpec::Run(_)
            | JobSpec::Judge(_)
            | JobSpec::Report(_)
            | JobSpec::Split(_)
            | JobSpec::Pipeline(_) => Role::Operator,
        }
    }

    #[must_use]
    pub fn kind(&self) -> JobKind {
        match self {
            JobSpec::InstallHarness2(_) => JobKind::InstallHarness2,
            JobSpec::Demo(_) => JobKind::Demo,
            JobSpec::Setup(_) => JobKind::Setup,
            JobSpec::Check(_) => JobKind::Check,
            JobSpec::Preview(_) => JobKind::Preview,
            JobSpec::Run(_) => JobKind::Run,
            JobSpec::Judge(_) => JobKind::Judge,
            JobSpec::Report(_) => JobKind::Report,
            JobSpec::Split(_) => JobKind::Split,
            JobSpec::Bridge(_) => JobKind::Bridge,
            JobSpec::Pipeline(_) => JobKind::Pipeline,
        }
    }

    /// The experiment this job reads or writes, if any.
    #[must_use]
    pub fn experiment(&self) -> Option<&str> {
        match self {
            JobSpec::Demo(p) => Some(&p.experiment),
            JobSpec::Preview(p) => Some(&p.experiment),
            JobSpec::Run(p) => Some(&p.experiment),
            JobSpec::Judge(p) => Some(&p.experiment),
            JobSpec::Report(p) => Some(&p.experiment),
            JobSpec::Pipeline(p) => Some(&p.experiment),
            JobSpec::InstallHarness2(_)
            | JobSpec::Setup(_)
            | JobSpec::Check(_)
            | JobSpec::Split(_)
            | JobSpec::Bridge(_) => None,
        }
    }

    /// True when this job can call paid model endpoints.
    #[must_use]
    pub fn may_spend(&self) -> bool {
        matches!(
            self,
            JobSpec::Run(_) | JobSpec::Judge(_) | JobSpec::Pipeline(_) | JobSpec::Bridge(_)
        )
    }
}

/// The variant name of a [`JobSpec`], for filtering and display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    InstallHarness2,
    Demo,
    Setup,
    Check,
    Preview,
    Run,
    Judge,
    Report,
    Split,
    Bridge,
    Pipeline,
}

impl JobKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            JobKind::InstallHarness2 => "install_harness2",
            JobKind::Demo => "demo",
            JobKind::Setup => "setup",
            JobKind::Check => "check",
            JobKind::Preview => "preview",
            JobKind::Run => "run",
            JobKind::Judge => "judge",
            JobKind::Report => "report",
            JobKind::Split => "split",
            JobKind::Bridge => "bridge",
            JobKind::Pipeline => "pipeline",
        }
    }
}

/// Optional extras of the harness2 Python package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum InstallExtra {
    /// openpyxl + python-pptx (WorkBuddy office rendering). Always installed.
    Render,
    /// LiteLLM proxy for `harness2-bridge` (Codex on Vertex AI).
    Bridge,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct InstallParams {
    /// Full 40-hex commit of google-research/harness2. `None` = the commit this Studio
    /// release was tested against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    /// Optional extras: `render` (always installed) and `bridge` (the LiteLLM proxy `harness2-bridge` needs).
    #[serde(default)]
    pub extras: Vec<InstallExtra>,
    /// Re-create the virtual environment even if harness2 is already installed.
    #[serde(default)]
    pub reinstall: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DemoParams {
    /// Experiment that receives the demo output (created as a demo experiment if missing).
    pub experiment: String,
    /// Recursion budget for the demo (1 to 8).
    #[serde(default = "default_demo_k")]
    pub k: u32,
    /// Which recursion modes the demo runs.
    #[serde(default)]
    pub mode: DemoMode,
}

fn default_demo_k() -> u32 {
    2
}

/// Highest `k` the server accepts for the demo (it multiplies local executions only).
pub const DEMO_K_MAX: u32 = 8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SetupParams {
    pub bench: Bench,
    /// wb only. `None` for wb installs all three subsets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subset: Option<WbSubset>,
    /// Executor framework (`oc` OpenCode or `cx` Codex).
    #[serde(default = "default_substrate")]
    pub substrate: Substrate,
    /// Print the setup plan without changing anything.
    #[serde(default)]
    pub dry_run: bool,
    /// Model profile whose settings setup bakes in (WorkBuddy routes, Codex config).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

fn default_substrate() -> Substrate {
    Substrate::Oc
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CheckParams {
    /// Benchmark, WorkBuddy subset and executor: the "cell" every harness2 command takes.
    pub cell: Cell,
    /// Profile whose model settings are checked. `None` = the default profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PreviewParams {
    /// Experiment name (one HARNESS2_RUNS_DIR managed by the server).
    pub experiment: String,
    /// Benchmark, WorkBuddy subset and executor: the "cell" every harness2 command takes.
    pub cell: Cell,
    /// Run identity and run control options of the sweep.
    pub options: RunOptions,
    /// Model profile for model settings and price assumptions; absent = the experiment's profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunParams {
    /// Experiment name (one HARNESS2_RUNS_DIR managed by the server).
    pub experiment: String,
    /// Benchmark, WorkBuddy subset and executor: the "cell" every harness2 command takes.
    pub cell: Cell,
    /// Run identity and run control options of the sweep.
    pub options: RunOptions,
    /// Model profile (executor, improver, judge). Must match the experiment's profile.
    pub profile: String,
    /// Stop the job once recorded spend reaches this many USD (checked as task summaries
    /// land; a task in flight finishes first). `None` = no cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_usd: Option<f64>,
    /// Must be `true` when the server runs jobs without container isolation: the executor
    /// then has the server user's filesystem and network (upstream behaviour).
    #[serde(default)]
    pub acknowledge_risks: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct JudgeParams {
    /// Experiment name (one HARNESS2_RUNS_DIR managed by the server).
    pub experiment: String,
    /// Benchmark, WorkBuddy subset and executor: the "cell" every harness2 command takes.
    pub cell: Cell,
    /// The sweep's recursion budget k (part of its run identity: use the k the run used).
    pub k: u32,
    /// The sweep's stream-order seed, if the run used one (part of its run identity; absent when the run had no seed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// Recursion modes to judge: the same ones the run used.
    pub modes: Vec<Mode>,
    /// Score the reference (base) executions into `base_judged.json` instead.
    #[serde(default)]
    pub bases: bool,
    /// Model profile name; must be the profile the experiment is pinned to.
    pub profile: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ReportParams {
    /// Experiment name (one HARNESS2_RUNS_DIR managed by the server).
    pub experiment: String,
    /// Benchmark, WorkBuddy subset and executor: the "cell" every harness2 command takes.
    pub cell: Cell,
    /// Recursion budgets to report together (`--k 1,2,3`).
    pub ks: Vec<u32>,
    /// The sweep's stream-order seed, if the run used one (part of its run identity; absent when the run had no seed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// Recursion modes to report: the same ones the run used.
    pub modes: Vec<Mode>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SplitParams {
    pub bench: Bench,
    /// WorkBuddy-Bench subset (wb only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subset: Option<WbSubset>,
    /// Seed of the stratified shuffle (default 42).
    #[serde(default = "default_split_seed")]
    pub seed: u64,
    /// Fraction of each domain's tasks that go to `train` (exclusive 0..1; default 0.5).
    #[serde(default = "default_train_fraction")]
    pub train_fraction: f64,
    /// Name for the split file (same rules as experiment names). Must not exist yet.
    pub name: String,
}

fn default_split_seed() -> u64 {
    42
}

fn default_train_fraction() -> f64 {
    0.5
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BridgeParams {
    /// Vertex model id to serve. `None` = the profile's Codex model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Listen on all interfaces so WorkBuddy's containers can reach it (upstream
    /// `--host 0.0.0.0`). Off = loopback only.
    #[serde(default)]
    pub expose_to_containers: bool,
    /// TCP port for the bridge; absent = reuse the saved port or pick a free one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Profile supplying the Vertex project and the Codex model.
    pub profile: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PipelineParams {
    /// Experiment name (one HARNESS2_RUNS_DIR managed by the server).
    pub experiment: String,
    /// Benchmark, WorkBuddy subset and executor: the "cell" every harness2 command takes.
    pub cell: Cell,
    /// Run identity and run control options of the sweep.
    pub options: RunOptions,
    /// Model profile name; must be the profile the experiment is pinned to.
    pub profile: String,
    /// Also score the reference executions, so the report can compare against the base.
    #[serde(default)]
    pub judge_bases: bool,
    /// Stop the job once recorded spend reaches this many USD (improver spend; harness2 does not record executor rollout spend). Absent = no cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_usd: Option<f64>,
    /// Must be `true` when the server runs jobs without container isolation (the executor then has the server user's files and network). Set it only with the user's informed consent.
    #[serde(default)]
    pub acknowledge_risks: bool,
}

/// Where a job's processes run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Isolation {
    /// Directly on the server host as the server's user (upstream behaviour).
    Host,
    /// Inside a hardened container from the configured runner image.
    Container,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    /// Exit 0: everything requested completed.
    Succeeded,
    /// harness2 exit 2: finished, but some tasks failed or stayed ungraded.
    Partial,
    Failed,
    Cancelled,
}

impl JobStatus {
    #[must_use]
    pub fn is_terminal(self) -> bool {
        !matches!(self, JobStatus::Queued | JobStatus::Running)
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            JobStatus::Queued => "queued",
            JobStatus::Running => "running",
            JobStatus::Succeeded => "succeeded",
            JobStatus::Partial => "partial",
            JobStatus::Failed => "failed",
            JobStatus::Cancelled => "cancelled",
        }
    }
}

/// A job as returned by the API.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Job {
    /// Job id (UUIDv7).
    pub id: String,
    /// Which harness2 operation the job runs.
    pub kind: JobKind,
    /// What the job runs, as submitted.
    pub spec: JobSpec,
    /// Lifecycle state.
    pub status: JobStatus,
    /// Display name of the principal that created it.
    pub created_by: String,
    /// Creation time (RFC 3339, UTC).
    pub created_at: String,
    /// Start time (RFC 3339, UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    /// End time (RFC 3339, UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    /// Process exit code, when the process has exited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Where the job's processes run.
    pub isolation: Isolation,
    /// One entry per process the job runs (a pipeline has several).
    #[serde(default)]
    pub steps: Vec<JobStep>,
    /// Task progress, when the job reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<Progress>,
    /// Parsed outcome, once the job has finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<JobResult>,
    /// One-line failure reason, safe to display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Lines in the job log so far.
    pub log_lines: u64,
    /// Spend recorded in task summaries so far (USD), for jobs that may spend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spent_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct JobStep {
    /// e.g. `check`, `run`, `judge`, `judge-bases`, `report`, `fetch`, `venv`, `install`.
    pub name: String,
    /// Lifecycle state of this step.
    pub status: JobStatus,
    /// Process exit code, when the process has exited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Start time (RFC 3339, UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    /// End time (RFC 3339, UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
}

/// Task progress parsed from harness2's `[domain] i/n status task` lines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Progress {
    /// Tasks finished `ok` or `skipped`.
    pub done: u32,
    /// Tasks finished with any other status (disjoint from `done`).
    pub failed: u32,
    /// Total tasks in the stream when known (from a preview or the sweep header).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u32>,
    /// Most recent `domain / task` line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
}

/// Parsed outcome of a finished job, by kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobResult {
    Install(InstallResult),
    Demo(DemoResult),
    Setup(SetupResult),
    Check(CheckResult),
    Preview(PreviewResult),
    Run(RunResult),
    Judge(JudgeResult),
    Report(ReportResult),
    Split(SplitResult),
    Bridge(BridgeInfo),
    Pipeline(PipelineResult),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct InstallResult {
    /// The 40-hex harness2 commit now installed.
    pub commit: String,
    /// `harness2` package version from its metadata.
    pub version: String,
    /// Python version of the venv, e.g. `3.12.14`.
    pub python: String,
    /// Optional package extras installed.
    pub extras: Vec<InstallExtra>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DemoResult {
    /// Demo tasks completed.
    pub done: u32,
    /// Demo tasks that failed.
    pub failed: u32,
    /// Local executions the demo ran.
    pub execution_count: u32,
    /// Deterministic editor calls the demo made.
    pub editor_calls: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SetupResult {
    /// Nothing was changed; `plan` lists what setup would do.
    pub dry_run: bool,
    /// The commands setup ran (or, for a dry run, would run), one per line.
    pub plan: Vec<String>,
    /// Notes setup printed (`Note: ...`, WorkBuddy model requirements, ...).
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CheckResult {
    /// harness2-check found nothing missing.
    pub ready: bool,
    /// Installed task count, from `Ready: ...; N tasks.`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tasks: Option<u32>,
    /// Each `FAIL: ...` line, without the prefix.
    pub failures: Vec<String>,
}

/// One task in the sweep's stream order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct StreamEntry {
    /// 1-based position in the stream.
    pub index: u32,
    /// Domain (practice area, occupation or category) the task belongs to.
    pub domain: String,
    /// Task id.
    pub task: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DomainCount {
    /// Domain (practice area, occupation or category) the task belongs to.
    pub domain: String,
    /// Number of tasks.
    pub tasks: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ModeNamespace {
    /// Recursion mode.
    pub mode: Mode,
    /// e.g. `lab-oc-agg1-s7`.
    pub namespace: String,
}

/// Parsed `harness2 --dry-run` output plus the server's estimate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PreviewResult {
    /// The `# <bench>-<sub> split=... seed=... k=... modes=...` header line.
    pub header: String,
    /// The namespace each requested mode writes.
    pub namespaces: Vec<ModeNamespace>,
    /// Namespace whose task dirs hold `summary.json` for both modes (the parallel one).
    pub summary_namespace: String,
    /// Domains run concurrently.
    pub workers: u32,
    /// Every selected task in stream order.
    pub stream: Vec<StreamEntry>,
    /// Domains in the selection with their task counts.
    pub domains: Vec<DomainCount>,
    /// Tasks in the selection.
    pub total_tasks: u32,
    /// Domains in the selection.
    pub total_domains: u32,
    /// The server's size and cost estimate for this selection.
    pub estimate: CostEstimate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunResult {
    /// Summary namespace of the sweep, e.g. `lab-oc-agg1`.
    pub namespace: String,
    /// Tasks completed (`ok` or resumed).
    pub done: u32,
    /// Tasks that did not complete.
    pub failed: u32,
    /// Domains whose worker died; their remaining tasks did not run.
    pub dead_domains: Vec<String>,
    /// File name of the sweep record under `adapt/_sweeps/`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<String>,
    /// harness2's suggested next command, verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct JudgeResult {
    /// The reference executions were scored (`base_judged.json`).
    pub bases: bool,
    /// Rows in the judge cache after this run.
    pub rows: u32,
    /// Rows the judge graded.
    pub graded: u32,
    /// Judge failures or no pick: NOT zeros, excluded from denominators.
    pub ungraded: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ReportResult {
    /// `harness2-report` output verbatim (the publication-format block).
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SplitResult {
    /// Split file name.
    pub name: String,
    /// Tasks in the train partition.
    pub train: u32,
    /// Tasks in the test partition.
    pub test: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BridgeInfo {
    /// `HARNESS2_CODEX_BASE_URL` the bridge serves. Its key is stored as the secret
    /// `HARNESS2_CODEX_API_KEY` and never returned.
    pub base_url: String,
    /// Vertex model id the bridge serves.
    pub model: String,
    /// URL containers use to reach the bridge (`HARNESS2_CODEX_CONTAINER_URL`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PipelineResult {
    /// Results of the steps that finished, in order.
    pub steps: Vec<JobResult>,
}

/// Cost and size of a sweep before running it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CostEstimate {
    /// Tasks in the selection.
    pub tasks: u32,
    /// Recursion modes counted.
    pub modes: u32,
    /// Recursion budget.
    pub k: u32,
    /// Executor rollouts: k shared base runs + per mode 3k (2k candidates + k finals).
    pub rollouts: u64,
    /// Improver sessions: per task and mode 3k, plus a selection session per mode when
    /// k > 1 and a reflection session per adopted harness.
    pub improver_sessions: u64,
    /// USD range when prices are known (from the profile or past runs in this experiment).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usd_low: Option<f64>,
    /// High end of the USD range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usd_high: Option<f64>,
    /// `heuristic` (profile prices) or `history` (measured from earlier runs).
    pub basis: String,
    /// Plain-language caveats (judging not included, retries, ...).
    pub notes: Vec<String>,
}

/// Body of `POST /api/v1/estimate`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct EstimateRequest {
    /// Number of tasks in the sweep (for example `total_tasks` of a preview).
    pub tasks: u32,
    /// Recursion budget k (1 to 32).
    pub k: u32,
    /// Recursion modes to run.
    pub modes: Vec<Mode>,
    /// Model profile whose price assumptions to use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Use measured spend from this experiment's earlier runs when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experiment: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum LogStream {
    Stdout,
    Stderr,
    /// Lines the server writes itself (step boundaries, isolation notes, cancellation).
    System,
}

/// One log line. Secret values are replaced with `[REDACTED:<NAME>]` before storage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LogLine {
    /// 0-based line number within the job log.
    pub n: u64,
    /// Time of the event (RFC 3339, UTC).
    pub ts: String,
    /// Which output stream the line came from.
    pub stream: LogStream,
    /// Line text with secret values redacted.
    pub text: String,
}

/// `GET /api/v1/jobs/{id}/log?from=N&limit=M`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LogChunk {
    /// Log lines in order.
    pub lines: Vec<LogLine>,
    /// Pass as `from` to continue.
    pub next: u64,
    /// The job is terminal and every line has been returned.
    pub complete: bool,
}

/// Payload of each `data:` frame on `GET /api/v1/jobs/{id}/events`. The first frame is
/// always a `snapshot`; `heartbeat` arrives at least every 15 s.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobEvent {
    Snapshot {
        job: Box<Job>,
    },
    Status {
        status: JobStatus,
        exit_code: Option<i32>,
        error: Option<String>,
    },
    Log {
        line: LogLine,
    },
    Progress {
        progress: Progress,
    },
    /// A step of a multi-step job changed state.
    Step {
        step: JobStep,
    },
    Heartbeat {
        ts: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Bench, Cell, Substrate};

    #[test]
    fn job_spec_wire_shape() {
        let spec = JobSpec::Check(CheckParams {
            cell: Cell {
                bench: Bench::Lab,
                subset: None,
                substrate: Substrate::Oc,
            },
            profile: None,
        });
        let v = serde_json::to_value(&spec).unwrap();
        assert_eq!(v["kind"], "check");
        assert_eq!(v["params"]["cell"]["bench"], "lab");
        let back: JobSpec = serde_json::from_value(v).unwrap();
        assert_eq!(back, spec);
    }

    #[test]
    fn roles_per_kind() {
        let install = JobSpec::InstallHarness2(InstallParams {
            rev: None,
            extras: vec![],
            reinstall: false,
        });
        assert_eq!(install.required_role(), Role::Admin);
        let demo = JobSpec::Demo(DemoParams {
            experiment: "d".into(),
            k: 2,
            mode: DemoMode::Both,
        });
        assert_eq!(demo.required_role(), Role::Operator);
        assert!(!demo.may_spend());
        assert_eq!(demo.experiment(), Some("d"));
    }

    #[test]
    fn terminal_statuses() {
        assert!(!JobStatus::Queued.is_terminal());
        assert!(!JobStatus::Running.is_terminal());
        for s in [
            JobStatus::Succeeded,
            JobStatus::Partial,
            JobStatus::Failed,
            JobStatus::Cancelled,
        ] {
            assert!(s.is_terminal());
        }
    }

    #[test]
    fn job_event_tagging() {
        let e = JobEvent::Heartbeat {
            ts: "2026-10-23T00:00:00Z".into(),
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["type"], "heartbeat");
    }

    #[test]
    fn demo_defaults() {
        let p: DemoParams = serde_json::from_str(r#"{"experiment":"demo"}"#).unwrap();
        assert_eq!(p.k, 2);
        assert_eq!(p.mode, DemoMode::Both);
    }
}
