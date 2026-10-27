//! `/system/info`, `/system/doctor`, `/benchmarks`, `/openapi.json`.

use axum::Json;
use axum::extract::State;
use h2w_api::{
    AuthMode, BenchmarkStatus, DoctorReport, ErrorBody, Isolation, ProfileKind, Role, SystemInfo,
};
use serde::Deserialize;

use super::blocking;
use crate::error::ApiError;
use crate::http::AppState;
use crate::http::extract::{ApiQuery, Auth, ViewerRole, deny};

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
