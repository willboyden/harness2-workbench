//! The experiments list and the "new experiment" dialog.

use h2w_api::{CreateExperimentRequest, Experiment, ExperimentKind, ModelProfile, Role};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::NavigateOptions;
use leptos_router::hooks::use_navigate;

use crate::actions;
use crate::api::{self, ApiError};
use crate::components::form::{SelectField, TextArea, TextField};
use crate::components::icons::{Icon, icon};
use crate::components::modal::Modal;
use crate::components::ui::{Ago, EmptyState, ErrorPanel, PageHeader, Skeleton};
use crate::components::widgets::{HeadlineScores, kind_badge};
use crate::logic::format;
use crate::logic::names;
use crate::logic::urls::{api as paths, route};
use crate::state::{session, toasts};

/// The create-experiment dialog. `open` controls it; on success it navigates to the new
/// experiment.
#[component]
pub(crate) fn CreateExperiment(
    open: RwSignal<bool>,
    #[prop(optional)] on_created: Option<Callback<String>>,
) -> impl IntoView {
    let t = toasts();
    let navigate = use_navigate();
    let name = RwSignal::new(String::new());
    let description = RwSignal::new(String::new());
    let tags = RwSignal::new(String::new());
    let profile = RwSignal::new(String::new());
    let touched = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let server_error = RwSignal::new(None::<ApiError>);
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
    Effect::new(move |_| {
        // Preselect the only profile when there is exactly one.
        if let Some(Ok(list)) = profiles.get()
            && list.len() == 1
            && profile.get_untracked().is_empty()
        {
            profile.set(list[0].name.clone());
        }
    });
    let name_error = Signal::derive(move || {
        let n = name.get();
        if let Some(e) = server_error.with(|e| e.as_ref().and_then(|e| e.field("name"))) {
            return Some(e);
        }
        if !touched.get() && n.is_empty() {
            return None;
        }
        names::experiment_name(&n).err()
    });
    let tags_error = Signal::derive(move || names::parse_tags(&tags.get()).err());
    let options = Signal::derive(move || {
        let mut v = vec![(
            String::new(),
            "No profile yet (pin one on the first run)".to_owned(),
        )];
        if let Some(Ok(list)) = profiles.get() {
            v.extend(list.into_iter().map(|p| {
                let label = format!("{} — {}", p.name, crate::logic::profile::kind_label(p.kind));
                (p.name, label)
            }));
        }
        v
    });
    let close = Callback::new(move |()| {
        open.set(false);
        server_error.set(None);
    });
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        touched.set(true);
        server_error.set(None);
        let n = name.get_untracked().trim().to_owned();
        if names::experiment_name(&n).is_err() {
            return;
        }
        let Ok(tag_list) = names::parse_tags(&tags.get_untracked()) else {
            return;
        };
        let req = CreateExperimentRequest {
            name: n,
            description: description.get_untracked().trim().to_owned(),
            tags: tag_list,
            profile: Some(profile.get_untracked()).filter(|p| !p.is_empty()),
            kind: ExperimentKind::Standard,
        };
        busy.set(true);
        let navigate = navigate.clone();
        spawn_local(async move {
            let result = api::post::<_, Experiment>(&paths::experiments(false), &req).await;
            busy.set(false);
            match result {
                Ok(e) => {
                    open.set(false);
                    name.set(String::new());
                    description.set(String::new());
                    tags.set(String::new());
                    touched.set(false);
                    t.success(&format!("Experiment {} created", e.name));
                    match on_created {
                        Some(cb) => cb.run(e.name),
                        None => navigate(&route::experiment(&e.name), NavigateOptions::default()),
                    }
                }
                Err(e) => server_error.set(Some(e)),
            }
        });
    };
    view! {
        <Modal open=open on_close=close title="New experiment" testid="create-experiment-dialog">
            <form on:submit=submit.clone() novalidate=true>
                <p class="muted">
                    "An experiment is one harness2 results folder. Sweeps you run in it share its model profile."
                </p>
                <TextField
                    id="exp-name"
                    label="Name"
                    value=name
                    required=true
                    error=name_error
                    placeholder="lab-pilot"
                    testid="experiment-name"
                    help="Lowercase letters, digits, - and _ (at most 63, starting with a letter or digit). It is also the folder name and cannot be changed later."
                />
                <TextArea
                    id="exp-desc"
                    label="Description"
                    value=description
                    rows=2
                    placeholder="What question does this experiment answer?"
                />
                <TextField
                    id="exp-tags"
                    label="Tags"
                    value=tags
                    error=tags_error
                    placeholder="pilot, lab"
                    help="Optional, separated by commas or spaces."
                />
                {move || match profiles.get() {
                    None => view! { <Skeleton lines=1 /> }.into_any(),
                    Some(Err(e)) => view! { <ErrorPanel error=e /> }.into_any(),
                    Some(Ok(_)) => view! {
                        <SelectField
                            id="exp-profile"
                            label="Model profile"
                            value=profile
                            options=options
                            help="Pinned for good: harness2 refuses to resume or judge results under different models, so every run in this experiment must use this profile."
                        />
                    }
                    .into_any(),
                }}
                {move || {
                    server_error
                        .get()
                        .filter(|e| e.field("name").is_none())
                        .map(|e| view! { <ErrorPanel error=e /> })
                }}
                <div class="modal-actions">
                    <button type="button" class="btn btn-secondary" on:click=move |_| close.run(())>
                        "Cancel"
                    </button>
                    <button type="submit" class="btn btn-primary" disabled=move || busy.get() data-testid="experiment-create-submit">
                        {move || if busy.get() { "Creating…" } else { "Create experiment" }}
                    </button>
                </div>
            </form>
        </Modal>
    }
}

