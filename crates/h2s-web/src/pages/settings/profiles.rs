//! Model profiles: list, create, edit, delete. API keys are referenced by secret NAME.

use h2s_api::{ModelProfile, ProfileKind, Role, SecretInfo};
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{self, ApiError};
use crate::components::form::{Checkbox, RadioCards, TextArea, TextField};
use crate::components::icons::{Icon, icon};
use crate::components::modal::Modal;
use crate::components::ui::{Badge, Card, EmptyState, ErrorPanel, Notice, Term, loaded};
use crate::logic::format;
use crate::logic::jobs::Tone;
use crate::logic::profile::{
    EnvRow, KINDS, ProfileForm, kind_from_value, kind_help, kind_label, kind_value,
};
use crate::logic::urls::api as paths;
use crate::state::{ConfirmRequest, session, toasts, ui};

/// One signal per editable field, so inputs bind directly.
#[derive(Clone, Copy)]
struct Fields {
    name: RwSignal<String>,
    kind: RwSignal<String>,
    description: RwSignal<String>,
    solver: RwSignal<String>,
    improver: RwSignal<String>,
    judge: RwSignal<String>,
    codex: RwSignal<String>,
    v_project: RwSignal<String>,
    v_location: RwSignal<String>,
    v_creds: RwSignal<String>,
    ep_url: RwSignal<String>,
    ep_secret: RwSignal<String>,
    judge_url: RwSignal<String>,
    judge_secret: RwSignal<String>,
    codex_url: RwSignal<String>,
    codex_secret: RwSignal<String>,
    env: RwSignal<Vec<(u32, EnvRow)>>,
    next_env: RwSignal<u32>,
    prices: RwSignal<bool>,
    r_low: RwSignal<String>,
    r_high: RwSignal<String>,
    s_low: RwSignal<String>,
    s_high: RwSignal<String>,
    egress: RwSignal<String>,
}

impl Fields {
    fn new() -> Fields {
        let s = || RwSignal::new(String::new());
        Fields {
            name: s(),
            kind: RwSignal::new(kind_value(ProfileKind::OpenaiCompatible).to_owned()),
            description: s(),
            solver: s(),
            improver: s(),
            judge: s(),
            codex: s(),
            v_project: s(),
            v_location: RwSignal::new("global".to_owned()),
            v_creds: s(),
            ep_url: s(),
            ep_secret: s(),
            judge_url: s(),
            judge_secret: s(),
            codex_url: s(),
            codex_secret: s(),
            env: RwSignal::new(Vec::new()),
            next_env: RwSignal::new(1),
            prices: RwSignal::new(false),
            r_low: s(),
            r_high: s(),
            s_low: s(),
            s_high: s(),
            egress: s(),
        }
    }

    fn load(self, f: &ProfileForm) {
        self.name.set(f.name.clone());
        self.kind.set(kind_value(f.kind).to_owned());
        self.description.set(f.description.clone());
        self.solver.set(f.solver_model.clone());
        self.improver.set(f.improver_model.clone());
        self.judge.set(f.judge_model.clone());
        self.codex.set(f.codex_model.clone());
        self.v_project.set(f.vertex_project.clone());
        self.v_location.set(f.vertex_location.clone());
        self.v_creds.set(f.vertex_credentials_secret.clone());
        self.ep_url.set(f.endpoint_url.clone());
        self.ep_secret.set(f.endpoint_secret.clone());
        self.judge_url.set(f.judge_url.clone());
        self.judge_secret.set(f.judge_secret.clone());
        self.codex_url.set(f.codex_url.clone());
        self.codex_secret.set(f.codex_secret.clone());
        let rows: Vec<(u32, EnvRow)> = f
            .extra_env
            .iter()
            .cloned()
            .zip(1_u32..)
            .map(|(r, i)| (i, r))
            .collect();
        self.next_env
            .set(u32::try_from(rows.len()).unwrap_or(0) + 1);
        self.env.set(rows);
        self.prices.set(f.prices);
        self.r_low.set(f.rollout_low.clone());
        self.r_high.set(f.rollout_high.clone());
        self.s_low.set(f.session_low.clone());
        self.s_high.set(f.session_high.clone());
        self.egress.set(f.egress_hosts.clone());
    }

