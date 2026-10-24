//! Executes one job: prepares its private directory, runs each step as its own process
//! group (argv only, cleared environment), streams output through the redactor into the
//! log, enforces cancellation, the wall-clock limit and the budget, and records the result.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use h2s_api::{
    BridgeInfo, ExperimentKind, Isolation, Job, JobEvent, JobResult, JobSpec, JobStatus, JobStep,
    LogLine, LogStream, PipelineResult, Progress, SplitInfo,
};
use h2s_core::parse::BridgeSettings;
use h2s_core::redact::Redactor;
use nix::sys::signal::Signal;
use tokio::sync::{mpsc, oneshot};

use super::backend::{ExitSemantics, JobPlan, ParseKind, PlanInput, StepOutput, StepPlan};
use super::logs::{LineReader, LogFile, PathMasker};
use super::{CancelReason, JobManager, LiveJob, container, procfs, validate};
use crate::db::audit::AuditEntry;
use crate::secrets::SecretSnapshot;
use crate::util::{now_rfc3339, now_rfc3339_millis, truncate_chars};

/// Captured stdout kept per step for parsing.
const CAPTURE_MAX: usize = 32 * 1024 * 1024;
/// stderr lines kept for the failure message.
const STDERR_TAIL: usize = 20;
/// The bridge's key setting line; never logged with its value whatever the parser says.
const BRIDGE_KEY_PREFIX: &str = "HARNESS2_CODEX_API_KEY=";
/// Secret the bridge key is stored as.
const BRIDGE_SECRET: &str = "HARNESS2_CODEX_API_KEY";

struct Finish {
    status: JobStatus,
    exit_code: Option<i32>,
    error: Option<String>,
    result: Option<JobResult>,
}

impl Finish {
    fn failed(msg: impl Into<String>) -> Finish {
        Finish {
            status: JobStatus::Failed,
            exit_code: None,
            error: Some(msg.into()),
            result: None,
        }
    }
}

/// Run `live` to completion and record its final state.
pub(super) async fn run(mgr: Arc<JobManager>, live: Arc<LiveJob>) {
    let started = Instant::now();
    let timeout = mgr.settings().timeout;
    let watchdog = {
        let l = Arc::clone(&live);
        tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            l.request_cancel(CancelReason::Timeout(timeout));
        })
    };

    let log_path = super::logs::log_path(&mgr.settings().paths.job_logs_dir, &live.id);
    let pump = match LogFile::open(&log_path).await {
        Ok((file, existing)) => Some(Pump::start(
            Arc::clone(&mgr),
            Arc::clone(&live),
            file,
            existing,
        )),
        Err(e) => {
            tracing::error!(job = %live.id, error = %e, "cannot open the job log");
            None
        }
    };
    let finish = match &pump {
        Some(p) => execute(&mgr, &live, p).await,
        None => Finish::failed("could not open the job log"),
    };
    watchdog.abort();
    finalize(&mgr, &live, finish, started, pump).await;
}

struct Ctx<'a> {
    mgr: &'a Arc<JobManager>,
    live: &'a Arc<LiveJob>,
    pump: &'a Pump,
    experiment: Option<(PathBuf, ExperimentKind)>,
    profile: Option<h2s_api::ModelProfile>,
    job_dir: PathBuf,
    masker: PathMasker,
    argv_guard: Redactor,
}

