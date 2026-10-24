//! Transient notifications in a polite live region (bottom-right on desktop).

use leptos::prelude::*;

use super::icons::{Icon, icon};
use crate::logic::jobs::Tone;
use crate::state::toasts;

#[component]
pub(crate) fn ToastHost() -> impl IntoView {
    let t = toasts();
    view! {
        <div class="toasts" role="region" aria-label="Notifications" aria-live="polite">
            <For each=move || t.list.get() key=|toast| toast.id let:toast>
                {
                    let id = toast.id;
                    let glyph = match toast.tone {
                        Tone::Success => Icon::Check,
                        Tone::Danger | Tone::Warning => Icon::Alert,
                        Tone::Info | Tone::Neutral => Icon::Sparkle,
                    };
                    view! {
                        <div class=format!("toast {}", toast.tone.class()) data-testid="toast">
                            <span class="toast-icon">{icon(glyph)}</span>
                            <div class="toast-text">
                                <p class="toast-title">{toast.title.clone()}</p>
                                {(!toast.body.is_empty())
                                    .then(|| view! { <p class="toast-body">{toast.body.clone()}</p> })}
                                {toast
                                    .link
                                    .clone()
                                    .map(|(label, href)| {
                                        view! {
                                            <a class="toast-link" href=href>
                                                {label}
                                            </a>
                                        }
                                    })}
                            </div>
                            <button
                                type="button"
                                class="icon-btn toast-close"
                                aria-label="Dismiss notification"
                                on:click=move |_| t.dismiss(id)
                            >
                                {icon(Icon::Close)}
                            </button>
                        </div>
                    }
                }
            </For>
        </div>
    }
}
