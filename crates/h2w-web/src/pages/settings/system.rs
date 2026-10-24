//! System information: versions, isolation, features and the full environment check.

use h2w_api::{
    AuthMode, DoctorReport, InstallExtra, InstallParams, Isolation, JobSpec, Role, SystemInfo,
};
use leptos::prelude::*;
use leptos_router::hooks::use_navigate;

use crate::actions;
use crate::api;
use crate::components::form::{Checkbox, TextField};
use crate::components::icons::{Icon, icon};
use crate::components::ui::{Badge, Card, Skeleton, Term, loaded};
use crate::components::widgets::{Harness2Status, level_label, level_tone};
use crate::logic::format;
use crate::logic::jobs::Tone;
use crate::logic::names;
use crate::logic::urls::api as paths;
use crate::state::{ConfirmRequest, session, toasts, ui};

/// Admin: install harness2 at the tested commit or another one, optionally reinstalling.
#[component]
fn InstallForm() -> impl IntoView {
    let t = toasts();
    let u = ui();
    let navigate = use_navigate();
    let rev = RwSignal::new(String::new());
    let reinstall = RwSignal::new(false);
    let bridge = RwSignal::new(false);
    let rev_error = Signal::derive(move || {
        let r = rev.get();
        let r = r.trim();
        (!r.is_empty()).then(|| names::commit(r).err()).flatten()
    });
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let r = rev.get_untracked().trim().to_owned();
        if !r.is_empty() && names::commit(&r).is_err() {
            return;
        }
        let mut extras = vec![InstallExtra::Render];
        if bridge.get_untracked() {
            extras.push(InstallExtra::Bridge);
        }
        let other = !r.is_empty();
        let spec = JobSpec::InstallHarness2(InstallParams {
            rev: other.then_some(r),
            extras,
            reinstall: reinstall.get_untracked(),
        });
        let navigate = navigate.clone();
        u.ask(ConfirmRequest {
            title: "Install harness2?".to_owned(),
            body: if other {
                "This installs a commit this Workbench release was not tested with. The server refuses it unless paths.allow_untested_harness2_rev = true, and output formats the Workbench reads may differ.".to_owned()
            } else {
                "This installs the commit this Workbench release was tested with.".to_owned()
            },
            confirm_label: "Install".to_owned(),
            danger: false,
            on_confirm: Callback::new(move |()| actions::start_job(spec.clone(), t, None, navigate.clone())),
        });
    };
    view! {
        <Card title="Install or reinstall harness2">
            <form on:submit=submit novalidate=true>
                <TextField
                    id="install-rev"
                    label="Commit (optional)"
                    value=rev
                    error=rev_error
                    placeholder="40-character commit id"
                    help="Empty = the commit this Workbench release was tested with. Another commit needs paths.allow_untested_harness2_rev = true in the server configuration."
                />
                <Checkbox id="install-reinstall" label="Re-create the virtual environment" checked=reinstall help="Use when the environment is damaged or after a Python upgrade." />
                <Checkbox id="install-bridge" label="Include the bridge extra" checked=bridge help="Installs the LiteLLM proxy harness2-bridge needs to serve Codex on Vertex AI." />
                <button type="submit" class="btn btn-primary">{icon(Icon::Download)}" Install"</button>
            </form>
        </Card>
    }
}

