//! Onboarding: what harness2 is, then a working setup step by step.

use h2w_api::{
    BenchmarkStatus, CostEstimate, DoctorReport, EstimateRequest, Experiment, Mode, ModelProfile,
    Role,
};
use leptos::prelude::*;
use leptos_router::hooks::use_navigate;

use crate::actions;
use crate::api;
use crate::app::load_system;
use crate::components::icons::{Icon, icon};
use crate::components::ui::{Card, PageHeader, Skeleton, Term, loaded};
use crate::components::widgets::{DoctorSummary, Harness2Status, RecursionExplainer};
use crate::logic::format;
use crate::logic::harness::{self, COMPONENTS};
use crate::logic::urls::{api as paths, route};
use crate::state::{session, toasts};

#[component]
fn Step(
    n: u32,
    #[prop(into)] title: String,
    #[prop(into)] done: Signal<bool>,
    children: Children,
) -> impl IntoView {
    view! {
        <li class="onboard-step" class:done=move || done.get()>
            <span class="onboard-num" aria-hidden="true">{move || if done.get() { "✓".to_owned() } else { n.to_string() }}</span>
            <div>
                <h2>
                    {title}
                    {move || done.get().then(|| view! { <span class="visually-hidden">" (done)"</span> })}
                </h2>
                {children()}
            </div>
        </li>
    }
}

