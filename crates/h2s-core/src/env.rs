//! The environment a job's harness2 process receives, built from a model profile.
//!
//! Jobs never inherit the server's environment. The server starts from an empty map plus
//! [`BASE_ENV_PASSTHROUGH`] and adds exactly what [`build`] returns. Secret values travel
//! only in the child's environment (never argv), and every one of them is also returned
//! for the job's [`crate::redact::Redactor`].
//!
//! Upstream facts this relies on (harness2 at [`crate::TESTED_HARNESS2_COMMIT`], OpenCode
//! 1.14.18): OpenCode merges the file named by `OPENCODE_CONFIG` and bundles
//! `@ai-sdk/openai-compatible`, so the generated provider works offline; every OpenCode
//! process harness2 starts (executor runners, improver sessions) inherits the environment;
//! JobBench's judge reads `HARNESS2_JUDGE_BASE_URL`/`HARNESS2_JUDGE_API_KEY`; LAB's judge
//! sends `gpt*`/`o1`/`o3`/`o4`/`o5` model ids to the official OpenAI client, which reads
//! `OPENAI_BASE_URL`/`OPENAI_API_KEY`. WorkBuddy's setup requires Vertex Gemini models, so
//! `openai_compatible` profiles cannot drive WorkBuddy (see `docs/notes/core.md`).

use h2s_api::{Endpoint, ModelProfile, ProfileKind};

use crate::{CoreError, Result, names};

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

/// A planned variable value: a literal, or the NAME of a secret resolved at build time.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Val {
    Lit(String),
    Secret(String),
}

/// What [`build`] will produce, before secrets are resolved.
#[derive(Debug, Default)]
struct Plan {
    vars: Vec<(String, Val)>,
    opencode_config: Option<String>,
    gcp_credentials_secret: Option<String>,
    egress_hosts: Vec<String>,
}

impl Plan {
    fn lit(&mut self, name: &str, value: impl Into<String>) {
        self.vars.push((name.to_owned(), Val::Lit(value.into())));
    }

    fn secret(&mut self, name: &str, secret: &str) {
        self.vars
            .push((name.to_owned(), Val::Secret(secret.to_owned())));
    }

    fn host_of(&mut self, url: &str) {
        if let Some((host, _)) = names::url_host(url) {
            self.egress_hosts.push(host);
        }
    }
}

/// LAB's judge hands these model ids to the official OpenAI client.
fn is_openai_judge(model: &str) -> bool {
    model.starts_with("gpt")
        || ["o1", "o3", "o4", "o5"]
            .iter()
            .any(|p| model.starts_with(p))
}

/// Model-id prefixes LAB's judge (`evaluation/judge.py`) recognises.
const LAB_JUDGE_PREFIXES: &[&str] = &[
    "claude", "gemini", "gpt", "o1", "o3", "o4", "o5", "mistral", "vertex/",
];

#[allow(dead_code)] // Used by tests; proposed as a public helper for the server.
/// Whether LAB's judge recognises this model id (the server warns when it does not; the
/// profile stays valid because JobBench and WorkBuddy judge differently).
#[must_use]
pub(crate) fn lab_judge_supported(model: &str) -> bool {
    LAB_JUDGE_PREFIXES.iter().any(|p| model.starts_with(p))
}

fn vertex_vars(plan: &mut Plan, profile: &ModelProfile, ctx: &EnvContext) {
    if let Some(v) = &profile.vertex {
        plan.lit("GOOGLE_CLOUD_PROJECT", v.project.clone());
        plan.lit("GOOGLE_CLOUD_LOCATION", v.location.clone());
        if let Some(secret) = &v.credentials_secret {
            plan.lit(
                "GOOGLE_APPLICATION_CREDENTIALS",
                ctx.gcp_credentials_path.clone(),
            );
            plan.gcp_credentials_secret = Some(secret.clone());
        }
        plan.egress_hosts.push(if v.location == "global" {
            "aiplatform.googleapis.com".to_owned()
        } else {
            format!("{}-aiplatform.googleapis.com", v.location)
        });
        plan.egress_hosts.push("oauth2.googleapis.com".to_owned());
    }
}

