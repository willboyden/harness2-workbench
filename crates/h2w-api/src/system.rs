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
    /// Everything needed is in place.
    pub ready: bool,
    /// Names of the parts that are not ready.
    pub failing: Vec<String>,
}

/// `GET /api/v1/system/info`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SystemInfo {
    pub workbench_version: String,
    /// Git commit the server was built from, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_commit: Option<String>,
    pub harness2: Harness2Status,
    /// Isolation new jobs use.
    pub isolation: Isolation,
    /// `isolation.preacknowledge_host_risks`: an administrator accepted the risks of host
    /// isolation for everyone, so jobs need no `acknowledge_risks`. Additive after contract
    /// v1.2.
    #[serde(default)]
    pub host_risks_preacknowledged: bool,
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
    /// `[judge] cloud`: how judge and pipeline jobs with a cloud judge are treated. Additive
    /// after contract v1.9.
    #[serde(default)]
    pub cloud_judges: crate::CloudJudgePolicy,
    /// `[judge] private_hosts`: judge hosts the administrator declared to be on their own
    /// infrastructure (`.example.com` covers a domain), so the web UI classifies judges as
    /// the server does. Additive after contract v1.9.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub private_judge_hosts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Harness2Status {
    /// Installed on the server.
    pub installed: bool,
    /// Installed and kept up to date by this server (vs an existing checkout it points at).
    pub managed: bool,
    /// 40-hex git commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// The commit this Workbench release was tested against.
    pub tested_commit: String,
    /// Version string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Python version of the virtual environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python: Option<String>,
    /// Optional package extras installed.
    #[serde(default)]
    pub extras: Vec<InstallExtra>,
    /// Executors this checkout supports: upstream's `oc` and `cx`, plus `hm` (Hermes Agent)
    /// and `ts` (Turnstone) when it is a harness2 that adds them. A server before contract
    /// v1.10 omits the field: read that as `oc` and `cx`.
    #[serde(default = "crate::Substrate::upstream_set")]
    pub substrates: Vec<crate::Substrate>,
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
    /// Harness label (`base`, `inv1`, `dom1`, `agg1`, `c1`, ...).
    pub label: String,
    /// Severity.
    pub level: CheckLevel,
    /// What was found.
    pub detail: String,
    /// How to fix it.
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
    /// One entry per check.
    pub checks: Vec<DoctorCheck>,
    /// No check is `fail`.
    pub ok: bool,
    /// When this was generated (RFC 3339, UTC).
    pub generated_at: String,
}

/// `GET /api/v1/benchmarks`: one entry per benchmark (wb: one per subset).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BenchmarkStatus {
    pub bench: Bench,
    /// WorkBuddy-Bench subset (wb only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subset: Option<WbSubset>,
    /// Human-readable name.
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
    /// Upstream repository.
    pub upstream_url: String,
    /// Licence summary; WorkBuddy's custom Tencent licence has a territorial limitation.
    pub license_note: String,
    /// What this benchmark grades and how (one paragraph).
    pub description: String,
    /// Set when this server does not set up or run the benchmark: its jobs are refused, and
    /// the Web UI shows this instead of the setup state. Absent = available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<BenchmarkUnavailable>,
}

/// Why a server does not set up or run a benchmark (`BenchmarkStatus.unavailable`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BenchmarkUnavailable {
    pub reason: UnavailableReason,
    /// One sentence for people: the administrator's note, or why the isolation mode rules
    /// the benchmark out.
    pub message: String,
    /// Where the benchmark runs instead (`http` or `https`), when the administrator gave one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// `BenchmarkUnavailable.message` for WorkBuddy-Bench under container isolation, and the
/// refusal of its jobs there.
pub const WB_NEEDS_HOST_ISOLATION: &str = "WorkBuddy-Bench needs host isolation in this version: \
     it starts a Docker container for every trial, which a job inside a container cannot do, so \
     this server (isolation mode \"container\") refuses its jobs. Run it on a Workbench with host \
     isolation.";

/// What makes a benchmark unavailable on a server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum UnavailableReason {
    /// An administrator listed it under `[benchmarks] not_offered` (it runs elsewhere).
    NotOffered,
    /// This server's isolation mode cannot run it: WorkBuddy-Bench starts a Docker
    /// container per trial, which a job inside a container cannot do.
    Isolation,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_level_wire() {
        assert_eq!(serde_json::to_value(CheckLevel::Warn).unwrap(), "warn");
    }
}
