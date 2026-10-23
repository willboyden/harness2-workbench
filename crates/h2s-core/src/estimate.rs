//! Size and cost of a sweep before running it.
//!
//! Counts follow upstream's budget identity (`core/ledger.py`): per task, k base rollouts
//! shared by the modes, then per mode 3k more rollouts (2k candidates + k finals) and 3k
//! improver sessions; plus, per mode, one selector session when k > 1 and one reflection
//! session when an adoption is reflected on.

use h2s_api::{CostEstimate, Mode, Prices};

/// Measured spend from earlier runs of the same profile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct History {
    /// Mean recorded USD per (task, mode) — improver, selection and reflection spend
    /// (harness2 does not record executor rollout spend in summaries).
    pub usd_per_task_mode: f64,
    /// How many (task, mode) records the mean is over.
    pub samples: u32,
}

/// Below this many samples the history range is widened.
const FEW_SAMPLES: u32 = 10;

fn prices_ok(p: &Prices) -> bool {
    let all = [
        p.rollout_usd_low,
        p.rollout_usd_high,
        p.session_usd_low,
        p.session_usd_high,
    ];
    all.iter().all(|v| v.is_finite() && *v >= 0.0)
        && p.rollout_usd_low <= p.rollout_usd_high
        && p.session_usd_low <= p.session_usd_high
}

fn money(v: f64) -> String {
    format!("${v:.2}")
}

