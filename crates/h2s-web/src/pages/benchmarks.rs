//! Benchmarks: what is installed, what each needs, licences, setup (plan first), readiness
//! checks and train/test splits.

use h2s_api::{
    Bench, BenchmarkStatus, Cell, CheckParams, JobResult, JobSpec, ModelProfile, Role, SetupParams,
    SplitInfo, SplitParams, Substrate,
};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_navigate;

use crate::actions;
use crate::api;
use crate::components::form::{RadioCards, SelectField, TextField};
use crate::components::icons::{Icon, icon};
use crate::components::modal::Modal;
use crate::components::ui::{Ago, Badge, Card, EmptyState, Notice, PageHeader, Term, loaded};
use crate::logic::format;
use crate::logic::jobs::Tone;
use crate::logic::names;
use crate::logic::urls::{api as paths, route};
use crate::state::{ConfirmRequest, session, toasts, ui};

fn key(b: &BenchmarkStatus) -> String {
    match b.subset {
        Some(s) => format!("{}-{}", b.bench.as_cli(), s.as_cli()),
        None => b.bench.as_cli().to_owned(),
    }
}

/// Setup dialog: executor and profile, then a dry-run plan or the real setup.
#[component]
fn SetupDialog(open: RwSignal<bool>, target: RwSignal<Option<BenchmarkStatus>>) -> impl IntoView {
    let t = toasts();
    let u = ui();
    let navigate = use_navigate();
    let substrate = RwSignal::new("oc".to_owned());
    let profile = RwSignal::new(String::new());
    let profiles = LocalResource::new(move || {
        let o = open.get();
        async move {
            if o {
                api::get::<Vec<ModelProfile>>(&paths::profiles()).await
            } else {
                Ok(Vec::new())
            }
        }
    });
    let options = Signal::derive(move || {
        let mut v = vec![(
            String::new(),
            "None (no model settings baked in)".to_owned(),
        )];
        if let Some(Ok(list)) = profiles.get() {
            v.extend(list.into_iter().map(|p| (p.name.clone(), p.name)));
        }
        v
    });
    let spec = move |dry_run: bool| -> Option<JobSpec> {
        let b = target.get_untracked()?;
        Some(JobSpec::Setup(SetupParams {
            bench: b.bench,
            subset: b.subset,
            substrate: if substrate.get_untracked() == "cx" {
                Substrate::Cx
            } else {
                Substrate::Oc
            },
            dry_run,
            profile: Some(profile.get_untracked()).filter(|p| !p.is_empty()),
        }))
    };
    let close = Callback::new(move |()| open.set(false));
    let title = Signal::derive(move || {
        target
            .get()
            .map(|b| format!("Set up {}", b.display_name))
            .unwrap_or_default()
    });
    let plan = {
        let navigate = navigate.clone();
        move |_| {
            if let Some(s) = spec(true) {
                open.set(false);
                actions::start_job(s, t, None, navigate.clone());
            }
        }
    };
    let run = move |_| {
        let Some(s) = spec(false) else { return };
        let navigate = navigate.clone();
        open.set(false);
        u.ask(ConfirmRequest {
            title: "Run setup now?".to_owned(),
            body: "Setup clones the pinned upstream code, applies patches, builds environments and downloads task data. It needs network access and can take a long time.".to_owned(),
            confirm_label: "Run setup".to_owned(),
            danger: false,
            on_confirm: Callback::new(move |()| actions::start_job(s.clone(), t, None, navigate.clone())),
        });
    };
    view! {
        <Modal open=open on_close=close title=title>
            <p class="muted">"Plan first: a dry run prints every command setup would run, without changing anything."</p>
            {move || {
                target
                    .get()
                    .filter(|b| b.bench == Bench::Wb)
                    .map(|b| {
                        view! {
                            <Notice tone=Tone::Warning title="Licence with a territorial limitation">
                                <p>{b.license_note}</p>
                            </Notice>
                        }
                    })
            }}
            <RadioCards
                name="setup-substrate"
                legend="Executor to prepare"
                value=substrate
                compact=true
                options=vec![
                    ("oc".to_owned(), "OpenCode".to_owned(), "Upstream default.".to_owned()),
                    ("cx".to_owned(), "Codex".to_owned(), "Codex CLI configuration.".to_owned()),
                ]
            />
            <SelectField
                id="setup-profile"
                label="Model profile (optional)"
                value=profile
                options=options
                help="Some benchmarks bake model routes into their environments (WorkBuddy, Codex)."
            />
            <div class="modal-actions">
                <button type="button" class="btn btn-secondary" on:click=plan.clone()>
                    "Plan (dry run)"
                </button>
                <button type="button" class="btn btn-primary" on:click=run.clone()>
                    "Set up now"
                </button>
            </div>
        </Modal>
    }
}

