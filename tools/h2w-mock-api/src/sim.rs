//! Job simulation: each created job runs a short script that writes log lines, reports
//! progress and steps over the event channel, and finally stores a result like the real
//! server would. Cancellation is checked between steps.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use h2w_api::{
    Bench, BridgeInfo, Cell, CheckResult, CostEstimate, DemoResult, DomainCount, InstallExtra,
    InstallResult, JobEvent, JobResult, JobSpec, JobStatus, JobStep, JudgeResult, LogStream, Mode,
    ModeNamespace, PipelineResult, PreviewResult, Prices, Progress, ReportResult, RunOptions,
    RunResult, SetupResult, SplitInfo, SplitResult, StreamEntry, Substrate,
};

use crate::state::{self, ExpData, Inner, TESTED_COMMIT};
use crate::sweeps::{self, SweepSpec, TaskSpec, TaskState};
use crate::util::{Rng, now_secs, rfc3339, seed_of};

pub(crate) type Shared = Arc<Mutex<Inner>>;

pub(crate) fn lock(s: &Shared) -> MutexGuard<'_, Inner> {
    match s.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Upstream's namespace name for a cell, mode, k and seed.
pub(crate) fn namespace(cell: &Cell, mode: Mode, k: u32, seed: Option<u64>) -> String {
    let mut s = format!(
        "{}-{}-{}{k}",
        cell.bench_name(),
        cell.substrate.as_cli(),
        mode.token()
    );
    if let Some(seed) = seed {
        s.push_str(&format!("-s{seed}"));
    }
    s
}

/// The estimate the real server computes (same budget identity).
pub(crate) fn estimate(
    tasks: u32,
    k: u32,
    modes: &[Mode],
    prices: Option<&Prices>,
) -> CostEstimate {
    let m = u64::try_from(modes.len()).unwrap_or(0);
    let (t, kk) = (u64::from(tasks), u64::from(k));
    let rollouts = t * (kk + 3 * kk * m);
    let sessions = t * m * 3 * kk + t * m * u64::from(k > 1) + t * m;
    let (usd_low, usd_high) = prices.map_or((None, None), |p| {
        (
            Some(rollouts as f64 * p.rollout_usd_low + sessions as f64 * p.session_usd_low),
            Some(rollouts as f64 * p.rollout_usd_high + sessions as f64 * p.session_usd_high),
        )
    });
    let mut notes = vec![
        "Judging is not included; it adds one judge call per task and mode.".to_owned(),
        "Each improver stage may retry once, so sessions can be up to twice the count shown."
            .to_owned(),
    ];
    if prices.is_none() {
        notes.push("The profile has no prices and this experiment has no earlier runs, so there is no dollar figure.".to_owned());
    }
    CostEstimate {
        tasks,
        modes: u32::try_from(m).unwrap_or(0),
        k,
        rollouts,
        improver_sessions: sessions,
        usd_low,
        usd_high,
        basis: "heuristic".to_owned(),
        notes,
    }
}

/// The task stream a sweep would run (domain order, optional seeded shuffle).
pub(crate) fn stream(cell: &Cell, opts: &RunOptions) -> Vec<StreamEntry> {
    let mut out = Vec::new();
    for (domain, ids) in state::domains_for(cell) {
        if !opts.only_domains.is_empty() && !opts.only_domains.iter().any(|d| d == domain) {
            continue;
        }
        let prefix = domain.to_lowercase().replace(' ', "-");
        let take = opts.max_tasks.map_or(ids.len(), |m| {
            usize::try_from(m).unwrap_or(usize::MAX).min(ids.len())
        });
        let mut ids: Vec<&str> = ids.into_iter().take(take).collect();
        if let Some(seed) = opts.seed {
            let mut rng = Rng::new(seed ^ seed_of(domain));
            for i in (1..ids.len()).rev() {
                let j = usize::try_from(rng.below(i as u64 + 1)).unwrap_or(0);
                ids.swap(i, j);
            }
        }
        for id in ids {
            out.push(StreamEntry {
                index: u32::try_from(out.len() + 1).unwrap_or(0),
                domain: domain.to_owned(),
                task: format!("{prefix}/{id}"),
            });
        }
    }
    out
}

