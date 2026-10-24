//! Experiments: metadata in the database, results in `<experiments_dir>/<name>`.

use std::path::PathBuf;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use h2s_api::{
    AuditOutcome, CreateExperimentRequest, ErrorBody, Experiment, ExperimentKind, Isolation,
    KeyValue, Manifest, ManifestJob, UpdateExperimentRequest,
};
use h2s_core::isolation::EXPERIMENT_MOUNT;
use h2s_core::results::ExperimentDir;
use http::StatusCode;
use serde::Deserialize;

use super::{blocking, check_name, results_read};
use crate::db::experiments::ExperimentRecord;
use crate::error::ApiError;
use crate::http::AppState;
use crate::http::extract::{ApiJson, ApiQuery, Auth, OperatorRole, ViewerRole};
use crate::util::{now_unix, rfc3339};

/// Longest description accepted.
pub const DESCRIPTION_MAX: usize = 4000;
/// Most tags per experiment.
pub const TAGS_MAX: usize = 32;

/// Open an experiment's results read-only (host path plus the container mount as roots).
pub(crate) fn open_dir(
    path: &std::path::Path,
    kind: ExperimentKind,
) -> Result<ExperimentDir, ApiError> {
    ExperimentDir::open(path, kind, &[EXPERIMENT_MOUNT.to_owned()]).map_err(ApiError::from)
}

/// The record and directory of an experiment named in a URL.
pub(crate) async fn locate(
    state: &AppState,
    name: &str,
) -> Result<(ExperimentRecord, PathBuf), ApiError> {
    check_name("exp", h2s_core::names::experiment_name(name))?;
    let n = name.to_owned();
    let rec = state
        .db
        .call(move |c| crate::db::experiments::get(c, &n))
        .await?
        .ok_or_else(|| ApiError::not_found(format!("no experiment named {name}")))?;
    let root = state.cfg.paths.experiments_dir.clone();
    let n = name.to_owned();
    let path = blocking(move || {
        crate::expdirs::existing(&root, &n).map_err(|e| match e {
            crate::expdirs::DirError::Missing(_) => {
                ApiError::not_found(format!("the directory of experiment {n} is missing"))
            }
            other => ApiError::conflict(other.to_string()),
        })
    })
    .await?;
    Ok((rec, path))
}

