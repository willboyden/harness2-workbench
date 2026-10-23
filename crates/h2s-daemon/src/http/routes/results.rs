//! Read-only views of what harness2 wrote, through `h2s_core::results::ExperimentDir`
//! (cap-std: no path from a request or a results file can leave the experiment).

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use h2s_api::{
    AuditSession, Comparison, ErrorBody, ExportBundle, ExportTarget, FileContent, FileEntry,
    HarnessDiff, HarnessTree, LibraryDomain, Mode, Page, ScoreReport, Scores, TaskDetail, TaskRow,
};
use h2s_core::namespace::Namespace;
use h2s_core::results::DEFAULT_READ_CAP;
use http::{HeaderValue, header};
use serde::Deserialize;

use super::experiments::{locate, open_dir};
use super::{blocking, check_name, clamp_limit};
use crate::error::ApiError;
use crate::http::AppState;
use crate::http::extract::{ApiQuery, Auth, ViewerRole};

#[derive(Debug, Deserialize)]
pub struct SweepPath {
    pub exp: String,
    pub sweep: String,
}

#[derive(Debug, Deserialize)]
pub struct TaskPath {
    pub exp: String,
    pub sweep: String,
    pub task: String,
}

#[derive(Debug, Deserialize)]
pub struct ModePath {
    pub exp: String,
    pub sweep: String,
    pub task: String,
    pub mode: String,
}

#[derive(Debug, Deserialize)]
pub struct LabelPath {
    pub exp: String,
    pub sweep: String,
    pub task: String,
    pub mode: String,
    pub label: String,
}

#[derive(Debug, Deserialize)]
pub struct AuditPath {
    pub exp: String,
    pub sweep: String,
    pub session: String,
    pub part: String,
}

/// A sweep id from a URL (the summary namespace).
fn sweep_ns(s: &str) -> Result<Namespace, ApiError> {
    Namespace::parse(s).ok_or_else(|| ApiError::not_found(format!("no sweep named {s}")))
}

/// `parallel`/`sequential` (or the on-disk `agg`/`seq`); 422 otherwise.
fn mode(field: &str, s: &str) -> Result<Mode, ApiError> {
    Mode::from_token(s)
        .ok_or_else(|| ApiError::invalid(field, "must be parallel or sequential (agg or seq)"))
}

fn label(field: &str, s: &str) -> Result<(), ApiError> {
    check_name(field, h2s_core::names::harness_label(s))
}

