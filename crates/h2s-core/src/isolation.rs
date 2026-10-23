//! Container isolation: the exact `docker`/`podman run` argv for a job.
//!
//! Every runtime-level flag is a fixed literal chosen here; request data only ever appears
//! after the image name (the program and its already-validated arguments) or as validated
//! mount sources from server configuration.

use crate::{CoreError, Result};

/// Where the experiment directory is mounted inside the runner container.
pub const EXPERIMENT_MOUNT: &str = "/h2s/experiment";
/// Where the harness2 checkout (with its venv and `dependencies/`) is mounted.
pub const HARNESS2_MOUNT: &str = "/h2s/harness2";
/// Where per-job generated files (OpenCode config, key file) are mounted read-only.
pub const JOBFILES_MOUNT: &str = "/h2s/job";
/// Label put on every container the server starts; checked before kill/logs/rm.
pub const JOB_LABEL: &str = "io.github.harness2-studio.job";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runtime {
    Docker,
    Podman,
}

impl Runtime {
    #[must_use]
    pub fn program(self) -> &'static str {
        match self {
            Runtime::Docker => "docker",
            Runtime::Podman => "podman",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    /// Absolute host path from server configuration (validated: absolute, no `:`, `,` or
    /// control characters, not a symlink — the last checked by the server at run time).
    pub host: String,
    /// One of the fixed mount points above.
    pub container: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContainerSpec {
    pub runtime: Runtime,
    /// Pinned image reference (`name:tag` or `name@sha256:...`), from configuration.
    pub image: String,
    /// Job id; becomes the container name `h2s-job-<id>` and the label value.
    pub job_id: String,
    pub mounts: Vec<Mount>,
    /// Variable NAMES passed through with `-e NAME` (values come from the runtime's env).
    pub env_names: Vec<String>,
    /// `none`, or the name of a pre-created network (e.g. one behind an egress proxy).
    pub network: String,
    pub cpus: Option<f64>,
    pub memory_mb: Option<u64>,
    pub pids_limit: u32,
    /// Working directory inside the container.
    pub workdir: String,
    /// `uid:gid` to run as (default: the image's non-root user).
    pub user: Option<String>,
}

/// `<runtime> run --rm --name h2s-job-<id> --label <JOB_LABEL>=<id> --cap-drop ALL
/// --security-opt no-new-privileges --pids-limit N [--cpus] [--memory] --network NET
/// [-v ...] [-e NAME ...] -w WORKDIR [--user U] <image> <program> <args...>`.
pub fn run_argv(spec: &ContainerSpec, program: &str, args: &[String]) -> Result<Vec<String>> {
    let _ = (spec, program, args);
    Err(CoreError::Unimplemented("isolation::run_argv"))
}

/// argv that prints the job label of container `h2s-job-<id>` (to verify before killing).
#[must_use]
pub fn inspect_label_argv(runtime: Runtime, job_id: &str) -> Vec<String> {
    let _ = (runtime, job_id);
    Vec::new()
}

/// argv that stops container `h2s-job-<id>` (call only after the label matched).
#[must_use]
pub fn kill_argv(runtime: Runtime, job_id: &str) -> Vec<String> {
    let _ = (runtime, job_id);
    Vec::new()
}
