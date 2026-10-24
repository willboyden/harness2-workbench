//! Port of upstream `harness2/core/report.py`: judged rows -> the figure each benchmark
//! reports, pick provenance, and upstream's NOTE/WARNING lines.
//!
//! Ungraded rows are counted and excluded from denominators, exactly as upstream does.

use std::collections::{BTreeMap, BTreeSet};

use h2s_api::{JudgedRow, Mode, ModeSummary};

use crate::pyfmt;

/// Upstream `Scored.graded` (`n_criteria > 0`), as recorded on the row; a row without a
/// fraction cannot be averaged and counts as ungraded.
fn graded(r: &JudgedRow) -> bool {
    r.graded && r.frac.is_some()
}

#[derive(PartialEq)]
enum Kind {
    Base,
    Final,
    Candidate,
}

/// Upstream `_kind`: each sequential checkpoint `c1..ck` is a composed final.
fn kind(r: &JudgedRow) -> Kind {
    let lbl = r.label.as_str();
    if lbl.starts_with("base") {
        return Kind::Base;
    }
    let is_final = match r.mode {
        Mode::Parallel => lbl.starts_with("agg"),
        Mode::Sequential => lbl
            .strip_prefix('c')
            .and_then(|n| n.parse::<u32>().ok())
            .is_some_and(|n| (1..=r.k).contains(&n) && lbl == format!("c{n}")),
    };
    if is_final {
        Kind::Final
    } else {
        Kind::Candidate
    }
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// One summary per (mode, k), sorted by (k, mode). `fell_back` and `usd` are keyed by
/// (mode, k) and come from the task summaries.
#[must_use]
pub fn summarise(
    rows: &[JudgedRow],
    fell_back: &BTreeMap<(Mode, u32), u32>,
    usd: &BTreeMap<(Mode, u32), f64>,
) -> Vec<ModeSummary> {
    let keys: BTreeSet<(u32, Mode)> = rows.iter().map(|r| (r.k, r.mode)).collect();
    let mut out = Vec::with_capacity(keys.len());
    for (k, mode) in keys {
        let group: Vec<&JudgedRow> = rows.iter().filter(|r| r.mode == mode && r.k == k).collect();
        let good: Vec<&JudgedRow> = group.iter().copied().filter(|r| graded(r)).collect();
        let metrics: BTreeSet<String> = good
            .iter()
            .filter_map(|r| r.metric.clone())
            .filter(|m| !m.is_empty())
            .collect();
        let n = count(good.len());
        let (score, all_pass) = if good.is_empty() {
            (None, None)
        } else {
            let fracs: Vec<f64> = good.iter().filter_map(|r| r.frac).collect();
            let mean = pyfmt::mean(&fracs).unwrap_or(0.0);
            let passed = good.iter().filter(|r| r.all_pass).count();
            (
                Some(pyfmt::round(100.0 * mean, 2)),
                Some(pyfmt::round(
                    f64::from(100 * count(passed)) / f64::from(n),
                    2,
                )),
            )
        };
        out.push(ModeSummary {
            mode,
            k,
            n,
            n_ungraded: count(group.len() - good.len()),
            score,
            all_pass,
            kept_base: count(good.iter().filter(|r| r.chosen_is_base).count()),
            chose_final: count(good.iter().filter(|r| kind(r) == Kind::Final).count()),
            chose_candidate: count(good.iter().filter(|r| kind(r) == Kind::Candidate).count()),
            fell_back: fell_back.get(&(mode, k)).copied().unwrap_or(0),
            usd: pyfmt::round(usd.get(&(mode, k)).copied().unwrap_or(0.0), 2),
            metrics: metrics.into_iter().collect(),
        });
    }
    out
}

/// `Summary.name`: `parallel k=1`.
fn name(s: &ModeSummary) -> String {
    format!("{} k={}", s.mode.as_cli(), s.k)
}

/// `Summary.line(bench)`.
fn line(s: &ModeSummary, bench: &str) -> String {
    if s.n == 0 {
        return format!("  {}: no graded task", name(s));
    }
    let score = s.score.unwrap_or(0.0);
    let figures = if bench == "lab" {
        format!(
            "partial {score:6.2}% | pass {:5.1}%",
            s.all_pass.unwrap_or(0.0)
        )
    } else {
        format!("score {score:6.2}%")
    };
    let pick = format!(
        " | picked output: final {} / candidate {} / base {}",
        s.chose_final, s.chose_candidate, s.kept_base
    );
    let fb = if s.fell_back > 0 {
        format!(" | fell back {}", s.fell_back)
    } else {
        String::new()
    };
    let ug = if s.n_ungraded > 0 {
        format!(" | ungraded {}", s.n_ungraded)
    } else {
        String::new()
    };
    format!("  {}: n={:>3} | {figures}{pick}{fb}{ug}", name(s), s.n)
}

/// Upstream `render()`: the printable block (`title`, rule, one line per summary, then the
/// k=1 noise-floor NOTE and the WARNINGs). Returns `(text, notes)` where `notes` holds the
/// NOTE/WARNING paragraphs on their own. LAB lines show partial credit and all-pass; the
/// others show one score.
///
/// Lines are ordered by (k, mode) and the NOTE/WARNING paragraphs by (mode, k), which is
/// the order `harness2-report` prints them in (its summaries arrive sorted by (mode, k)).
/// Each entry of `notes` is one paragraph with its indentation removed and its lines
/// joined by a space.
#[must_use]
pub fn render(summaries: &[ModeSummary], title: &str, bench_name: &str) -> (String, Vec<String>) {
    let mut lines: Vec<String> = Vec::new();
    let mut paragraphs: Vec<Vec<String>> = Vec::new();
    if !title.is_empty() {
        lines.push(title.to_owned());
        lines.push("=".repeat(title.chars().count().max(40)));
    }
    let mut by_k: Vec<&ModeSummary> = summaries.iter().collect();
    by_k.sort_by_key(|s| (s.k, s.mode));
    for s in &by_k {
        lines.push(line(s, bench_name));
    }
    let mut by_mode: Vec<&ModeSummary> = summaries.iter().collect();
    by_mode.sort_by_key(|s| (s.mode, s.k));

    let mut at1: BTreeMap<Mode, f64> = BTreeMap::new();
    for s in &by_mode {
        if s.k == 1
            && let Some(score) = s.score
        {
            at1.insert(s.mode, score);
        }
    }
    if let (Some(agg), Some(seq)) = (at1.get(&Mode::Parallel), at1.get(&Mode::Sequential)) {
        let gap = (agg - seq).abs();
        paragraphs.push(vec![
            "  NOTE  at k=1 the parallel and sequential modes are the same procedure (one \
             INVARIANT draw, one DOMAIN draw, one composition)."
                .to_owned(),
            format!(
                "        The {gap:.2} pt between them is therefore a NOISE FLOOR, not a result \
                 -- an effect at k>=2 has to clear it."
            ),
        ]);
    }
    for s in &by_mode {
        if s.n > 0 && f64::from(s.chose_final) <= 0.25 * f64::from(s.n) {
            paragraphs.push(vec![format!(
                "  WARNING  {}: the mode's own final was chosen on only {}/{} tasks. These \
                 records include outputs outside the published pool of {} composed finals.",
                name(s),
                s.chose_final,
                s.n,
                s.k
            )]);
        }
    }
    for s in &by_mode {
        if s.n > 0 && f64::from(s.kept_base) >= 0.8 * f64::from(s.n) {
            paragraphs.push(vec![format!(
                "  WARNING  {}: selection chose a reference output on {}/{} tasks, outside the \
                 published pool of composed outputs.",
                name(s),
                s.kept_base,
                s.n
            )]);
        }
    }
    for s in &by_mode {
        if s.fell_back > 0 {
            paragraphs.push(vec![format!(
                "  WARNING  {}: {} stage(s) fell back to the inherited harness. Inspect the stage \
                 errors when interpreting these results.",
                name(s),
                s.fell_back
            )]);
        }
    }
    for s in &by_mode {
        if s.metrics.len() > 1 {
            paragraphs.push(vec![format!(
                "  WARNING  {}: graded rows carry MIXED scoring units ({}). The score averages \
                 weighted points with unweighted criteria counts (about 20 pt apart on JobBench); \
                 re-judge the fallback rows before comparing this line with any published number.",
                name(s),
                s.metrics.join(", ")
            )]);
        }
    }
    let mut notes = Vec::with_capacity(paragraphs.len());
    for p in paragraphs {
        lines.push(String::new());
        notes.push(p.iter().map(|l| l.trim()).collect::<Vec<_>>().join(" "));
        lines.extend(p);
    }
    (lines.join("\n"), notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(task: &str, mode: Mode, k: u32, label: &str, frac: Option<f64>, all: bool) -> JudgedRow {
        JudgedRow {
            task_id: task.into(),
            mode,
            k,
            label: label.into(),
            run_id: format!("ns/{task}/{label}"),
            chosen_is_base: label.starts_with("base"),
            n_passed: None,
            n_criteria: None,
            all_pass: all,
            frac,
            graded: frac.is_some(),
            error: String::new(),
            metric: None,
            points_passed: None,
            points_max: None,
        }
    }

    #[test]
    fn summary_figures_and_lines() {
        let rows = vec![
            row("a", Mode::Parallel, 1, "agg1", Some(0.5), false),
            row("b", Mode::Parallel, 1, "agg1", Some(1.0), true),
            row("c", Mode::Parallel, 1, "agg1", None, false),
            row("a", Mode::Sequential, 1, "c1", Some(0.25), false),
            row("b", Mode::Sequential, 1, "inv1", Some(0.75), true),
        ];
        let fb = BTreeMap::from([((Mode::Sequential, 1), 2)]);
        let usd = BTreeMap::from([((Mode::Parallel, 1), 1.005)]);
        let s = summarise(&rows, &fb, &usd);
        assert_eq!(s.len(), 2);
        assert_eq!((s[0].mode, s[0].n, s[0].n_ungraded), (Mode::Parallel, 2, 1));
        assert_eq!((s[0].score, s[0].all_pass), (Some(75.0), Some(50.0)));
        assert_eq!(s[0].usd, 1.0);
        assert_eq!(
            (s[1].chose_final, s[1].chose_candidate, s[1].fell_back),
            (1, 1, 2)
        );
        let (text, notes) = render(&s, "lab-oc modes=parallel,sequential", "lab");
        let expected = "lab-oc modes=parallel,sequential\n\
                        ========================================\n  \
                        parallel k=1: n=  2 | partial  75.00% | pass  50.0% | picked output: final 2 / candidate 0 / base 0 | ungraded 1\n  \
                        sequential k=1: n=  2 | partial  50.00% | pass  50.0% | picked output: final 1 / candidate 1 / base 0 | fell back 2\n\
                        \n  \
                        NOTE  at k=1 the parallel and sequential modes are the same procedure (one INVARIANT draw, one DOMAIN draw, one composition).\n        \
                        The 25.00 pt between them is therefore a NOISE FLOOR, not a result -- an effect at k>=2 has to clear it.\n\
                        \n  \
                        WARNING  sequential k=1: 2 stage(s) fell back to the inherited harness. Inspect the stage errors when interpreting these results.";
        assert_eq!(text, expected);
        assert_eq!(notes.len(), 2);
        assert!(notes[0].starts_with("NOTE  at k=1"));
        assert!(notes[0].contains("composition). The 25.00 pt"));
    }

    #[test]
    fn empty_and_non_lab_lines() {
        let rows = vec![row("a", Mode::Parallel, 2, "base1", None, false)];
        let s = summarise(&rows, &BTreeMap::new(), &BTreeMap::new());
        assert_eq!(s[0].n, 0);
        let (text, notes) = render(&s, "", "jb");
        assert_eq!(text, "  parallel k=2: no graded task");
        assert!(notes.is_empty());
        let rows = vec![row("a", Mode::Parallel, 2, "base1", Some(0.5), false)];
        let s = summarise(&rows, &BTreeMap::new(), &BTreeMap::new());
        let (text, notes) = render(&s, "", "jb");
        assert!(text.starts_with(
            "  parallel k=2: n=  1 | score  50.00% | picked output: final 0 / candidate 0 / base 1"
        ));
        assert_eq!(notes.len(), 2);
    }
}
