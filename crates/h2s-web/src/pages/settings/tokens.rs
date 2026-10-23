//! API tokens: create (shown once), list, revoke.

use h2s_api::{CreateTokenRequest, Role, TokenCreated, TokenInfo};
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api;
use crate::components::form::{SelectField, TextField};
use crate::components::ui::{Ago, Badge, Card, CopyButton, EmptyState, loaded};
use crate::logic::format;
use crate::logic::jobs::Tone;
use crate::logic::names;
use crate::logic::urls::api as paths;
use crate::state::{ConfirmRequest, toasts, ui};

fn role_from(s: &str) -> Role {
    match s {
        "admin" => Role::Admin,
        "operator" => Role::Operator,
        _ => Role::Viewer,
    }
}

#[component]
pub(crate) fn Tokens() -> impl IntoView {
    let t = toasts();
    let u = ui();
    let data = LocalResource::new(|| async { api::get::<Vec<TokenInfo>>(&paths::tokens()).await });
    let name = RwSignal::new(String::new());
    let role = RwSignal::new("operator".to_owned());
    let expiry = RwSignal::new("90".to_owned());
    let tried = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let created = RwSignal::new(None::<TokenCreated>);
    let name_error = Signal::derive(move || {
        let n = name.get();
        (tried.get() || !n.is_empty())
            .then(|| names::token_name(&n).err())
            .flatten()
    });
    let create = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        tried.set(true);
        let n = name.get_untracked().trim().to_owned();
        if names::token_name(&n).is_err() {
            return;
        }
        let req = CreateTokenRequest {
            name: n,
            role: role_from(&role.get_untracked()),
            expires_in_days: expiry.get_untracked().parse().ok(),
        };
        busy.set(true);
        spawn_local(async move {
            let r = api::post::<_, TokenCreated>(&paths::tokens(), &req).await;
            busy.set(false);
            match r {
                Ok(c) => {
                    tried.set(false);
                    name.set(String::new());
                    created.set(Some(c));
                    data.refetch();
                }
                Err(e) => t.error("Could not create the token", &e),
            }
        });
    };
    let revoke = move |id: String, label: String| {
        u.ask(ConfirmRequest {
            title: format!("Revoke “{label}”?"),
            body: "Requests with this token fail from now on, and browser sessions opened with it end. This cannot be undone.".to_owned(),
            confirm_label: "Revoke".to_owned(),
            danger: true,
            on_confirm: Callback::new(move |()| {
                let id = id.clone();
                spawn_local(async move {
                    match api::delete(&paths::token(&id)).await {
                        Ok(()) => {
                            t.success("Token revoked");
                            data.refetch();
                        }
                        Err(e) => t.error("Could not revoke the token", &e),
                    }
                });
            }),
        });
    };
    let now = u.now;
    view! {
        <p class="muted">
            "Tokens let people and tools (CI, the MCP server) use the API. Give each its own token with the smallest role it needs. "
            "Viewers read; operators also create experiments and start or cancel jobs; admins also install, set up benchmarks and manage profiles, secrets, tokens and the audit log."
        </p>
        {move || {
            created
                .get()
                .map(|c| {
                    let token = c.token.clone();
                    view! {
                        <div class="token-once" role="alert" data-testid="token-created">
                            <p class="notice-title">{format!("Token “{}” created. Copy it now: it is never shown again.", c.info.name)}</p>
                            <code>{c.token}</code>
                            <div class="pill-row">
                                <CopyButton text=token label="Copy token" testid="token-copy" />
                                <button type="button" class="btn btn-ghost btn-sm" on:click=move |_| created.set(None)>"Done, I saved it"</button>
                            </div>
                        </div>
                    }
                })
        }}
        <Card title="Create a token">
            <form on:submit=create novalidate=true class="form-grid">
                <TextField id="token-name" label="Name" value=name error=name_error placeholder="ci-runner" help="Who or what uses it, e.g. “alice laptop” or “ci-runner”." />
                <SelectField
                    id="token-role"
                    label="Role"
                    value=role
                    options=Signal::stored(vec![
                        ("viewer".to_owned(), "viewer — read only".to_owned()),
                        ("operator".to_owned(), "operator — run and cancel jobs".to_owned()),
                        ("admin".to_owned(), "admin — everything".to_owned()),
                    ])
                />
                <SelectField
                    id="token-expiry"
                    label="Expires"
                    value=expiry
                    options=Signal::stored(vec![
                        ("30".to_owned(), "in 30 days".to_owned()),
                        ("90".to_owned(), "in 90 days (recommended)".to_owned()),
                        ("180".to_owned(), "in 180 days".to_owned()),
                        ("365".to_owned(), "in a year".to_owned()),
                        ("never".to_owned(), "never (discouraged)".to_owned()),
                    ])
                />
                <div class="span-2">
                    <button type="submit" class="btn btn-primary" disabled=move || busy.get() data-testid="token-create">"Create token"</button>
                </div>
            </form>
        </Card>
        {loaded(
            data,
            4,
            move |list| {
                if list.is_empty() {
                    return view! { <EmptyState title="No tokens" body="Create one above." /> }.into_any();
                }
                view! {
                    <div class="table-wrap">
                        <table class="table">
                            <thead>
                                <tr>
                                    <th scope="col">"Name"</th>
                                    <th scope="col">"Role"</th>
                                    <th scope="col">"Created"</th>
                                    <th scope="col">"Last used"</th>
                                    <th scope="col">"Expires"</th>
                                    <th scope="col">"Status"</th>
                                    <th scope="col"><span class="visually-hidden">"Actions"</span></th>
                                </tr>
                            </thead>
                            <tbody>
                                {list
                                    .into_iter()
                                    .map(|tk| {
                                        let expired = tk.expires_at.as_deref().and_then(format::parse_rfc3339).is_some_and(|e| e < now.get_untracked());
                                        let (id, label) = (tk.id.clone(), tk.name.clone());
                                        view! {
                                            <tr data-testid="token-row">
                                                <td>
                                                    <span class="cell-title">{tk.name.clone()}</span>
                                                    <span class="cell-sub mono">{format!("id {}", tk.id)}</span>
                                                </td>
                                                <td><Badge tone=Tone::Neutral text=tk.role.as_str() /></td>
                                                <td class="nowrap"><Ago ts=tk.created_at.clone() /><span class="cell-sub">{tk.created_by.clone()}</span></td>
                                                <td class="nowrap">{tk.last_used_at.clone().map_or_else(|| view! { <span class="muted">"never"</span> }.into_any(), |l| view! { <Ago ts=l /> }.into_any())}</td>
                                                <td class="nowrap">{tk.expires_at.clone().map_or_else(|| "never".to_owned(), |e| format::timestamp(&e))}</td>
                                                <td>
                                                    {if tk.revoked {
                                                        view! { <Badge tone=Tone::Danger text="revoked" /> }.into_any()
                                                    } else if expired {
                                                        view! { <Badge tone=Tone::Warning text="expired" /> }.into_any()
                                                    } else {
                                                        view! { <Badge tone=Tone::Success text="active" /> }.into_any()
                                                    }}
                                                </td>
                                                <td>
                                                    {(!tk.revoked).then(|| view! {
                                                        <button type="button" class="btn btn-ghost btn-sm" on:click=move |_| revoke(id.clone(), label.clone())>"Revoke"</button>
                                                    })}
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
    }
}