pub(crate) fn preview(cell: &Cell, opts: &RunOptions, prices: Option<&Prices>) -> PreviewResult {
    let st = stream(cell, opts);
    let mut domains: Vec<DomainCount> = Vec::new();
    for e in &st {
        match domains.iter_mut().find(|d| d.domain == e.domain) {
            Some(d) => d.tasks += 1,
            None => domains.push(DomainCount {
                domain: e.domain.clone(),
                tasks: 1,
            }),
        }
    }
    let modes: Vec<Mode> = Mode::ALL
        .into_iter()
        .filter(|m| opts.modes.contains(m))
        .collect();
    let total = u32::try_from(st.len()).unwrap_or(0);
    PreviewResult {
        header: format!(
            "# {}-{} split={} seed={} k={} modes={}",
            cell.bench_name(),
            cell.substrate.as_cli(),
            opts.split.as_cli(),
            opts.seed
                .map_or_else(|| "none".to_owned(), |s| s.to_string()),
            opts.k,
            opts.modes_cli()
        ),
        namespaces: modes
            .iter()
            .map(|&m| ModeNamespace {
                mode: m,
                namespace: namespace(cell, m, opts.k, opts.seed),
            })
            .collect(),
        summary_namespace: namespace(cell, Mode::Parallel, opts.k, opts.seed),
        workers: opts.workers,
        total_domains: u32::try_from(domains.len()).unwrap_or(0),
        domains,
        total_tasks: total,
        estimate: estimate(total, opts.k, &modes, prices),
        stream: st,
    }
}

struct Ctx {
    s: Shared,
    id: String,
}

enum Stop {
    Cancelled,
}

impl Ctx {
    fn with<R>(&self, f: impl FnOnce(&mut Inner, usize) -> R) -> Option<R> {
        let mut g = lock(&self.s);
        let idx = g.job_index(&self.id)?;
        Some(f(&mut g, idx))
    }

    fn log(&self, text: &str) {
        self.with(|g, i| g.log(i, LogStream::Stdout, text));
    }

    fn err(&self, text: &str) {
        self.with(|g, i| g.log(i, LogStream::Stderr, text));
    }

    fn sys(&self, text: &str) {
        self.with(|g, i| g.log(i, LogStream::System, text));
    }

    async fn sleep(&self, ms: u64) -> Result<(), Stop> {
        let mut left = ms;
        while left > 0 {
            let chunk = left.min(250);
            tokio::time::sleep(Duration::from_millis(chunk)).await;
            left -= chunk;
            if self.with(|g, i| g.jobs[i].cancel).unwrap_or(true) {
                return Err(Stop::Cancelled);
            }
        }
        Ok(())
    }

    fn progress(&self, p: Progress) {
        self.with(|g, i| {
            g.jobs[i].job.progress = Some(p.clone());
            g.emit(i, JobEvent::Progress { progress: p });
        });
    }

    fn step(&self, name: &str, status: JobStatus) {
        self.with(|g, i| {
            let now = rfc3339(now_secs());
            let job = &mut g.jobs[i].job;
            let step = match job.steps.iter_mut().find(|s| s.name == name) {
                Some(s) => {
                    s.status = status;
                    if status.is_terminal() {
                        s.finished_at = Some(now);
                        s.exit_code = Some(i32::from(status != JobStatus::Succeeded));
                    }
                    s.clone()
                }
                None => {
                    let s = JobStep {
                        name: name.to_owned(),
                        status,
                        exit_code: None,
                        started_at: Some(now),
                        finished_at: None,
                    };
                    job.steps.push(s.clone());
                    s
                }
            };
            g.emit(i, JobEvent::Step { step });
        });
        if status == JobStatus::Running {
            self.sys(&format!("step {name}: started"));
        } else {
            self.sys(&format!("step {name}: {}", status.as_str()));
        }
    }

