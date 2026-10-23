//! The 404 page for unknown in-app routes.

use leptos::prelude::*;
use leptos_router::hooks::use_location;

use crate::components::ui::PageHeader;

#[component]
pub(crate) fn NotFound() -> impl IntoView {
    let loc = use_location();
    view! {
        <div class="page">
            <PageHeader title="Page not found" />
            <div class="empty-state">
                <h2>"Nothing lives at " <code>{move || loc.pathname.get()}</code></h2>
                <p>"The link may be old, or the experiment, sweep or job may have been renamed or archived."</p>
                <div class="empty-actions">
                    <a class="btn btn-primary" href="/">"Go to the dashboard"</a>
                    <a class="btn btn-secondary" href="/experiments">"Browse experiments"</a>
                    <a class="btn btn-secondary" href="/jobs">"Browse jobs"</a>
                </div>
            </div>
        </div>
    }
}
