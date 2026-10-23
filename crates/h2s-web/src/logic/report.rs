//! Score reports and sweep comparisons: what to draw and how to explain it.

use h2s_api::{Comparison, ModeSummary, TaskDelta};

use super::format;

/// How an upstream report note should be shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoteLevel {
    Note,
    Warning,
    Other,
}

pub(crate) fn note_level(note: &str) -> NoteLevel {
    let t = note.trim_start();
    if t.starts_with("WARNING") || t.starts_with("WARN") {
        NoteLevel::Warning
    } else if t.starts_with("NOTE") {
        NoteLevel::Note
    } else {
        NoteLevel::Other
    }
}

/// Where a mode's adopted harnesses came from, as `(label, count, explanation)`.
pub(crate) fn provenance(s: &ModeSummary) -> [(&'static str, u32, &'static str); 3] {
    [
        (
            "final",
            s.chose_final,
            "The selector adopted a composed final harness.",
        ),
        (
            "candidate",
            s.chose_candidate,
            "The selector preferred a single probed candidate over the compositions.",
        ),
        (
            "base",
            s.kept_base,
            "No edit beat the unmodified harness, so the base was kept.",
        ),
    ]
}

/// Display name for a summary row: `parallel · k=3`.
pub(crate) fn summary_label(s: &ModeSummary) -> String {
    format!("{} · k={}", s.mode.as_cli(), s.k)
}

/// Plain-language reading of a sign test for newcomers.
pub(crate) fn sign_test_reading(c: &Comparison) -> String {
    let decisive = c.wins + c.losses;
    let Some(p) = c.sign_test_p else {
        return "No task was graded on both sides, so the two cannot be compared yet. Judge \
                both sweeps first."
            .to_owned();
    };
    let direction = if c.wins > c.losses {
        "B did better than A"
    } else if c.losses > c.wins {
        "A did better than B"
    } else {
        "neither side did better"
    };
    let small = if decisive < 6 {
        " With fewer than 6 decisive tasks (ties are dropped), the sign test can never reach \
         p < 0.05, so run more tasks before drawing conclusions."
    } else {
        ""
    };
    let strength = if c.wins == c.losses {
        "There is no evidence of a difference."
    } else if p < 0.01 {
        "That split would be very unlikely if the two were equally good: strong evidence of a \
         real difference."
    } else if p < 0.05 {
        "That split would be unlikely if the two were equally good: moderate evidence of a \
         real difference."
    } else if p < 0.10 {
        "That split could happen by chance fairly often: weak evidence at best."
    } else {
        "That split happens by chance often when two harnesses are equally good, so it is not \
         evidence of a difference."
    };
    format!(
        "On {} graded on both sides, {direction}: {} wins, {} losses and {} ties ({}). \
         {strength}{small}",
        format::plural(u64::from(c.n_common), "task", "tasks"),
        c.wins,
        c.losses,
        c.ties,
        format::p_value(p),
    )
}

/// One bar of the per-task delta chart.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DeltaBar {
    pub(crate) task_id: String,
    pub(crate) domain: String,
    /// `b - a` in percentage points (the API carries fractions 0..1).
    pub(crate) points: f64,
}

/// Tasks graded on both sides, as percentage-point deltas sorted from best to worst.
pub(crate) fn delta_bars(tasks: &[TaskDelta]) -> Vec<DeltaBar> {
    let mut v: Vec<DeltaBar> = tasks
        .iter()
        .filter_map(|t| {
            t.delta.map(|d| DeltaBar {
                task_id: t.task_id.clone(),
                domain: t.domain.clone(),
                points: d * 100.0,
            })
        })
        .collect();
    v.sort_by(|a, b| {
        b.points
            .partial_cmp(&a.points)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.task_id.cmp(&b.task_id))
    });
    v
}

/// Symmetric axis extent for a delta chart, in points: at least 10, rounded up to 10.
pub(crate) fn delta_extent(bars: &[DeltaBar]) -> f64 {
    let max = bars.iter().map(|b| b.points.abs()).fold(0.0_f64, f64::max);
    ((max / 10.0).ceil() * 10.0).max(10.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use h2s_api::Mode;

    fn cmp(wins: u32, losses: u32, ties: u32, p: Option<f64>) -> Comparison {
        Comparison {
            a: "e/s/parallel".into(),
            b: "e/s/sequential".into(),
            tasks: vec![],
            n_common: wins + losses + ties,
            mean_a: None,
            mean_b: None,
            mean_delta: None,
            wins,
            losses,
            ties,
            sign_test_p: p,
        }
    }

    #[test]
    fn notes() {
        assert_eq!(note_level("NOTE: k=1 has a noise floor"), NoteLevel::Note);
        assert_eq!(note_level("WARNING: 3 tasks fell back"), NoteLevel::Warning);
        assert_eq!(note_level("something else"), NoteLevel::Other);
    }

    #[test]
    fn sign_test_readings() {
        let strong = sign_test_reading(&cmp(14, 1, 2, Some(0.001)));
        assert!(strong.contains("B did better than A"), "{strong}");
        assert!(strong.contains("strong evidence"), "{strong}");
        let weak = sign_test_reading(&cmp(3, 1, 0, Some(0.625)));
        assert!(weak.contains("not evidence"), "{weak}");
        assert!(weak.contains("fewer than 6 decisive"), "{weak}");
        let even = sign_test_reading(&cmp(4, 4, 1, Some(1.0)));
        assert!(even.contains("no evidence"), "{even}");
        assert!(sign_test_reading(&cmp(0, 0, 0, None)).contains("Judge both sweeps"));
        let moderate = sign_test_reading(&cmp(2, 9, 0, Some(0.065)));
        assert!(moderate.contains("A did better than B"));
        assert!(moderate.contains("weak evidence"));
    }

    #[test]
    fn delta_bars_sorted_and_scaled() {
        let t = |id: &str, d: Option<f64>| TaskDelta {
            task_id: id.into(),
            domain: "x".into(),
            a: None,
            b: None,
            delta: d,
        };
        let bars = delta_bars(&[t("a", Some(-0.1)), t("b", None), t("c", Some(0.25))]);
        assert_eq!(bars.len(), 2);
        assert_eq!(bars[0].task_id, "c");
        assert!((bars[0].points - 25.0).abs() < 1e-9);
        assert!((delta_extent(&bars) - 30.0).abs() < 1e-9);
        assert!((delta_extent(&[]) - 10.0).abs() < 1e-9);
    }

    #[test]
    fn summary_labels() {
        let s = ModeSummary {
            mode: Mode::Sequential,
            k: 3,
            n: 10,
            n_ungraded: 1,
            score: Some(50.0),
            all_pass: Some(20.0),
            kept_base: 2,
            chose_final: 7,
            chose_candidate: 1,
            fell_back: 0,
            usd: 1.0,
            metrics: vec![],
        };
        assert_eq!(summary_label(&s), "sequential · k=3");
        assert_eq!(provenance(&s)[2].1, 2);
    }
}
