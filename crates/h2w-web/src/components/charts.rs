//! Hand-written SVG charts. Labels are HTML next to SVG bars, so text stays legible at any
//! width; every chart has an accessible name and a text equivalent (a table or list).

use h2w_api::ModeSummary;
use leptos::prelude::*;

use crate::logic::format;
use crate::logic::report::{DeltaBar, provenance, summary_label};

fn pct_attr(v: f64) -> String {
    format!("{:.2}", v.clamp(0.0, 100.0))
}

/// Score and all-pass bars per (mode, k), with the base score as a dashed marker.
#[component]
pub(crate) fn ScoreBars(summaries: Vec<ModeSummary>, base_score: Option<f64>) -> impl IntoView {
    let description = summaries
        .iter()
        .map(|s| {
            format!(
                "{}: score {}, all-pass {}",
                summary_label(s),
                format::opt_pct(s.score),
                format::opt_pct(s.all_pass)
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    let label = match base_score {
        Some(b) => format!(
            "Scores by mode. {description}. Base executions: {}.",
            format::pct(b)
        ),
        None => format!("Scores by mode. {description}."),
    };
    view! {
        <figure class="score-bars" role="group" aria-label=label>
            <div class="legend" aria-hidden="true">
                <span><i class="swatch score"></i>"Mean partial credit"</span>
                <span><i class="swatch allpass"></i>"All-pass rate"</span>
                {base_score.map(|b| view! { <span><i class="swatch base"></i>{format!("Base executions ({})", format::pct(b))}</span> })}
            </div>
            {summaries
                .into_iter()
                .map(|s| {
                    let score = s.score.unwrap_or(0.0);
                    let all = s.all_pass.unwrap_or(0.0);
                    view! {
                        <div class="bar-row">
                            <span class="bar-label">{summary_label(&s)}<span class="muted small">{format!(" ({} graded)", s.n)}</span></span>
                            <div class="bar-pair">
                                <div class="bar-line">
                                    <svg class="bar-svg" viewBox="0 0 100 10" preserveAspectRatio="none" aria-hidden="true">
                                        <rect class="bar-track" x="0" y="0" width="100" height="10"></rect>
                                        <rect class="bar-score" x="0" y="0" width=pct_attr(score) height="10"></rect>
                                        {base_score.map(|b| view! { <line class="base-line" x1=pct_attr(b) x2=pct_attr(b) y1="0" y2="10"></line> })}
                                    </svg>
                                    <span class="bar-value">{format::opt_pct(s.score)}</span>
                                </div>
                                <div class="bar-line">
                                    <svg class="bar-svg" viewBox="0 0 100 10" preserveAspectRatio="none" aria-hidden="true">
                                        <rect class="bar-track" x="0" y="0" width="100" height="10"></rect>
                                        <rect class="bar-allpass" x="0" y="0" width=pct_attr(all) height="10"></rect>
                                    </svg>
                                    <span class="bar-value">{format::opt_pct(s.all_pass)}</span>
                                </div>
                            </div>
                        </div>
                    }
                })
                .collect_view()}
            <div class="bar-axis" aria-hidden="true">
                <span>"0%"</span>
                <span>"50%"</span>
                <span>"100%"</span>
            </div>
        </figure>
    }
}

/// Where adopted harnesses came from: composed finals, single candidates, or the base.
#[component]
pub(crate) fn ProvenanceBar(summary: ModeSummary) -> impl IntoView {
    let parts = provenance(&summary);
    let total: u32 = parts.iter().map(|p| p.1).sum();
    let label = format!(
        "{}: {} final, {} candidate, {} base",
        summary_label(&summary),
        parts[0].1,
        parts[1].1,
        parts[2].1
    );
    let mut x = 0.0_f64;
    let segs: Vec<(String, String, &'static str)> = parts
        .iter()
        .map(|(name, n, _)| {
            let w = if total == 0 {
                0.0
            } else {
                f64::from(*n) / f64::from(total) * 100.0
            };
            let seg = (format!("{x:.2}"), format!("{w:.2}"), *name);
            x += w;
            seg
        })
        .collect();
    view! {
        <svg class="prov-bar" viewBox="0 0 100 10" preserveAspectRatio="none" role="img" aria-label=label>
            <rect class="bar-track" x="0" y="0" width="100" height="10"></rect>
            {segs
                .into_iter()
                .map(|(x, w, name)| {
                    view! { <rect class=format!("seg-{name}") x=x y="0" width=w height="10"></rect> }
                })
                .collect_view()}
        </svg>
    }
}

/// All per-task deltas at a glance, sorted from best to worst (no text inside the SVG).
#[component]
pub(crate) fn DeltaOverview(bars: Vec<DeltaBar>, extent: f64) -> impl IntoView {
    let n = bars.len().max(1) as f64;
    let w = 100.0 / n;
    let wins = bars.iter().filter(|b| b.points > 0.0).count();
    let losses = bars.iter().filter(|b| b.points < 0.0).count();
    let label = format!(
        "Per-task change from A to B, sorted: {wins} tasks improved, {losses} got worse, {} unchanged. Axis from minus {extent} to plus {extent} points.",
        bars.len() - wins - losses
    );
    view! {
        <div class="delta-overview">
            <div class="delta-axis" aria-hidden="true">
                <span>{format!("+{extent:.0} pts")}</span>
                <span>"0"</span>
                <span>{format!("\u{2212}{extent:.0} pts")}</span>
            </div>
            <svg class="delta-svg" viewBox="0 0 100 100" preserveAspectRatio="none" role="img" aria-label=label>
                <line class="gridline" x1="0" x2="100" y1="0.5" y2="0.5"></line>
                <line class="gridline" x1="0" x2="100" y1="99.5" y2="99.5"></line>
                {bars
                    .iter()
                    .enumerate()
                    .map(|(i, b)| {
                        let h = (b.points.abs() / extent * 50.0).min(50.0);
                        let (y, class, h) = if b.points > 0.0 {
                            (50.0 - h, "delta-pos", h)
                        } else if b.points < 0.0 {
                            (50.0, "delta-neg", h)
                        } else {
                            (49.0, "delta-zero", 2.0)
                        };
                        view! {
                            <rect
                                class=class
                                x=format!("{:.3}", i as f64 * w + w * 0.12)
                                y=format!("{y:.3}")
                                width=format!("{:.3}", w * 0.76)
                                height=format!("{h:.3}")
                            ></rect>
                        }
                    })
                    .collect_view()}
                <line class="zero-line" x1="0" x2="100" y1="50" y2="50"></line>
            </svg>
        </div>
    }
}

/// A single diverging bar for one task's delta (used in table rows).
#[component]
pub(crate) fn DeltaBarCell(points: f64, extent: f64) -> impl IntoView {
    let half = (points.abs() / extent * 50.0).min(50.0);
    let (x, class) = if points >= 0.0 {
        (50.0, "delta-pos")
    } else {
        (50.0 - half, "delta-neg")
    };
    view! {
        <svg class="delta-cell" viewBox="0 0 100 10" preserveAspectRatio="none" aria-hidden="true">
            <rect class="bar-track" x="0" y="0" width="100" height="10"></rect>
            <rect class=class x=format!("{x:.2}") y="0" width=format!("{half:.2}") height="10"></rect>
            <line class="zero-line" x1="50" x2="50" y1="0" y2="10"></line>
        </svg>
    }
}