async fn execute(mgr: &Arc<JobManager>, live: &Arc<LiveJob>, pump: &Pump) -> Finish {
    let isolation = live.snapshot().isolation;
    pump.system(format!(
        "job {} ({}) started; isolation: {}",
        live.id,
        live.spec.kind().as_str(),
        crate::db::jobs::isolation_str(isolation)
    ))
    .await;
    let prepared = match prepare(mgr, live, pump, isolation).await {
        Ok(p) => p,
        Err(msg) => {
            pump.system(format!("could not prepare the job: {msg}"))
                .await;
            return Finish::failed(format!("could not prepare the job: {msg}"));
        }
    };
    let (ctx, plan) = prepared;
    if let Some(r) = live.cancel_reason() {
        return Finish {
            status: r.status(),
            exit_code: None,
            error: Some(r.message()),
            result: None,
        };
    }

    // Record the main command for reproducibility manifests.
    let main = plan
        .steps
        .iter()
        .find(|s| s.parse == ParseKind::Run)
        .or_else(|| plan.steps.last());
    if let Some(m) = main {
        let argv: Vec<String> = m
            .display
            .iter()
            .map(|a| ctx.masker.mask(a).into_owned())
            .collect();
        let id = live.id.clone();
        let _ = mgr
            .db()
            .call(move |c| crate::db::jobs::set_argv(c, &id, &argv))
            .await;
    }

    // All steps start queued.
    live.mutate(|job| {
        job.steps = plan
            .steps
            .iter()
            .map(|s| JobStep {
                name: s.name.clone(),
                status: JobStatus::Queued,
                exit_code: None,
                started_at: None,
                finished_at: None,
            })
            .collect();
        job.steps
            .iter()
            .map(|s| JobEvent::Step { step: s.clone() })
            .collect()
    });
    mgr.persist_job(live.snapshot()).await;

    let mut results: Vec<JobResult> = Vec::new();
    let mut last_result: Option<JobResult> = None;
    let mut exit_code = None;
    let mut failure: Option<String> = None;
    let mut partial = false;
    let mut summary_ns: Option<String> = None;
    let single = plan.steps.len() == 1;
    for (idx, step) in plan.steps.iter().enumerate() {
        if live.cancel_reason().is_some() {
            break;
        }
        set_step(live, idx, |s| {
            s.status = JobStatus::Running;
            s.started_at = Some(now_rfc3339());
        });
        mgr.persist_job(live.snapshot()).await;

        let outcome = run_step(&ctx, step, summary_ns.as_deref()).await;
        let mut status = match outcome.exit_code {
            Some(0) => JobStatus::Succeeded,
            Some(2) if step.exit == ExitSemantics::Harness2 => JobStatus::Partial,
            _ => JobStatus::Failed,
        };
        if outcome.spawn_error.is_none()
            && let Some(r) = live.cancel_reason()
            && status != JobStatus::Succeeded
        {
            status = r.status();
        }
        exit_code = outcome.exit_code;

        // Parse whatever the step produced, even on failure (check reports its problems).
        let parsed = if outcome.spawn_error.is_none() {
            let out = StepOutput {
                spec: &live.spec,
                exit_code: outcome.exit_code,
                stdout: &outcome.capture.stdout,
                job_dir: &ctx.job_dir,
                experiment: ctx.experiment.as_ref().map(|(p, k)| (p.as_path(), *k)),
                profile: ctx.profile.as_ref(),
                harness2_home: &mgr.settings().paths.harness2_home,
                harness2_managed: mgr.settings().paths.harness2_managed,
            };
            match mgr.backend().parse_step(step, &out) {
                Ok(r) => r,
                Err(e) => {
                    ctx.pump
                        .system(format!("could not read the result of {}: {e}", step.name))
                        .await;
                    if status == JobStatus::Succeeded {
                        status = JobStatus::Failed;
                        failure.get_or_insert_with(|| format!("{}: {e}", step.name));
                    }
                    None
                }
            }
        } else {
            None
        };
        if let Some(JobResult::Preview(p)) = &parsed {
            summary_ns = Some(p.summary_namespace.clone());
            live.mutate(|job| {
                let mut prog = job.progress.clone().unwrap_or(Progress {
                    done: 0,
                    failed: 0,
                    total: None,
                    current: None,
                });
                prog.total = Some(p.total_tasks);
                job.progress = Some(prog.clone());
                vec![JobEvent::Progress { progress: prog }]
            });
        }
        if let Some(r) = parsed {
            results.push(r.clone());
            last_result = Some(r);
        } else {
            last_result = None;
        }

        set_step(live, idx, |s| {
            s.status = status;
            s.exit_code = outcome.exit_code;
            s.finished_at = Some(now_rfc3339());
        });
        mgr.persist_job(live.snapshot()).await;

        match status {
            JobStatus::Succeeded => {}
            JobStatus::Partial => partial = true,
            _ => {
                if failure.is_none() {
                    let why = outcome.spawn_error.clone().unwrap_or_else(|| {
                        let code = outcome
                            .exit_code
                            .map_or_else(|| "a signal".to_owned(), |c| format!("code {c}"));
                        let tail = outcome
                            .capture
                            .stderr_tail
                            .iter()
                            .rev()
                            .find(|l| !l.trim().is_empty())
                            .map(|l| format!(": {}", truncate_chars(l.trim(), 300)))
                            .unwrap_or_default();
                        format!("exited with {code}{tail}")
                    });
                    failure = Some(if single {
                        why
                    } else {
                        format!("step {} failed: {why}", step.name)
                    });
                }
                break;
            }
        }
    }

    let result = if matches!(live.spec, JobSpec::Pipeline(_)) {
        Some(JobResult::Pipeline(PipelineResult { steps: results }))
    } else if matches!(live.spec, JobSpec::Bridge(_)) {
        last_result.or_else(|| live.snapshot().result)
    } else {
        last_result
    };
    if let Some(r) = live.cancel_reason() {
        return Finish {
            status: r.status(),
            exit_code,
            error: Some(r.message()),
            result,
        };
    }
    if let Some(f) = failure {
        return Finish {
            status: JobStatus::Failed,
            exit_code,
            error: Some(f),
            result,
        };
    }
    let mut finish = Finish {
        status: if partial {
            JobStatus::Partial
        } else {
            JobStatus::Succeeded
        },
        exit_code,
        error: partial.then(|| "finished, but some tasks failed or stayed ungraded".to_owned()),
        result,
    };
    if let (JobSpec::Split(p), Some(JobResult::Split(s))) = (&live.spec, &finish.result)
        && let Err(e) = register_split(mgr, &ctx.job_dir, p, s).await
    {
        finish = Finish::failed(format!("could not save the split: {e}"));
    }
    finish
}

