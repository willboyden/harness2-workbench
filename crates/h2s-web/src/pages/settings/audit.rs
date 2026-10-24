//! The tamper-evident audit log: paged, newest first, with chain verification.

use h2s_api::{AuditEvent, AuditOutcome, AuditVerification, Page};
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{self, ApiError};
use crate::components::ui::{Badge, Cursor, EmptyState, ErrorPanel, Notice, Pager, Skeleton};
use crate::logic::format;
use crate::logic::jobs::Tone;
use crate::logic::urls::api as paths;

fn outcome_badge(o: AuditOutcome) -> impl IntoView {
    match o {
        AuditOutcome::Success => view! { <Badge tone=Tone::Success text="success" /> },
        AuditOutcome::Denied => view! { <Badge tone=Tone::Warning text="denied" /> },
        AuditOutcome::Error => view! { <Badge tone=Tone::Danger text="error" /> },
    }
}

#[component]
pub(crate) fn AuditLog() -> impl IntoView {
    let cursor = Cursor::new();
    let page = LocalResource::new(move || {
        let c = cursor.current.get();
        async move { api::get::<Page<AuditEvent>>(&paths::audit_log(50, c.as_deref())).await }
    });
    let verification = RwSignal::new(None::<Result<AuditVerification, ApiError>>);
    let busy = RwSignal::new(false);
    let verify = move |_| {
        busy.set(true);
        spawn_local(async move {
            let r = api::get::<AuditVerification>(&paths::audit_verify()).await;
            busy.set(false);
            verification.set(Some(r));
        });
    };
    let next = Signal::derive(move || page.get().and_then(Result::ok).and_then(|p| p.next_cursor));
    view! {
        <p class="muted">
            "Every sign-in, job start and cancel, and every change to experiments, profiles, secrets and tokens is appended to a SHA-256 hash chain. Values and request bodies are never recorded."
        </p>
        <div class="pill-row">
            <button type="button" class="btn btn-secondary" disabled=move || busy.get() on:click=verify data-testid="audit-verify">
                {move || if busy.get() { "Verifying…" } else { "Verify the chain" }}
            </button>
        </div>
        {move || {
            verification
                .get()
                .map(|r| match r {
                    Ok(v) if v.ok => view! {
                        <Notice tone=Tone::Success title="The chain is intact">
                            <p>{format!("All {} entries link correctly.", format::int(v.entries))}</p>
                        </Notice>
                    }
                    .into_any(),
                    Ok(v) => view! {
                        <Notice tone=Tone::Danger title="The chain is broken">
                            <p>
                                {format!(
                                    "Entry {} does not match its hash: the log was changed after it was written. {} entries checked.",
                                    v.first_bad_seq.map_or_else(|| "?".to_owned(), |s| s.to_string()),
                                    format::int(v.entries)
                                )}
                            </p>
                        </Notice>
                    }
                    .into_any(),
                    Err(e) => view! { <ErrorPanel error=e /> }.into_any(),
                })
        }}
        {move || match page.get() {
            None => view! { <Skeleton lines=8 /> }.into_any(),
            Some(Err(e)) => view! { <ErrorPanel error=e /> }.into_any(),
            Some(Ok(p)) if p.items.is_empty() => view! { <EmptyState title="The audit log is empty" body="Entries appear as people sign in and act." /> }.into_any(),
            Some(Ok(p)) => view! {
                <div class="table-wrap">
                    <table class="table">
                        <thead>
                            <tr>
                                <th scope="col" class="num">"#"</th>
                                <th scope="col">"When"</th>
                                <th scope="col">"Who"</th>
                                <th scope="col">"Action"</th>
                                <th scope="col">"Target"</th>
                                <th scope="col">"Outcome"</th>
                                <th scope="col">"Details"</th>
                            </tr>
                        </thead>
                        <tbody>
                            {p.items
                                .into_iter()
                                .map(|e| {
                                    view! {
                                        <tr>
                                            <td class="num">{e.seq}</td>
                                            <td class="nowrap">{format::timestamp(&e.ts)}</td>
                                            <td>{e.actor.clone()}{e.ip.clone().map(|ip| view! { <span class="cell-sub">{ip}</span> })}</td>
                                            <td class="mono">{e.action.clone()}</td>
                                            <td class="mono small cell-wrap">{e.target.clone()}</td>
                                            <td>{outcome_badge(e.outcome)}</td>
                                            <td class="small cell-wrap">
                                                {e.detail.clone().unwrap_or_default()}
                                                {e.request_id.map(|r| view! { <span class="cell-sub mono">{format!("request {r}")}</span> })}
                                            </td>
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
        <Pager cursor=cursor next=next />
    }
}
