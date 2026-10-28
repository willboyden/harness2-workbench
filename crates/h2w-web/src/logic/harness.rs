//! Harness trees: the eight components (c1..c8), grouping files by component, the
//! always-on budget, and ordering of harness labels.

use h2w_api::{Census, Component, HarnessFile, HarnessRole, ModeRecord, Surface, TaskDetail};

pub(crate) const COMPONENTS: [Component; 8] = [
    Component::SystemPrompt,
    Component::Guardrails,
    Component::Memory,
    Component::ToolGuidance,
    Component::Subagents,
    Component::Skills,
    Component::Scripts,
    Component::Plugins,
];

pub(crate) fn code(c: Component) -> &'static str {
    match c {
        Component::SystemPrompt => "c1",
        Component::Guardrails => "c2",
        Component::Memory => "c3",
        Component::ToolGuidance => "c4",
        Component::Subagents => "c5",
        Component::Skills => "c6",
        Component::Scripts => "c7",
        Component::Plugins => "c8",
    }
}

pub(crate) fn name(c: Component) -> &'static str {
    match c {
        Component::SystemPrompt => "System prompt",
        Component::Guardrails => "Guardrails",
        Component::Memory => "Memory",
        Component::ToolGuidance => "Tool guidance",
        Component::Subagents => "Sub-agents",
        Component::Skills => "Skills",
        Component::Scripts => "Scripts",
        Component::Plugins => "Plugins & hooks",
    }
}

/// Plain-language description of each component, for newcomers.
pub(crate) fn blurb(c: Component) -> &'static str {
    match c {
        Component::SystemPrompt => {
            "The standing instructions the agent reads first on every task (always on)."
        }
        Component::Guardrails => {
            "Rules the agent must not break, such as checks before submitting (always on)."
        }
        Component::Memory => {
            "Long-term notes carried into every session: facts and lessons (always on)."
        }
        Component::ToolGuidance => "Advice on when and how to use each tool (always on).",
        Component::Subagents => {
            "Helper agents the main agent can delegate to, each with its own context."
        }
        Component::Skills => "On-demand workflows the agent loads only when a task needs them.",
        Component::Scripts => {
            "Deterministic helper programs the agent can run instead of reasoning by hand."
        }
        Component::Plugins => {
            "Code that acts inside the tool loop: OpenCode plugins or Codex hooks."
        }
    }
}

/// Whether the component's text counts against the always-on budget.
pub(crate) fn always_on(c: Component) -> bool {
    matches!(
        c,
        Component::SystemPrompt
            | Component::Guardrails
            | Component::Memory
            | Component::ToolGuidance
    )
}

pub(crate) fn surface_label(s: Surface) -> &'static str {
    match s {
        Surface::Opencode => "OpenCode",
        Surface::Codex => "Codex",
        Surface::Hermes => "Hermes Agent",
        Surface::Turnstone => "Turnstone",
    }
}

