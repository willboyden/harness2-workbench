//! Domain widgets shared by several pages.

use h2s_api::{
    CheckLevel, DoctorReport, ExperimentKind, InstallParams, Job, JobSpec, Mode, ModeScore, Role,
    SystemInfo,
};
use leptos::prelude::*;
use leptos_router::hooks::use_navigate;

use super::icons::{Icon, icon};
use super::ui::{Ago, Badge, Notice, ProgressBar, StatusBadge, Term};
use crate::actions;
use crate::logic::format;
use crate::logic::jobs::{self, Tone};
use crate::logic::urls::route;
use crate::state::{session, toasts};

pub(crate) fn mode_label(m: Mode) -> &'static str {
    match m {
        Mode::Parallel => "parallel",
        Mode::Sequential => "sequential",
    }
}

pub(crate) fn kind_badge(kind: ExperimentKind) -> impl IntoView {
    match kind {
        ExperimentKind::Standard => view! { <Badge tone=Tone::Neutral text="benchmark" /> },
        ExperimentKind::Demo => view! { <Badge tone=Tone::Info text="demo" /> },
    }
}

/// `parallel k=1: 62.5% · all-pass 20.0% (12 graded)` chips, one per mode.
#[component]
pub(crate) fn HeadlineScores(scores: Option<Vec<ModeScore>>) -> impl IntoView {
    match scores {
        None => view! { <span class="muted small">"not judged yet"</span> }.into_any(),
        Some(list) if list.is_empty() => {
            view! { <span class="muted small">"no graded tasks"</span> }.into_any()
        }
        Some(list) => view! {
            <span class="score-chips">
                {list
                    .into_iter()
                    .map(|m| {
                        let title = format!(
                            "{} mode, k = {}: mean partial credit {}, all-pass {}, over {} graded tasks",
                            mode_label(m.mode),
                            m.k,
                            format::opt_pct(m.score),
                            format::opt_pct(m.all_pass),
                            m.graded
                        );
                        view! {
                            <span class="score-chip" title=title>
                                <span class="score-chip-mode">{format!("{} k={}", mode_label(m.mode), m.k)}</span>
                                <strong>{format::opt_pct(m.score)}</strong>
                                <span class="muted">{format!("all-pass {}", format::opt_pct(m.all_pass))}</span>
                            </span>
                        }
                    })
                    .collect_view()}
            </span>
        }
        .into_any(),
    }
}

/// The harness2 installation block (`data-testid="harness2-status"`).
#[component]
pub(crate) fn Harness2Status(info: SystemInfo) -> impl IntoView {
    let s = session();
    let t = toasts();
    let navigate = use_navigate();
    let h = info.harness2;
    let busy = RwSignal::new(false);
    let install = move |_| {
        actions::start_job(
            JobSpec::InstallHarness2(InstallParams {
                rev: None,
                extras: vec![h2s_api::InstallExtra::Render],
                reinstall: false,
            }),
            t,
            Some(busy),
            navigate.clone(),
        );
    };
    let tested = h.tested_commit.clone();
    let mismatch = h
        .commit
        .as_ref()
        .is_some_and(|c| !c.is_empty() && *c != tested);
    view! {
        <div class="h2-status" data-testid="harness2-status">
            {if h.installed {
                view! {
                    <div class="h2-status-row">
                        <Badge tone=Tone::Success text="installed" />
                        <span>
                            {h.version.clone().map(|v| format!("harness2 {v}")).unwrap_or_else(|| "harness2".to_owned())}
                        </span>
                    </div>
                    <dl class="kv">
                        <dt>"Commit"</dt>
                        <dd>
                            <code>{h.commit.clone().map(|c| format::short_commit(&c)).unwrap_or_else(|| "unknown".to_owned())}</code>
                        </dd>
                        <dt>"Tested with"</dt>
                        <dd>
                            <code>{format::short_commit(&tested)}</code>
                        </dd>
                        <dt>"Python"</dt>
                        <dd>{h.python.clone().unwrap_or_else(|| format::DASH.to_owned())}</dd>
                        <dt>"Checkout"</dt>
                        <dd>{if h.managed { "managed by the Studio" } else { "existing checkout (not modified)" }}</dd>
                    </dl>
                    {mismatch
                        .then(|| {
                            view! {
                                <Notice tone=Tone::Warning>
                                    <p>
                                        "This harness2 commit differs from the one this Studio release was tested against. Most things should work; parsing of new output formats may not."
                                    </p>
                                </Notice>
                            }
                        })}
                }
                    .into_any()
            } else {
                view! {
                    <Notice tone=Tone::Warning title="harness2 is not installed">
                        <p>
                            "Nothing can run until harness2 is installed: the Studio fetches it at the tested commit, creates its Python 3.12 environment and installs it."
                        </p>
                        {move || {
                            if s.can(Role::Admin) {
                                view! {
                                    <button
                                        type="button"
                                        class="btn btn-primary btn-sm"
                                        disabled=move || busy.get()
                                        on:click=install.clone()
                                    >
                                        {icon(Icon::Download)}
                                        " Install harness2"
                                    </button>
                                }
                                    .into_any()
                            } else {
                                view! { <p class="muted small">"Ask an administrator to install it."</p> }.into_any()
                            }
                        }}
                    </Notice>
                }
                    .into_any()
            }}
        </div>
    }
}

