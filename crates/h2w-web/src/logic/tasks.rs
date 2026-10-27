//! The task page's recursion timeline, derived from `TaskDetail`.

use h2w_api::{BaseOutcome, Ledger, Mode, ModeRecord, Selection, Stage};

use super::jobs::Tone;

/// A group of improvement stages shown together on the timeline.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Phase<'a> {
    pub(crate) title: String,
    pub(crate) explanation: &'static str,
    pub(crate) stages: Vec<&'a Stage>,
}

fn label_number(label: &str) -> u32 {
    let digits: String = label.chars().filter(char::is_ascii_digit).collect();
    digits.parse().unwrap_or(0)
}

/// Parallel mode: all general edits, all domain edits, then all compositions. Sequential
/// mode: one phase per step (`inv n`, `dom n`, then its composition `c n`).
pub(crate) fn phases(mode: &ModeRecord) -> Vec<Phase<'_>> {
    let dim = |d: &str| -> Vec<&Stage> { mode.stages.iter().filter(|s| s.dim == d).collect() };
    match mode.mode {
        Mode::Parallel => {
            let mut out = Vec::new();
            let inv = dim("INV");
            if !inv.is_empty() {
                out.push(Phase {
                    title: "Propose & probe: general edits (INV)".to_owned(),
                    explanation: "Each candidate edits the harness in a task-independent way and \
                                  is probed by running it on this task.",
                    stages: inv,
                });
            }
            let dom = dim("DOM");
            if !dom.is_empty() {
                out.push(Phase {
                    title: "Propose & probe: domain edits (DOM)".to_owned(),
                    explanation: "Each candidate specialises the harness for this task's domain \
                                  and is probed the same way.",
                    stages: dom,
                });
            }
            let agg: Vec<&Stage> = mode
                .stages
                .iter()
                .filter(|s| s.dim == "AGG" || s.dim == "COMPOSE")
                .collect();
            if !agg.is_empty() {
                out.push(Phase {
                    title: "Compose finals (AGG)".to_owned(),
                    explanation: "Independent compositions combine what the probes learned into \
                                  refined final harnesses.",
                    stages: agg,
                });
            }
            let known = ["INV", "DOM", "AGG", "COMPOSE"];
            let other: Vec<&Stage> = mode
                .stages
                .iter()
                .filter(|s| !known.contains(&s.dim.as_str()))
                .collect();
            if !other.is_empty() {
                out.push(Phase {
                    title: "Other stages".to_owned(),
                    explanation: "Stages this version of the Workbench does not recognise.",
                    stages: other,
                });
            }
            out
        }
        Mode::Sequential => {
            let mut steps: Vec<u32> = mode.stages.iter().map(|s| label_number(&s.label)).collect();
            steps.sort_unstable();
            steps.dedup();
            steps
                .into_iter()
                .map(|n| Phase {
                    title: format!("Step {n}: propose, probe, compose"),
                    explanation: "One Propose–Probe–Compose step; its composition carries the \
                                  harness forward to the next step.",
                    stages: mode
                        .stages
                        .iter()
                        .filter(|s| label_number(&s.label) == n)
                        .collect(),
                })
                .collect()
        }
    }
}

pub(crate) fn dim_label(dim: &str) -> &str {
    match dim {
        "INV" => "General edit",
        "DOM" => "Domain edit",
        "AGG" | "COMPOSE" => "Composition",
        other => other,
    }
}

/// Badge text and tone summarising a stage's outcome.
pub(crate) fn stage_outcome(s: &Stage) -> (&'static str, Tone) {
    if !s.error.is_empty() {
        ("error", Tone::Danger)
    } else if s.fell_back {
        ("fell back", Tone::Warning)
    } else if s.kept_base {
        ("kept base", Tone::Neutral)
    } else {
        ("accepted", Tone::Success)
    }
}

/// One sentence on how the selector decided.
pub(crate) fn selection_summary(sel: &Selection) -> String {
    if sel.trivial {
        return format!(
            "With k = 1 there was a single final, {}, so it was adopted without a selector \
             session.",
            sel.chosen_label
        );
    }
    let evidence = match sel.evidence.as_str() {
        "deliverable" => "the deliverables each run produced",
        "trajectory" => "the trajectories of each run",
        _ => "the runs",
    };
    let pick = if sel.chosen_is_base {
        format!("kept the base harness ({})", sel.chosen_label)
    } else {
        format!("adopted {}", sel.chosen_label)
    };
    let mut s = format!(
        "The blind selector compared {evidence} for {} anonymised candidates and {pick}.",
        sel.n_candidates
    );
    if sel.fallback {
        s.push_str(" The selector did not produce a usable answer, so a fallback rule picked.");
    }
    s
}

