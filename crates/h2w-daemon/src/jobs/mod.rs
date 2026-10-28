//! The jobs engine (ARCHITECTURE.md §10).
//!
//! `queued -> running -> succeeded | partial | failed | cancelled`. A single scheduler
//! starts queued jobs in FIFO order within `jobs.max_concurrent`, one writing job per
//! experiment at a time (later writers of the same experiment wait their turn), install
//! and setup jobs alone (they change the harness2 checkout), and one bridge at a time
//! (bridges are long-running services and do not count against `max_concurrent`).
//!
//! Every non-terminal job has a [`LiveJob`] in memory: the authoritative [`Job`] state, a
//! broadcast channel of [`JobEvent`]s and a ring buffer of recent log lines. Event sends
//! and subscriptions happen under the same lock, so a subscriber's snapshot and its event
//! stream never overlap or miss anything. The database is written at every state change.
//!
//! On startup, jobs left `running` by a previous server become `failed` ("server
//! restarted") and their process groups are terminated when they are positively identified:
//! same boot, and a group member still carrying the job's marker (see [`procfs`]).

pub mod backend;
pub mod container;
pub mod core_backend;
pub mod logs;
pub mod procfs;
mod runner;
pub mod validate;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use h2w_api::{
    ExperimentKind, Isolation, Job, JobEvent, JobKind, JobSpec, JobStatus, LogChunk, LogLine,
    LogStream, Page, Principal,
};
use tokio::sync::{Notify, broadcast};
use tokio_util::sync::CancellationToken;

use self::backend::{ContainerSettings, JobBackend};
use crate::config::{Config, ContainerSection, Paths};
use crate::db::audit::AuditEntry;
use crate::db::{Db, DbResult};
use crate::error::ApiError;
use crate::metrics::Metrics;
use crate::secrets::SecretStore;
use crate::util::{now_rfc3339, now_unix};

/// Recent log lines kept per live job (replayed to reconnecting event streams).
pub const RING_CAPACITY: usize = 1000;
/// Events buffered per subscriber before it is resynchronised with a fresh snapshot.
pub const EVENT_CAPACITY: usize = 1024;
/// Largest `limit` of `GET /jobs/{id}/log`.
pub const LOG_PAGE_MAX: u64 = 5000;

/// Engine settings derived from the configuration.
#[derive(Debug, Clone)]
pub struct JobSettings {
    pub max_concurrent: usize,
    pub max_queued: usize,
    pub timeout: Duration,
    pub budget_poll: Duration,
    /// SIGTERM -> SIGKILL grace on cancellation (10 s in production).
    pub kill_grace: Duration,
    pub log_retention: Duration,
    /// `jobs.max_log_bytes`.
    pub max_log_bytes: u64,
    /// `jobs.min_free_disk_mb` in bytes (0 = no floor).
    pub min_free_disk: u64,
    pub isolation: Isolation,
    pub preacknowledged: bool,
    /// `paths.allow_untested_harness2_rev`.
    pub allow_untested_rev: bool,
    pub paths: Paths,
    pub container: ContainerSection,
    /// `[[benchmarks.not_offered]]`: their jobs are refused.
    pub not_offered: Vec<h2w_core::bench::NotOffered>,
    /// `[judge] cloud`: judge and pipeline jobs with a cloud judge need an acknowledgement or
    /// are refused.
    pub cloud_judges: h2w_api::CloudJudgePolicy,
    /// `[judge] private_hosts` (validated).
    pub judge_private_hosts: Vec<String>,
}

impl JobSettings {
    #[must_use]
    pub fn from_config(cfg: &Config) -> JobSettings {
        let j = &cfg.file.jobs;
        JobSettings {
            max_concurrent: usize::try_from(j.max_concurrent).unwrap_or(2).max(1),
            max_queued: usize::try_from(j.max_queued).unwrap_or(100).max(1),
            timeout: cfg.job_timeout,
            budget_poll: cfg.budget_poll,
            kill_grace: Duration::from_secs(10),
            log_retention: Duration::from_secs(u64::from(j.log_retention_days) * 86_400),
            max_log_bytes: j.max_log_bytes,
            min_free_disk: j.min_free_disk_mb.saturating_mul(1024 * 1024),
            isolation: cfg.file.isolation.mode,
            preacknowledged: cfg.file.isolation.preacknowledge_host_risks,
            allow_untested_rev: cfg.file.paths.allow_untested_harness2_rev,
            paths: cfg.paths.clone(),
            container: cfg.file.isolation.container.clone(),
            not_offered: cfg.not_offered.clone(),
            cloud_judges: cfg.file.judge.cloud,
            judge_private_hosts: cfg.judge_private_hosts.clone(),
        }
    }
}

/// Who asked, for authorization and audit.
#[derive(Debug, Clone)]
pub struct Actor {
    pub principal: Principal,
    pub ip: Option<String>,
    pub request_id: Option<String>,
}

impl Actor {
    /// The server itself (recovery, internal transitions).
    #[must_use]
    pub fn system() -> Actor {
        Actor {
            principal: Principal {
                id: crate::db::audit::SERVER_ACTOR.into(),
                name: crate::db::audit::SERVER_ACTOR.into(),
                role: h2w_api::Role::Admin,
                kind: h2w_api::PrincipalKind::Token,
            },
            ip: None,
            request_id: None,
        }
    }

    pub(crate) fn audit(
        &self,
        action: &str,
        target: &str,
        outcome: h2w_api::AuditOutcome,
    ) -> AuditEntry {
        AuditEntry {
            actor: crate::db::audit::actor_of(&self.principal),
            action: action.to_owned(),
            target: target.to_owned(),
            outcome,
            ip: self.ip.clone(),
            request_id: self.request_id.clone(),
            detail: None,
        }
    }
}

/// Why a running job is being stopped.
#[derive(Debug, Clone, PartialEq)]
pub enum CancelReason {
    /// Cancelled through the API by this principal.
    User(String),
    /// The budget guard saw spend at or above the cap.
    Budget,
    /// The wall-clock limit passed.
    Timeout(Duration),
    /// The server is shutting down.
    Shutdown,
    /// Free space on the data directory fell below `jobs.min_free_disk_mb` (MiB).
    DiskSpace(u64),
}

impl CancelReason {
    #[must_use]
    pub fn status(&self) -> JobStatus {
        match self {
            CancelReason::User(_) | CancelReason::Budget => JobStatus::Cancelled,
            CancelReason::Timeout(_) | CancelReason::Shutdown | CancelReason::DiskSpace(_) => {
                JobStatus::Failed
            }
        }
    }

