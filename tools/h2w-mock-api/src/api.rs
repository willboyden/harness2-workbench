//! The mock's HTTP surface: the §8 routes the UI uses, the security headers and checks the
//! real server applies (so UI mistakes show up in development), static files with the SPA
//! fallback, and job events as a real server-sent-events stream.

use std::collections::HashMap;
use std::convert::Infallible;
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use h2w_api::{
    ApiError, AuditOutcome, AuditVerification, Bench, Census, Comparison, CostEstimate,
    CreateExperimentRequest, CreateTokenRequest, ErrorBody, ErrorCode, EstimateRequest, Experiment,
    ExportTarget, FieldProblem, Health, Isolation, Job, JobEvent, JobSpec, JobStatus, KeyValue,
    LoginRequest, Manifest, ManifestJob, Mode, ModelProfile, Page, Principal, PrincipalKind,
    ProfileCheck, ProfileKind, Readiness, Role, SESSION_COOKIE, SetSecretRequest, Surface,
    SweepIdentity, SystemInfo, TaskDelta, TokenCreated, TokenInfo, UpdateExperimentRequest,
    ValidateHarnessRequest, ValidationReport,
};
use tokio_stream::wrappers::{BroadcastStream, IntervalStream};
use tokio_stream::{Stream, StreamExt};

use crate::sim::{self, Shared, lock};
use crate::state::{self, ExpData, Inner, JobData, TESTED_COMMIT};
use crate::sweeps;
use crate::util::{fake_hash, hex, now_secs, rfc3339, sign_test_p, tar_gz};

/// The Content-Security-Policy the real server sends (ARCHITECTURE.md §9), verbatim.
pub(crate) const CSP: &str = "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; img-src 'self' data:; font-src 'self'; connect-src 'self'; manifest-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) s: Shared,
    pub(crate) dist: PathBuf,
    pub(crate) csp_report: bool,
    pub(crate) task_ms: u64,
    /// Refuse the event stream (503) to exercise the UI's polling fallback.
    pub(crate) sse_off: bool,
}

pub(crate) struct Fail(StatusCode, ErrorBody);

impl IntoResponse for Fail {
    fn into_response(self) -> Response {
        (self.0, Json(self.1)).into_response()
    }
}

type R<T> = Result<T, Fail>;

fn fail(status: StatusCode, code: ErrorCode, message: &str) -> Fail {
    Fail(
        status,
        ErrorBody {
            error: ApiError {
                code,
                message: message.to_owned(),
                details: Vec::new(),
                request_id: Some(hex(8)),
            },
        },
    )
}

fn invalid(problems: Vec<(String, String)>) -> Fail {
    let message = problems
        .first()
        .map_or_else(|| "invalid input".to_owned(), |(f, p)| format!("{f}: {p}"));
    Fail(
        StatusCode::UNPROCESSABLE_ENTITY,
        ErrorBody {
            error: ApiError {
                code: ErrorCode::Validation,
                message,
                details: problems
                    .into_iter()
                    .map(|(field, problem)| FieldProblem { field, problem })
                    .collect(),
                request_id: Some(hex(8)),
            },
        },
    )
}

fn not_found(what: &str) -> Fail {
    fail(
        StatusCode::NOT_FOUND,
        ErrorCode::NotFound,
        &format!("{what} not found"),
    )
}

// ---------------------------------------------------------------------------------------
// Authentication

fn role_of_dev_token(token: &str) -> Role {
    if token.contains("viewer") {
        Role::Viewer
    } else if token.contains("operator") {
        Role::Operator
    } else {
        Role::Admin
    }
}

fn principal_for_token(g: &Inner, token: &str, kind: PrincipalKind) -> Option<Principal> {
    if let Some(rest) = token.strip_prefix("h2w_dev_") {
        let role = role_of_dev_token(rest);
        return Some(Principal {
            id: format!("dev-{}", role.as_str()),
            name: format!("dev {}", role.as_str()),
            role,
            kind,
        });
    }
    // Tokens created through the API in this session: `h2w_<id>_<secret>`.
    let id = token.strip_prefix("h2w_")?.split('_').next()?;
    let t = g.tokens.iter().find(|t| t.id == id && !t.revoked)?;
    Some(Principal {
        id: t.id.clone(),
        name: t.name.clone(),
        role: t.role,
        kind,
    })
}

fn session_cookie(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';').find_map(|kv| {
        let (k, v) = kv.trim().split_once('=')?;
        (k == SESSION_COOKIE).then(|| v.to_owned())
    })
}

fn principal(st: &AppState, headers: &HeaderMap) -> Option<Principal> {
    let g = lock(&st.s);
    if let Some(auth) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        && let Some(token) = auth.strip_prefix("Bearer ")
    {
        return principal_for_token(&g, token.trim(), PrincipalKind::Token);
    }
    let sid = session_cookie(headers)?;
    g.sessions.get(&sid).cloned()
}

fn require(st: &AppState, headers: &HeaderMap, role: Role) -> R<Principal> {
    let Some(p) = principal(st, headers) else {
        return Err(fail(
            StatusCode::UNAUTHORIZED,
            ErrorCode::Unauthorized,
            "sign in first",
        ));
    };
    if !p.role.allows(role) {
        lock(&st.s).add_audit(
            &p.name,
            "access.denied",
            role.as_str(),
            AuditOutcome::Denied,
            None,
        );
        return Err(fail(
            StatusCode::FORBIDDEN,
            ErrorCode::Forbidden,
            &format!(
                "this needs the {} role; you are {}",
                role.as_str(),
                p.role.as_str()
            ),
        ));
    }
    Ok(p)
}

async fn login(State(st): State<AppState>, Json(body): Json<LoginRequest>) -> Response {
    let found = {
        let g = lock(&st.s);
        principal_for_token(&g, body.token.trim(), PrincipalKind::Session)
    };
    let Some(p) = found else {
        lock(&st.s).add_audit(
            "anonymous",
            "auth.login",
            "unknown",
            AuditOutcome::Denied,
            Some("invalid token".into()),
        );
        return fail(
            StatusCode::UNAUTHORIZED,
            ErrorCode::Unauthorized,
            "invalid or revoked token",
        )
        .into_response();
    };
    let sid = hex(32);
    {
        let mut g = lock(&st.s);
        g.sessions.insert(sid.clone(), p.clone());
        g.add_audit(&p.name, "auth.login", &p.id, AuditOutcome::Success, None);
    }
    let cookie = format!("{SESSION_COOKIE}={sid}; HttpOnly; SameSite=Strict; Path=/");
    let mut resp = Json(p).into_response();
    if let Ok(v) = HeaderValue::from_str(&cookie) {
        resp.headers_mut().insert(header::SET_COOKIE, v);
    }
    resp
}

async fn logout(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(sid) = session_cookie(&headers) {
        lock(&st.s).sessions.remove(&sid);
    }
    let mut resp = StatusCode::NO_CONTENT.into_response();
    resp.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static("h2w_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0"),
    );
    resp
}

async fn me(State(st): State<AppState>, headers: HeaderMap) -> R<Json<Principal>> {
    require(&st, &headers, Role::Viewer).map(Json)
}

// ---------------------------------------------------------------------------------------
// System

async fn system_info(State(st): State<AppState>, headers: HeaderMap) -> R<Json<SystemInfo>> {
    let p = require(&st, &headers, Role::Viewer)?;
    let g = lock(&st.s);
    let admin = p.role == Role::Admin;
    let running = g
        .jobs
        .iter()
        .filter(|j| j.job.status == JobStatus::Running)
        .count();
    Ok(Json(SystemInfo {
        workbench_version: "0.1.0".to_owned(),
        build_commit: Some("0f3c9a1".to_owned()),
        harness2: h2w_api::Harness2Status {
            installed: g.installed,
            managed: true,
            commit: g.installed.then(|| TESTED_COMMIT.to_owned()),
            tested_commit: TESTED_COMMIT.to_owned(),
            version: g.installed.then(|| "0.4.1".to_owned()),
            python: g.installed.then(|| "3.12.11".to_owned()),
            extras: if g.installed {
                vec![h2w_api::InstallExtra::Render]
            } else {
                Vec::new()
            },
            home: admin.then(|| "/srv/harness2-workbench/harness2".to_owned()),
        },
        isolation: g.isolation,
        host_risks_preacknowledged: g.host_risks_preacknowledged,
        container_runtime: (g.isolation == Isolation::Container).then(|| "podman".to_owned()),
        auth_mode: h2w_api::AuthMode::Token,
        server_time: rfc3339(now_secs()),
        jobs_running: u32::try_from(running).unwrap_or(0),
        jobs_max_concurrent: 2,
        data_dir: admin.then(|| "/srv/harness2-workbench".to_owned()),
        features: vec!["otel".to_owned(), "container".to_owned()],
    }))
}