#[component]
pub(crate) fn System() -> impl IntoView {
    let s = session();
    let info = LocalResource::new(|| async { api::get::<SystemInfo>(&paths::system_info()).await });
    let refresh = RwSignal::new(false);
    let doctor = LocalResource::new(move || {
        let r = refresh.get();
        async move { api::get::<DoctorReport>(&paths::doctor(r)).await }
    });
    view! {
        {loaded(
            info,
            6,
            |i| {
                let features = i.features.clone();
                let status_info = i.clone();
                let home = i.harness2.home.clone();
                view! {
                    <div class="grid-2">
                        <Card title="Server">
                            <dl class="kv">
                                <dt>"Workbench version"</dt>
                                <dd>{i.workbench_version.clone()}{i.build_commit.clone().map(|c| format!(" ({c})"))}</dd>
                                <dt>"Authentication"</dt>
                                <dd>{match i.auth_mode { AuthMode::Token => "API tokens (browser sessions from a token)", AuthMode::Proxy => "trusted reverse proxy (SSO), plus tokens" }}</dd>
                                <dt>"Isolation"</dt>
                                <dd>
                                    <Term id="isolation">
                                        {match i.isolation { Isolation::Host => "host", Isolation::Container => "container" }}
                                    </Term>
                                    {i.container_runtime.clone().map(|r| format!(" ({r})"))}
                                </dd>
                                <dt>"Jobs"</dt>
                                <dd>{format!("{} running, at most {} at once", i.jobs_running, i.jobs_max_concurrent)}</dd>
                                <dt>"Server time"</dt>
                                <dd>{format::timestamp(&i.server_time)}</dd>
                                {i.data_dir.clone().map(|d| view! { <dt>"Data directory"</dt><dd class="mono small">{d}</dd> })}
                                <dt>"Features"</dt>
                                <dd>
                                    <span class="pill-row">
                                        {features.into_iter().map(|f| view! { <Badge tone=Tone::Neutral text=f /> }).collect_view()}
                                    </span>
                                </dd>
                            </dl>
                            {(i.isolation == Isolation::Host)
                                .then(|| {
                                    let ack = if i.host_risks_preacknowledged {
                                        "An administrator pre-acknowledged these risks for everyone (isolation.preacknowledge_host_risks), so runs do not ask."
                                    } else {
                                        "Starting a run asks for an explicit acknowledgement."
                                    };
                                    view! {
                                        <p class="small muted">
                                            "Jobs run directly on this host as the server's user, as upstream does, so an executor can also read and change the Workbench's own data (it is effectively an administrator). "
                                            {ack}
                                            " Container isolation, set in h2workbench.toml, is the boundary."
                                        </p>
                                    }
                                })}
                        </Card>
                        <Card title="harness2">
                            <Harness2Status info=status_info />
                            {home.map(|h| view! { <p class="small muted">"Checkout: "<code>{h}</code></p> })}
                        </Card>
                    </div>
                }
            },
        )}
        {move || s.can(Role::Admin).then(|| view! { <InstallForm /> })}
        <Card title="Environment checks">
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
                                " Re-run checks"
                            </button>
                        }
                    })
            }}
            {move || match doctor.get() {
                None => view! { <Skeleton lines=6 /> }.into_any(),
                Some(Err(e)) => view! { <crate::components::ui::ErrorPanel error=e /> }.into_any(),
                Some(Ok(r)) => view! {
                    <p class="small muted">{format!("Checked {}.", format::timestamp(&r.generated_at))}</p>
                    <div class="table-wrap">
                        <table class="table">
                            <thead>
                                <tr>
                                    <th scope="col">"Check"</th>
                                    <th scope="col">"Result"</th>
                                    <th scope="col">"Details and fix"</th>
                                    <th scope="col">"Needed for"</th>
                                </tr>
                            </thead>
                            <tbody>
                                {r.checks
                                    .into_iter()
                                    .map(|c| {
                                        view! {
                                            <tr>
                                                <td><span class="cell-title">{c.label.clone()}</span><span class="cell-sub mono">{c.id.clone()}</span></td>
                                                <td><Badge tone=level_tone(c.level) text=level_label(c.level) /></td>
                                                <td>
                                                    {c.detail.clone()}
                                                    {c.remedy.clone().map(|r| view! { <span class="cell-sub">"Fix: "{r}</span> })}
                                                </td>
                                                <td class="small">{c.needed_for.join(", ")}</td>
                                            </tr>
                                        }
                                    })
                                    .collect_view()}
                            </tbody>
                        </table>
                    </div>
                }
                .into_any(),
            }}
        </Card>
    }
}