#[component]
fn SplitDialog(open: RwSignal<bool>, benches: Vec<BenchmarkStatus>) -> impl IntoView {
    let t = toasts();
    let navigate = use_navigate();
    let target = RwSignal::new(benches.first().map(key).unwrap_or_default());
    let name = RwSignal::new(String::new());
    let seed = RwSignal::new("42".to_owned());
    let fraction = RwSignal::new("0.5".to_owned());
    let options = Signal::stored(
        benches
            .iter()
            .map(|b| (key(b), b.display_name.clone()))
            .collect::<Vec<_>>(),
    );
    let name_error = Signal::derive(move || {
        let n = name.get();
        (!n.is_empty())
            .then(|| names::experiment_name(&n).err())
            .flatten()
    });
    let fraction_error = Signal::derive(move || match fraction.get().trim().parse::<f64>() {
        Ok(f) if f > 0.0 && f < 1.0 => None,
        _ => Some("between 0 and 1, e.g. 0.5".to_owned()),
    });
    let seed_error = Signal::derive(move || {
        seed.get()
            .trim()
            .parse::<u64>()
            .err()
            .map(|_| "a whole number".to_owned())
    });
    let close = Callback::new(move |()| open.set(false));
    let create = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let Some(b) = benches.iter().find(|b| key(b) == target.get_untracked()) else {
            return;
        };
        let (Ok(seed), Ok(fr)) = (
            seed.get_untracked().trim().parse::<u64>(),
            fraction.get_untracked().trim().parse::<f64>(),
        ) else {
            return;
        };
        let n = name.get_untracked();
        if names::experiment_name(&n).is_err() || !(fr > 0.0 && fr < 1.0) {
            return;
        }
        open.set(false);
        actions::start_job(
            JobSpec::Split(SplitParams {
                bench: b.bench,
                subset: b.subset,
                seed,
                train_fraction: fr,
                name: n,
            }),
            t,
            None,
            navigate.clone(),
        );
    };
    view! {
        <Modal open=open on_close=close title="New train/test split">
            <form on:submit=create.clone() novalidate=true>
                <p class="muted">
                    "A reproducible, domain-stratified partition: run the recursion on the train part and evaluate on the test part."
                </p>
                <SelectField id="split-bench" label="Benchmark" value=target options=options />
                <TextField id="split-name" label="Name" value=name required=true error=name_error placeholder="lab-half" help="Same rules as experiment names; must be new." />
                <div class="form-grid">
                    <TextField id="split-seed" label="Seed" value=seed kind="number" error=seed_error />
                    <TextField id="split-fraction" label="Train fraction" value=fraction inputmode="decimal" error=fraction_error help="Share of each domain's tasks that go to train." />
                </div>
                <div class="modal-actions">
                    <button type="button" class="btn btn-secondary" on:click=move |_| close.run(())>"Cancel"</button>
                    <button type="submit" class="btn btn-primary">"Create split"</button>
                </div>
            </form>
        </Modal>
    }
}

