//! Tool input types.
//!
//! Where the REST API takes a body, the input embeds the `h2s_api` wire type (flattened, so a
//! tool's arguments are exactly the API fields plus a few MCP-only knobs such as `wait_secs`).
//! Path and query parameters keep the REST names: `experiment`, `sweep`, `task`, `mode`,
//! `label`, `from`, `to`, `path`, `session`, `part`, `job_id`.

use h2s_api::{
    BridgeParams, CheckParams, CreateExperimentRequest, DEMO_K_MAX, DemoParams, EstimateRequest,
    ExportTarget, InstallExtra, InstallParams, JobKind, JobStatus, JudgeParams, K_MAX,
    MAX_TASKS_MAX, Mode, PipelineParams, PreviewParams, ReportParams, RunParams, SetupParams,
    SplitParams, UpdateExperimentRequest, ValidateHarnessRequest, WORKERS_MAX,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::schema::{ToolInput, bounds, describe, redescribe};

/// Longest wait any tool performs (also the cap of `wait_for_job`).
pub(crate) const MAX_WAIT_SECS: u32 = 600;

const EXPERIMENT: &str = "Experiment name (see list_experiments).";
const PROFILE_PINNED: &str = "Model profile name (see list_profiles). It must be the profile the \
                              experiment is pinned to.";
const RUN_K: &str = "The sweep's recursion budget k (part of its run identity; use the same k \
                     the run used).";
const RUN_SEED: &str = "The sweep's stream-order seed, if the run used one (part of its run \
                        identity; omit when the run had no seed).";
const RUN_MODES: &str = "Recursion modes, the same ones the run used.";
const BUDGET: &str = "Stop the job once recorded spend reaches this many USD (checked as task \
                      summaries land; a task in flight finishes first). harness2 records \
                      improver spend only, not executor rollouts. Omit for no cap.";
const ACKNOWLEDGE_RISKS: &str = "Must be true when the server runs jobs without container \
                                 isolation (server_info shows isolation host): the executor then \
                                 has the server user's files and network. Set it only with the \
                                 user's informed consent.";

fn quick_wait() -> u32 {
    45
}

fn default_wait_timeout() -> u32 {
    50
}

fn default_poll() -> u32 {
    2
}

fn default_base() -> String {
    "base".to_owned()
}

/// Bounds the server enforces on [`h2s_api::RunOptions`], written into the schema.
fn run_options_bounds(schema: &mut Value) {
    bounds(schema, &["options", "k"], 1, u64::from(K_MAX));
    bounds(schema, &["options", "workers"], 1, u64::from(WORKERS_MAX));
    bounds(
        schema,
        &["options", "max_tasks"],
        1,
        u64::from(MAX_TASKS_MAX),
    );
}

fn wait_bounds(schema: &mut Value) {
    bounds(schema, &["wait_secs"], 0, u64::from(MAX_WAIT_SECS));
}

// ---------------------------------------------------------------------------------------------
// No arguments

/// No arguments.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct NoArgs {}

impl ToolInput for NoArgs {}

// ---------------------------------------------------------------------------------------------
// System

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct DoctorInput {
    /// Re-run every probe now instead of using the server's 60-second cache (operator role).
    #[serde(default)]
    pub(crate) refresh: bool,
}

impl ToolInput for DoctorInput {}

// ---------------------------------------------------------------------------------------------
// Experiments

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ListExperimentsInput {
    /// Also list archived experiments.
    #[serde(default)]
    pub(crate) include_archived: bool,
}

impl ToolInput for ListExperimentsInput {}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ExperimentInput {
    /// Experiment name (see list_experiments).
    pub(crate) experiment: String,
}

impl ToolInput for ExperimentInput {}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ProfileInput {
    /// Model profile name (see list_profiles).
    pub(crate) profile: String,
}

impl ToolInput for ProfileInput {}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct CreateExperimentInput {
    #[serde(flatten)]
    pub(crate) request: CreateExperimentRequest,
}

impl ToolInput for CreateExperimentInput {
    fn patch(schema: &mut Value) {
        describe(
            schema,
            &["name"],
            "Experiment name, also its directory name: lowercase letters, digits, '-' and '_', \
             1-63 characters, starting with a letter or digit.",
        );
        describe(
            schema,
            &["description"],
            "What the experiment is for (free text).",
        );
        describe(schema, &["tags"], "Labels for filtering experiments.");
        describe(
            schema,
            &["profile"],
            "Model profile to pin (see list_profiles). Runs, judging and pipelines in this \
             experiment must then use this profile.",
        );
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct UpdateExperimentInput {
    /// Experiment name (see list_experiments).
    pub(crate) experiment: String,
    #[serde(flatten)]
    pub(crate) request: UpdateExperimentRequest,
}

impl ToolInput for UpdateExperimentInput {
    fn patch(schema: &mut Value) {
        describe(
            schema,
            &["description"],
            "New description (replaces the old one). Omit to keep it.",
        );
        describe(
            schema,
            &["tags"],
            "New tag list (replaces all tags; [] removes them). Omit to keep them.",
        );
        describe(
            schema,
            &["archived"],
            "true archives the experiment (hidden from list_experiments, nothing deleted); false \
             restores it. Omit to keep it as is.",
        );
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct EstimateInput {
    #[serde(flatten)]
    pub(crate) request: EstimateRequest,
}

impl ToolInput for EstimateInput {
    fn patch(schema: &mut Value) {
        redescribe(
            schema,
            &["tasks"],
            "Number of tasks in the sweep (total_tasks from preview_run).",
        );
        describe(schema, &["k"], "Recursion budget k (1-32).");
        bounds(schema, &["k"], 1, u64::from(K_MAX));
        describe(schema, &["modes"], "Recursion modes the sweep would run.");
        describe(
            schema,
            &["profile"],
            "Model profile whose price assumptions to use (see list_profiles).",
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Sweep results

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SweepInput {
    /// Experiment name.
    pub(crate) experiment: String,
    /// Sweep id: the summary namespace, e.g. `lab-oc-agg1-s7` or `demo-oc-agg2` (see
    /// get_experiment → sweeps[].id).
    pub(crate) sweep: String,
}

impl ToolInput for SweepInput {}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ListTasksInput {
    /// Experiment name.
    pub(crate) experiment: String,
    /// Sweep id (summary namespace, see get_experiment).
    pub(crate) sweep: String,
    /// Only tasks of this domain (occupation, practice area or category).
    #[serde(default)]
    pub(crate) domain: Option<String>,
    /// Only tasks with this status as harness2 records it: ok, skipped, partial, base-failed,
    /// running.
    #[serde(default)]
    pub(crate) status: Option<String>,
    /// Page size (default 50; the server clamps it).
    #[serde(default)]
    pub(crate) limit: Option<u32>,
    /// `next_cursor` from the previous page.
    #[serde(default)]
    pub(crate) cursor: Option<String>,
}

impl ToolInput for ListTasksInput {
    fn patch(schema: &mut Value) {
        bounds(schema, &["limit"], 1, 1000);
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct TaskInput {
    /// Experiment name.
    pub(crate) experiment: String,
    /// Sweep id (summary namespace).
    pub(crate) sweep: String,
    /// Task slug (`slug` from list_tasks; task ids with '/' use '__').
    pub(crate) task: String,
}

impl ToolInput for TaskInput {}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct HarnessInput {
    /// Experiment name.
    pub(crate) experiment: String,
    /// Sweep id (summary namespace).
    pub(crate) sweep: String,
    /// Task slug (from list_tasks).
    pub(crate) task: String,
    /// Recursion mode whose harnesses to read.
    pub(crate) mode: Mode,
    /// Harness label: base, a candidate (inv1.., dom1..) or a final (agg1.. in parallel mode,
    /// c1.. in sequential mode); see get_task → modes[].harnesses.
    pub(crate) label: String,
}

impl ToolInput for HarnessInput {}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct DiffInput {
    /// Experiment name.
    pub(crate) experiment: String,
    /// Sweep id (summary namespace).
    pub(crate) sweep: String,
    /// Task slug (from list_tasks).
    pub(crate) task: String,
    /// Recursion mode of both harnesses.
    pub(crate) mode: Mode,
    /// Label of the old harness (default `base`).
    #[serde(default = "default_base")]
    pub(crate) from: String,
    /// Label of the new harness, e.g. the selector's choice (get_task →
    /// modes[].selection.chosen_label).
    pub(crate) to: String,
}

impl ToolInput for DiffInput {}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ExportInput {
    /// Experiment name.
    pub(crate) experiment: String,
    /// Sweep id (summary namespace).
    pub(crate) sweep: String,
    /// Task slug (from list_tasks).
    pub(crate) task: String,
    /// Recursion mode.
    pub(crate) mode: Mode,
    /// Harness label to export (usually the selector's chosen final).
    pub(crate) label: String,
    /// Project layout to produce (default `raw`).
    #[serde(default = "default_target")]
    pub(crate) target: ExportTarget,
}

fn default_target() -> ExportTarget {
    ExportTarget::Raw
}

impl ToolInput for ExportInput {}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct RolloutFilesInput {
    /// Experiment name.
    pub(crate) experiment: String,
    /// Sweep id (summary namespace).
    pub(crate) sweep: String,
    /// Task slug (from list_tasks).
    pub(crate) task: String,
    /// Recursion mode.
    pub(crate) mode: Mode,
    /// Label of the harness whose execution to browse (base, inv1, dom1, agg1, c1, ...).
    pub(crate) label: String,
    /// Directory inside the rollout, `/`-separated and relative (default: its root).
    #[serde(default)]
    pub(crate) path: String,
}

impl ToolInput for RolloutFilesInput {}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct RolloutFileInput {
    /// Experiment name.
    pub(crate) experiment: String,
    /// Sweep id (summary namespace).
    pub(crate) sweep: String,
    /// Task slug (from list_tasks).
    pub(crate) task: String,
    /// Recursion mode.
    pub(crate) mode: Mode,
    /// Label of the harness whose execution to read.
    pub(crate) label: String,
    /// File path inside the rollout, `/`-separated and relative (from list_rollout_files).
    pub(crate) path: String,
}

impl ToolInput for RolloutFileInput {}

/// Which text of an audit session to read.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AuditPart {
    /// The task brief the improver or selector was given.
    Mission,
    /// Its system prompt.
    System,
    /// What it answered, including proposed edits.
    Response,
}

impl AuditPart {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            AuditPart::Mission => "mission",
            AuditPart::System => "system",
            AuditPart::Response => "response",
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct AuditSessionInput {
    /// Experiment name.
    pub(crate) experiment: String,
    /// Sweep id (summary namespace).
    pub(crate) sweep: String,
    /// Session id from list_audit_sessions, e.g. `inv1.a1.3f9c0d2e11aa7b40`.
    pub(crate) session: String,
    /// Which part to read.
    pub(crate) part: AuditPart,
}

impl ToolInput for AuditSessionInput {}

/// One side of a comparison.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SweepModeRef {
    /// Experiment name.
    pub(crate) experiment: String,
    /// Sweep id (summary namespace).
    pub(crate) sweep: String,
    /// Recursion mode.
    pub(crate) mode: Mode,
}

impl SweepModeRef {
    /// The `<experiment>/<sweep>/<mode>` form `GET /compare` takes.
    pub(crate) fn query_value(&self) -> String {
        format!("{}/{}/{}", self.experiment, self.sweep, self.mode.as_cli())
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct CompareInput {
    /// Baseline side (deltas are b - a).
    pub(crate) a: SweepModeRef,
    /// Compared side.
    pub(crate) b: SweepModeRef,
}

impl ToolInput for CompareInput {}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ValidateInput {
    #[serde(flatten)]
    pub(crate) request: ValidateHarnessRequest,
}

impl ToolInput for ValidateInput {
    fn patch(schema: &mut Value) {
        describe(
            schema,
            &["files", "[]", "path"],
            "Path relative to the harness root, '/'-separated (e.g. systemprompt.md, \
             skills/review/SKILL.md).",
        );
        describe(schema, &["files", "[]", "content"], "The file's full text.");
    }
}

// ---------------------------------------------------------------------------------------------
// Jobs

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ListJobsInput {
    /// Only jobs in this state.
    #[serde(default)]
    pub(crate) status: Option<JobStatus>,
    /// Only jobs of this kind.
    #[serde(default)]
    pub(crate) kind: Option<JobKind>,
    /// Only jobs that read or write this experiment.
    #[serde(default)]
    pub(crate) experiment: Option<String>,
    /// Page size (default 10; the server clamps it).
    #[serde(default)]
    pub(crate) limit: Option<u32>,
    /// `next_cursor` from the previous page.
    #[serde(default)]
    pub(crate) cursor: Option<String>,
}

impl ToolInput for ListJobsInput {
    fn patch(schema: &mut Value) {
        bounds(schema, &["limit"], 1, 200);
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct JobInput {
    /// Job id (returned by the tool that started the job, or from list_jobs).
    pub(crate) job_id: String,
}

impl ToolInput for JobInput {}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct WaitInput {
    /// Job id (returned by the tool that started the job, or from list_jobs).
    pub(crate) job_id: String,
    /// Longest time to wait, in seconds (default 50, max 600). On timeout the job is returned
    /// as it is and keeps running.
    #[serde(default = "default_wait_timeout")]
    pub(crate) timeout_secs: u32,
    /// Seconds between status checks when the server's event stream is unavailable (default
    /// 2).
    #[serde(default = "default_poll")]
    pub(crate) poll_interval_secs: u32,
}

impl ToolInput for WaitInput {
    fn patch(schema: &mut Value) {
        bounds(schema, &["timeout_secs"], 1, u64::from(MAX_WAIT_SECS));
        bounds(schema, &["poll_interval_secs"], 1, 30);
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct JobLogInput {
    /// Job id.
    pub(crate) job_id: String,
    /// First line to return (0-based; default 0). Ignored when `tail` is set.
    #[serde(default)]
    pub(crate) from: u64,
    /// Lines to return (default 200, max 5000).
    #[serde(default)]
    pub(crate) limit: Option<u32>,
    /// Return the last N lines instead of paging from `from`.
    #[serde(default)]
    pub(crate) tail: Option<u32>,
}

impl ToolInput for JobLogInput {
    fn patch(schema: &mut Value) {
        bounds(schema, &["limit"], 1, 5000);
        bounds(schema, &["tail"], 1, 5000);
    }
}

/// Every job-starting tool: the job's parameters (flattened) plus how long to wait.
macro_rules! job_input {
    ($name:ident, $params:ty, $wait_default:literal, $wait_doc:literal, |$schema:ident| $patch:block) => {
        #[derive(Debug, Deserialize, JsonSchema)]
        pub(crate) struct $name {
            #[serde(flatten)]
            pub(crate) params: $params,
            #[doc = $wait_doc]
            #[serde(default = $wait_default)]
            pub(crate) wait_secs: u32,
        }

        impl ToolInput for $name {
            fn patch($schema: &mut Value) {
                wait_bounds($schema);
                $patch
            }
        }
    };
}

fn no_wait() -> u32 {
    0
}

job_input!(
    SetupInput,
    SetupParams,
    "no_wait",
    "Seconds to wait for the job before returning (default 0: return the queued job at once; \
     max 600). Setups usually take minutes: follow them with wait_for_job.",
    |schema| {
        describe(schema, &["bench"], "Benchmark to install.");
    }
);

job_input!(
    CheckInput,
    CheckParams,
    "quick_wait",
    "Seconds to wait for the result before returning (default 45, max 600; 0 = return the \
     queued job at once).",
    |schema| {}
);

job_input!(
    PreviewInput,
    PreviewParams,
    "quick_wait",
    "Seconds to wait for the preview before returning (default 45, max 600; 0 = return the \
     queued job at once).",
    |schema| {
        run_options_bounds(schema);
        describe(schema, &["experiment"], EXPERIMENT);
        redescribe(
            schema,
            &["profile"],
            "Model profile for model settings and price assumptions (default: the experiment's \
             profile).",
        );
    }
);

job_input!(
    RunInput,
    RunParams,
    "no_wait",
    "Seconds to wait before returning (default 0: return the queued job at once; max 600). \
     Sweeps take minutes to hours: follow them with wait_for_job.",
    |schema| {
        run_options_bounds(schema);
        describe(schema, &["experiment"], EXPERIMENT);
        redescribe(schema, &["budget_usd"], BUDGET);
        redescribe(schema, &["acknowledge_risks"], ACKNOWLEDGE_RISKS);
    }
);

job_input!(
    PipelineInput,
    PipelineParams,
    "no_wait",
    "Seconds to wait before returning (default 0: return the queued job at once; max 600). \
     Pipelines take minutes to hours: follow them with wait_for_job.",
    |schema| {
        run_options_bounds(schema);
        describe(schema, &["experiment"], EXPERIMENT);
        redescribe(schema, &["profile"], PROFILE_PINNED);
        redescribe(schema, &["budget_usd"], BUDGET);
        redescribe(schema, &["acknowledge_risks"], ACKNOWLEDGE_RISKS);
    }
);

job_input!(
    DemoInput,
    DemoParams,
    "quick_wait",
    "Seconds to wait for the demo before returning (default 45, max 600; 0 = return the \
     queued job at once).",
    |schema| {
        bounds(schema, &["k"], 1, u64::from(DEMO_K_MAX));
        redescribe(
            schema,
            &["mode"],
            "Recursion mode(s) the demo runs: parallel, sequential or both (default).",
        );
    }
);

job_input!(
    JudgeInput,
    JudgeParams,
    "no_wait",
    "Seconds to wait before returning (default 0: return the queued job at once; max 600). \
     Follow judging with wait_for_job.",
    |schema| {
        describe(schema, &["experiment"], EXPERIMENT);
        redescribe(schema, &["k"], RUN_K);
        bounds(schema, &["k"], 1, u64::from(K_MAX));
        redescribe(schema, &["seed"], RUN_SEED);
        redescribe(schema, &["modes"], RUN_MODES);
        redescribe(schema, &["profile"], PROFILE_PINNED);
    }
);

job_input!(
    ReportInput,
    ReportParams,
    "quick_wait",
    "Seconds to wait for the report before returning (default 45, max 600; 0 = return the \
     queued job at once).",
    |schema| {
        describe(schema, &["experiment"], EXPERIMENT);
        bounds(schema, &["ks", "[]"], 1, u64::from(K_MAX));
        redescribe(schema, &["seed"], RUN_SEED);
        redescribe(schema, &["modes"], RUN_MODES);
    }
);

job_input!(
    SplitInput,
    SplitParams,
    "quick_wait",
    "Seconds to wait for the split before returning (default 45, max 600; 0 = return the \
     queued job at once).",
    |schema| {
        describe(schema, &["bench"], "Benchmark whose tasks to partition.");
        describe(schema, &["subset"], "WorkBuddy subset (wb only).");
        describe(
            schema,
            &["seed"],
            "Shuffle seed of the partition (default 42).",
        );
        if let Some(Value::Object(node)) = crate::schema::node(schema, &["train_fraction"]) {
            node.insert("exclusiveMinimum".into(), 0.into());
            node.insert("exclusiveMaximum".into(), 1.into());
        }
    }
);

/// `install_harness2`. Unlike the API, there is no `rev`: an MCP client always installs the
/// commit this Studio release was tested against (audit F13: an arbitrary commit could be a
/// fork-only one whose build code runs on the server).
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct InstallInput {
    /// Optional extras: render (always installed) and bridge (the LiteLLM proxy start_bridge
    /// needs).
    #[serde(default)]
    pub(crate) extras: Vec<InstallExtra>,
    /// Re-create the virtual environment even if harness2 is already installed.
    #[serde(default)]
    pub(crate) reinstall: bool,
    /// Seconds to wait before returning (default 0: return the queued job at once; max 600).
    /// Installs take minutes: follow them with wait_for_job.
    #[serde(default = "no_wait")]
    pub(crate) wait_secs: u32,
}

impl InstallInput {
    /// The job parameters: the tested commit, always.
    pub(crate) fn params(&self) -> InstallParams {
        InstallParams {
            rev: None,
            extras: self.extras.clone(),
            reinstall: self.reinstall,
        }
    }
}

impl ToolInput for InstallInput {
    fn patch(schema: &mut Value) {
        wait_bounds(schema);
        // One line each (wrapped doc comments would put newlines into the schema).
        redescribe(
            schema,
            &["extras"],
            "Optional extras: render (always installed) and bridge (the LiteLLM proxy \
             start_bridge needs).",
        );
        redescribe(
            schema,
            &["wait_secs"],
            "Seconds to wait before returning (default 0: return the queued job at once; max \
             600). Installs take minutes: follow them with wait_for_job.",
        );
    }
}

job_input!(
    BridgeInput,
    BridgeParams,
    "no_wait",
    "Seconds to wait before returning (default 0: return the queued job at once; max 600). \
     The bridge runs until cancelled, so it never finishes on its own.",
    |schema| {
        describe(
            schema,
            &["profile"],
            "Model profile whose Vertex AI settings and Codex model the bridge uses.",
        );
        describe(
            schema,
            &["port"],
            "TCP port for the bridge (default: harness2-bridge's own).",
        );
        bounds(schema, &["port"], 1, 65535);
    }
);
