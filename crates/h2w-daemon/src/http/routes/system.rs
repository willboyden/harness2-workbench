//! `/system/info`, `/system/doctor`, `/system/config`, `/system/config/validate`,
//! `/benchmarks`, `/openapi.json`.

use axum::Json;
use axum::extract::State;
use h2w_api::{
    AuditOutcome, AuthMode, BenchmarkStatus, ConfigValidation, DoctorReport, ErrorBody, Isolation,
    ProfileKind, Role, ServerConfigFile, SystemInfo, ValidateConfigRequest,
};
use serde::Deserialize;

use super::blocking;
use crate::config::process_env;
use crate::config_editor::{self, CONFIG_TEXT_MAX};
use crate::error::ApiError;
use crate::http::AppState;
use crate::http::extract::{AdminRole, ApiJson, ApiQuery, Auth, ViewerRole, deny};

/// Build-time commit, when the build sets `H2W_BUILD_COMMIT`.
pub const BUILD_COMMIT: Option<&str> = option_env!("H2W_BUILD_COMMIT");

/// Features this build has and this configuration enables.
#[must_use]
pub fn features(state: &AppState) -> Vec<String> {
    let mut f = Vec::new();
    if cfg!(feature = "otel") {
        f.push("otel".to_owned());
    }
    if state.cfg.isolation() == Isolation::Container {
        f.push("container".to_owned());
    }
    if state.cfg.file.auth.mode == AuthMode::Proxy {
        f.push("proxy_auth".to_owned());
    }
    if !state.cfg.file.telemetry.metrics_token_secret.is_empty() {
        f.push("metrics_token".to_owned());
    }
    if crate::http::static_ui::ui_built() {
        f.push("web_ui".to_owned());
    }
    f
}