pub(crate) fn level_tone(l: CheckLevel) -> Tone {
    match l {
        CheckLevel::Ok => Tone::Success,
        CheckLevel::Warn => Tone::Warning,
        CheckLevel::Fail => Tone::Danger,
        CheckLevel::Skip => Tone::Neutral,
    }
}

pub(crate) fn level_label(l: CheckLevel) -> &'static str {
    match l {
        CheckLevel::Ok => "ok",
        CheckLevel::Warn => "warning",
        CheckLevel::Fail => "failed",
        CheckLevel::Skip => "skipped",
    }
}

/// Counts and the checks that need attention, with remedies.
#[component]
pub(crate) fn DoctorSummary(
    report: DoctorReport,
    #[prop(default = 4)] max_items: usize,
) -> impl IntoView {
    let count = |l: CheckLevel| report.checks.iter().filter(|c| c.level == l).count();
    let (ok, warn, fail) = (
        count(CheckLevel::Ok),
        count(CheckLevel::Warn),
        count(CheckLevel::Fail),
    );
    let mut attention: Vec<_> = report
        .checks
        .iter()
        .filter(|c| matches!(c.level, CheckLevel::Fail | CheckLevel::Warn))
        .cloned()
        .collect();
    attention.sort_by_key(|c| c.level != CheckLevel::Fail);
    let more = attention.len().saturating_sub(max_items);
    attention.truncate(max_items);
    view! {
        <div class="doctor-summary">
            <div class="doctor-counts">
                <Badge tone=Tone::Success text=format!("{ok} ok") />
                <Badge tone=Tone::Warning text=format!("{warn} warnings") />
                <Badge tone=Tone::Danger text=format!("{fail} failed") />
                <span class="muted small">"checked " <Ago ts=report.generated_at.clone() /></span>
            </div>
            {if attention.is_empty() {
                view! { <p class="muted">"Everything the Studio checks is in order."</p> }.into_any()
            } else {
                view! {
                    <ul class="check-list">
                        {attention
                            .into_iter()
                            .map(|c| {
                                view! {
                                    <li class=format!("check {}", level_tone(c.level).class())>
                                        <span class="check-head">
                                            <Badge tone=level_tone(c.level) text=level_label(c.level) />
                                            <strong>{c.label.clone()}</strong>
                                        </span>
                                        <span class="small">{c.detail.clone()}</span>
                                        {c.remedy.clone().map(|r| view! { <span class="small remedy">"Fix: "{r}</span> })}
                                    </li>
                                }
                            })
                            .collect_view()}
                    </ul>
                }
                    .into_any()
            }}
            {(more > 0).then(|| view! { <p class="muted small">{format!("…and {more} more.")}</p> })}
        </div>
    }
}

/// A compact job row: title, status, progress and age.
#[component]
pub(crate) fn JobMini(job: Job) -> impl IntoView {
    let progress = job.progress.clone();
    let fraction = progress.as_ref().and_then(jobs::progress_fraction);
    let label = progress
        .as_ref()
        .map(jobs::progress_text)
        .unwrap_or_else(|| jobs::status_help(job.status).to_owned());
    let running = job.status == h2s_api::JobStatus::Running;
    let status = job.status;
    view! {
        <li class="job-mini">
            <a href=route::job(&job.id) class="job-mini-link">
                <span class="job-mini-title">{jobs::spec_title(&job.spec)}</span>
                <span class="job-mini-meta">
                    <StatusBadge status=status />
                    <span class="muted small">
                        {jobs::kind_label(job.kind)}" · "<Ago ts=job.created_at.clone() />
                    </span>
                </span>
            </a>
            {running
                .then(|| {
                    view! {
                        <ProgressBar fraction=fraction label=label.clone() />
                        <span class="muted small">{label}</span>
                    }
                })}
        </li>
    }
}

/// "What is this?" explainer of the two recursion levels, used on the dashboard and welcome.
#[component]
pub(crate) fn RecursionExplainer() -> impl IntoView {
    view! {
        <div class="explainer">
            <div class="explainer-item">
                <h3>
                    <Term id="task-recursion">"Task-level recursion"</Term>
                </h3>
                <p>
                    "On each task, an improver model proposes harness edits, each edit is probed by running the task, and the evidence is composed into refined harnesses ("
                    <Term id="ppc">"Propose–Probe–Compose"</Term>
                    "). A blind selector adopts the best run."
                </p>
            </div>
            <div class="explainer-item">
                <h3>
                    <Term id="harnessing-recursion">"Harnessing-level recursion"</Term>
                </h3>
                <p>
                    "Every adoption and the lesson learned from it go into a per-domain "
                    <Term id="library">"experience library"</Term>
                    ", so later tasks of the same domain start from what earlier ones learned."
                </p>
            </div>
        </div>
    }
}