/// Files grouped by component in c1..c8 order; every component appears (possibly empty).
pub(crate) fn group_files(files: &[HarnessFile]) -> Vec<(Component, Vec<&HarnessFile>)> {
    COMPONENTS
        .iter()
        .map(|c| {
            let mut v: Vec<&HarnessFile> = files.iter().filter(|f| f.component == *c).collect();
            v.sort_by(|a, b| a.path.cmp(&b.path));
            (*c, v)
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BudgetLevel {
    Ok,
    /// Above 80 % of the cap.
    Near,
    /// Over the cap: harness2 rejects the tree.
    Over,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Budget {
    pub(crate) used: u32,
    pub(crate) cap: u32,
    /// `used / cap`, clamped to `0..=1` for drawing.
    pub(crate) fraction: f64,
    pub(crate) level: BudgetLevel,
}

/// The always-on character budget of a harness.
pub(crate) fn budget(c: &Census) -> Budget {
    let cap = c.always_on_max;
    let raw = if cap == 0 {
        0.0
    } else {
        f64::from(c.always_on_chars) / f64::from(cap)
    };
    let level = if c.always_on_chars > cap {
        BudgetLevel::Over
    } else if raw > 0.8 {
        BudgetLevel::Near
    } else {
        BudgetLevel::Ok
    };
    Budget {
        used: c.always_on_chars,
        cap,
        fraction: raw.clamp(0.0, 1.0),
        level,
    }
}

pub(crate) fn role_label(r: HarnessRole) -> &'static str {
    match r {
        HarnessRole::Base => "base",
        HarnessRole::Candidate => "candidate",
        HarnessRole::Final => "final",
    }
}

/// Sort key: base first, then inv, dom, agg, c, each by number.
pub(crate) fn label_key(label: &str) -> (u8, u32, String) {
    let digits_at = label
        .find(|c: char| c.is_ascii_digit())
        .unwrap_or(label.len());
    let (prefix, num) = label.split_at(digits_at);
    let rank = match prefix {
        "base" => 0,
        "inv" => 1,
        "dom" => 2,
        "agg" => 3,
        "c" => 4,
        _ => 5,
    };
    (rank, num.parse().unwrap_or(0), label.to_owned())
}

pub(crate) fn sort_labels(labels: &mut [String]) {
    labels.sort_by_key(|l| label_key(l));
}

/// Rollouts that can be browsed for a mode: the task's base executions (`base1..basek`,
/// shared by both modes) plus one execution per candidate and final harness.
pub(crate) fn rollout_labels(detail: &TaskDetail, mode: &ModeRecord) -> Vec<String> {
    let mut out: Vec<String> = detail
        .base
        .iter()
        .filter(|b| b.available)
        .map(|b| b.label.clone())
        .collect();
    let mut rest: Vec<String> = mode
        .harnesses
        .iter()
        .filter(|h| h.role != HarnessRole::Base)
        .map(|h| h.label.clone())
        .collect();
    sort_labels(&mut rest);
    out.extend(rest);
    out
}

/// Harness labels of a mode in display order, the chosen one marked.
pub(crate) fn harness_labels(mode: &ModeRecord) -> Vec<(String, HarnessRole, bool)> {
    let mut v: Vec<(String, HarnessRole, bool)> = mode
        .harnesses
        .iter()
        .map(|h| (h.label.clone(), h.role, h.chosen))
        .collect();
    v.sort_by_key(|(l, _, _)| label_key(l));
    v
}

/// The default pair for the diff viewer: base -> the chosen harness (or the first final).
pub(crate) fn default_diff_pair(mode: &ModeRecord) -> Option<(String, String)> {
    let labels = harness_labels(mode);
    let from = labels
        .iter()
        .find(|(_, r, _)| *r == HarnessRole::Base)
        .map(|(l, _, _)| l.clone())?;
    let to = labels
        .iter()
        .find(|(_, _, chosen)| *chosen)
        .filter(|(l, _, _)| *l != from)
        .or_else(|| labels.iter().find(|(_, r, _)| *r == HarnessRole::Final))
        .or_else(|| labels.iter().find(|(l, _, _)| *l != from))
        .map(|(l, _, _)| l.clone())?;
    Some((from, to))
}

#[cfg(test)]
mod tests {
    use super::*;
    use h2w_api::{BaseRun, HarnessRef, Mode};

    fn file(path: &str, c: Component) -> HarnessFile {
        HarnessFile {
            path: path.into(),
            component: c,
            bytes: 10,
            content: None,
        }
    }

    fn mode_record(labels: &[(&str, HarnessRole, bool)]) -> ModeRecord {
        ModeRecord {
            mode: Mode::Parallel,
            ok: true,
            finals: vec![],
            stages: vec![],
            ledger: None,
            similarity: None,
            selection: None,
            errors: vec![],
            harnesses: labels
                .iter()
                .map(|(l, r, c)| HarnessRef {
                    label: (*l).to_owned(),
                    role: *r,
                    chosen: *c,
                })
                .collect(),
            scores: vec![],
        }
    }

    #[test]
    fn components_and_grouping() {
        let codes: Vec<&str> = COMPONENTS.iter().map(|c| code(*c)).collect();
        assert_eq!(codes, vec!["c1", "c2", "c3", "c4", "c5", "c6", "c7", "c8"]);
        let files = vec![
            file("skills/b/SKILL.md", Component::Skills),
            file("systemprompt.md", Component::SystemPrompt),
            file("skills/a/SKILL.md", Component::Skills),
        ];
        let g = group_files(&files);
        assert_eq!(g.len(), 8);
        assert_eq!(g[0].1.len(), 1);
        assert_eq!(g[5].1[0].path, "skills/a/SKILL.md");
        assert!(g[7].1.is_empty());
        assert!(always_on(Component::Memory));
        assert!(!always_on(Component::Skills));
    }

    #[test]
    fn budget_levels() {
        let mut c = Census {
            always_on_chars: 1000,
            always_on_max: 24_000,
            ..Census::default()
        };
        assert_eq!(budget(&c).level, BudgetLevel::Ok);
        c.always_on_chars = 20_000;
        assert_eq!(budget(&c).level, BudgetLevel::Near);
        c.always_on_chars = 30_000;
        let b = budget(&c);
        assert_eq!(b.level, BudgetLevel::Over);
        assert!((b.fraction - 1.0).abs() < f64::EPSILON);
        c.always_on_max = 0;
        assert!(budget(&c).fraction.abs() < f64::EPSILON);
    }

    #[test]
    fn label_ordering() {
        let mut v: Vec<String> = ["c2", "agg1", "base", "dom1", "inv2", "inv1", "c10", "c1"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        sort_labels(&mut v);
        assert_eq!(
            v,
            vec!["base", "inv1", "inv2", "dom1", "agg1", "c1", "c2", "c10"]
        );
    }

    #[test]
    fn rollouts_and_default_diff() {
        let m = mode_record(&[
            ("agg1", HarnessRole::Final, false),
            ("base", HarnessRole::Base, false),
            ("inv1", HarnessRole::Candidate, false),
            ("agg2", HarnessRole::Final, true),
        ]);
        let detail = TaskDetail {
            task_id: "t".into(),
            slug: "t".into(),
            domain: "d".into(),
            k: 2,
            seed: None,
            status: "ok".into(),
            base: vec![
                BaseRun {
                    label: "base1".into(),
                    run_id: "r1".into(),
                    available: true,
                },
                BaseRun {
                    label: "base2".into(),
                    run_id: "r2".into(),
                    available: false,
                },
            ],
            modes: vec![],
            errors: vec![],
            task_prompt: None,
        };
        assert_eq!(
            rollout_labels(&detail, &m),
            vec!["base1", "inv1", "agg1", "agg2"]
        );
        assert_eq!(
            default_diff_pair(&m),
            Some(("base".to_owned(), "agg2".to_owned()))
        );
        let none_chosen = mode_record(&[
            ("base", HarnessRole::Base, true),
            ("c1", HarnessRole::Final, false),
        ]);
        assert_eq!(
            default_diff_pair(&none_chosen),
            Some(("base".to_owned(), "c1".to_owned()))
        );
        assert_eq!(default_diff_pair(&mode_record(&[])), None);
    }
}
