//! The home page: is the lab ready, what is running, what came out of recent experiments.

use std::time::Duration;

use h2w_api::{DoctorReport, Experiment, Job, Page, Role};
use leptos::prelude::*;
use leptos_router::hooks::use_navigate;

use crate::actions;
use crate::api;
use crate::app::load_system;
use crate::browser;
use crate::components::icons::{Icon, icon};
use crate::components::ui::{Card, EmptyState, PageHeader, Skeleton, Stat, loaded};
use crate::components::widgets::{
    DoctorSummary, Harness2Status, HeadlineScores, JobMini, RecursionExplainer, kind_badge,
};
use crate::logic::format;
use crate::logic::urls::{api as paths, route};
use crate::state::{session, toasts};

const WELCOME_DISMISSED: &str = "h2w-welcome-dismissed";

#[component]
pub(crate) fn Dashboard() -> impl IntoView {
    let s = session();
    let t = toasts();
    let navigate = use_navigate();
    load_system(s);

    let tick = RwSignal::new(0_u32);
    if let Ok(h) =
        set_interval_with_handle(move || tick.update(|n| *n += 1), Duration::from_secs(5))
    {
        on_cleanup(move || h.clear());
    }
    let refresh_doctor = RwSignal::new(false);
    let doctor = LocalResource::new(move || {
        let refresh = refresh_doctor.get();
        async move { api::get::<DoctorReport>(&paths::doctor(refresh)).await }
    });
    let experiments = LocalResource::new(|| async {
        api::get::<Vec<Experiment>>(&paths::experiments(false)).await
    });
    let active = LocalResource::new(move || {
        tick.track();
        async move {
            let running = api::get::<Page<Job>>(&paths::jobs("running", "", "", 20, None)).await?;
            let queued = api::get::<Page<Job>>(&paths::jobs("queued", "", "", 20, None)).await?;
            let mut all = running.items;
            all.extend(queued.items);
            Ok::<_, api::ApiError>(all)
        }
    });
    let recent = LocalResource::new(move || {
        tick.track();
        async move {
            let page = api::get::<Page<Job>>(&paths::jobs("", "", "", 12, None)).await?;
            Ok::<_, api::ApiError>(
                page.items
                    .into_iter()
                    .filter(|j| j.status.is_terminal())
                    .take(5)
                    .collect::<Vec<_>>(),
            )
        }
    });

    let dismissed = RwSignal::new(browser::storage_get(WELCOME_DISMISSED).is_some());
    let first_time = move || {
        let no_experiments = experiments
            .get()
            .and_then(Result::ok)
            .is_some_and(|e| e.is_empty());
        let not_installed = s
            .system
            .with(|i| i.as_ref().is_some_and(|i| !i.harness2.installed));
        !dismissed.get() && (no_experiments || not_installed)
    };
    let demo_busy = RwSignal::new(false);
    let run_demo = {
        let navigate = navigate.clone();
        move |_| actions::start_job(actions::demo_spec(), t, Some(demo_busy), navigate.clone())
    };

    let stats = move || {
        let exps = experiments.get().and_then(Result::ok).unwrap_or_default();
        let sweeps: usize = exps.iter().map(|e| e.sweeps.len()).sum();
        let spend: f64 = exps
            .iter()
            .flat_map(|e| e.sweeps.iter())
            .map(|s| s.usd)
            .sum();
        let running = s
            .system
            .with(|i| {
                i.as_ref()
                    .map(|i| format!("{} / {}", i.jobs_running, i.jobs_max_concurrent))
            })
            .unwrap_or_else(|| format::DASH.to_owned());
        (exps.len(), sweeps, spend, running)
    };

    view! {
        <div class="page" data-testid="dashboard">
            <PageHeader title="Dashboard" subtitle="Your harness2 lab at a glance.">
                {move || {
                    s.can(Role::Operator)
                        .then(|| {
                            view! {
                                <button
                                    type="button"
                                    class="btn btn-secondary"
                                    disabled=move || demo_busy.get()
                                    on:click=run_demo.clone()
                                >
                                    {icon(Icon::Play)}
                                    " Run the demo"
                                </button>
                                <a class="btn btn-primary" href="/new">
                                    {icon(Icon::Plus)}
                                    " New run"
                                </a>
                            }
                        })
                }}
                {move || {
                    // Benchmark setup is an administrator's job.
                    s.can(Role::Admin)
                        .then(|| {
                            view! {
                                <a class="btn btn-secondary" href="/benchmarks">
                                    {icon(Icon::Box)}
                                    " Set up a benchmark"
                                </a>
                            }
                        })
                }}
            </PageHeader>

            {move || {
                first_time()
                    .then(|| {
                        view! {
                            <section class="banner" aria-labelledby="welcome-banner-title">
                                <div>
                                    <h2 id="welcome-banner-title">"New to harness2?"</h2>
                                    <p>
                                        "The guided tour explains what harness2 does, checks this server, installs what is missing, runs the free demo and walks you through a first small run with a cost estimate."
                                    </p>
                                </div>
                                <div class="banner-actions">
                                    <a class="btn btn-primary" href="/welcome">
                                        "Start the guided tour"
                                    </a>
                                    <button
                                        type="button"
                                        class="btn btn-ghost"
                                        on:click=move |_| {
                                            browser::storage_set(WELCOME_DISMISSED, "1");
                                            dismissed.set(true);
                                        }
                                    >
                                        "Dismiss"
                                    </button>
                                </div>
                            </section>
                        }
                    })
            }}

            <div class="stats">
                <Stat label="Experiments" value=Signal::derive(move || stats().0.to_string()) />
                <Stat label="Sweeps" value=Signal::derive(move || stats().1.to_string()) />
                <Stat
                    label="Jobs running / allowed"
                    value=Signal::derive(move || stats().3)
                />
                <Stat
                    label="Recorded spend"
                    value=Signal::derive(move || format::usd(stats().2))
                    hint="Improver and selection sessions only"
                />
            </div>

            <div class="grid-2">
                <Card title="harness2">
                    {move || match s.system.get() {
                        Some(info) => view! { <Harness2Status info=info /> }.into_any(),
                        None => view! { <Skeleton lines=3 /> }.into_any(),
                    }}
                </Card>
                <Card title="Environment checks">
                    {loaded(doctor, 3, |r| view! { <DoctorSummary report=r /> })}
                    {move || {
                        s.can(Role::Operator)
                            .then(|| {
                                view! {
                                    <button
                                        type="button"
                                        class="btn btn-ghost btn-sm"
                                        on:click=move |_| {
                                            if refresh_doctor.get_untracked() {
                                                doctor.refetch();
                                            } else {
                                                refresh_doctor.set(true);
                                            }
                                        }
                                    >
                                        {icon(Icon::Refresh)}
                                        " Re-run checks"
                                    </button>
                                }
                            })
                    }}
                </Card>
            </div>

            <div class="grid-2">
                <Card title="Running and queued jobs">
                    {loaded(
                        active,
                        2,
                        move |jobs| {
                            if jobs.is_empty() {
                                let hint = if s.can(Role::Operator) {
                                    "Nothing is running. Start the demo or a new run to see live progress here."
                                } else {
                                    "Nothing is running right now."
                                };
                                view! { <p class="muted">{hint}</p> }.into_any()
                            } else {
                                view! {
                                    <ul class="job-mini-list">
                                        {jobs.into_iter().map(|j| view! { <JobMini job=j /> }).collect_view()}
                                    </ul>
                                }
                                    .into_any()
                            }
                        },
                    )}
                    <h3 class="subhead">"Recent"</h3>
                    {loaded(
                        recent,
                        3,
                        |finished| {
                            if finished.is_empty() {
                                view! { <p class="muted">"No finished jobs yet."</p> }.into_any()
                            } else {
                                view! {
                                    <ul class="job-mini-list">
                                        {finished
                                            .into_iter()
                                            .map(|j| view! { <JobMini job=j /> })
                                            .collect_view()}
                                    </ul>
                                }
                                    .into_any()
                            }
                        },
                    )}
                    <a class="link-more" href="/jobs">"All jobs →"</a>
                </Card>
                <Card title="Recent experiments">
                    {loaded(
                        experiments,
                        4,
                        |mut exps| {
                            if exps.is_empty() {
                                return view! {
                                    <EmptyState
                                        title="No experiments yet"
                                        body="An experiment is a folder of harness2 results tied to one model profile. Run the demo for a free example, or create one for a benchmark run."
                                    >
                                        <a class="btn btn-primary" href="/experiments">
                                            "Create an experiment"
                                        </a>
                                    </EmptyState>
                                }
                                    .into_any();
                            }
                            exps.sort_by_key(|e| std::cmp::Reverse(latest(e)));
                            view! {
                                <ul class="exp-mini-list">
                                    {exps
                                        .into_iter()
                                        .take(5)
                                        .map(|e| {
                                            let best = e.sweeps.iter().find(|s| s.headline.is_some()).cloned();
                                            view! {
                                                <li class="exp-mini">
                                                    <div class="exp-mini-head">
                                                        <a href=route::experiment(&e.name)>
                                                            <strong>{e.name.clone()}</strong>
                                                        </a>
                                                        {kind_badge(e.kind)}
                                                        <span class="muted small">
                                                            {format::plural(e.sweeps.len() as u64, "sweep", "sweeps")}
                                                        </span>
                                                    </div>
                                                    {match best {
                                                        Some(sw) => {
                                                            view! {
                                                                <div class="exp-mini-scores">
                                                                    <a class="small" href=route::sweep(&e.name, &sw.id)>
                                                                        {sw.id.clone()}
                                                                    </a>
                                                                    <HeadlineScores scores=sw.headline.clone() base=sw.base_score />
                                                                </div>
                                                            }
                                                                .into_any()
                                                        }
                                                        None => {
                                                            view! {
                                                                <p class="muted small">
                                                                    {if e.description.is_empty() {
                                                                        "No judged sweeps yet.".to_owned()
                                                                    } else {
                                                                        e.description.clone()
                                                                    }}
                                                                </p>
                                                            }
                                                                .into_any()
                                                        }
                                                    }}
                                                </li>
                                            }
                                        })
                                        .collect_view()}
                                </ul>
                                <a class="link-more" href="/experiments">"All experiments →"</a>
                            }
                                .into_any()
                        },
                    )}
                </Card>
            </div>

            <Card title="How harness2 improves an agent">
                <RecursionExplainer />
                <p class="small">
                    <a href="/help">"Read the glossary"</a>
                    " · "
                    <a href="/welcome">"Take the guided tour"</a>
                </p>
            </Card>
        </div>
    }
}

/// Sort key: the newest sweep update, else the creation time.
fn latest(e: &Experiment) -> String {
    e.sweeps
        .iter()
        .filter_map(|s| s.updated_at.clone())
        .max()
        .unwrap_or_else(|| e.created_at.clone())
}
