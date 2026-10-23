//! Port of upstream `harness2/core/report.py`: judged rows -> the figure each benchmark
//! reports, pick provenance, and upstream's NOTE/WARNING lines.
//!
//! Ungraded rows are counted and excluded from denominators, exactly as upstream does.

use std::collections::BTreeMap;

use h2s_api::{JudgedRow, Mode, ModeSummary};

/// One summary per (mode, k), sorted by (k, mode). `fell_back` and `usd` are keyed by
/// (mode, k) and come from the task summaries.
#[must_use]
pub fn summarise(
    rows: &[JudgedRow],
    fell_back: &BTreeMap<(Mode, u32), u32>,
    usd: &BTreeMap<(Mode, u32), f64>,
) -> Vec<ModeSummary> {
    let _ = (rows, fell_back, usd);
    Vec::new()
}

/// Upstream `render()`: the printable block (`title`, rule, one line per summary, then the
/// k=1 noise-floor NOTE and the WARNINGs). Returns `(text, notes)` where `notes` holds the
/// NOTE/WARNING paragraphs on their own. LAB lines show partial credit and all-pass; the
/// others show one score.
#[must_use]
pub fn render(summaries: &[ModeSummary], title: &str, bench_name: &str) -> (String, Vec<String>) {
    let _ = (summaries, title, bench_name);
    (String::new(), Vec::new())
}
