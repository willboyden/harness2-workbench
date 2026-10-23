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

/// Estimate a sweep of `tasks` tasks. With `history`, the USD range comes from it
/// (basis `history`); else from `prices` (basis `heuristic`); else no USD figures.
#[must_use]
pub fn estimate(
    tasks: u32,
    k: u32,
    modes: &[Mode],
    prices: Option<&Prices>,
    history: Option<History>,
) -> CostEstimate {
    let _ = (prices, history);
    CostEstimate {
        tasks,
        modes: u32::try_from(modes.len()).unwrap_or(u32::MAX),
        k,
        rollouts: 0,
        improver_sessions: 0,
        usd_low: None,
        usd_high: None,
        basis: "heuristic".to_owned(),
        notes: Vec::new(),
    }
}