fn set_step(live: &LiveJob, idx: usize, f: impl FnOnce(&mut JobStep)) {
    live.mutate(|job| match job.steps.get_mut(idx) {
        Some(s) => {
            f(s);
            vec![JobEvent::Step { step: s.clone() }]
        }
        None => Vec::new(),
    });
}

async fn prepare<'a>(
    mgr: &'a Arc<JobManager>,
    live: &'a Arc<LiveJob>,
    pump: &'a Pump,
    isolation: Isolation,
) -> Result<(Ctx<'a>, JobPlan), String> {
    let settings = mgr.settings();
    let spec = &live.spec;

    let experiment = match spec.experiment() {
        Some(name) => {
            let n = name.to_owned();
            let rec = mgr
                .db()
                .call(move |c| crate::db::experiments::get(c, &n))
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("experiment {name} no longer exists"))?;
            let root = settings.paths.experiments_dir.clone();
            let n = name.to_owned();
            let path = tokio::task::spawn_blocking(move || crate::expdirs::existing(&root, &n))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
            Some((path, rec.kind))
        }
        None => None,
    };

    let profile_name = match validate::profile_of(spec) {
        Some(p) => Some(p.to_owned()),
        // `check` without a profile uses the profile named `default` when there is one.
        None if matches!(spec, JobSpec::Check(_)) => Some("default".to_owned()),
        None => None,
    };
    let profile = match profile_name {
        Some(n) => {
            let explicit = validate::profile_of(spec).is_some();
            let name = n.clone();
            let p = mgr
                .db()
                .call(move |c| crate::db::profiles::get(c, &name))
                .await
                .map_err(|e| e.to_string())?;
            if p.is_none() && explicit {
                return Err(format!("profile {n} no longer exists"));
            }
            p
        }
        None => None,
    };
    if let Some(p) = &profile {
        for w in h2s_core::env::profile_warnings(p) {
            pump.system(format!("profile {}: {w}", p.name)).await;
        }
    }
    let names = profile
        .as_ref()
        .map(crate::db::profiles::referenced_secrets)
        .unwrap_or_default();
    let snapshot = SecretSnapshot(
        mgr.secrets()
            .values_for(&names)
            .await
            .map_err(|e| e.to_string())?,
    );
    if let Some(missing) = names.iter().find(|n| !snapshot.0.contains_key(*n)) {
        return Err(format!("secret {missing} is not set"));
    }

    let job_dir = mgr.job_dir(&live.id);
    let _ = tokio::fs::remove_dir_all(&job_dir).await;
    crate::util::ensure_private_dir(&job_dir).map_err(|e| format!("job directory: {e}"))?;

    let split_file = match validate::options_of(spec).and_then(|o| o.split_file.clone()) {
        Some(name) => {
            if !crate::util::is_single_component(&name) {
                return Err("invalid split name".into());
            }
            let src = settings.paths.splits_dir.join(format!("{name}.json"));
            let dst = job_dir.join("split.json");
            let bytes = tokio::fs::read(&src)
                .await
                .map_err(|e| format!("split {name}: {e}"))?;
            crate::util::write_private_new(&dst, &bytes).map_err(|e| format!("split copy: {e}"))?;
            Some(dst)
        }
        None => None,
    };

    let container = if isolation == Isolation::Container {
        Some(mgr.resolved_container().await?)
    } else {
        None
    };

    let input = PlanInput {
        job_id: &live.id,
        spec,
        isolation,
        harness2_home: &settings.paths.harness2_home,
        harness2_managed: settings.paths.harness2_managed,
        experiment: experiment.as_ref().map(|(p, k)| (p.as_path(), *k)),
        job_dir: &job_dir,
        split_file: split_file.as_deref(),
        profile: profile.as_ref(),
        secrets: &snapshot,
        container: container.as_ref(),
    };
    let plan = mgr.backend().plan(&input)?;
    if plan.steps.is_empty() {
        return Err("nothing to run".into());
    }
    for (name, content) in &plan.files {
        if !crate::util::is_single_component(name) || name.starts_with('.') {
            return Err(format!("refused generated file name {name:?}"));
        }
        crate::util::write_private_new(&job_dir.join(name), content.as_bytes())
            .map_err(|e| format!("writing {name}: {e}"))?;
    }

    // Redact every value this job can see, plus every stored secret (defence in depth).
    let mut secrets: Vec<(String, String)> = mgr
        .secrets()
        .all_values()
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|(n, v)| (n, v.as_str().to_owned()))
        .collect();
    secrets.extend(
        plan.secrets
            .iter()
            .map(|(n, v)| (n.clone(), v.as_str().to_owned())),
    );
    // Command lines are visible to every local user (/proc); refuse any that would carry a
    // secret. Very short values are skipped here (they match ordinary arguments).
    let argv_guard = Redactor::new(
        secrets
            .iter()
            .filter(|(_, v)| v.len() >= h2s_core::redact::MIN_SECRET_LEN)
            .cloned()
            .collect(),
    );
    pump.set_redactor(Redactor::new(secrets)).await;

    let mut pairs = vec![
        (job_dir.display().to_string(), "<job>".to_owned()),
        (
            settings.paths.experiments_dir.display().to_string(),
            "<experiments>".to_owned(),
        ),
        (
            settings.paths.harness2_home.display().to_string(),
            "<harness2>".to_owned(),
        ),
        (
            settings.paths.data_dir.display().to_string(),
            "<data>".to_owned(),
        ),
    ];
    if let Some((p, _)) = &experiment {
        pairs.push((p.display().to_string(), "<experiment>".to_owned()));
    }
    if let Some(home) = std::env::var_os("HOME") {
        pairs.push((home.to_string_lossy().into_owned(), "~".to_owned()));
    }
    let masker = PathMasker::new(pairs);
    pump.set_masker(masker.clone()).await;

    Ok((
        Ctx {
            mgr,
            live,
            pump,
            experiment,
            profile,
            job_dir,
            masker,
            argv_guard,
        },
        plan,
    ))
}

