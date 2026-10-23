//! Job view-model: labels, titles, tones, live-event application and reconnect backoff.

use h2s_api::{
    Bench, Cell, DemoMode, Job, JobEvent, JobKind, JobResult, JobSpec, JobStatus, Mode, Progress,
    Role,
};

/// Visual tone of a badge or notice (maps to CSS classes `tone-*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    Neutral,
    Info,
    Success,
    Warning,
    Danger,
}

impl Tone {
    pub(crate) fn class(self) -> &'static str {
        match self {
            Tone::Neutral => "tone-neutral",
            Tone::Info => "tone-info",
            Tone::Success => "tone-success",
            Tone::Warning => "tone-warning",
            Tone::Danger => "tone-danger",
        }
    }
}

pub(crate) fn status_tone(s: JobStatus) -> Tone {
    match s {
        JobStatus::Queued | JobStatus::Cancelled => Tone::Neutral,
        JobStatus::Running => Tone::Info,
        JobStatus::Succeeded => Tone::Success,
        JobStatus::Partial => Tone::Warning,
        JobStatus::Failed => Tone::Danger,
    }
}

/// One sentence explaining a status to a newcomer.
pub(crate) fn status_help(s: JobStatus) -> &'static str {
    match s {
        JobStatus::Queued => {
            "Waiting for a free slot (or for another job writing to the same experiment)."
        }
        JobStatus::Running => "Running now.",
        JobStatus::Succeeded => "Finished: everything requested completed.",
        JobStatus::Partial => {
            "Finished, but some tasks failed or stayed ungraded (harness2 exit code 2)."
        }
        JobStatus::Failed => "Stopped with an error.",
        JobStatus::Cancelled => "Cancelled before it finished.",
    }
}

pub(crate) const ALL_STATUSES: [JobStatus; 6] = [
    JobStatus::Queued,
    JobStatus::Running,
    JobStatus::Succeeded,
    JobStatus::Partial,
    JobStatus::Failed,
    JobStatus::Cancelled,
];

pub(crate) const ALL_KINDS: [JobKind; 11] = [
    JobKind::Run,
    JobKind::Pipeline,
    JobKind::Preview,
    JobKind::Judge,
    JobKind::Report,
    JobKind::Demo,
    JobKind::Check,
    JobKind::Setup,
    JobKind::Split,
    JobKind::InstallHarness2,
    JobKind::Bridge,
];

pub(crate) fn kind_label(k: JobKind) -> &'static str {
    match k {
        JobKind::InstallHarness2 => "Install harness2",
        JobKind::Demo => "Demo",
        JobKind::Setup => "Benchmark setup",
        JobKind::Check => "Readiness check",
        JobKind::Preview => "Preview (dry run)",
        JobKind::Run => "Refinement run",
        JobKind::Judge => "Judging",
        JobKind::Report => "Report",
        JobKind::Split => "Train/test split",
        JobKind::Bridge => "Codex bridge",
        JobKind::Pipeline => "Pipeline",
    }
}

pub(crate) fn kind_from_str(s: &str) -> Option<JobKind> {
    ALL_KINDS.into_iter().find(|k| k.as_str() == s)
}

pub(crate) fn status_from_str(s: &str) -> Option<JobStatus> {
    ALL_STATUSES.into_iter().find(|k| k.as_str() == s)
}

/// `LAB · OpenCode`, `WorkBuddy-Bench office · Codex`.
pub(crate) fn cell_label(c: &Cell) -> String {
    match c.subset {
        Some(sub) if c.bench == Bench::Wb => format!(
            "{} {} · {}",
            c.bench.display_name(),
            sub.as_cli(),
            c.substrate.display_name()
        ),
        _ => format!(
            "{} · {}",
            c.bench.display_name(),
            c.substrate.display_name()
        ),
    }
}

