//! The environment a job's harness2 process receives, built from a model profile.
//!
//! Jobs never inherit the server's environment. The server starts from an empty map plus
//! [`BASE_ENV_PASSTHROUGH`] and adds exactly what [`build`] returns. Secret values travel
//! only in the child's environment (never argv), and every one of them is also returned
//! for the job's [`crate::redact::Redactor`].

use h2s_api::ModelProfile;

use crate::{CoreError, Result};

/// Variables copied from the server's own environment when present.
pub const BASE_ENV_PASSTHROUGH: &[&str] = &[
    "PATH", "HOME", "LANG", "LC_ALL", "TZ", "TMPDIR", "USER", "LOGNAME",
];

/// Names a profile's `extra_env` may never set (they control the loader, the interpreter,
/// the server or credentials the server manages itself).
pub const RESERVED_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "SHELL",
    "IFS",
    "ENV",
    "BASH_ENV",
    "NODE_OPTIONS",
    "OPENCODE_CONFIG",
    "OPENCODE_CONFIG_CONTENT",
    "OPENCODE_PERMISSION",
    "HARNESS2_RUNS_DIR",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "DOCKER_HOST",
    "SSH_AUTH_SOCK",
];

/// Prefixes a profile's `extra_env` may never set.
pub const RESERVED_ENV_PREFIXES: &[&str] = &[
    "LD_",
    "DYLD_",
    "PYTHON",
    "H2S_",
    "UV_",
    "BUN_",
    "NPM_CONFIG_",
];

/// OpenCode provider id the server registers for `openai_compatible` profiles; model ids
/// become `h2s/<model>`.
pub const OPENCODE_PROVIDER_ID: &str = "h2s";

/// Environment variable carrying the OpenAI-compatible endpoint key to OpenCode (referenced
/// from the generated config as `{env:H2S_PROVIDER_API_KEY}`).
pub const PROVIDER_KEY_ENV: &str = "H2S_PROVIDER_API_KEY";

/// Looks up secret values by name.
pub trait SecretLookup {
    fn secret(&self, name: &str) -> Option<String>;
}

/// Paths (as the harness2 process will see them) the server prepares for the job.
#[derive(Debug, Clone)]
pub struct EnvContext {
    /// Experiment directory -> `HARNESS2_RUNS_DIR`.
    pub runs_dir: String,
    /// Where the generated OpenCode config will be written (for `OPENCODE_CONFIG`).
    pub opencode_config_path: String,
    /// Where a service-account key will be written (for `GOOGLE_APPLICATION_CREDENTIALS`).
    pub gcp_credentials_path: String,
}

/// Everything a job needs from its profile. `Debug` never prints values.
#[derive(Clone, Default)]
pub struct JobEnv {
    /// Variables to set in the child (values may be secret).
    pub vars: Vec<(String, String)>,
    /// `(secret name, value)` for the job's redactor.
    pub secrets: Vec<(String, String)>,
    /// JSON of the OpenCode config to write at `opencode_config_path` (0600), if any.
    pub opencode_config: Option<String>,
    /// Service-account JSON to write at `gcp_credentials_path` (0600), if any.
    pub gcp_credentials_json: Option<String>,
    /// Hosts the job must reach (for the container egress allowlist).
    pub egress_hosts: Vec<String>,
}

impl std::fmt::Debug for JobEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.vars.iter().map(|(n, _)| n.as_str()).collect();
        f.debug_struct("JobEnv")
            .field("vars", &names)
            .field("secrets", &self.secrets.len())
            .field("opencode_config", &self.opencode_config.is_some())
            .field("gcp_credentials_json", &self.gcp_credentials_json.is_some())
            .field("egress_hosts", &self.egress_hosts)
            .finish()
    }
}

/// Validate a profile (names, model ids, URLs, env names, secret names) without resolving
/// secrets. Collects every problem.
pub fn validate_profile(profile: &ModelProfile) -> Result<()> {
    let _ = profile;
    Err(CoreError::Unimplemented("env::validate_profile"))
}

/// Build the job environment for `profile`. Fails with the NAME of any referenced secret
/// that is not set.
///
/// - `vertex`: `GOOGLE_CLOUD_PROJECT`, `GOOGLE_CLOUD_LOCATION`, `HARNESS2_SOLVER_MODEL`,
///   `HARNESS2_IMPROVER_MODEL`, `HARNESS2_JUDGE_MODEL`, optional key file.
/// - `openai_compatible`: an OpenCode config registering provider [`OPENCODE_PROVIDER_ID`]
///   (`@ai-sdk/openai-compatible`, `baseURL` = endpoint, `apiKey` = `{env:...}`),
///   `OPENCODE_CONFIG`, models as `h2s/<id>`, and the judge / Codex endpoint variables.
/// - `custom`: `extra_env` only.
///
/// `HARNESS2_RUNS_DIR` is always set from the context.
pub fn build(
    profile: &ModelProfile,
    secrets: &dyn SecretLookup,
    ctx: &EnvContext,
) -> Result<JobEnv> {
    let _ = (profile, secrets, ctx);
    Err(CoreError::Unimplemented("env::build"))
}