    #[must_use]
    pub fn message(&self) -> String {
        match self {
            CancelReason::User(who) => format!("cancelled by {who}"),
            CancelReason::Budget => "budget reached".to_owned(),
            CancelReason::Timeout(d) => format!("timed out after {}", human_duration(*d)),
            CancelReason::Shutdown => "server stopped".to_owned(),
            CancelReason::DiskSpace(mb) => {
                format!("stopped: less than {mb} MiB free on the data directory")
            }
        }
    }
}

/// `1h 30m`, `45s`.
#[must_use]
pub fn human_duration(d: Duration) -> String {
    let s = d.as_secs();
    match (s / 3600, (s % 3600) / 60, s % 60) {
        (0, 0, sec) => format!("{sec}s"),
        (0, m, 0) => format!("{m}m"),
        (0, m, sec) => format!("{m}m {sec}s"),
        (h, 0, _) => format!("{h}h"),
        (h, m, _) => format!("{h}h {m}m"),
    }
}

struct LiveState {
    job: Job,
    tx: broadcast::Sender<JobEvent>,
    ring: VecDeque<LogLine>,
}

/// A queued or running job.
pub struct LiveJob {
    pub id: String,
    pub seq: i64,
    pub spec: JobSpec,
    state: Mutex<LiveState>,
    cancel: CancellationToken,
    reason: Mutex<Option<CancelReason>>,
}

impl std::fmt::Debug for LiveJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveJob")
            .field("id", &self.id)
            .field("seq", &self.seq)
            .finish_non_exhaustive()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

impl LiveJob {
    fn new(seq: i64, job: Job) -> LiveJob {
        let (tx, _) = broadcast::channel(EVENT_CAPACITY);
        LiveJob {
            id: job.id.clone(),
            seq,
            spec: job.spec.clone(),
            state: Mutex::new(LiveState {
                job,
                tx,
                ring: VecDeque::new(),
            }),
            cancel: CancellationToken::new(),
            reason: Mutex::new(None),
        }
    }

    /// The current state.
    #[must_use]
    pub fn snapshot(&self) -> Job {
        lock(&self.state).job.clone()
    }

    #[must_use]
    pub fn status(&self) -> JobStatus {
        lock(&self.state).job.status
    }

    /// Apply `f` and broadcast the events it returns, atomically with respect to
    /// subscribers.
    pub(crate) fn mutate(&self, f: impl FnOnce(&mut Job) -> Vec<JobEvent>) {
        let mut st = lock(&self.state);
        for ev in f(&mut st.job) {
            let _ = st.tx.send(ev);
        }
    }

    /// Record one log line (already redacted and masked).
    pub(crate) fn push_log(&self, line: LogLine) {
        let mut st = lock(&self.state);
        st.job.log_lines = line.n + 1;
        if st.ring.len() >= RING_CAPACITY {
            st.ring.pop_front();
        }
        st.ring.push_back(line.clone());
        let _ = st.tx.send(JobEvent::Log { line });
    }

    fn subscribe(&self) -> (Job, broadcast::Receiver<JobEvent>, Vec<LogLine>) {
        let st = lock(&self.state);
        (
            st.job.clone(),
            st.tx.subscribe(),
            st.ring.iter().cloned().collect(),
        )
    }

    /// Ask a running job to stop (the first reason wins).
    pub fn request_cancel(&self, reason: CancelReason) {
        {
            let mut r = lock(&self.reason);
            if r.is_none() {
                *r = Some(reason);
            }
        }
        self.cancel.cancel();
    }

    #[must_use]
    pub fn cancel_reason(&self) -> Option<CancelReason> {
        lock(&self.reason).clone()
    }
}

/// What an event-stream subscriber starts from.
pub struct Subscription {
    pub snapshot: Job,
    /// `None` when the job is already terminal.
    pub events: Option<broadcast::Receiver<JobEvent>>,
    /// Recent lines (for `Last-Event-ID` replay).
    pub recent: Vec<LogLine>,
}

/// Filters for listing jobs.
pub type JobFilter = crate::db::jobs::JobFilter;

/// The jobs engine.
pub struct JobManager {
    db: Db,
    settings: JobSettings,
    backend: Arc<dyn JobBackend>,
    secrets: SecretStore,
    metrics: Arc<Metrics>,
    /// Read permits and the sweep cache, shared with the HTTP layer.
    reads: Arc<crate::reads::Reads>,
    /// Per-principal share of refusal records, shared with the HTTP layer's `access.denied`.
    denial_audits: Arc<crate::ratelimit::PrincipalLimiter>,
    live: Mutex<HashMap<String, Arc<LiveJob>>>,
    sched: tokio::sync::Mutex<()>,
    wake: Notify,
    tasks: Mutex<HashMap<String, tokio::task::JoinHandle<()>>>,
    shutting_down: AtomicBool,
    scheduler_alive: AtomicBool,
    container_settings: tokio::sync::Mutex<Option<ContainerSettings>>,
}

impl std::fmt::Debug for JobManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobManager")
            .field("settings", &self.settings)
            .finish_non_exhaustive()
    }
}

impl JobManager {
    #[must_use]
    pub fn new(
        db: Db,
        settings: JobSettings,
        backend: Arc<dyn JobBackend>,
        secrets: SecretStore,
        metrics: Arc<Metrics>,
        reads: Arc<crate::reads::Reads>,
        denial_audits: Arc<crate::ratelimit::PrincipalLimiter>,
    ) -> Arc<JobManager> {
        Arc::new(JobManager {
            db,
            settings,
            backend,
            secrets,
            metrics,
            reads,
            denial_audits,
            live: Mutex::new(HashMap::new()),
            sched: tokio::sync::Mutex::new(()),
            wake: Notify::new(),
            tasks: Mutex::new(HashMap::new()),
            shutting_down: AtomicBool::new(false),
            scheduler_alive: AtomicBool::new(false),
            container_settings: tokio::sync::Mutex::new(None),
        })
    }

    #[must_use]
    pub fn settings(&self) -> &JobSettings {
        &self.settings
    }

    /// Recover from a previous run, then start the scheduler and housekeeping.
    ///
    /// # Errors
    /// Database errors during recovery.
    pub async fn start(self: &Arc<Self>) -> DbResult<()> {
        self.recover().await?;
        self.scheduler_alive.store(true, Ordering::SeqCst);
        let me = Arc::clone(self);
        tokio::spawn(async move { me.scheduler_loop().await });
        let me = Arc::clone(self);
        tokio::spawn(async move { me.retention_loop().await });
        Ok(())
    }