/// The API view of an experiment: its record plus the sweeps on disk (one at a time and
/// cached, see `http::sweeps`; a sweep that cannot be read is left out and logged) and,
/// with `with_size`, the size of its directory.
async fn view(
    state: &AppState,
    rec: ExperimentRecord,
    path: Option<PathBuf>,
    with_size: bool,
) -> Result<Experiment, ApiError> {
    let kind = rec.kind;
    let name = rec.name.clone();
    let cache = std::sync::Arc::clone(&state.sweep_cache);
    let (sweeps, size) = match path {
        Some(p) => {
            results_read(state, move || {
                let dir = match open_dir(&p, kind) {
                    Ok(d) => d,
                    Err(e) => {
                        tracing::warn!(experiment = %name, error = %e.message, "could not open an experiment's directory");
                        return Ok((Vec::new(), None));
                    }
                };
                let sweeps = cache.list(&name, &p, kind, &dir);
                let size = if with_size {
                    dir.size_bytes().ok()
                } else {
                    None
                };
                Ok((sweeps, size))
            })
            .await?
        }
        None => (Vec::new(), None),
    };
    Ok(Experiment {
        name: rec.name,
        kind: rec.kind,
        description: rec.description,
        tags: rec.tags,
        profile: rec.profile,
        created_at: rfc3339(rec.created_at),
        created_by: rec.created_by,
        archived: rec.archived,
        sweeps,
        size_bytes: size,
    })
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct ListQuery {
    /// Include archived experiments.
    #[serde(default)]
    pub archived: Option<bool>,
}

/// Experiments with their sweeps (archived ones with `archived=true`).
#[utoipa::path(get, path = "/api/v1/experiments", tag = "experiments", params(ListQuery),
    responses((status = 200, body = Vec<Experiment>), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn list(
    _auth: Auth<ViewerRole>,
    State(state): State<AppState>,
    ApiQuery(q): ApiQuery<ListQuery>,
) -> Result<Json<Vec<Experiment>>, ApiError> {
    let include = q.archived.unwrap_or(false);
    let recs = state
        .db
        .call(move |c| crate::db::experiments::list(c, include))
        .await?;
    let mut out = Vec::with_capacity(recs.len());
    for rec in recs {
        let root = state.cfg.paths.experiments_dir.clone();
        let n = rec.name.clone();
        let path = blocking(move || Ok(crate::expdirs::existing(&root, &n).ok())).await?;
        out.push(view(&state, rec, path, false).await?);
    }
    Ok(Json(out))
}

fn validate_meta(description: Option<&str>, tags: Option<&[String]>) -> Result<(), ApiError> {
    let mut problems = Vec::new();
    if let Some(d) = description
        && (d.chars().count() > DESCRIPTION_MAX || d.contains('\0'))
    {
        problems.push(h2s_api::FieldProblem {
            field: "description".into(),
            problem: format!("at most {DESCRIPTION_MAX} characters, no NUL"),
        });
    }
    if let Some(t) = tags {
        if t.len() > TAGS_MAX {
            problems.push(h2s_api::FieldProblem {
                field: "tags".into(),
                problem: format!("at most {TAGS_MAX} tags"),
            });
        }
        for (i, tag) in t.iter().enumerate() {
            if let Err(e) = check_name(&format!("tags[{i}]"), h2s_core::names::tag(tag)) {
                problems.extend(e.details);
            }
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(ApiError::validation(problems))
    }
}

/// Create an experiment (adopting an existing directory of that name).
#[utoipa::path(post, path = "/api/v1/experiments", tag = "experiments",
    request_body = CreateExperimentRequest,
    responses((status = 201, body = Experiment), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn create(
    auth: Auth<OperatorRole>,
    State(state): State<AppState>,
    ApiJson(req): ApiJson<CreateExperimentRequest>,
) -> Result<Response, ApiError> {
    check_name("name", h2s_core::names::experiment_name(&req.name))?;
    validate_meta(Some(&req.description), Some(&req.tags))?;
    if let Some(p) = &req.profile {
        check_name("profile", h2s_core::names::profile_name(p))?;
        let n = p.clone();
        let exists = state
            .db
            .call(move |c| crate::db::profiles::get(c, &n))
            .await?
            .is_some();
        if !exists {
            return Err(ApiError::invalid(
                "profile",
                format!("no profile named {p}"),
            ));
        }
    }
    let n = req.name.clone();
    if state
        .db
        .call(move |c| crate::db::experiments::get(c, &n))
        .await?
        .is_some()
    {
        return Err(ApiError::conflict(format!(
            "an experiment named {} already exists",
            req.name
        )));
    }
    let root = state.cfg.paths.experiments_dir.clone();
    let n = req.name.clone();
    let (path, created) = blocking(move || {
        crate::expdirs::create_or_adopt(&root, &n).map_err(|e| ApiError::conflict(e.to_string()))
    })
    .await?;
    let rec = ExperimentRecord {
        name: req.name.clone(),
        kind: req.kind,
        description: req.description.clone(),
        tags: req.tags.clone(),
        profile: req.profile.clone(),
        created_at: now_unix(),
        created_by: auth.principal.name.clone(),
        archived: false,
    };
    let r = rec.clone();
    let entry = auth
        .audit("experiment.create", &req.name, AuditOutcome::Success)
        .detail(if created {
            "new directory"
        } else {
            "adopted an existing directory"
        });
    let res = state
        .db
        .call(move |c| {
            crate::db::write_tx(c, |tx| {
                crate::db::experiments::insert(tx, &r)?;
                crate::db::audit::append(tx, &entry)?;
                Ok(())
            })
        })
        .await;
    if let Err(e) = res {
        // The directory stays: a concurrent request may have adopted it, and an empty
        // directory is adopted by the next create anyway.
        return Err(match e {
            crate::db::DbError::Sqlite(_) => {
                ApiError::conflict(format!("an experiment named {} already exists", req.name))
            }
            other => other.into(),
        });
    }
    let exp = view(&state, rec, Some(path), false).await?;
    Ok((StatusCode::CREATED, Json(exp)).into_response())
}

/// One experiment, with its sweeps and size.
#[utoipa::path(get, path = "/api/v1/experiments/{exp}", tag = "experiments",
    params(("exp" = String, Path, description = "Experiment name")),
    responses((status = 200, body = Experiment), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn get_one(
    _auth: Auth<ViewerRole>,
    State(state): State<AppState>,
    Path(exp): Path<String>,
) -> Result<Json<Experiment>, ApiError> {
    let (rec, path) = locate(&state, &exp).await?;
    Ok(Json(view(&state, rec, Some(path), true).await?))
}

/// Change description, tags or the archived flag.
#[utoipa::path(patch, path = "/api/v1/experiments/{exp}", tag = "experiments",
    params(("exp" = String, Path, description = "Experiment name")),
    request_body = UpdateExperimentRequest,
    responses((status = 200, body = Experiment), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn update(
    auth: Auth<OperatorRole>,
    State(state): State<AppState>,
    Path(exp): Path<String>,
    ApiJson(req): ApiJson<UpdateExperimentRequest>,
) -> Result<Json<Experiment>, ApiError> {
    let (_, path) = locate(&state, &exp).await?;
    validate_meta(req.description.as_deref(), req.tags.as_deref())?;
    let mut changed = Vec::new();
    if req.description.is_some() {
        changed.push("description");
    }
    if req.tags.is_some() {
        changed.push("tags");
    }
    if let Some(a) = req.archived {
        changed.push(if a { "archived" } else { "unarchived" });
    }
    let entry = auth
        .audit("experiment.update", &exp, AuditOutcome::Success)
        .detail(changed.join(", "));
    let name = exp.clone();
    let rec = state
        .db
        .call(move |c| {
            crate::db::write_tx(c, |tx| {
                let r = crate::db::experiments::update(
                    tx,
                    &name,
                    req.description.as_deref(),
                    req.tags.as_deref(),
                    req.archived,
                )?;
                crate::db::audit::append(tx, &entry)?;
                Ok(r)
            })
        })
        .await?
        .ok_or_else(|| ApiError::not_found(format!("no experiment named {exp}")))?;
    Ok(Json(view(&state, rec, Some(path), false).await?))
}

/// Replace absolute host paths in an argv with placeholders.
fn sanitize_argv(state: &AppState, argv: Vec<String>) -> Vec<String> {
    let p = &state.cfg.paths;
    let masker = crate::jobs::logs::PathMasker::new(vec![
        (
            p.experiments_dir.display().to_string(),
            "<experiments>".into(),
        ),
        (p.harness2_home.display().to_string(), "<harness2>".into()),
        (p.data_dir.display().to_string(), "<data>".into()),
    ]);
    argv.into_iter()
        .map(|a| masker.mask(&a).into_owned())
        .collect()
}

/// The experiment's reproducibility manifest.
#[utoipa::path(get, path = "/api/v1/experiments/{exp}/manifest", tag = "experiments",
    params(("exp" = String, Path, description = "Experiment name")),
    responses((status = 200, body = Manifest), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn manifest(
    _auth: Auth<ViewerRole>,
    State(state): State<AppState>,
    Path(exp): Path<String>,
) -> Result<Json<Manifest>, ApiError> {
    let (rec, path) = locate(&state, &exp).await?;
    let profile = match &rec.profile {
        Some(p) => {
            let n = p.clone();
            state
                .db
                .call(move |c| crate::db::profiles::get(c, &n))
                .await?
        }
        None => None,
    };
    let n = exp.clone();
    let rows = state
        .db
        .call(move |c| crate::db::jobs::for_experiment(c, &n))
        .await?;
    let jobs: Vec<ManifestJob> = rows
        .into_iter()
        .map(|r| ManifestJob {
            id: r.job.id,
            kind: r.job.kind,
            status: r.job.status,
            argv: sanitize_argv(&state, r.argv.unwrap_or_default()),
            created_at: r.job.created_at,
            finished_at: r.job.finished_at,
        })
        .collect();
    let mut models = Vec::new();
    let mut env_names = Vec::new();
    if let Some(p) = &profile {
        let mut push = |k: &str, v: &Option<String>| {
            if let Some(v) = v {
                models.push(KeyValue {
                    key: k.to_owned(),
                    value: v.clone(),
                });
            }
        };
        push("solver", &Some(p.solver_model.clone()));
        push("improver", &p.improver_model);
        push("judge", &p.judge_model);
        push("codex", &p.codex_model);
        // Variable NAMES only: build the environment against placeholder values.
        struct Placeholder;
        impl h2s_core::env::SecretLookup for Placeholder {
            fn secret(&self, _: &str) -> Option<String> {
                Some("[set]".to_owned())
            }
        }
        let ctx = h2s_core::env::EnvContext {
            runs_dir: EXPERIMENT_MOUNT.to_owned(),
            opencode_config_path: "/h2s/job/opencode.json".to_owned(),
            gcp_credentials_path: "/h2s/job/gcp-credentials.json".to_owned(),
        };
        if let Ok(env) = h2s_core::env::build(p, &Placeholder, &ctx) {
            env_names = env.vars.into_iter().map(|(n, _)| n).collect();
            env_names.sort();
            env_names.dedup();
        }
    }
    let isolation = state.cfg.isolation();
    let network_policy = match isolation {
        Isolation::Host => "host: jobs share the server's filesystem and network".to_owned(),
        Isolation::Container => {
            let c = &state.cfg.file.isolation.container;
            if c.network == "none" {
                format!("container ({}), network none", c.image)
            } else {
                let allow: Vec<&str> = state
                    .cfg
                    .file
                    .egress
                    .allow
                    .iter()
                    .map(crate::config::AllowEntry::host)
                    .collect();
                format!(
                    "container ({}), network {} behind the egress proxy; allowlist: {}",
                    c.image,
                    c.network,
                    allow.join(", ")
                )
            }
        }
    };
    let home = state.cfg.paths.harness2_home.clone();
    let managed = state.cfg.paths.harness2_managed;
    let kind = rec.kind;
    let (identities, digest, status) = results_read(&state, move || {
        let dir = open_dir(&path, kind)?;
        let identities = dir.sweep_identities()?;
        let digest = dir.results_digest()?;
        Ok((
            identities,
            digest,
            h2s_core::install::read_status(&home, managed),
        ))
    })
    .await?;
    let input = h2s_core::manifest::ManifestInput {
        generated_at: crate::util::now_rfc3339(),
        studio_version: env!("CARGO_PKG_VERSION").to_owned(),
        experiment: exp,
        harness2_commit: status.commit,
        dependency_pins: h2s_core::bench::DEPENDENCY_PINS
            .iter()
            .map(|(k, v)| KeyValue {
                key: (*k).to_owned(),
                value: (*v).to_owned(),
            })
            .chain(std::iter::once(KeyValue {
                key: "codex".to_owned(),
                value: h2s_core::bench::CODEX_VERSION.to_owned(),
            }))
            .collect(),
        models,
        env_names,
        isolation,
        network_policy,
        jobs,
        sweep_identities: identities,
        results_digest: digest,
        jobbench_revision: None,
    };
    Ok(Json(h2s_core::manifest::build(input)))
}
