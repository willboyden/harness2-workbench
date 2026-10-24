//! MCP prompts: guided workflows built from the tools.
//!
//! Prompt arguments arrive as strings (MCP clients send them that way); each is checked
//! (fixed choices, numbers, length) before it is written into the prompt text.

use rmcp::{
    ErrorData as McpError,
    model::{GetPromptResult, JsonObject, Prompt, PromptArgument, PromptMessage, Role},
};
use serde_json::Value;

const IMPROVE: &str = "improve_agent_harness";
const ANALYZE: &str = "analyze_sweep";
const ADOPT: &str = "adopt_best_harness";

/// Longest identifier argument accepted.
const MAX_ID: usize = 128;
/// Longest free-text argument accepted.
const MAX_TEXT: usize = 1000;

fn arg(name: &str, description: &str, required: bool) -> PromptArgument {
    PromptArgument::new(name)
        .with_description(description)
        .with_required(required)
}

pub(crate) fn list() -> Vec<Prompt> {
    vec![
        Prompt::new(
            IMPROVE,
            Some(
                "Guided workflow to measure and improve an AI agent's harness with harness2: \
                 environment check, model profile, benchmark setup and check, preview, cost \
                 estimate and your go-ahead, pipeline, waiting, report, inspection of the best \
                 harness, and export.",
            ),
            Some(vec![
                arg(
                    "goal",
                    "What you want to improve or learn, in your words.",
                    false,
                ),
                arg("bench", "Benchmark: jb, lab or wb.", false),
                arg(
                    "subset",
                    "WorkBuddy subset (wb only): office, web or code.",
                    false,
                ),
                arg(
                    "substrate",
                    "Executor: oc (OpenCode, default) or cx (Codex).",
                    false,
                ),
                arg("experiment", "Experiment to use or create.", false),
                arg("profile", "Model profile to run with.", false),
                arg("k", "Recursion budget k (1-32, default 1).", false),
                arg("budget_usd", "Spend cap in USD for the run.", false),
            ]),
        )
        .with_title("Improve an agent harness with harness2"),
        Prompt::new(
            ANALYZE,
            Some(
                "Read-only analysis of one sweep: scores with their units, ungraded rows, best \
                 and worst tasks, what changed in the harness and why, the lessons learned, and \
                 how confident the result is.",
            ),
            Some(vec![
                arg("experiment", "Experiment name.", true),
                arg("sweep", "Sweep id (summary namespace).", true),
                arg("mode", "Focus on one mode: parallel or sequential.", false),
            ]),
        )
        .with_title("Analyse a sweep"),
        Prompt::new(
            ADOPT,
            Some(
                "Pick the best harness of a sweep, explain its changes, validate it and export \
                 it as project files for OpenCode, Codex or raw, with your approval before \
                 anything is written.",
            ),
            Some(vec![
                arg("experiment", "Experiment name.", true),
                arg("sweep", "Sweep id (summary namespace).", true),
                arg(
                    "target",
                    "Export layout: opencode (default), codex or raw.",
                    false,
                ),
                arg("mode", "Preferred mode: parallel or sequential.", false),
                arg(
                    "task",
                    "A specific task slug to take the harness from.",
                    false,
                ),
            ]),
        )
        .with_title("Adopt the best harness"),
    ]
}

/// Validated prompt arguments.
struct PromptArgs<'a> {
    name: &'a str,
    args: Option<&'a JsonObject>,
}

