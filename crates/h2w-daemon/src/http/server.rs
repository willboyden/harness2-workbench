//! Building the application and serving it.
//!
//! The accept loop is our own (hyper's HTTP/1 connection builder) rather than
//! `axum::serve`, so the server can bound header read time (slow-loris), cap concurrent
//! connections, attach the peer address to each request, and shut down gracefully: stop
//! accepting, end event streams, stop running jobs (SIGTERM, then SIGKILL), drain.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use axum::Router;
use axum::extract::ConnectInfo;
use hyper::body::Incoming;
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, oneshot, watch};
use tower::ServiceExt;
use zeroize::Zeroizing;

use super::{AppInner, AppState, host_allowlist, router};
use crate::config::Config;
use crate::db::Db;
use crate::doctor::DoctorService;
use crate::jobs::backend::JobBackend;
use crate::jobs::{JobManager, JobSettings};
use crate::metrics::Metrics;
use crate::ratelimit::RateLimiter;
use crate::secrets::{SecretEnv, SecretStore};

/// Concurrent connections served; more are closed at once.
pub const MAX_CONNECTIONS: usize = 1024;

/// Open connections per client address (IPv6 per /64), so one client cannot hold every
/// connection slot (`server.max_connections_per_ip`).
#[derive(Debug)]
pub struct ConnectionCounts {
    open: std::sync::Mutex<std::collections::HashMap<std::net::IpAddr, usize>>,
    max: usize,
}

/// One counted connection; released when dropped.
#[derive(Debug)]
pub struct ConnectionSlot {
    counts: Arc<ConnectionCounts>,
    key: std::net::IpAddr,
}

impl ConnectionCounts {
    #[must_use]
    pub fn new(max: usize) -> ConnectionCounts {
        ConnectionCounts {
            open: std::sync::Mutex::new(std::collections::HashMap::new()),
            max: max.max(1),
        }
    }

    fn lock(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashMap<std::net::IpAddr, usize>> {
        match self.open.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }

    /// A slot for a connection from `ip`, or `None` when that client is at its limit.
    #[must_use]
    pub fn try_acquire(self: &Arc<Self>, ip: std::net::IpAddr) -> Option<ConnectionSlot> {
        let key = crate::ratelimit::bucket_key(ip);
        let mut open = self.lock();
        let n = open.entry(key).or_insert(0);
        if *n >= self.max {
            return None;
        }
        *n += 1;
        Some(ConnectionSlot {
            counts: Arc::clone(self),
            key,
        })
    }

    /// Connections open from `ip`'s client.
    #[must_use]
    pub fn open_from(&self, ip: std::net::IpAddr) -> usize {
        self.lock()
            .get(&crate::ratelimit::bucket_key(ip))
            .copied()
            .unwrap_or(0)
    }
}

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        let mut open = self.counts.lock();
        if let Some(n) = open.get_mut(&self.key) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                open.remove(&self.key);
            }
        }
    }
}

/// Seconds between two warnings about refused connections (each is also counted in
/// `h2w_connections_refused_total`), so a flood does not flood the log.
const REFUSED_WARN_EVERY: i64 = 60;
/// Time a client has to send its request headers.
pub const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);
/// How long shutdown waits for running jobs to stop.
pub const JOB_STOP_WAIT: Duration = Duration::from_secs(20);

/// Why the server could not start.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("cannot create the data directories: {0}")]
    Dirs(std::io::Error),
    // Each message includes its cause, which is therefore not also a `source()`: printing
    // the chain (`{:#}`, as the CLI does) would show it twice.
    #[error("cannot open the database: {0}")]
    Db(crate::db::DbError),
    #[error("cannot listen on {addr}: {error}")]
    Bind {
        addr: SocketAddr,
        error: std::io::Error,
    },
    #[error("{0}")]
    Config(String),
}

impl From<crate::db::DbError> for StartError {
    fn from(e: crate::db::DbError) -> Self {
        StartError::Db(e)
    }
}

/// Options that tests (and embedders) may set; the defaults read the process environment.
pub struct ServerOptions {
    pub backend: Arc<dyn JobBackend>,
    /// Secret environment (lookup + names); `None` = the process environment.
    pub secret_env: Option<(SecretEnv, Vec<String>)>,
    /// Proxy shared secret; `None` = read `auth.proxy.secret_env` from the environment.
    pub proxy_secret: Option<String>,
    /// SIGTERM -> SIGKILL grace for job cancellation (10 s by default).
    pub kill_grace: Option<Duration>,
    /// Event-stream heartbeat (15 s by default; tests shorten it).
    pub sse_heartbeat: Option<Duration>,
}