#[component]
fn BenchCard(b: BenchmarkStatus, on_setup: Callback<BenchmarkStatus>) -> impl IntoView {
    let s = session();
    let check_busy = RwSignal::new(false);
    let check_result =
        RwSignal::new(None::<Result<(h2s_api::Job, Option<h2s_api::CheckResult>), api::ApiError>>);
    let cell = Cell {
        bench: b.bench,
        subset: b.subset,
        substrate: Substrate::Oc,
    };
    let check = move |_| {
        check_busy.set(true);
        spawn_local(async move {
            let r = actions::run_and_wait(
                &JobSpec::Check(CheckParams {
                    cell,
                    profile: None,
                }),
                120,
            )
            .await;
            check_busy.set(false);
            check_result.set(Some(r.map(|j| {
                let res = match &j.result {
                    Some(JobResult::Check(c)) => Some(c.clone()),
                    _ => None,
                };
                (j, res)
            })));
        });
    };
    let wb = b.bench == Bench::Wb;
    let b2 = b.clone();
    view! {
        <Card class="bench-card" title=b.display_name.clone()>
            <div class="pill-row">
                {if b.installed {
                    view! { <Badge tone=Tone::Success text="installed" /> }.into_any()
                } else {
                    view! { <Badge tone=Tone::Warning text="not set up" /> }.into_any()
                }}
                {b.at_pinned_commit.map(|p| {
                    if p {
                        view! { <Badge tone=Tone::Neutral text="at pinned commit" /> }.into_any()
                    } else {
                        view! { <Badge tone=Tone::Warning text="not at the pinned commit" /> }.into_any()
                    }
                })}
                {if b.data_present {
                    view! { <Badge tone=Tone::Neutral text="task data present" /> }.into_any()
                } else {
                    view! { <Badge tone=Tone::Neutral text="no task data" /> }.into_any()
                }}
            </div>
            <p>{b.description.clone()}</p>
            {if wb {
                view! {
                    <Notice tone=Tone::Warning title="Licence: territorial limitation">
                        <p>{b.license_note.clone()}</p>
                    </Notice>
                }
                .into_any()
            } else {
                view! { <p class="small muted"><strong>"Licence: "</strong>{b.license_note.clone()}</p> }.into_any()
            }}
            <p class="small"><strong>"Needs: "</strong>{b.requirements.join(", ")}</p>
            {(!b.domains.is_empty())
                .then(|| {
                    let total: u32 = b.domains.iter().map(|d| d.tasks).sum();
                    view! {
                        <details class="disclosure">
                            <summary>{format!("{} {}s, {} tasks", b.domains.len(), b.bench.domain_word(), total)}</summary>
                            <ul class="small">
                                {b.domains.iter().map(|d| view! { <li>{format!("{} ({})", d.domain, d.tasks)}</li> }).collect_view()}
                            </ul>
                        </details>
                    }
                })}
            <p class="small">
                <a href=b.upstream_url.clone() rel="noopener noreferrer external" target="_blank">"Upstream project"</a>
            </p>
            {move || {
                check_result
                    .get()
                    .map(|r| match r {
                        Ok((job, Some(c))) => view! {
                            <Notice tone=if c.ready { Tone::Success } else { Tone::Danger } title=if c.ready { "Ready" } else { "Not ready" }>
                                {c.tasks.map(|n| view! { <p>{format!("{} tasks installed.", format::int(u64::from(n)))}</p> })}
                                {(!c.failures.is_empty()).then(|| view! { <ul>{c.failures.iter().map(|f| view! { <li>{f.clone()}</li> }).collect_view()}</ul> })}
                                <p class="small"><a href=route::job(&job.id)>"Open the check's log"</a></p>
                            </Notice>
                        }
                        .into_any(),
                        Ok((job, None)) => view! {
                            <Notice tone=Tone::Warning title="The check did not finish">
                                <p><a href=route::job(&job.id)>"Open the job"</a></p>
                            </Notice>
                        }
                        .into_any(),
                        Err(e) => view! { <Notice tone=Tone::Danger title="Check failed"><p>{e.to_string()}</p></Notice> }.into_any(),
                    })
            }}
            <div class="bench-actions">
                {move || {
                    s.can(Role::Operator)
                        .then(|| {
                            view! {
                                <button type="button" class="btn btn-secondary btn-sm" disabled=move || check_busy.get() on:click=check>
                                    {move || if check_busy.get() { "Checking…" } else { "Check readiness" }}
                                </button>
                            }
                        })
                }}
                {move || {
                    let b3 = b2.clone();
                    s.can(Role::Admin)
                        .then(|| {
                            view! {
                                <button type="button" class="btn btn-primary btn-sm" on:click=move |_| on_setup.run(b3.clone())>
                                    {icon(Icon::Download)}
                                    {if b3.installed { " Set up again…" } else { " Set up…" }}
                                </button>
                            }
                        })
                }}
            </div>
        </Card>
    }
}

