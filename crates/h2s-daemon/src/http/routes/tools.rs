//! `POST /estimate`, `POST /harness/validate`, `GET /splits`.

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::State;
use h2s_api::{
    CostEstimate, ErrorBody, EstimateRequest, FieldProblem, K_MAX, MAX_TASKS_MAX, SplitInfo,
    ValidateHarnessRequest, ValidationReport,
};

use super::{blocking, check_name};
use crate::error::ApiError;
use crate::http::AppState;
use crate::http::extract::{ApiJson, Auth, ViewerRole};

/// Most files in a harness validation request.
pub const VALIDATE_FILES_MAX: usize = 5000;

/// Size and cost of a sweep before running it.
#[utoipa::path(post, path = "/api/v1/estimate", tag = "tools", request_body = EstimateRequest,
    responses((status = 200, body = CostEstimate), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn estimate(
    _auth: Auth<ViewerRole>,
    State(state): State<AppState>,
    ApiJson(req): ApiJson<EstimateRequest>,
) -> Result<Json<CostEstimate>, ApiError> {
    let mut problems = Vec::new();
    let mut p = |field: &str, problem: String| {
        problems.push(FieldProblem {
            field: field.to_owned(),
            problem,
        });
    };
    if req.tasks == 0 || req.tasks > MAX_TASKS_MAX {
        p("tasks", format!("must be between 1 and {MAX_TASKS_MAX}"));
    }
    if req.k == 0 || req.k > K_MAX {
        p("k", format!("must be between 1 and {K_MAX}"));
    }
    if req.modes.is_empty() {
        p("modes", "choose at least one recursion mode".to_owned());
    }
    let mut seen = Vec::new();
    for m in &req.modes {
        if seen.contains(m) {
            p("modes", format!("{} listed twice", m.as_cli()));
        }
        seen.push(*m);
    }
    if !problems.is_empty() {
        return Err(ApiError::validation(problems));
    }
    let prices = match &req.profile {
        Some(name) => {
            check_name("profile", h2s_core::names::profile_name(name))?;
            let n = name.clone();
            let prof = state
                .db
                .call(move |c| crate::db::profiles::get(c, &n))
                .await?
                .ok_or_else(|| ApiError::invalid("profile", format!("no profile named {name}")))?;
            prof.prices
        }
        None => None,
    };
    let history = match &req.experiment {
        Some(name) => {
            let (rec, path) = super::experiments::locate(&state, name).await?;
            let kind = rec.kind;
            super::results_read(&state, move || {
                Ok(crate::jobs::core_backend::history(&path, kind))
            })
            .await?
        }
        None => None,
    };
    Ok(Json(h2s_core::estimate::estimate(
        req.tasks,
        req.k,
        &req.modes,
        prices.as_ref(),
        history,
    )))
}

/// Check a harness tree against harness2's own validation rules.
#[utoipa::path(post, path = "/api/v1/harness/validate", tag = "tools", request_body = ValidateHarnessRequest,
    responses((status = 200, body = ValidationReport), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn validate_harness(
    _auth: Auth<ViewerRole>,
    ApiJson(req): ApiJson<ValidateHarnessRequest>,
) -> Result<Json<ValidationReport>, ApiError> {
    if req.files.len() > VALIDATE_FILES_MAX {
        return Err(ApiError::invalid(
            "files",
            format!("at most {VALIDATE_FILES_MAX} files"),
        ));
    }
    let mut tree = BTreeMap::new();
    for (i, f) in req.files.into_iter().enumerate() {
        check_name(
            &format!("files[{i}].path"),
            h2s_core::names::relative_path(&f.path),
        )?;
        if f.path.is_empty() {
            return Err(ApiError::invalid(
                format!("files[{i}].path"),
                "must name a file",
            ));
        }
        if tree.insert(f.path.clone(), f.content).is_some() {
            return Err(ApiError::invalid(
                format!("files[{i}].path"),
                "listed twice",
            ));
        }
    }
    let surface = req.surface;
    let allow = req.allow_subagents;
    Ok(Json(
        blocking(move || Ok(h2s_core::tree::validate(surface, &tree, allow))).await?,
    ))
}

/// Train/test split files created by `split` jobs.
#[utoipa::path(get, path = "/api/v1/splits", tag = "tools",
    responses((status = 200, body = Vec<SplitInfo>), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn splits(
    _auth: Auth<ViewerRole>,
    State(state): State<AppState>,
) -> Result<Json<Vec<SplitInfo>>, ApiError> {
    Ok(Json(state.db.call(|c| crate::db::splits::list(c)).await?))
}