impl ServerOptions {
    #[must_use]
    pub fn new(backend: Arc<dyn JobBackend>) -> ServerOptions {
        ServerOptions {
            backend,
            secret_env: None,
            proxy_secret: None,
            kill_grace: None,
            sse_heartbeat: None,
        }
    }
}

/// A bound, ready server.
pub struct Server {
    pub state: AppState,
    listener: TcpListener,
    shutdown_tx: watch::Sender<bool>,
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server")
            .field("addr", &self.state.bound)
            .finish_non_exhaustive()
    }
}

impl Server {
    /// Create directories, open the database, recover jobs, and bind `cfg.listen`.
    ///
    /// # Errors
    /// See [`StartError`].
    pub async fn bind(cfg: Config, opts: ServerOptions) -> Result<Server, StartError> {
        let listener = TcpListener::bind(cfg.listen)
            .await
            .map_err(|error| StartError::Bind {
                addr: cfg.listen,
                error,
            })?;
        Self::with_listener(cfg, opts, listener).await
    }

    /// Like [`Server::bind`] with an already bound listener.
    ///
    /// # Errors
    /// See [`StartError`].
    pub async fn with_listener(
        cfg: Config,
        opts: ServerOptions,
        listener: TcpListener,
    ) -> Result<Server, StartError> {
        cfg.paths.ensure().map_err(StartError::Dirs)?;
        let bound = listener.local_addr().map_err(|error| StartError::Bind {
            addr: cfg.listen,
            error,
        })?;
        let proxy_secret = if cfg.file.auth.mode == h2w_api::AuthMode::Proxy {
            let name = &cfg.file.auth.proxy.secret_env;
            let v = opts
                .proxy_secret
                .clone()
                .or_else(|| std::env::var(name).ok())
                .filter(|v| !v.is_empty())
                .ok_or_else(|| {
                    StartError::Config(format!(
                        "auth.mode = \"proxy\" needs the shared secret in the environment variable {name}"
                    ))
                })?;
            if v.len() < 16 {
                return Err(StartError::Config(format!(
                    "{name} must be at least 16 characters"
                )));
            }
            Some(Zeroizing::new(v))
        } else {
            None
        };
        let db = Db::open(&cfg.paths.db_path)?;
        let secrets = match opts.secret_env {
            Some((env, names)) => SecretStore::with_env(db.clone(), env, names),
            None => SecretStore::from_process_env(db.clone()),
        };
        let metrics = Arc::new(Metrics::new());
        let mut settings = JobSettings::from_config(&cfg);
        if let Some(g) = opts.kill_grace {
            settings.kill_grace = g;
        }
        let jobs = JobManager::new(
            db.clone(),
            settings,
            opts.backend,
            secrets.clone(),
            Arc::clone(&metrics),
        );
        jobs.start().await?;
        jobs.probe_container_runtime();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let hosts = host_allowlist(&cfg, bound);
        let per_minute = cfg.file.server.login_attempts_per_minute;
        let state = AppState::new(AppInner {
            cfg: Arc::new(cfg),
            db,
            jobs,
            secrets,
            metrics,
            login_limiter: RateLimiter::new(per_minute),
            auth_failures: RateLimiter::new(super::extract::AUTH_FAILURES_PER_MINUTE),
            failure_audits: RateLimiter::new(super::extract::AUDITED_FAILURES_PER_MINUTE),
            doctor: DoctorService::default(),
            results_reads: Arc::new(tokio::sync::Semaphore::new(super::routes::RESULTS_READS)),
            sweep_cache: Arc::new(super::sweeps::SweepCache::default()),
            hosts,
            sse_open: AtomicUsize::new(0),
            sse_by_principal: std::sync::Mutex::new(std::collections::HashMap::new()),
            sse_heartbeat: opts.sse_heartbeat.unwrap_or(super::routes::jobs::HEARTBEAT),
            shutdown: shutdown_rx,
            proxy_secret,
            bound,
        });
        if let Ok(Some((seq, hash))) = state.db.call(|c| crate::db::audit::head(c)).await {
            tracing::info!(seq, head = %hash, "audit chain head");
        }
        Ok(Server {
            state,
            listener,
            shutdown_tx,
        })
    }

    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.state.bound
    }

    /// Serve until `signal` completes, then shut down gracefully.
    pub async fn run(self, signal: impl Future<Output = ()> + Send) {
        let Server {
            state,
            listener,
            shutdown_tx,
        } = self;
        let app = router(state.clone());
        let limit = Arc::new(Semaphore::new(MAX_CONNECTIONS));
        let per_client = Arc::new(ConnectionCounts::new(
            usize::try_from(state.cfg.file.server.max_connections_per_ip).unwrap_or(256),
        ));
        let mut last_warning = i64::MIN;
        let mut refused = |reason: &'static str| {
            state
                .metrics
                .connections_refused
                .get_or_create(&crate::metrics::ReasonLabels { reason })
                .inc();
            let now = crate::util::now_unix();
            if now.saturating_sub(last_warning) >= REFUSED_WARN_EVERY {
                last_warning = now;
                tracing::warn!(
                    reason,
                    "connection limit reached; closing new connections (counted in h2w_connections_refused_total)"
                );
            }
        };
        let housekeeping = {
            let db = state.db.clone();
            tokio::spawn(async move {
                loop {
                    let now = crate::util::now_unix();
                    let _ = db
                        .call(move |c| crate::db::sessions::purge_expired(c, now))
                        .await;
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                }
            })
        };
        tokio::pin!(signal);
        loop {
            tokio::select! {
                accepted = listener.accept() => match accepted {
                    Ok((stream, peer)) => {
                        let Ok(permit) = Arc::clone(&limit).try_acquire_owned() else {
                            refused("total");
                            drop(stream);
                            continue;
                        };
                        // Trusted reverse proxies carry many clients; they limit their own.
                        let slot = if state.cfg.is_trusted_proxy(peer.ip()) {
                            None
                        } else {
                            match per_client.try_acquire(peer.ip()) {
                                Some(s) => Some(s),
                                None => {
                                    refused("per_client");
                                    drop(stream);
                                    continue;
                                }
                            }
                        };
                        let app = app.clone();
                        let rx = shutdown_tx.subscribe();
                        tokio::spawn(async move {
                            serve_connection(stream, peer, app, rx).await;
                            drop(slot);
                            drop(permit);
                        });
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "accept failed");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                },
                () = &mut signal => break,
            }
        }
        tracing::info!("shutting down");
        drop(listener);
        let _ = shutdown_tx.send(true);
        state.jobs.shutdown(JOB_STOP_WAIT).await;
        housekeeping.abort();
        let all = u32::try_from(MAX_CONNECTIONS).unwrap_or(u32::MAX);
        if tokio::time::timeout(Duration::from_secs(10), limit.acquire_many(all))
            .await
            .is_err()
        {
            tracing::warn!("some connections did not close in time");
        }
    }

    /// Run in the background (tests); stop with [`RunningServer::stop`].
    #[must_use]
    pub fn spawn(self) -> RunningServer {
        let addr = self.local_addr();
        let state = self.state.clone();
        let (tx, rx) = oneshot::channel::<()>();
        let task = tokio::spawn(self.run(async move {
            let _ = rx.await;
        }));
        RunningServer {
            addr,
            state,
            stop: Some(tx),
            task,
        }
    }
}