impl PromptArgs<'_> {
    /// Refuse arguments the prompt does not declare.
    fn check_known(&self, known: &[&str]) -> Result<(), McpError> {
        if let Some(args) = self.args {
            for key in args.keys() {
                if !known.contains(&key.as_str()) {
                    return Err(McpError::invalid_params(
                        format!(
                            "prompt {} has no argument `{key}` (arguments: {})",
                            self.name,
                            known.join(", ")
                        ),
                        None,
                    ));
                }
            }
        }
        Ok(())
    }

    /// The trimmed value, `None` when absent or empty.
    fn raw(&self, key: &str) -> Option<String> {
        let value = self.args?.get(key)?;
        let text = match value {
            Value::String(s) => s.trim().to_owned(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            _ => return None,
        };
        (!text.is_empty()).then_some(text)
    }

    fn invalid(&self, key: &str, why: &str) -> McpError {
        McpError::invalid_params(format!("prompt {}: `{key}` {why}", self.name), None)
    }

    fn identifier(&self, key: &str) -> Result<Option<String>, McpError> {
        let Some(value) = self.raw(key) else {
            return Ok(None);
        };
        let ok = value.len() <= MAX_ID
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
        if ok {
            Ok(Some(value))
        } else {
            Err(self.invalid(
                key,
                "must be an identifier (letters, digits, '.', '_', '-', at most 128 characters)",
            ))
        }
    }

    fn required(&self, key: &str) -> Result<String, McpError> {
        self.identifier(key)?
            .ok_or_else(|| self.invalid(key, "is required"))
    }

    fn choice(&self, key: &str, allowed: &[&str]) -> Result<Option<String>, McpError> {
        match self.raw(key) {
            None => Ok(None),
            Some(v) if allowed.contains(&v.as_str()) => Ok(Some(v)),
            Some(_) => Err(self.invalid(key, &format!("must be one of {}", allowed.join(", ")))),
        }
    }

    fn text(&self, key: &str) -> Result<Option<String>, McpError> {
        let Some(value) = self.raw(key) else {
            return Ok(None);
        };
        if value.chars().count() > MAX_TEXT {
            return Err(self.invalid(key, "is too long (at most 1000 characters)"));
        }
        Ok(Some(
            value
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect(),
        ))
    }
}

pub(crate) fn get(name: &str, args: Option<&JsonObject>) -> Result<GetPromptResult, McpError> {
    let a = PromptArgs { name, args };
    let (description, text) = match name {
        IMPROVE => ("Improve an agent harness with harness2", improve(&a)?),
        ANALYZE => ("Analyse a sweep", analyze(&a)?),
        ADOPT => ("Adopt the best harness", adopt(&a)?),
        _ => {
            return Err(McpError::invalid_params(
                format!("unknown prompt {name:?}; prompts: {IMPROVE}, {ANALYZE}, {ADOPT}"),
                None,
            ));
        }
    };
    Ok(
        GetPromptResult::new(vec![PromptMessage::new_text(Role::User, text)])
            .with_description(description),
    )
}

