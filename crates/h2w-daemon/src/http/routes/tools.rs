//! `POST /estimate`, `POST /harness/validate`, `GET /splits`, `POST /splits`.

use std::collections::{BTreeMap, HashSet};

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use h2w_api::{
    AuditOutcome, Bench, ChooseSplitRequest, CostEstimate, ErrorBody, EstimateRequest,
    FieldProblem, K_MAX, MAX_TASKS_MAX, SPLIT_TASKS_MAX, SplitInfo, ValidateHarnessRequest,
    ValidationReport,
};

use super::{blocking, check_name};
use crate::error::ApiError;
use crate::http::AppState;
use crate::http::extract::{ApiJson, Auth, OperatorRole, ViewerRole};

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
            check_name("profile", h2w_core::names::profile_name(name))?;
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
            let reads = std::sync::Arc::clone(&state.reads);
            let name = name.clone();
            // The same bounded, cached sweep listing as the experiment views.
            super::results_read(&state, move |cancel| {
                let dir = super::experiments::open_dir(&path, kind)?;
                let mut budget = cancel.budget(
                    crate::reads::VIEW_BYTES,
                    u32::try_from(crate::sweeps::MAX_SWEEPS_LISTED).unwrap_or(u32::MAX),
                );
                let sweeps = reads.sweeps_of(&name, &path, kind, &dir, cancel, &mut budget);
                Ok(crate::jobs::core_backend::history_of(&sweeps))
            })
            .await?
        }
        None => None,
    };
    Ok(Json(h2w_core::estimate::estimate(
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
            h2w_core::names::relative_path(&f.path),
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
        blocking(move || Ok(h2w_core::tree::validate(surface, &tree, allow))).await?,
    ))
}