fn judge_vars(plan: &mut Plan, profile: &ModelProfile, judge_ep: Option<&Endpoint>) {
    if let Some(ep) = judge_ep {
        plan.lit("HARNESS2_JUDGE_BASE_URL", ep.base_url.clone());
        if let Some(s) = &ep.api_key_secret {
            plan.secret("HARNESS2_JUDGE_API_KEY", s);
        }
        plan.host_of(&ep.base_url);
        if profile.judge_model.as_deref().is_some_and(is_openai_judge) {
            plan.lit("OPENAI_BASE_URL", ep.base_url.clone());
            if let Some(s) = &ep.api_key_secret {
                plan.secret("OPENAI_API_KEY", s);
            }
        }
    }
}

fn codex_vars(plan: &mut Plan, profile: &ModelProfile, default_model: &str) {
    if let Some(ep) = &profile.codex_endpoint {
        plan.lit("HARNESS2_CODEX_BASE_URL", ep.base_url.clone());
        if let Some(s) = &ep.api_key_secret {
            plan.secret("HARNESS2_CODEX_API_KEY", s);
        }
        plan.lit(
            "HARNESS2_CODEX_MODEL",
            profile
                .codex_model
                .clone()
                .unwrap_or_else(|| default_model.to_owned()),
        );
        plan.host_of(&ep.base_url);
    } else if let Some(m) = &profile.codex_model {
        plan.lit("HARNESS2_CODEX_MODEL", m.clone());
    }
}

/// The OpenCode config registering the profile's endpoint as provider `h2s`.
fn opencode_config(profile: &ModelProfile, ep: &Endpoint) -> String {
    let mut models = serde_json::Map::new();
    let improver = profile
        .improver_model
        .as_deref()
        .unwrap_or(&profile.solver_model);
    for id in [profile.solver_model.as_str(), improver] {
        models.insert(id.to_owned(), serde_json::json!({ "name": id }));
    }
    let mut options = serde_json::Map::new();
    options.insert("baseURL".into(), ep.base_url.clone().into());
    if ep.api_key_secret.is_some() {
        options.insert(
            "apiKey".into(),
            format!("{{env:{PROVIDER_KEY_ENV}}}").into(),
        );
    }
    let config = serde_json::json!({
        "$schema": "https://opencode.ai/config.json",
        "provider": {
            OPENCODE_PROVIDER_ID: {
                "npm": "@ai-sdk/openai-compatible",
                "name": "Harness² Studio",
                "options": options,
                "models": models,
            }
        }
    });
    serde_json::to_string_pretty(&config).unwrap_or_default()
}

fn plan(profile: &ModelProfile, ctx: &EnvContext) -> Plan {
    let mut plan = Plan::default();
    plan.lit("HARNESS2_RUNS_DIR", ctx.runs_dir.clone());
    let improver = profile
        .improver_model
        .clone()
        .unwrap_or_else(|| profile.solver_model.clone());
    match profile.kind {
        ProfileKind::Vertex => {
            vertex_vars(&mut plan, profile, ctx);
            plan.lit("HARNESS2_SOLVER_MODEL", profile.solver_model.clone());
            plan.lit("HARNESS2_IMPROVER_MODEL", improver);
            if let Some(j) = &profile.judge_model {
                plan.lit("HARNESS2_JUDGE_MODEL", j.clone());
            }
            judge_vars(&mut plan, profile, profile.judge_endpoint.as_ref());
            let bare = profile
                .solver_model
                .split_once('/')
                .map_or(profile.solver_model.as_str(), |(_, m)| m);
            codex_vars(&mut plan, profile, bare);
        }
        ProfileKind::OpenaiCompatible => {
            if let Some(ep) = &profile.endpoint {
                plan.opencode_config = Some(opencode_config(profile, ep));
                plan.lit("OPENCODE_CONFIG", ctx.opencode_config_path.clone());
                if let Some(s) = &ep.api_key_secret {
                    plan.secret(PROVIDER_KEY_ENV, s);
                }
                plan.host_of(&ep.base_url);
            }
            vertex_vars(&mut plan, profile, ctx);
            plan.lit(
                "HARNESS2_SOLVER_MODEL",
                format!("{OPENCODE_PROVIDER_ID}/{}", profile.solver_model),
            );
            plan.lit(
                "HARNESS2_IMPROVER_MODEL",
                format!("{OPENCODE_PROVIDER_ID}/{improver}"),
            );
            if let Some(j) = &profile.judge_model {
                plan.lit("HARNESS2_JUDGE_MODEL", j.clone());
            }
            // The judge defaults to the profile's endpoint when it has a judge model.
            let judge_ep = profile
                .judge_endpoint
                .as_ref()
                .or(profile.judge_model.as_ref().and(profile.endpoint.as_ref()));
            judge_vars(&mut plan, profile, judge_ep);
            codex_vars(&mut plan, profile, &profile.solver_model);
        }
        ProfileKind::Custom => {}
    }
    for e in &profile.extra_env {
        match (&e.value, &e.secret) {
            (Some(v), None) => plan.lit(&e.name, v.clone()),
            (None, Some(s)) => plan.secret(&e.name, s),
            _ => {}
        }
    }
    if profile.egress_hosts.is_empty() {
        plan.egress_hosts.sort();
        plan.egress_hosts.dedup();
    } else {
        let mut hosts = profile.egress_hosts.clone();
        hosts.sort();
        hosts.dedup();
        plan.egress_hosts = hosts;
    }
    plan
}

