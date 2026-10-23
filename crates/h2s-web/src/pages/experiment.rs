//! One experiment: its sweeps, its jobs, its manifest.

use h2s_api::{Experiment, Job, Page, Role, UpdateExperimentRequest};
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api;
use crate::components::form::{TextArea, TextField};
use crate::components::icons::{Icon, icon};
use crate::components::modal::Modal;
use crate::components::ui::{Ago, Badge, Card, Crumb, EmptyState, PageHeader, Term, loaded};
use crate::components::widgets::{HeadlineScores, JobMini, kind_badge, mode_label};
use crate::logic::format;
use crate::logic::jobs::Tone;
use crate::logic::names;
use crate::logic::urls::{api as paths, route};
use crate::nav::param;
use crate::state::{ConfirmRequest, session, toasts, ui};

#[component]
fn EditExperiment(
    open: RwSignal<bool>,
    exp: Experiment,
    on_saved: Callback<Experiment>,
) -> impl IntoView {
    let t = toasts();
    let description = RwSignal::new(exp.description.clone());
    let tags = RwSignal::new(exp.tags.join(", "));
    let tags_error = Signal::derive(move || names::parse_tags(&tags.get()).err());
    let busy = RwSignal::new(false);
    let name = exp.name;
    let close = Callback::new(move |()| open.set(false));
    let save = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let Ok(tag_list) = names::parse_tags(&tags.get_untracked()) else {
            return;
        };
        let req = UpdateExperimentRequest {
            description: Some(description.get_untracked()),
            tags: Some(tag_list),
            archived: None,
        };
        let url = paths::experiment(&name);
        busy.set(true);
        spawn_local(async move {
            let r = api::patch::<_, Experiment>(&url, &req).await;
            busy.set(false);
            match r {
                Ok(e) => {
                    open.set(false);
                    t.success("Saved");
                    on_saved.run(e);
                }
                Err(e) => t.error("Could not save", &e),
            }
        });
    };
    view! {
        <Modal open=open on_close=close title="Edit experiment">
            <form on:submit=save.clone() novalidate=true>
                <TextArea id="edit-desc" label="Description" value=description rows=3 />
                <TextField id="edit-tags" label="Tags" value=tags error=tags_error help="Separated by commas or spaces." />
                <p class="muted small">"The name and the pinned profile cannot change."</p>
                <div class="modal-actions">
                    <button type="button" class="btn btn-secondary" on:click=move |_| close.run(())>"Cancel"</button>
                    <button type="submit" class="btn btn-primary" disabled=move || busy.get()>"Save"</button>
                </div>
            </form>
        </Modal>
    }
}

