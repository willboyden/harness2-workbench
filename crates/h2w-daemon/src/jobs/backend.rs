//! How a [`JobSpec`] becomes processes and results.
//!
//! The job engine (queue, writer locks, process groups, logs, SSE, cancellation, budget
//! guard, recovery) is generic; everything that depends on harness2's CLI contract — the
//! identifier rules, the exact argv, the output parsers, the spend reader — sits behind
//! [`JobBackend`]. Production uses [`super::core_backend::CoreBackend`] (`h2w-core`); the
//! integration tests drive the same engine with a backend that runs stub executables.

use std::path::{Path, PathBuf};

use h2w_api::{ExperimentKind, FieldProblem, Isolation, JobResult, JobSpec, ModelProfile};
use h2w_core::parse::{BridgeSettings, TaskProgress};
use zeroize::Zeroizing;

use crate::config::RuntimeKind;
use crate::secrets::SecretSnapshot;

/// Container settings resolved at run time (the user is detected when not configured).
#[derive(Debug, Clone, PartialEq)]
pub struct ContainerSettings {
    pub runtime: RuntimeKind,
    pub image: String,
    pub network: String,
    pub cpus: f64,
    pub memory_mb: u64,
    pub pids_limit: u32,
    /// `uid:gid` or a user name; never empty once resolved.
    pub user: String,
    /// `http://host:port` set as the proxy inside the container, when configured.
    pub proxy: Option<String>,
}

/// Everything a backend may use to plan one job.
pub struct PlanInput<'a> {
    pub job_id: &'a str,
    pub spec: &'a JobSpec,
    pub isolation: Isolation,
    /// Host path of the harness2 checkout.
    pub harness2_home: &'a Path,
    pub harness2_managed: bool,
    /// Host path and kind of the experiment directory, for experiment-bound jobs.
    pub experiment: Option<(&'a Path, ExperimentKind)>,
    /// Host path of the job's private 0700 directory (generated files, scratch).
    pub job_dir: &'a Path,
    /// Host path of the split file copied into the job dir (`run`/`preview`/`pipeline` with
    /// `split_file`).
    pub split_file: Option<&'a Path>,
    pub profile: Option<&'a ModelProfile>,
    pub secrets: &'a SecretSnapshot,
    /// Present when `isolation` is `container`.
    pub container: Option<&'a ContainerSettings>,
}

/// How an exit status maps to a job status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitSemantics {
    /// harness2 programs: 0 = succeeded, 2 = partial, anything else = failed.
    Harness2,
    /// Everything else (git, uv, ...): 0 = succeeded, anything else = failed.
    Plain,
}

/// Which parser turns a finished step into a [`JobResult`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseKind {
    None,
    /// The last install step: read what is installed now.
    Install,
    Demo,
    Setup {
        dry_run: bool,
    },
    Check,
    Preview,
    Run,
    Judge {
        bases: bool,
    },
    Report,
    Split,
    Bridge,
}

/// A container a step runs in (for label-verified cancellation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerRef {
    pub runtime: RuntimeKind,
    pub job_id: String,
}

/// One process of a job.
#[derive(Debug, Clone)]
pub struct StepPlan {
    /// `preview`, `check`, `run`, `judge`, `judge-bases`, `report`, `fetch`, `venv`, ...
    pub name: String,
    /// Absolute path, or a bare name resolved on the job's `PATH`.
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// Added after the base passthrough variables. Values may be secret.
    pub env: Vec<(String, String)>,
    /// Pass the container runtime's own variables (`DOCKER_HOST`, `XDG_RUNTIME_DIR`, ...).
    pub runtime_env: bool,
    pub container: Option<ContainerRef>,
    pub exit: ExitSemantics,
    pub parse: ParseKind,
    /// Parse `[domain] i/n status task` progress lines.
    pub progress: bool,
    /// Run the budget guard while this step runs.
    pub budget: bool,
    /// Capture `harness2-bridge` settings (and its key) from stdout.
    pub bridge: bool,
    /// The command as shown in logs and manifests (program name, not path; no secrets).
    pub display: Vec<String>,
}

/// A planned job.
#[derive(Default)]
pub struct JobPlan {
    pub steps: Vec<StepPlan>,
    /// Files written into the job dir (0600) before the first step: (file name, content).
    pub files: Vec<(String, Zeroizing<String>)>,
    /// `(name, value)` of every secret the job can see, for its redactor.
    pub secrets: Vec<(String, Zeroizing<String>)>,
    /// Hosts the job must reach (container egress).
    pub egress_hosts: Vec<String>,
}

impl std::fmt::Debug for JobPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobPlan")
            .field(
                "steps",
                &self.steps.iter().map(|s| &s.name).collect::<Vec<_>>(),
            )
            .field(
                "files",
                &self.files.iter().map(|(n, _)| n).collect::<Vec<_>>(),
            )
            .field(
                "secrets",
                &self.secrets.iter().map(|(n, _)| n).collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

/// What a finished step produced.
pub struct StepOutput<'a> {
    pub spec: &'a JobSpec,
    pub exit_code: Option<i32>,
    /// Captured stdout, secrets redacted (bounded).
    pub stdout: &'a str,
    pub job_dir: &'a Path,
    pub experiment: Option<(&'a Path, ExperimentKind)>,
    pub profile: Option<&'a ModelProfile>,
    pub harness2_home: &'a Path,
    pub harness2_managed: bool,
    /// Measured spend of the experiment's earlier sweeps, for a preview's estimate (the
    /// engine reads it within its read bounds; `None` when there is none).
    pub history: Option<h2w_core::estimate::History>,
}

/// The harness2-specific half of the job engine.
pub trait JobBackend: Send + Sync + 'static {
    /// Identifier rules that need `h2w-core` (names, domains, model ids, commits), as
    /// field problems (`params.experiment`, `params.options.only_domains[0]`, ...).
    fn check_names(&self, spec: &JobSpec) -> Vec<FieldProblem>;

    /// The processes to run, in order.
    ///
    /// # Errors
    /// A user-facing sentence (the job fails with it).
    fn plan(&self, input: &PlanInput<'_>) -> Result<JobPlan, String>;

    /// The result of a finished step.
    ///
    /// # Errors
    /// A user-facing sentence when the output cannot be understood.
    fn parse_step(
        &self,
        step: &StepPlan,
        out: &StepOutput<'_>,
    ) -> Result<Option<JobResult>, String>;

    /// One sweep progress line.
    fn progress_line(&self, line: &str) -> Option<TaskProgress>;

    /// One `harness2-bridge` stdout line; `true` when it carried a setting.
    fn bridge_line(&self, line: &str, acc: &mut BridgeSettings) -> bool;

    /// Spend recorded so far in a sweep's summaries (USD), when readable within `budget`.
    fn spend_usd(
        &self,
        experiment_dir: &Path,
        kind: ExperimentKind,
        summary_ns: &str,
        budget: &mut h2w_core::results::ReadBudget,
    ) -> Option<f64>;
}
