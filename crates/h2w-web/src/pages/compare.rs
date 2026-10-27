//! Compare two results task by task, with a sign test: two (experiment, sweep, mode) results,
//! or a mode against the sweep's starting harness (`base`), with the starting harness's measured
//! run-to-run variation when the sweeps hold it.
//!
//! `Comparison` values are fractions 0..1 (contract addendum v1.1); they are shown as
//! percentages and percentage points.

use h2w_api::{CompareSide, Comparison, Experiment, Mode};
use leptos::prelude::*;

use crate::api;
use crate::components::charts::{DeltaBarCell, DeltaOverview};
use crate::components::form::SelectField;
use crate::components::ui::{
    Card, EmptyState, ErrorPanel, Notice, PageHeader, Skeleton, Stat, Term,
};
use crate::components::widgets::mode_label;
use crate::logic::format;
use crate::logic::jobs::Tone;
use crate::logic::report::{delta_bars, delta_extent, noise_reading, sign_test_reading};
use crate::logic::urls::{SweepRef, api as paths, route};
use crate::nav::{query_param, query_setter};

fn frac(v: Option<f64>) -> String {
    v.map_or_else(|| format::DASH.to_owned(), format::frac_pct)
}

fn side_label(side: CompareSide) -> &'static str {
    match side {
        CompareSide::Mode(m) => mode_label(m),
        CompareSide::Base => "starting harness",
    }
}