#[component]
pub(crate) fn ExperimentPage() -> impl IntoView {
    let s = session();
    let t = toasts();
    let u = ui();
    let exp = param("exp");
    let data = LocalResource::new(move || {
        let e = exp.get();
        async move { api::get::<Experiment>(&paths::experiment(&e)).await }
    });
    let jobs = LocalResource::new(move || {
        let e = exp.get();
        async move { api::get::<Page<Job>>(&paths::jobs("", "", &e, 8, None)).await }
    });
    let edit_open = RwSignal::new(false);
    let saved = Callback::new(move |_e: Experiment| data.refetch());
    let toggle_archive = move |archived: bool| {
        let name = exp.get_untracked();
        let verb = if archived { "Archive" } else { "Restore" };
        u.ask(ConfirmRequest {
            title: format!("{verb} {name}?"),
            body: if archived {
                "Archived experiments are hidden from lists and pickers. Nothing is deleted; you can restore it any time.".to_owned()
            } else {
                "The experiment will show up in lists and pickers again.".to_owned()
            },
            confirm_label: verb.to_owned(),
            danger: false,
            on_confirm: Callback::new(move |()| {
                let url = paths::experiment(&name);
                let req = UpdateExperimentRequest {
                    archived: Some(archived),
                    ..UpdateExperimentRequest::default()
                };
                spawn_local(async move {
                    match api::patch::<_, Experiment>(&url, &req).await {
                        Ok(_) => {
                            t.success(if archived { "Archived" } else { "Restored" });
                            data.refetch();
                        }
                        Err(e) => t.error("Could not update the experiment", &e),
                    }
                });
            }),
        });
    };
    let title = Signal::derive(move || exp.get());
    view! {
        <div class="page">
            <PageHeader
                title=title
                subtitle=Signal::derive(move || {
                    data.get().and_then(Result::ok).map(|e| e.description).unwrap_or_default()
                })
                crumbs=vec![Crumb("Experiments".into(), "/experiments".into())]
            >
                {move || {
                    let e = exp.get();
                    let can = s.can(Role::Operator);
                    let archived = data.get().and_then(Result::ok).is_some_and(|x| x.archived);
                    view! {
                        {can
                            .then(|| {
                                view! {
                                    <a class="btn btn-primary" href=route::new_run_for(&e)>
                                        {icon(Icon::Plus)}
                                        " New run here"
                                    </a>
                                }
                            })}
                        <a
                            class="btn btn-secondary"
                            href=paths::manifest(&e)
                            download=format!("{e}-manifest.json")
                            rel="external"
                            data-testid="manifest-download"
                        >
                            {icon(Icon::Download)}
                            " Manifest (JSON)"
                        </a>
                        {can
                            .then(|| {
                                view! {
                                    <button type="button" class="btn btn-secondary" on:click=move |_| edit_open.set(true)>
                                        "Edit"
                                    </button>
                                    <button type="button" class="btn btn-ghost" on:click=move |_| toggle_archive(!archived)>
                                        {if archived { "Restore" } else { "Archive" }}
                                    </button>
                                }
                            })}
                    }
                }}
            </PageHeader>
            {loaded(
                data,
                5,
                move |e| {
                    let name = e.name.clone();
                    let total_tasks: u32 = e.sweeps.iter().map(|s| s.tasks).sum();
                    let spend: f64 = e.sweeps.iter().map(|s| s.usd).sum();
                    let edit = e.clone();
                    view! {
                        <div class="stats">
                            <div class="stat">
                                <span class="stat-label">"Kind"</span>
                                <span class="stat-value small">{kind_badge(e.kind)}</span>
                                <span class="stat-hint">
                                    {match e.kind {
                                        h2s_api::ExperimentKind::Demo => "written by harness2-demo",
                                        h2s_api::ExperimentKind::Standard => "benchmark runs",
                                    }}
                                </span>
                            </div>
                            <div class="stat">
                                <span class="stat-label">"Model profile"</span>
                                <span class="stat-value small">
                                    {e.profile.clone().unwrap_or_else(|| "not pinned".to_owned())}
                                </span>
                                <span class="stat-hint">"Every run here must use it"</span>
                            </div>
                            <div class="stat">
                                <span class="stat-label">"Tasks across sweeps"</span>
                                <span class="stat-value">{format::int(u64::from(total_tasks))}</span>
                                <span class="stat-hint">{format::plural(e.sweeps.len() as u64, "sweep", "sweeps")}</span>
                            </div>
                            <div class="stat">
                                <span class="stat-label">"Recorded spend"</span>
                                <span class="stat-value">{format::usd(spend)}</span>
                                <span class="stat-hint">
                                    {e.size_bytes.map(|b| format!("{} on disk", format::bytes(b))).unwrap_or_default()}
                                </span>
                            </div>
                        </div>
                        <p class="muted small">
                            "Created by "{e.created_by.clone()}" · "<Ago ts=e.created_at.clone() />
                            {(!e.tags.is_empty()).then(|| format!(" · tags: {}", e.tags.join(", ")))}
                        </p>
                        <Card title="Sweeps">
                            <p class="muted small">
                                "A " <Term id="sweep">"sweep"</Term>
                                " is one benchmark cell, budget "<Term id="k">"k"</Term>", seed and tag. Both recursion modes of a sweep live under one id (its "
                                <Term id="namespace">"summary namespace"</Term>")."
                            </p>
                            {if e.sweeps.is_empty() {
                                view! {
                                    <EmptyState
                                        title="No sweeps yet"
                                        body="Start a run in this experiment: preview the task stream, see the cost estimate, then launch."
                                    >
                                        <a class="btn btn-primary" href=route::new_run_for(&name)>"Start a run"</a>
                                    </EmptyState>
                                }
                                    .into_any()
                            } else {
                                view! {
                                    <div class="table-wrap">
                                        <table class="table">
                                            <thead>
                                                <tr>
                                                    <th scope="col">"Sweep"</th>
                                                    <th scope="col">"Executor"</th>
                                                    <th scope="col" class="num">"k"</th>
                                                    <th scope="col" class="num">"Seed"</th>
                                                    <th scope="col">"Modes"</th>
                                                    <th scope="col">"Tasks"</th>
                                                    <th scope="col" class="num">"Spend"</th>
                                                    <th scope="col">"Scores"</th>
                                                    <th scope="col">"Updated"</th>
                                                </tr>
                                            </thead>
                                            <tbody>
                                                {e
                                                    .sweeps
                                                    .iter()
                                                    .map(|sw| {
                                                        let modes = sw.modes.iter().map(|m| mode_label(*m)).collect::<Vec<_>>().join(" + ");
                                                        view! {
                                                            <tr data-testid="sweep-row" data-sweep=sw.id.clone()>
                                                                <td class="nowrap">
                                                                    <a class="cell-title mono" href=route::sweep(&name, &sw.id)>{sw.id.clone()}</a>
                                                                    <span class="cell-sub">{sw.bench_name.clone()}</span>
                                                                </td>
                                                                <td>{sw.substrate.display_name()}</td>
                                                                <td class="num">{sw.k}</td>
                                                                <td class="num">{sw.seed.map_or_else(|| format::DASH.to_owned(), |s| s.to_string())}</td>
                                                                <td>{modes}</td>
                                                                <td>
                                                                    <span class="pill-row">
                                                                        <span>{sw.tasks}</span>
                                                                        <Badge tone=Tone::Success text=format!("{} ok", sw.ok) />
                                                                        {(sw.partial > 0).then(|| view! { <Badge tone=Tone::Warning text=format!("{} partial", sw.partial) /> })}
                                                                        {(sw.failed > 0).then(|| view! { <Badge tone=Tone::Danger text=format!("{} failed", sw.failed) /> })}
                                                                    </span>
                                                                </td>
                                                                <td class="num">{format::usd(sw.usd)}</td>
                                                                <td>
                                                                    <HeadlineScores scores=sw.headline.clone() />
                                                                </td>
                                                                <td class="nowrap">
                                                                    {sw.updated_at.clone().map(|u| view! { <Ago ts=u /> })}
                                                                </td>
                                                            </tr>
                                                        }
                                                    })
                                                    .collect_view()}
                                            </tbody>
                                        </table>
                                    </div>
                                }
                                    .into_any()
                            }}
                        </Card>
                        <EditExperiment open=edit_open exp=edit on_saved=saved />
                    }
                },
            )}
            <Card title="Jobs in this experiment">
                {loaded(
                    jobs,
                    3,
                    |page| {
                        if page.items.is_empty() {
                            view! { <p class="muted">"No jobs have touched this experiment yet."</p> }.into_any()
                        } else {
                            view! {
                                <ul class="job-mini-list">
                                    {page.items.into_iter().map(|j| view! { <JobMini job=j /> }).collect_view()}
                                </ul>
                            }
                                .into_any()
                        }
                    },
                )}
                <a class="link-more" href=move || format!("/jobs?experiment={}", crate::logic::urls::enc(&exp.get()))>
                    "All jobs of this experiment →"
                </a>
            </Card>
        </div>
    }
}
