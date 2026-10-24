//! The HTTP server: application state, the route table, the router.
//!
//! [`ROUTES`] is the only place API routes are registered: [`router`] builds the router
//! from it, and `tests/rbac.rs` walks it to prove every route enforces the role written
//! next to it (the handlers enforce roles themselves, through `extract::Auth<...>`; the
//! table records the contract the test checks them against).

pub mod extract;
pub mod guard;
pub mod openapi;
pub mod routes;
pub mod server;
pub mod static_ui;

use std::net::SocketAddr;
use std::ops::Deref;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use axum::Router;
use axum::routing::{MethodRouter, delete, get, patch, post, put};
use h2s_api::Role;
use http::Method;
use tokio::sync::watch;
use zeroize::Zeroizing;

use crate::config::{Config, HostEntry};
use crate::db::Db;
use crate::db::audit::AuditEntry;
use crate::doctor::DoctorService;
use crate::jobs::JobManager;
use crate::metrics::Metrics;
use crate::ratelimit::RateLimiter;
use crate::secrets::SecretStore;

/// Shared state of the server.
#[derive(Clone)]
pub struct AppState(Arc<AppInner>);

/// What [`AppState`] holds.
pub struct AppInner {
    pub cfg: Arc<Config>,
    pub db: Db,
    pub jobs: Arc<JobManager>,
    pub secrets: SecretStore,
    pub metrics: Arc<Metrics>,
    pub login_limiter: RateLimiter,
    pub doctor: DoctorService,
    /// `Host` values this server answers to (configured + loopback forms of the bound port).
    pub hosts: Vec<HostEntry>,
    pub sse_open: AtomicUsize,
    pub shutdown: watch::Receiver<bool>,
    /// Shared secret a trusted reverse proxy presents (proxy auth mode).
    pub proxy_secret: Option<Zeroizing<String>>,
    pub bound: SocketAddr,
}

impl Deref for AppState {
    type Target = AppInner;
    fn deref(&self) -> &AppInner {
        &self.0
    }
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("bound", &self.bound)
            .finish_non_exhaustive()
    }
}

impl AppState {
    #[must_use]
    pub fn new(inner: AppInner) -> AppState {
        AppState(Arc::new(inner))
    }

    /// Append to the audit log; failures are logged (the request itself proceeds).
    pub async fn audit(&self, entry: AuditEntry) {
        if let Err(e) = self
            .db
            .call(move |c| crate::db::audit::append_tx(c, &entry))
            .await
        {
            tracing::error!(error = %e, "could not append to the audit log");
        }
    }
}

/// The `Host` values a server bound to `bound` answers to.
#[must_use]
pub fn host_allowlist(cfg: &Config, bound: SocketAddr) -> Vec<HostEntry> {
    let mut hosts = cfg.allowed_hosts.clone();
    let port = Some(bound.port());
    for h in ["127.0.0.1", "localhost", "::1"] {
        hosts.push(HostEntry {
            host: h.to_owned(),
            port,
        });
    }
    if !bound.ip().is_unspecified() && !bound.ip().is_loopback() {
        hosts.push(HostEntry {
            host: bound.ip().to_string(),
            port,
        });
    }
    hosts
}

/// Who may call a route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// No authentication (health, readiness, login).
    Public,
    /// Any authenticated principal.
    Authenticated,
    /// At least this role.
    Role(Role),
    /// `POST /jobs`: at least operator, plus `JobSpec::required_role()`.
    JobSpec,
    /// `/metrics`: a viewer, or the configured metrics token.
    MetricsOrViewer,
}

/// One API route.
#[derive(Debug)]
pub struct RouteDef {
    pub method: Method,
    pub path: &'static str,
    pub access: Access,
    pub handler: fn() -> MethodRouter<AppState>,
}

use routes::{
    admin, auth as auth_routes, experiments, jobs as job_routes, results, root, system, tools,
};

const V: Access = Access::Role(Role::Viewer);
const O: Access = Access::Role(Role::Operator);
const A: Access = Access::Role(Role::Admin);