/// Server status (admin-only fields filled for admins).
#[utoipa::path(get, path = "/api/v1/system/info", tag = "system",
    responses((status = 200, body = SystemInfo), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn info(
    auth: Auth<ViewerRole>,
    State(state): State<AppState>,
) -> Result<Json<SystemInfo>, ApiError> {
    let home = state.cfg.paths.harness2_home.clone();
    let managed = state.cfg.paths.harness2_managed;
    let mut harness2 = blocking(move || Ok(h2w_core::install::read_status(&home, managed))).await?;
    if auth.is_admin() {
        harness2.home = Some(state.cfg.paths.harness2_home.display().to_string());
    } else {
        harness2.home = None;
    }
    let container_runtime = if state.cfg.isolation() == Isolation::Container {
        let runtime = state.cfg.file.isolation.container.runtime.as_str();
        let answers = state.jobs.container_runtime_answered().await
            || state.doctor.cached().await.is_some_and(|r| {
                r.checks
                    .iter()
                    .any(|c| c.id == runtime && c.level == h2w_api::CheckLevel::Ok)
            });
        answers.then(|| runtime.to_owned())
    } else {
        None
    };
    Ok(Json(SystemInfo {
        workbench_version: env!("CARGO_PKG_VERSION").to_owned(),
        build_commit: BUILD_COMMIT.map(str::to_owned),
        harness2,
        isolation: state.cfg.isolation(),
        host_risks_preacknowledged: state.cfg.file.isolation.preacknowledge_host_risks,
        container_runtime,
        auth_mode: state.cfg.file.auth.mode,
        server_time: crate::util::now_rfc3339(),
        jobs_running: state.jobs.running_count(),
        jobs_max_concurrent: state.cfg.file.jobs.max_concurrent,
        data_dir: auth
            .is_admin()
            .then(|| state.cfg.paths.data_dir.display().to_string()),
        features: features(&state),
        cloud_judges: state.cfg.file.judge.cloud,
        private_judge_hosts: state.cfg.judge_private_hosts.clone(),
    }))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct DoctorQuery {
    /// Run the probes now instead of using the cached report (operator).
    #[serde(default)]
    pub refresh: Option<bool>,
}

/// Environment checks (cached for 60 s).
#[utoipa::path(get, path = "/api/v1/system/doctor", tag = "system", params(DoctorQuery),
    responses((status = 200, body = DoctorReport), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn doctor(
    auth: Auth<ViewerRole>,
    State(state): State<AppState>,
    ApiQuery(q): ApiQuery<DoctorQuery>,
) -> Result<Json<DoctorReport>, ApiError> {
    let refresh = q.refresh.unwrap_or(false);
    if refresh && !auth.principal.role.allows(Role::Operator) {
        return Err(deny(
            &state,
            &auth.principal,
            &auth.meta,
            "GET /api/v1/system/doctor?refresh=true",
            Role::Operator,
        )
        .await);
    }
    let st = state.clone();
    let facts = async move {
        let home = st.cfg.paths.harness2_home.clone();
        let managed = st.cfg.paths.harness2_managed;
        let harness2 =
            tokio::task::spawn_blocking(move || h2w_core::install::read_status(&home, managed))
                .await
                .unwrap_or_else(|_| {
                    h2w_core::install::read_status(std::path::Path::new("/"), managed)
                });
        let uses_vertex = st
            .db
            .call(|c| crate::db::profiles::list(c))
            .await
            .map(|ps| ps.iter().any(|p| p.kind == ProfileKind::Vertex))
            .unwrap_or(false);
        crate::doctor::ContextFacts {
            harness2,
            isolation: st.cfg.isolation(),
            data_dir: st.cfg.paths.data_dir.clone(),
            uses_vertex,
            container: (st.cfg.isolation() == Isolation::Container)
                .then(|| st.cfg.file.isolation.container.clone()),
        }
    };
    Ok(Json(state.doctor.report(refresh, facts).await))
}

/// `true` when `text` holds the value of a secret the server knows: stored secrets,
/// `H2W_SECRET_*` variables and the reverse proxy's shared secret. Values shorter than
/// `MIN_SECRET_LEN` are left out: a weak value matches unrelated text by chance and would make
/// every read refuse.
/// `(name, value)` of every secret the server knows.
async fn secret_values(state: &AppState) -> Result<Vec<(String, String)>, ApiError> {
    let mut pairs: Vec<(String, String)> = state
        .secrets
        .all_values()
        .await?
        .into_iter()
        .map(|(n, v)| (n, v.to_string()))
        .collect();
    if let Some(p) = &state.proxy_secret {
        pairs.push(("PROXY_SECRET".to_owned(), p.to_string()));
    }
    Ok(pairs)
}

/// The server's configuration file, read-only (admin). Sent only when every key is a known
/// setting and nothing in it looks like a credential (`config_editor::refusal`) or is the
/// value of a known secret; otherwise `refused` says why, without quoting it. Never writes.
#[utoipa::path(get, path = "/api/v1/system/config", tag = "system",
    responses((status = 200, body = ServerConfigFile), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn config_file(
    auth: Auth<AdminRole>,
    State(state): State<AppState>,
) -> Result<Json<ServerConfigFile>, ApiError> {
    let env_overrides = config_editor::overrides_in_force(&process_env);
    let max_bytes = CONFIG_TEXT_MAX as u64;
    let Some(path) = state.cfg.source.clone() else {
        return Ok(Json(ServerConfigFile {
            path: None,
            text: None,
            refused: None,
            changed_since_start: false,
            env_overrides,
            max_bytes,
        }));
    };
    let running = state.cfg.file.clone();
    let p = path.clone();
    let secrets = secret_values(&state).await?;
    let read = blocking(move || {
        Ok(config_editor::read_capped(&p).map(|text| {
            let changed = config_editor::differs_from(&text, &running, &process_env);
            // The shape allowlist first; the stored-secret check (values of 8+ bytes) only on
            // a text it passes, so a withheld file says nothing about the stored secrets.
            let redactor = h2w_core::redact::Redactor::new_ignoring_weak(secrets);
            let refusal = config_editor::refusal_or_stored_secret(&text, &process_env, |s| {
                redactor.finds_secret(s)
            });
            (text, changed, refusal)
        }))
    })
    .await?;
    let (text, refused, changed_since_start) = match read {
        Ok((_, changed, Some(why))) => (None, Some(why), changed),
        Ok((text, changed, None)) => (Some(text), None, changed),
        Err(why) => (None, Some(why), true),
    };
    let target = path.display().to_string();
    let entry = match &refused {
        None => auth.audit("config.read", &target, AuditOutcome::Success),
        Some(why) => auth
            .audit("config.read", &target, AuditOutcome::Denied)
            .detail(format!("not sent: {why}")),
    };
    state.audit(entry).await;
    Ok(Json(ServerConfigFile {
        path: Some(target),
        text,
        refused: refused.map(|why| format!("The configuration file is not sent because {why}.")),
        changed_since_start,
        env_overrides,
        max_bytes,
    }))
}

/// Check a configuration text with the server's own parser and validator (admin). The text
/// is checked in memory: nothing is written and no setting changes.
#[utoipa::path(post, path = "/api/v1/system/config/validate", tag = "system",
    request_body = ValidateConfigRequest,
    responses((status = 200, body = ConfigValidation), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn validate_config(
    _auth: Auth<AdminRole>,
    ApiJson(req): ApiJson<ValidateConfigRequest>,
) -> Result<Json<ConfigValidation>, ApiError> {
    if req.text.len() > CONFIG_TEXT_MAX {
        return Err(ApiError::invalid(
            "text",
            format!("at most {} KiB", CONFIG_TEXT_MAX / 1024),
        ));
    }
    // check_text reads no secret and its messages never quote the posted text, so the
    // answer cannot confirm a guess at a secret's value.
    let v = blocking(move || Ok(config_editor::check_text(&req.text, &process_env))).await?;
    Ok(Json(v))
}

/// Benchmarks, their installation state, and whether this server sets them up and runs them
/// (`unavailable`: an administrator's `[[benchmarks.not_offered]]` entry, or WorkBuddy-Bench
/// under container isolation; their jobs are refused).
#[utoipa::path(get, path = "/api/v1/benchmarks", tag = "system",
    responses((status = 200, body = Vec<BenchmarkStatus>), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn benchmarks(
    _auth: Auth<ViewerRole>,
    State(state): State<AppState>,
) -> Result<Json<Vec<BenchmarkStatus>>, ApiError> {
    let home = state.cfg.paths.harness2_home.clone();
    let isolation = state.cfg.isolation();
    let not_offered = state.cfg.not_offered.clone();
    Ok(Json(
        blocking(move || Ok(h2w_core::bench::status_on(&home, isolation, &not_offered))).await?,
    ))
}

/// This API's OpenAPI 3.1 document.
#[utoipa::path(get, path = "/api/v1/openapi.json", tag = "system",
    responses((status = 200, description = "OpenAPI 3.1 document", content_type = "application/json"),
        (status = "default", description = "Error", body = ErrorBody)))]
pub async fn openapi(_auth: Auth<ViewerRole>) -> Json<serde_json::Value> {
    Json(crate::http::openapi::document_json())
}