    /// The scheduler is running (readiness).
    #[must_use]
    pub fn is_alive(&self) -> bool {
        self.scheduler_alive.load(Ordering::SeqCst)
    }

    /// Jobs running now.
    #[must_use]
    pub fn running_count(&self) -> u32 {
        let live = lock(&self.live);
        u32::try_from(
            live.values()
                .filter(|j| j.status() == JobStatus::Running)
                .count(),
        )
        .unwrap_or(u32::MAX)
    }

    /// `<harness2_home>/.venv/bin/harness2` exists.
    #[must_use]
    pub fn harness2_installed(&self) -> bool {
        self.settings
            .paths
            .harness2_home
            .join(".venv/bin")
            .join(h2w_core::cli::Program::Harness2.bin_name())
            .is_file()
    }

    fn live_get(&self, id: &str) -> Option<Arc<LiveJob>> {
        lock(&self.live).get(id).cloned()
    }

    async fn persist(&self, job: Job) {
        let id = job.id.clone();
        if let Err(e) = self
            .db
            .call(move |c| crate::db::jobs::save_state(c, &job))
            .await
        {
            tracing::error!(job = %id, error = %e, "could not save job state");
        }
    }

    /// Record a refused job request within the caller's per-principal share of refusal
    /// records (the rest are only counted), so a principal cannot flood the audit log with
    /// requests it is not allowed to make.
    async fn audit_refusal(&self, actor: &Actor, entry: AuditEntry) {
        let key = crate::http::extract::principal_key(&actor.principal);
        if self.denial_audits.check_principal(&key).is_ok() {
            self.audit(entry).await;
        } else {
            self.metrics.audit_suppressed.inc();
        }
    }

    pub(crate) async fn audit(&self, entry: AuditEntry) {
        if let Err(e) = self
            .db
            .call(move |c| crate::db::audit::append_tx(c, &entry))
            .await
        {
            tracing::error!(error = %e, "could not append to the audit log");
        }
    }

    /// Settings for container jobs, resolving the user on first use.
    async fn container_settings(&self) -> Result<ContainerSettings, String> {
        let mut cached = self.container_settings.lock().await;
        if let Some(c) = cached.as_ref() {
            return Ok(c.clone());
        }
        let c = &self.settings.container;
        let user = container::resolve_user(&container::SystemRunner, c.runtime, &c.user).await?;
        let s = ContainerSettings {
            runtime: c.runtime,
            image: c.image.clone(),
            network: c.network.clone(),
            cpus: c.cpus,
            memory_mb: c.memory_mb,
            pids_limit: c.pids_limit,
            user,
            proxy: Some(c.proxy.clone()).filter(|p| !p.is_empty()),
        };
        *cached = Some(s.clone());
        Ok(s)
    }

    // -----------------------------------------------------------------------------------
    // Creating jobs
    // -----------------------------------------------------------------------------------

    /// Validate and enqueue a job (ARCHITECTURE.md §8 "Job validation"), auditing the
    /// outcome either way.
    ///
    /// # Errors
    /// 403, 409, 422 or 503 as the contract specifies.
    pub async fn create(&self, spec: JobSpec, actor: &Actor) -> Result<Job, ApiError> {
        let kind = spec.kind();
        let target = spec.experiment().map_or_else(
            || kind.as_str().to_owned(),
            |e| format!("{}:{e}", kind.as_str()),
        );
        let synthetic = validate::cloud_judge_acknowledged(&spec) == Some(true);
        let result = self.create_inner(spec, actor).await;
        let entry = match &result {
            Ok(job) => actor
                .audit("job.create", &job.id, h2w_api::AuditOutcome::Success)
                .detail(if synthetic {
                    format!("{target}; acknowledge_cloud_judge: data is generated or synthetic")
                } else {
                    target
                }),
            Err(e) => {
                let entry = actor
                    .audit("job.create", &target, h2w_api::AuditOutcome::Denied)
                    .detail(e.message.clone());
                self.audit_refusal(actor, entry).await;
                return result;
            }
        };
        self.audit(entry).await;
        result
    }

