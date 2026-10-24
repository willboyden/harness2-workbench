//! Secrets: write-only. The page shows whether a secret is set and its length, never a value.

use h2s_api::{SecretInfo, SetSecretRequest};
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api;
use crate::components::form::TextField;
use crate::components::modal::Modal;
use crate::components::ui::{Ago, Badge, Card, EmptyState, loaded};
use crate::logic::format;
use crate::logic::jobs::Tone;
use crate::logic::names;
use crate::logic::urls::api as paths;
use crate::state::{ConfirmRequest, Toasts, toasts, ui};

/// Store a value; the input is cleared whatever happens.
fn store(
    t: Toasts,
    name: String,
    value: RwSignal<String>,
    busy: RwSignal<bool>,
    done: Callback<()>,
) {
    let v = value.get_untracked();
    value.set(String::new());
    busy.set(true);
    spawn_local(async move {
        let r =
            api::put::<_, SecretInfo>(&paths::secret(&name), &SetSecretRequest { value: v }).await;
        busy.set(false);
        match r {
            Ok(info) => {
                t.success(&format!(
                    "Secret {} set ({})",
                    info.name,
                    info.chars.map_or_else(
                        || "length unknown".to_owned(),
                        |n| format!("{n} characters")
                    )
                ));
                done.run(());
            }
            Err(e) => t.error("Could not store the secret", &e),
        }
    });
}

#[component]
pub(crate) fn Secrets() -> impl IntoView {
    let t = toasts();
    let u = ui();
    let data =
        LocalResource::new(|| async { api::get::<Vec<SecretInfo>>(&paths::secrets()).await });
    let name = RwSignal::new(String::new());
    let value = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let tried = RwSignal::new(false);
    let refresh = Callback::new(move |()| data.refetch());
    let name_error = Signal::derive(move || {
        let n = name.get();
        (tried.get() || !n.is_empty())
            .then(|| names::secret_name(&n).err())
            .flatten()
    });
    let value_error = Signal::derive(move || {
        (tried.get() && value.get().is_empty()).then(|| "Paste the value.".to_owned())
    });
    let add = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        tried.set(true);
        let n = name.get_untracked().trim().to_owned();
        if names::secret_name(&n).is_err() || value.get_untracked().is_empty() {
            return;
        }
        tried.set(false);
        name.set(String::new());
        store(t, n, value, busy, refresh);
    };
    // "Set value" dialog for an existing name.
    let set_open = RwSignal::new(false);
    let set_name = RwSignal::new(String::new());
    let set_value = RwSignal::new(String::new());
    let set_close = Callback::new(move |()| {
        set_value.set(String::new());
        set_open.set(false);
    });
    let set_submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if set_value.get_untracked().is_empty() {
            return;
        }
        set_open.set(false);
        store(t, set_name.get_untracked(), set_value, busy, refresh);
    };
    let remove = move |n: String| {
        u.ask(ConfirmRequest {
            title: format!("Delete secret {n}?"),
            body: "Profiles that reference it will refuse to run until it is set again.".to_owned(),
            confirm_label: "Delete".to_owned(),
            danger: true,
            on_confirm: Callback::new(move |()| {
                let n = n.clone();
                spawn_local(async move {
                    match api::delete(&paths::secret(&n)).await {
                        Ok(()) => {
                            t.success(&format!("Secret {n} deleted"));
                            data.refetch();
                        }
                        Err(e) => t.error("Could not delete the secret", &e),
                    }
                });
            }),
        });
    };
    view! {
        <p class="muted">
            "Values are write-only: the server never returns them, and they never appear in logs (each job's output is redacted). This page shows only whether a secret is set and its length, so a truncated paste is easy to spot. Environment variables "
            <code>"H2S_SECRET_<NAME>"</code>" on the server take precedence."
        </p>
        <Card title="Add or replace a secret">
            <form on:submit=add novalidate=true class="form-grid">
                <TextField id="secret-name" label="Name" value=name error=name_error placeholder="MODEL_GATEWAY_KEY" help="Uppercase letters, digits and underscores." />
                <TextField
                    id="secret-value"
                    label="Value"
                    value=value
                    kind="password"
                    autocomplete="new-password"
                    error=value_error
                    help="Not shown again after saving."
                />
                <div class="span-2">
                    <button type="submit" class="btn btn-primary" disabled=move || busy.get()>"Save secret"</button>
                </div>
            </form>
        </Card>
        {loaded(
            data,
            4,
            move |list| {
                if list.is_empty() {
                    return view! { <EmptyState title="No secrets yet" body="Add API keys here, then reference them by name in model profiles." /> }.into_any();
                }
                view! {
                    <div class="table-wrap">
                        <table class="table">
                            <thead>
                                <tr>
                                    <th scope="col">"Name"</th>
                                    <th scope="col">"Status"</th>
                                    <th scope="col">"Updated"</th>
                                    <th scope="col"><span class="visually-hidden">"Actions"</span></th>
                                </tr>
                            </thead>
                            <tbody>
                                {list
                                    .into_iter()
                                    .map(|s| {
                                        let (n1, n2) = (s.name.clone(), s.name.clone());
                                        view! {
                                            <tr data-testid="secret-row" data-name=s.name>
                                                <td class="mono">{s.name.clone()}</td>
                                                <td>
                                                    {if s.set {
                                                        let text = s.chars.map_or_else(|| "set".to_owned(), |n| format!("set, {} chars", format::int(u64::from(n))));
                                                        view! { <Badge tone=Tone::Success text=text /> }.into_any()
                                                    } else {
                                                        view! { <Badge tone=Tone::Danger text="not set (referenced by a profile)" /> }.into_any()
                                                    }}
                                                </td>
                                                <td class="nowrap">{s.updated_at.clone().map(|u| view! { <Ago ts=u /> })}</td>
                                                <td class="nowrap">
                                                    <button
                                                        type="button"
                                                        class="btn btn-secondary btn-sm"
                                                        on:click=move |_| {
                                                            set_name.set(n1.clone());
                                                            set_open.set(true);
                                                        }
                                                    >
                                                        {if s.set { "Replace value" } else { "Set value" }}
                                                    </button>
                                                    {s.set.then(|| view! { <button type="button" class="btn btn-ghost btn-sm" on:click=move |_| remove(n2.clone())>"Delete"</button> })}
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
            },
        )}
        <Modal open=set_open on_close=set_close title=Signal::derive(move || format!("Set {}", set_name.get()))>
            <form on:submit=set_submit novalidate=true>
                <TextField id="secret-set-value" label="New value" value=set_value kind="password" autocomplete="new-password" help="Replaces the stored value. It is not shown again." />
                <div class="modal-actions">
                    <button type="button" class="btn btn-secondary" on:click=move |_| set_close.run(())>"Cancel"</button>
                    <button type="submit" class="btn btn-primary">"Save"</button>
                </div>
            </form>
        </Modal>
    }
}