fn push(errs: &mut Vec<(String, String)>, field: &str, r: Result<()>) {
    if let Err(e) = r {
        let problem = match e {
            CoreError::Invalid { problem, .. } => problem,
            other => other.to_string(),
        };
        errs.push((field.to_owned(), problem));
    }
}

fn check_endpoint(errs: &mut Vec<(String, String)>, field: &str, ep: &Endpoint) {
    push(
        errs,
        &format!("{field}.base_url"),
        names::base_url(&ep.base_url),
    );
    if let Some(s) = &ep.api_key_secret {
        push(
            errs,
            &format!("{field}.api_key_secret"),
            names::secret_name(s),
        );
    }
}

fn into_error(mut errs: Vec<(String, String)>) -> Result<()> {
    match errs.len() {
        0 => Ok(()),
        1 => {
            let (field, problem) = errs.remove(0);
            Err(CoreError::Invalid { field, problem })
        }
        _ => Err(CoreError::InvalidMany(errs)),
    }
}

fn text_ok(s: &str, max: usize) -> bool {
    s.len() <= max && !s.contains('\0')
}

/// Validate a profile (names, model ids, URLs, env names, secret names) without resolving
/// secrets. Collects every problem.
///
/// Kind rules: `vertex` needs Vertex settings and `provider/model` executor and improver
/// ids; `openai_compatible` needs an endpoint and bare model ids (Vertex settings are
/// optional there, for a judge on Vertex); `custom` takes raw variables in `extra_env` only.
/// `extra_env` may not repeat a name or set one the profile's kind already sets. A judge
/// model LAB does not recognise is not an error (JobBench and WorkBuddy judge differently).
pub fn validate_profile(profile: &ModelProfile) -> Result<()> {
    let mut errs = Vec::new();
    push(&mut errs, "name", names::profile_name(&profile.name));
    if profile.description.chars().count() > 2000 {
        errs.push((
            "description".into(),
            "must be at most 2000 characters".into(),
        ));
    }
    push(
        &mut errs,
        "solver_model",
        names::model_id(&profile.solver_model),
    );
    for (field, v) in [
        ("improver_model", &profile.improver_model),
        ("judge_model", &profile.judge_model),
        ("codex_model", &profile.codex_model),
    ] {
        if let Some(m) = v {
            push(&mut errs, field, names::model_id(m));
        }
    }
    match profile.kind {
        ProfileKind::Vertex => {
            if profile.vertex.is_none() {
                errs.push((
                    "vertex".into(),
                    "a vertex profile needs Vertex settings".into(),
                ));
            }
            if profile.endpoint.is_some() {
                errs.push((
                    "endpoint".into(),
                    "applies to openai_compatible profiles only".into(),
                ));
            }
            for (field, m) in [
                ("solver_model", Some(&profile.solver_model)),
                ("improver_model", profile.improver_model.as_ref()),
            ] {
                if let Some(m) = m
                    && !m.contains('/')
                {
                    errs.push((
                        field.into(),
                        "use OpenCode's provider/model form, e.g. google-vertex/gemini-3.5-flash"
                            .into(),
                    ));
                }
            }
        }
        ProfileKind::OpenaiCompatible => {
            if profile.endpoint.is_none() {
                errs.push((
                    "endpoint".into(),
                    "an openai_compatible profile needs an endpoint".into(),
                ));
            }
            for (field, m) in [
                ("solver_model", Some(&profile.solver_model)),
                ("improver_model", profile.improver_model.as_ref()),
            ] {
                if let Some(m) = m
                    && m.starts_with(&format!("{OPENCODE_PROVIDER_ID}/"))
                {
                    errs.push((
                        field.into(),
                        format!("use the bare model id; the server adds {OPENCODE_PROVIDER_ID}/"),
                    ));
                }
            }
        }
        ProfileKind::Custom => {
            let extras = [
                ("vertex", profile.vertex.is_some()),
                ("endpoint", profile.endpoint.is_some()),
                ("judge_endpoint", profile.judge_endpoint.is_some()),
                ("codex_endpoint", profile.codex_endpoint.is_some()),
            ];
            for (field, present) in extras {
                if present {
                    errs.push((
                        field.into(),
                        "custom profiles take raw variables in extra_env only".into(),
                    ));
                }
            }
        }
    }
    if let Some(v) = &profile.vertex {
        let project_ok = !v.project.is_empty()
            && v.project.len() <= 100
            && v.project.bytes().all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'-' | b'.' | b':')
            })
            && v.project
                .bytes()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric());
        if !project_ok {
            errs.push((
                "vertex.project".into(),
                "expected a Google Cloud project id (lowercase letters, digits, '-')".into(),
            ));
        }
        let location_ok = !v.location.is_empty()
            && v.location.len() <= 40
            && v.location
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-');
        if !location_ok {
            errs.push((
                "vertex.location".into(),
                "expected a location such as global or us-central1".into(),
            ));
        }
        if let Some(s) = &v.credentials_secret {
            push(
                &mut errs,
                "vertex.credentials_secret",
                names::secret_name(s),
            );
        }
    }
    for (field, ep) in [
        ("endpoint", &profile.endpoint),
        ("judge_endpoint", &profile.judge_endpoint),
        ("codex_endpoint", &profile.codex_endpoint),
    ] {
        if let Some(ep) = ep {
            check_endpoint(&mut errs, field, ep);
        }
    }
    if let Some(p) = &profile.prices {
        let all = [
            p.rollout_usd_low,
            p.rollout_usd_high,
            p.session_usd_low,
            p.session_usd_high,
        ];
        if !all.iter().all(|v| v.is_finite() && *v >= 0.0)
            || p.rollout_usd_low > p.rollout_usd_high
            || p.session_usd_low > p.session_usd_high
        {
            errs.push((
                "prices".into(),
                "prices must be finite, non-negative, with low <= high".into(),
            ));
        }
    }
    for h in &profile.egress_hosts {
        push(&mut errs, "egress_hosts", names::hostname(h));
    }
    // extra_env: names, exactly one source, no duplicates, no clash with generated names.
    let ctx = EnvContext {
        runs_dir: String::new(),
        opencode_config_path: String::new(),
        gcp_credentials_path: String::new(),
    };
    let generated: Vec<String> = {
        let mut p = profile.clone();
        p.extra_env.clear();
        plan(&p, &ctx).vars.into_iter().map(|(n, _)| n).collect()
    };
    let mut seen: Vec<&str> = Vec::new();
    for e in &profile.extra_env {
        let field = format!("extra_env.{}", e.name);
        push(&mut errs, &field, names::env_name(&e.name));
        if seen.contains(&e.name.as_str()) {
            errs.push((field.clone(), "is set twice".into()));
        }
        seen.push(&e.name);
        if generated.contains(&e.name) {
            errs.push((
                field.clone(),
                "is already set from the profile's settings; change it there".into(),
            ));
        }
        match (&e.value, &e.secret) {
            (Some(v), None) => {
                if !text_ok(v, 32 * 1024) {
                    errs.push((field, "value must be at most 32 KiB without NUL".into()));
                }
            }
            (None, Some(s)) => push(&mut errs, &field, names::secret_name(s)),
            _ => errs.push((field, "set exactly one of value or secret".into())),
        }
    }
    into_error(errs)
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
///
/// Judge endpoint (`judge_endpoint`, else for `openai_compatible` profiles with a judge model
/// the main endpoint): `HARNESS2_JUDGE_BASE_URL`/`HARNESS2_JUDGE_API_KEY`, plus
/// `OPENAI_BASE_URL`/`OPENAI_API_KEY` when the judge model is a `gpt*`/`o1`/`o3`/`o4`/`o5` id
/// (LAB's judge uses the official OpenAI client for those). Codex endpoint:
/// `HARNESS2_CODEX_BASE_URL`, `HARNESS2_CODEX_API_KEY`, `HARNESS2_CODEX_MODEL`.
pub fn build(
    profile: &ModelProfile,
    secrets: &dyn SecretLookup,
    ctx: &EnvContext,
) -> Result<JobEnv> {
    validate_profile(profile)?;
    for (field, v) in [
        ("runs_dir", &ctx.runs_dir),
        ("opencode_config_path", &ctx.opencode_config_path),
        ("gcp_credentials_path", &ctx.gcp_credentials_path),
    ] {
        if !text_ok(v, 4096) {
            return Err(CoreError::invalid(field, "must be a path without NUL"));
        }
    }
    let plan = plan(profile, ctx);
    let mut env = JobEnv {
        opencode_config: plan.opencode_config,
        egress_hosts: plan.egress_hosts,
        ..JobEnv::default()
    };
    let mut missing: Vec<String> = Vec::new();
    let mut resolve = |name: &str, env: &mut JobEnv| -> Option<String> {
        match secrets.secret(name) {
            Some(v) if !v.is_empty() => {
                if !env.secrets.iter().any(|(n, _)| n == name) {
                    env.secrets.push((name.to_owned(), v.clone()));
                }
                Some(v)
            }
            _ => {
                if !missing.iter().any(|m| m == name) {
                    missing.push(name.to_owned());
                }
                None
            }
        }
    };
    for (var, val) in plan.vars {
        match val {
            Val::Lit(v) => env.vars.push((var, v)),
            Val::Secret(s) => {
                if let Some(v) = resolve(&s, &mut env) {
                    env.vars.push((var, v));
                }
            }
        }
    }
    if let Some(s) = &plan.gcp_credentials_secret {
        env.gcp_credentials_json = resolve(s, &mut env);
    }
    if !missing.is_empty() {
        let errs = missing
            .into_iter()
            .map(|name| {
                (
                    format!("secret {name}"),
                    format!("secret {name} is not set"),
                )
            })
            .collect();
        into_error(errs)?;
    }
    if let Some((name, _)) = env.secrets.iter().find(|(_, v)| v.contains('\0')) {
        return Err(CoreError::invalid(
            format!("secret {name}"),
            format!("secret {name} contains a NUL byte"),
        ));
    }
    Ok(env)
}