    #[allow(clippy::too_many_lines)] // one linear checklist, mirroring the contract
    async fn create_inner(&self, spec: JobSpec, actor: &Actor) -> Result<Job, ApiError> {
        let kind = spec.kind();
        let need = spec.required_role();
        if !actor.principal.role.allows(need) {
            return Err(ApiError::forbidden(format!(
                "starting a job of kind {} requires the {} role",
                kind.as_str(),
                need.as_str()
            )));
        }
        let mut problems = validate::structural(&spec);
        problems.extend(self.backend.check_names(&spec));
        if !problems.is_empty() {
            return Err(ApiError::validation(problems));
        }

        let isolation = self.settings.isolation;
        // The same rule as `GET /benchmarks` shows: an administrator's `not_offered` entry,
        // or WorkBuddy-Bench under container isolation.
        if let Some(why) = validate::bench_of(&spec)
            .and_then(|b| h2w_core::bench::unavailable(b, isolation, &self.settings.not_offered))
        {
            return Err(ApiError::conflict(why.message));
        }
        if isolation == Isolation::Container && matches!(spec, JobSpec::Bridge(_)) {
            return Err(ApiError::conflict(
                "The Codex bridge needs host isolation in this version",
            ));
        }
        if isolation == Isolation::Host
            && validate::acknowledged(&spec) == Some(false)
            && !self.settings.preacknowledged
        {
            return Err(ApiError::invalid(
                "params.acknowledge_risks",
                "jobs run on the server host without isolation (the executor gets the server \
                 user's files and network); set acknowledge_risks to true to proceed",
            ));
        }
        if let JobSpec::InstallHarness2(p) = &spec {
            if !self.settings.paths.harness2_managed {
                return Err(ApiError::conflict(
                    "harness2_home is an existing checkout (paths.harness2_managed = false); \
                     the server does not modify it",
                ));
            }
            if let Some(rev) = &p.rev
                && rev != h2w_core::TESTED_HARNESS2_COMMIT
                && !self.settings.allow_untested_rev
            {
                return Err(ApiError::invalid(
                    "params.rev",
                    format!(
                        "only the tested harness2 commit {} can be installed; an \
                         administrator can allow others with \
                         paths.allow_untested_harness2_rev = true",
                        h2w_core::TESTED_HARNESS2_COMMIT
                    ),
                ));
            }
        } else if !self.harness2_installed() {
            return Err(ApiError::unavailable(
                "harness2 is not installed; an admin can run the install_harness2 job",
            ));
        }
        // Hermes Agent and Turnstone need a harness2 that adds them (`Harness2Status`).
        if let Some((field, sub)) = validate::substrate_of(&spec).filter(|(_, s)| !s.upstream()) {
            let paths = &self.settings.paths;
            let supported =
                h2w_core::install::read_status(&paths.harness2_home, paths.harness2_managed)
                    .substrates;
            if !supported.contains(&sub) {
                return Err(ApiError::invalid(
                    field,
                    format!(
                        "the installed harness2 has no {} executor: it needs a harness2 that adds \
                         it (docs/notes/substrates.md)",
                        sub.display_name()
                    ),
                ));
            }
        }

        // The experiment.
        let mut experiment = None;
        if let Some(name) = spec.experiment() {
            let n = name.to_owned();
            let rec = self
                .db
                .call(move |c| crate::db::experiments::get(c, &n))
                .await?;
            let rec = match rec {
                Some(r) => r,
                None if matches!(spec, JobSpec::Demo(_)) => {
                    self.create_demo_experiment(name, actor).await?
                }
                None => {
                    return Err(ApiError::invalid(
                        "params.experiment",
                        format!("no experiment named {name}; create it first"),
                    ));
                }
            };
            let is_demo = rec.kind == ExperimentKind::Demo;
            if matches!(spec, JobSpec::Demo(_)) && !is_demo {
                return Err(ApiError::conflict(format!(
                    "{name} is not a demo experiment; the demo writes its own layout"
                )));
            }
            if !matches!(spec, JobSpec::Demo(_)) && is_demo {
                return Err(ApiError::conflict(format!(
                    "{name} is a demo experiment; benchmark jobs need a standard experiment"
                )));
            }
            if rec.archived && validate::writes_experiment(&spec) {
                return Err(ApiError::conflict(format!("{name} is archived")));
            }
            if let (Some(p), Some(pinned)) = (validate::profile_of(&spec), rec.profile.as_deref())
                && p != pinned
            {
                return Err(ApiError::conflict(format!(
                    "experiment {name} is pinned to profile {pinned}; harness2 refuses to \
                     resume or judge a sweep under different models"
                )));
            }
            experiment = Some(rec);
        }

        // The profile and its secrets.
        let mut profile = None;
        if let Some(pn) = validate::profile_of(&spec) {
            let n = pn.to_owned();
            let p = self
                .db
                .call(move |c| crate::db::profiles::get(c, &n))
                .await?
                .ok_or_else(|| {
                    ApiError::invalid("params.profile", format!("no profile named {pn}"))
                })?;
            // A profile stored before a rule tightened (a newly reserved environment name,
            // for example) is refused here, not accepted and failed while the job prepares.
            if let Err(e) = h2w_core::env::validate_profile(&p) {
                return Err(ApiError::invalid(
                    "params.profile",
                    format!(
                        "profile {pn} no longer passes validation ({e}); an administrator must update it"
                    ),
                ));
            }
            let mut missing = Vec::new();
            for s in crate::db::profiles::referenced_secrets(&p) {
                if self.secrets.get(&s).await?.is_none() {
                    missing.push(h2w_api::FieldProblem {
                        field: "params.profile".into(),
                        problem: format!("profile {pn} needs secret {s}, which is not set"),
                    });
                }
            }
            if !missing.is_empty() {
                return Err(ApiError::validation(missing));
            }
            // Hermes Agent and Turnstone call an OpenAI-compatible endpoint: the profile's own, or
            // one its extra environment names.
            if matches!(spec, JobSpec::Run(_) | JobSpec::Pipeline(_))
                && let Some(c) = validate::cell_of(&spec).filter(|c| c.substrate.lab_only())
            {
                let var = if c.substrate == h2w_api::Substrate::Hm {
                    "HARNESS2_HERMES_BASE_URL"
                } else {
                    "HARNESS2_TURNSTONE_BASE_URL"
                };
                let endpoint = (p.kind == h2w_api::ProfileKind::OpenaiCompatible
                    && p.endpoint.is_some())
                    || p.extra_env.iter().any(|e| e.name == var);
                if !endpoint {
                    return Err(ApiError::invalid(
                        "params.profile",
                        format!(
                            "{} needs an OpenAI-compatible endpoint: use an openai_compatible \
                             profile, or set {var} in profile {pn}'s extra environment",
                            c.substrate.display_name()
                        ),
                    ));
                }
            }
            // A judge that is not local sees the experiment's tasks and outputs: the caller
            // confirms they are generated or synthetic, or the policy refuses the job.
            if let Some(acknowledged) = validate::cloud_judge_acknowledged(&spec)
                && let Some(site) = h2w_api::judge_site(&p, &self.settings.judge_private_hosts)
                && site.needs_acknowledgement()
            {
                if self.settings.cloud_judges == h2w_api::CloudJudgePolicy::Refuse {
                    return Err(ApiError::conflict(format!(
                        "this server refuses cloud judges ([judge] cloud = \"refuse\") and \
                         profile {pn}'s judge is not local ({})",
                        site.describe()
                    )));
                }
                if !acknowledged {
                    return Err(ApiError::invalid(
                        "params.acknowledge_cloud_judge",
                        format!(
                            "{} Set acknowledge_cloud_judge to true to confirm that this \
                             experiment's tasks and outputs are generated or synthetic data.",
                            h2w_api::cloud_judge_warning(&site)
                        ),
                    ));
                }
            }
            profile = Some(p);
        }
        if let JobSpec::Bridge(b) = &spec
            && b.model.is_none()
            && profile.as_ref().is_none_or(|p| p.codex_model.is_none())
        {
            return Err(ApiError::invalid(
                "params.model",
                "give a model or set codex_model on the profile",
            ));
        }

        // Split files.
        if let Some(opts) = validate::options_of(&spec)
            && let Some(sf) = &opts.split_file
        {
            let n = sf.clone();
            let split = self
                .db
                .call(move |c| crate::db::splits::get(c, &n))
                .await?
                .ok_or_else(|| {
                    ApiError::invalid("params.options.split_file", format!("no split named {sf}"))
                })?;
            if let Some(cell) = validate::cell_of(&spec)
                && (split.bench != cell.bench || split.subset != cell.subset)
            {
                return Err(ApiError::invalid(
                    "params.options.split_file",
                    format!("split {sf} was made for another benchmark"),
                ));
            }
        }
        if let JobSpec::Split(s) = &spec {
            let n = s.name.clone();
            let exists = self
                .db
                .call(move |c| crate::db::splits::get(c, &n))
                .await?
                .is_some()
                || self
                    .settings
                    .paths
                    .splits_dir
                    .join(format!("{}.json", s.name))
                    .exists();
            if exists {
                return Err(ApiError::conflict(format!(
                    "a split named {} already exists",
                    s.name
                )));
            }
        }

        // Queue capacity.
        let queued = lock(&self.live)
            .values()
            .filter(|j| j.status() == JobStatus::Queued)
            .count();
        if queued >= self.settings.max_queued {
            return Err(ApiError::unavailable(format!(
                "the job queue is full ({queued} jobs waiting); try again later"
            )));
        }

        // Pin the profile on the first writing job of an experiment that has none.
        if let (Some(rec), Some(p)) = (&experiment, &profile)
            && rec.profile.is_none()
            && validate::writes_experiment(&spec)
        {
            let (en, pn) = (rec.name.clone(), p.name.clone());
            self.db
                .call(move |c| crate::db::experiments::pin_profile(c, &en, &pn))
                .await?;
        }

        let id = uuid::Uuid::now_v7().to_string();
        let job = Job {
            id: id.clone(),
            kind,
            spec,
            status: JobStatus::Queued,
            created_by: actor.principal.name.clone(),
            created_at: now_rfc3339(),
            started_at: None,
            finished_at: None,
            exit_code: None,
            isolation,
            steps: Vec::new(),
            progress: None,
            result: None,
            error: None,
            log_lines: 0,
            spent_usd: None,
        };
        let j = job.clone();
        let seq = self
            .db
            .call(move |c| crate::db::jobs::insert(c, &j))
            .await?;
        lock(&self.live).insert(id, Arc::new(LiveJob::new(seq, job.clone())));
        self.wake.notify_one();
        Ok(job)
    }