/// Train/test split files: random ones from `split` jobs, and chosen ones (`POST /splits`).
#[utoipa::path(get, path = "/api/v1/splits", tag = "tools",
    responses((status = 200, body = Vec<SplitInfo>), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn splits(
    _auth: Auth<ViewerRole>,
    State(state): State<AppState>,
) -> Result<Json<Vec<SplitInfo>>, ApiError> {
    Ok(Json(state.db.call(|c| crate::db::splits::list(c)).await?))
}

/// Most bytes of one task id in a chosen split.
pub const TASK_ID_MAX: usize = 255;

/// What is wrong with a task id, if anything. harness2's ids are the task's directory relative to
/// the benchmark's dataset (`<domain>/<task>` for LAB and JobBench, a bare name for WorkBuddy);
/// whether the task exists is harness2's check, when a job uses the split.
fn task_id_problem(id: &str) -> Option<String> {
    if id.is_empty() || id.len() > TASK_ID_MAX {
        return Some(format!("must be 1-{TASK_ID_MAX} bytes"));
    }
    if id.chars().any(char::is_control) || id.contains('\\') {
        return Some("must not contain control characters or '\\'".to_owned());
    }
    if id.trim() != id {
        return Some("must not start or end with whitespace".to_owned());
    }
    if id.split('/').any(|s| s.is_empty() || s == "." || s == "..") {
        return Some(
            "must be a task id such as <domain>/<task>: no empty, '.' or '..' part".to_owned(),
        );
    }
    None
}

/// Check a chosen split and build its file in harness2's split format (what
/// `scripts/split_tasks.py` writes; `--split train|test` reads `benchmark`, `train` and `test`).
/// Returns the file and its `(train, test, train_fraction)`.
fn chosen_split(req: &ChooseSplitRequest) -> Result<(Vec<u8>, u32, u32, f64), Vec<FieldProblem>> {
    let mut problems = Vec::new();
    let mut p = |field: String, problem: String| problems.push(FieldProblem { field, problem });
    match (req.bench, req.subset) {
        (Bench::Wb, None) => p(
            "subset".into(),
            "WorkBuddy-Bench needs a subset: office, web or code".into(),
        ),
        (b, Some(_)) if b != Bench::Wb => {
            p(
                "subset".into(),
                "applies to WorkBuddy-Bench (wb) only".into(),
            );
        }
        _ => {}
    }
    if req.train.is_empty() {
        p("train".into(), "choose at least one task".into());
    }
    if req.train.len() + req.test.len() > SPLIT_TASKS_MAX {
        p(
            "train".into(),
            format!("at most {SPLIT_TASKS_MAX} tasks in train and test together"),
        );
    }
    let mut seen: HashSet<&str> = HashSet::new();
    for (side, ids) in [("train", &req.train), ("test", &req.test)] {
        for (i, id) in ids.iter().enumerate().take(SPLIT_TASKS_MAX) {
            if let Some(why) = task_id_problem(id) {
                p(format!("{side}[{i}]"), why);
            } else if !seen.insert(id.as_str()) {
                p(
                    format!("{side}[{i}]"),
                    format!("{id} is listed twice: a task belongs to one side, once"),
                );
            }
        }
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    let (mut train, mut test) = (req.train.clone(), req.test.clone());
    train.sort();
    test.sort();
    let (n_train, n_test) = (train.len(), test.len());
    #[allow(clippy::cast_precision_loss)] // at most SPLIT_TASKS_MAX tasks
    let fraction = n_train as f64 / (n_train + n_test) as f64;
    let bench = match req.subset {
        Some(s) => format!("wb-{}", s.as_cli()),
        None => req.bench.as_cli().to_owned(),
    };
    let file = serde_json::json!({
        "benchmark": bench,
        "seed": 0,
        "train_fraction": fraction,
        "train": train,
        "test": test,
        "chosen_by": "harness2-workbench",
    });
    let bytes = serde_json::to_vec_pretty(&file).map_err(|e| {
        vec![FieldProblem {
            field: "train".into(),
            problem: e.to_string(),
        }]
    })?;
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    Ok((bytes, count(n_train), count(n_test), fraction))
}

/// Save a split of chosen tasks: `split: train` with `split_file` this name runs exactly the
/// `train` tasks.
#[utoipa::path(post, path = "/api/v1/splits", tag = "tools", request_body = ChooseSplitRequest,
    responses((status = 201, body = SplitInfo), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn choose_split(
    auth: Auth<OperatorRole>,
    State(state): State<AppState>,
    ApiJson(req): ApiJson<ChooseSplitRequest>,
) -> Result<(StatusCode, Json<SplitInfo>), ApiError> {
    check_name("name", h2w_core::names::experiment_name(&req.name))?;
    let (bytes, train, test, train_fraction) = chosen_split(&req).map_err(ApiError::validation)?;
    let n = req.name.clone();
    if state
        .db
        .call(move |c| crate::db::splits::get(c, &n))
        .await?
        .is_some()
    {
        return Err(ApiError::conflict(format!(
            "a split named {} already exists",
            req.name
        )));
    }
    let dir = state.cfg.paths.splits_dir.clone();
    let path = dir.join(format!("{}.json", req.name));
    let file = path.clone();
    blocking(move || {
        crate::util::ensure_private_dir(&dir)
            .and_then(|()| crate::util::write_private_new(&file, &bytes))
            .map_err(|e| ApiError::conflict(format!("split file: {e}")))
    })
    .await?;
    let info = SplitInfo {
        name: req.name.clone(),
        bench: req.bench,
        subset: req.subset,
        seed: 0,
        train_fraction,
        train,
        test,
        created_at: crate::util::now_rfc3339(),
        chosen: true,
    };
    let rec = info.clone();
    let now = crate::util::now_unix();
    let entry = auth
        .audit("split.choose", &req.name, AuditOutcome::Success)
        .detail(format!("{train} train, {test} test"));
    if let Err(e) = state
        .db
        .call(move |c| crate::db::splits::insert(c, &rec, now))
        .await
    {
        let _ = std::fs::remove_file(&path);
        return Err(e.into());
    }
    state.audit(entry).await;
    Ok((StatusCode::CREATED, Json(info)))
}

#[cfg(test)]
mod split_tests {
    use super::*;
    use h2w_api::WbSubset;

    fn req(
        bench: Bench,
        subset: Option<WbSubset>,
        train: &[&str],
        test: &[&str],
    ) -> ChooseSplitRequest {
        ChooseSplitRequest {
            name: "chosen".into(),
            bench,
            subset,
            train: train.iter().map(|s| (*s).to_owned()).collect(),
            test: test.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    #[test]
    fn writes_harness2s_split_format() {
        let (bytes, train, test, fraction) = chosen_split(&req(
            Bench::Jb,
            None,
            &[
                "reporters_and_correspondents/task1",
                "data_entry_keyers/task1",
            ],
            &["producers/task1"],
        ))
        .unwrap();
        assert_eq!((train, test), (2, 1));
        assert!((fraction - 2.0 / 3.0).abs() < 1e-12);
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["benchmark"], "jb");
        assert_eq!(v["train"][0], "data_entry_keyers/task1", "sorted");
        assert_eq!(v["test"][0], "producers/task1");
        let (bytes, ..) = chosen_split(&req(
            Bench::Wb,
            Some(WbSubset::Office),
            &["delivery-package-readonly-diff"],
            &[],
        ))
        .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["benchmark"], "wb-office");
    }

    #[test]
    fn refuses_bad_choices() {
        let fields = |r: ChooseSplitRequest| -> Vec<String> {
            chosen_split(&r)
                .unwrap_err()
                .into_iter()
                .map(|p| p.field)
                .collect()
        };
        assert_eq!(fields(req(Bench::Lab, None, &[], &[])), ["train"]);
        assert_eq!(fields(req(Bench::Wb, None, &["x"], &[])), ["subset"]);
        assert_eq!(
            fields(req(Bench::Lab, Some(WbSubset::Web), &["a/b"], &[])),
            ["subset"]
        );
        assert_eq!(
            fields(req(Bench::Lab, None, &["a/b", "a/b"], &[])),
            ["train[1]"]
        );
        assert_eq!(
            fields(req(Bench::Lab, None, &["a/b"], &["a/b"])),
            ["test[0]"]
        );
        for bad in [
            "", "../etc", "a//b", "/a/b", "a/./b", " a/b", "a/b\n", "a\\b",
        ] {
            assert_eq!(
                fields(req(Bench::Lab, None, &[bad], &[])),
                ["train[0]"],
                "{bad:?}"
            );
        }
        let long = "x".repeat(TASK_ID_MAX + 1);
        assert_eq!(
            fields(req(Bench::Lab, None, &[long.as_str()], &[])),
            ["train[0]"]
        );
    }
}
