//! Glossary of every term, frequently asked questions and keyboard shortcuts.

use leptos::prelude::*;

use crate::components::ui::PageHeader;
use crate::logic::glossary::{self, FAQ, SHORTCUTS, TERMS};
use crate::logic::harness::{self, COMPONENTS};
use crate::logic::palette::rank;

#[component]
pub(crate) fn Help() -> impl IntoView {
    let filter = RwSignal::new(String::new());
    let visible = Memo::new(move |_| {
        let q = filter.get();
        let mut idx = rank(&q, TERMS.iter().map(|t| t.term));
        // Also match on the definition text.
        if !q.trim().is_empty() {
            let ql = q.to_lowercase();
            for (i, t) in TERMS.iter().enumerate() {
                if !idx.contains(&i)
                    && (t.short.to_lowercase().contains(&ql) || t.long.to_lowercase().contains(&ql))
                {
                    idx.push(i);
                }
            }
        }
        idx
    });
    view! {
        <div class="page help-page">
            <PageHeader
                title="Help"
                subtitle="Every term the Workbench uses, answers to common questions, and keyboard shortcuts."
            />
            <nav class="toc" aria-label="On this page">
                <a href="#glossary">"Glossary"</a>
                <a href="#components">"The eight components"</a>
                <a href="#faq">"FAQ"</a>
                <a href="#shortcuts">"Keyboard shortcuts"</a>
            </nav>

            <section id="glossary" aria-labelledby="glossary-title" class="card">
                <h2 id="glossary-title" class="card-title">"Glossary"</h2>
                <div class="field">
                    <label for="page-search">"Filter terms"</label>
                    <input
                        id="page-search"
                        class="input"
                        type="search"
                        placeholder="e.g. rollout, ungraded, k"
                        bind:value=filter
                    />
                </div>
                <dl class="glossary">
                    {move || {
                        let idx = visible.get();
                        if idx.is_empty() {
                            return view! { <p class="muted">"No term matches."</p> }.into_any();
                        }
                        idx.into_iter()
                            .filter_map(|i| TERMS.get(i))
                            .map(|t| {
                                view! {
                                    <div class="glossary-entry" id=format!("term-{}", t.id)>
                                        <dt>
                                            <a class="anchor" href=format!("#term-{}", t.id) aria-label=format!("Link to {}", t.term)>
                                                "#"
                                            </a>
                                            {t.term}
                                        </dt>
                                        <dd>
                                            <p><strong>{t.short}</strong></p>
                                            <p>{t.long}</p>
                                            {(!t.see_also.is_empty())
                                                .then(|| {
                                                    view! {
                                                        <p class="small muted">
                                                            "See also: "
                                                            {t
                                                                .see_also
                                                                .iter()
                                                                .filter_map(|id| glossary::find(id))
                                                                .map(|o| {
                                                                    view! {
                                                                        <a class="see-also" href=format!("#term-{}", o.id)>{o.term}</a>
                                                                    }
                                                                })
                                                                .collect_view()}
                                                        </p>
                                                    }
                                                })}
                                        </dd>
                                    </div>
                                }
                            })
                            .collect_view()
                            .into_any()
                    }}
                </dl>
            </section>

            <section id="components" aria-labelledby="components-title" class="card">
                <h2 id="components-title" class="card-title">"The eight harness components"</h2>
                <div class="table-wrap">
                <table class="table">
                    <thead>
                        <tr>
                            <th scope="col">"Id"</th>
                            <th scope="col">"Component"</th>
                            <th scope="col">"What it is"</th>
                            <th scope="col">"Always on"</th>
                        </tr>
                    </thead>
                    <tbody>
                        {COMPONENTS
                            .iter()
                            .map(|c| {
                                view! {
                                    <tr>
                                        <td><code>{harness::code(*c)}</code></td>
                                        <td>{harness::name(*c)}</td>
                                        <td>{harness::blurb(*c)}</td>
                                        <td>{if harness::always_on(*c) { "yes" } else { "no" }}</td>
                                    </tr>
                                }
                            })
                            .collect_view()}
                    </tbody>
                </table>
                </div>
            </section>

            <section id="faq" aria-labelledby="faq-title" class="card">
                <h2 id="faq-title" class="card-title">"Frequently asked questions"</h2>
                {FAQ
                    .iter()
                    .map(|f| {
                        view! {
                            <details class="faq">
                                <summary>{f.q}</summary>
                                <p>{f.a}</p>
                            </details>
                        }
                    })
                    .collect_view()}
            </section>

            <section id="shortcuts" aria-labelledby="shortcuts-title" class="card">
                <h2 id="shortcuts-title" class="card-title">"Keyboard shortcuts"</h2>
                <div class="table-wrap">
                <table class="table">
                    <thead>
                        <tr>
                            <th scope="col">"Keys"</th>
                            <th scope="col">"Action"</th>
                        </tr>
                    </thead>
                    <tbody>
                        {SHORTCUTS
                            .iter()
                            .map(|s| view! { <tr><td><kbd>{s.keys}</kbd></td><td>{s.action}</td></tr> })
                            .collect_view()}
                    </tbody>
                </table>
                </div>
            </section>
        </div>
    }
}