    async fn create_demo_experiment(
        &self,
        name: &str,
        actor: &Actor,
    ) -> Result<crate::db::experiments::ExperimentRecord, ApiError> {
        let root = self.settings.paths.experiments_dir.clone();
        let n = name.to_owned();
        tokio::task::spawn_blocking(move || crate::expdirs::create_or_adopt(&root, &n))
            .await?
            .map_err(|e| ApiError::conflict(e.to_string()))?;
        let rec = crate::db::experiments::ExperimentRecord {
            name: name.to_owned(),
            kind: ExperimentKind::Demo,
            description: "Created by the harness2 demo".to_owned(),
            tags: vec!["demo".to_owned()],
            profile: None,
            created_at: now_unix(),
            created_by: actor.principal.name.clone(),
            archived: false,
        };
        let r = rec.clone();
        let entry = actor
            .audit("experiment.create", name, h2w_api::AuditOutcome::Success)
            .detail("demo experiment created for a demo job");
        self.db
            .call(move |c| {
                crate::db::write_tx(c, |tx| {
                    crate::db::experiments::insert(tx, &r)?;
                    crate::db::audit::append(tx, &entry)?;
                    Ok(())
                })
            })
            .await
            .map_err(|e| match e {
                crate::db::DbError::Sqlite(_) => {
                    ApiError::conflict(format!("experiment {name} was created concurrently"))
                }
                other => other.into(),
            })?;
        Ok(rec)
    }

    // -----------------------------------------------------------------------------------
    // Reading jobs
    // -----------------------------------------------------------------------------------

    /// One job (live state for queued/running jobs).
    ///
    /// # Errors
    /// 404 for unknown ids.
    pub async fn get(&self, id: &str) -> Result<Job, ApiError> {
        if let Some(l) = self.live_get(id) {
            return Ok(l.snapshot());
        }
        let n = id.to_owned();
        self.db
            .call(move |c| crate::db::jobs::get(c, &n))
            .await?
            .map(|r| r.job)
            .ok_or_else(|| ApiError::not_found(format!("no job {id}")))
    }