    fn form(self) -> ProfileForm {
        ProfileForm {
            name: self.name.get(),
            kind: kind_from_value(&self.kind.get()),
            description: self.description.get(),
            solver_model: self.solver.get(),
            improver_model: self.improver.get(),
            judge_model: self.judge.get(),
            codex_model: self.codex.get(),
            vertex_project: self.v_project.get(),
            vertex_location: self.v_location.get(),
            vertex_credentials_secret: self.v_creds.get(),
            endpoint_url: self.ep_url.get(),
            endpoint_secret: self.ep_secret.get(),
            judge_url: self.judge_url.get(),
            judge_secret: self.judge_secret.get(),
            codex_url: self.codex_url.get(),
            codex_secret: self.codex_secret.get(),
            extra_env: self.env.get().into_iter().map(|(_, r)| r).collect(),
            prices: self.prices.get(),
            rollout_low: self.r_low.get(),
            rollout_high: self.r_high.get(),
            session_low: self.s_low.get(),
            session_high: self.s_high.get(),
            egress_hosts: self.egress.get(),
        }
    }
}

#[component]
fn EnvRows(f: Fields) -> impl IntoView {
    let set = move |id: u32, edit: Box<dyn Fn(&mut EnvRow)>| {
        f.env.update(|rows| {
            if let Some((_, r)) = rows.iter_mut().find(|(i, _)| *i == id) {
                edit(r);
            }
        });
    };
    view! {
        <fieldset>
            <legend>"Extra environment variables"</legend>
            <p class="field-help">"Literal values, or a reference to a secret by name. Variables that control the loader, the interpreter or the server are refused."</p>
            <For each=move || f.env.get() key=|(id, _)| *id let:row>
                {
                    let (id, r) = row;
                    let from_secret = r.from_secret;
                    view! {
                        <div class="env-row">
                            <input
                                class="input mono"
                                aria-label="Variable name"
                                placeholder="NAME"
                                prop:value=r.name.clone()
                                on:input=move |ev| { let v = event_target_value(&ev); set(id, Box::new(move |r| r.name.clone_from(&v))); }
                            />
                            <select
                                class="input"
                                aria-label="Value source"
                                on:change=move |ev| { let secret = event_target_value(&ev) == "secret"; set(id, Box::new(move |r| r.from_secret = secret)); }
                            >
                                <option value="value" selected=!from_secret>"literal value"</option>
                                <option value="secret" selected=from_secret>"secret"</option>
                            </select>
                            <input
                                class="input mono"
                                aria-label="Value or secret name"
                                list="secret-names"
                                placeholder=if from_secret { "SECRET_NAME" } else { "value" }
                                prop:value=if from_secret { r.secret } else { r.value }
                                on:input=move |ev| {
                                    let v = event_target_value(&ev);
                                    set(id, Box::new(move |r| if r.from_secret { r.secret.clone_from(&v) } else { r.value.clone_from(&v) }));
                                }
                            />
                            <button
                                type="button"
                                class="icon-btn"
                                aria-label="Remove variable"
                                on:click=move |_| f.env.update(|rows| rows.retain(|(i, _)| *i != id))
                            >
                                {icon(Icon::Close)}
                            </button>
                        </div>
                    }
                }
            </For>
            <button
                type="button"
                class="btn btn-ghost btn-sm"
                on:click=move |_| {
                    let id = f.next_env.get_untracked();
                    f.next_env.set(id + 1);
                    f.env.update(|rows| rows.push((id, EnvRow::default())));
                }
            >
                {icon(Icon::Plus)}
                " Add variable"
            </button>
        </fieldset>
    }
}