/// Problems with a mode's budget ledger (R = rollouts, P = improver sessions).
/// The line under a task's base score: how many reference rollouts were graded and, from two
/// on, how far they moved (`2 rollouts, spread 4.4 pts`).
pub(crate) fn base_detail(b: &BaseOutcome) -> String {
    let n = super::format::plural(u64::from(b.graded), "rollout", "rollouts");
    match b.spread {
        Some(s) => format!("{n}, spread {}", super::format::points(s * 100.0)),
        None => n,
    }
}

pub(crate) fn ledger_problems(l: &Ledger) -> Vec<String> {
    let mut v = Vec::new();
    if l.r != l.expected_r {
        v.push(format!(
            "Rollouts recorded R = {} but the budget identity expects {}.",
            l.r, l.expected_r
        ));
    }
    if l.p != l.expected_p {
        v.push(format!(
            "Improver sessions recorded P = {} but the budget identity expects {}.",
            l.p, l.expected_p
        ));
    }
    v
}

#[cfg(test)]
mod tests {
    #[test]
    fn base_details() {
        let mut b = BaseOutcome {
            graded: 1,
            score: 0.86,
            spread: None,
            all_pass: false,
        };
        assert_eq!(base_detail(&b), "1 rollout");
        b.graded = 2;
        b.spread = Some(0.044);
        assert_eq!(base_detail(&b), "2 rollouts, spread 4.4 pts");
    }

    use super::*;
    use h2w_api::AnonId;

    fn stage(label: &str, dim: &str) -> Stage {
        Stage {
            label: label.into(),
            dim: dim.into(),
            fell_back: false,
            kept_base: false,
            attempts: 1,
            usd: 0.1,
            cached: false,
            notes: String::new(),
            error: String::new(),
        }
    }

    fn record(mode: Mode, stages: Vec<Stage>) -> ModeRecord {
        ModeRecord {
            mode,
            ok: true,
            finals: vec![],
            stages,
            ledger: None,
            similarity: None,
            selection: None,
            errors: vec![],
            harnesses: vec![],
            scores: vec![],
        }
    }

    #[test]
    fn parallel_phases() {
        let m = record(
            Mode::Parallel,
            vec![
                stage("inv1", "INV"),
                stage("dom1", "DOM"),
                stage("inv2", "INV"),
                stage("agg1", "AGG"),
                stage("x1", "WEIRD"),
            ],
        );
        let p = phases(&m);
        let titles: Vec<&str> = p.iter().map(|p| p.title.as_str()).collect();
        assert_eq!(p.len(), 4, "{titles:?}");
        assert_eq!(p[0].stages.len(), 2);
        assert_eq!(p[2].stages[0].label, "agg1");
        assert_eq!(p[3].stages[0].dim, "WEIRD");
    }

    #[test]
    fn sequential_phases_group_by_step() {
        let m = record(
            Mode::Sequential,
            vec![
                stage("inv1", "INV"),
                stage("dom1", "DOM"),
                stage("c1", "COMPOSE"),
                stage("inv2", "INV"),
                stage("c2", "COMPOSE"),
            ],
        );
        let p = phases(&m);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].stages.len(), 3);
        assert_eq!(p[1].title, "Step 2: propose, probe, compose");
    }

    #[test]
    fn outcomes_and_selection() {
        let mut s = stage("inv1", "INV");
        assert_eq!(stage_outcome(&s).0, "accepted");
        s.kept_base = true;
        assert_eq!(stage_outcome(&s).0, "kept base");
        s.fell_back = true;
        assert_eq!(stage_outcome(&s).0, "fell back");
        s.error = "boom".into();
        assert_eq!(stage_outcome(&s).1, Tone::Danger);
        let sel = Selection {
            ok: true,
            chosen_label: "agg2".into(),
            chosen_run_id: "r".into(),
            chosen_is_base: false,
            id_map: vec![AnonId {
                id: "A".into(),
                label: "agg2".into(),
            }],
            n_candidates: 3,
            why: String::new(),
            trivial: false,
            fallback: true,
            dropped: vec![],
            usd: 0.0,
            error: String::new(),
            evidence: "deliverable".into(),
            harness_fingerprint: None,
            experience: None,
        };
        let text = selection_summary(&sel);
        assert!(text.contains("3 anonymised candidates"));
        assert!(text.contains("adopted agg2"));
        assert!(text.contains("fallback"));
        let trivial = Selection {
            trivial: true,
            ..sel
        };
        assert!(selection_summary(&trivial).contains("k = 1"));
    }

    #[test]
    fn ledger_balance() {
        let mut l = Ledger {
            r: 4,
            expected_r: 4,
            p: 3,
            expected_p: 3,
            session_attempts: 3,
            usd: 1.0,
            rollout_usd: 0.0,
            session_usd: 1.0,
        };
        assert!(ledger_problems(&l).is_empty());
        l.r = 3;
        l.p = 4;
        assert_eq!(ledger_problems(&l).len(), 2);
        assert_eq!(dim_label("DOM"), "Domain edit");
    }
}