/// Every route of the server except the static UI fallback.
pub static ROUTES: &[RouteDef] = &[
    RouteDef {
        method: Method::GET,
        path: "/healthz",
        access: Access::Public,
        handler: || get(root::healthz),
    },
    RouteDef {
        method: Method::GET,
        path: "/readyz",
        access: Access::Public,
        handler: || get(root::readyz),
    },
    RouteDef {
        method: Method::GET,
        path: "/metrics",
        access: Access::MetricsOrViewer,
        handler: || get(root::metrics),
    },
    RouteDef {
        method: Method::POST,
        path: "/api/v1/auth/login",
        access: Access::Public,
        handler: || post(auth_routes::login),
    },
    RouteDef {
        method: Method::POST,
        path: "/api/v1/auth/logout",
        access: Access::Authenticated,
        handler: || post(auth_routes::logout),
    },
    RouteDef {
        method: Method::GET,
        path: "/api/v1/auth/me",
        access: Access::Authenticated,
        handler: || get(auth_routes::me),
    },
    RouteDef {
        method: Method::GET,
        path: "/api/v1/system/info",
        access: V,
        handler: || get(system::info),
    },
    RouteDef {
        method: Method::GET,
        path: "/api/v1/system/doctor",
        access: V,
        handler: || get(system::doctor),
    },
    RouteDef {
        method: Method::GET,
        path: "/api/v1/benchmarks",
        access: V,
        handler: || get(system::benchmarks),
    },
    RouteDef {
        method: Method::GET,
        path: "/api/v1/openapi.json",
        access: V,
        handler: || get(system::openapi),
    },
    RouteDef {
        method: Method::GET,
        path: "/api/v1/profiles",
        access: V,
        handler: || get(admin::list_profiles),
    },
    RouteDef {
        method: Method::PUT,
        path: "/api/v1/profiles/{name}",
        access: A,
        handler: || put(admin::put_profile),
    },
    RouteDef {
        method: Method::DELETE,
        path: "/api/v1/profiles/{name}",
        access: A,
        handler: || delete(admin::delete_profile),
    },
    RouteDef {
        method: Method::GET,
        path: "/api/v1/profiles/{name}/check",
        access: V,
        handler: || get(admin::check_profile),
    },
    RouteDef {
        method: Method::GET,
        path: "/api/v1/secrets",
        access: A,
        handler: || get(admin::list_secrets),
    },
    RouteDef {
        method: Method::PUT,
        path: "/api/v1/secrets/{name}",
        access: A,
        handler: || put(admin::put_secret),
    },
    RouteDef {
        method: Method::DELETE,
        path: "/api/v1/secrets/{name}",
        access: A,
        handler: || delete(admin::delete_secret),
    },
    RouteDef {
        method: Method::GET,
        path: "/api/v1/tokens",
        access: A,
        handler: || get(admin::list_tokens),
    },
    RouteDef {
        method: Method::POST,
        path: "/api/v1/tokens",
        access: A,
        handler: || post(admin::create_token),
    },
    RouteDef {
        method: Method::DELETE,
        path: "/api/v1/tokens/{id}",
        access: A,
        handler: || delete(admin::revoke_token),
    },
    RouteDef {
        method: Method::GET,
        path: "/api/v1/audit",
        access: A,
        handler: || get(admin::list_audit),
    },
    RouteDef {
        method: Method::GET,
        path: "/api/v1/audit/verify",
        access: A,
        handler: || get(admin::verify_audit),
    },
    RouteDef {
        method: Method::GET,
        path: "/api/v1/experiments",
        access: V,
        handler: || get(experiments::list),
    },
    RouteDef {
        method: Method::POST,
        path: "/api/v1/experiments",
        access: O,
        handler: || post(experiments::create),
    },
    RouteDef {
        method: Method::GET,
        path: "/api/v1/experiments/{exp}",
        access: V,
        handler: || get(experiments::get_one),
    },
    RouteDef {
        method: Method::PATCH,
        path: "/api/v1/experiments/{exp}",
        access: O,
        handler: || patch(experiments::update),
    },
    RouteDef {
        method: Method::GET,
        path: "/api/v1/experiments/{exp}/manifest",
        access: V,
        handler: || get(experiments::manifest),
    },
    RouteDef {
        method: Method::GET,
        path: "/api/v1/experiments/{exp}/sweeps/{sweep}/tasks",
        access: V,
        handler: || get(results::tasks),
    },
    RouteDef {
        method: Method::GET,
        path: "/api/v1/experiments/{exp}/sweeps/{sweep}/tasks/{task}",
        access: V,
        handler: || get(results::task),
    },
    RouteDef {
        method: Method::GET,
        path: "/api/v1/experiments/{exp}/sweeps/{sweep}/tasks/{task}/modes/{mode}/harnesses/{label}",
        access: V,
        handler: || get(results::harness),
    },
    RouteDef {
        method: Method::GET,
        path: "/api/v1/experiments/{exp}/sweeps/{sweep}/tasks/{task}/modes/{mode}/diff",
        access: V,
        handler: || get(results::diff),
    },
    RouteDef {
        method: Method::GET,
        path: "/api/v1/experiments/{exp}/sweeps/{sweep}/tasks/{task}/modes/{mode}/harnesses/{label}/export",
        access: V,
        handler: || get(results::export),
    },
    RouteDef {
        method: Method::GET,
        path: "/api/v1/experiments/{exp}/sweeps/{sweep}/tasks/{task}/modes/{mode}/rollouts/{label}/files",
        access: V,
        handler: || get(results::rollout_files),
    },
    RouteDef {
        method: Method::GET,
        path: "/api/v1/experiments/{exp}/sweeps/{sweep}/tasks/{task}/modes/{mode}/rollouts/{label}/file",
        access: V,
        handler: || get(results::rollout_file),
    },
    RouteDef {
        method: Method::GET,
        path: "/api/v1/experiments/{exp}/sweeps/{sweep}/scores",
        access: V,
        handler: || get(results::scores),
    },
    RouteDef {
        method: Method::GET,
        path: "/api/v1/experiments/{exp}/sweeps/{sweep}/report",
        access: V,
        handler: || get(results::report),
    },
    RouteDef {
        method: Method::GET,
        path: "/api/v1/experiments/{exp}/sweeps/{sweep}/library",
        access: V,
        handler: || get(results::library),
    },
    RouteDef {
        method: Method::GET,
        path: "/api/v1/experiments/{exp}/sweeps/{sweep}/audit",
        access: V,
        handler: || get(results::audit_sessions),
    },
    RouteDef {
        method: Method::GET,
        path: "/api/v1/experiments/{exp}/sweeps/{sweep}/audit/{session}/{part}",
        access: V,
        handler: || get(results::audit_text),
    },
    RouteDef {
        method: Method::GET,
        path: "/api/v1/compare",
        access: V,
        handler: || get(results::compare),
    },
    RouteDef {
        method: Method::POST,
        path: "/api/v1/estimate",
        access: V,
        handler: || post(tools::estimate),
    },
    RouteDef {
        method: Method::POST,
        path: "/api/v1/harness/validate",
        access: V,
        handler: || post(tools::validate_harness),
    },
    RouteDef {
        method: Method::GET,
        path: "/api/v1/splits",
        access: V,
        handler: || get(tools::splits),
    },
    RouteDef {
        method: Method::GET,
        path: "/api/v1/jobs",
        access: V,
        handler: || get(job_routes::list),
    },
    RouteDef {
        method: Method::POST,
        path: "/api/v1/jobs",
        access: Access::JobSpec,
        handler: || post(job_routes::create),
    },
    RouteDef {
        method: Method::GET,
        path: "/api/v1/jobs/{id}",
        access: V,
        handler: || get(job_routes::get_one),
    },
    RouteDef {
        method: Method::POST,
        path: "/api/v1/jobs/{id}/cancel",
        access: O,
        handler: || post(job_routes::cancel),
    },
    RouteDef {
        method: Method::GET,
        path: "/api/v1/jobs/{id}/log",
        access: V,
        handler: || get(job_routes::log),
    },
    RouteDef {
        method: Method::GET,
        path: "/api/v1/jobs/{id}/events",
        access: V,
        handler: || get(job_routes::events),
    },
];

/// The complete router: [`ROUTES`], the static UI fallback, and the middleware.
pub fn router(state: AppState) -> Router {
    let mut r: Router<AppState> = Router::new();
    for d in ROUTES {
        r = r.route(d.path, (d.handler)());
    }
    r.route_layer(axum::middleware::from_fn(guard::route_label))
        .fallback(static_ui::fallback)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            guard::guard,
        ))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for d in ROUTES {
            assert!(
                seen.insert((d.method.clone(), d.path)),
                "duplicate {} {}",
                d.method,
                d.path
            );
        }
    }

    #[test]
    fn api_routes_live_under_the_prefix() {
        for d in ROUTES {
            assert!(
                d.path.starts_with(h2s_api::API_PREFIX)
                    || matches!(d.path, "/healthz" | "/readyz" | "/metrics"),
                "{}",
                d.path
            );
        }
    }
}