#[component]
pub(crate) fn Benchmarks() -> impl IntoView {
    let s = session();
    let benches = LocalResource::new(|| async {
        api::get::<Vec<BenchmarkStatus>>(&paths::benchmarks()).await
    });
    let splits =
        LocalResource::new(|| async { api::get::<Vec<SplitInfo>>(&paths::splits()).await });
    let setup_open = RwSignal::new(false);
    let setup_target = RwSignal::new(None::<BenchmarkStatus>);
    let split_open = RwSignal::new(false);
    let on_setup = Callback::new(move |b: BenchmarkStatus| {
        setup_target.set(Some(b));
        setup_open.set(true);
    });
    view! {
        <div class="page">
            <PageHeader
                title="Benchmarks"
                subtitle="What harness2 can run on this server. Setup clones each benchmark's pinned code and data; checks confirm a benchmark is ready without calling any model."
            />
            {move || {
                (!s.can(Role::Admin))
                    .then(|| {
                        view! {
                            <Notice tone=Tone::Info>
                                <p>"Setting benchmarks up needs the admin role; operators can run readiness checks and create splits."</p>
                            </Notice>
                        }
                    })
            }}
            {loaded(
                benches,
                6,
                move |list| {
                    view! {
                        <div class="bench-grid">
                            {list.into_iter().map(|b| view! { <BenchCard b=b on_setup=on_setup /> }).collect_view()}
                        </div>
                    }
                },
            )}
            <Card title="Train/test splits">
                <p class="muted small">
                    "A " <Term id="split">"split"</Term>
                    " lets you improve harnesses on one part of a benchmark and evaluate on tasks the recursion never saw."
                </p>
                {loaded(
                    splits,
                    3,
                    |list| {
                        if list.is_empty() {
                            view! { <EmptyState title="No splits yet" body="Create one to run train and test parts separately." /> }.into_any()
                        } else {
                            view! {
                                <div class="table-wrap">
                                    <table class="table">
                                        <thead>
                                            <tr>
                                                <th scope="col">"Name"</th>
                                                <th scope="col">"Benchmark"</th>
                                                <th scope="col" class="num">"Seed"</th>
                                                <th scope="col" class="num">"Train"</th>
                                                <th scope="col" class="num">"Test"</th>
                                                <th scope="col">"Created"</th>
                                            </tr>
                                        </thead>
                                        <tbody>
                                            {list
                                                .into_iter()
                                                .map(|sp| {
                                                    view! {
                                                        <tr>
                                                            <td class="mono">{sp.name.clone()}</td>
                                                            <td>{sp.bench.display_name()}{sp.subset.map(|s| format!(" {}", s.as_cli()))}</td>
                                                            <td class="num">{sp.seed}</td>
                                                            <td class="num">{format!("{} ({:.0}%)", sp.train, sp.train_fraction * 100.0)}</td>
                                                            <td class="num">{sp.test}</td>
                                                            <td class="nowrap"><Ago ts=sp.created_at /></td>
                                                        </tr>
                                                    }
                                                })
                                                .collect_view()}
                                        </tbody>
                                    </table>
                                </div>
                            }
                            .into_any()
                        }
                    },
                )}
                {move || {
                    s.can(Role::Operator)
                        .then(|| view! { <button type="button" class="btn btn-secondary btn-sm" on:click=move |_| split_open.set(true)>"New split…"</button> })
                }}
            </Card>
            <SetupDialog open=setup_open target=setup_target />
            {move || match benches.get() {
                Some(Ok(list)) => view! { <SplitDialog open=split_open benches=list /> }.into_any(),
                _ => ().into_any(),
            }}
        </div>
    }
}