/// `parallel + sequential`.
pub(crate) fn modes_label(modes: &[Mode]) -> String {
    let mut v: Vec<&str> = Mode::ALL
        .iter()
        .filter(|m| modes.contains(m))
        .map(|m| m.as_cli())
        .collect();
    if v.is_empty() {
        v.push("no modes");
    }
    v.join(" + ")
}

fn demo_mode_label(m: DemoMode) -> &'static str {
    match m {
        DemoMode::Parallel => "parallel",
        DemoMode::Sequential => "sequential",
        DemoMode::Both => "parallel + sequential",
    }
}

/// A one-line description of what a job does.
pub(crate) fn spec_title(spec: &JobSpec) -> String {
    match spec {
        JobSpec::InstallHarness2(p) => match &p.rev {
            Some(r) => format!("Install harness2 at {}", super::format::short_commit(r)),
            None => "Install harness2 (tested commit)".to_owned(),
        },
        JobSpec::Demo(p) => format!(
            "Demo into {} · k={} · {}",
            p.experiment,
            p.k,
            demo_mode_label(p.mode)
        ),
        JobSpec::Setup(p) => {
            let sub = p
                .subset
                .map(|s| format!(" {}", s.as_cli()))
                .unwrap_or_default();
            format!(
                "{}Set up {}{sub} for {}",
                if p.dry_run { "Plan: " } else { "" },
                p.bench.display_name(),
                p.substrate.display_name()
            )
        }
        JobSpec::Check(p) => format!("Check {}", cell_label(&p.cell)),
        JobSpec::Preview(p) => format!(
            "Preview {} · k={} · {}",
            cell_label(&p.cell),
            p.options.k,
            modes_label(&p.options.modes)
        ),
        JobSpec::Run(p) => format!(
            "Run {} · k={} · {}",
            cell_label(&p.cell),
            p.options.k,
            modes_label(&p.options.modes)
        ),
        JobSpec::Judge(p) => format!(
            "Judge {}{} · k={} · {}",
            if p.bases { "base runs of " } else { "" },
            cell_label(&p.cell),
            p.k,
            modes_label(&p.modes)
        ),
        JobSpec::Report(p) => {
            let ks: Vec<String> = p.ks.iter().map(u32::to_string).collect();
            format!("Report {} · k={}", cell_label(&p.cell), ks.join(","))
        }
        JobSpec::Split(p) => {
            let sub = p
                .subset
                .map(|s| format!(" {}", s.as_cli()))
                .unwrap_or_default();
            format!("Split {}{sub} into “{}”", p.bench.display_name(), p.name)
        }
        JobSpec::Bridge(p) => format!(
            "Codex bridge{}",
            p.model
                .as_ref()
                .map(|m| format!(" for {m}"))
                .unwrap_or_default()
        ),
        JobSpec::Pipeline(p) => format!(
            "Pipeline {} · k={} · {}",
            cell_label(&p.cell),
            p.options.k,
            modes_label(&p.options.modes)
        ),
    }
}

/// Fraction of the stream finished (`done + failed` over `total`), when the total is known.
pub(crate) fn progress_fraction(p: &Progress) -> Option<f64> {
    let total = p.total.filter(|t| *t > 0)?;
    Some((f64::from(p.done + p.failed) / f64::from(total)).clamp(0.0, 1.0))
}

/// `7 of 12 tasks · 1 failed`.
pub(crate) fn progress_text(p: &Progress) -> String {
    let finished = p.done + p.failed;
    let base = match p.total {
        Some(t) => format!("{finished} of {t} tasks"),
        None => format!("{finished} tasks finished"),
    };
    if p.failed > 0 {
        format!("{base} · {} failed", p.failed)
    } else {
        base
    }
}

