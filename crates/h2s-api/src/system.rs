//! Server status, environment checks and benchmark installation state.

use serde::{Deserialize, Serialize};

use crate::{AuthMode, Bench, DomainCount, InstallExtra, Isolation, WbSubset};

/// `GET /healthz` (unauthenticated, minimal on purpose).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Health {
    /// Always `ok` when the process answers.
    pub status: String,
}

/// `GET /readyz` (unauthenticated): 200 when the database opens and the job runner runs,
/// 503 otherwise. Names the failing parts, never paths or versions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Readiness {
    pub ready: bool,
    pub failing: Vec<String>,
}

/// `GET /api/v1/system/info`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SystemInfo {
    pub studio_version: String,
    /// Git commit the server was built from, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_commit: Option<String>,
    pub harness2: Harness2Status,
    /// Isolation new jobs use.
    pub isolation: Isolation,
    /// `docker` or `podman` when container isolation is configured and the runtime answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_runtime: Option<String>,
    pub auth_mode: AuthMode,
    pub server_time: String,
    /// Jobs currently running / allowed at once.
    pub jobs_running: u32,
    pub jobs_max_concurrent: u32,
    /// Admin only: where the server keeps its database, logs and experiments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<String>,
    /// Feature flags compiled in or enabled (`otel`, `container`, `proxy_auth`, ...).
    pub features: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Harness2Status {
    pub installed: bool,
    /// Installed and kept up to date by this server (vs an existing checkout it points at).
    pub managed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// The commit this Studio release was tested against.
    pub tested_commit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python: Option<String>,
    #[serde(default)]
    pub extras: Vec<InstallExtra>,
    /// Admin only: the checkout directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum CheckLevel {
    Ok,
    /// Works, but something deserves attention (old version, optional tool missing).
    Warn,
    /// Something the user asked for cannot work until fixed.
    Fail,
    /// Not applicable here (e.g. Docker checks when no benchmark needs Docker).
    Skip,
}

/// One environment check with a concrete remedy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DoctorCheck {
    /// Stable id, e.g. `uv`, `python312`, `node20`, `git`, `docker`, `pandoc`, `pdftoppm`,
    /// `gcloud`, `adc`, `harness2`, `container_runtime`, `runner_image`, `disk`.
    pub id: String,
    pub label: String,
    pub level: CheckLevel,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remedy: Option<String>,
    /// Which features need this (`demo`, `lab`, `jb`, `wb`, `codex`, `vertex`, ...).
    pub needed_for: Vec<String>,
}

/// `GET /api/v1/system/doctor`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DoctorReport {
    pub checks: Vec<DoctorCheck>,
    /// No check is `fail`.
    pub ok: bool,
    pub generated_at: String,
}

/// `GET /api/v1/benchmarks`: one entry per benchmark (wb: one per subset).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BenchmarkStatus {
    pub bench: Bench,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subset: Option<WbSubset>,
    pub display_name: String,
    /// The upstream checkout exists under harness2's `dependencies/`.
    pub installed: bool,
    /// The checkout is at harness2's pinned commit (`None` when not installed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_pinned_commit: Option<bool>,
    /// Task data present (JobBench dataset, WorkBuddy subset archive).
    pub data_present: bool,
    /// Domains and task counts from the most recent preview, when one ran.
    #[serde(default)]
    pub domains: Vec<DomainCount>,
    /// Host tools this benchmark needs beyond harness2 itself.
    pub requirements: Vec<String>,
    pub upstream_url: String,
    /// Licence summary; WorkBuddy's custom Tencent licence has a territorial limitation.
    pub license_note: String,
    /// What this benchmark grades and how (one paragraph).
    pub description: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_level_wire() {
        assert_eq!(serde_json::to_value(CheckLevel::Warn).unwrap(), "warn");
    }
}