#[component]
pub(crate) fn Experiments() -> impl IntoView {
    let s = session();
    let t = toasts();
    let navigate = use_navigate();
    let show_archived = RwSignal::new(false);
    let filter = RwSignal::new(String::new());
    let create_open = RwSignal::new(false);
    let data = LocalResource::new(move || {
        let archived = show_archived.get();
        async move { api::get::<Vec<Experiment>>(&paths::experiments(archived)).await }
    });
    let open_row = crate::nav::row_navigator();
    let demo_busy = RwSignal::new(false);
    let run_demo = {
        let navigate = navigate.clone();
        move |_| actions::start_job(actions::demo_spec(), t, Some(demo_busy), navigate.clone())
    };
    view! {
        <div class="page">
            <PageHeader
                title="Experiments"
                subtitle="Each experiment is one harness2 results folder, pinned to a model profile. Open one to see its sweeps."
            >
                {move || {
                    s.can(Role::Operator)
                        .then(|| {
                            view! {
                                <button
                                    type="button"
                                    class="btn btn-primary"
                                    data-testid="experiment-create"
                                    on:click=move |_| create_open.set(true)
                                >
                                    {icon(Icon::Plus)}
                                    " New experiment"
                                </button>
                            }
                        })
                }}
            </PageHeader>
            <div class="toolbar">
                <div class="field grow">
                    <label for="page-search">"Filter"</label>
                    <input
                        id="page-search"
                        class="input"
                        type="search"
                        placeholder="Name, description, tag or profile"
                        bind:value=filter
                    />
                </div>
                <div class="field field-check">
                    <input id="show-archived" type="checkbox" bind:checked=show_archived />
                    <label for="show-archived">"Show archived"</label>
                </div>
            </div>
            {move || match data.get() {
                None => view! { <Skeleton lines=6 /> }.into_any(),
                Some(Err(e)) => {
                    let retry = Callback::new(move |()| data.refetch());
                    view! { <ErrorPanel error=e retry=retry /> }.into_any()
                }
                Some(Ok(list)) if list.is_empty() => {
                    let run_demo = run_demo.clone();
                    view! {
                    <EmptyState
                        title="No experiments yet"
                        body="Run the free demo to get an example experiment in about a second, or create one for a benchmark run."
                    >
                        {move || {
                            let run_demo = run_demo.clone();
                            s.can(Role::Operator)
                                .then(|| {
                                    view! {
                                        <button type="button" class="btn btn-secondary" disabled=move || demo_busy.get() on:click=run_demo.clone()>
                                            {icon(Icon::Play)}
                                            " Run the demo"
                                        </button>
                                        <button type="button" class="btn btn-primary" on:click=move |_| create_open.set(true)>
                                            "Create an experiment"
                                        </button>
                                    }
                                })
                        }}
                    </EmptyState>
                    }
                    .into_any()
                }
                Some(Ok(list)) => {
                    let q = filter.get().to_lowercase();
                    let mut rows: Vec<Experiment> = list
                        .into_iter()
                        .filter(|e| {
                            q.is_empty() || e.name.contains(&q) || e.description.to_lowercase().contains(&q)
                                || e.tags.iter().any(|t| t.contains(&q))
                                || e.profile.as_deref().is_some_and(|p| p.contains(&q))
                        })
                        .collect();
                    rows.sort_by(|a, b| b.created_at.cmp(&a.created_at));
                    if rows.is_empty() {
                        return view! { <p class="muted">"No experiment matches the filter."</p> }.into_any();
                    }
                    view! {
                        <div class="table-wrap">
                            <table class="table">
                                <caption class="visually-hidden">"Experiments"</caption>
                                <thead>
                                    <tr>
                                        <th scope="col">"Experiment"</th>
                                        <th scope="col">"Kind"</th>
                                        <th scope="col">"Profile"</th>
                                        <th scope="col" class="num">"Sweeps"</th>
                                        <th scope="col">"Best result"</th>
                                        <th scope="col" class="num">"Spend"</th>
                                        <th scope="col">"Created"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {rows
                                        .into_iter()
                                        .map(|e| {
                                            let spend: f64 = e.sweeps.iter().map(|s| s.usd).sum();
                                            let best = e.sweeps.iter().find(|s| s.headline.is_some()).and_then(|s| s.headline.clone());
                                            let href = route::experiment(&e.name);
                                            let open_row = open_row.clone();
                                            view! {
                                                <tr class="row-link" on:click=move |ev| open_row(&ev, &href)>
                                                    <th scope="row" data-testid="experiment-row" data-name=e.name.clone()>
                                                        <a class="cell-title" href=route::experiment(&e.name)>
                                                            {e.name.clone()}
                                                        </a>
                                                        {e.archived.then(|| view! { " " <span class="badge tone-neutral">"archived"</span> })}
                                                        <span class="cell-sub">{e.description.clone()}</span>
                                                        {(!e.tags.is_empty())
                                                            .then(|| {
                                                                view! {
                                                                    <span class="cell-sub">
                                                                        {e.tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" ")}
                                                                    </span>
                                                                }
                                                            })}
                                                    </th>
                                                    <td>{kind_badge(e.kind)}</td>
                                                    <td class="nowrap">
                                                        {e.profile.clone().map_or_else(
                                                            || view! { <span class="muted">"not pinned"</span> }.into_any(),
                                                            |p| view! { <code>{p}</code> }.into_any(),
                                                        )}
                                                    </td>
                                                    <td class="num">{e.sweeps.len()}</td>
                                                    <td>
                                                        <HeadlineScores scores=best />
                                                    </td>
                                                    <td class="num">{format::usd(spend)}</td>
                                                    <td class="nowrap">
                                                        <Ago ts=e.created_at.clone() />
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
                }
            }}
            <CreateExperiment open=create_open />
        </div>
    }
}