/// Apply a live event to a job. Returns `true` when something visible changed.
pub(crate) fn apply_event(job: &mut Job, ev: &JobEvent) -> bool {
    match ev {
        JobEvent::Snapshot { job: snap } => {
            *job = (**snap).clone();
            true
        }
        JobEvent::Status {
            status,
            exit_code,
            error,
        } => {
            job.status = *status;
            if exit_code.is_some() {
                job.exit_code = *exit_code;
            }
            if error.is_some() {
                job.error.clone_from(error);
            }
            true
        }
        JobEvent::Log { line } => {
            let before = job.log_lines;
            job.log_lines = job.log_lines.max(line.n + 1);
            job.log_lines != before
        }
        JobEvent::Progress { progress } => {
            job.progress = Some(progress.clone());
            true
        }
        JobEvent::Step { step } => {
            match job.steps.iter_mut().find(|s| s.name == step.name) {
                Some(s) => *s = step.clone(),
                None => job.steps.push(step.clone()),
            }
            true
        }
        JobEvent::Heartbeat { .. } => false,
    }
}

/// Operators and admins may cancel a job that has not finished.
pub(crate) fn can_cancel(role: Role, job: &Job) -> bool {
    role.allows(Role::Operator) && !job.status.is_terminal()
}

/// The sweep a finished run produced: `(experiment, summary namespace)`.
pub(crate) fn produced_sweep(job: &Job) -> Option<(String, String)> {
    let exp = job.spec.experiment()?.to_owned();
    let ns = match job.result.as_ref()? {
        JobResult::Run(r) => Some(r.namespace.clone()),
        JobResult::Pipeline(p) => p.steps.iter().find_map(|s| match s {
            JobResult::Run(r) => Some(r.namespace.clone()),
            _ => None,
        }),
        _ => None,
    }?;
    Some((exp, ns))
}

/// Reconnect delay for the live event stream: 1 s, 2 s, 4 s ... capped at 30 s.
pub(crate) fn backoff_ms(attempt: u32) -> u32 {
    1000_u32.saturating_mul(1_u32 << attempt.min(5)).min(30_000)
}

/// After this many consecutive failed connections, the job page polls instead.
pub(crate) const SSE_FAILURES_BEFORE_POLLING: u32 = 4;

/// How the job page is receiving updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LiveMode {
    Connecting,
    Live,
    Reconnecting(u32),
    Polling,
    /// The job is terminal and the log is complete; nothing more will arrive.
    Ended,
}

impl LiveMode {
    pub(crate) fn label(self) -> String {
        match self {
            LiveMode::Connecting => "Connecting to live updates…".to_owned(),
            LiveMode::Live => "Live".to_owned(),
            LiveMode::Reconnecting(n) => format!("Reconnecting (attempt {n})…"),
            LiveMode::Polling => "Live stream unavailable; refreshing every few seconds".to_owned(),
            LiveMode::Ended => "Finished".to_owned(),
        }
    }