#[component]
fn ProfileEditor(
    open: RwSignal<bool>,
    editing: RwSignal<Option<String>>,
    f: Fields,
    on_saved: Callback<()>,
) -> impl IntoView {
    let t = toasts();
    let tried = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let server_error = RwSignal::new(None::<ApiError>);
    let secrets = LocalResource::new(move || {
        let o = open.get();
        async move {
            if o {
                api::get::<Vec<SecretInfo>>(&paths::secrets()).await
            } else {
                Ok(Vec::new())
            }
        }
    });
    let problems = Memo::new(move |_| f.form().to_profile().err().unwrap_or_default());
    let err = move |field: &'static str| {
        Signal::derive(move || {
            if !tried.get() {
                return None;
            }
            problems.with(|p| p.iter().find(|(k, _)| k == field).map(|(_, v)| v.clone()))
        })
    };
    let kind = Signal::derive(move || kind_from_value(&f.kind.get()));
    let close = Callback::new(move |()| {
        open.set(false);
        tried.set(false);
        server_error.set(None);
    });
    let save = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        tried.set(true);
        server_error.set(None);
        let Ok(profile) = f.form().to_profile() else {
            return;
        };
        busy.set(true);
        spawn_local(async move {
            let r = api::put::<_, ModelProfile>(&paths::profile(&profile.name), &profile).await;
            busy.set(false);
            match r {
                Ok(p) => {
                    t.success(&format!("Profile {} saved", p.name));
                    open.set(false);
                    tried.set(false);
                    on_saved.run(());
                }
                Err(e) => server_error.set(Some(e)),
            }
        });
    };
    let title = Signal::derive(move || match editing.get() {
        Some(n) => format!("Edit profile {n}"),
        None => "New model profile".to_owned(),
    });
    view! {
        <Modal open=open on_close=close title=title wide=true testid="profile-editor">
            <form on:submit=save novalidate=true>
                <datalist id="secret-names">
                    {move || {
                        secrets
                            .get()
                            .and_then(Result::ok)
                            .unwrap_or_default()
                            .into_iter()
                            .map(|s| view! { <option value=s.name></option> })
                            .collect_view()
                    }}
                </datalist>
                <RadioCards
                    name="profile-kind"
                    legend="Kind"
                    value=f.kind
                    options=KINDS.iter().map(|k| (kind_value(*k).to_owned(), kind_label(*k).to_owned(), kind_help(*k).to_owned())).collect()
                />
                <div class="form-grid">
                    <TextField
                        id="pf-name"
                        label="Name"
                        value=f.name
                        required=true
                        error=err("name")
                        disabled=Signal::derive(move || editing.get().is_some())
                        help="Lowercase letters, digits, - and _. Experiments pin a profile by this name."
                    />
                    <TextField id="pf-desc" label="Description" value=f.description />
                    <TextField
                        id="pf-solver"
                        label="Solver (executor) model"
                        value=f.solver
                        required=true
                        error=err("solver_model")
                        help=Signal::derive(move || match kind.get() {
                            ProfileKind::OpenaiCompatible => "The model id your endpoint serves, e.g. open-model-32b.".to_owned(),
                            _ => "OpenCode provider/model form, e.g. google-vertex/gemini-3.5-flash.".to_owned(),
                        })
                    />
                    <TextField id="pf-improver" label="Improver model" value=f.improver error=err("improver_model") help="Empty = same as the solver." />
                    <TextField id="pf-judge" label="Judge model" value=f.judge error=err("judge_model") help="Bare model id the judge uses." />
                    <TextField id="pf-codex" label="Codex model" value=f.codex error=err("codex_model") help="For the Codex executor." />
                </div>
                {move || match kind.get() {
                    ProfileKind::Vertex => view! {
                        <h3>"Google Vertex AI"</h3>
                        <div class="form-grid">
                            <TextField id="pf-project" label="Project id" value=f.v_project required=true error=err("vertex_project") />
                            <TextField id="pf-location" label="Location" value=f.v_location help="Upstream default: global." />
                            <TextField
                                id="pf-creds"
                                label="Service-account key secret (optional)"
                                value=f.v_creds
                                error=err("vertex_credentials_secret")
                                help="Name of a secret holding the key file's JSON. Empty = the server user's Application Default Credentials."
                            />
                        </div>
                    }
                    .into_any(),
                    ProfileKind::OpenaiCompatible => view! {
                        <h3>"Endpoint"</h3>
                        <div class="form-grid">
                            <TextField id="pf-ep-url" label="Base URL" value=f.ep_url required=true error=err("endpoint") placeholder="http://127.0.0.1:8000/v1" />
                            <TextField id="pf-ep-secret" label="API key secret (optional)" value=f.ep_secret error=err("endpoint_secret") help="The NAME of a secret, never the key itself." />
                        </div>
                    }
                    .into_any(),
                    ProfileKind::Custom => view! {
                        <Notice tone=Tone::Info>
                            <p>"Custom profiles pass the extra environment variables below to harness2 as they are."</p>
                        </Notice>
                    }
                    .into_any(),
                }}
                <details class="disclosure">
                    <summary>"Judge and Codex endpoints (optional)"</summary>
                    <div class="form-grid">
                        <TextField id="pf-judge-url" label="Judge base URL" value=f.judge_url error=err("judge") help="HARNESS2_JUDGE_BASE_URL" />
                        <TextField id="pf-judge-secret" label="Judge API key secret" value=f.judge_secret error=err("judge_secret") />
                        <TextField id="pf-codex-url" label="Codex base URL" value=f.codex_url error=err("codex") help="A Responses-API endpoint (or the Codex bridge)." />
                        <TextField id="pf-codex-secret" label="Codex API key secret" value=f.codex_secret error=err("codex_secret") />
                    </div>
                </details>
                <EnvRows f=f />
                {move || problems.with(|p| p.iter().find(|(k, _)| k.starts_with("extra_env")).map(|(_, v)| v.clone())).filter(|_| tried.get()).map(|e| view! { <p class="field-error" role="alert">{e}</p> })}
                <Checkbox id="pf-prices" label="Prices for estimates" checked=f.prices help="USD per executor rollout and per improver session, as a low–high range." />
                {move || {
                    f.prices
                        .get()
                        .then(|| {
                            view! {
                                <div class="form-grid">
                                    <TextField id="pf-r-low" label="Rollout, low (USD)" value=f.r_low inputmode="decimal" error=err("rollout_low") />
                                    <TextField id="pf-r-high" label="Rollout, high (USD)" value=f.r_high inputmode="decimal" error=err("rollout_high") />
                                    <TextField id="pf-s-low" label="Session, low (USD)" value=f.s_low inputmode="decimal" error=err("session_low") />
                                    <TextField id="pf-s-high" label="Session, high (USD)" value=f.s_high inputmode="decimal" error=err("session_high") />
                                </div>
                                {move || err("prices").get().map(|e| view! { <p class="field-error" role="alert">{e}</p> })}
                            }
                        })
                }}
                <TextArea
                    id="pf-egress"
                    label="Egress hosts"
                    value=f.egress
                    rows=2
                    monospace=true
                    error=err("egress_hosts")
                    help="Hosts container jobs may reach (one per line; *.example.com for subdomains). Empty = derived from the endpoints."
                />
                {move || server_error.get().map(|e| view! { <ErrorPanel error=e /> })}
                {move || {
                    (tried.get() && !problems.with(Vec::is_empty))
                        .then(|| view! { <Notice tone=Tone::Danger><p>"Fix the highlighted fields to save."</p></Notice> })
                }}
                <div class="modal-actions">
                    <button type="button" class="btn btn-secondary" on:click=move |_| close.run(())>"Cancel"</button>
                    <button type="submit" class="btn btn-primary" disabled=move || busy.get()>"Save profile"</button>
                </div>
            </form>
        </Modal>
    }
}