async fn register_split(
    mgr: &Arc<JobManager>,
    job_dir: &Path,
    p: &h2s_api::SplitParams,
    s: &h2s_api::SplitResult,
) -> Result<(), String> {
    let src = job_dir
        .join(super::core_backend::SPLIT_OUT_DIR)
        .join(super::core_backend::SPLIT_FILE);
    let dst = mgr
        .settings()
        .paths
        .splits_dir
        .join(format!("{}.json", p.name));
    let bytes = tokio::fs::read(&src).await.map_err(|e| e.to_string())?;
    crate::util::write_private_new(&dst, &bytes).map_err(|e| e.to_string())?;
    let info = SplitInfo {
        name: p.name.clone(),
        bench: p.bench,
        subset: p.subset,
        seed: p.seed,
        train_fraction: p.train_fraction,
        train: s.train,
        test: s.test,
        created_at: now_rfc3339(),
    };
    let now = crate::util::now_unix();
    let res = mgr
        .db()
        .call(move |c| crate::db::splits::insert(c, &info, now))
        .await;
    if let Err(e) = res {
        let _ = tokio::fs::remove_file(&dst).await;
        return Err(e.to_string());
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// One step
// ---------------------------------------------------------------------------------------

#[derive(Default)]
struct Capture {
    stdout: String,
    stderr_tail: Vec<String>,
}

struct StepOutcome {
    exit_code: Option<i32>,
    spawn_error: Option<String>,
    capture: Capture,
}

fn passthrough_env(cmd: &mut tokio::process::Command, runtime_env: bool) {
    for (name, value) in std::env::vars_os() {
        if name.to_str().is_some_and(h2s_core::env::is_passed_through) {
            cmd.env(name, value);
        }
    }
    if runtime_env {
        for name in container::RUNTIME_ENV_PASSTHROUGH {
            if let Some(v) = std::env::var_os(name) {
                cmd.env(name, v);
            }
        }
    }
}

async fn run_step(ctx: &Ctx<'_>, step: &StepPlan, summary_ns: Option<&str>) -> StepOutcome {
    let pump = ctx.pump;
    let grace = ctx.mgr.settings().kill_grace;
    let spawn_fail = |msg: String| StepOutcome {
        exit_code: None,
        spawn_error: Some(msg),
        capture: Capture::default(),
    };

    // A secret value must never appear in argv (visible in /proc to every local user).
    let leaks = std::iter::once(&step.program)
        .chain(&step.args)
        .any(|a| ctx.argv_guard.redact(a) != a.as_str());
    if leaks {
        let msg = "refused: a secret value would appear on the command line".to_owned();
        pump.system(msg.clone()).await;
        return spawn_fail(msg);
    }

    pump.begin(step.progress, step.bridge).await;
    pump.system(format!("▶ {}: {}", step.name, step.display.join(" ")))
        .await;

    let mut cmd = tokio::process::Command::new(&step.program);
    cmd.args(&step.args)
        .current_dir(&step.cwd)
        .env_clear()
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .process_group(0)
        .kill_on_drop(false);
    passthrough_env(&mut cmd, step.runtime_env);
    for (k, v) in &step.env {
        cmd.env(k, v);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let what = step.display.first().map_or("the program", String::as_str);
            let msg = format!("could not start {what}: {e}");
            pump.system(msg.clone()).await;
            let _ = pump.end().await;
            return spawn_fail(msg);
        }
    };
    let pid = child.id().and_then(|p| i32::try_from(p).ok());
    if let Some(pid) = pid {
        let start = procfs::start_time(pid).and_then(|s| i64::try_from(s).ok());
        let id = ctx.live.id.clone();
        let in_container = step.container.is_some();
        let _ = ctx
            .mgr
            .db()
            .call(move |c| {
                crate::db::jobs::set_process(c, &id, Some(i64::from(pid)), start, in_container)
            })
            .await;
    }

    let out_task = child
        .stdout
        .take()
        .map(|s| tokio::spawn(read_into(s, LogStream::Stdout, pump.sender())));
    let err_task = child
        .stderr
        .take()
        .map(|s| tokio::spawn(read_into(s, LogStream::Stderr, pump.sender())));

    let budget_task = match (
        step.budget,
        validate::budget_of(&ctx.live.spec),
        &ctx.experiment,
    ) {
        (true, Some(cap), Some((dir, kind))) => {
            let ns = summary_ns
                .map(str::to_owned)
                .or_else(|| fallback_summary_ns(&ctx.live.spec));
            if ns.is_none() {
                pump.system("budget guard: the sweep namespace is unknown".into())
                    .await;
            }
            Some(tokio::spawn(budget_guard(
                Arc::clone(ctx.mgr),
                Arc::clone(ctx.live),
                dir.clone(),
                *kind,
                ns,
                cap,
            )))
        }
        _ => None,
    };

    let status = tokio::select! {
        s = child.wait() => s,
        () = ctx.live.cancel.cancelled() => {
            let why = ctx.live.cancel_reason().map_or_else(|| "cancelled".to_owned(), |r| r.message());
            pump.system(format!("stopping {}: {why}", step.name)).await;
            if let Some(c) = &step.container {
                match container::verify_and_stop(&container::SystemRunner, c.runtime, &c.job_id).await {
                    Ok(true) => pump.system("container stopped".into()).await,
                    Ok(false) => {}
                    Err(e) => pump.system(format!("container not stopped: {e}")).await,
                }
            }
            if let Some(pid) = pid {
                let _ = procfs::signal_group(pid, Signal::SIGTERM);
            }
            match tokio::time::timeout(grace, child.wait()).await {
                Ok(s) => s,
                Err(_) => {
                    pump.system(format!(
                        "{} did not stop within {}; sending SIGKILL",
                        step.name,
                        super::human_duration(grace)
                    ))
                    .await;
                    if let Some(pid) = pid {
                        let _ = procfs::signal_group(pid, Signal::SIGKILL);
                    }
                    child.wait().await
                }
            }
        }
    };
    if let Some(t) = budget_task {
        t.abort();
    }

    // Readers end at EOF; a grandchild holding the pipes open must not hang the job.
    let readers: Vec<_> = out_task.into_iter().chain(err_task).collect();
    let mut readers = futures::future::join_all(readers);
    if tokio::time::timeout(Duration::from_secs(2), &mut readers)
        .await
        .is_err()
    {
        if let Some(pid) = pid {
            let _ = procfs::signal_group(pid, Signal::SIGKILL);
        }
        let _ = tokio::time::timeout(Duration::from_secs(2), &mut readers).await;
    }
    // Leftover members of the group (background processes) do not outlive the step.
    if let Some(pid) = pid
        && procfs::group_alive(pid)
    {
        let _ = procfs::signal_group(pid, Signal::SIGKILL);
    }
    {
        let id = ctx.live.id.clone();
        let _ = ctx
            .mgr
            .db()
            .call(move |c| crate::db::jobs::set_process(c, &id, None, None, false))
            .await;
    }

    let capture = pump.end().await;
    let exit_code = match status {
        Ok(s) => {
            if let Some(code) = s.code() {
                pump.system(format!("■ {} exited with code {code}", step.name))
                    .await;
                Some(code)
            } else {
                use std::os::unix::process::ExitStatusExt;
                let sig = s.signal().unwrap_or(0);
                pump.system(format!("■ {} was killed by signal {sig}", step.name))
                    .await;
                None
            }
        }
        Err(e) => {
            pump.system(format!(
                "■ {}: could not wait for the process: {e}",
                step.name
            ))
            .await;
            None
        }
    };
    StepOutcome {
        exit_code,
        spawn_error: None,
        capture,
    }
}

/// The summary namespace of a run's sweep when the preview did not report one.
fn fallback_summary_ns(spec: &JobSpec) -> Option<String> {
    let (cell, opts) = (validate::cell_of(spec)?, validate::options_of(spec)?);
    Some(
        h2s_core::namespace::Namespace::for_cell(&cell, h2s_api::Mode::Parallel, opts.k, opts.seed)
            .to_string(),
    )
}

async fn read_into<R: tokio::io::AsyncRead + Unpin>(
    r: R,
    stream: LogStream,
    tx: mpsc::Sender<PumpMsg>,
) {
    let mut reader = LineReader::new(r);
    loop {
        match reader.next_line().await {
            Ok(Some(text)) => {
                if tx.send(PumpMsg::Out { stream, text }).await.is_err() {
                    return;
                }
            }
            Ok(None) => return,
            Err(e) => {
                let _ = tx
                    .send(PumpMsg::System(format!("reading {stream:?} failed: {e}")))
                    .await;
                return;
            }
        }
    }
}

/// Poll the sweep's recorded spend; cancel the job once it reaches `cap` USD. Spend is
/// counted from when the step started (earlier runs of a resumed sweep do not count).
async fn budget_guard(
    mgr: Arc<JobManager>,
    live: Arc<LiveJob>,
    exp_dir: PathBuf,
    kind: ExperimentKind,
    summary_ns: Option<String>,
    cap: f64,
) {
    let Some(ns) = summary_ns else {
        return;
    };
    let read = |mgr: &Arc<JobManager>| {
        let backend = Arc::clone(mgr.backend());
        let (d, n) = (exp_dir.clone(), ns.clone());
        tokio::task::spawn_blocking(move || backend.spend_usd(&d, kind, &n))
    };
    let baseline = read(&mgr).await.ok().flatten().unwrap_or(0.0);
    let experiment = live.spec.experiment().unwrap_or_default().to_owned();
    let mut last = 0.0_f64;
    loop {
        tokio::time::sleep(mgr.settings().budget_poll).await;
        let Some(total) = read(&mgr).await.ok().flatten() else {
            continue;
        };
        let spent = (total - baseline).max(0.0);
        if spent > last {
            mgr.metrics().add_spend(&experiment, spent - last);
            last = spent;
        }
        live.mutate(|job| {
            job.spent_usd = Some(spent);
            Vec::new()
        });
        if spent >= cap {
            live.request_cancel(CancelReason::Budget);
            return;
        }
    }
}

// ---------------------------------------------------------------------------------------
// The log pump: one task per job serializes every line into the log.
// ---------------------------------------------------------------------------------------

enum PumpMsg {
    Begin { progress: bool, bridge: bool },
    Out { stream: LogStream, text: String },
    System(String),
    End(oneshot::Sender<Capture>),
    SetRedactor(Redactor),
    SetMasker(PathMasker),
    Close(oneshot::Sender<()>),
}

/// Handle to a job's log pump.
struct Pump {
    tx: mpsc::Sender<PumpMsg>,
    task: tokio::task::JoinHandle<()>,
}

struct PumpState {
    mgr: Arc<JobManager>,
    live: Arc<LiveJob>,
    file: LogFile,
    next_n: u64,
    redactor: Redactor,
    masker: PathMasker,
    progress: bool,
    bridge: bool,
    bridge_acc: BridgeSettings,
    bridge_stored: bool,
    capture: Capture,
    last_persist: Instant,
}

impl Pump {
    fn start(mgr: Arc<JobManager>, live: Arc<LiveJob>, file: LogFile, existing: u64) -> Pump {
        let (tx, rx) = mpsc::channel(4096);
        let state = PumpState {
            mgr,
            live,
            file,
            next_n: existing,
            redactor: Redactor::default(),
            masker: PathMasker::default(),
            progress: false,
            bridge: false,
            bridge_acc: BridgeSettings {
                base_url: String::new(),
                api_key: String::new(),
                model: String::new(),
                container_url: None,
            },
            bridge_stored: false,
            capture: Capture::default(),
            last_persist: Instant::now(),
        };
        let task = tokio::spawn(state.run(rx));
        Pump { tx, task }
    }

    fn sender(&self) -> mpsc::Sender<PumpMsg> {
        self.tx.clone()
    }

    async fn system(&self, text: String) {
        let _ = self.tx.send(PumpMsg::System(text)).await;
    }

    async fn begin(&self, progress: bool, bridge: bool) {
        let _ = self.tx.send(PumpMsg::Begin { progress, bridge }).await;
    }

    async fn end(&self) -> Capture {
        let (tx, rx) = oneshot::channel();
        if self.tx.send(PumpMsg::End(tx)).await.is_err() {
            return Capture::default();
        }
        rx.await.unwrap_or_default()
    }

    async fn set_redactor(&self, r: Redactor) {
        let _ = self.tx.send(PumpMsg::SetRedactor(r)).await;
    }

    async fn set_masker(&self, m: PathMasker) {
        let _ = self.tx.send(PumpMsg::SetMasker(m)).await;
    }

    async fn close(self) {
        let (tx, rx) = oneshot::channel();
        if self.tx.send(PumpMsg::Close(tx)).await.is_ok() {
            let _ = rx.await;
        }
        drop(self.tx);
        let _ = self.task.await;
    }
}

impl PumpState {
    async fn run(mut self, mut rx: mpsc::Receiver<PumpMsg>) {
        while let Some(msg) = rx.recv().await {
            match msg {
                PumpMsg::Begin { progress, bridge } => {
                    self.progress = progress;
                    self.bridge = bridge;
                    self.capture = Capture::default();
                }
                PumpMsg::Out { stream, text } => self.line(stream, text).await,
                PumpMsg::System(text) => {
                    let text = self.clean(&text);
                    self.write(LogStream::System, text).await;
                }
                PumpMsg::End(reply) => {
                    self.flush().await;
                    let _ = reply.send(std::mem::take(&mut self.capture));
                }
                PumpMsg::SetRedactor(r) => self.redactor = r,
                PumpMsg::SetMasker(m) => self.masker = m,
                PumpMsg::Close(reply) => {
                    self.flush().await;
                    let _ = reply.send(());
                    return;
                }
            }
            if rx.is_empty() {
                self.flush().await;
            }
        }
        self.flush().await;
    }

    fn clean(&self, text: &str) -> String {
        let r = self.redactor.redact(text);
        self.masker.mask(&r).into_owned()
    }

    async fn line(&mut self, stream: LogStream, raw: String) {
        let mut raw = raw;
        if self.bridge && stream == LogStream::Stdout {
            self.bridge_capture(&raw).await;
            if let Some(v) = raw.strip_prefix(BRIDGE_KEY_PREFIX)
                && !v.is_empty()
            {
                raw = format!("{BRIDGE_KEY_PREFIX}[REDACTED:{BRIDGE_SECRET}]");
            }
        }
        let redacted = self.redactor.redact(&raw).into_owned();
        match stream {
            LogStream::Stdout => {
                if self.capture.stdout.len() + redacted.len() < CAPTURE_MAX {
                    self.capture.stdout.push_str(&redacted);
                    self.capture.stdout.push('\n');
                }
                if self.progress
                    && let Some(tp) = self.mgr.backend().progress_line(&redacted)
                {
                    let ok = matches!(tp.status.as_str(), "ok" | "skipped");
                    let current = self
                        .masker
                        .mask(&format!("{} / {}", tp.domain, tp.task))
                        .into_owned();
                    self.live.mutate(|job| {
                        let mut p = job.progress.clone().unwrap_or(Progress {
                            done: 0,
                            failed: 0,
                            total: None,
                            current: None,
                        });
                        if ok {
                            p.done += 1;
                        } else {
                            p.failed += 1;
                        }
                        p.current = Some(current);
                        job.progress = Some(p.clone());
                        vec![JobEvent::Progress { progress: p }]
                    });
                }
            }
            LogStream::Stderr => {
                if self.capture.stderr_tail.len() >= STDERR_TAIL {
                    self.capture.stderr_tail.remove(0);
                }
                self.capture
                    .stderr_tail
                    .push(self.masker.mask(&redacted).into_owned());
            }
            LogStream::System => {}
        }
        let shown = self.masker.mask(&redacted).into_owned();
        self.write(stream, shown).await;
    }

    async fn bridge_capture(&mut self, raw: &str) {
        let carried = self.mgr.backend().bridge_line(raw, &mut self.bridge_acc);
        // Safety net independent of the parser: the key line is recognised here too.
        if self.bridge_acc.api_key.is_empty()
            && let Some(v) = raw.strip_prefix(BRIDGE_KEY_PREFIX)
            && !v.trim().is_empty()
        {
            self.bridge_acc.api_key = v.trim().to_owned();
        }
        if !self.bridge_acc.api_key.is_empty() && !self.bridge_stored {
            self.bridge_stored = true;
            let key = self.bridge_acc.api_key.clone();
            self.redactor.add(BRIDGE_SECRET.to_owned(), key.clone());
            let actor = self.live.snapshot().created_by;
            let entry = AuditEntry::new(
                actor,
                "secret.capture",
                BRIDGE_SECRET,
                h2s_api::AuditOutcome::Success,
            )
            .detail(format!("captured from bridge job {}", self.live.id));
            let res = self
                .mgr
                .db()
                .call(move |c| {
                    crate::db::write_tx(c, |tx| {
                        crate::db::secrets::set(tx, BRIDGE_SECRET, &key, crate::util::now_unix())?;
                        crate::db::audit::append(tx, &entry)?;
                        Ok(())
                    })
                })
                .await;
            if let Err(e) = res {
                tracing::error!(error = %e, "could not store the bridge key");
            }
        }
        if carried || !self.bridge_acc.base_url.is_empty() {
            let acc = &self.bridge_acc;
            if !acc.base_url.is_empty() && !acc.model.is_empty() {
                let info = BridgeInfo {
                    base_url: acc.base_url.clone(),
                    model: acc.model.clone(),
                    container_url: acc.container_url.clone(),
                };
                let changed = !matches!(self.live.snapshot().result, Some(JobResult::Bridge(ref b)) if *b == info);
                if changed {
                    self.live.mutate(|job| {
                        job.result = Some(JobResult::Bridge(info));
                        Vec::new()
                    });
                    self.mgr.persist_job(self.live.snapshot()).await;
                }
            }
        }
    }

    async fn write(&mut self, stream: LogStream, text: String) {
        let line = LogLine {
            n: self.next_n,
            ts: now_rfc3339_millis(),
            stream,
            text,
        };
        if let Err(e) = self.file.append(&line).await {
            tracing::error!(job = %self.live.id, error = %e, "cannot write the job log");
            return;
        }
        self.next_n += 1;
        self.live.push_log(line);
        if self.last_persist.elapsed() > Duration::from_secs(2) {
            self.last_persist = Instant::now();
            self.flush().await;
            self.mgr.persist_job(self.live.snapshot()).await;
        }
    }

    async fn flush(&mut self) {
        if let Err(e) = self.file.flush().await {
            tracing::error!(job = %self.live.id, error = %e, "cannot flush the job log");
        }
    }
}

// ---------------------------------------------------------------------------------------
// Finishing
// ---------------------------------------------------------------------------------------

async fn finalize(
    mgr: &Arc<JobManager>,
    live: &Arc<LiveJob>,
    finish: Finish,
    started: Instant,
    pump: Option<Pump>,
) {
    // Failure reasons are shown to every viewer: no host paths.
    let paths = &mgr.settings().paths;
    let mut pairs = vec![
        (
            mgr.job_dir(&live.id).display().to_string(),
            "<job>".to_owned(),
        ),
        (
            paths.experiments_dir.display().to_string(),
            "<experiments>".to_owned(),
        ),
        (
            paths.harness2_home.display().to_string(),
            "<harness2>".to_owned(),
        ),
        (paths.data_dir.display().to_string(), "<data>".to_owned()),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        pairs.push((home.to_string_lossy().into_owned(), "~".to_owned()));
    }
    let masker = PathMasker::new(pairs);
    let error = finish.error.map(|e| truncate_chars(&masker.mask(&e), 1000));
    if let Some(p) = &pump {
        let line = match &error {
            Some(e) => format!("job {}: {e}", finish.status.as_str()),
            None => format!("job {}", finish.status.as_str()),
        };
        p.system(line).await;
    }
    if let Some(p) = pump {
        p.close().await;
    }
    let now = now_rfc3339();
    let status = finish.status;
    let exit_code = finish.exit_code;
    live.mutate(|job: &mut Job| {
        job.status = status;
        job.exit_code = exit_code;
        job.error.clone_from(&error);
        if finish.result.is_some() || !matches!(job.spec, JobSpec::Bridge(_)) {
            job.result = finish.result;
        }
        job.finished_at = Some(now.clone());
        let mut events = Vec::new();
        for s in &mut job.steps {
            if !s.status.is_terminal() {
                s.status = if s.status == JobStatus::Running {
                    status
                } else {
                    JobStatus::Cancelled
                };
                s.finished_at = Some(now.clone());
                events.push(JobEvent::Step { step: s.clone() });
            }
        }
        events.push(JobEvent::Status {
            status,
            exit_code,
            error: error.clone(),
        });
        events
    });
    let job = live.snapshot();
    mgr.persist_job(job.clone()).await;
    let _ = tokio::fs::remove_dir_all(mgr.job_dir(&live.id)).await;
    mgr.metrics().job_finished(
        job.kind.as_str(),
        status.as_str(),
        started.elapsed().as_secs_f64(),
    );
    tracing::info!(job = %job.id, kind = job.kind.as_str(), status = status.as_str(), "job finished");
    mgr.job_done(&live.id);
}