#[component]
pub(crate) fn Welcome() -> impl IntoView {
    let s = session();
    let t = toasts();
    let navigate = use_navigate();
    load_system(s);
    let refresh = RwSignal::new(false);
    let doctor = LocalResource::new(move || {
        let r = refresh.get();
        async move { api::get::<DoctorReport>(&paths::doctor(r)).await }
    });
    let experiments = LocalResource::new(|| async {
        api::get::<Vec<Experiment>>(&paths::experiments(false)).await
    });
    let profiles =
        LocalResource::new(|| async { api::get::<Vec<ModelProfile>>(&paths::profiles()).await });
    let benchmarks = LocalResource::new(|| async {
        api::get::<Vec<BenchmarkStatus>>(&paths::benchmarks()).await
    });
    let estimate = LocalResource::new(move || {
        // Prefer a profile with prices, so the estimate can show dollars.
        let profile = profiles.get().and_then(Result::ok).and_then(|l| {
            l.iter()
                .find(|p| p.prices.is_some())
                .or_else(|| l.first())
                .map(|p| p.name.clone())
        });
        async move {
            api::post::<_, CostEstimate>(
                &paths::estimate(),
                &EstimateRequest {
                    tasks: 1,
                    k: 1,
                    modes: vec![Mode::Parallel],
                    profile,
                    experiment: None,
                },
            )
            .await
        }
    });
    let doctor_ok = Signal::derive(move || doctor.get().and_then(Result::ok).is_some_and(|d| d.ok));
    let installed = Signal::derive(move || {
        s.system
            .with(|i| i.as_ref().is_some_and(|i| i.harness2.installed))
    });
    let demo_done = Signal::derive(move || {
        experiments
            .get()
            .and_then(Result::ok)
            .is_some_and(|l| l.iter().any(|e| e.kind == h2w_api::ExperimentKind::Demo))
    });
    let has_profile = Signal::derive(move || {
        profiles
            .get()
            .and_then(Result::ok)
            .is_some_and(|l| !l.is_empty())
    });
    let has_bench = Signal::derive(move || {
        benchmarks
            .get()
            .and_then(Result::ok)
            .is_some_and(|l| l.iter().any(|b| b.installed))
    });
    let has_run = Signal::derive(move || {
        experiments.get().and_then(Result::ok).is_some_and(|l| {
            l.iter()
                .any(|e| e.kind == h2w_api::ExperimentKind::Standard && !e.sweeps.is_empty())
        })
    });
    let demo_busy = RwSignal::new(false);
    let run_demo =
        move |_| actions::start_job(actions::demo_spec(), t, Some(demo_busy), navigate.clone());
    view! {
        <div class="page welcome-page">
            <PageHeader
                title="Welcome to Harness² Workbench"
                subtitle="A guided tour: what harness2 does, then a working setup in seven steps. Each step turns green when it is done."
            />
            <Card title="What harness2 does">
                <p class="reading">
                    "An AI agent is a model plus a "<Term id="harness">"harness"</Term>
                    ": the instructions, rules, memory, tool advice, helpers and code around it. harness2 improves the harness, never the model, by trying edits on real benchmark tasks, keeping what works, and remembering the lessons for the next task."
                </p>
                <h3>"The eight parts of a harness"</h3>
                <div class="component-cards">
                    {COMPONENTS
                        .iter()
                        .map(|c| {
                            view! {
                                <div class="component-card">
                                    <strong>
                                        <span class="component-code">{harness::code(*c)}</span>
                                        " "
                                        {harness::name(*c)}
                                    </strong>
                                    <p>{harness::blurb(*c)}</p>
                                </div>
                            }
                        })
                        .collect_view()}
                </div>
                <h3>"Two levels of recursion"</h3>
                <RecursionExplainer />
                <p class="small">
                    "How much search each task gets is the budget "<Term id="k">"k"</Term>
                    ": per task and mode, 4k executor rollouts and 3k improver sessions. "
                    <Term id="parallel">"Parallel mode"</Term>" explores in width, "
                    <Term id="sequential">"sequential mode"</Term>" in depth. "
                    <a href="/help">"The glossary"</a>" explains every term."
                </p>
            </Card>
            <ol class="onboard-steps">
                <Step n=1 title="Check this server" done=doctor_ok>
                    <p>"The environment doctor looks for every tool harness2 and the benchmarks need, and says how to fix what is missing."</p>
                    {loaded(doctor, 3, |r| view! { <DoctorSummary report=r max_items=6 /> })}
                    {move || {
                        s.can(Role::Operator)
                            .then(|| {
                                view! {
                                    <button
                                        type="button"
                                        class="btn btn-secondary btn-sm"
                                        on:click=move |_| {
                                            if refresh.get_untracked() {
                                                doctor.refetch();
                                            } else {
                                                refresh.set(true);
                                            }
                                        }
                                    >
                                        {icon(Icon::Refresh)}
                                        " Check again"
                                    </button>
                                }
                            })
                    }}
                </Step>
                <Step n=2 title="Install harness2" done=installed>
                    <p>"The Workbench fetches harness2 at the commit this release was tested with, creates its Python 3.12 environment and installs it."</p>
                    {move || match s.system.get() {
                        Some(info) => view! { <Harness2Status info=info /> }.into_any(),
                        None => view! { <Skeleton lines=2 /> }.into_any(),
                    }}
                </Step>
                <Step n=3 title="Run the free demo" done=demo_done>
                    <p>
                        "harness2-demo runs the real recursion and library workflow with deterministic local editors instead of models: no credentials, no network, about a second. It writes a small "
                        <em>"demo"</em>" experiment you can explore like a real one."
                    </p>
                    <div class="pill-row">
                        {move || {
                            s.can(Role::Operator)
                                .then(|| {
                                    view! {
                                        <button type="button" class="btn btn-primary" disabled=move || demo_busy.get() on:click=run_demo.clone()>
                                            {icon(Icon::Play)}
                                            " Run the demo"
                                        </button>
                                    }
                                })
                        }}
                        {move || demo_done.get().then(|| view! { <a class="btn btn-secondary" href=route::experiment(actions::DEMO_EXPERIMENT)>"Explore the demo results"</a> })}
                    </div>
                </Step>
                <Step n=4 title="Add a model profile" done=has_profile>
                    <p>
                        "A "<Term id="profile">"profile"</Term>
                        " says which models execute, improve and judge, and how to reach them: Google Vertex AI (upstream's setup), any OpenAI-compatible endpoint (LiteLLM, vLLM, SGLang, Ollama or a hosted gateway), or custom environment variables. Keys are stored as write-only secrets."
                    </p>
                    {move || {
                        profiles
                            .get()
                            .and_then(Result::ok)
                            .filter(|l| !l.is_empty())
                            .map(|l| view! { <p class="small">"Profiles: "{l.iter().map(|p| p.name.clone()).collect::<Vec<_>>().join(", ")}</p> })
                    }}
                    <a class="btn btn-secondary" href="/settings/profiles">
                        {move || if s.can(Role::Admin) { "Add or edit profiles" } else { "See the profiles" }}
                    </a>
                </Step>
                <Step n=5 title="Set up a benchmark" done=has_bench>
                    <p>
                        "JobBench (professional work), LAB (legal work) and WorkBuddy-Bench (office, web, code) install from pinned upstream code. Plan the setup first: a dry run lists every command without changing anything."
                    </p>
                    {move || {
                        benchmarks
                            .get()
                            .and_then(Result::ok)
                            .map(|l| {
                                let names: Vec<String> = l.iter().filter(|b| b.installed).map(|b| b.display_name.clone()).collect();
                                if names.is_empty() {
                                    view! { <p class="small muted">"Nothing is set up yet."</p> }.into_any()
                                } else {
                                    view! { <p class="small">"Ready: "{names.join(", ")}</p> }.into_any()
                                }
                            })
                    }}
                    <a class="btn btn-secondary" href="/benchmarks">"Open benchmarks"</a>
                </Step>
                <Step n=6 title="Your first small run" done=has_run>
                    <p>
                        "Start with the smoke-test preset: one domain, one task, k = 1, parallel mode only. The wizard previews the exact task, shows the estimate, and asks before anything costs money."
                    </p>
                    {move || match estimate.get() {
                        Some(Ok(e)) => view! {
                            <p class="small">
                                {format!(
                                    "A smoke test is about {} executor rollouts and {} improver sessions{}.",
                                    e.rollouts,
                                    e.improver_sessions,
                                    format::usd_range(e.usd_low, e.usd_high).map(|u| format!(", roughly {u} at your profile's prices")).unwrap_or_default()
                                )}
                            </p>
                        }
                        .into_any(),
                        _ => ().into_any(),
                    }}
                    <a class="btn btn-primary" href="/new?preset=smoke">"Start a smoke test"</a>
                </Step>
                <Step n=7 title="Read the results" done=Signal::derive(|| false)>
                    <p>
                        "Each sweep has a task list, a score report (with the noise-floor and fallback notes), the experience library and the improver audit trail. A task's page shows the whole recursion: probes, compositions, the blind selection and the lesson, plus every harness as files, diffs between any two, exports and rollouts. "
                        <a href="/compare">"Compare"</a>" puts two results side by side with a sign test."
                    </p>
                    <a class="btn btn-secondary" href="/experiments">"Browse experiments"</a>
                </Step>
            </ol>
        </div>
    }
}