    fn spend(&self, usd: f64) -> f64 {
        self.with(|g, i| {
            let j = &mut g.jobs[i].job;
            let total = j.spent_usd.unwrap_or(0.0) + usd;
            j.spent_usd = Some(total);
            total
        })
        .unwrap_or(0.0)
    }

    fn finish(&self, status: JobStatus, result: Option<JobResult>, error: Option<String>) {
        self.with(|g, i| {
            let now = rfc3339(now_secs());
            let job = &mut g.jobs[i].job;
            job.status = status;
            job.finished_at = Some(now);
            job.exit_code = Some(match status {
                JobStatus::Succeeded => 0,
                JobStatus::Partial => 2,
                JobStatus::Cancelled => 143,
                _ => 1,
            });
            if result.is_some() {
                job.result = result;
            }
            job.error.clone_from(&error);
            for s in &mut job.steps {
                if !s.status.is_terminal() {
                    s.status = if status == JobStatus::Cancelled {
                        JobStatus::Cancelled
                    } else {
                        status
                    };
                }
            }
            let exit_code = job.exit_code;
            g.log(i, LogStream::System, &format!("job {}", status.as_str()));
            g.emit(
                i,
                JobEvent::Status {
                    status,
                    exit_code,
                    error,
                },
            );
        });
    }
}

/// Start simulating a queued job. `task_ms` is the time per task of a run.
pub(crate) fn spawn(s: Shared, id: String, task_ms: u64) {
    tokio::spawn(async move {
        let ctx = Ctx { s, id };
        // Single writer per experiment: wait while another writing job of it runs.
        loop {
            let busy = ctx
                .with(|g, i| {
                    let me = &g.jobs[i].job;
                    let Some(exp) = me.spec.experiment().map(str::to_owned) else {
                        return false;
                    };
                    let writes = |sp: &JobSpec| {
                        matches!(
                            sp,
                            JobSpec::Demo(_)
                                | JobSpec::Run(_)
                                | JobSpec::Judge(_)
                                | JobSpec::Pipeline(_)
                        )
                    };
                    writes(&me.spec)
                        && g.jobs.iter().any(|o| {
                            o.job.id != me.id
                                && o.job.status == JobStatus::Running
                                && writes(&o.job.spec)
                                && o.job.spec.experiment() == Some(exp.as_str())
                        })
                })
                .unwrap_or(false);
            let cancelled = ctx.with(|g, i| g.jobs[i].cancel).unwrap_or(true);
            if cancelled {
                ctx.finish(
                    JobStatus::Cancelled,
                    None,
                    Some("cancelled while queued".to_owned()),
                );
                return;
            }
            if !busy {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1000)).await;
        }
        ctx.with(|g, i| {
            let job = &mut g.jobs[i].job;
            job.status = JobStatus::Running;
            job.started_at = Some(rfc3339(now_secs()));
            g.emit(
                i,
                JobEvent::Status {
                    status: JobStatus::Running,
                    exit_code: None,
                    error: None,
                },
            );
        });
        ctx.sys("job started (isolation: host)");
        let spec = ctx.with(|g, i| g.jobs[i].job.spec.clone());
        let Some(spec) = spec else { return };
        match run(&ctx, &spec, task_ms).await {
            Ok((status, result, error)) => ctx.finish(status, result, error),
            Err(Stop::Cancelled) => {
                ctx.sys("cancellation requested: SIGTERM sent to the process group");
                ctx.finish(JobStatus::Cancelled, None, Some("cancelled".to_owned()));
            }
        }
    });
}

type Outcome = (JobStatus, Option<JobResult>, Option<String>);

