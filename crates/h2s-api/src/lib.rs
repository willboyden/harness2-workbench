//! Wire types shared by every Harness² Studio component.
//!
//! **This crate is the LOCKED contract** between the server (`h2s-daemon`), the web UI
//! (`h2s-web`) and the MCP server (`h2s-mcp`). Changes after the initial fan-out are
//! additive only and must be recorded in `docs/ARCHITECTURE.md` §9 (contract addenda).
//!
//! Conventions (see `docs/ARCHITECTURE.md` §4):
//! - JSON field names are `snake_case`; enums serialize as `snake_case` strings.
//! - Timestamps are RFC 3339 strings in UTC (`2026-10-23T12:00:00Z`).
//! - Identifiers are opaque strings; the server validates every identifier it receives.
//! - Money is US dollars as `f64`, as harness2 records it.
//! - Absolute host paths appear only in admin-only fields, never in viewer responses.
//!
//! This crate compiles for `wasm32-unknown-unknown` (the web UI) and natively. It holds
//! data and pure helpers only: no I/O, no async, no platform code.

#![forbid(unsafe_code)]

pub mod config;
pub mod harness2;
pub mod jobs;
pub mod results;
pub mod system;

pub use config::*;
pub use harness2::*;
pub use jobs::*;
pub use results::*;
pub use system::*;

use serde::{Deserialize, Serialize};

/// Every API route lives under this prefix.
pub const API_PREFIX: &str = "/api/v1";

/// Header every state-changing request (any method except GET/HEAD/OPTIONS) must carry with
/// the value `1`. Browsers cannot attach a custom header cross-site without a CORS preflight,
/// which the server never grants, so this blocks cross-site request forgery.
pub const CSRF_HEADER: &str = "x-h2s-request";

/// Name of the HttpOnly session cookie set by `POST /api/v1/auth/login`.
pub const SESSION_COOKIE: &str = "h2s_session";

/// Every API token starts with this prefix: `h2s_<id>_<secret>`.
pub const TOKEN_PREFIX: &str = "h2s_";

/// Response header carrying the per-request id (also present in error bodies and logs).
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// Stable machine-readable error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Malformed request (bad JSON, wrong types).
    BadRequest,
    /// Well-formed but semantically invalid input; `details` lists the problems.
    Validation,
    /// No or invalid credentials.
    Unauthorized,
    /// Authenticated, but the role does not allow this.
    Forbidden,
    NotFound,
    /// The request conflicts with current state (e.g. experiment exists, job finished).
    Conflict,
    PayloadTooLarge,
    RateLimited,
    /// The `Host` header is not one this server answers to (DNS-rebinding defence).
    MisdirectedRequest,
    /// A dependency (harness2, the container runtime, the database) is not available.
    Unavailable,
    Internal,
}

/// Body of every non-2xx JSON response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ErrorBody {
    /// The error.
    pub error: ApiError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ApiError {
    /// Machine-readable error code.
    pub code: ErrorCode,
    /// Human-readable, safe to show to the user. Never contains secret values.
    pub message: String,
    /// For `validation`: one entry per problem, `{"field": "...", "problem": "..."}`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<FieldProblem>,
    /// Request id, also in the server's logs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FieldProblem {
    /// Field the problem is about.
    pub field: String,
    /// What is wrong, as a sentence.
    pub problem: String,
}

/// A page of a list. Pass `next_cursor` back as `cursor` to get the next page; `None`
/// means this is the last page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Page<T> {
    /// The entries on this page.
    pub items: Vec<T>,
    /// Pass back as `cursor` for the next page; absent on the last page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// A named string value (dependency pin, model id, ...).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct KeyValue {
    /// Key.
    pub key: String,
    /// Value.
    pub value: String,
}

/// A text file addressed by a relative, `/`-separated path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FileText {
    /// Relative, `/`-separated path (no `..`, no leading `/`).
    pub path: String,
    /// The file's text (UTF-8).
    pub content: String,
}

/// Authorization role. Ordered: `Viewer < Operator < Admin`; a role allows everything the
/// roles below it allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Read everything except secrets metadata, tokens and the audit log.
    Viewer,
    /// Viewer + create experiments, start/cancel jobs that use already-installed tooling.
    Operator,
    /// Operator + install harness2, set up benchmarks, run the bridge, manage profiles,
    /// secrets, tokens; read the audit log.
    Admin,
}

impl Role {
    /// True when a caller holding `self` may perform an action that requires `required`.
    #[must_use]
    pub fn allows(self, required: Role) -> bool {
        self >= required
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Operator => "operator",
            Role::Admin => "admin",
        }
    }
}

/// How the caller authenticated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum PrincipalKind {
    /// `Authorization: Bearer h2s_...`
    Token,
    /// Browser session cookie created by logging in with a token.
    Session,
    /// Identity asserted by a trusted reverse proxy (SSO deployments).
    Proxy,
}

/// The authenticated caller, as returned by `GET /api/v1/auth/me`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Principal {
    /// Token id, session's token id, or the proxy user name.
    pub id: String,
    /// Display name (token name or proxy user).
    pub name: String,
    /// Role of the caller.
    pub role: Role,
    /// How the caller authenticated.
    pub kind: PrincipalKind,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_are_ordered() {
        assert!(Role::Admin.allows(Role::Viewer));
        assert!(Role::Admin.allows(Role::Operator));
        assert!(Role::Operator.allows(Role::Viewer));
        assert!(!Role::Viewer.allows(Role::Operator));
        assert!(!Role::Operator.allows(Role::Admin));
    }

    #[test]
    fn error_body_shape_is_stable() {
        let body = ErrorBody {
            error: ApiError {
                code: ErrorCode::Validation,
                message: "bad k".into(),
                details: vec![FieldProblem {
                    field: "k".into(),
                    problem: "must be >= 1".into(),
                }],
                request_id: Some("r1".into()),
            },
        };
        let v = serde_json::to_value(&body).unwrap();
        assert_eq!(v["error"]["code"], "validation");
        assert_eq!(v["error"]["details"][0]["field"], "k");
    }
}