#[component]
pub(crate) fn Compare() -> impl IntoView {
    let a_q = query_param("a");
    let b_q = query_param("b");
    let set_query = query_setter();
    // `agg`/`seq` links read the same as `parallel`/`sequential` (the pickers use the latter).
    let a = RwSignal::new(SweepRef::normalize_param(&a_q.get_untracked()));
    let b = RwSignal::new(SweepRef::normalize_param(&b_q.get_untracked()));
    let experiments = LocalResource::new(|| async {
        api::get::<Vec<Experiment>>(&paths::experiments(false)).await
    });
    // Every (experiment, sweep, mode) that exists, and the starting harness of every sweep
    // whose reference rollouts were judged, with k for the noise-floor note.
    let choices = Memo::new(move |_| {
        let mut v: Vec<(SweepRef, u32, bool)> = Vec::new();
        if let Some(Ok(list)) = experiments.get() {
            for e in list {
                for s in &e.sweeps {
                    let r = |side| SweepRef {
                        experiment: e.name.clone(),
                        sweep: s.id.clone(),
                        side,
                    };
                    for m in &s.modes {
                        v.push((r(CompareSide::Mode(*m)), s.k, s.headline.is_some()));
                    }
                    if s.base_score.is_some() {
                        v.push((r(CompareSide::Base), s.k, true));
                    }
                }
            }
        }
        v
    });
    let options = Signal::derive(move || {
        let mut v = vec![(String::new(), "Choose a sweep and mode…".to_owned())];
        v.extend(choices.get().into_iter().map(|(r, k, judged)| {
            let label = format!(
                "{} / {} / {} (k={}){}",
                r.experiment,
                r.sweep,
                side_label(r.side),
                k,
                if judged { "" } else { " — not judged" }
            );
            (r.to_param(), label)
        }));
        v
    });
    // Keep the URL in step with the pickers.
    {
        let set_query = set_query.clone();
        Effect::new(move |prev: Option<()>| {
            let (x, y) = (a.get(), b.get());
            if prev.is_some() {
                set_query(&[("a", &x), ("b", &y)]);
            }
        });
    }
    // Only A given: default B to the other mode of the same sweep, or, for the starting
    // harness, to the sweep's first mode (did refinement beat the harness it started from?).
    Effect::new(move |_| {
        if b.get_untracked().is_empty()
            && let Some(ra) = SweepRef::parse(&a.get())
        {
            let others: &[CompareSide] = match ra.side {
                CompareSide::Mode(Mode::Parallel) => &[CompareSide::Mode(Mode::Sequential)],
                CompareSide::Mode(Mode::Sequential) => &[CompareSide::Mode(Mode::Parallel)],
                CompareSide::Base => &[
                    CompareSide::Mode(Mode::Parallel),
                    CompareSide::Mode(Mode::Sequential),
                ],
            };
            let found = others.iter().find_map(|&side| {
                let candidate = SweepRef { side, ..ra.clone() };
                choices
                    .with(|c| c.iter().any(|(r, _, _)| *r == candidate))
                    .then_some(candidate)
            });
            if let Some(candidate) = found {
                b.set(candidate.to_param());
            }
        }
    });
    let comparison = LocalResource::new(move || {
        let (x, y) = (SweepRef::parse(&a.get()), SweepRef::parse(&b.get()));
        async move {
            match (x, y) {
                (Some(x), Some(y)) => api::get::<Comparison>(&paths::compare(&x, &y))
                    .await
                    .map(Some),
                _ => Ok(None),
            }
        }
    });
    let k_of = move |param: &str| {
        let r = SweepRef::parse(param)?;
        choices.with(|c| c.iter().find(|(x, _, _)| *x == r).map(|(_, k, _)| *k))
    };
    view! {
        <div class="page">
            <PageHeader
                title="Compare"
                subtitle="Two sweeps, the two modes of one sweep, or a mode against the harness it started from, task by task: who won, by how much, and whether it is more than chance."
            />
            <div class="compare-pickers">
                <div class="compare-side">
                    <h3>"A (the reference)"</h3>
                    <SelectField id="compare-a" label="Sweep and mode" value=a options=options />
                </div>
                <div class="compare-side">
                    <h3>"B (the challenger)"</h3>
                    <SelectField id="compare-b" label="Sweep and mode" value=b options=options />
                </div>
            </div>
            <div>
                <button
                    type="button"
                    class="btn btn-secondary btn-sm"
                    on:click=move |_| {
                        let (x, y) = (a.get_untracked(), b.get_untracked());
                        a.set(y);
                        b.set(x);
                    }
                >
                    "Swap A and B"
                </button>
            </div>
            {move || match comparison.get() {
                None => view! { <Skeleton lines=6 /> }.into_any(),
                Some(Err(e)) => view! { <ErrorPanel error=e /> }.into_any(),
                Some(Ok(None)) => view! {
                    <EmptyState
                        title="Pick two results"
                        body="Choose a sweep and mode for A and for B. A good start: a sweep's starting harness as A and its parallel mode as B, which asks whether refinement beat the harness it started from (the starting harness is offered once its reference rollouts are judged)."
                    />
                }
                .into_any(),
                Some(Ok(Some(c))) => {
                    let bars = delta_bars(&c.tasks);
                    let extent = delta_extent(&bars);
                    let reading = sign_test_reading(&c);
                    let noise = noise_reading(&c);
                    let low_k = noise.is_none()
                        && [k_of(&a.get_untracked()), k_of(&b.get_untracked())].contains(&Some(1));
                    let mut rows = c.tasks.clone();
                    rows.sort_by(|x, y| {
                        y.delta
                            .unwrap_or(f64::NEG_INFINITY)
                            .partial_cmp(&x.delta.unwrap_or(f64::NEG_INFINITY))
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                    let (a_href, b_href) = (
                        SweepRef::parse(&c.a).map(|r| route::sweep(&r.experiment, &r.sweep)),
                        SweepRef::parse(&c.b).map(|r| route::sweep(&r.experiment, &r.sweep)),
                    );
                    view! {
                        <div class="stats">
                            <Stat label="Mean score A" value=frac(c.mean_a) hint=c.a.clone() />
                            <Stat label="Mean score B" value=frac(c.mean_b) hint=c.b.clone() />
                            <Stat
                                label="B − A"
                                value=c.mean_delta.map_or_else(|| format::DASH.to_owned(), |d| format::signed_points(d * 100.0))
                                hint="mean over tasks graded on both sides"
                            />
                            <Stat
                                label="Wins / losses / ties"
                                value=format!("{} / {} / {}", c.wins, c.losses, c.ties)
                                hint=format!("{} tasks in common", c.n_common)
                            />
                            <Stat
                                label="Sign test"
                                value=c.sign_test_p.map_or_else(|| format::DASH.to_owned(), format::p_value)
                                hint="two-sided, ties dropped"
                            />
                            {c.noise.as_ref().map(|n| view! {
                                <Stat
                                    label="Noise floor"
                                    value=format::points(n.mean_abs * 100.0)
                                    hint="the starting harness, run to run, per task"
                                />
                            })}
                        </div>
                        <Card title="What this means">
                            <p class="reading">{reading}</p>
                            {noise.map(|text| view! {
                                <p class="reading">
                                    {text}" "<Term id="noise-floor">"About the noise floor"</Term>
                                </p>
                            })}
                            {low_k
                                .then(|| {
                                    view! {
                                        <Notice tone=Tone::Info>
                                            <p>
                                                "At least one side ran with k = 1, which has a "
                                                <Term id="noise-floor">"noise floor"</Term>
                                                ": a few points of difference can be run-to-run variation."
                                            </p>
                                        </Notice>
                                    }
                                })}
                            <p class="small muted">
                                "Per-task scores are mean partial credit; "<Term id="ungraded">"ungraded"</Term>
                                " tasks on either side are left out of the means and the test. "
                                <Term id="sign-test">"About the sign test"</Term>
                            </p>
                            <p class="small">
                                {a_href.map(|h| view! { <a href=h>"Open sweep A"</a> })}
                                " · "
                                {b_href.map(|h| view! { <a href=h>"Open sweep B"</a> })}
                            </p>
                        </Card>
                        <Card title="Per-task change (B − A)">
                            {if bars.is_empty() {
                                view! { <p class="muted">"No task was graded on both sides."</p> }.into_any()
                            } else {
                                view! {
                                    <DeltaOverview bars=bars.clone() extent=extent />
                                    <div class="legend" aria-hidden="true">
                                        <span><i class="swatch pos"></i>"B better"</span>
                                        <span><i class="swatch neg"></i>"A better"</span>
                                        <span><i class="swatch neutral"></i>"tie"</span>
                                    </div>
                                }
                                .into_any()
                            }}
                            <div class="table-wrap">
                                <table class="table">
                                    <thead>
                                        <tr>
                                            <th scope="col">"Task"</th>
                                            <th scope="col" class="num">"A"</th>
                                            <th scope="col" class="num">"B"</th>
                                            <th scope="col" class="num">"B − A"</th>
                                            <th scope="col"><span class="visually-hidden">"Change chart"</span></th>
                                        </tr>
                                    </thead>
                                    <tbody>
                                        {rows
                                            .into_iter()
                                            .map(|t| {
                                                view! {
                                                    <tr>
                                                        <td>
                                                            <span class="cell-title mono">{t.task_id.clone()}</span>
                                                            <span class="cell-sub">{t.domain.clone()}</span>
                                                        </td>
                                                        <td class="num">{frac(t.a)}</td>
                                                        <td class="num">{frac(t.b)}</td>
                                                        <td class="num">{t.delta.map_or_else(|| format::DASH.to_owned(), |d| format::signed_points(d * 100.0))}</td>
                                                        <td>{t.delta.map(|d| view! { <DeltaBarCell points=d * 100.0 extent=extent /> })}</td>
                                                    </tr>
                                                }
                                            })
                                            .collect_view()}
                                    </tbody>
                                </table>
                            </div>
                        </Card>
                    }
                    .into_any()
                }
            }}
        </div>
    }
}