async fn run(ctx: &Ctx, spec: &JobSpec, task_ms: u64) -> Result<Outcome, Stop> {
    match spec {
        JobSpec::InstallHarness2(p) => {
            for (step, lines) in [
                (
                    "fetch",
                    vec![
                        "fetching google-research/harness2",
                        "checked out the tested commit",
                    ],
                ),
                ("venv", vec!["creating .venv with Python 3.12.11"]),
                (
                    "install",
                    vec![
                        "Resolved 61 packages in 1.2s",
                        "Installed 61 packages in 3.4s",
                    ],
                ),
            ] {
                ctx.step(step, JobStatus::Running);
                for l in lines {
                    ctx.sleep(600).await?;
                    ctx.log(l);
                }
                ctx.step(step, JobStatus::Succeeded);
            }
            ctx.with(|g, _| {
                g.installed = true;
                g.benches = state::benchmarks(true);
            });
            Ok((
                JobStatus::Succeeded,
                Some(JobResult::Install(InstallResult {
                    commit: p.rev.clone().unwrap_or_else(|| TESTED_COMMIT.to_owned()),
                    version: "0.4.1".to_owned(),
                    python: "3.12.11".to_owned(),
                    extras: if p.extras.is_empty() {
                        vec![InstallExtra::Render]
                    } else {
                        p.extras.clone()
                    },
                })),
                None,
            ))
        }
        JobSpec::Demo(p) => {
            let tasks = [
                ("writing", "release-notes"),
                ("writing", "status-update"),
                ("analysis", "latency-table"),
                ("analysis", "cost-summary"),
            ];
            ctx.log(&format!(
                "harness2-demo --k {} --mode {} --output <experiment>",
                p.k,
                p.mode.as_cli()
            ));
            for (n, (d, t)) in tasks.iter().enumerate() {
                ctx.sleep(700).await?;
                let done = u32::try_from(n + 1).unwrap_or(0);
                ctx.log(&format!("[{d}] {}/2 ok {t}", n % 2 + 1));
                ctx.log(&format!(
                    "  editor: proposed {} edits, composed {} finals",
                    2 * p.k,
                    p.k
                ));
                ctx.progress(Progress {
                    done,
                    failed: 0,
                    total: Some(4),
                    current: Some(format!("{d} / {t}")),
                });
            }
            ctx.with(|g, _| {
                let e = g
                    .experiments
                    .entry(p.experiment.clone())
                    .or_insert_with(state::demo_experiment);
                for s in &mut e.sweeps {
                    s.updated_at = rfc3339(now_secs());
                }
            });
            ctx.log("demo: 4 done, 0 failed");
            Ok((
                JobStatus::Succeeded,
                Some(JobResult::Demo(DemoResult {
                    done: 4,
                    failed: 0,
                    execution_count: 4 * (p.k + 3 * p.k * 2),
                    editor_calls: 4 * 6 * p.k,
                })),
                None,
            ))
        }
        JobSpec::Setup(p) => {
            let plan = vec![
                format!("git clone {} (pinned)", p.bench.upstream_checkout()),
                "apply upstream patches".to_owned(),
                "uv sync (benchmark environment)".to_owned(),
                format!(
                    "build {} from the pinned checkout",
                    p.substrate.display_name()
                ),
                "download task data".to_owned(),
            ];
            let notes = if p.bench == Bench::Wb {
                vec!["Note: WorkBuddy-Bench needs Docker and is under a custom Tencent licence with a territorial limitation.".to_owned()]
            } else {
                Vec::new()
            };
            if p.dry_run {
                ctx.sleep(400).await?;
                for l in &plan {
                    ctx.log(&format!("would run: {l}"));
                }
                return Ok((
                    JobStatus::Succeeded,
                    Some(JobResult::Setup(SetupResult {
                        dry_run: true,
                        plan,
                        notes,
                    })),
                    None,
                ));
            }
            for l in &plan {
                ctx.sleep(700).await?;
                ctx.log(l);
                if p.bench == Bench::Wb && l.starts_with("uv sync") {
                    ctx.err(
                        "permission denied while trying to connect to the Docker daemon socket",
                    );
                    return Ok((
                        JobStatus::Failed,
                        None,
                        Some("docker: permission denied on the Docker socket".to_owned()),
                    ));
                }
            }
            ctx.with(|g, _| {
                for b in &mut g.benches {
                    if b.bench == p.bench && (p.subset.is_none() || b.subset == p.subset) {
                        b.installed = true;
                        b.at_pinned_commit = Some(true);
                        b.data_present = true;
                    }
                }
            });
            Ok((
                JobStatus::Succeeded,
                Some(JobResult::Setup(SetupResult {
                    dry_run: false,
                    plan,
                    notes,
                })),
                None,
            ))
        }
        JobSpec::Check(p) => {
            let r = check(ctx, &p.cell).await?;
            let status = if r.ready {
                JobStatus::Succeeded
            } else {
                JobStatus::Failed
            };
            let err = (!r.ready).then(|| "not ready".to_owned());
            Ok((status, Some(JobResult::Check(r)), err))
        }
        JobSpec::Preview(p) => {
            let prices = ctx
                .with(|g, _| {
                    p.profile
                        .as_ref()
                        .and_then(|n| g.profiles.get(n))
                        .and_then(|pr| pr.prices.clone())
                })
                .flatten();
            ctx.sleep(900).await?;
            let r = preview(&p.cell, &p.options, prices.as_ref());
            ctx.log(&r.header);
            for e in &r.stream {
                ctx.log(&format!("{:>3} [{}] {}", e.index, e.domain, e.task));
            }
            Ok((JobStatus::Succeeded, Some(JobResult::Preview(r)), None))
        }
        JobSpec::Run(p) => {
            let r = sweep(
                ctx,
                &p.experiment,
                &p.cell,
                &p.options,
                &p.profile,
                p.budget_usd,
                task_ms,
                false,
            )
            .await?;
            let status = if r.failed > 0 {
                JobStatus::Partial
            } else {
                JobStatus::Succeeded
            };
            Ok((status, Some(JobResult::Run(r)), None))
        }
        JobSpec::Judge(p) => {
            ctx.sleep(1500).await?;
            let r = judge(ctx, &p.experiment, &p.cell, p.k, p.seed, p.bases);
            Ok((JobStatus::Succeeded, Some(JobResult::Judge(r)), None))
        }
        JobSpec::Report(p) => {
            ctx.sleep(600).await?;
            let text = ctx
                .with(|g, _| {
                    let k = p.ks.first().copied().unwrap_or(1);
                    let id = namespace(&p.cell, Mode::Parallel, k, p.seed);
                    g.experiments
                        .get(&p.experiment)
                        .and_then(|e| e.sweep(&id))
                        .map(|s| sweeps::report(s).text)
                })
                .flatten()
                .unwrap_or_else(|| "no judged rows yet".to_owned());
            for l in text.lines() {
                ctx.log(l);
            }
            Ok((
                JobStatus::Succeeded,
                Some(JobResult::Report(ReportResult { text })),
                None,
            ))
        }
        JobSpec::Split(p) => {
            ctx.sleep(500).await?;
            let total = u32::try_from(
                state::domains_for(&Cell {
                    bench: p.bench,
                    subset: p.subset,
                    substrate: Substrate::Oc,
                })
                .iter()
                .map(|d| d.1.len())
                .sum::<usize>(),
            )
            .unwrap_or(0);
            let target = f64::from(total) * p.train_fraction;
            let train = (0..=total)
                .find(|t| f64::from(*t) + 0.5 > target)
                .unwrap_or(total);
            ctx.with(|g, _| {
                g.splits.push(SplitInfo {
                    name: p.name.clone(),
                    bench: p.bench,
                    subset: p.subset,
                    seed: p.seed,
                    train_fraction: p.train_fraction,
                    train,
                    test: total - train,
                    created_at: rfc3339(now_secs()),
                    chosen: false,
                });
            });
            ctx.log(&format!(
                "split {}: {train} train, {} test (seed {})",
                p.bench.as_cli(),
                total - train,
                p.seed
            ));
            Ok((
                JobStatus::Succeeded,
                Some(JobResult::Split(SplitResult {
                    name: p.name.clone(),
                    train,
                    test: total - train,
                })),
                None,
            ))
        }
        JobSpec::Bridge(p) => {
            let port = p.port.unwrap_or(4100);
            let host = if p.expose_to_containers {
                "0.0.0.0"
            } else {
                "127.0.0.1"
            };
            ctx.log(&format!(
                "harness2-bridge listening on http://{host}:{port}/v1"
            ));
            ctx.log("API key captured into secret HARNESS2_CODEX_API_KEY: [REDACTED:HARNESS2_CODEX_API_KEY]");
            let info = BridgeInfo {
                base_url: format!("http://127.0.0.1:{port}/v1"),
                model: p
                    .model
                    .clone()
                    .unwrap_or_else(|| "gemini-3.5-flash".to_owned()),
                container_url: p
                    .expose_to_containers
                    .then(|| format!("http://host.docker.internal:{port}/v1")),
            };
            // The server parses the bridge's URL as soon as it prints it.
            ctx.with(|g, i| g.jobs[i].job.result = Some(JobResult::Bridge(info)));
            loop {
                ctx.sleep(5000).await?;
                ctx.log("bridge: healthy, 0 requests in flight");
            }
        }
        JobSpec::Pipeline(p) => {
            let mut results = Vec::new();
            let mut names = vec!["preview", "check", "run", "judge"];
            if p.judge_bases {
                names.push("judge-bases");
            }
            names.push("report");
            for n in &names {
                ctx.with(|g, i| {
                    g.jobs[i].job.steps.push(h2w_api::JobStep {
                        name: (*n).to_owned(),
                        status: JobStatus::Queued,
                        exit_code: None,
                        started_at: None,
                        finished_at: None,
                    });
                });
            }
            ctx.step("preview", JobStatus::Running);
            ctx.sleep(600).await?;
            ctx.step("preview", JobStatus::Succeeded);
            ctx.step("check", JobStatus::Running);
            let c = check(ctx, &p.cell).await?;
            let ready = c.ready;
            results.push(JobResult::Check(c));
            if !ready {
                ctx.step("check", JobStatus::Failed);
                return Ok((
                    JobStatus::Failed,
                    Some(JobResult::Pipeline(PipelineResult { steps: results })),
                    Some("check failed".to_owned()),
                ));
            }
            ctx.step("check", JobStatus::Succeeded);
            ctx.step("run", JobStatus::Running);
            let r = sweep(
                ctx,
                &p.experiment,
                &p.cell,
                &p.options,
                &p.profile,
                p.budget_usd,
                task_ms,
                true,
            )
            .await?;
            let partial = r.failed > 0;
            results.push(JobResult::Run(r));
            ctx.step(
                "run",
                if partial {
                    JobStatus::Partial
                } else {
                    JobStatus::Succeeded
                },
            );
            ctx.step("judge", JobStatus::Running);
            ctx.sleep(1200).await?;
            results.push(JobResult::Judge(judge(
                ctx,
                &p.experiment,
                &p.cell,
                p.options.k,
                p.options.seed,
                false,
            )));
            ctx.step("judge", JobStatus::Succeeded);
            if p.judge_bases {
                ctx.step("judge-bases", JobStatus::Running);
                ctx.sleep(900).await?;
                results.push(JobResult::Judge(judge(
                    ctx,
                    &p.experiment,
                    &p.cell,
                    p.options.k,
                    p.options.seed,
                    true,
                )));
                ctx.step("judge-bases", JobStatus::Succeeded);
            }
            ctx.step("report", JobStatus::Running);
            ctx.sleep(400).await?;
            let text = ctx
                .with(|g, _| {
                    let id = namespace(&p.cell, Mode::Parallel, p.options.k, p.options.seed);
                    g.experiments
                        .get(&p.experiment)
                        .and_then(|e| e.sweep(&id))
                        .map(|s| sweeps::report(s).text)
                })
                .flatten()
                .unwrap_or_default();
            results.push(JobResult::Report(ReportResult { text }));
            ctx.step("report", JobStatus::Succeeded);
            Ok((
                if partial {
                    JobStatus::Partial
                } else {
                    JobStatus::Succeeded
                },
                Some(JobResult::Pipeline(PipelineResult { steps: results })),
                None,
            ))
        }
    }
}