fn secret_badge(s: &SecretInfo) -> impl IntoView + use<> {
    if s.set {
        let text = match s.chars {
            Some(n) => format!("{} set, {n} chars", s.name),
            None => format!("{} set", s.name),
        };
        view! { <Badge tone=Tone::Success text=text /> }.into_any()
    } else {
        view! { <Badge tone=Tone::Danger text=format!("{} not set", s.name) /> }.into_any()
    }
}

#[component]
pub(crate) fn Profiles() -> impl IntoView {
    let s = session();
    let t = toasts();
    let u = ui();
    let data =
        LocalResource::new(|| async { api::get::<Vec<ModelProfile>>(&paths::profiles()).await });
    let open = RwSignal::new(false);
    let editing = RwSignal::new(None::<String>);
    let fields = Fields::new();
    let saved = Callback::new(move |()| data.refetch());
    let new_profile = move |_| {
        fields.load(&ProfileForm::default());
        editing.set(None);
        open.set(true);
    };
    let delete = move |name: String| {
        u.ask(ConfirmRequest {
            title: format!("Delete profile {name}?"),
            body: "Experiments pinned to it keep their results but could not run again until a profile of that name exists. The server refuses while an experiment pins it.".to_owned(),
            confirm_label: "Delete".to_owned(),
            danger: true,
            on_confirm: Callback::new(move |()| {
                let name = name.clone();
                spawn_local(async move {
                    match api::delete(&paths::profile(&name)).await {
                        Ok(()) => {
                            t.success(&format!("Profile {name} deleted"));
                            data.refetch();
                        }
                        Err(e) => t.error("Could not delete the profile", &e),
                    }
                });
            }),
        });
    };
    view! {
        <p class="muted">
            "A " <Term id="profile">"model profile"</Term>
            " says which models harness2 uses and how to reach them. Keys are referenced by secret name; their values live under Secrets."
        </p>
        {move || {
            s.can(Role::Admin)
                .then(|| view! {
                    <div>
                        <button type="button" class="btn btn-primary" data-testid="profile-create" on:click=new_profile>
                            {icon(Icon::Plus)}
                            " New profile"
                        </button>
                    </div>
                })
        }}
        {loaded(
            data,
            4,
            move |list| {
                if list.is_empty() {
                    return view! {
                        <EmptyState
                            title="No model profiles yet"
                            body="Create one for Google Vertex AI (upstream's setup), for any OpenAI-compatible endpoint, or with custom environment variables."
                        />
                    }
                    .into_any();
                }
                view! {
                    <div class="bench-grid">
                        {list
                            .into_iter()
                            .map(|p| {
                                let form = ProfileForm::from_profile(&p);
                                let name = p.name.clone();
                                let missing = p.secrets.iter().any(|s| !s.set);
                                let badges: Vec<_> = p.secrets.iter().map(secret_badge).collect();
                                view! {
                                    <Card title=p.name.clone() class="profile-card">
                                        <div class="pill-row">
                                            <Badge tone=Tone::Info text=kind_label(p.kind) />
                                            {missing.then(|| view! { <Badge tone=Tone::Danger text="secret missing" /> })}
                                        </div>
                                        {(!p.description.is_empty()).then(|| view! { <p>{p.description.clone()}</p> })}
                                        <dl class="kv">
                                            <dt>"Solver"</dt><dd class="mono">{p.solver_model.clone()}</dd>
                                            <dt>"Improver"</dt><dd class="mono">{p.improver_model.clone().unwrap_or_else(|| "same as solver".to_owned())}</dd>
                                            {p.judge_model.clone().map(|m| view! { <dt>"Judge"</dt><dd class="mono">{m}</dd> })}
                                            {p.codex_model.clone().map(|m| view! { <dt>"Codex"</dt><dd class="mono">{m}</dd> })}
                                            {p.endpoint.clone().map(|e| view! { <dt>"Endpoint"</dt><dd class="mono">{e.base_url}</dd> })}
                                            {p.vertex.clone().map(|v| view! { <dt>"Vertex"</dt><dd>{format!("{} · {}", v.project, v.location)}</dd> })}
                                            {p.prices.clone().map(|pr| view! {
                                                <dt>"Prices"</dt>
                                                <dd>{format!("rollout {}–{}, session {}–{}", format::usd(pr.rollout_usd_low), format::usd(pr.rollout_usd_high), format::usd(pr.session_usd_low), format::usd(pr.session_usd_high))}</dd>
                                            })}
                                            {(!p.egress_hosts.is_empty()).then(|| view! { <dt>"Egress"</dt><dd class="mono small">{p.egress_hosts.join(", ")}</dd> })}
                                        </dl>
                                        {(!badges.is_empty())
                                            .then(|| view! { <div class="pill-row">{badges.into_iter().collect_view()}</div> })}
                                        {missing.then(|| view! { <p class="small"><a href="/settings/secrets">"Set the missing secret"</a>" before running with this profile."</p> })}
                                        {move || {
                                            let form = form.clone();
                                            let name = name.clone();
                                            let n2 = name.clone();
                                            s.can(Role::Admin)
                                                .then(|| {
                                                    view! {
                                                        <div class="bench-actions">
                                                            <button
                                                                type="button"
                                                                class="btn btn-secondary btn-sm"
                                                                on:click=move |_| {
                                                                    fields.load(&form);
                                                                    editing.set(Some(name.clone()));
                                                                    open.set(true);
                                                                }
                                                            >
                                                                "Edit"
                                                            </button>
                                                            <button type="button" class="btn btn-ghost btn-sm" on:click=move |_| delete(n2.clone())>
                                                                "Delete"
                                                            </button>
                                                        </div>
                                                    }
                                                })
                                        }}
                                    </Card>
                                }
                            })
                            .collect_view()}
                    </div>
                }
                .into_any()
            },
        )}
        <ProfileEditor open=open editing=editing f=fields on_saved=saved />
    }
}