/// A server running in the background.
#[derive(Debug)]
pub struct RunningServer {
    pub addr: SocketAddr,
    pub state: AppState,
    stop: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl RunningServer {
    /// `http://127.0.0.1:<port>`
    #[must_use]
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Shut down gracefully and wait for it.
    pub async fn stop(mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
        let _ = (&mut self.task).await;
    }

    /// Abort without the graceful job shutdown (simulates a crash for recovery tests).
    pub fn abort(self) {
        self.task.abort();
    }
}

async fn serve_connection(
    stream: TcpStream,
    peer: SocketAddr,
    app: Router,
    mut shutdown: watch::Receiver<bool>,
) {
    let _ = stream.set_nodelay(true);
    let io = TokioIo::new(stream);
    let svc = hyper::service::service_fn(move |mut req: http::Request<Incoming>| {
        req.extensions_mut().insert(ConnectInfo(peer));
        let app = app.clone();
        async move { app.oneshot(req).await }
    });
    let mut builder = hyper::server::conn::http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(HEADER_READ_TIMEOUT)
        .max_buf_size(64 * 1024)
        .keep_alive(true);
    let conn = builder.serve_connection(io, svc);
    tokio::pin!(conn);
    let mut closing = false;
    loop {
        tokio::select! {
            res = conn.as_mut() => {
                if let Err(e) = res {
                    tracing::debug!(error = %e, "connection ended with an error");
                }
                break;
            }
            changed = shutdown.changed(), if !closing => {
                closing = true;
                if changed.is_ok() || *shutdown.borrow() {
                    conn.as_mut().graceful_shutdown();
                }
            }
        }
    }
}