async fn check(ctx: &Ctx, cell: &Cell) -> Result<CheckResult, Stop> {
    ctx.sleep(700).await?;
    let ready = ctx
        .with(|g, _| {
            g.installed
                && g.benches.iter().any(|b| {
                    b.bench == cell.bench
                        && (cell.subset.is_none() || b.subset == cell.subset)
                        && b.installed
                })
        })
        .unwrap_or(false);
    let tasks: usize = state::domains_for(cell).iter().map(|d| d.1.len()).sum();
    if ready {
        ctx.log(&format!(
            "Ready: {}-{}; {tasks} tasks.",
            cell.bench_name(),
            cell.substrate.as_cli()
        ));
        Ok(CheckResult {
            ready: true,
            tasks: u32::try_from(tasks).ok(),
            failures: Vec::new(),
        })
    } else {
        let f = format!(
            "{} is not set up; run setup from the Benchmarks page",
            cell.bench_name()
        );
        ctx.err(&format!("FAIL: {f}"));
        Ok(CheckResult {
            ready: false,
            tasks: None,
            failures: vec![f],
        })
    }
}

#[allow(clippy::too_many_arguments)]
async fn sweep(
    ctx: &Ctx,
    exp: &str,
    cell: &Cell,
    opts: &RunOptions,
    profile: &str,
    budget: Option<f64>,
    task_ms: u64,
    in_pipeline: bool,
) -> Result<RunResult, Stop> {
    let prices = ctx
        .with(|g, _| g.profiles.get(profile).and_then(|p| p.prices.clone()))
        .flatten();
    let pv = preview(cell, opts, prices.as_ref());
    if !in_pipeline {
        ctx.step("preview", JobStatus::Running);
        ctx.sleep(500).await?;
        ctx.step("preview", JobStatus::Succeeded);
        ctx.step("run", JobStatus::Running);
    }
    ctx.log(&pv.header);
    let total = pv.total_tasks;
    let id = pv.summary_namespace.clone();
    let modes: Vec<Mode> = Mode::ALL
        .into_iter()
        .filter(|m| opts.modes.contains(m))
        .collect();
    let models = ctx
        .with(|g, _| {
            g.profiles.get(profile).map(|p| {
                (
                    p.solver_model.clone(),
                    p.improver_model
                        .clone()
                        .unwrap_or_else(|| p.solver_model.clone()),
                )
            })
        })
        .flatten()
        .unwrap_or_else(|| ("solver".to_owned(), "improver".to_owned()));
    // Register the sweep so the experiment page shows it while it runs.
    ctx.with(|g, _| {
        let e = g
            .experiments
            .entry(exp.to_owned())
            .or_insert_with(|| ExpData {
                exp: h2w_api::Experiment {
                    name: exp.to_owned(),
                    kind: h2w_api::ExperimentKind::Standard,
                    description: String::new(),
                    tags: Vec::new(),
                    profile: Some(profile.to_owned()),
                    created_at: rfc3339(now_secs()),
                    created_by: "admin".to_owned(),
                    archived: false,
                    sweeps: Vec::new(),
                    size_bytes: None,
                },
                sweeps: Vec::new(),
            });
        e.sweeps.retain(|s| s.id != id);
        e.sweeps.push(SweepSpec {
            id: id.clone(),
            bench_name: cell.bench_name(),
            cell: Some(*cell),
            k: opts.k,
            seed: opts.seed,
            modes: modes.clone(),
            tasks: pv
                .stream
                .iter()
                .map(|s| TaskSpec {
                    id: s.task.clone(),
                    domain: s.domain.clone(),
                    state: TaskState::Running,
                })
                .collect(),
            judged: false,
            base_judged: false,
            updated_at: rfc3339(now_secs()),
            models,
        });
    });
    let mut rng = Rng::new(seed_of(&id));
    let (mut done, mut failed) = (0_u32, 0_u32);
    for (n, e) in pv.stream.iter().enumerate() {
        ctx.progress(Progress {
            done,
            failed,
            total: Some(total),
            current: Some(format!("{} / {}", e.domain, e.task)),
        });
        ctx.log(&format!(
            "[{}] starting {} (k={}, modes={})",
            e.domain,
            e.task,
            opts.k,
            opts.modes_cli()
        ));
        ctx.sleep(task_ms / 3).await?;
        ctx.log(&format!("  base: {} reference rollouts", opts.k));
        ctx.sleep(task_ms / 3).await?;
        ctx.log(&format!(
            "  improver: {} proposals probed, {} compositions",
            2 * opts.k * u32::try_from(modes.len()).unwrap_or(1),
            opts.k
        ));
        ctx.sleep(task_ms / 3).await?;
        let partial = rng.unit() < 0.12;
        let state = if partial {
            TaskState::Partial
        } else {
            TaskState::Ok
        };
        if partial {
            failed += 1;
            ctx.err(&format!("  {}: c2 rollout timed out after 1800 s", e.task));
        } else {
            done += 1;
        }
        let spent = ctx.spend(0.18 + rng.unit() * 0.5);
        ctx.log(&format!(
            "[{}] {}/{} {} {}",
            e.domain,
            n + 1,
            total,
            if partial { "partial" } else { "ok" },
            e.task
        ));
        ctx.with(|g, _| {
            if let Some(sw) = g
                .experiments
                .get_mut(exp)
                .and_then(|x| x.sweeps.iter_mut().find(|s| s.id == id))
                && let Some(t) = sw.tasks.get_mut(n)
            {
                t.state = state;
                sw.updated_at = rfc3339(now_secs());
            }
        });
        if let Some(cap) = budget
            && spent >= cap
        {
            ctx.err(&format!(
                "budget reached: recorded spend {spent:.2} USD >= cap {cap:.2} USD"
            ));
            ctx.with(|g, i| g.jobs[i].cancel = true);
            return Err(Stop::Cancelled);
        }
    }
    ctx.progress(Progress {
        done,
        failed,
        total: Some(total),
        current: None,
    });
    ctx.log(&format!("sweep finished: {done} done, {failed} failed"));
    Ok(RunResult {
        namespace: id.clone(),
        done,
        failed,
        dead_domains: Vec::new(),
        record: Some(format!("{id}-{}.json", now_secs())),
        next: Some(format!(
            "harness2-judge --bench {} --k {} --modes {}",
            cell.bench.as_cli(),
            opts.k,
            opts.modes_cli()
        )),
    })
}

