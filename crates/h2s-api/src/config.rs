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
    /// Literal value (never a secret; use `secret`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Name of a stored secret whose value becomes the variable's value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
}

/// Price assumptions for estimates (USD per executor rollout / improver session).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Prices {
    /// Low estimate of USD per executor rollout.
    pub rollout_usd_low: f64,
    /// High estimate of USD per executor rollout.
    pub rollout_usd_high: f64,
    /// Low estimate of USD per improver session.
    pub session_usd_low: f64,
    /// High estimate of USD per improver session.
    pub session_usd_high: f64,
}

/// Which models harness2 uses, and how to reach them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ModelProfile {
    /// `^[a-z0-9][a-z0-9_-]{0,62}$`.
    pub name: String,
    /// How the models are reached.
    pub kind: ProfileKind,
    /// Free-text description.
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
    /// Vertex AI project settings (`vertex` profiles).
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
    /// Further harness2 environment variables (literals or secret references).
    #[serde(default)]
    pub extra_env: Vec<EnvVar>,
    /// Price assumptions for cost estimates.
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
    /// A value is stored.
    pub set: bool,
    /// Length of the stored value, so a truncated paste is noticeable without exposure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chars: Option<u32>,
    /// When the value was last set (RFC 3339, UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

/// `GET /api/v1/profiles/{name}/check`: what jobs with this profile would lack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ProfileCheck {
    /// What the profile cannot do (no judge model, a judge LAB does not recognise, WorkBuddy
    /// without Vertex Gemini, ...). Jobs still start; harness2 may then refuse or skip work.
    pub warnings: Vec<String>,
    /// Referenced secrets with no value (neither stored nor `H2S_SECRET_<NAME>`). Jobs with
    /// this profile are refused (422) until they are set.
    pub missing_secrets: Vec<String>,
}

/// Body of `PUT /api/v1/secrets/{name}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SetSecretRequest {
    /// The secret value. Write-only: never returned by any endpoint.
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TokenInfo {
    /// Public token id (the `<id>` in `h2s_<id>_<secret>`).
    pub id: String,
    /// Display name given at creation.
    pub name: String,
    /// Role the token grants.
    pub role: Role,
    /// Creation time (RFC 3339, UTC).
    pub created_at: String,
    /// Display name of the principal that created it.
    pub created_by: String,
    /// Last successful authentication with it (RFC 3339, UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<String>,
    /// Expiry (RFC 3339, UTC); absent = never.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    /// The token no longer authenticates.
    pub revoked: bool,
}

/// Body of `POST /api/v1/tokens`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CreateTokenRequest {
    /// Display name (letters, digits, space, `._-@`, up to 64).
    pub name: String,
    /// Role the token grants.
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
    /// The new token. Shown exactly once; store it like a password.
    pub token: String,
    /// The token's metadata.
    pub info: TokenInfo,
}

/// Body of `POST /api/v1/auth/login`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LoginRequest {
    /// An API token; exchanged for an HttpOnly session cookie.
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
    /// Position in the audit chain (from 1).
    pub seq: u64,
    /// Time of the event (RFC 3339, UTC).
    pub ts: String,
    /// Principal display name, or `anonymous`.
    pub actor: String,
    /// e.g. `job.create`, `job.cancel`, `token.create`, `secret.set`, `auth.login`.
    pub action: String,
    /// What was acted on (job id, token id, secret NAME, experiment), never a value.
    pub target: String,
    /// Whether the action succeeded, was denied or failed.
    pub outcome: AuditOutcome,
    /// Client address as the server saw it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    /// Request id, also in the server's logs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// What was found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Hash of the previous record (hex).
    pub prev_hash: String,
    /// SHA-256 of this record chained to the previous one (hex).
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AuditVerification {
    /// Every record's hash matches its content and its predecessor.
    pub ok: bool,
    /// Records checked.
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
    /// Split file name (the value for `RunOptions.split_file`).
    pub name: String,
    pub bench: Bench,
    /// WorkBuddy-Bench subset (wb only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subset: Option<WbSubset>,
    /// Seed of the stratified shuffle.
    pub seed: u64,
    /// Fraction of each domain's tasks in the train partition.
    pub train_fraction: f64,
    /// Tasks in the train partition.
    pub train: u32,
    /// Tasks in the test partition.
    pub test: u32,
    /// Creation time (RFC 3339, UTC).
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