    /// A page of jobs, newest first.
    ///
    /// # Errors
    /// Database errors; 400 for a bad cursor.
    pub async fn list(
        &self,
        filter: JobFilter,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<Page<Job>, ApiError> {
        let before = match cursor {
            Some(c) => Some(
                crate::db::cursor::decode(c)
                    .ok_or_else(|| ApiError::bad_request("invalid cursor"))?,
            ),
            None => None,
        };
        let page = self
            .db
            .call(move |c| crate::db::jobs::list(c, &filter, limit, before))
            .await?;
        let items = page
            .items
            .into_iter()
            .map(|r| match self.live_get(&r.job.id) {
                Some(l) => l.snapshot(),
                None => r.job,
            })
            .collect();
        Ok(Page {
            items,
            next_cursor: page.next_cursor,
        })
    }

    /// A page of a job's log.
    ///
    /// # Errors
    /// 404 for unknown jobs.
    pub async fn log(&self, id: &str, from: u64, limit: u64) -> Result<LogChunk, ApiError> {
        let job = self.get(id).await?;
        let path = logs::log_path(&self.settings.paths.job_logs_dir, &job.id);
        let limit = limit.clamp(1, LOG_PAGE_MAX);
        // Under a read permit, and at most `logs::PAGE_BYTES` of lines per page.
        let page = self
            .reads
            .run(move |_| {
                logs::read_page(&path, from, limit, logs::PAGE_BYTES).map_err(|e| {
                    tracing::error!(error = %e, "could not read a job log");
                    ApiError::internal("could not read the job log")
                })
            })
            .await?;
        // A terminal job's log ends at the end of its file (also when retention deleted it).
        let complete = job.status.is_terminal() && (page.next >= job.log_lines || page.eof);
        Ok(LogChunk {
            lines: page.lines,
            next: page.next,
            complete,
        })
    }

    /// Snapshot and live events of a job.
    ///
    /// # Errors
    /// 404 for unknown jobs.
    pub async fn subscribe(&self, id: &str) -> Result<Subscription, ApiError> {
        if let Some(l) = self.live_get(id) {
            let (snapshot, rx, recent) = l.subscribe();
            return Ok(Subscription {
                snapshot,
                events: Some(rx),
                recent,
            });
        }
        let job = self.get(id).await?;
        Ok(Subscription {
            snapshot: job,
            events: None,
            recent: Vec::new(),
        })
    }

    /// Lines `from..to` for an event-stream replay (bounded).
    pub(crate) async fn replay(&self, id: &str, from: u64, to: u64) -> Vec<LogLine> {
        if to <= from {
            return Vec::new();
        }
        let path = logs::log_path(&self.settings.paths.job_logs_dir, id);
        let limit = (to - from).min(LOG_PAGE_MAX);
        // At most one page: the client pages the rest (`GET /jobs/{id}/log`).
        self.reads
            .run(move |_| {
                logs::read_page(&path, from, limit, logs::PAGE_BYTES)
                    .map_err(|e| ApiError::internal(e.to_string()))
            })
            .await
            .map(|p| p.lines)
            .unwrap_or_default()
    }

    // -----------------------------------------------------------------------------------
    // Cancelling
    // -----------------------------------------------------------------------------------

    /// Cancel a queued job at once, or ask a running one to stop (SIGTERM to its process
    /// group, SIGKILL after the grace period). Needs the role that could start the job.
    ///
    /// # Errors
    /// 404 unknown, 409 terminal, 403 role.
    pub async fn cancel(&self, id: &str, actor: &Actor) -> Result<Job, ApiError> {
        let result = self.cancel_inner(id, actor).await;
        let entry = match &result {
            Ok(_) => actor.audit("job.cancel", id, h2w_api::AuditOutcome::Success),
            Err(e) => {
                let entry = actor
                    .audit("job.cancel", id, h2w_api::AuditOutcome::Denied)
                    .detail(e.message.clone());
                self.audit_refusal(actor, entry).await;
                return result;
            }
        };
        self.audit(entry).await;
        result
    }

    async fn cancel_inner(&self, id: &str, actor: &Actor) -> Result<Job, ApiError> {
        let _sched = self.sched.lock().await;
        let Some(live) = self.live_get(id) else {
            let job = self.get(id).await?;
            return Err(ApiError::conflict(format!(
                "job {} already finished ({})",
                job.id,
                job.status.as_str()
            )));
        };
        let need = live.spec.required_role().max(h2w_api::Role::Operator);
        if !actor.principal.role.allows(need) {
            return Err(ApiError::forbidden(format!(
                "cancelling a job of kind {} requires the {} role",
                live.spec.kind().as_str(),
                need.as_str()
            )));
        }
        let reason = CancelReason::User(actor.principal.name.clone());
        match live.status() {
            JobStatus::Queued => {
                let n = id.to_owned();
                let now = now_unix();
                let moved = self
                    .db
                    .call(move |c| {
                        crate::db::jobs::transition(
                            c,
                            &n,
                            JobStatus::Queued,
                            JobStatus::Cancelled,
                            now,
                        )
                    })
                    .await?;
                if !moved {
                    return Err(ApiError::conflict("the job changed state; try again"));
                }
                let msg = reason.message();
                live.mutate(|job| {
                    job.status = JobStatus::Cancelled;
                    job.finished_at = Some(crate::util::rfc3339(now));
                    job.error = Some(msg.clone());
                    vec![JobEvent::Status {
                        status: JobStatus::Cancelled,
                        exit_code: None,
                        error: Some(msg),
                    }]
                });
                let job = live.snapshot();
                self.persist(job.clone()).await;
                lock(&self.live).remove(id);
                self.metrics
                    .job_finished(job.kind.as_str(), JobStatus::Cancelled.as_str(), 0.0);
                Ok(job)
            }
            JobStatus::Running => {
                live.request_cancel(reason);
                Ok(live.snapshot())
            }
            other => Err(ApiError::conflict(format!(
                "job {id} already finished ({})",
                other.as_str()
            ))),
        }
    }

    // -----------------------------------------------------------------------------------
    // Scheduling
    // -----------------------------------------------------------------------------------

    async fn scheduler_loop(self: Arc<Self>) {
        loop {
            if self.shutting_down.load(Ordering::SeqCst) {
                break;
            }
            self.schedule_once().await;
            tokio::select! {
                () = self.wake.notified() => {}
                () = tokio::time::sleep(Duration::from_secs(5)) => {}
            }
        }
        self.scheduler_alive.store(false, Ordering::SeqCst);
    }

    /// Start every queued job the rules allow, oldest first.
    pub(crate) async fn schedule_once(self: &Arc<Self>) {
        let _sched = self.sched.lock().await;
        if self.shutting_down.load(Ordering::SeqCst) {
            return;
        }
        let mut jobs: Vec<Arc<LiveJob>> = lock(&self.live).values().cloned().collect();
        jobs.sort_by_key(|j| j.seq);
        let running: Vec<&Arc<LiveJob>> = jobs
            .iter()
            .filter(|j| j.status() == JobStatus::Running)
            .collect();
        let is_bridge = |j: &LiveJob| matches!(j.spec, JobSpec::Bridge(_));
        let mut slots_used = running.iter().filter(|j| !is_bridge(j)).count();
        let mut bridge_running = running.iter().any(|j| is_bridge(j));
        let exclusive_running = running.iter().any(|j| validate::is_exclusive(&j.spec));
        if exclusive_running {
            return;
        }
        let mut writers: HashSet<String> = running
            .iter()
            .filter(|j| validate::writes_experiment(&j.spec))
            .filter_map(|j| j.spec.experiment().map(str::to_owned))
            .collect();
        let mut waiting_writers: HashSet<String> = HashSet::new();
        let mut barrier = false;
        for job in jobs.iter().filter(|j| j.status() == JobStatus::Queued) {
            if validate::is_exclusive(&job.spec) {
                if slots_used == 0 && !barrier {
                    if self.start_job(job).await {
                        return; // nothing else may start next to it
                    }
                } else {
                    barrier = true;
                }
                continue;
            }
            if is_bridge(job) {
                if !bridge_running && !barrier && self.start_job(job).await {
                    bridge_running = true;
                }
                continue;
            }
            if barrier || slots_used >= self.settings.max_concurrent {
                continue;
            }
            if validate::writes_experiment(&job.spec)
                && let Some(exp) = job.spec.experiment()
            {
                if writers.contains(exp) || waiting_writers.contains(exp) {
                    waiting_writers.insert(exp.to_owned());
                    continue;
                }
                if self.start_job(job).await {
                    writers.insert(exp.to_owned());
                    slots_used += 1;
                }
                continue;
            }
            if self.start_job(job).await {
                slots_used += 1;
            }
        }
    }

    async fn start_job(self: &Arc<Self>, live: &Arc<LiveJob>) -> bool {
        let id = live.id.clone();
        let now = now_unix();
        let moved = self
            .db
            .call(move |c| {
                crate::db::jobs::transition(c, &id, JobStatus::Queued, JobStatus::Running, now)
            })
            .await;
        match moved {
            Ok(true) => {}
            Ok(false) => return false,
            Err(e) => {
                tracing::error!(job = %live.id, error = %e, "could not start job");
                return false;
            }
        }
        live.mutate(|job| {
            job.status = JobStatus::Running;
            job.started_at = Some(crate::util::rfc3339(now));
            vec![JobEvent::Status {
                status: JobStatus::Running,
                exit_code: None,
                error: None,
            }]
        });
        self.metrics
            .jobs_running
            .set(i64::from(self.running_count()));
        let me = Arc::clone(self);
        let l = Arc::clone(live);
        let runner = tokio::spawn(async move { runner::run(me, l).await });
        // A panicking runner must not leave its job `running` (holding a slot) forever.
        let me = Arc::clone(self);
        let l = Arc::clone(live);
        let supervisor = tokio::spawn(async move {
            if let Err(e) = runner.await
                && e.is_panic()
            {
                tracing::error!(job = %l.id, "the job runner panicked");
                me.fail_after_panic(&l).await;
            }
        });
        lock(&self.tasks).insert(live.id.clone(), supervisor);
        true
    }

    async fn fail_after_panic(&self, live: &Arc<LiveJob>) {
        let id = live.id.clone();
        let row = self
            .db
            .call(move |c| crate::db::jobs::get(c, &id))
            .await
            .ok()
            .flatten();
        if let Some(rec) = row.as_ref().and_then(recorded_process) {
            procfs::terminate_recorded_group(&rec, self.settings.kill_grace).await;
        }
        let now = now_rfc3339();
        live.mutate(|job| {
            job.status = JobStatus::Failed;
            job.error = Some("internal error in the job runner (see the server log)".to_owned());
            job.finished_at = Some(now.clone());
            let mut events: Vec<JobEvent> = Vec::new();
            for s in &mut job.steps {
                if !s.status.is_terminal() {
                    s.status = JobStatus::Failed;
                    s.finished_at = Some(now.clone());
                    events.push(JobEvent::Step { step: s.clone() });
                }
            }
            events.push(JobEvent::Status {
                status: JobStatus::Failed,
                exit_code: None,
                error: job.error.clone(),
            });
            events
        });
        let job = live.snapshot();
        self.persist(job.clone()).await;
        let _ = tokio::fs::remove_dir_all(self.job_dir(&live.id)).await;
        self.metrics
            .job_finished(job.kind.as_str(), JobStatus::Failed.as_str(), 0.0);
        self.job_done(&live.id);
    }

    /// Called by the runner when a job ended: state is already final and persisted.
    pub(crate) fn job_done(&self, id: &str) {
        lock(&self.live).remove(id);
        lock(&self.tasks).remove(id);
        self.metrics
            .jobs_running
            .set(i64::from(self.running_count()));
        self.wake.notify_one();
    }

    // -----------------------------------------------------------------------------------
    // Recovery, retention, shutdown
    // -----------------------------------------------------------------------------------

    async fn recover(&self) -> DbResult<()> {
        let running = self
            .db
            .call(|c| crate::db::jobs::with_status(c, JobStatus::Running))
            .await?;
        for row in running {
            let id = row.job.id.clone();
            let mut notes = Vec::new();
            if row.in_container {
                let runtime = self.settings.container.runtime;
                match container::verify_and_stop(&container::SystemRunner, runtime, &id).await {
                    Ok(true) => notes.push("stopped its container".to_owned()),
                    Ok(false) => {}
                    Err(e) => notes.push(format!("container not stopped: {e}")),
                }
            }
            if row.pid.is_some() {
                let note = match recorded_process(&row) {
                    Some(rec) => {
                        match procfs::terminate_recorded_group(&rec, self.settings.kill_grace).await
                        {
                            procfs::Termination::Signalled => {
                                Some(format!("terminated its process group {}", rec.pid))
                            }
                            procfs::Termination::OtherBoot => Some(
                                "its processes were recorded before a reboot; nothing signalled"
                                    .to_owned(),
                            ),
                            procfs::Termination::NotFound => None,
                        }
                    }
                    None => Some(
                        "its processes could not be identified (incomplete record); nothing \
                         signalled"
                            .to_owned(),
                    ),
                };
                notes.extend(note);
            }
            let mut job = row.job;
            let log_path = logs::log_path(&self.settings.paths.job_logs_dir, &id);
            let (mut file, existing) = match logs::LogFile::open(&log_path).await {
                Ok(f) => (Some(f.0), f.1),
                Err(_) => (None, job.log_lines),
            };
            let mut n = existing;
            let mut text = "server restarted while this job was running; marked failed".to_owned();
            if !notes.is_empty() {
                text.push_str(&format!(" ({})", notes.join("; ")));
            }
            if let Some(f) = file.as_mut() {
                let _ = f
                    .append(&LogLine {
                        n,
                        ts: crate::util::now_rfc3339_millis(),
                        stream: LogStream::System,
                        text,
                    })
                    .await;
                let _ = f.flush().await;
                n += 1;
            }
            let now = now_rfc3339();
            job.status = JobStatus::Failed;
            job.error = Some("server restarted".to_owned());
            job.finished_at = Some(now.clone());
            job.log_lines = n;
            for step in &mut job.steps {
                if !step.status.is_terminal() {
                    step.status = if step.status == JobStatus::Running {
                        JobStatus::Failed
                    } else {
                        JobStatus::Cancelled
                    };
                    step.finished_at = Some(now.clone());
                }
            }
            let j = job.clone();
            self.db
                .call(move |c| {
                    crate::db::jobs::save_state(c, &j)?;
                    crate::db::jobs::set_process(c, &j.id, None)
                })
                .await?;
            self.metrics
                .job_finished(job.kind.as_str(), JobStatus::Failed.as_str(), 0.0);
            tracing::warn!(job = %id, "job was running when the server stopped; marked failed");
            let dir = self.settings.paths.job_files_dir.join(&id);
            let _ = tokio::fs::remove_dir_all(dir).await;
        }
        let queued = self
            .db
            .call(|c| crate::db::jobs::with_status(c, JobStatus::Queued))
            .await?;
        let mut keep = HashSet::new();
        {
            let mut live = lock(&self.live);
            for row in queued {
                keep.insert(row.job.id.clone());
                live.insert(row.job.id.clone(), Arc::new(LiveJob::new(row.seq, row.job)));
            }
        }
        // Leftover per-job directories of jobs that are not queued any more.
        if let Ok(mut rd) = tokio::fs::read_dir(&self.settings.paths.job_files_dir).await {
            while let Ok(Some(e)) = rd.next_entry().await {
                let name = e.file_name().to_string_lossy().into_owned();
                if !keep.contains(&name) {
                    let _ = tokio::fs::remove_dir_all(e.path()).await;
                }
            }
        }
        Ok(())
    }

    async fn retention_loop(self: Arc<Self>) {
        loop {
            if self.shutting_down.load(Ordering::SeqCst) {
                return;
            }
            let secs = i64::try_from(self.settings.log_retention.as_secs()).unwrap_or(i64::MAX);
            let cutoff = now_unix().saturating_sub(secs);
            match self
                .db
                .call(move |c| crate::db::jobs::logs_to_purge(c, cutoff))
                .await
            {
                Ok(ids) => {
                    for id in ids {
                        let p = logs::log_path(&self.settings.paths.job_logs_dir, &id);
                        match tokio::fs::remove_file(&p).await {
                            Ok(()) => {}
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                            Err(e) => {
                                tracing::warn!(job = %id, error = %e, "could not delete an old job log");
                                continue;
                            }
                        }
                        let i = id.clone();
                        let _ = self
                            .db
                            .call(move |c| crate::db::jobs::mark_log_purged(c, &i))
                            .await;
                    }
                }
                Err(e) => tracing::warn!(error = %e, "log retention query failed"),
            }
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    }

    /// Stop scheduling, ask running jobs to stop, and wait (bounded) for them to end.
    pub async fn shutdown(&self, wait: Duration) {
        self.shutting_down.store(true, Ordering::SeqCst);
        self.wake.notify_one();
        let running: Vec<Arc<LiveJob>> = lock(&self.live)
            .values()
            .filter(|j| j.status() == JobStatus::Running)
            .cloned()
            .collect();
        for j in &running {
            j.request_cancel(CancelReason::Shutdown);
        }
        let handles: Vec<tokio::task::JoinHandle<()>> =
            lock(&self.tasks).drain().map(|(_, h)| h).collect();
        let all = futures::future::join_all(handles);
        if tokio::time::timeout(wait, all).await.is_err() {
            tracing::warn!("some jobs did not stop in time");
        }
    }

    /// Kinds of the live jobs (for tests and diagnostics).
    #[must_use]
    pub fn live_kinds(&self) -> Vec<(String, JobKind, JobStatus)> {
        lock(&self.live)
            .values()
            .map(|j| (j.id.clone(), j.spec.kind(), j.status()))
            .collect()
    }

    pub(crate) fn backend(&self) -> &Arc<dyn JobBackend> {
        &self.backend
    }

    pub(crate) fn db(&self) -> &Db {
        &self.db
    }

    pub(crate) fn secrets(&self) -> &SecretStore {
        &self.secrets
    }

    pub(crate) fn reads(&self) -> &Arc<crate::reads::Reads> {
        &self.reads
    }

    pub(crate) fn metrics(&self) -> &Metrics {
        &self.metrics
    }

    pub(crate) async fn persist_job(&self, job: Job) {
        self.persist(job).await;
    }

    pub(crate) async fn resolved_container(&self) -> Result<ContainerSettings, String> {
        self.container_settings().await
    }

    /// `true` once the container runtime answered (container settings resolved).
    pub async fn container_runtime_answered(&self) -> bool {
        self.container_settings.lock().await.is_some()
    }

    /// Ask the container runtime once in the background (container isolation only), so the
    /// first container job does not wait for it and `/system/info` can report it.
    pub fn probe_container_runtime(self: &Arc<Self>) {
        if self.settings.isolation != Isolation::Container {
            return;
        }
        let me = Arc::clone(self);
        tokio::spawn(async move {
            match me.container_settings().await {
                Ok(c) => {
                    tracing::info!(runtime = c.runtime.as_str(), user = %c.user, "container runtime answered");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "the container runtime did not answer; container jobs will fail until it does");
                }
            }
            // With a proxy configured, jobs must not have another way out.
            if let Some(check) =
                container::network_check(&container::SystemRunner, &me.settings.container).await
            {
                if check.level == h2w_api::CheckLevel::Ok {
                    tracing::info!("{}", check.detail);
                } else {
                    tracing::warn!(
                        remedy = check.remedy.as_deref().unwrap_or(""),
                        "SECURITY: {}",
                        check.detail
                    );
                }
            }
        });
    }

    pub(crate) fn job_dir(&self, id: &str) -> PathBuf {
        self.settings.paths.job_files_dir.join(id)
    }
}

/// What a job row recorded about its running step's process, when complete.
fn recorded_process(row: &crate::db::jobs::JobRow) -> Option<procfs::Recorded> {
    Some(procfs::Recorded {
        pid: i32::try_from(row.pid?).ok()?,
        start: u64::try_from(row.pid_start?).ok()?,
        boot_id: row.pid_boot.clone()?,
        job_id: row.job.id.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_well() {
        assert_eq!(human_duration(Duration::from_secs(5)), "5s");
        assert_eq!(human_duration(Duration::from_secs(120)), "2m");
        assert_eq!(human_duration(Duration::from_secs(125)), "2m 5s");
        assert_eq!(human_duration(Duration::from_secs(48 * 3600)), "48h");
        assert_eq!(human_duration(Duration::from_secs(5400)), "1h 30m");
    }

    #[test]
    fn cancel_reasons_map_to_statuses() {
        assert_eq!(
            CancelReason::User("a".into()).status(),
            JobStatus::Cancelled
        );
        assert_eq!(CancelReason::Budget.status(), JobStatus::Cancelled);
        assert_eq!(CancelReason::Budget.message(), "budget reached");
        assert_eq!(
            CancelReason::Timeout(Duration::from_secs(2)).status(),
            JobStatus::Failed
        );
        assert_eq!(CancelReason::Shutdown.status(), JobStatus::Failed);
        assert_eq!(CancelReason::DiskSpace(256).status(), JobStatus::Failed);
        assert!(
            CancelReason::DiskSpace(256)
                .message()
                .contains("256 MiB free")
        );
    }
}