/// Estimate a sweep of `tasks` tasks. With `history`, the USD range comes from it
/// (basis `history`); else from `prices` (basis `heuristic`); else no USD figures.
///
/// A history with no samples or a non-finite mean is ignored, and so are prices that are
/// negative, non-finite or have low > high (a note says so).
#[must_use]
pub fn estimate(
    tasks: u32,
    k: u32,
    modes: &[Mode],
    prices: Option<&Prices>,
    history: Option<History>,
) -> CostEstimate {
    let n_modes = Mode::ALL.iter().filter(|m| modes.contains(m)).count();
    let m = u64::try_from(n_modes).unwrap_or(0);
    let t = u64::from(tasks);
    let k64 = u64::from(k);
    let rollouts = if m == 0 {
        0
    } else {
        t.saturating_mul(k64.saturating_add(m.saturating_mul(3).saturating_mul(k64)))
    };
    let per_task_mode = if k == 0 {
        0
    } else {
        3 * k64 + u64::from(k > 1) + 1
    };
    let sessions = t.saturating_mul(m).saturating_mul(per_task_mode);

    let mut notes = Vec::new();
    if rollouts == 0 {
        notes.push("Nothing to run: no tasks, no recursion budget or no mode selected.".to_owned());
    }
    notes.push(format!(
        "Per task: {k} reference run(s) shared by the modes; per mode {} candidate and final runs \
         and {} improver sessions.",
        3 * k64,
        3 * k64
    ));
    if k > 1 {
        notes.push("Each task and mode adds one selector session (k > 1).".to_owned());
    } else {
        notes.push("At k = 1 the single final is adopted without a selector session.".to_owned());
    }
    notes.push(
        "A reflection session follows each adoption; it is skipped when selection fails."
            .to_owned(),
    );
    notes.push(
        "Judging is not included: harness2-judge makes one judge call per selected run (and per \
         reference run with --bases)."
            .to_owned(),
    );
    notes.push(
        "Retries are not included: a rejected proposal gets one retry and a failed improver call \
         one more attempt, each a further session."
            .to_owned(),
    );
    notes.push(
        "A resumed sweep reuses finished rollouts and cached sessions, which cost nothing."
            .to_owned(),
    );

    let rollouts_f = rollouts as f64;
    let sessions_f = sessions as f64;
    let mut basis = "heuristic".to_owned();
    let mut usd_low = None;
    let mut usd_high = None;
    let usable_history = history
        .filter(|h| h.samples > 0 && h.usd_per_task_mode.is_finite() && h.usd_per_task_mode >= 0.0);
    if let Some(h) = usable_history {
        let point = h.usd_per_task_mode * (t * m) as f64;
        let (lo, hi) = if h.samples >= FEW_SAMPLES {
            (0.8, 1.25)
        } else {
            (0.5, 2.0)
        };
        usd_low = Some(point * lo);
        usd_high = Some(point * hi);
        basis = "history".to_owned();
        notes.push(format!(
            "Measured from {} earlier (task, mode) record(s) of this experiment at {} per task and \
             mode; the range is {}% to {}% of that.",
            h.samples,
            money(h.usd_per_task_mode),
            (lo * 100.0).round(),
            (hi * 100.0).round()
        ));
        if h.samples < FEW_SAMPLES {
            notes.push("Few samples, so the range is wide.".to_owned());
        }
        notes.push(
            "Spend scales with k: the history is representative only for sweeps with the same k."
                .to_owned(),
        );
        notes.push(
            "harness2 records improver spend only (editing, composing, selection, reflection); \
             executor rollout spend is not recorded and not included."
                .to_owned(),
        );
        if let Some(p) = prices.filter(|p| prices_ok(p)) {
            notes.push(format!(
                "Executor rollouts at the profile's prices would add {} to {}.",
                money(rollouts_f * p.rollout_usd_low),
                money(rollouts_f * p.rollout_usd_high)
            ));
        }
    } else if let Some(p) = prices {
        if prices_ok(p) {
            usd_low = Some(rollouts_f * p.rollout_usd_low + sessions_f * p.session_usd_low);
            usd_high = Some(rollouts_f * p.rollout_usd_high + sessions_f * p.session_usd_high);
            notes.push(
                "From the profile's price assumptions per executor rollout and improver session."
                    .to_owned(),
            );
        } else {
            notes.push(
                "The profile's prices are invalid (negative, not finite, or low above high): no \
                 cost figure."
                    .to_owned(),
            );
        }
    } else {
        notes.push(
            "No prices in the profile and no earlier runs in this experiment: no cost figure."
                .to_owned(),
        );
    }
    CostEstimate {
        tasks,
        modes: u32::try_from(n_modes).unwrap_or(u32::MAX),
        k,
        rollouts,
        improver_sessions: sessions,
        usd_low,
        usd_high,
        basis,
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_follow_the_budget_identity() {
        // One mode, k = 1: 4 rollouts, 3 + 0 + 1 sessions per task.
        let e = estimate(10, 1, &[Mode::Parallel], None, None);
        assert_eq!((e.rollouts, e.improver_sessions), (40, 40));
        assert_eq!(
            (e.usd_low, e.usd_high, e.basis.as_str()),
            (None, None, "heuristic")
        );
        // Two modes, k = 2: 2 shared + 2 * 6 = 14 rollouts; 2 * (6 + 1 + 1) = 16 sessions.
        let e = estimate(1, 2, &[Mode::Sequential, Mode::Parallel], None, None);
        assert_eq!((e.rollouts, e.improver_sessions, e.modes), (14, 16, 2));
        let dup = estimate(1, 2, &[Mode::Parallel, Mode::Parallel], None, None);
        assert_eq!((dup.rollouts, dup.modes), (8, 1));
        let none = estimate(5, 1, &[], None, None);
        assert_eq!((none.rollouts, none.improver_sessions), (0, 0));
        assert!(none.notes[0].starts_with("Nothing to run"));
        let huge = estimate(
            u32::MAX,
            u32::MAX,
            &[Mode::Parallel, Mode::Sequential],
            None,
            None,
        );
        assert!(huge.rollouts > 0);
    }

    #[test]
    fn usd_from_prices_or_history() {
        let p = Prices {
            rollout_usd_low: 0.1,
            rollout_usd_high: 0.2,
            session_usd_low: 1.0,
            session_usd_high: 2.0,
        };
        let e = estimate(2, 1, &[Mode::Parallel], Some(&p), None);
        assert_eq!(e.usd_low, Some(8.0 * 0.1 + 8.0 * 1.0));
        assert_eq!(e.usd_high, Some(8.0 * 0.2 + 8.0 * 2.0));
        let h = History {
            usd_per_task_mode: 3.0,
            samples: 20,
        };
        let e = estimate(2, 1, &[Mode::Parallel], Some(&p), Some(h));
        assert_eq!(e.basis, "history");
        assert_eq!((e.usd_low, e.usd_high), (Some(6.0 * 0.8), Some(7.5)));
        assert!(
            e.notes
                .iter()
                .any(|n| n.contains("executor rollout spend is not recorded"))
        );
        assert!(
            e.notes
                .iter()
                .any(|n| n.contains("would add $0.80 to $1.60"))
        );
        let bad = Prices {
            rollout_usd_low: -1.0,
            ..p
        };
        let e = estimate(2, 1, &[Mode::Parallel], Some(&bad), None);
        assert_eq!(e.usd_low, None);
        let empty = History {
            usd_per_task_mode: 1.0,
            samples: 0,
        };
        assert_eq!(
            estimate(2, 1, &[Mode::Parallel], None, Some(empty)).basis,
            "heuristic"
        );
    }
}