    pub(crate) fn tone(self) -> Tone {
        match self {
            LiveMode::Live => Tone::Success,
            LiveMode::Connecting | LiveMode::Reconnecting(_) => Tone::Info,
            LiveMode::Polling => Tone::Warning,
            LiveMode::Ended => Tone::Neutral,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use h2s_api::{
        DemoParams, Isolation, JobStep, LogLine, LogStream, RunOptions, RunParams, RunResult,
        Substrate,
    };

    fn job() -> Job {
        Job {
            id: "j1".into(),
            kind: JobKind::Demo,
            spec: JobSpec::Demo(DemoParams {
                experiment: "demo".into(),
                k: 2,
                mode: DemoMode::Both,
            }),
            status: JobStatus::Queued,
            created_by: "admin".into(),
            created_at: "2026-10-23T12:00:00Z".into(),
            started_at: None,
            finished_at: None,
            exit_code: None,
            isolation: Isolation::Host,
            steps: vec![],
            progress: None,
            result: None,
            error: None,
            log_lines: 0,
            spent_usd: None,
        }
    }

    #[test]
    fn titles() {
        assert_eq!(
            spec_title(&job().spec),
            "Demo into demo · k=2 · parallel + sequential"
        );
        let run = JobSpec::Run(RunParams {
            experiment: "e".into(),
            cell: Cell {
                bench: Bench::Wb,
                subset: Some(h2s_api::WbSubset::Office),
                substrate: Substrate::Cx,
            },
            options: RunOptions {
                k: 3,
                modes: vec![Mode::Sequential, Mode::Parallel],
                ..RunOptions::default()
            },
            profile: "p".into(),
            budget_usd: None,
            acknowledge_risks: true,
        });
        assert_eq!(
            spec_title(&run),
            "Run WorkBuddy-Bench office · Codex · k=3 · parallel + sequential"
        );
    }

    #[test]
    fn events_update_the_job() {
        let mut j = job();
        assert!(apply_event(
            &mut j,
            &JobEvent::Status {
                status: JobStatus::Running,
                exit_code: None,
                error: None
            }
        ));
        assert_eq!(j.status, JobStatus::Running);
        let line = LogLine {
            n: 41,
            ts: "t".into(),
            stream: LogStream::Stdout,
            text: "x".into(),
        };
        assert!(apply_event(&mut j, &JobEvent::Log { line }));
        assert_eq!(j.log_lines, 42);
        let step = JobStep {
            name: "run".into(),
            status: JobStatus::Running,
            exit_code: None,
            started_at: None,
            finished_at: None,
        };
        apply_event(&mut j, &JobEvent::Step { step: step.clone() });
        apply_event(
            &mut j,
            &JobEvent::Step {
                step: JobStep {
                    status: JobStatus::Succeeded,
                    ..step
                },
            },
        );
        assert_eq!(j.steps.len(), 1);
        assert_eq!(j.steps[0].status, JobStatus::Succeeded);
        assert!(!apply_event(
            &mut j,
            &JobEvent::Heartbeat { ts: "t".into() }
        ));
        let mut snap = job();
        snap.status = JobStatus::Failed;
        apply_event(
            &mut j,
            &JobEvent::Snapshot {
                job: Box::new(snap),
            },
        );
        assert_eq!(j.status, JobStatus::Failed);
        assert_eq!(j.log_lines, 0);
    }

    #[test]
    fn progress_and_permissions() {
        let p = Progress {
            done: 6,
            failed: 1,
            total: Some(14),
            current: None,
        };
        assert_eq!(progress_fraction(&p), Some(0.5));
        assert_eq!(progress_text(&p), "7 of 14 tasks · 1 failed");
        let unknown = Progress {
            total: None,
            failed: 0,
            ..p
        };
        assert_eq!(progress_fraction(&unknown), None);
        assert_eq!(progress_text(&unknown), "6 tasks finished");
        let mut j = job();
        assert!(can_cancel(Role::Operator, &j));
        assert!(!can_cancel(Role::Viewer, &j));
        j.status = JobStatus::Succeeded;
        assert!(!can_cancel(Role::Admin, &j));
    }

    #[test]
    fn produced_sweep_from_results() {
        let mut j = job();
        assert_eq!(produced_sweep(&j), None);
        j.result = Some(JobResult::Pipeline(h2s_api::PipelineResult {
            steps: vec![JobResult::Run(RunResult {
                namespace: "lab-oc-agg1".into(),
                done: 1,
                failed: 0,
                dead_domains: vec![],
                record: None,
                next: None,
            })],
        }));
        assert_eq!(
            produced_sweep(&j),
            Some(("demo".to_owned(), "lab-oc-agg1".to_owned()))
        );
    }

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff_ms(0), 1000);
        assert_eq!(backoff_ms(1), 2000);
        assert_eq!(backoff_ms(3), 8000);
        assert_eq!(backoff_ms(5), 30_000);
        assert_eq!(backoff_ms(u32::MAX), 30_000);
    }

    #[test]
    fn string_round_trips() {
        for k in ALL_KINDS {
            assert_eq!(kind_from_str(k.as_str()), Some(k));
        }
        for s in ALL_STATUSES {
            assert_eq!(status_from_str(s.as_str()), Some(s));
        }
        assert_eq!(kind_from_str("nope"), None);
    }
}