async fn doctor(
    State(st): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> R<Json<h2w_api::DoctorReport>> {
    let role = if q.get("refresh").is_some_and(|v| v == "true") {
        Role::Operator
    } else {
        Role::Viewer
    };
    require(&st, &headers, role)?;
    let installed = lock(&st.s).installed;
    let mut r = state::doctor(installed);
    if role == Role::Operator {
        r.generated_at = rfc3339(now_secs());
    }
    Ok(Json(r))
}

async fn benchmarks(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> R<Json<Vec<h2w_api::BenchmarkStatus>>> {
    require(&st, &headers, Role::Viewer)?;
    Ok(Json(lock(&st.s).benches.clone()))
}

async fn splits(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> R<Json<Vec<h2w_api::SplitInfo>>> {
    require(&st, &headers, Role::Viewer)?;
    Ok(Json(lock(&st.s).splits.clone()))
}

// ---------------------------------------------------------------------------------------
// Profiles, secrets, tokens, audit

fn valid_slug(s: &str) -> bool {
    let mut c = s.chars();
    c.next()
        .is_some_and(|f| f.is_ascii_lowercase() || f.is_ascii_digit())
        && s.len() <= 63
        && c.all(|x| x.is_ascii_lowercase() || x.is_ascii_digit() || x == '_' || x == '-')
}

fn valid_secret_name(s: &str) -> bool {
    let mut c = s.chars();
    c.next().is_some_and(|f| f.is_ascii_uppercase())
        && s.len() <= 64
        && c.all(|x| x.is_ascii_uppercase() || x.is_ascii_digit() || x == '_')
}

async fn profiles(State(st): State<AppState>, headers: HeaderMap) -> R<Json<Vec<ModelProfile>>> {
    require(&st, &headers, Role::Viewer)?;
    let g = lock(&st.s);
    Ok(Json(
        g.profiles.values().map(|p| g.profile_view(p)).collect(),
    ))
}

async fn put_profile(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Json(mut body): Json<ModelProfile>,
) -> R<Json<ModelProfile>> {
    let p = require(&st, &headers, Role::Admin)?;
    if !valid_slug(&name) {
        return Err(invalid(vec![(
            "name".into(),
            "use lowercase letters, digits, - and _".into(),
        )]));
    }
    if body.name != name {
        return Err(invalid(vec![(
            "name".into(),
            "must match the name in the URL".into(),
        )]));
    }
    if body.solver_model.trim().is_empty() {
        return Err(invalid(vec![(
            "solver_model".into(),
            "enter a model id".into(),
        )]));
    }
    body.secrets.clear();
    let mut g = lock(&st.s);
    g.profiles.insert(name.clone(), body.clone());
    g.add_audit(&p.name, "profile.put", &name, AuditOutcome::Success, None);
    let view = g.profile_view(&body);
    Ok(Json(view))
}

/// Server rule for profile names (`h2w_core::names::profile_name`), as the server words it.
const PROFILE_NAME_RULE: &str =
    "use 1-63 lowercase letters, digits, '_' or '-', starting with a letter or digit";

/// `GET /profiles/{name}/check` (addendum v1.2), answering like the server: viewer role, 422
/// for a malformed name, 404 "no profile named <name>" for an unknown one.
async fn check_profile(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> R<Json<ProfileCheck>> {
    require(&st, &headers, Role::Viewer)?;
    if !valid_slug(&name) {
        return Err(invalid(vec![("name".into(), PROFILE_NAME_RULE.into())]));
    }
    let g = lock(&st.s);
    let Some(p) = g.profiles.get(&name) else {
        return Err(fail(
            StatusCode::NOT_FOUND,
            ErrorCode::NotFound,
            &format!("no profile named {name}"),
        ));
    };
    let missing_secrets = referenced_secrets(p)
        .into_iter()
        .filter(|n| !g.secrets.get(n).is_some_and(|s| s.set))
        .collect();
    Ok(Json(ProfileCheck {
        warnings: profile_warnings(p),
        missing_secrets,
    }))
}

/// Secrets a profile references, in the server's order (endpoints, Vertex credentials, extra
/// environment), without duplicates.
fn referenced_secrets(p: &ModelProfile) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let names = [&p.endpoint, &p.judge_endpoint, &p.codex_endpoint]
        .into_iter()
        .flatten()
        .filter_map(|e| e.api_key_secret.clone())
        .chain(p.vertex.as_ref().and_then(|v| v.credentials_secret.clone()))
        .chain(p.extra_env.iter().filter_map(|e| e.secret.clone()));
    for n in names {
        if !out.contains(&n) {
            out.push(n);
        }
    }
    out
}

/// The main cases of the server's `h2w_core::env::profile_warnings`, in its words (the server
/// derives them from the full environment plan; this is close enough to exercise the UI).
fn profile_warnings(p: &ModelProfile) -> Vec<String> {
    let env = |name: &str| p.extra_env.iter().any(|e| e.name == name);
    let mut w = Vec::new();
    let solver = p.solver_model.trim();
    if solver.is_empty() {
        w.push(
            "No executor model: harness2 refuses to run without HARNESS2_SOLVER_MODEL.".to_owned(),
        );
    } else if p.kind != ProfileKind::OpenaiCompatible && !solver.contains('/') {
        w.push(format!(
            "The executor model {solver:?} is not in OpenCode's provider/model form, which \
             harness2 requires."
        ));
    }
    let judge = p
        .judge_model
        .as_deref()
        .map(str::trim)
        .filter(|j| !j.is_empty());
    match judge {
        None => w.push(
            "No judge model: harness2 starts no sweep and judges nothing without \
             HARNESS2_JUDGE_MODEL."
                .to_owned(),
        ),
        Some(j) => {
            const LAB_JUDGES: [&str; 9] = [
                "claude", "gemini", "gpt", "o1", "o3", "o4", "o5", "mistral", "vertex/",
            ];
            if !LAB_JUDGES.iter().any(|x| j.starts_with(x)) {
                w.push(format!(
                    "LAB's judge does not recognise the judge model {j:?} (it accepts ids \
                     starting with claude, gemini, gpt, o1, o3, o4, o5, mistral or vertex/), so \
                     this profile cannot judge LAB."
                ));
            }
            if p.judge_endpoint.is_none() && p.vertex.is_none() && !env("HARNESS2_JUDGE_BASE_URL") {
                w.push(
                    "JobBench judging needs a judge endpoint or Vertex AI settings (harness2 \
                     reads HARNESS2_JUDGE_BASE_URL or GOOGLE_CLOUD_PROJECT)."
                        .to_owned(),
                );
            }
            let judge_key = p
                .judge_endpoint
                .as_ref()
                .is_some_and(|e| e.api_key_secret.is_some())
                || env("HARNESS2_JUDGE_API_KEY");
            if !j.starts_with("gemini") && !judge_key {
                w.push(
                    "JobBench needs a judge endpoint with an API key for a non-Gemini judge \
                     (HARNESS2_JUDGE_BASE_URL and HARNESS2_JUDGE_API_KEY)."
                        .to_owned(),
                );
            }
        }
    }
    let wb_ok = solver.starts_with("google-vertex/gemini")
        && judge.is_some_and(|j| j.starts_with("gemini"))
        && p.vertex.is_some();
    if !wb_ok {
        w.push(
            "WorkBuddy-Bench cannot use this profile: its setup requires a google-vertex/gemini \
             executor, a gemini judge and Vertex AI settings."
                .to_owned(),
        );
    }
    if p.codex_endpoint.is_none() && !env("HARNESS2_CODEX_BASE_URL") {
        w.push(
            "The Codex executor (substrate cx) needs a Responses-API endpoint: set the \
             profile's Codex endpoint (for Vertex AI, run the bridge job and use its URL and \
             key)."
                .to_owned(),
        );
    }
    w
}

async fn delete_profile(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> R<StatusCode> {
    let p = require(&st, &headers, Role::Admin)?;
    let mut g = lock(&st.s);
    if let Some(e) = g
        .experiments
        .values()
        .find(|e| e.exp.profile.as_deref() == Some(name.as_str()))
    {
        return Err(fail(
            StatusCode::CONFLICT,
            ErrorCode::Conflict,
            &format!("experiment {} is pinned to this profile", e.exp.name),
        ));
    }
    if g.profiles.remove(&name).is_none() {
        return Err(not_found("profile"));
    }
    g.add_audit(
        &p.name,
        "profile.delete",
        &name,
        AuditOutcome::Success,
        None,
    );
    Ok(StatusCode::NO_CONTENT)
}

async fn secrets(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> R<Json<Vec<h2w_api::SecretInfo>>> {
    require(&st, &headers, Role::Admin)?;
    let g = lock(&st.s);
    let mut all: Vec<h2w_api::SecretInfo> = g.secrets.values().cloned().collect();
    for p in g.profiles.values() {
        for s in g.profile_view(p).secrets {
            if !all.iter().any(|x| x.name == s.name) {
                all.push(s);
            }
        }
    }
    all.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Json(all))
}

async fn put_secret(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Json(body): Json<SetSecretRequest>,
) -> R<Json<h2w_api::SecretInfo>> {
    let p = require(&st, &headers, Role::Admin)?;
    if !valid_secret_name(&name) {
        return Err(invalid(vec![(
            "name".into(),
            "uppercase letters, digits and _, starting with a letter".into(),
        )]));
    }
    if body.value.is_empty() {
        return Err(invalid(vec![("value".into(), "the value is empty".into())]));
    }
    // The mock keeps only metadata; the value is dropped right here.
    let info = h2w_api::SecretInfo {
        name: name.clone(),
        set: true,
        chars: u32::try_from(body.value.chars().count()).ok(),
        updated_at: Some(rfc3339(now_secs())),
    };
    let mut g = lock(&st.s);
    g.secrets.insert(name.clone(), info.clone());
    g.add_audit(&p.name, "secret.set", &name, AuditOutcome::Success, None);
    Ok(Json(info))
}

async fn delete_secret(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> R<StatusCode> {
    let p = require(&st, &headers, Role::Admin)?;
    let mut g = lock(&st.s);
    if g.secrets.remove(&name).is_none() {
        return Err(not_found("secret"));
    }
    g.add_audit(&p.name, "secret.delete", &name, AuditOutcome::Success, None);
    Ok(StatusCode::NO_CONTENT)
}

async fn tokens(State(st): State<AppState>, headers: HeaderMap) -> R<Json<Vec<TokenInfo>>> {
    require(&st, &headers, Role::Admin)?;
    Ok(Json(lock(&st.s).tokens.clone()))
}

async fn create_token(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateTokenRequest>,
) -> R<(StatusCode, Json<TokenCreated>)> {
    let p = require(&st, &headers, Role::Admin)?;
    let name = body.name.trim();
    if name.is_empty()
        || name.len() > 64
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '_' | '-' | '@'))
    {
        return Err(invalid(vec![(
            "name".into(),
            "1 to 64 letters, digits, spaces and . _ - @".into(),
        )]));
    }
    let id = hex(3);
    let info = TokenInfo {
        id: id.clone(),
        name: name.to_owned(),
        role: body.role,
        created_at: rfc3339(now_secs()),
        created_by: p.name.clone(),
        last_used_at: None,
        expires_at: body
            .expires_in_days
            .map(|d| rfc3339(now_secs() + i64::from(d) * 86_400)),
        revoked: false,
    };
    let token = format!("h2w_{id}_{}", hex(24));
    let mut g = lock(&st.s);
    g.tokens.push(info.clone());
    g.add_audit(
        &p.name,
        "token.create",
        &id,
        AuditOutcome::Success,
        Some(format!("role={}", body.role.as_str())),
    );
    Ok((StatusCode::CREATED, Json(TokenCreated { token, info })))
}

async fn revoke_token(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> R<StatusCode> {
    let p = require(&st, &headers, Role::Admin)?;
    let mut g = lock(&st.s);
    let Some(t) = g.tokens.iter_mut().find(|t| t.id == id) else {
        return Err(not_found("token"));
    };
    t.revoked = true;
    let revoked_name = t.name.clone();
    g.sessions.retain(|_, s| s.id != id);
    g.add_audit(
        &p.name,
        "token.revoke",
        &id,
        AuditOutcome::Success,
        Some(revoked_name),
    );
    Ok(StatusCode::NO_CONTENT)
}

fn paging(q: &HashMap<String, String>, default: usize, max: usize) -> (usize, usize) {
    let limit = q
        .get("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
        .clamp(1, max);
    let offset = q.get("cursor").and_then(|v| v.parse().ok()).unwrap_or(0);
    (offset, limit)
}

fn page<T>(items: Vec<T>, offset: usize, limit: usize) -> Page<T> {
    let total = items.len();
    let next_cursor = (offset + limit < total).then(|| (offset + limit).to_string());
    Page {
        items: items.into_iter().skip(offset).take(limit).collect(),
        next_cursor,
    }
}

async fn audit_log(
    State(st): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> R<Json<Page<h2w_api::AuditEvent>>> {
    require(&st, &headers, Role::Admin)?;
    let (offset, limit) = paging(&q, 50, 500);
    let mut items = lock(&st.s).audit.clone();
    items.reverse();
    Ok(Json(page(items, offset, limit)))
}

async fn audit_verify(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> R<Json<AuditVerification>> {
    require(&st, &headers, Role::Admin)?;
    let g = lock(&st.s);
    let first_bad = g
        .audit
        .windows(2)
        .find(|w| w[1].prev_hash != w[0].hash)
        .map(|w| w[1].seq);
    Ok(Json(AuditVerification {
        ok: first_bad.is_none(),
        entries: g.audit.len() as u64,
        first_bad_seq: first_bad,
    }))
}

// ---------------------------------------------------------------------------------------
// Experiments and results

async fn experiments(
    State(st): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> R<Json<Vec<Experiment>>> {
    require(&st, &headers, Role::Viewer)?;
    let archived = q.get("archived").is_some_and(|v| v == "true");
    let g = lock(&st.s);
    Ok(Json(
        g.experiments
            .values()
            .filter(|e| archived || !e.exp.archived)
            .map(ExpData::view)
            .collect(),
    ))
}

async fn create_experiment(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateExperimentRequest>,
) -> R<(StatusCode, Json<Experiment>)> {
    let p = require(&st, &headers, Role::Operator)?;
    if !valid_slug(&body.name) {
        return Err(invalid(vec![("name".into(), "start with a lowercase letter or digit; then lowercase letters, digits, - and _ (at most 63)".into())]));
    }
    let mut g = lock(&st.s);
    if g.experiments.contains_key(&body.name) {
        return Err(fail(
            StatusCode::CONFLICT,
            ErrorCode::Conflict,
            &format!("experiment {} already exists", body.name),
        ));
    }
    if let Some(prof) = &body.profile
        && !g.profiles.contains_key(prof)
    {
        return Err(invalid(vec![(
            "profile".into(),
            format!("no profile named {prof}"),
        )]));
    }
    let exp = Experiment {
        name: body.name.clone(),
        kind: body.kind,
        description: body.description,
        tags: body.tags,
        profile: body.profile,
        created_at: rfc3339(now_secs()),
        created_by: p.name.clone(),
        archived: false,
        sweeps: Vec::new(),
        size_bytes: Some(0),
    };
    g.experiments.insert(
        body.name.clone(),
        ExpData {
            exp: exp.clone(),
            sweeps: Vec::new(),
        },
    );
    g.add_audit(
        &p.name,
        "experiment.create",
        &body.name,
        AuditOutcome::Success,
        None,
    );
    Ok((StatusCode::CREATED, Json(exp)))
}

async fn get_experiment(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(exp): Path<String>,
) -> R<Json<Experiment>> {
    require(&st, &headers, Role::Viewer)?;
    let g = lock(&st.s);
    g.experiments
        .get(&exp)
        .map(|e| Json(e.view()))
        .ok_or_else(|| not_found("experiment"))
}

async fn patch_experiment(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(exp): Path<String>,
    Json(body): Json<UpdateExperimentRequest>,
) -> R<Json<Experiment>> {
    let p = require(&st, &headers, Role::Operator)?;
    let mut g = lock(&st.s);
    let e = g
        .experiments
        .get_mut(&exp)
        .ok_or_else(|| not_found("experiment"))?;
    if let Some(d) = body.description {
        e.exp.description = d;
    }
    if let Some(t) = body.tags {
        e.exp.tags = t;
    }
    if let Some(a) = body.archived {
        e.exp.archived = a;
    }
    let view = e.view();
    g.add_audit(
        &p.name,
        "experiment.update",
        &exp,
        AuditOutcome::Success,
        None,
    );
    Ok(Json(view))
}

async fn manifest(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(exp): Path<String>,
) -> R<Json<Manifest>> {
    require(&st, &headers, Role::Viewer)?;
    let g = lock(&st.s);
    let e = g
        .experiments
        .get(&exp)
        .ok_or_else(|| not_found("experiment"))?;
    let profile = e.exp.profile.as_ref().and_then(|p| g.profiles.get(p));
    let mut models = Vec::new();
    if let Some(p) = profile {
        models.push(KeyValue {
            key: "HARNESS2_SOLVER_MODEL".into(),
            value: p.solver_model.clone(),
        });
        if let Some(m) = &p.improver_model {
            models.push(KeyValue {
                key: "HARNESS2_IMPROVER_MODEL".into(),
                value: m.clone(),
            });
        }
        if let Some(m) = &p.judge_model {
            models.push(KeyValue {
                key: "HARNESS2_JUDGE_MODEL".into(),
                value: m.clone(),
            });
        }
    }
    let jobs: Vec<ManifestJob> = g
        .jobs
        .iter()
        .filter(|j| j.job.spec.experiment() == Some(exp.as_str()))
        .map(|j| ManifestJob {
            id: j.job.id.clone(),
            kind: j.job.kind,
            status: j.job.status,
            argv: argv(&j.job.spec),
            created_at: j.job.created_at.clone(),
            finished_at: j.job.finished_at.clone(),
        })
        .collect();
    Ok(Json(Manifest {
        schema_version: 1,
        generated_at: rfc3339(now_secs()),
        workbench_version: "0.1.0".into(),
        experiment: exp.clone(),
        harness2_commit: Some(TESTED_COMMIT.into()),
        dependency_pins: vec![
            KeyValue { key: "job-bench-eval".into(), value: "fa03a17f9ff662f136798a274e10011c3aa00fd0".into() },
            KeyValue { key: "harvey-labs".into(), value: "b58a28ae9fde5ee4b810e53b4fd0a1915325a35d".into() },
            KeyValue { key: "opencode".into(), value: "23fb5e0516c99ac04a1aa46c193efda2e1b9bb24".into() },
            KeyValue { key: "codex".into(), value: "0.144.6".into() },
        ],
        models,
        env_names: vec!["HARNESS2_RUNS_DIR".into(), "HARNESS2_SOLVER_MODEL".into(), "HARNESS2_IMPROVER_MODEL".into(), "HARNESS2_JUDGE_MODEL".into(), "OPENCODE_CONFIG".into()],
        isolation: g.isolation,
        network_policy: "host: the executor has the server user's network access".into(),
        jobs,
        sweep_identities: e
            .sweeps
            .iter()
            .map(|s| SweepIdentity {
                sweep: s.id.clone(),
                identity_json: format!(
                    "{{\"bench\": \"{}\", \"k\": {}, \"seed\": {}, \"modes\": {:?}, \"solver\": \"{}\", \"improver\": \"{}\", \"tasks\": {}}}",
                    s.bench_name,
                    s.k,
                    s.seed.map_or_else(|| "null".to_owned(), |x| x.to_string()),
                    s.modes.iter().map(|m| m.as_cli()).collect::<Vec<_>>(),
                    s.models.0,
                    s.models.1,
                    s.tasks.len()
                ),
            })
            .collect(),
        results_digest: fake_hash(&exp),
        jobbench_revision: e.sweeps.iter().any(|s| s.bench_name == "jb").then(|| "2026-08-14".into()),
    }))
}

fn argv(spec: &JobSpec) -> Vec<String> {
    let mut v: Vec<String> = match spec {
        JobSpec::InstallHarness2(_) => vec![
            "uv".into(),
            "pip".into(),
            "install".into(),
            "-e".into(),
            ".[render]".into(),
        ],
        JobSpec::Demo(p) => vec![
            "harness2-demo".into(),
            "--k".into(),
            p.k.to_string(),
            "--mode".into(),
            p.mode.as_cli().into(),
            "--output".into(),
            "<experiment>".into(),
        ],
        JobSpec::Setup(p) => vec!["harness2-setup".into(), p.bench.as_cli().into()],
        JobSpec::Check(p) => vec![
            "harness2-check".into(),
            "--bench".into(),
            p.cell.bench.as_cli().into(),
        ],
        JobSpec::Preview(p) => vec![
            "harness2".into(),
            "--bench".into(),
            p.cell.bench.as_cli().into(),
            "--k".into(),
            p.options.k.to_string(),
            "--modes".into(),
            p.options.modes_cli(),
            "--dry-run".into(),
        ],
        JobSpec::Run(p) => vec![
            "harness2".into(),
            "--bench".into(),
            p.cell.bench.as_cli().into(),
            "--k".into(),
            p.options.k.to_string(),
            "--modes".into(),
            p.options.modes_cli(),
        ],
        JobSpec::Judge(p) => vec![
            "harness2-judge".into(),
            "--bench".into(),
            p.cell.bench.as_cli().into(),
            "--k".into(),
            p.k.to_string(),
        ],
        JobSpec::Report(p) => vec![
            "harness2-report".into(),
            "--bench".into(),
            p.cell.bench.as_cli().into(),
        ],
        JobSpec::Split(p) => vec![
            "python".into(),
            "scripts/split_tasks.py".into(),
            p.bench.as_cli().into(),
        ],
        JobSpec::Bridge(_) => vec!["harness2-bridge".into()],
        JobSpec::Pipeline(p) => vec![
            "harness2".into(),
            "--bench".into(),
            p.cell.bench.as_cli().into(),
            "--k".into(),
            p.options.k.to_string(),
            "--modes".into(),
            p.options.modes_cli(),
        ],
    };
    let seed = match spec {
        JobSpec::Run(p) => p.options.seed,
        JobSpec::Pipeline(p) => p.options.seed,
        JobSpec::Preview(p) => p.options.seed,
        _ => None,
    };
    if let Some(seed) = seed {
        v.push("--seed".into());
        v.push(seed.to_string());
    }
    v
}

fn with_sweep<T>(
    st: &AppState,
    exp: &str,
    sweep: &str,
    f: impl FnOnce(&sweeps::SweepSpec) -> R<T>,
) -> R<T> {
    let g = lock(&st.s);
    let e = g
        .experiments
        .get(exp)
        .ok_or_else(|| not_found("experiment"))?;
    let s = e.sweep(sweep).ok_or_else(|| not_found("sweep"))?;
    f(s)
}

fn mode_param(m: &str) -> R<Mode> {
    Mode::from_token(m).ok_or_else(|| {
        fail(
            StatusCode::BAD_REQUEST,
            ErrorCode::BadRequest,
            "mode must be parallel or sequential",
        )
    })
}

async fn tasks(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path((exp, sweep)): Path<(String, String)>,
    Query(q): Query<HashMap<String, String>>,
) -> R<Json<Page<h2w_api::TaskRow>>> {
    require(&st, &headers, Role::Viewer)?;
    let (offset, limit) = paging(&q, 50, 200);
    with_sweep(&st, &exp, &sweep, |s| {
        let rows: Vec<h2w_api::TaskRow> = sweeps::task_rows(s)
            .into_iter()
            .filter(|r| {
                q.get("domain")
                    .is_none_or(|d| d.is_empty() || *d == r.domain)
            })
            .filter(|r| {
                q.get("status")
                    .is_none_or(|d| d.is_empty() || *d == r.status)
            })
            .collect();
        Ok(Json(page(rows, offset, limit)))
    })
}

async fn task(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path((exp, sweep, task)): Path<(String, String, String)>,
) -> R<Json<h2w_api::TaskDetail>> {
    require(&st, &headers, Role::Viewer)?;
    with_sweep(&st, &exp, &sweep, |s| {
        let t = s.task(&task).ok_or_else(|| not_found("task"))?;
        Ok(Json(sweeps::task_detail(s, t)))
    })
}

async fn harness(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path((exp, sweep, task, mode, label)): Path<(String, String, String, String, String)>,
) -> R<Json<h2w_api::HarnessTree>> {
    require(&st, &headers, Role::Viewer)?;
    let mode = mode_param(&mode)?;
    with_sweep(&st, &exp, &sweep, |s| {
        let t = s.task(&task).ok_or_else(|| not_found("task"))?;
        sweeps::harness_tree(s, t, mode, &label)
            .map(Json)
            .ok_or_else(|| not_found("harness"))
    })
}

async fn diff(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path((exp, sweep, task, mode)): Path<(String, String, String, String)>,
    Query(q): Query<HashMap<String, String>>,
) -> R<Json<h2w_api::HarnessDiff>> {
    require(&st, &headers, Role::Viewer)?;
    let mode = mode_param(&mode)?;
    let from = q.get("from").cloned().unwrap_or_else(|| "base".to_owned());
    let Some(to) = q.get("to").cloned() else {
        return Err(invalid(vec![(
            "to".into(),
            "choose the harness to compare with".into(),
        )]));
    };
    with_sweep(&st, &exp, &sweep, |s| {
        let t = s.task(&task).ok_or_else(|| not_found("task"))?;
        sweeps::harness_diff(s, t, mode, &from, &to)
            .map(Json)
            .ok_or_else(|| not_found("harness"))
    })
}

async fn export(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path((exp, sweep, task, mode, label)): Path<(String, String, String, String, String)>,
    Query(q): Query<HashMap<String, String>>,
) -> R<Response> {
    require(&st, &headers, Role::Viewer)?;
    let mode = mode_param(&mode)?;
    let target = match q.get("target").map_or("raw", String::as_str) {
        "raw" => ExportTarget::Raw,
        "opencode" => ExportTarget::Opencode,
        "codex" => ExportTarget::Codex,
        _ => {
            return Err(invalid(vec![(
                "target".into(),
                "raw, opencode or codex".into(),
            )]));
        }
    };
    let tar = q.get("format").is_some_and(|f| f == "tar.gz");
    with_sweep(&st, &exp, &sweep, |s| {
        let t = s.task(&task).ok_or_else(|| not_found("task"))?;
        let bundle =
            sweeps::export(s, t, mode, &label, target).ok_or_else(|| not_found("harness"))?;
        if !tar {
            return Ok(Json(bundle).into_response());
        }
        let files: Vec<(String, String)> = bundle
            .files
            .into_iter()
            .map(|f| (f.path, f.content))
            .collect();
        let body = tar_gz(&files);
        let name = format!("{}-{}-{label}.tar.gz", task, mode.token());
        Ok((
            [
                (header::CONTENT_TYPE, "application/gzip".to_owned()),
                (
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{name}\""),
                ),
            ],
            body,
        )
            .into_response())
    })
}

fn safe_rel(path: &str) -> bool {
    !path.starts_with('/')
        && !path.contains('\\')
        && !path.split('/').any(|s| s == "..")
        && path.len() <= 1024
}

async fn rollout_files(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path((exp, sweep, task, mode, label)): Path<(String, String, String, String, String)>,
    Query(q): Query<HashMap<String, String>>,
) -> R<Json<Vec<h2w_api::FileEntry>>> {
    require(&st, &headers, Role::Viewer)?;
    let _ = mode_param(&mode)?;
    let path = q.get("path").cloned().unwrap_or_default();
    if !safe_rel(&path) {
        return Err(invalid(vec![(
            "path".into(),
            "relative paths inside the rollout only".into(),
        )]));
    }
    with_sweep(&st, &exp, &sweep, |s| {
        s.task(&task).ok_or_else(|| not_found("task"))?;
        let _ = label;
        sweeps::rollout_files(&path)
            .map(Json)
            .ok_or_else(|| not_found("directory"))
    })
}

async fn rollout_file(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path((exp, sweep, task, mode, label)): Path<(String, String, String, String, String)>,
    Query(q): Query<HashMap<String, String>>,
) -> R<Json<h2w_api::FileContent>> {
    require(&st, &headers, Role::Viewer)?;
    let _ = mode_param(&mode)?;
    let path = q.get("path").cloned().unwrap_or_default();
    if !safe_rel(&path) {
        return Err(invalid(vec![(
            "path".into(),
            "relative paths inside the rollout only".into(),
        )]));
    }
    with_sweep(&st, &exp, &sweep, |s| {
        let t = s.task(&task).ok_or_else(|| not_found("task"))?;
        sweeps::rollout_file(t, &label, &path)
            .map(Json)
            .ok_or_else(|| not_found("file"))
    })
}

async fn scores(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path((exp, sweep)): Path<(String, String)>,
) -> R<Json<h2w_api::Scores>> {
    require(&st, &headers, Role::Viewer)?;
    with_sweep(&st, &exp, &sweep, |s| Ok(Json(sweeps::scores(s))))
}

async fn report(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path((exp, sweep)): Path<(String, String)>,
) -> R<Json<h2w_api::ScoreReport>> {
    require(&st, &headers, Role::Viewer)?;
    with_sweep(&st, &exp, &sweep, |s| Ok(Json(sweeps::report(s))))
}

async fn library(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path((exp, sweep)): Path<(String, String)>,
) -> R<Json<Vec<h2w_api::LibraryDomain>>> {
    require(&st, &headers, Role::Viewer)?;
    with_sweep(&st, &exp, &sweep, |s| Ok(Json(sweeps::library(s))))
}

async fn sweep_audit(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path((exp, sweep)): Path<(String, String)>,
) -> R<Json<Vec<h2w_api::AuditSession>>> {
    require(&st, &headers, Role::Viewer)?;
    with_sweep(&st, &exp, &sweep, |s| Ok(Json(sweeps::audit_sessions(s))))
}

async fn sweep_audit_part(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path((exp, sweep, session, part)): Path<(String, String, String, String)>,
) -> R<Response> {
    require(&st, &headers, Role::Viewer)?;
    with_sweep(&st, &exp, &sweep, |s| {
        let text =
            sweeps::audit_part(s, &session, &part).ok_or_else(|| not_found("session part"))?;
        Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], text).into_response())
    })
}

async fn compare(
    State(st): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> R<Json<Comparison>> {
    require(&st, &headers, Role::Viewer)?;
    let side = |key: &str| -> R<(String, String, Mode)> {
        let v = q
            .get(key)
            .ok_or_else(|| invalid(vec![(key.to_owned(), "pick a sweep and mode".into())]))?;
        let mut p = v.split('/');
        match (p.next(), p.next(), p.next().and_then(Mode::from_token)) {
            (Some(e), Some(s), Some(m)) => Ok((e.to_owned(), s.to_owned(), m)),
            _ => Err(invalid(vec![(
                key.to_owned(),
                "use experiment/sweep/mode".into(),
            )])),
        }
    };
    let (ae, asw, am) = side("a")?;
    let (be, bsw, bm) = side("b")?;
    let g = lock(&st.s);
    let get = |e: &str, s: &str| -> R<sweeps::SweepSpec> {
        g.experiments
            .get(e)
            .and_then(|x| x.sweep(s))
            .cloned()
            .ok_or_else(|| not_found("sweep"))
    };
    let (sa, sb) = (get(&ae, &asw)?, get(&be, &bsw)?);
    let (ra, rb) = (sweeps::scores(&sa), sweeps::scores(&sb));
    let frac = |rows: &[h2w_api::JudgedRow], m: Mode, id: &str| {
        rows.iter()
            .find(|r| r.mode == m && r.task_id == id && r.graded)
            .and_then(|r| r.frac)
    };
    let mut ids: Vec<(String, String)> = sa
        .tasks
        .iter()
        .chain(sb.tasks.iter())
        .map(|t| (t.id.clone(), t.domain.clone()))
        .collect();
    ids.sort();
    ids.dedup();
    let mut tasks = Vec::new();
    let (mut wins, mut losses, mut ties) = (0, 0, 0);
    let (mut sum_a, mut sum_b, mut n) = (0.0, 0.0, 0_u32);
    for (id, domain) in ids {
        let (a, b) = (frac(&ra.rows, am, &id), frac(&rb.rows, bm, &id));
        let delta = a.zip(b).map(|(a, b)| b - a);
        if let (Some(a), Some(b)) = (a, b) {
            n += 1;
            sum_a += a;
            sum_b += b;
            if (b - a).abs() < 1e-9 {
                ties += 1;
            } else if b > a {
                wins += 1;
            } else {
                losses += 1;
            }
        }
        tasks.push(TaskDelta {
            task_id: id,
            domain,
            a,
            b,
            delta,
        });
    }
    let nf = f64::from(n);
    Ok(Json(Comparison {
        a: format!("{ae}/{asw}/{}", am.as_cli()),
        b: format!("{be}/{bsw}/{}", bm.as_cli()),
        tasks,
        n_common: n,
        mean_a: (n > 0).then(|| sum_a / nf),
        mean_b: (n > 0).then(|| sum_b / nf),
        mean_delta: (n > 0).then(|| (sum_b - sum_a) / nf),
        wins,
        losses,
        ties,
        sign_test_p: sign_test_p(wins, losses),
    }))
}

async fn estimate(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<EstimateRequest>,
) -> R<Json<CostEstimate>> {
    require(&st, &headers, Role::Viewer)?;
    if body.k == 0 || body.k > h2w_api::K_MAX || body.modes.is_empty() {
        return Err(invalid(vec![(
            "k".into(),
            "k must be 1..32 and at least one mode chosen".into(),
        )]));
    }
    let g = lock(&st.s);
    let prices = body
        .profile
        .as_ref()
        .and_then(|p| g.profiles.get(p))
        .and_then(|p| p.prices.clone());
    let mut est = sim::estimate(body.tasks, body.k, &body.modes, prices.as_ref());
    if let Some(e) = body.experiment.as_ref().and_then(|e| g.experiments.get(e)) {
        let history: Vec<(f64, u32)> = e
            .sweeps
            .iter()
            .map(|s| {
                (
                    sweeps::summary(s).usd,
                    u32::try_from(s.tasks.len() * s.modes.len()).unwrap_or(0),
                )
            })
            .filter(|(_, n)| *n > 0)
            .collect();
        let (usd, n): (f64, u32) = history
            .iter()
            .fold((0.0, 0), |(u, k), (a, b)| (u + a, k + b));
        if n > 0 {
            let per = usd / f64::from(n);
            let units = f64::from(body.tasks) * body.modes.len() as f64 * f64::from(body.k);
            est.usd_low = Some(per * units * 0.8);
            est.usd_high = Some(per * units * 1.3);
            est.basis = "history".to_owned();
            est.notes.push(format!("Based on {n} earlier task-mode records in this experiment (improver and selection spend only)."));
        }
    }
    Ok(Json(est))
}

async fn validate_harness(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ValidateHarnessRequest>,
) -> R<Json<ValidationReport>> {
    require(&st, &headers, Role::Viewer)?;
    let cap = match body.surface {
        Surface::Opencode => 24_000,
        Surface::Codex => 8_000,
    };
    let always: usize = body
        .files
        .iter()
        .filter(|f| {
            matches!(
                f.path.as_str(),
                "systemprompt.md"
                    | "guardrails.md"
                    | "LongTermMEMORY.md"
                    | "tool_descriptions/tool_guidance.md"
                    | "AGENTS.md"
            )
        })
        .map(|f| f.content.chars().count())
        .sum();
    let always = u32::try_from(always).unwrap_or(u32::MAX);
    let mut errors = Vec::new();
    if always > cap {
        errors.push(format!(
            "always-on text is {always} characters; the cap is {cap}"
        ));
    }
    if !body.allow_subagents && body.files.iter().any(|f| f.path.starts_with("sub_agents/")) {
        errors.push("sub-agents are not allowed on this deployment".to_owned());
    }
    Ok(Json(ValidationReport {
        ok: errors.is_empty(),
        errors,
        off_surface: body
            .files
            .iter()
            .filter(|f| f.path.starts_with("notes/"))
            .map(|f| f.path.clone())
            .collect(),
        census: Census {
            always_on_chars: always,
            always_on_max: cap,
            n_files: u32::try_from(body.files.len()).unwrap_or(0),
            ..Census::default()
        },
    }))
}

// ---------------------------------------------------------------------------------------
// Jobs

async fn list_jobs(
    State(st): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> R<Json<Page<Job>>> {
    require(&st, &headers, Role::Viewer)?;
    let (offset, limit) = paging(&q, 25, 100);
    let g = lock(&st.s);
    let mut items: Vec<Job> = g
        .jobs
        .iter()
        .map(|j| j.job.clone())
        .filter(|j| {
            q.get("status")
                .is_none_or(|s| s.is_empty() || j.status.as_str() == s)
        })
        .filter(|j| {
            q.get("kind")
                .is_none_or(|k| k.is_empty() || j.kind.as_str() == k)
        })
        .filter(|j| {
            q.get("experiment")
                .is_none_or(|e| e.is_empty() || j.spec.experiment() == Some(e.as_str()))
        })
        .collect();
    items.reverse();
    Ok(Json(page(items, offset, limit)))
}

fn validate_spec(g: &Inner, spec: &JobSpec) -> R<()> {
    let opts_and_cell = match spec {
        JobSpec::Preview(p) => Some((&p.options, &p.cell)),
        JobSpec::Run(p) => Some((&p.options, &p.cell)),
        JobSpec::Pipeline(p) => Some((&p.options, &p.cell)),
        _ => None,
    };
    if let Some((o, c)) = opts_and_cell {
        let mut problems = o.validate().err().unwrap_or_default();
        if let Err(e) = c.validate() {
            problems.push(("cell".into(), e));
        }
        if let Some(f) = &o.split_file
            && !g.splits.iter().any(|s| &s.name == f)
        {
            problems.push(("split_file".into(), format!("no split named {f}")));
        }
        if !problems.is_empty() {
            return Err(invalid(problems));
        }
    }
    match spec {
        JobSpec::Demo(p) if p.k == 0 || p.k > h2w_api::DEMO_K_MAX => {
            return Err(invalid(vec![(
                "k".into(),
                "the demo takes k from 1 to 8".into(),
            )]));
        }
        JobSpec::Setup(p) if p.bench != Bench::Wb && p.subset.is_some() => {
            return Err(invalid(vec![(
                "subset".into(),
                "subsets apply to WorkBuddy-Bench only".into(),
            )]));
        }
        JobSpec::Split(p)
            if !(p.train_fraction > 0.0 && p.train_fraction < 1.0) || !valid_slug(&p.name) =>
        {
            return Err(invalid(vec![(
                "train_fraction".into(),
                "between 0 and 1 (exclusive), with a valid name".into(),
            )]));
        }
        _ => {}
    }
    if !g.installed && !matches!(spec, JobSpec::InstallHarness2(_)) {
        return Err(fail(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::Unavailable,
            "harness2 is not installed yet; an administrator can install it from the dashboard",
        ));
    }
    if let Some(exp) = spec.experiment()
        && !matches!(spec, JobSpec::Demo(_))
    {
        let e = g
            .experiments
            .get(exp)
            .ok_or_else(|| not_found("experiment"))?;
        let profile = match spec {
            JobSpec::Run(p) => Some(&p.profile),
            JobSpec::Pipeline(p) => Some(&p.profile),
            JobSpec::Judge(p) => Some(&p.profile),
            _ => None,
        };
        if let Some(prof) = profile {
            if let Some(pinned) = &e.exp.profile
                && pinned != prof
            {
                return Err(fail(
                    StatusCode::CONFLICT,
                    ErrorCode::Conflict,
                    &format!(
                        "experiment {exp} is pinned to profile {pinned}; harness2 refuses to resume or judge a namespace under different models"
                    ),
                ));
            }
            let p = g.profiles.get(prof).ok_or_else(|| {
                invalid(vec![("profile".into(), format!("no profile named {prof}"))])
            })?;
            if let Some(missing) = g.profile_view(p).secrets.iter().find(|s| !s.set) {
                return Err(invalid(vec![(
                    "profile".into(),
                    format!("secret {} is not set", missing.name),
                )]));
            }
        }
    }
    // Like the server: host isolation needs the acknowledgement unless pre-acknowledged.
    let host = g.isolation == Isolation::Host && !g.host_risks_preacknowledged;
    let ack = match spec {
        JobSpec::Run(p) => Some(p.acknowledge_risks),
        JobSpec::Pipeline(p) => Some(p.acknowledge_risks),
        _ => None,
    };
    if host && ack == Some(false) {
        return Err(invalid(vec![(
            "acknowledge_risks".into(),
            "jobs run on the host without isolation: acknowledge the risks to continue".into(),
        )]));
    }
    Ok(())
}

async fn create_job(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(spec): Json<JobSpec>,
) -> R<(StatusCode, Json<Job>)> {
    let p = require(&st, &headers, spec.required_role())?;
    let job = {
        let mut g = lock(&st.s);
        validate_spec(&g, &spec)?;
        let (tx, _) = tokio::sync::broadcast::channel(1024);
        let job = Job {
            id: crate::util::job_id(),
            kind: spec.kind(),
            spec: spec.clone(),
            status: JobStatus::Queued,
            created_by: p.name.clone(),
            created_at: rfc3339(now_secs()),
            started_at: None,
            finished_at: None,
            exit_code: None,
            isolation: g.isolation,
            steps: Vec::new(),
            progress: None,
            result: None,
            error: None,
            log_lines: 0,
            spent_usd: spec.may_spend().then_some(0.0),
        };
        g.jobs.push(JobData {
            job: job.clone(),
            log: Vec::new(),
            tx,
            cancel: false,
        });
        g.add_audit(
            &p.name,
            "job.create",
            &job.id,
            AuditOutcome::Success,
            Some(spec.kind().as_str().to_owned()),
        );
        job
    };
    sim::spawn(st.s.clone(), job.id.clone(), st.task_ms);
    Ok((StatusCode::CREATED, Json(job)))
}

async fn get_job(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> R<Json<Job>> {
    require(&st, &headers, Role::Viewer)?;
    let g = lock(&st.s);
    let i = g.job_index(&id).ok_or_else(|| not_found("job"))?;
    Ok(Json(g.jobs[i].job.clone()))
}

async fn cancel_job(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> R<Json<Job>> {
    let p = require(&st, &headers, Role::Operator)?;
    let mut g = lock(&st.s);
    let i = g.job_index(&id).ok_or_else(|| not_found("job"))?;
    if g.jobs[i].job.status.is_terminal() {
        return Err(fail(
            StatusCode::CONFLICT,
            ErrorCode::Conflict,
            "the job has already finished",
        ));
    }
    g.jobs[i].cancel = true;
    g.log(
        i,
        h2w_api::LogStream::System,
        &format!("cancel requested by {}", p.name),
    );
    g.add_audit(&p.name, "job.cancel", &id, AuditOutcome::Success, None);
    Ok(Json(g.jobs[i].job.clone()))
}

async fn job_log(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> R<Json<h2w_api::LogChunk>> {
    require(&st, &headers, Role::Viewer)?;
    let from: usize = q.get("from").and_then(|v| v.parse().ok()).unwrap_or(0);
    let limit: usize = q
        .get("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(500)
        .clamp(1, 5000);
    let g = lock(&st.s);
    let i = g.job_index(&id).ok_or_else(|| not_found("job"))?;
    let j = &g.jobs[i];
    let lines: Vec<h2w_api::LogLine> = j.log.iter().skip(from).take(limit).cloned().collect();
    let next = (from + lines.len()) as u64;
    Ok(Json(h2w_api::LogChunk {
        complete: j.job.status.is_terminal() && next >= j.log.len() as u64,
        next,
        lines,
    }))
}

fn sse_event(ev: &JobEvent) -> Event {
    let e = Event::default()
        .json_data(ev)
        .unwrap_or_else(|_| Event::default().data("{}"));
    match ev {
        JobEvent::Log { line } => e.id(line.n.to_string()),
        _ => e,
    }
}

type EventStream = Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>;

async fn job_events(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> R<Response> {
    require(&st, &headers, Role::Viewer)?;
    if st.sse_off {
        return Err(fail(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::Unavailable,
            "event stream disabled (mock --sse-off)",
        ));
    }
    let (rx, snapshot) = {
        let g = lock(&st.s);
        let i = g.job_index(&id).ok_or_else(|| not_found("job"))?;
        (g.jobs[i].tx.subscribe(), g.jobs[i].job.clone())
    };
    let terminal = snapshot.status.is_terminal();
    let first = tokio_stream::once(Ok::<Event, Infallible>(sse_event(&JobEvent::Snapshot {
        job: Box::new(snapshot),
    })));
    let stream: EventStream = if terminal {
        Box::pin(first)
    } else {
        let live = BroadcastStream::new(rx)
            .filter_map(|r| r.ok())
            .map(|ev| Ok::<Event, Infallible>(sse_event(&ev)));
        let beats = IntervalStream::new(tokio::time::interval_at(
            tokio::time::Instant::now() + Duration::from_secs(15),
            Duration::from_secs(15),
        ))
        .map(|_| {
            Ok::<Event, Infallible>(sse_event(&JobEvent::Heartbeat {
                ts: rfc3339(now_secs()),
            }))
        });
        Box::pin(first.chain(live.merge(beats)))
    };
    Ok(Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(10))
                .text("keep-alive"),
        )
        .into_response())
}

// ---------------------------------------------------------------------------------------
// Root endpoints, static files, security middleware

async fn healthz() -> Json<Health> {
    Json(Health {
        status: "ok".to_owned(),
    })
}

async fn readyz() -> Json<Readiness> {
    Json(Readiness {
        ready: true,
        failing: Vec::new(),
    })
}

async fn csp_report(body: String) -> StatusCode {
    #[allow(clippy::print_stderr)]
    {
        eprintln!("CSP VIOLATION REPORT: {body}");
    }
    StatusCode::NO_CONTENT
}

async fn static_or_index(State(st): State<AppState>, method: Method, uri: Uri) -> Response {
    let path = uri.path();
    if path.starts_with("/api/") {
        return not_found("route").into_response();
    }
    if method != Method::GET && method != Method::HEAD {
        return fail(
            StatusCode::METHOD_NOT_ALLOWED,
            ErrorCode::BadRequest,
            "method not allowed",
        )
        .into_response();
    }
    let rel = path.trim_start_matches('/');
    let safe = !rel.split('/').any(|s| s == ".." || s.starts_with('.')) && !rel.contains('\\');
    let file = if !rel.is_empty() && safe {
        st.dist.join(rel)
    } else {
        st.dist.join("index.html")
    };
    let (file, is_index) = if file.is_file() {
        (file, rel.is_empty())
    } else {
        (st.dist.join("index.html"), true)
    };
    match tokio::fs::read(&file).await {
        Ok(bytes) => {
            let mime = mime_guess::from_path(&file).first_or_octet_stream();
            let mut resp = Response::new(Body::from(bytes));
            if let Ok(v) = HeaderValue::from_str(mime.as_ref()) {
                resp.headers_mut().insert(header::CONTENT_TYPE, v);
            }
            resp.headers_mut().insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static(if is_index {
                    "no-cache"
                } else {
                    "no-cache, max-age=0"
                }),
            );
            resp
        }
        Err(_) => (
            StatusCode::NOT_FOUND,
            "dist/web not found: build the UI first with `cargo xtask web`",
        )
            .into_response(),
    }
}

async fn security(State(st): State<AppState>, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_owned();
    let is_api = path.starts_with("/api/");
    // DNS-rebinding defence: answer only to loopback host names.
    let host_ok = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|h| {
            let name = h.rsplit_once(':').map_or(h, |(n, _)| n);
            matches!(name, "127.0.0.1" | "localhost" | "[::1]")
        });
    let mut resp = if !host_ok {
        fail(
            StatusCode::MISDIRECTED_REQUEST,
            ErrorCode::MisdirectedRequest,
            "unknown Host",
        )
        .into_response()
    } else if is_api && !matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS) {
        let csrf_ok = req
            .headers()
            .get(h2w_api::CSRF_HEADER)
            .is_some_and(|v| v == "1");
        let origin_ok = req
            .headers()
            .get(header::ORIGIN)
            .and_then(|o| o.to_str().ok())
            .is_none_or(|o| {
                let host = req
                    .headers()
                    .get(header::HOST)
                    .and_then(|h| h.to_str().ok())
                    .unwrap_or_default();
                o == format!("http://{host}")
            });
        if !csrf_ok {
            fail(
                StatusCode::FORBIDDEN,
                ErrorCode::Forbidden,
                "missing x-h2w-request: 1 header",
            )
            .into_response()
        } else if !origin_ok {
            fail(
                StatusCode::FORBIDDEN,
                ErrorCode::Forbidden,
                "cross-origin request refused",
            )
            .into_response()
        } else {
            next.run(req).await
        }
    } else {
        next.run(req).await
    };
    let h = resp.headers_mut();
    let csp = if st.csp_report {
        format!("{CSP}; report-uri /__csp-report")
    } else {
        CSP.to_owned()
    };
    if let Ok(v) = HeaderValue::from_str(&csp) {
        h.insert(header::CONTENT_SECURITY_POLICY, v);
    }
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(
        "permissions-policy",
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    h.insert(
        "cross-origin-opener-policy",
        HeaderValue::from_static("same-origin"),
    );
    h.insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("same-origin"),
    );
    if is_api {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    resp
}

pub(crate) fn router(st: AppState) -> Router {
    let api = Router::new()
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/auth/me", get(me))
        .route("/system/info", get(system_info))
        .route("/system/doctor", get(doctor))
        .route("/benchmarks", get(benchmarks))
        .route("/splits", get(splits))
        .route("/profiles", get(profiles))
        .route("/profiles/{name}", put(put_profile).delete(delete_profile))
        .route("/profiles/{name}/check", get(check_profile))
        .route("/secrets", get(secrets))
        .route("/secrets/{name}", put(put_secret).delete(delete_secret))
        .route("/tokens", get(tokens).post(create_token))
        .route("/tokens/{id}", axum::routing::delete(revoke_token))
        .route("/audit", get(audit_log))
        .route("/audit/verify", get(audit_verify))
        .route("/experiments", get(experiments).post(create_experiment))
        .route(
            "/experiments/{exp}",
            get(get_experiment).patch(patch_experiment),
        )
        .route("/experiments/{exp}/manifest", get(manifest))
        .route("/experiments/{exp}/sweeps/{sweep}/tasks", get(tasks))
        .route("/experiments/{exp}/sweeps/{sweep}/tasks/{task}", get(task))
        .route(
            "/experiments/{exp}/sweeps/{sweep}/tasks/{task}/modes/{mode}/harnesses/{label}",
            get(harness),
        )
        .route(
            "/experiments/{exp}/sweeps/{sweep}/tasks/{task}/modes/{mode}/diff",
            get(diff),
        )
        .route(
            "/experiments/{exp}/sweeps/{sweep}/tasks/{task}/modes/{mode}/harnesses/{label}/export",
            get(export),
        )
        .route(
            "/experiments/{exp}/sweeps/{sweep}/tasks/{task}/modes/{mode}/rollouts/{label}/files",
            get(rollout_files),
        )
        .route(
            "/experiments/{exp}/sweeps/{sweep}/tasks/{task}/modes/{mode}/rollouts/{label}/file",
            get(rollout_file),
        )
        .route("/experiments/{exp}/sweeps/{sweep}/scores", get(scores))
        .route("/experiments/{exp}/sweeps/{sweep}/report", get(report))
        .route("/experiments/{exp}/sweeps/{sweep}/library", get(library))
        .route("/experiments/{exp}/sweeps/{sweep}/audit", get(sweep_audit))
        .route(
            "/experiments/{exp}/sweeps/{sweep}/audit/{session}/{part}",
            get(sweep_audit_part),
        )
        .route("/compare", get(compare))
        .route("/estimate", post(estimate))
        .route("/harness/validate", post(validate_harness))
        .route("/jobs", get(list_jobs).post(create_job))
        .route("/jobs/{id}", get(get_job))
        .route("/jobs/{id}/cancel", post(cancel_job))
        .route("/jobs/{id}/log", get(job_log))
        .route("/jobs/{id}/events", get(job_events));
    Router::new()
        .nest("/api/v1", api)
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/__csp-report", post(csp_report))
        .fallback(static_or_index)
        .layer(middleware::from_fn_with_state(st.clone(), security))
        .with_state(st)
}

/// Seed the populated mock with a live run and a pipeline queued behind it.
pub(crate) fn start_live_jobs(st: &AppState, live_task_ms: u64) {
    for (spec, ms) in [
        (state::live_run_spec(), live_task_ms),
        (state::queued_spec(), st.task_ms),
    ] {
        let id = {
            let mut g = lock(&st.s);
            let (tx, _) = tokio::sync::broadcast::channel(1024);
            let job = Job {
                id: crate::util::job_id(),
                kind: spec.kind(),
                spec,
                status: JobStatus::Queued,
                created_by: "ci-runner".to_owned(),
                created_at: rfc3339(now_secs() - 90),
                started_at: None,
                finished_at: None,
                exit_code: None,
                isolation: Isolation::Host,
                steps: Vec::new(),
                progress: None,
                result: None,
                error: None,
                log_lines: 0,
                spent_usd: Some(0.0),
            };
            let id = job.id.clone();
            g.jobs.push(JobData {
                job,
                log: Vec::new(),
                tx,
                cancel: false,
            });
            id
        };
        sim::spawn(st.s.clone(), id, ms);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert!(valid_slug("lab-pilot"));
        assert!(!valid_slug("Lab"));
        assert!(!valid_slug(""));
        assert!(valid_secret_name("MODEL_GATEWAY_KEY"));
        assert!(!valid_secret_name("lower"));
        assert!(safe_rel("out/a.md"));
        assert!(!safe_rel("../x"));
        assert!(!safe_rel("/etc/passwd"));
    }

    #[test]
    fn dev_tokens_map_to_roles() {
        assert_eq!(role_of_dev_token("viewer_1"), Role::Viewer);
        assert_eq!(role_of_dev_token("operator"), Role::Operator);
        assert_eq!(role_of_dev_token("anything"), Role::Admin);
    }

    fn profile() -> ModelProfile {
        let ep = |key: &str| h2w_api::Endpoint {
            base_url: "https://gateway.example.test/v1".into(),
            api_key_secret: Some(key.into()),
        };
        ModelProfile {
            name: "gw".into(),
            kind: ProfileKind::OpenaiCompatible,
            description: String::new(),
            solver_model: "model-a".into(),
            improver_model: None,
            judge_model: Some("gpt-judge".into()),
            codex_model: None,
            vertex: None,
            endpoint: Some(ep("GW_KEY")),
            judge_endpoint: Some(ep("JUDGE_KEY")),
            codex_endpoint: Some(ep("GW_KEY")),
            extra_env: vec![h2w_api::EnvVar {
                name: "EXTRA".into(),
                value: None,
                secret: Some("EXTRA_KEY".into()),
            }],
            prices: None,
            egress_hosts: Vec::new(),
            secrets: Vec::new(),
        }
    }

    #[test]
    fn profile_check_parts() {
        let p = profile();
        // The server's order, without duplicates.
        assert_eq!(referenced_secrets(&p), ["GW_KEY", "JUDGE_KEY", "EXTRA_KEY"]);
        // A complete gateway profile only lacks WorkBuddy (needs Vertex Gemini).
        let w = profile_warnings(&p);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].starts_with("WorkBuddy-Bench cannot use this profile"));
        let bare = ModelProfile {
            judge_model: None,
            codex_endpoint: None,
            ..profile()
        };
        let w = profile_warnings(&bare);
        assert!(w.iter().any(|x| x.starts_with("No judge model")), "{w:?}");
        assert!(
            w.iter().any(|x| x.starts_with("The Codex executor")),
            "{w:?}"
        );
        assert!(!PROFILE_NAME_RULE.is_empty() && !valid_slug("Bad Name"));
    }

    #[test]
    fn host_acknowledgement_follows_preack() {
        let unacknowledged = |spec: JobSpec| match spec {
            JobSpec::Run(mut p) => {
                p.acknowledge_risks = false;
                JobSpec::Run(p)
            }
            other => other,
        };
        let spec = unacknowledged(state::live_run_spec());
        let mut g = Inner::populated();
        assert!(validate_spec(&g, &state::live_run_spec()).is_ok());
        // Host isolation without the acknowledgement is refused...
        assert!(validate_spec(&g, &spec).is_err());
        // ...unless the server pre-acknowledges host risks.
        g.host_risks_preacknowledged = true;
        assert!(validate_spec(&g, &spec).is_ok());
        g.host_risks_preacknowledged = false;
        g.isolation = Isolation::Container;
        assert!(validate_spec(&g, &spec).is_ok());
    }

    #[test]
    fn experiment_kinds_serialize() {
        assert_eq!(
            serde_json::to_value(h2w_api::ExperimentKind::Demo).unwrap(),
            "demo"
        );
    }
}