#[cfg(test)]
mod tests {
    use super::*;
    use h2s_api::{EnvVar, Prices, VertexSettings};
    use std::collections::HashMap;

    struct Map(HashMap<&'static str, &'static str>);
    impl SecretLookup for Map {
        fn secret(&self, name: &str) -> Option<String> {
            self.0.get(name).map(|v| (*v).to_owned())
        }
    }

    fn ctx() -> EnvContext {
        EnvContext {
            runs_dir: "/data/experiments/e1".into(),
            opencode_config_path: "/data/jobs/j1/opencode.json".into(),
            gcp_credentials_path: "/data/jobs/j1/gcp.json".into(),
        }
    }

    fn base(kind: ProfileKind) -> ModelProfile {
        ModelProfile {
            name: "p".into(),
            kind,
            description: String::new(),
            solver_model: "glm-5".into(),
            improver_model: None,
            judge_model: None,
            codex_model: None,
            vertex: None,
            endpoint: None,
            judge_endpoint: None,
            codex_endpoint: None,
            extra_env: Vec::new(),
            prices: None,
            egress_hosts: Vec::new(),
            secrets: Vec::new(),
        }
    }

    fn var<'a>(env: &'a JobEnv, name: &str) -> Option<&'a str> {
        env.vars
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn openai_compatible_profile() {
        let mut p = base(ProfileKind::OpenaiCompatible);
        p.endpoint = Some(Endpoint {
            base_url: "http://127.0.0.1:4000/v1".into(),
            api_key_secret: Some("LLM_KEY".into()),
        });
        p.judge_model = Some("gpt-5-mini".into());
        p.codex_endpoint = Some(Endpoint {
            base_url: "http://codex.internal:8080/v1".into(),
            api_key_secret: None,
        });
        let secrets = Map(HashMap::from([("LLM_KEY", "sk-test-0123456789")]));
        let env = build(&p, &secrets, &ctx()).unwrap();
        assert_eq!(var(&env, "HARNESS2_RUNS_DIR"), Some("/data/experiments/e1"));
        assert_eq!(
            var(&env, "OPENCODE_CONFIG"),
            Some("/data/jobs/j1/opencode.json")
        );
        assert_eq!(var(&env, PROVIDER_KEY_ENV), Some("sk-test-0123456789"));
        assert_eq!(var(&env, "HARNESS2_SOLVER_MODEL"), Some("h2s/glm-5"));
        assert_eq!(var(&env, "HARNESS2_IMPROVER_MODEL"), Some("h2s/glm-5"));
        assert_eq!(var(&env, "HARNESS2_JUDGE_MODEL"), Some("gpt-5-mini"));
        assert_eq!(
            var(&env, "HARNESS2_JUDGE_BASE_URL"),
            Some("http://127.0.0.1:4000/v1")
        );
        assert_eq!(
            var(&env, "HARNESS2_JUDGE_API_KEY"),
            Some("sk-test-0123456789")
        );
        assert_eq!(
            var(&env, "OPENAI_BASE_URL"),
            Some("http://127.0.0.1:4000/v1")
        );
        assert_eq!(var(&env, "OPENAI_API_KEY"), Some("sk-test-0123456789"));
        assert_eq!(
            var(&env, "HARNESS2_CODEX_BASE_URL"),
            Some("http://codex.internal:8080/v1")
        );
        assert_eq!(var(&env, "HARNESS2_CODEX_MODEL"), Some("glm-5"));
        assert_eq!(
            env.secrets,
            vec![("LLM_KEY".into(), "sk-test-0123456789".into())]
        );
        assert_eq!(env.egress_hosts, vec!["127.0.0.1", "codex.internal"]);
        let cfg: serde_json::Value =
            serde_json::from_str(env.opencode_config.as_deref().unwrap()).unwrap();
        let prov = &cfg["provider"]["h2s"];
        assert_eq!(prov["npm"], "@ai-sdk/openai-compatible");
        assert_eq!(prov["options"]["baseURL"], "http://127.0.0.1:4000/v1");
        assert_eq!(prov["options"]["apiKey"], "{env:H2S_PROVIDER_API_KEY}");
        assert_eq!(prov["models"]["glm-5"]["name"], "glm-5");
        assert!(!env.opencode_config.as_deref().unwrap().contains("sk-test"));
        let dbg = format!("{env:?}");
        assert!(!dbg.contains("sk-test"));
    }

    #[test]
    fn missing_secret_is_named_never_valued() {
        let mut p = base(ProfileKind::OpenaiCompatible);
        p.endpoint = Some(Endpoint {
            base_url: "https://gateway.example.com/v1".into(),
            api_key_secret: Some("GATEWAY_KEY".into()),
        });
        match build(&p, &Map(HashMap::new()), &ctx()) {
            Err(CoreError::Invalid { problem, .. }) => {
                assert_eq!(problem, "secret GATEWAY_KEY is not set");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn vertex_profile_with_key_file() {
        let mut p = base(ProfileKind::Vertex);
        p.solver_model = "google-vertex/gemini-3.5-flash".into();
        p.improver_model = Some("google-vertex/gemini-3.5-pro".into());
        p.judge_model = Some("gemini-3.5-pro".into());
        p.vertex = Some(VertexSettings {
            project: "my-proj-123".into(),
            location: "us-central1".into(),
            credentials_secret: Some("GCP_SA".into()),
        });
        p.extra_env = vec![EnvVar {
            name: "HARNESS2_EXTRA".into(),
            value: Some("1".into()),
            secret: None,
        }];
        let secrets = Map(HashMap::from([(
            "GCP_SA",
            "{\"type\": \"service_account\"}",
        )]));
        let env = build(&p, &secrets, &ctx()).unwrap();
        assert_eq!(var(&env, "GOOGLE_CLOUD_PROJECT"), Some("my-proj-123"));
        assert_eq!(var(&env, "GOOGLE_CLOUD_LOCATION"), Some("us-central1"));
        assert_eq!(
            var(&env, "GOOGLE_APPLICATION_CREDENTIALS"),
            Some("/data/jobs/j1/gcp.json")
        );
        assert_eq!(
            var(&env, "HARNESS2_IMPROVER_MODEL"),
            Some("google-vertex/gemini-3.5-pro")
        );
        assert_eq!(var(&env, "HARNESS2_EXTRA"), Some("1"));
        assert_eq!(var(&env, "OPENCODE_CONFIG"), None);
        assert_eq!(
            env.gcp_credentials_json.as_deref(),
            Some("{\"type\": \"service_account\"}")
        );
        assert_eq!(env.secrets.len(), 1);
        assert_eq!(
            env.egress_hosts,
            vec![
                "oauth2.googleapis.com",
                "us-central1-aiplatform.googleapis.com"
            ]
        );
    }

    #[test]
    fn custom_profile_and_validation() {
        let mut p = base(ProfileKind::Custom);
        p.extra_env = vec![
            EnvVar {
                name: "HARNESS2_SOLVER_MODEL".into(),
                value: Some("x/y".into()),
                secret: None,
            },
            EnvVar {
                name: "MY_TOKEN".into(),
                value: None,
                secret: Some("TOK".into()),
            },
        ];
        p.egress_hosts = vec!["api.example.com".into()];
        let env = build(&p, &Map(HashMap::from([("TOK", "tok-value-123")])), &ctx()).unwrap();
        assert_eq!(
            env.vars.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            vec!["HARNESS2_RUNS_DIR", "HARNESS2_SOLVER_MODEL", "MY_TOKEN"]
        );
        assert_eq!(env.egress_hosts, vec!["api.example.com"]);

        let mut bad = base(ProfileKind::Vertex);
        bad.name = "Bad Name".into();
        bad.extra_env = vec![
            EnvVar {
                name: "LD_PRELOAD".into(),
                value: Some("x".into()),
                secret: None,
            },
            EnvVar {
                name: "HARNESS2_SOLVER_MODEL".into(),
                value: Some("x".into()),
                secret: None,
            },
            EnvVar {
                name: "BOTH".into(),
                value: Some("x".into()),
                secret: Some("Y".into()),
            },
        ];
        bad.prices = Some(Prices {
            rollout_usd_low: 2.0,
            rollout_usd_high: 1.0,
            session_usd_low: 0.0,
            session_usd_high: 0.0,
        });
        match validate_profile(&bad) {
            Err(CoreError::InvalidMany(errs)) => {
                let fields: Vec<&str> = errs.iter().map(|(f, _)| f.as_str()).collect();
                for f in [
                    "name",
                    "vertex",
                    "solver_model",
                    "prices",
                    "extra_env.LD_PRELOAD",
                    "extra_env.HARNESS2_SOLVER_MODEL",
                    "extra_env.BOTH",
                ] {
                    assert!(fields.contains(&f), "{f} missing from {fields:?}");
                }
            }
            other => panic!("{other:?}"),
        }
        assert!(lab_judge_supported("vertex/claude-x"));
        assert!(!lab_judge_supported("glm-5"));
    }
}
