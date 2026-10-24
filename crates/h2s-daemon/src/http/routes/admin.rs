//! Profiles, secrets, tokens and the audit log.

use axum::Json;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use h2s_api::{
    AuditEvent, AuditOutcome, AuditVerification, CreateTokenRequest, ErrorBody, ModelProfile, Page,
    ProfileCheck, SecretInfo, SetSecretRequest, TokenCreated, TokenInfo,
};
use http::StatusCode;
use serde::Deserialize;

use super::{check_name, clamp_limit};
use crate::db::tokens::TokenRecord;
use crate::error::ApiError;
use crate::http::AppState;
use crate::http::extract::{AdminRole, ApiJson, ApiQuery, Auth, ViewerRole};
use crate::util::now_unix;

/// Longest secret value accepted (a service-account key file is a few KiB).
pub const SECRET_MAX_BYTES: usize = 64 * 1024;

async fn with_secret_infos(
    state: &AppState,
    mut p: ModelProfile,
) -> Result<ModelProfile, ApiError> {
    let mut infos = Vec::new();
    for name in crate::db::profiles::referenced_secrets(&p) {
        infos.push(state.secrets.info(&name).await?);
    }
    p.secrets = infos;
    Ok(p)
}

/// Every model profile (with the state of the secrets each references).
#[utoipa::path(get, path = "/api/v1/profiles", tag = "profiles",
    responses((status = 200, body = Vec<ModelProfile>), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn list_profiles(
    _auth: Auth<ViewerRole>,
    State(state): State<AppState>,
) -> Result<Json<Vec<ModelProfile>>, ApiError> {
    let profiles = state.db.call(|c| crate::db::profiles::list(c)).await?;
    let mut out = Vec::with_capacity(profiles.len());
    for p in profiles {
        out.push(with_secret_infos(&state, p).await?);
    }
    Ok(Json(out))
}

/// What jobs with a profile would lack: `h2s_core::env::profile_warnings` and the referenced
/// secrets without a value (job creation refuses those with 422).
#[utoipa::path(get, path = "/api/v1/profiles/{name}/check", tag = "profiles",
    params(("name" = String, Path, description = "Profile name")),
    responses((status = 200, body = ProfileCheck), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn check_profile(
    _auth: Auth<ViewerRole>,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<ProfileCheck>, ApiError> {
    check_name("name", h2s_core::names::profile_name(&name))?;
    let n = name.clone();
    let profile = state
        .db
        .call(move |c| crate::db::profiles::get(c, &n))
        .await?
        .ok_or_else(|| ApiError::not_found(format!("no profile named {name}")))?;
    let mut missing_secrets = Vec::new();
    for secret in crate::db::profiles::referenced_secrets(&profile) {
        if !state.secrets.info(&secret).await?.set {
            missing_secrets.push(secret);
        }
    }
    Ok(Json(ProfileCheck {
        warnings: h2s_core::env::profile_warnings(&profile),
        missing_secrets,
    }))
}

/// Create or replace a profile.
#[utoipa::path(put, path = "/api/v1/profiles/{name}", tag = "profiles",
    params(("name" = String, Path, description = "Profile name")),
    request_body = ModelProfile,
    responses((status = 200, body = ModelProfile), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn put_profile(
    auth: Auth<AdminRole>,
    State(state): State<AppState>,
    Path(name): Path<String>,
    ApiJson(mut profile): ApiJson<ModelProfile>,
) -> Result<Json<ModelProfile>, ApiError> {
    check_name("name", h2s_core::names::profile_name(&name))?;
    if profile.name != name {
        return Err(ApiError::invalid(
            "name",
            "the profile name in the body must match the URL",
        ));
    }
    profile.secrets.clear();
    h2s_core::env::validate_profile(&profile)?;
    let warnings = h2s_core::env::profile_warnings(&profile);
    for w in &warnings {
        tracing::warn!(profile = %name, "{w}");
    }
    let p = profile.clone();
    let mut entry = auth.audit("profile.put", &name, AuditOutcome::Success);
    if !warnings.is_empty() {
        entry.detail = Some(format!("warnings: {}", warnings.join("; ")));
    }
    state
        .db
        .call(move |c| {
            crate::db::write_tx(c, |tx| {
                crate::db::profiles::put(tx, &p, now_unix())?;
                crate::db::audit::append(tx, &entry)?;
                Ok(())
            })
        })
        .await?;
    Ok(Json(with_secret_infos(&state, profile).await?))
}

/// Delete a profile (409 while an experiment is pinned to it).
#[utoipa::path(delete, path = "/api/v1/profiles/{name}", tag = "profiles",
    params(("name" = String, Path, description = "Profile name")),
    responses((status = 204, description = "Deleted"), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn delete_profile(
    auth: Auth<AdminRole>,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    check_name("name", h2s_core::names::profile_name(&name))?;
    let n = name.clone();
    let pinned = state
        .db
        .call(move |c| crate::db::experiments::pinning(c, &n))
        .await?;
    if !pinned.is_empty() {
        return Err(ApiError::conflict(format!(
            "profile {name} is pinned by experiment(s): {}",
            pinned.join(", ")
        )));
    }
    let n = name.clone();
    let entry = auth.audit("profile.delete", &name, AuditOutcome::Success);
    let deleted = state
        .db
        .call(move |c| {
            crate::db::write_tx(c, |tx| {
                let d = crate::db::profiles::delete(tx, &n)?;
                if d {
                    crate::db::audit::append(tx, &entry)?;
                }
                Ok(d)
            })
        })
        .await?;
    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found(format!("no profile named {name}")))
    }
}

/// Metadata of every secret (stored, from the environment, or referenced but unset).
#[utoipa::path(get, path = "/api/v1/secrets", tag = "secrets",
    responses((status = 200, body = Vec<SecretInfo>), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn list_secrets(
    _auth: Auth<AdminRole>,
    State(state): State<AppState>,
) -> Result<Json<Vec<SecretInfo>>, ApiError> {
    let profiles = state.db.call(|c| crate::db::profiles::list(c)).await?;
    let mut referenced: Vec<String> = profiles
        .iter()
        .flat_map(crate::db::profiles::referenced_secrets)
        .collect();
    let metrics = &state.cfg.file.telemetry.metrics_token_secret;
    if !metrics.is_empty() {
        referenced.push(metrics.clone());
    }
    Ok(Json(state.secrets.list(&referenced).await?))
}

/// Set a secret value (write-only: only its metadata comes back).
#[utoipa::path(put, path = "/api/v1/secrets/{name}", tag = "secrets",
    params(("name" = String, Path, description = "Secret NAME")),
    request_body = SetSecretRequest,
    responses((status = 200, body = SecretInfo), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn put_secret(
    auth: Auth<AdminRole>,
    State(state): State<AppState>,
    Path(name): Path<String>,
    body: Result<ApiJson<SetSecretRequest>, ApiError>,
) -> Result<Json<SecretInfo>, ApiError> {
    check_name("name", h2s_core::names::secret_name(&name))?;
    // Never echo anything about the body back: a malformed request may hold the value.
    let ApiJson(req) = body.map_err(|e| {
        if e.status == StatusCode::PAYLOAD_TOO_LARGE {
            e
        } else {
            ApiError::bad_request("the body must be {\"value\": \"...\"}")
        }
    })?;
    let value = zeroize::Zeroizing::new(req.value);
    if value.is_empty() {
        return Err(ApiError::invalid("value", "must not be empty"));
    }
    if value.len() > SECRET_MAX_BYTES {
        return Err(ApiError::invalid(
            "value",
            format!("must be at most {SECRET_MAX_BYTES} bytes"),
        ));
    }
    if value.contains('\0') {
        return Err(ApiError::invalid(
            "value",
            "must not contain NUL characters",
        ));
    }
    let chars = value.chars().count();
    let weak = value.len() < h2s_core::redact::MIN_SECRET_LEN;
    if weak {
        tracing::warn!(secret = %name, "a short secret value was set; short values are easy to mistake for ordinary text");
    }
    let mut detail = format!("{chars} chars");
    if weak {
        detail.push_str("; shorter than recommended");
    }
    if state.secrets.from_env(&name) {
        detail.push_str("; shadowed by H2S_SECRET_ environment variable");
    }
    let entry = auth
        .audit("secret.set", &name, AuditOutcome::Success)
        .detail(detail);
    let n = name.clone();
    state
        .db
        .call(move |c| {
            crate::db::write_tx(c, |tx| {
                crate::db::secrets::set(tx, &n, &value, now_unix())?;
                crate::db::audit::append(tx, &entry)?;
                Ok(())
            })
        })
        .await?;
    Ok(Json(state.secrets.info(&name).await?))
}

/// Delete a stored secret.
#[utoipa::path(delete, path = "/api/v1/secrets/{name}", tag = "secrets",
    params(("name" = String, Path, description = "Secret NAME")),
    responses((status = 204, description = "Deleted"), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn delete_secret(
    auth: Auth<AdminRole>,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    check_name("name", h2s_core::names::secret_name(&name))?;
    let n = name.clone();
    let entry = auth.audit("secret.delete", &name, AuditOutcome::Success);
    let deleted = state
        .db
        .call(move |c| {
            crate::db::write_tx(c, |tx| {
                let d = crate::db::secrets::delete(tx, &n)?;
                if d {
                    crate::db::audit::append(tx, &entry)?;
                }
                Ok(d)
            })
        })
        .await?;
    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found(format!(
            "no stored secret named {name}"
        )))
    }
}

/// Every token, newest first (never the secret).
#[utoipa::path(get, path = "/api/v1/tokens", tag = "tokens",
    responses((status = 200, body = Vec<TokenInfo>), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn list_tokens(
    _auth: Auth<AdminRole>,
    State(state): State<AppState>,
) -> Result<Json<Vec<TokenInfo>>, ApiError> {
    let tokens = state.db.call(|c| crate::db::tokens::list(c)).await?;
    Ok(Json(tokens.iter().map(TokenRecord::info).collect()))
}

/// Mint a token for `name` with `role`; the response shows it exactly once.
///
/// # Errors
/// Validation and database errors.
pub async fn mint_token(
    state: &AppState,
    created_by: &str,
    req: &CreateTokenRequest,
    entry: crate::db::audit::AuditEntry,
) -> Result<TokenCreated, ApiError> {
    check_name("name", h2s_core::names::token_name(&req.name))?;
    if let Some(d) = req.expires_in_days
        && !(1..=3650).contains(&d)
    {
        return Err(ApiError::invalid(
            "expires_in_days",
            "must be between 1 and 3650 (omit it for a token that never expires)",
        ));
    }
    let now = now_unix();
    for _ in 0..5 {
        let id = crate::auth::new_token_id().map_err(|_| ApiError::internal("no randomness"))?;
        let minted = crate::auth::mint(&id).map_err(|_| ApiError::internal("no randomness"))?;
        let rec = TokenRecord {
            id: minted.id.clone(),
            secret_hash: minted.secret_hash,
            name: req.name.clone(),
            role: req.role,
            created_at: now,
            created_by: created_by.to_owned(),
            expires_at: req.expires_in_days.map(|d| now + i64::from(d) * 86_400),
            revoked_at: None,
            last_used_at: None,
        };
        let r = rec.clone();
        let mut e = entry.clone();
        e.target.clone_from(&rec.id);
        e.detail = Some(format!("{} ({})", rec.name, rec.role.as_str()));
        let inserted = state
            .db
            .call(move |c| {
                crate::db::write_tx(c, |tx| {
                    if crate::db::tokens::exists(tx, &r.id)? {
                        return Ok(false);
                    }
                    crate::db::tokens::insert(tx, &r)?;
                    crate::db::audit::append(tx, &e)?;
                    Ok(true)
                })
            })
            .await?;
        if inserted {
            return Ok(TokenCreated {
                token: minted.token.as_str().to_owned(),
                info: rec.info(),
            });
        }
    }
    Err(ApiError::internal("could not allocate a token id"))
}

/// Create a token.
#[utoipa::path(post, path = "/api/v1/tokens", tag = "tokens",
    request_body = CreateTokenRequest,
    responses((status = 201, body = TokenCreated), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn create_token(
    auth: Auth<AdminRole>,
    State(state): State<AppState>,
    ApiJson(req): ApiJson<CreateTokenRequest>,
) -> Result<Response, ApiError> {
    let entry = auth.audit("token.create", "-", AuditOutcome::Success);
    let created = mint_token(&state, &auth.principal.name, &req, entry).await?;
    Ok((StatusCode::CREATED, Json(created)).into_response())
}

fn valid_token_id(id: &str) -> bool {
    id.len() == crate::auth::TOKEN_ID_LEN
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// Revoke a token; its sessions end at once.
#[utoipa::path(delete, path = "/api/v1/tokens/{id}", tag = "tokens",
    params(("id" = String, Path, description = "Token id")),
    responses((status = 204, description = "Revoked"), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn revoke_token(
    auth: Auth<AdminRole>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    if !valid_token_id(&id) {
        return Err(ApiError::not_found("no such token"));
    }
    let tid = id.clone();
    let entry = auth.audit("token.revoke", &id, AuditOutcome::Success);
    let outcome = state
        .db
        .call(move |c| {
            crate::db::write_tx(c, |tx| {
                let now = now_unix();
                let Some(t) = crate::db::tokens::get(tx, &tid)? else {
                    return Ok(Err(ApiError::not_found("no such token")));
                };
                if t.revoked_at.is_some() {
                    return Ok(Err(ApiError::conflict("the token is already revoked")));
                }
                if t.role == h2s_api::Role::Admin
                    && t.is_active(now)
                    && crate::db::tokens::count_active_admins(tx, now)? <= 1
                {
                    return Ok(Err(ApiError::conflict(
                        "this is the last active admin token; create another admin token first",
                    )));
                }
                crate::db::tokens::revoke(tx, &tid, now)?;
                crate::db::audit::append(tx, &entry)?;
                Ok(Ok(()))
            })
        })
        .await?;
    outcome.map(|()| StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct AuditQuery {
    /// Page size (1..=500, default 100).
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

/// The audit log, newest first.
#[utoipa::path(get, path = "/api/v1/audit", tag = "audit", params(AuditQuery),
    responses((status = 200, body = Page<AuditEvent>), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn list_audit(
    _auth: Auth<AdminRole>,
    State(state): State<AppState>,
    ApiQuery(q): ApiQuery<AuditQuery>,
) -> Result<Json<Page<AuditEvent>>, ApiError> {
    let limit = clamp_limit(q.limit, 100, 500);
    let before = match q.cursor.as_deref() {
        Some(c) => Some(
            crate::db::cursor::decode(c)
                .ok_or_else(|| ApiError::invalid("cursor", "invalid cursor"))?,
        ),
        None => None,
    };
    Ok(Json(
        state
            .db
            .call(move |c| crate::db::audit::list(c, limit, before))
            .await?,
    ))
}

/// Re-walk the audit hash chain.
#[utoipa::path(get, path = "/api/v1/audit/verify", tag = "audit",
    responses((status = 200, body = AuditVerification), (status = "default", description = "Error", body = ErrorBody)))]
pub async fn verify_audit(
    _auth: Auth<AdminRole>,
    State(state): State<AppState>,
) -> Result<Json<AuditVerification>, ApiError> {
    Ok(Json(state.db.call(|c| crate::db::audit::verify(c)).await?))
}
