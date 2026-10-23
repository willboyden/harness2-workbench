//! Settings: model profiles, secrets, API tokens, the audit log and system information.

mod audit;
mod profiles;
mod secrets;
mod system;
mod tokens;

use h2s_api::Role;
use leptos::prelude::*;

use crate::components::ui::{EmptyState, Notice, PageHeader};
use crate::logic::jobs::Tone;
use crate::nav::param;
use crate::state::session;

const SECTIONS: [(&str, &str, Role); 5] = [
    ("profiles", "Model profiles", Role::Viewer),
    ("secrets", "Secrets", Role::Admin),
    ("tokens", "API tokens", Role::Admin),
    ("audit", "Audit log", Role::Admin),
    ("system", "System", Role::Viewer),
];

#[component]
pub(crate) fn Settings() -> impl IntoView {
    let s = session();
    let section = param("section");
    let title = Signal::derive(move || {
        let sec = section.get();
        SECTIONS.iter().find(|(id, _, _)| *id == sec).map_or_else(
            || "Settings".to_owned(),
            |(_, label, _)| format!("Settings · {label}"),
        )
    });
    view! {
        <div class="page">
            <PageHeader title=title />
            <nav class="tabs settings-nav" aria-label="Settings sections">
                {SECTIONS
                    .iter()
                    .map(|(id, label, role)| {
                        let id = *id;
                        let role = *role;
                        view! {
                            <a
                                class="tab"
                                href=format!("/settings/{id}")
                                aria-current=move || (section.get() == id).then_some("page")
                            >
                                {*label}
                                {move || (!s.can(role)).then(|| view! { <span class="visually-hidden">" (admins only)"</span> })}
                            </a>
                        }
                    })
                    .collect_view()}
            </nav>
            {move || {
                let sec = section.get();
                let Some((_, label, role)) = SECTIONS.iter().find(|(id, _, _)| *id == sec).copied() else {
                    return view! {
                        <EmptyState title="Unknown settings section" body="Choose a section above." />
                    }
                    .into_any();
                };
                if !s.can(role) {
                    return view! {
                        <Notice tone=Tone::Info title=format!("{label} are for administrators")>
                            <p>"Your role does not include this section. The server refuses these requests anyway; ask an administrator."</p>
                        </Notice>
                    }
                    .into_any();
                }
                match sec.as_str() {
                    "profiles" => view! { <profiles::Profiles /> }.into_any(),
                    "secrets" => view! { <secrets::Secrets /> }.into_any(),
                    "tokens" => view! { <tokens::Tokens /> }.into_any(),
                    "audit" => view! { <audit::AuditLog /> }.into_any(),
                    _ => view! { <system::System /> }.into_any(),
                }
            }}
        </div>
    }
}
