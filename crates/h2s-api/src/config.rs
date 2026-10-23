//! Model profiles, secrets, tokens, authentication and the audit log.

use serde::{Deserialize, Serialize};

use crate::{Bench, Role, WbSubset};

/// How browsers and API clients authenticate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    /// API tokens; the web UI exchanges one for a session cookie.
    Token,
    /// A trusted reverse proxy (SSO) asserts the user and role in headers; tokens still
    /// work for API clients.
    Proxy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ProfileKind {
    /// Google Vertex AI with Application Default Credentials (upstream's configuration).
    Vertex,
    /// Any OpenAI-compatible endpoint (LiteLLM, vLLM, SGLang, Ollama, a hosted gateway),
    /// wired into OpenCode as a custom provider and into the judges' base-URL settings.
    OpenaiCompatible,
    /// Raw harness2 environment variables, for setups the other kinds do not cover.
    Custom,
}

/// An OpenAI-compatible endpoint. The key is referenced by secret NAME, never inlined.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Endpoint {
    /// e.g. `http://127.0.0.1:4000/v1`.
    pub base_url: String,
    /// Name of the secret holding the API key, if the endpoint needs one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_secret: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct VertexSettings {
    /// `GOOGLE_CLOUD_PROJECT`.
    pub project: String,
    /// `GOOGLE_CLOUD_LOCATION` (upstream default `global`).
    #[serde(default = "default_location")]
    pub location: String,
    /// Secret holding a service-account key file's JSON, written to a 0600 temp file for
    /// the job and passed as `GOOGLE_APPLICATION_CREDENTIALS`. `None` = use the server
    /// user's Application Default Credentials (`gcloud auth application-default login`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials_secret: Option<String>,
}

fn default_location() -> String {
    "global".to_owned()
}

/// One extra environment variable for the job: a literal value or a secret reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct EnvVar {
    /// `^[A-Z_][A-Z0-9_]{0,127}$`; names that control the server or the child's loader
    /// (`LD_*`, `PYTHON*`, `PATH`, ...) are refused.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
}

/// Price assumptions for estimates (USD per executor rollout / improver session).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Prices {
    pub rollout_usd_low: f64,
    pub rollout_usd_high: f64,
    pub session_usd_low: f64,
    pub session_usd_high: f64,
}

/// Which models harness2 uses, and how to reach them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ModelProfile {
    /// `^[a-z0-9][a-z0-9_-]{0,62}$`.
    pub name: String,
    pub kind: ProfileKind,
    #[serde(default)]
    pub description: String,
    /// `HARNESS2_SOLVER_MODEL` in OpenCode `provider/model` form (for `openai_compatible`
    /// profiles, the bare model id; the server adds its provider prefix).
    pub solver_model: String,
    /// `HARNESS2_IMPROVER_MODEL`; `None` = same as the solver.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub improver_model: Option<String>,
    /// `HARNESS2_JUDGE_MODEL` (bare id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judge_model: Option<String>,
    /// `HARNESS2_CODEX_MODEL`, for the Codex executor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vertex: Option<VertexSettings>,
    /// Executor/improver endpoint (`openai_compatible`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<Endpoint>,
    /// `HARNESS2_JUDGE_BASE_URL` / `HARNESS2_JUDGE_API_KEY`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judge_endpoint: Option<Endpoint>,
    /// `HARNESS2_CODEX_BASE_URL` / `HARNESS2_CODEX_API_KEY` (a Responses-API endpoint).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_endpoint: Option<Endpoint>,
    #[serde(default)]
    pub extra_env: Vec<EnvVar>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prices: Option<Prices>,
    /// Hosts the jobs must reach (added to the container egress allowlist). Derived from
    /// the endpoints when empty.
    #[serde(default)]
    pub egress_hosts: Vec<String>,
    /// Secrets this profile references and whether each is set (response only).
    #[serde(default)]
    pub secrets: Vec<SecretInfo>,
}

/// A secret's metadata. Values are write-only: no endpoint ever returns them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SecretInfo {
    /// `^[A-Z][A-Z0-9_]{0,63}$`.
    pub name: String,
    pub set: bool,
    /// Length of the stored value, so a truncated paste is noticeable without exposure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chars: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

/// Body of `PUT /api/v1/secrets/{name}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SetSecretRequest {
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TokenInfo {
    /// Public token id (the `<id>` in `h2s_<id>_<secret>`).
    pub id: String,
    pub name: String,
    pub role: Role,
    pub created_at: String,
    pub created_by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    pub revoked: bool,
}

/// Body of `POST /api/v1/tokens`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CreateTokenRequest {
    pub name: String,
    pub role: Role,
    /// `None` = never expires (discouraged; the UI defaults to 90 days).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in_days: Option<u32>,
}

/// Response of `POST /api/v1/tokens`. `token` is shown exactly once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TokenCreated {
    pub token: String,
    pub info: TokenInfo,
}

/// Body of `POST /api/v1/auth/login`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LoginRequest {
    pub token: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    Success,
    /// Refused by authentication, authorization or validation.
    Denied,
    Error,
}

/// One tamper-evident audit record: `hash = sha256(prev_hash || canonical JSON of the
/// record without `hash`)`. `GET /api/v1/audit/verify` re-walks the chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AuditEvent {
    pub seq: u64,
    pub ts: String,
    /// Principal display name, or `anonymous`.
    pub actor: String,
    /// e.g. `job.create`, `job.cancel`, `token.create`, `secret.set`, `auth.login`.
    pub action: String,
    /// What was acted on (job id, token id, secret NAME, experiment), never a value.
    pub target: String,
    pub outcome: AuditOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub prev_hash: String,
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AuditVerification {
    pub ok: bool,
    pub entries: u64,
    /// First record whose hash does not match, when `ok` is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_bad_seq: Option<u64>,
}

/// A train/test partition file created by a `split` job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SplitInfo {
    pub name: String,
    pub bench: Bench,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subset: Option<WbSubset>,
    pub seed: u64,
    pub train_fraction: f64,
    pub train: u32,
    pub test: u32,
    pub created_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_minimal_json() {
        let p: ModelProfile = serde_json::from_str(
            r#"{"name":"local","kind":"openai_compatible","solver_model":"glm-5"}"#,
        )
        .unwrap();
        assert_eq!(p.kind, ProfileKind::OpenaiCompatible);
        assert!(p.extra_env.is_empty());
    }

    #[test]
    fn vertex_location_defaults_global() {
        let v: VertexSettings = serde_json::from_str(r#"{"project":"p"}"#).unwrap();
        assert_eq!(v.location, "global");
    }
}