fn judge(ctx: &Ctx, exp: &str, cell: &Cell, k: u32, seed: Option<u64>, bases: bool) -> JudgeResult {
    let id = namespace(cell, Mode::Parallel, k, seed);
    let r = ctx
        .with(|g, _| {
            let sw = g
                .experiments
                .get_mut(exp)?
                .sweeps
                .iter_mut()
                .find(|s| s.id == id)?;
            if bases {
                sw.base_judged = true;
            } else {
                sw.judged = true;
            }
            let s = sweeps::scores(sw);
            Some(if bases {
                let n = u32::try_from(s.base_rows.len()).unwrap_or(0);
                JudgeResult {
                    bases: true,
                    rows: n,
                    graded: n,
                    ungraded: 0,
                }
            } else {
                let rows = u32::try_from(s.rows.len()).unwrap_or(0);
                let graded = u32::try_from(s.rows.iter().filter(|r| r.graded).count()).unwrap_or(0);
                JudgeResult {
                    bases: false,
                    rows,
                    graded,
                    ungraded: rows - graded,
                }
            })
        })
        .flatten()
        .unwrap_or(JudgeResult {
            bases,
            rows: 0,
            graded: 0,
            ungraded: 0,
        });
    ctx.log(&format!(
        "judge{}: {} rows, {} graded, {} ungraded",
        if bases { " --bases" } else { "" },
        r.rows,
        r.graded,
        r.ungraded
    ));
    r
}