fn improve(a: &PromptArgs<'_>) -> Result<String, McpError> {
    a.check_known(&[
        "goal",
        "bench",
        "subset",
        "substrate",
        "experiment",
        "profile",
        "k",
        "budget_usd",
    ])?;
    let goal = a.text("goal")?;
    let bench = a.choice("bench", &["jb", "lab", "wb"])?;
    let subset = a.choice("subset", &["office", "web", "code"])?;
    let substrate = a.choice("substrate", &["oc", "cx"])?;
    let experiment = a.identifier("experiment")?;
    let profile = a.identifier("profile")?;
    let k = match a.raw("k") {
        None => 1,
        Some(v) => match v.parse::<u32>() {
            Ok(k) if (1..=h2s_api::K_MAX).contains(&k) => k,
            _ => return Err(a.invalid("k", "must be a whole number from 1 to 32")),
        },
    };
    let budget = match a.raw("budget_usd") {
        None => None,
        Some(v) => match v.parse::<f64>() {
            Ok(b) if b.is_finite() && b > 0.0 => Some(b),
            _ => return Err(a.invalid("budget_usd", "must be a positive number of dollars")),
        },
    };

    let cell = match (bench.as_deref(), subset.as_deref()) {
        (Some("wb"), Some(sub)) => format!("WorkBuddy-Bench, subset {sub} (bench wb)"),
        (Some("wb"), None) => "WorkBuddy-Bench (bench wb; ask me for the subset)".to_owned(),
        (Some("jb"), _) => "JobBench (bench jb)".to_owned(),
        (Some("lab"), _) => "LAB (bench lab)".to_owned(),
        _ => "not chosen yet: list_benchmarks and ask me".to_owned(),
    };
    let executor = match substrate.as_deref() {
        Some("cx") => "Codex (substrate cx; it needs a Responses-API endpoint, e.g. start_bridge)",
        _ => "OpenCode (substrate oc)",
    };
    let budget = budget.map_or_else(
        || "ask me before starting".to_owned(),
        |b| format!("{b:.2} USD (pass it as budget_usd)"),
    );
    Ok(format!(
        "Help me improve an AI agent's harness with harness2, using the Harness² Studio tools.

Goal: {goal}
Benchmark: {cell}. Executor: {executor}.
Experiment: {experiment}. Model profile: {profile}. Recursion budget k: {k}. Spend cap: {budget}.

Work through these steps in order. Stop and ask me before anything that costs money, needs a \
role I do not have, or changes the server, and tell me what each step found.

1. server_info: confirm the server answers and note my role (viewer reads; operator runs jobs; \
admin installs). If harness2 is not installed, it needs install_harness2 (admin), then \
wait_for_job.
2. doctor: list failing checks that matter for this benchmark and executor, with their \
remedies.
3. list_profiles: pick the profile; all of its secrets must be set.
4. list_benchmarks: if the benchmark is not installed, setup_benchmark (admin; network-heavy; \
dry_run=true first), then check_benchmark until result.ready is true.
5. get_experiment, or create_experiment pinned to the profile.
6. preview_run with a small scope first (options.max_tasks, options.only_domains), k={k} and \
modes [parallel] unless I say otherwise; read the task stream and totals.
7. estimate_cost with the preview's total_tasks; show me the rollouts, improver sessions and \
USD range, and wait for my explicit go-ahead.
8. start_pipeline with budget_usd and judge_bases=true. If server_info shows host isolation, \
the server needs acknowledge_risks=true: the executor then runs with the server user's files \
and network. Explain that to me. You may set it only if server_info's policy allows it and I \
agree; otherwise I start the run in the web UI.
9. wait_for_job until the job finishes (call it again after a timeout); on failure read \
get_job_log with tail=100.
10. get_report for the sweep (the run step's namespace): per mode, score and all-pass rate in \
percent next to base_score. Ungraded tasks are left out, not zero; repeat upstream's notes and \
warnings, such as the noise floor at k=1.
11. list_tasks and get_task for the best and the worst tasks; diff_harnesses from base to the \
chosen final to explain what changed; get_library for the lessons per domain.
12. Recommend the best harness with the evidence, then export_harness for my agent (opencode \
or codex), run validate_harness on a raw export, and show me the files before anything is \
written to my project.",
        goal = goal.as_deref().unwrap_or(
            "measure the current agent harness on a benchmark and improve it with harness2"
        ),
        experiment = experiment
            .as_deref()
            .unwrap_or("not chosen yet (suggest a name)"),
        profile = profile.as_deref().unwrap_or("not chosen yet"),
    ))
}

fn analyze(a: &PromptArgs<'_>) -> Result<String, McpError> {
    a.check_known(&["experiment", "sweep", "mode"])?;
    let experiment = a.required("experiment")?;
    let sweep = a.required("sweep")?;
    let mode = a.choice("mode", &["parallel", "sequential"])?;
    let focus = mode
        .map(|m| format!(" Focus on the {m} mode."))
        .unwrap_or_default();
    Ok(format!(
        "Analyse sweep {sweep} of experiment {experiment} with the Harness² Studio tools.{focus} \
This is read-only: do not start, cancel or change anything.

1. get_experiment(experiment=\"{experiment}\"): the sweep's cell, k, seed, modes, task counts \
and spend.
2. get_report(experiment=\"{experiment}\", sweep=\"{sweep}\"): per mode and k, the graded \
count n and ungraded count (left out, not zero), score and all-pass rate in PERCENT, base_score \
when present, how often the selector kept the base harness or chose a final or a candidate, \
fallbacks and spend. List every NOTE and WARNING.
3. get_scores: find ungraded rows and judge errors (per-task frac is a FRACTION 0..1).
4. list_tasks: which domains and tasks gained or lost; spot failed or partial tasks.
5. For the two best and the two worst tasks: get_task (the selector's why, the improver's \
notes, fallbacks) and diff_harnesses from base to the chosen label.
6. get_library: the lessons the reflector recorded per domain.
7. list_audit_sessions: sessions with errors or many attempts; read_audit_session for one or \
two that explain a failure.
8. If a comparable sweep exists (same cell, another seed, k or split), compare_sweeps and \
report wins, losses and the sign-test p-value (deltas are fractions; times 100 for percentage \
points).

Finish with a short summary: what improved and by how much (with units), how confident we can \
be (graded counts, p-values, noise warnings), and what to try next."
    ))
}

fn adopt(a: &PromptArgs<'_>) -> Result<String, McpError> {
    a.check_known(&["experiment", "sweep", "target", "mode", "task"])?;
    let experiment = a.required("experiment")?;
    let sweep = a.required("sweep")?;
    let target = a
        .choice("target", &["opencode", "codex", "raw"])?
        .unwrap_or_else(|| "opencode".to_owned());
    let mode = a.choice("mode", &["parallel", "sequential"])?;
    let task = a.identifier("task")?;
    let mode_line = mode
        .map(|m| format!(" (I prefer the {m} mode)"))
        .unwrap_or_default();
    let choose = match task {
        Some(task) => format!(
            "Use task {task}: get_task(experiment=\"{experiment}\", sweep=\"{sweep}\", \
             task=\"{task}\") gives modes[].selection.chosen_label."
        ),
        None => "Use list_tasks to find tasks where the chosen harness beat the base (score, \
                 all_pass) and get_library for each domain's adopted harnesses and lessons; \
                 get_task gives modes[].selection.chosen_label."
            .to_owned(),
    };
    Ok(format!(
        "Help me adopt the best harness from sweep {sweep} of experiment {experiment} into my \
agent project as {target} files. Nothing may be written to my project until I approve the \
files.

1. get_report(experiment=\"{experiment}\", sweep=\"{sweep}\"): choose the mode with the best \
score{mode_line}; mind the ungraded counts and the warnings.
2. {choose}
3. diff_harnesses from base to the chosen label, and explain each change (its component, and \
why, from the improver's notes).
4. get_harness for the chosen label to read it in full.
5. export_harness with target={target}; also export with target=raw and run validate_harness \
on those files (surface opencode or codex), and report any errors.
6. Show me every file path and its content, the export notes, and how to install them. Write \
files only after I confirm."
    ))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn args(v: Value) -> JsonObject {
        match v {
            Value::Object(o) => o,
            _ => JsonObject::new(),
        }
    }

    fn text(result: &GetPromptResult) -> String {
        result.messages[0]
            .content
            .as_text()
            .map(|t| t.text.clone())
            .unwrap_or_default()
    }

    #[test]
    fn lists_three_prompts_with_arguments() {
        let prompts = list();
        let names: Vec<&str> = prompts.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec![IMPROVE, ANALYZE, ADOPT]);
        for p in &prompts {
            assert!(p.description.as_deref().is_some_and(|d| d.len() > 20));
            assert!(!p.arguments.as_deref().unwrap_or_default().is_empty());
        }
    }

    #[test]
    fn improve_fills_in_arguments() {
        let a = args(json!({"bench": "wb", "subset": "code", "k": "2", "budget_usd": "5"}));
        let result = get(IMPROVE, Some(&a)).unwrap();
        let t = text(&result);
        assert!(t.contains("WorkBuddy-Bench, subset code"), "{t}");
        assert!(t.contains("k=2"), "{t}");
        assert!(t.contains("5.00 USD"), "{t}");
        assert!(t.contains("estimate_cost"));
        // No arguments at all is fine.
        assert!(get(IMPROVE, None).is_ok());
    }

    #[test]
    fn arguments_are_validated() {
        let bad = [
            (IMPROVE, json!({"bench": "xx"})),
            (IMPROVE, json!({"k": "64"})),
            (IMPROVE, json!({"budget_usd": "-1"})),
            (IMPROVE, json!({"typo": "x"})),
            (ANALYZE, json!({"experiment": "e"})),
            (ANALYZE, json!({"experiment": "e", "sweep": "s; rm -rf"})),
            (
                ADOPT,
                json!({"experiment": "e", "sweep": "s", "target": "zip"}),
            ),
        ];
        for (name, v) in bad {
            let a = args(v.clone());
            assert!(get(name, Some(&a)).is_err(), "{name} {v}");
        }
        assert!(get("nope", None).is_err());
    }

    #[test]
    fn analyze_and_adopt_name_their_sweep() {
        let a = args(json!({"experiment": "demo", "sweep": "demo-oc-agg2", "mode": "parallel"}));
        let t = text(&get(ANALYZE, Some(&a)).unwrap());
        assert!(t.contains("sweep demo-oc-agg2 of experiment demo"));
        assert!(t.contains("Focus on the parallel mode."));
        let t = text(&get(ADOPT, Some(&a)).unwrap());
        assert!(t.contains("as opencode files"));
    }
}