fn task_slug(s: &str) -> Result<(), ApiError> {
    check_name("task", h2s_core::names::task_slug(s))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct TasksQuery {
    /// Only tasks of this domain.
    pub domain: Option<String>,
    /// Only tasks with this status (`ok`, `skipped`, `partial`, `base-failed`, `running`).
    pub status: Option<String>,
    /// Page size (1..=1000, default 200).
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

/// Task rows of a sweep.
#[utoipa::path(get, path = "/api/v1/experiments/{exp}/sweeps/{sweep}/tasks", tag = "results",
    params(("exp" = String, Path), ("sweep" = String, Path, description = "Summary namespace"), TasksQuery),
    responses((status = 200, body = Page<TaskRow>), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn tasks(
    _auth: Auth<ViewerRole>,
    State(state): State<AppState>,
    Path(p): Path<SweepPath>,
    ApiQuery(q): ApiQuery<TasksQuery>,
) -> Result<Json<Page<TaskRow>>, ApiError> {
    let (rec, path) = locate(&state, &p.exp).await?;
    let ns = sweep_ns(&p.sweep)?;
    if let Some(d) = &q.domain {
        check_name("domain", h2s_core::names::domain(d))?;
    }
    if let Some(s) = &q.status
        && !matches!(
            s.as_str(),
            "ok" | "skipped" | "partial" | "base-failed" | "running" | "exception"
        )
    {
        return Err(ApiError::invalid(
            "status",
            "must be ok, skipped, partial, base-failed, running or exception",
        ));
    }
    let limit = usize::try_from(clamp_limit(q.limit, 200, 1000)).unwrap_or(200);
    let offset = match q.cursor.as_deref() {
        Some(c) => usize::try_from(
            crate::db::cursor::decode(c)
                .ok_or_else(|| ApiError::invalid("cursor", "invalid cursor"))?,
        )
        .map_err(|_| ApiError::invalid("cursor", "invalid cursor"))?,
        None => 0,
    };
    let kind = rec.kind;
    let rows = blocking(move || Ok(open_dir(&path, kind)?.tasks(&ns)?)).await?;
    let filtered: Vec<TaskRow> = rows
        .into_iter()
        .filter(|r| q.domain.as_ref().is_none_or(|d| &r.domain == d))
        .filter(|r| q.status.as_ref().is_none_or(|s| &r.status == s))
        .collect();
    let end = offset.saturating_add(limit).min(filtered.len());
    let items = filtered
        .get(offset.min(end)..end)
        .unwrap_or_default()
        .to_vec();
    let next_cursor = (end < filtered.len())
        .then(|| crate::db::cursor::encode(i64::try_from(end).unwrap_or(i64::MAX)));
    Ok(Json(Page { items, next_cursor }))
}

/// Everything known about one task.
#[utoipa::path(get, path = "/api/v1/experiments/{exp}/sweeps/{sweep}/tasks/{task}", tag = "results",
    params(("exp" = String, Path), ("sweep" = String, Path), ("task" = String, Path, description = "Task slug")),
    responses((status = 200, body = TaskDetail), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn task(
    _auth: Auth<ViewerRole>,
    State(state): State<AppState>,
    Path(p): Path<TaskPath>,
) -> Result<Json<TaskDetail>, ApiError> {
    let (rec, path) = locate(&state, &p.exp).await?;
    let ns = sweep_ns(&p.sweep)?;
    task_slug(&p.task)?;
    let kind = rec.kind;
    Ok(Json(
        blocking(move || Ok(open_dir(&path, kind)?.task(&ns, &p.task)?)).await?,
    ))
}

/// One harness tree with file contents.
#[utoipa::path(get, path = "/api/v1/experiments/{exp}/sweeps/{sweep}/tasks/{task}/modes/{mode}/harnesses/{label}", tag = "results",
    params(("exp" = String, Path), ("sweep" = String, Path), ("task" = String, Path),
        ("mode" = String, Path, description = "parallel | sequential (agg | seq)"), ("label" = String, Path)),
    responses((status = 200, body = HarnessTree), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn harness(
    _auth: Auth<ViewerRole>,
    State(state): State<AppState>,
    Path(p): Path<LabelPath>,
) -> Result<Json<HarnessTree>, ApiError> {
    let (rec, path) = locate(&state, &p.exp).await?;
    let ns = sweep_ns(&p.sweep)?;
    task_slug(&p.task)?;
    let m = mode("mode", &p.mode)?;
    label("label", &p.label)?;
    let kind = rec.kind;
    Ok(Json(
        blocking(move || Ok(open_dir(&path, kind)?.harness(&ns, &p.task, m, &p.label)?)).await?,
    ))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct DiffQuery {
    /// Harness label to diff from.
    pub from: String,
    /// Harness label to diff to.
    pub to: String,
}

/// File-by-file diff between two harness labels.
#[utoipa::path(get, path = "/api/v1/experiments/{exp}/sweeps/{sweep}/tasks/{task}/modes/{mode}/diff", tag = "results",
    params(("exp" = String, Path), ("sweep" = String, Path), ("task" = String, Path), ("mode" = String, Path), DiffQuery),
    responses((status = 200, body = HarnessDiff), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn diff(
    _auth: Auth<ViewerRole>,
    State(state): State<AppState>,
    Path(p): Path<ModePath>,
    ApiQuery(q): ApiQuery<DiffQuery>,
) -> Result<Json<HarnessDiff>, ApiError> {
    let (rec, path) = locate(&state, &p.exp).await?;
    let ns = sweep_ns(&p.sweep)?;
    task_slug(&p.task)?;
    let m = mode("mode", &p.mode)?;
    label("from", &q.from)?;
    label("to", &q.to)?;
    let kind = rec.kind;
    Ok(Json(
        blocking(move || Ok(open_dir(&path, kind)?.diff(&ns, &p.task, m, &q.from, &q.to)?)).await?,
    ))
}

/// Export format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
pub enum ExportFormat {
    #[serde(rename = "json")]
    Json,
    #[serde(rename = "tar.gz")]
    TarGz,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct ExportQuery {
    pub target: ExportTarget,
    pub format: ExportFormat,
}

/// A harness as project files: JSON, or a deterministic `.tar.gz` attachment.
#[utoipa::path(get, path = "/api/v1/experiments/{exp}/sweeps/{sweep}/tasks/{task}/modes/{mode}/harnesses/{label}/export", tag = "results",
    params(("exp" = String, Path), ("sweep" = String, Path), ("task" = String, Path), ("mode" = String, Path), ("label" = String, Path), ExportQuery),
    responses((status = 200, description = "format=json: the bundle; format=tar.gz: a gzip attachment",
            content((ExportBundle = "application/json"), ("application/gzip"))),
        (status = "default", description = "Error", body = ErrorBody)))]
pub async fn export(
    _auth: Auth<ViewerRole>,
    State(state): State<AppState>,
    Path(p): Path<LabelPath>,
    ApiQuery(q): ApiQuery<ExportQuery>,
) -> Result<Response, ApiError> {
    let (rec, path) = locate(&state, &p.exp).await?;
    let ns = sweep_ns(&p.sweep)?;
    task_slug(&p.task)?;
    let m = mode("mode", &p.mode)?;
    label("label", &p.label)?;
    let kind = rec.kind;
    let target = q.target;
    let lbl = p.label.clone();
    let bundle = blocking(move || {
        let tree = open_dir(&path, kind)?.harness(&ns, &p.task, m, &p.label)?;
        Ok(h2s_core::export::export(&tree, target)?)
    })
    .await?;
    match q.format {
        ExportFormat::Json => Ok(Json(bundle).into_response()),
        ExportFormat::TarGz => {
            let bytes = blocking(move || Ok(h2s_core::export::tar_gz(&bundle)?)).await?;
            let target_name = match target {
                ExportTarget::Raw => "raw",
                ExportTarget::Opencode => "opencode",
                ExportTarget::Codex => "codex",
            };
            let mut r = bytes.into_response();
            let h = r.headers_mut();
            h.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/gzip"),
            );
            if let Ok(v) = HeaderValue::from_str(&format!(
                "attachment; filename=\"harness-{lbl}-{target_name}.tar.gz\""
            )) {
                h.insert(header::CONTENT_DISPOSITION, v);
            }
            Ok(r)
        }
    }
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct FilesQuery {
    /// Relative path inside the rollout directory; omitted = its root.
    pub path: Option<String>,
}

/// List a rollout run directory.
#[utoipa::path(get, path = "/api/v1/experiments/{exp}/sweeps/{sweep}/tasks/{task}/modes/{mode}/rollouts/{label}/files", tag = "results",
    params(("exp" = String, Path), ("sweep" = String, Path), ("task" = String, Path), ("mode" = String, Path), ("label" = String, Path), FilesQuery),
    responses((status = 200, body = Vec<FileEntry>), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn rollout_files(
    _auth: Auth<ViewerRole>,
    State(state): State<AppState>,
    Path(p): Path<LabelPath>,
    ApiQuery(q): ApiQuery<FilesQuery>,
) -> Result<Json<Vec<FileEntry>>, ApiError> {
    let (rec, path) = locate(&state, &p.exp).await?;
    let ns = sweep_ns(&p.sweep)?;
    task_slug(&p.task)?;
    let m = mode("mode", &p.mode)?;
    label("label", &p.label)?;
    let sub = q.path.unwrap_or_default();
    check_name("path", h2s_core::names::relative_path(&sub))?;
    let kind = rec.kind;
    Ok(Json(
        blocking(
            move || Ok(open_dir(&path, kind)?.rollout_files(&ns, &p.task, m, &p.label, &sub)?),
        )
        .await?,
    ))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct FileQuery {
    /// Relative path of the file inside the rollout directory.
    pub path: String,
}

/// Read one rollout file (2 MiB cap).
#[utoipa::path(get, path = "/api/v1/experiments/{exp}/sweeps/{sweep}/tasks/{task}/modes/{mode}/rollouts/{label}/file", tag = "results",
    params(("exp" = String, Path), ("sweep" = String, Path), ("task" = String, Path), ("mode" = String, Path), ("label" = String, Path), FileQuery),
    responses((status = 200, body = FileContent), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn rollout_file(
    _auth: Auth<ViewerRole>,
    State(state): State<AppState>,
    Path(p): Path<LabelPath>,
    ApiQuery(q): ApiQuery<FileQuery>,
) -> Result<Json<FileContent>, ApiError> {
    let (rec, path) = locate(&state, &p.exp).await?;
    let ns = sweep_ns(&p.sweep)?;
    task_slug(&p.task)?;
    let m = mode("mode", &p.mode)?;
    label("label", &p.label)?;
    check_name("path", h2s_core::names::relative_path(&q.path))?;
    if q.path.is_empty() {
        return Err(ApiError::invalid("path", "name a file"));
    }
    let kind = rec.kind;
    Ok(Json(
        blocking(move || {
            Ok(open_dir(&path, kind)?.rollout_file(
                &ns,
                &p.task,
                m,
                &p.label,
                &q.path,
                DEFAULT_READ_CAP,
            )?)
        })
        .await?,
    ))
}

/// `judged.json` and `base_judged.json`.
#[utoipa::path(get, path = "/api/v1/experiments/{exp}/sweeps/{sweep}/scores", tag = "results",
    params(("exp" = String, Path), ("sweep" = String, Path)),
    responses((status = 200, body = Scores), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn scores(
    _auth: Auth<ViewerRole>,
    State(state): State<AppState>,
    Path(p): Path<SweepPath>,
) -> Result<Json<Scores>, ApiError> {
    let (rec, path) = locate(&state, &p.exp).await?;
    let ns = sweep_ns(&p.sweep)?;
    let kind = rec.kind;
    Ok(Json(
        blocking(move || Ok(open_dir(&path, kind)?.scores(&ns)?)).await?,
    ))
}

/// The score report computed from cached judge rows.
#[utoipa::path(get, path = "/api/v1/experiments/{exp}/sweeps/{sweep}/report", tag = "results",
    params(("exp" = String, Path), ("sweep" = String, Path)),
    responses((status = 200, body = ScoreReport), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn report(
    _auth: Auth<ViewerRole>,
    State(state): State<AppState>,
    Path(p): Path<SweepPath>,
) -> Result<Json<ScoreReport>, ApiError> {
    let (rec, path) = locate(&state, &p.exp).await?;
    let ns = sweep_ns(&p.sweep)?;
    let kind = rec.kind;
    Ok(Json(
        blocking(move || Ok(open_dir(&path, kind)?.report(&ns)?)).await?,
    ))
}

/// Experience libraries of a sweep.
#[utoipa::path(get, path = "/api/v1/experiments/{exp}/sweeps/{sweep}/library", tag = "results",
    params(("exp" = String, Path), ("sweep" = String, Path)),
    responses((status = 200, body = Vec<LibraryDomain>), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn library(
    _auth: Auth<ViewerRole>,
    State(state): State<AppState>,
    Path(p): Path<SweepPath>,
) -> Result<Json<Vec<LibraryDomain>>, ApiError> {
    let (rec, path) = locate(&state, &p.exp).await?;
    let ns = sweep_ns(&p.sweep)?;
    let kind = rec.kind;
    Ok(Json(
        blocking(move || Ok(open_dir(&path, kind)?.library(&ns)?)).await?,
    ))
}

/// Improver and selector sessions of a sweep.
#[utoipa::path(get, path = "/api/v1/experiments/{exp}/sweeps/{sweep}/audit", tag = "results",
    params(("exp" = String, Path), ("sweep" = String, Path)),
    responses((status = 200, body = Vec<AuditSession>), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn audit_sessions(
    _auth: Auth<ViewerRole>,
    State(state): State<AppState>,
    Path(p): Path<SweepPath>,
) -> Result<Json<Vec<AuditSession>>, ApiError> {
    let (rec, path) = locate(&state, &p.exp).await?;
    let ns = sweep_ns(&p.sweep)?;
    let kind = rec.kind;
    Ok(Json(
        blocking(move || Ok(open_dir(&path, kind)?.audit(&ns)?)).await?,
    ))
}

/// Audit session ids are file stems (`inv1.a1.3f9c0d2e11aa7b40`).
fn session_id_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 255
        && !s.starts_with('.')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// One text part (`mission`, `system`, `response`) of an audit session, as plain text.
#[utoipa::path(get, path = "/api/v1/experiments/{exp}/sweeps/{sweep}/audit/{session}/{part}", tag = "results",
    params(("exp" = String, Path), ("sweep" = String, Path), ("session" = String, Path),
        ("part" = String, Path, description = "mission | system | response")),
    responses((status = 200, description = "The text", content_type = "text/plain; charset=utf-8"),
        (status = "default", description = "Error", body = ErrorBody)))]
pub async fn audit_text(
    _auth: Auth<ViewerRole>,
    State(state): State<AppState>,
    Path(p): Path<AuditPath>,
) -> Result<Response, ApiError> {
    let (rec, path) = locate(&state, &p.exp).await?;
    let ns = sweep_ns(&p.sweep)?;
    if !session_id_ok(&p.session) {
        return Err(ApiError::invalid("session", "not a session id"));
    }
    if !matches!(p.part.as_str(), "mission" | "system" | "response") {
        return Err(ApiError::invalid(
            "part",
            "must be mission, system or response",
        ));
    }
    let kind = rec.kind;
    let text = blocking(move || {
        Ok(open_dir(&path, kind)?.audit_text(&ns, &p.session, &p.part, DEFAULT_READ_CAP)?)
    })
    .await?;
    let mut r = text.into_response();
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    Ok(r)
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct CompareQuery {
    /// `<experiment>/<sweep>/<mode>`
    pub a: String,
    /// `<experiment>/<sweep>/<mode>`
    pub b: String,
}

fn split_side(field: &str, s: &str) -> Result<(String, String, Mode), ApiError> {
    let parts: Vec<&str> = s.split('/').collect();
    let [exp, sweep, m] = parts.as_slice() else {
        return Err(ApiError::invalid(
            field,
            "must be <experiment>/<sweep>/<mode>",
        ));
    };
    Ok(((*exp).to_owned(), (*sweep).to_owned(), mode(field, m)?))
}

/// Per-task deltas between two (sweep, mode) results, with a sign test.
#[utoipa::path(get, path = "/api/v1/compare", tag = "results", params(CompareQuery),
    responses((status = 200, body = Comparison), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn compare(
    _auth: Auth<ViewerRole>,
    State(state): State<AppState>,
    ApiQuery(q): ApiQuery<CompareQuery>,
) -> Result<Json<Comparison>, ApiError> {
    let (ea, sa, ma) = split_side("a", &q.a)?;
    let (eb, sb, mb) = split_side("b", &q.b)?;
    let (ra, pa) = locate(&state, &ea).await?;
    let (rb, pb) = locate(&state, &eb).await?;
    let (na, nb) = (sweep_ns(&sa)?, sweep_ns(&sb)?);
    let (ka, kb) = (ra.kind, rb.kind);
    let (la, lb) = (q.a.clone(), q.b.clone());
    let cmp = blocking(move || {
        let da = open_dir(&pa, ka)?;
        let db = open_dir(&pb, kb)?;
        let scores_a = da.scores(&na)?;
        let scores_b = db.scores(&nb)?;
        let mut domains = BTreeMap::new();
        for r in da.tasks(&na)?.into_iter().chain(db.tasks(&nb)?) {
            domains.entry(r.task_id).or_insert(r.domain);
        }
        Ok(h2s_core::results::compare(
            &la, &scores_a, ma, &lb, &scores_b, mb, &domains,
        ))
    })
    .await?;
    Ok(Json(cmp))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_accept_both_spellings() {
        assert_eq!(mode("m", "parallel").unwrap(), Mode::Parallel);
        assert_eq!(mode("m", "agg").unwrap(), Mode::Parallel);
        assert_eq!(mode("m", "seq").unwrap(), Mode::Sequential);
        assert_eq!(mode("m", "both").unwrap_err().status.as_u16(), 422);
    }

    #[test]
    fn compare_sides() {
        let (e, s, m) = split_side("a", "demo/demo-oc-agg2/sequential").unwrap();
        assert_eq!(
            (e.as_str(), s.as_str(), m),
            ("demo", "demo-oc-agg2", Mode::Sequential)
        );
        assert!(split_side("a", "demo/x").is_err());
        assert!(split_side("a", "demo/x/y/z").is_err());
    }

    #[test]
    fn session_ids() {
        assert!(session_id_ok("inv1.a1.3f9c0d2e11aa7b40"));
        assert!(!session_id_ok("../x"));
        assert!(!session_id_ok(".x"));
        assert!(!session_id_ok("a/b"));
        assert!(!session_id_ok(""));
    }
}
