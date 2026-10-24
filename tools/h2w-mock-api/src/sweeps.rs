//! Deterministic sweep fixtures. A sweep is stored as a compact [`SweepSpec`]; everything
//! the API exposes about it (task rows, task details, harness trees, diffs, exports,
//! rollouts, scores, the report, the library, the improver audit trail) is generated from
//! it on request, always as `h2w_api` types so the shapes match the contract.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use h2w_api::{
    AnonId, AuditSession, BaseJudgedRow, BaseRun, Cell, Census, ChangeKind, Component, Experience,
    ExportBundle, ExportTarget, FileContent, FileDiff, FileEntry, FileKind, FileText, HarnessDiff,
    HarnessFile, HarnessRef, HarnessRole, HarnessTree, JudgedRow, Ledger, LibraryDomain,
    LibraryEntry, Mode, ModeRecord, ModeScore, ModeSummary, ScoreReport, Scores, Selection,
    Similarity, Stage, Substrate, Surface, SweepSummary, TaskDetail, TaskModeOutcome, TaskRow,
};

use crate::util::{Rng, seed_of, unified_diff};

/// How far a task got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskState {
    Ok,
    Partial,
    BaseFailed,
    Running,
}

impl TaskState {
    fn as_str(self) -> &'static str {
        match self {
            TaskState::Ok => "ok",
            TaskState::Partial => "partial",
            TaskState::BaseFailed => "base-failed",
            TaskState::Running => "running",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TaskSpec {
    pub(crate) id: String,
    pub(crate) domain: String,
    pub(crate) state: TaskState,
}

impl TaskSpec {
    pub(crate) fn slug(&self) -> String {
        self.id.replace('/', "__")
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SweepSpec {
    /// Summary namespace (`lab-oc-agg1`).
    pub(crate) id: String,
    pub(crate) bench_name: String,
    pub(crate) cell: Option<Cell>,
    pub(crate) k: u32,
    pub(crate) seed: Option<u64>,
    pub(crate) modes: Vec<Mode>,
    pub(crate) tasks: Vec<TaskSpec>,
    pub(crate) judged: bool,
    pub(crate) base_judged: bool,
    pub(crate) updated_at: String,
    pub(crate) models: (String, String),
}

impl SweepSpec {
    pub(crate) fn substrate(&self) -> Substrate {
        self.cell.map_or(Substrate::Oc, |c| c.substrate)
    }

    fn surface(&self) -> Surface {
        match self.substrate() {
            Substrate::Oc => Surface::Opencode,
            Substrate::Cx => Surface::Codex,
        }
    }

    pub(crate) fn task(&self, slug: &str) -> Option<&TaskSpec> {
        self.tasks.iter().find(|t| t.slug() == slug)
    }

    /// A deterministic, non-cryptographic generator for one stream of mock values of a task.
    fn rng(&self, task: &TaskSpec, stream: &str) -> Rng {
        Rng::new(seed_of(&format!("{}|{}|{stream}", self.id, task.id)))
    }

    /// Namespace of one mode (`lab-oc-agg1` / `lab-oc-seq1`).
    pub(crate) fn namespace(&self, mode: Mode) -> String {
        self.id.replacen("-agg", &format!("-{}", mode.token()), 1)
    }
}

/// Labels of one mode's harnesses: base, candidates, finals.
pub(crate) fn labels(mode: Mode, k: u32) -> Vec<(String, HarnessRole)> {
    let mut v = vec![("base".to_owned(), HarnessRole::Base)];
    for i in 1..=k {
        v.push((format!("inv{i}"), HarnessRole::Candidate));
    }
    for i in 1..=k {
        v.push((format!("dom{i}"), HarnessRole::Candidate));
    }
    let final_prefix = match mode {
        Mode::Parallel => "agg",
        Mode::Sequential => "c",
    };
    for i in 1..=k {
        v.push((format!("{final_prefix}{i}"), HarnessRole::Final));
    }
    v
}

fn domain_slug(domain: &str) -> String {
    domain
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// The chosen label of a mode for a task (deterministic), and whether it is the base.
fn chosen(spec: &SweepSpec, task: &TaskSpec, mode: Mode) -> (String, bool) {
    let mut rng = spec.rng(task, &format!("chosen-{}", mode.token()));
    let finals: Vec<String> = labels(mode, spec.k)
        .into_iter()
        .filter(|(_, r)| *r == HarnessRole::Final)
        .map(|(l, _)| l)
        .collect();
    let roll = rng.unit();
    if roll < 0.12 {
        ("base".to_owned(), true)
    } else if roll < 0.2 && spec.k > 1 {
        ("dom1".to_owned(), false)
    } else {
        let idx = usize::try_from(rng.below(finals.len() as u64)).unwrap_or(0);
        (
            finals
                .get(idx)
                .cloned()
                .unwrap_or_else(|| "agg1".to_owned()),
            false,
        )
    }
}

// ---------------------------------------------------------------------------------------
// Harness trees

const BASE_PROMPT: &str = "# Role\n\nYou are a careful professional assistant working inside a sandboxed project.\nRead the task statement completely before acting.\n\n# Working method\n\n1. Restate the deliverable in one sentence.\n2. List the inputs you were given and open each one.\n3. Draft the deliverable in the requested format.\n4. Re-read the task and check every requirement against the draft.\n\n# Output\n\nWrite the final deliverable to the path the task names. Do not leave placeholders.\nWhen something is ambiguous, state the assumption you made at the top of the deliverable.\n";

const BASE_GUARDRAILS: &str = "# Guardrails\n\n- Never invent facts, figures or citations that are not in the inputs.\n- Do not delete or overwrite input files.\n- Stop and report if a required input is missing.\n- Keep the deliverable within any length limit the task states.\n";

const BASE_MEMORY: &str =
    "# Long-term memory\n\n(empty: lessons from earlier tasks are added here)\n";

const BASE_TOOLS: &str = "# Tool guidance\n\n- Use `read` to open inputs; prefer reading whole files over guessing.\n- Use `write` once per deliverable; rewrite the whole file when revising.\n- Use `bash` only for deterministic checks such as word counts.\n";

fn harness_files_raw(
    spec: &SweepSpec,
    task: &TaskSpec,
    mode: Mode,
    label: &str,
) -> Vec<(String, Component, String)> {
    let surface = spec.surface();
    let dslug = domain_slug(&task.domain);
    let mut prompt = BASE_PROMPT.to_owned();
    let mut guard = BASE_GUARDRAILS.to_owned();
    let mut memory = BASE_MEMORY.to_owned();
    let mut tools = BASE_TOOLS.to_owned();
    let mut extra: Vec<(String, Component, String)> = Vec::new();
    let num: u32 = label
        .trim_start_matches(|c: char| c.is_ascii_alphabetic())
        .parse()
        .unwrap_or(1);
    let is_final = label.starts_with("agg") || label.starts_with('c');
    if label.starts_with("inv") || is_final {
        prompt = prompt.replace(
            "4. Re-read the task",
            "4. Before writing, outline the deliverable's sections and the evidence each needs.\n5. Re-read the task",
        );
        let _ = writeln!(
            guard,
            "- Quote the exact source sentence for every factual claim (variant {num})."
        );
    }
    if label.starts_with("dom") || is_final {
        memory = format!(
            "# Long-term memory\n\n- {}: reviewers check defined terms first; define every capitalised term before use.\n- {}: cite the clause or row number next to each claim.\n",
            task.domain, task.domain
        );
        let skill_dir = match surface {
            Surface::Opencode => format!("skills/{dslug}-review"),
            Surface::Codex => format!(".agents/skills/{dslug}-review"),
        };
        extra.push((
            format!("{skill_dir}/SKILL.md"),
            Component::Skills,
            format!(
                "---\nname: {dslug}-review\ndescription: Checklist for reviewing {} deliverables before submission.\n---\n\n# {} review\n\n1. List every requirement in the task statement.\n2. For each, point to the paragraph of the draft that satisfies it.\n3. Flag unsupported claims and fix them before submitting.\n",
                task.domain, task.domain
            ),
        ));
    }
    if is_final {
        tools.push_str("- Run `python scripts/check_requirements.py <draft>` before submitting.\n");
        extra.push((
            "scripts/check_requirements.py".to_owned(),
            Component::Scripts,
            "\"\"\"List task requirements that the draft does not mention.\"\"\"\nimport re\nimport sys\n\n\ndef main(task_path: str, draft_path: str) -> int:\n    task = open(task_path, encoding=\"utf-8\").read()\n    draft = open(draft_path, encoding=\"utf-8\").read().lower()\n    missing = [r for r in re.findall(r\"must ([^.]+)\\.\", task) if r.lower()[:24] not in draft]\n    for item in missing:\n        print(f\"missing: {item}\")\n    return 1 if missing else 0\n\n\nif __name__ == \"__main__\":\n    sys.exit(main(sys.argv[1], sys.argv[2]))\n".to_owned(),
        ));
        if num.is_multiple_of(2) || spec.k == 1 {
            match surface {
                Surface::Opencode => {
                    extra.push((
                        "sub_agents/reviewer.md".to_owned(),
                        Component::Subagents,
                        "---\ndescription: Independent reviewer that reads only the task and the draft.\n---\n\nYou review a draft against its task. Report each unmet requirement with a one-line fix. Do not rewrite the draft.\n".to_owned(),
                    ));
                    extra.push((
                        "plugin/word-limit.ts".to_owned(),
                        Component::Plugins,
                        "// Warn when a written deliverable exceeds the task's word limit.\nexport const WordLimit = async () => ({\n  \"tool.execute.after\": async (input: { tool: string }, output: { output: string }) => {\n    if (input.tool !== \"write\") return;\n    const words = output.output.split(/\\s+/).length;\n    if (words > 1500) output.output += \"\\n[plugin] Draft exceeds 1,500 words.\";\n  },\n});\n".to_owned(),
                    ));
                }
                Surface::Codex => extra.push((
                    ".codex/hooks.json".to_owned(),
                    Component::Plugins,
                    "{\n  \"post_tool_use\": [{ \"match\": \"apply_patch\", \"run\": \"python scripts/check_requirements.py task.md out/draft.md\" }]\n}\n".to_owned(),
                )),
            }
        }
    }
    if label == "base" {
        // The unmodified starting harness.
    }
    let _ = mode;
    let mut files = match surface {
        Surface::Opencode => vec![
            (
                "systemprompt.md".to_owned(),
                Component::SystemPrompt,
                prompt,
            ),
            ("guardrails.md".to_owned(), Component::Guardrails, guard),
            ("LongTermMEMORY.md".to_owned(), Component::Memory, memory),
            (
                "tool_descriptions/tool_guidance.md".to_owned(),
                Component::ToolGuidance,
                tools,
            ),
        ],
        Surface::Codex => vec![(
            "AGENTS.md".to_owned(),
            Component::SystemPrompt,
            format!("{prompt}\n{guard}\n{memory}\n{tools}"),
        )],
    };
    files.extend(extra);
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

fn census_of(surface: Surface, files: &[(String, Component, String)]) -> Census {
    let always_on: usize = files
        .iter()
        .filter(|(_, c, _)| {
            matches!(
                c,
                Component::SystemPrompt
                    | Component::Guardrails
                    | Component::Memory
                    | Component::ToolGuidance
            )
        })
        .map(|(_, _, t)| t.chars().count())
        .sum();
    let count =
        |c: Component| u32::try_from(files.iter().filter(|f| f.1 == c).count()).unwrap_or(0);
    Census {
        always_on_chars: u32::try_from(always_on).unwrap_or(u32::MAX),
        always_on_max: match surface {
            Surface::Opencode => 24_000,
            Surface::Codex => 8_000,
        },
        n_files: u32::try_from(files.len()).unwrap_or(0),
        skills: count(Component::Skills),
        sub_agents: count(Component::Subagents),
        scripts: count(Component::Scripts),
        plugins: if surface == Surface::Opencode {
            count(Component::Plugins)
        } else {
            0
        },
        hooks: if surface == Surface::Codex {
            count(Component::Plugins)
        } else {
            0
        },
    }
}

fn fingerprint(files: &[(String, Component, String)]) -> String {
    let joined: String = files.iter().map(|(p, _, t)| format!("{p}\n{t}")).collect();
    format!("{:016x}", seed_of(&joined))
}

pub(crate) fn harness_tree(
    spec: &SweepSpec,
    task: &TaskSpec,
    mode: Mode,
    label: &str,
) -> Option<HarnessTree> {
    let role = labels(mode, spec.k)
        .into_iter()
        .find(|(l, _)| l == label)?
        .1;
    let files = harness_files_raw(spec, task, mode, label);
    let surface = spec.surface();
    Some(HarnessTree {
        label: label.to_owned(),
        role,
        surface,
        census: census_of(surface, &files),
        fingerprint: fingerprint(&files),
        files: files
            .into_iter()
            .map(|(path, component, content)| HarnessFile {
                path,
                component,
                bytes: content.len() as u64,
                content: Some(content),
            })
            .collect(),
    })
}

pub(crate) fn harness_diff(
    spec: &SweepSpec,
    task: &TaskSpec,
    mode: Mode,
    from: &str,
    to: &str,
) -> Option<HarnessDiff> {
    let a = harness_tree(spec, task, mode, from)?;
    let b = harness_tree(spec, task, mode, to)?;
    let mut paths: Vec<(String, Component)> = a
        .files
        .iter()
        .chain(b.files.iter())
        .map(|f| (f.path.clone(), f.component))
        .collect();
    paths.sort();
    paths.dedup_by(|x, y| x.0 == y.0);
    let content = |t: &HarnessTree, p: &str| {
        t.files
            .iter()
            .find(|f| f.path == p)
            .and_then(|f| f.content.clone())
    };
    let mut changed = Vec::new();
    let mut same = Vec::new();
    for (path, component) in paths {
        let (old, new) = (content(&a, &path), content(&b, &path));
        let fd = match (old, new) {
            (None, Some(n)) => FileDiff {
                lines_added: u32::try_from(n.lines().count()).unwrap_or(0),
                lines_removed: 0,
                path,
                component,
                change: ChangeKind::Added,
                unified: n,
            },
            (Some(o), None) => FileDiff {
                lines_added: 0,
                lines_removed: u32::try_from(o.lines().count()).unwrap_or(0),
                path,
                component,
                change: ChangeKind::Removed,
                unified: o,
            },
            (Some(o), Some(n)) => {
                let (u, add, del) = unified_diff(&o, &n);
                FileDiff {
                    path,
                    component,
                    change: if u.is_empty() {
                        ChangeKind::Unchanged
                    } else {
                        ChangeKind::Modified
                    },
                    unified: u,
                    lines_added: add,
                    lines_removed: del,
                }
            }
            (None, None) => continue,
        };
        if fd.change == ChangeKind::Unchanged {
            same.push(fd);
        } else {
            changed.push(fd);
        }
    }
    changed.extend(same);
    Some(HarnessDiff {
        from: from.to_owned(),
        to: to.to_owned(),
        files: changed,
        always_on_char_delta: i64::from(b.census.always_on_chars)
            - i64::from(a.census.always_on_chars),
    })
}

pub(crate) fn export(
    spec: &SweepSpec,
    task: &TaskSpec,
    mode: Mode,
    label: &str,
    target: ExportTarget,
) -> Option<ExportBundle> {
    let tree = harness_tree(spec, task, mode, label)?;
    let text = |p: &str| {
        tree.files
            .iter()
            .find(|f| f.path == p)
            .and_then(|f| f.content.clone())
            .unwrap_or_default()
    };
    let mut notes = Vec::new();
    let files: Vec<FileText> = match target {
        ExportTarget::Raw => tree
            .files
            .iter()
            .map(|f| FileText {
                path: f.path.clone(),
                content: f.content.clone().unwrap_or_default(),
            })
            .collect(),
        ExportTarget::Opencode | ExportTarget::Codex => {
            let always_on = if tree.surface == Surface::Codex {
                text("AGENTS.md")
            } else {
                format!(
                    "{}\n{}\n{}\n{}",
                    text("systemprompt.md"),
                    text("guardrails.md"),
                    text("LongTermMEMORY.md"),
                    text("tool_descriptions/tool_guidance.md")
                )
            };
            let mut out = vec![FileText {
                path: "AGENTS.md".into(),
                content: always_on,
            }];
            for f in &tree.files {
                let content = f.content.clone().unwrap_or_default();
                let mapped = match (target, f.component) {
                    (ExportTarget::Opencode, Component::Skills) => Some(format!(
                        ".opencode/{}",
                        f.path.trim_start_matches(".agents/")
                    )),
                    (ExportTarget::Opencode, Component::Subagents) => {
                        Some(f.path.replace("sub_agents/", ".opencode/agent/"))
                    }
                    (ExportTarget::Opencode, Component::Plugins) => {
                        Some(f.path.replace("plugin/", ".opencode/plugin/"))
                    }
                    (ExportTarget::Codex, Component::Skills) => Some(format!(
                        ".agents/skills/{}",
                        f.path.rsplit("skills/").next().unwrap_or(&f.path)
                    )),
                    (ExportTarget::Codex, Component::Plugins) => {
                        Some(".codex/hooks.json".to_owned())
                    }
                    (_, Component::Scripts | Component::Subagents) => Some(f.path.clone()),
                    _ => None,
                };
                if let Some(path) = mapped {
                    out.push(FileText { path, content });
                }
            }
            if target == ExportTarget::Opencode && tree.census.plugins > 0 {
                notes.push("Plugins need OpenCode 1.14 or newer.".into());
            }
            if target == ExportTarget::Codex && tree.census.sub_agents > 0 {
                notes.push("Codex has no sub-agent surface; sub-agent files are exported as plain markdown for reference.".into());
            }
            out
        }
    };
    notes.push(format!(
        "Exported from {} ({} mode, task {}).",
        label,
        mode.as_cli(),
        task.id
    ));
    Some(ExportBundle {
        target,
        label: label.to_owned(),
        files,
        notes,
    })
}

// ---------------------------------------------------------------------------------------
// Task detail

fn stage_notes(dim: &str, domain: &str, i: u32) -> String {
    match dim {
        "INV" => format!(
            "<notes>\nAdded an outlining step before drafting (variant {i}) and a guardrail requiring a quoted source for every factual claim.\nWhy: the base run skipped two requirements and asserted one unsupported figure.\n</notes>"
        ),
        "DOM" => format!(
            "<notes>\nAdded a {domain} review skill and two memory entries about defined terms and citations.\nWhy: reviewers in {domain} check definitions first; the base deliverable used undefined terms.\n</notes>"
        ),
        _ => format!(
            "<notes>\nComposed the outlining step, the {domain} review skill and a requirements-check script (composition {i}).\nDropped the per-claim quoting rule: it doubled the deliverable length without improving it.\n</notes>"
        ),
    }
}

fn stages(spec: &SweepSpec, task: &TaskSpec, mode: Mode) -> Vec<Stage> {
    let mut rng = spec.rng(task, &format!("stages-{}", mode.token()));
    let mk = |rng: &mut Rng, label: String, dim: &str, i: u32| {
        let roll = rng.unit();
        let fell_back = roll < 0.07;
        let kept_base = !fell_back && roll < 0.12;
        let attempts = if fell_back || rng.unit() < 0.15 { 2 } else { 1 };
        let error = if fell_back {
            "proposal rejected twice: always-on text exceeded the cap".to_owned()
        } else {
            String::new()
        };
        Stage {
            label,
            dim: dim.to_owned(),
            fell_back,
            kept_base,
            attempts,
            usd: (0.04 + rng.unit() * 0.32) * f64::from(attempts),
            cached: rng.unit() < 0.05,
            notes: if fell_back {
                String::new()
            } else {
                stage_notes(dim, &task.domain, i)
            },
            error,
        }
    };
    let mut out = Vec::new();
    match mode {
        Mode::Parallel => {
            for i in 1..=spec.k {
                out.push(mk(&mut rng, format!("inv{i}"), "INV", i));
            }
            for i in 1..=spec.k {
                out.push(mk(&mut rng, format!("dom{i}"), "DOM", i));
            }
            for i in 1..=spec.k {
                out.push(mk(&mut rng, format!("agg{i}"), "AGG", i));
            }
        }
        Mode::Sequential => {
            for i in 1..=spec.k {
                out.push(mk(&mut rng, format!("inv{i}"), "INV", i));
                out.push(mk(&mut rng, format!("dom{i}"), "DOM", i));
                out.push(mk(&mut rng, format!("c{i}"), "COMPOSE", i));
            }
        }
    }
    out
}

fn experience(task: &TaskSpec, label: &str) -> Experience {
    Experience {
        cue: format!("{} tasks whose statement lists several “must” requirements.", task.domain),
        lesson: "Outline the deliverable against the requirement list first, then run the requirements-check script before submitting.".to_owned(),
        evidence: format!("The run adopted from {label} met every listed requirement; the base run missed two and the selector preferred it on completeness."),
        confidence: "medium".to_owned(),
        edits: format!(
            "component: c1 system prompt\nchange: outline step before drafting\nobserved: all sections present\nverdict: keep\n\ncomponent: c6 skill {}-review\nchange: domain review checklist\nobserved: caught an undefined term\nverdict: keep\n\ncomponent: c7 scripts/check_requirements.py\nchange: requirement coverage check\nobserved: flagged one missed requirement before submission\nverdict: keep",
            domain_slug(&task.domain)
        ),
    }
}

fn judged_frac(spec: &SweepSpec, task: &TaskSpec, mode: Mode) -> Option<(u32, u32)> {
    let mut rng = spec.rng(task, &format!("judge-{}", mode.token()));
    if rng.unit() < 0.06 {
        return None;
    }
    let n = if rng.unit() < 0.5 { 10 } else { 12 };
    let base = match mode {
        Mode::Parallel => 0.55,
        Mode::Sequential => 0.5,
    } + f64::from(spec.k.min(3)) * 0.05;
    let target = (base + rng.unit() * 0.4).min(1.0) * f64::from(n);
    let passed = (0..=n).find(|p| f64::from(*p) + 0.5 > target).unwrap_or(n);
    Some((passed, n))
}

/// JobBench's weighted judging writes fractional points (addendum v1.2): `n_passed` and
/// `n_criteria` stay empty and only the raw points are set. One fixed row shows it: the first
/// JobBench task in parallel mode scores 7.5 of 10 points.
fn weighted_points(spec: &SweepSpec, task: &TaskSpec, mode: Mode) -> bool {
    spec.bench_name == "jb"
        && mode == Mode::Parallel
        && spec.tasks.first().is_some_and(|t| t.id == task.id)
}

fn row_for(spec: &SweepSpec, task: &TaskSpec, mode: Mode) -> JudgedRow {
    let (label, is_base) = chosen(spec, task, mode);
    if weighted_points(spec, task, mode) {
        return JudgedRow {
            task_id: task.id.clone(),
            mode,
            k: spec.k,
            run_id: format!("{}-{}", spec.namespace(mode), task.slug()),
            label,
            chosen_is_base: is_base,
            n_passed: None,
            n_criteria: None,
            all_pass: false,
            frac: Some(0.75),
            graded: true,
            error: String::new(),
            metric: Some("points".to_owned()),
            points_passed: Some(7.5),
            points_max: Some(10.0),
        };
    }
    let judged = judged_frac(spec, task, mode);
    JudgedRow {
        task_id: task.id.clone(),
        mode,
        k: spec.k,
        run_id: format!("{}-{}", spec.namespace(mode), task.slug()),
        label,
        chosen_is_base: is_base,
        n_passed: judged.map(|j| j.0),
        n_criteria: judged.map(|j| j.1),
        all_pass: judged.is_some_and(|(p, n)| p == n),
        frac: judged.map(|(p, n)| f64::from(p) / f64::from(n)),
        graded: judged.is_some(),
        error: if judged.is_some() {
            String::new()
        } else {
            "judge returned no verdict (timeout)".to_owned()
        },
        metric: judged.map(|_| "criteria".to_owned()),
        points_passed: judged.map(|j| f64::from(j.0)),
        points_max: judged.map(|j| f64::from(j.1)),
    }
}

fn mode_runs(task: &TaskSpec) -> bool {
    task.state != TaskState::BaseFailed && task.state != TaskState::Running
}

pub(crate) fn task_detail(spec: &SweepSpec, task: &TaskSpec) -> TaskDetail {
    let base: Vec<BaseRun> = (1..=spec.k)
        .map(|i| BaseRun {
            label: format!("base{i}"),
            run_id: format!("{}-{}-base{i}", spec.id, task.slug()),
            available: !(task.state == TaskState::BaseFailed && i == spec.k),
        })
        .collect();
    let modes = spec
        .modes
        .iter()
        .map(|&mode| {
            let runs = mode_runs(task);
            let st = if runs { stages(spec, task, mode) } else { Vec::new() };
            let (chosen_label, chosen_is_base) = chosen(spec, task, mode);
            let mut rng = spec.rng(task, "ledger");
            let short_r = task.state == TaskState::Partial && mode == Mode::Sequential;
            let session_usd: f64 = st.iter().map(|s| s.usd).sum();
            let finals: Vec<String> = labels(mode, spec.k)
                .into_iter()
                .filter(|(_, r)| *r == HarnessRole::Final)
                .map(|(l, _)| l)
                .collect();
            let mut id_map: Vec<AnonId> = finals
                .iter()
                .chain(std::iter::once(&"base".to_owned()))
                .enumerate()
                .map(|(i, l)| AnonId {
                    id: char::from(b'A' + u8::try_from(i % 26).unwrap_or(0)).to_string(),
                    label: l.clone(),
                })
                .collect();
            if chosen_label.starts_with("dom") {
                id_map.push(AnonId {
                    id: char::from(b'A' + u8::try_from(id_map.len() % 26).unwrap_or(0)).to_string(),
                    label: chosen_label.clone(),
                });
            }
            let chosen_id = id_map
                .iter()
                .find(|a| a.label == chosen_label)
                .map_or_else(|| "A".to_owned(), |a| a.id.clone());
            let selection = runs.then(|| Selection {
                ok: true,
                chosen_label: chosen_label.clone(),
                chosen_run_id: format!("{}-{}-{chosen_label}", spec.namespace(mode), task.slug()),
                chosen_is_base,
                n_candidates: u32::try_from(id_map.len()).unwrap_or(0),
                why: if spec.k == 1 {
                    String::new()
                } else if chosen_is_base {
                    "None of the edited runs addressed the task better than the reference: the compositions added structure but dropped the cost table the task asked for.".to_owned()
                } else {
                    format!("{chosen_id} covers every requirement in the task statement and cites the source rows for its figures; the others omit the risk section or leave terms undefined.")
                },
                trivial: spec.k == 1,
                fallback: false,
                dropped: Vec::new(),
                usd: if spec.k == 1 { 0.0 } else { 0.08 + rng.unit() * 0.1 },
                error: String::new(),
                evidence: "deliverable".to_owned(),
                harness_fingerprint: Some(format!("{:016x}", seed_of(&format!("{}{chosen_label}", task.id)))),
                experience: (!chosen_is_base).then(|| experience(task, &chosen_label)),
                id_map,
            });
            let harnesses = labels(mode, spec.k)
                .into_iter()
                .filter(|_| runs)
                .map(|(label, role)| HarnessRef {
                    chosen: label == chosen_label,
                    label,
                    role,
                })
                .collect();
            let row = row_for(spec, task, mode);
            ModeRecord {
                mode,
                ok: runs,
                finals: if runs { finals } else { Vec::new() },
                ledger: runs.then(|| Ledger {
                    r: 4 * spec.k - u32::from(short_r),
                    expected_r: 4 * spec.k,
                    p: 3 * spec.k,
                    expected_p: 3 * spec.k,
                    session_attempts: st.iter().map(|s| s.attempts).sum(),
                    usd: session_usd,
                    rollout_usd: 0.0,
                    session_usd,
                }),
                similarity: (runs && mode == Mode::Parallel && spec.k > 1).then(|| Similarity {
                    n: spec.k,
                    n_distinct: spec.k,
                    mean_line_jaccard: 0.45 + rng.unit() * 0.35,
                }),
                selection,
                errors: if task.state == TaskState::Partial && mode == Mode::Sequential {
                    vec!["c2 rollout timed out after 1800 s; the stage reused its candidate's harness".to_owned()]
                } else {
                    Vec::new()
                },
                harnesses,
                scores: if spec.judged && runs { vec![row] } else { Vec::new() },
                stages: st,
            }
        })
        .collect();
    TaskDetail {
        task_id: task.id.clone(),
        slug: task.slug(),
        domain: task.domain.clone(),
        k: spec.k,
        seed: spec.seed,
        status: task.state.as_str().to_owned(),
        base,
        modes,
        errors: if task.state == TaskState::BaseFailed {
            vec![format!(
                "base{}: executor exited with code 1 (bun: out of memory)",
                spec.k
            )]
        } else {
            Vec::new()
        },
        task_prompt: Some(format!(
            "# Task {}\n\nDomain: {}\n\nYou are given the documents in `inputs/`. Prepare the deliverable described below and save it as `out/deliverable.md`.\n\n- The memo must summarise the three main obligations.\n- The memo must flag any clause that conflicts with the attached policy.\n- The memo must stay under 1,200 words.\n",
            task.id, task.domain
        )),
    }
}

pub(crate) fn task_rows(spec: &SweepSpec) -> Vec<TaskRow> {
    spec.tasks
        .iter()
        .map(|t| {
            let runs = mode_runs(t);
            TaskRow {
                task_id: t.id.clone(),
                slug: t.slug(),
                domain: t.domain.clone(),
                status: t.state.as_str().to_owned(),
                errors: u32::from(matches!(
                    t.state,
                    TaskState::Partial | TaskState::BaseFailed
                )),
                modes: spec
                    .modes
                    .iter()
                    .map(|&mode| {
                        let (label, is_base) = chosen(spec, t, mode);
                        let row = row_for(spec, t, mode);
                        let st = if runs {
                            stages(spec, t, mode)
                        } else {
                            Vec::new()
                        };
                        TaskModeOutcome {
                            mode,
                            ok: runs,
                            chosen_label: runs.then_some(label),
                            chosen_is_base: is_base,
                            fell_back: u32::try_from(st.iter().filter(|s| s.fell_back).count())
                                .unwrap_or(0),
                            usd: st.iter().map(|s| s.usd).sum(),
                            score: if spec.judged && runs { row.frac } else { None },
                            all_pass: (spec.judged && runs && row.graded).then_some(row.all_pass),
                        }
                    })
                    .collect(),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------------------
// Scores, report, library, audit

pub(crate) fn scores(spec: &SweepSpec) -> Scores {
    let rows = if spec.judged {
        spec.tasks
            .iter()
            .filter(|t| mode_runs(t))
            .flat_map(|t| spec.modes.iter().map(move |&m| row_for(spec, t, m)))
            .collect()
    } else {
        Vec::new()
    };
    let base_rows = if spec.base_judged {
        spec.tasks
            .iter()
            .flat_map(|t| {
                (1..=spec.k).map(move |i| {
                    let mut rng = spec.rng(t, &format!("basejudge{i}"));
                    let n = 10;
                    let passed = 3 + u32::try_from(rng.below(6)).unwrap_or(0);
                    BaseJudgedRow {
                        task_id: t.id.clone(),
                        label: format!("base{i}"),
                        run_id: format!("{}-{}-base{i}", spec.id, t.slug()),
                        n_passed: Some(passed),
                        n_criteria: Some(n),
                        all_pass: Some(passed == n),
                        points_passed: Some(f64::from(passed)),
                        points_max: Some(f64::from(n)),
                    }
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    Scores { rows, base_rows }
}

pub(crate) fn headline(spec: &SweepSpec) -> Option<Vec<ModeScore>> {
    if !spec.judged {
        return None;
    }
    let s = scores(spec);
    Some(
        spec.modes
            .iter()
            .map(|&m| {
                let graded: Vec<&JudgedRow> =
                    s.rows.iter().filter(|r| r.mode == m && r.graded).collect();
                let n = graded.len();
                let mean = |f: &dyn Fn(&JudgedRow) -> f64| {
                    (n > 0).then(|| graded.iter().map(|r| f(r)).sum::<f64>() / n as f64 * 100.0)
                };
                ModeScore {
                    mode: m,
                    k: spec.k,
                    graded: u32::try_from(n).unwrap_or(0),
                    score: mean(&|r| r.frac.unwrap_or(0.0)),
                    all_pass: mean(&|r| if r.all_pass { 1.0 } else { 0.0 }),
                }
            })
            .collect(),
    )
}

pub(crate) fn summary(spec: &SweepSpec) -> SweepSummary {
    let count = |s: TaskState| {
        u32::try_from(spec.tasks.iter().filter(|t| t.state == s).count()).unwrap_or(0)
    };
    let usd: f64 = task_rows(spec)
        .iter()
        .flat_map(|r| r.modes.iter())
        .map(|m| m.usd)
        .sum();
    SweepSummary {
        id: spec.id.clone(),
        bench_name: spec.bench_name.clone(),
        cell: spec.cell,
        substrate: spec.substrate(),
        k: spec.k,
        seed: spec.seed,
        tag: None,
        modes: spec.modes.clone(),
        tasks: u32::try_from(spec.tasks.len()).unwrap_or(0),
        ok: count(TaskState::Ok),
        partial: count(TaskState::Partial),
        failed: count(TaskState::BaseFailed),
        usd,
        headline: headline(spec),
        updated_at: Some(spec.updated_at.clone()),
    }
}

pub(crate) fn report(spec: &SweepSpec) -> ScoreReport {
    let s = scores(spec);
    let rows = task_rows(spec);
    let summaries: Vec<ModeSummary> = spec
        .modes
        .iter()
        .map(|&m| {
            let mine: Vec<&JudgedRow> = s.rows.iter().filter(|r| r.mode == m).collect();
            let graded: Vec<&&JudgedRow> = mine.iter().filter(|r| r.graded).collect();
            let n = graded.len();
            let score = (n > 0).then(|| {
                graded.iter().map(|r| r.frac.unwrap_or(0.0)).sum::<f64>() / n as f64 * 100.0
            });
            let all_pass = (n > 0)
                .then(|| graded.iter().filter(|r| r.all_pass).count() as f64 / n as f64 * 100.0);
            let outcomes: Vec<&TaskModeOutcome> = rows
                .iter()
                .flat_map(|r| r.modes.iter())
                .filter(|o| o.mode == m && o.ok)
                .collect();
            let kept_base = outcomes.iter().filter(|o| o.chosen_is_base).count();
            let chose_candidate = outcomes
                .iter()
                .filter(|o| {
                    o.chosen_label
                        .as_deref()
                        .is_some_and(|l| l.starts_with("inv") || l.starts_with("dom"))
                })
                .count();
            ModeSummary {
                mode: m,
                k: spec.k,
                n: u32::try_from(n).unwrap_or(0),
                n_ungraded: u32::try_from(mine.len() - n).unwrap_or(0),
                score,
                all_pass,
                kept_base: u32::try_from(kept_base).unwrap_or(0),
                chose_final: u32::try_from(outcomes.len() - kept_base - chose_candidate)
                    .unwrap_or(0),
                chose_candidate: u32::try_from(chose_candidate).unwrap_or(0),
                fell_back: outcomes.iter().map(|o| o.fell_back).sum(),
                usd: outcomes.iter().map(|o| o.usd).sum(),
                metrics: vec!["criteria".to_owned()],
            }
        })
        .collect();
    let mut notes = Vec::new();
    if spec.k == 1 {
        notes.push("NOTE: k=1 adopts each task's single final without selection; differences of a few points between sweeps are within the noise floor.".to_owned());
    }
    for sm in &summaries {
        if sm.kept_base > 0 {
            notes.push(format!(
                "NOTE: {}: the selector kept the base harness on {} task(s).",
                sm.mode.as_cli(),
                sm.kept_base
            ));
        }
        if sm.n_ungraded > 0 {
            notes.push(format!(
                "WARNING: {}: {} task(s) ungraded (judge error); excluded from the denominators, not counted as zero.",
                sm.mode.as_cli(),
                sm.n_ungraded
            ));
        }
        if sm.fell_back > 0 {
            notes.push(format!(
                "WARNING: {}: {} stage(s) fell back after two rejected proposals.",
                sm.mode.as_cli(),
                sm.fell_back
            ));
        }
    }
    let base_score = (!s.base_rows.is_empty()).then(|| {
        s.base_rows
            .iter()
            .map(|r| {
                f64::from(r.n_passed.unwrap_or(0)) / f64::from(r.n_criteria.unwrap_or(1).max(1))
            })
            .sum::<f64>()
            / s.base_rows.len() as f64
            * 100.0
    });
    let mut text = format!(
        "{} · {} · k={}\n",
        spec.bench_name,
        spec.substrate().display_name(),
        spec.k
    );
    let _ = writeln!(
        text,
        "{:<11} {:>3} {:>9} {:>9} {:>6} {:>6} {:>6} {:>8}",
        "mode", "n", "score", "all-pass", "final", "cand", "base", "usd"
    );
    for sm in &summaries {
        let _ = writeln!(
            text,
            "{:<11} {:>3} {:>8.1}% {:>8.1}% {:>6} {:>6} {:>6} {:>8.2}",
            sm.mode.as_cli(),
            sm.n,
            sm.score.unwrap_or(0.0),
            sm.all_pass.unwrap_or(0.0),
            sm.chose_final,
            sm.chose_candidate,
            sm.kept_base,
            sm.usd
        );
    }
    if let Some(b) = base_score {
        let _ = writeln!(text, "base executions: {b:.1}%");
    }
    for n in &notes {
        let _ = writeln!(text, "{n}");
    }
    ScoreReport {
        title: format!("{} — {}", spec.id, spec.bench_name),
        bench_name: spec.bench_name.clone(),
        summaries,
        notes,
        base_score,
        text,
    }
}

pub(crate) fn library(spec: &SweepSpec) -> Vec<LibraryDomain> {
    let mut out: BTreeMap<(Mode, String), Vec<LibraryEntry>> = BTreeMap::new();
    for t in spec.tasks.iter().filter(|t| mode_runs(t)) {
        for &m in &spec.modes {
            let (label, is_base) = chosen(spec, t, m);
            if is_base {
                continue;
            }
            let tree = harness_tree(spec, t, m, &label);
            let components = tree.map_or_else(String::new, |tr| {
                format!(
                    "{} skills, {} scripts, {} sub-agents; {} always-on chars",
                    tr.census.skills,
                    tr.census.scripts,
                    tr.census.sub_agents,
                    tr.census.always_on_chars
                )
            });
            out.entry((m, t.domain.clone())).or_default().push(LibraryEntry {
                task_slug: t.slug(),
                adopted_label: label.clone(),
                run_id: format!("{}-{}-{label}", spec.namespace(m), t.slug()),
                components,
                why: "Covered every requirement and cited its sources; the other runs omitted a required section.".to_owned(),
                experience: Some(experience(t, &label)),
            });
        }
    }
    out.into_iter()
        .map(|((mode, domain), entries)| LibraryDomain {
            mode,
            domain,
            entries,
        })
        .collect()
}

pub(crate) fn audit_sessions(spec: &SweepSpec) -> Vec<AuditSession> {
    let mut out = Vec::new();
    for t in spec.tasks.iter().filter(|t| mode_runs(t)).take(6) {
        for &m in &spec.modes {
            for st in stages(spec, t, m) {
                let mut rng = spec.rng(t, &format!("audit-{}-{}", m.token(), st.label));
                out.push(AuditSession {
                    id: format!(
                        "{}.a{}.{:016x}",
                        st.label,
                        st.attempts,
                        seed_of(&format!("{}{}{}", t.id, m.token(), st.label))
                    ),
                    label: st.label.clone(),
                    ok: !st.fell_back,
                    channel: "improver".to_owned(),
                    model: spec.models.1.clone(),
                    effort: "high".to_owned(),
                    attempts: st.attempts,
                    wall_s: 40.0 + rng.unit() * 260.0,
                    cost_usd: st.usd,
                    error: if st.fell_back {
                        "proposal rejected twice".to_owned()
                    } else {
                        String::new()
                    },
                    parts: vec![
                        "mission".to_owned(),
                        "system".to_owned(),
                        "response".to_owned(),
                    ],
                });
            }
            if spec.k > 1 {
                out.push(AuditSession {
                    id: format!(
                        "select.a1.{:016x}",
                        seed_of(&format!("{}{}select", t.id, m.token()))
                    ),
                    label: "select".to_owned(),
                    ok: true,
                    channel: "selector".to_owned(),
                    model: spec.models.1.clone(),
                    effort: "medium".to_owned(),
                    attempts: 1,
                    wall_s: 35.0,
                    cost_usd: 0.11,
                    error: String::new(),
                    parts: vec!["mission".to_owned(), "response".to_owned()],
                });
            }
        }
    }
    out
}

pub(crate) fn audit_part(spec: &SweepSpec, session: &str, part: &str) -> Option<String> {
    let s = audit_sessions(spec).into_iter().find(|s| s.id == session)?;
    if !s.parts.iter().any(|p| p == part) {
        return None;
    }
    Some(match part {
        "mission" => format!(
            "# Mission ({})\n\nYou improve the harness of an agent that just attempted the task below.\nRead the task, the current harness and the evidence from the probe runs.\nPropose ONE coherent edit as a complete replacement of the files you change.\n\nConstraints:\n- Always-on text (c1-c4) must stay under the surface cap.\n- Explain the change in <notes>.\n\n## Evidence\n\n- base1 deliverable missed requirements 2 and 3.\n- The judge rubric is NOT available to you.\n",
            s.label
        ),
        "system" => "You are a harness engineer. You never change the model; you change the instructions, tools guidance, memory and helper code around it. Prefer small, testable edits.".to_owned(),
        _ => format!(
            "I inspected the probe trajectory: the agent never re-read the task after drafting.\n\n<edit path=\"systemprompt.md\">… outline step added …</edit>\n\n<notes>\nAdded an outlining step and a requirements check ({}).\n</notes>\n",
            s.label
        ),
    })
}

// ---------------------------------------------------------------------------------------
// Rollouts

fn trajectory(task: &TaskSpec, label: &str) -> String {
    let mut t = format!("task: {} ({})\nharness: {label}\n\n", task.id, task.domain);
    let steps = [
        (
            "[ACTION] read inputs/brief.md",
            "[OBSERVATION] 84 lines: three obligations, a policy reference, a 1,200-word limit.",
        ),
        (
            "[ACTION] read inputs/policy.pdf.txt",
            "[OBSERVATION] 212 lines. Section 4 restricts automatic renewal.",
        ),
        (
            "[ACTION] bash wc -w out/draft.md",
            "[ERROR] out/draft.md: No such file or directory",
        ),
        (
            "[ACTION] write out/draft.md (1,034 words)",
            "[OBSERVATION] wrote 6,912 bytes",
        ),
        (
            "[ACTION] bash python scripts/check_requirements.py inputs/brief.md out/draft.md",
            "[OBSERVATION] missing: flag any clause that conflicts with the attached policy",
        ),
        (
            "[ACTION] write out/deliverable.md (1,118 words)",
            "[OBSERVATION] wrote 7,440 bytes",
        ),
    ];
    for (i, (a, o)) in steps.iter().enumerate() {
        let _ = writeln!(t, "step {:02} {a}", i + 1);
        let _ = writeln!(t, "  arguments elided for brevity");
        let _ = writeln!(t, "step {:02} {o}", i + 1);
        t.push('\n');
    }
    t.push_str("done: deliverable written to out/deliverable.md\n");
    t
}

/// Entries under `path` in a rollout directory.
pub(crate) fn rollout_files(path: &str) -> Option<Vec<FileEntry>> {
    let e = |name: &str, path: &str, kind: FileKind, bytes: u64| FileEntry {
        name: name.to_owned(),
        path: path.to_owned(),
        kind,
        bytes,
    };
    match path.trim_matches('/') {
        "" => Some(vec![
            e("logs", "logs", FileKind::Dir, 0),
            e("out", "out", FileKind::Dir, 0),
            e("inputs", "inputs", FileKind::Symlink, 0),
            e("meta.json", "meta.json", FileKind::File, 412),
            e("trajectory.md", "trajectory.md", FileKind::File, 1_830),
        ]),
        "out" => Some(vec![
            e(
                "deliverable.md",
                "out/deliverable.md",
                FileKind::File,
                7_440,
            ),
            e(
                "deliverable.docx",
                "out/deliverable.docx",
                FileKind::File,
                18_432,
            ),
        ]),
        "logs" => Some(vec![e(
            "executor.log",
            "logs/executor.log",
            FileKind::File,
            4_718_592,
        )]),
        _ => None,
    }
}

pub(crate) fn rollout_file(task: &TaskSpec, label: &str, path: &str) -> Option<FileContent> {
    let path = path.trim_matches('/');
    let (text, bytes, truncated, mime) = match path {
        "trajectory.md" => (Some(trajectory(task, label)), 1_830, false, "text/markdown"),
        "meta.json" => (
            Some(format!(
                "{{\n  \"task\": \"{}\",\n  \"label\": \"{label}\",\n  \"exit_code\": 0,\n  \"wall_s\": 412.7,\n  \"tokens_in\": 182340,\n  \"tokens_out\": 9214\n}}\n",
                task.id
            )),
            412,
            false,
            "application/json",
        ),
        "out/deliverable.md" => (
            Some(format!(
                "# Memo: {}\n\nAssumption: the attached policy is the version dated this quarter.\n\n## Obligations\n\n1. Deliver the monthly report by the fifth business day.\n2. Keep customer data inside the agreed regions.\n3. Give 60 days' notice before price changes.\n\n## Conflicts with the policy\n\n- Clause 9.2 renews automatically for three years; policy section 4 caps renewals at one year.\n\n## Recommendation\n\nNegotiate clause 9.2 down to a one-year renewal before signature.\n",
                task.id
            )),
            7_440,
            false,
            "text/markdown",
        ),
        "out/deliverable.docx" => (
            None,
            18_432,
            false,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        ),
        "logs/executor.log" => {
            let mut t = String::new();
            for i in 0..400 {
                let _ = writeln!(
                    t,
                    "{:05} executor: tick {i}: tokens_in=+{} tokens_out=+{}",
                    i,
                    400 + i % 37,
                    20 + i % 11
                );
            }
            (Some(t), 4_718_592, true, "text/plain")
        }
        _ => return None,
    };
    Some(FileContent {
        path: path.to_owned(),
        bytes,
        text,
        truncated,
        mime: mime.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use h2w_api::{Bench, Substrate};

    fn spec(k: u32) -> SweepSpec {
        SweepSpec {
            id: format!("lab-oc-agg{k}"),
            bench_name: "lab".into(),
            cell: Some(Cell {
                bench: Bench::Lab,
                subset: None,
                substrate: Substrate::Oc,
            }),
            k,
            seed: None,
            modes: vec![Mode::Parallel, Mode::Sequential],
            tasks: vec![
                TaskSpec {
                    id: "contracts/nda-review-001".into(),
                    domain: "Contracts".into(),
                    state: TaskState::Ok,
                },
                TaskSpec {
                    id: "employment/handbook-002".into(),
                    domain: "Employment".into(),
                    state: TaskState::Partial,
                },
            ],
            judged: true,
            base_judged: true,
            updated_at: "2026-10-20T10:00:00Z".into(),
            models: ("solver".into(), "improver".into()),
        }
    }

    #[test]
    fn jobbench_weighted_points_row() {
        let mut s = spec(2);
        s.bench_name = "jb".into();
        let row = row_for(&s, &s.tasks[0], Mode::Parallel);
        assert_eq!((row.n_passed, row.n_criteria), (None, None));
        assert_eq!((row.points_passed, row.points_max), (Some(7.5), Some(10.0)));
        assert_eq!(row.frac, Some(0.75));
        assert!(row.graded && !row.all_pass);
        // Every other row keeps integer counts with matching points.
        let other = row_for(&s, &s.tasks[0], Mode::Sequential);
        if other.graded {
            assert_eq!(other.points_passed, other.n_passed.map(f64::from));
        }
        let lab = row_for(&spec(2), &spec(2).tasks[0], Mode::Parallel);
        assert!(lab.n_passed.is_some() || !lab.graded);
    }

    #[test]
    fn detail_matches_budget_identity() {
        let s = spec(2);
        let d = task_detail(&s, &s.tasks[0]);
        assert_eq!(d.base.len(), 2);
        for m in &d.modes {
            let l = m.ledger.as_ref().unwrap();
            assert_eq!(l.expected_r, 8);
            assert_eq!(l.expected_p, 6);
            assert_eq!(m.stages.len(), 6);
            assert!(m.harnesses.iter().any(|h| h.chosen));
        }
        assert_eq!(s.namespace(Mode::Sequential), "lab-oc-seq2");
    }

    #[test]
    fn harness_diff_is_consistent() {
        let s = spec(2);
        let t = &s.tasks[0];
        let d = harness_diff(&s, t, Mode::Parallel, "base", "agg2").unwrap();
        assert!(d.files.iter().any(|f| f.change == ChangeKind::Added));
        assert!(d.files.iter().any(|f| f.change == ChangeKind::Modified));
        assert!(d.always_on_char_delta > 0);
        let tree = harness_tree(&s, t, Mode::Parallel, "agg2").unwrap();
        assert!(tree.census.always_on_chars < tree.census.always_on_max);
        assert!(harness_tree(&s, t, Mode::Parallel, "nope").is_none());
        let bundle = export(&s, t, Mode::Parallel, "agg2", ExportTarget::Opencode).unwrap();
        assert!(bundle.files.iter().any(|f| f.path == "AGENTS.md"));
    }

    #[test]
    fn report_and_headline() {
        let s = spec(1);
        let r = report(&s);
        assert_eq!(r.summaries.len(), 2);
        assert!(r.notes.iter().any(|n| n.contains("noise floor")));
        assert!(r.base_score.is_some());
        let h = headline(&s).unwrap();
        assert!(
            h.iter()
                .all(|m| m.score.is_some_and(|x| (0.0..=100.0).contains(&x)))
        );
        assert!(!library(&s).is_empty());
        let sessions = audit_sessions(&s);
        assert!(audit_part(&s, &sessions[0].id, "mission").is_some());
        assert!(audit_part(&s, &sessions[0].id, "bogus").is_none());
    }

    #[test]
    fn rollouts() {
        let s = spec(1);
        assert_eq!(rollout_files("").unwrap().len(), 5);
        assert!(
            rollout_file(&s.tasks[0], "agg1", "out/deliverable.docx")
                .unwrap()
                .text
                .is_none()
        );
        assert!(
            rollout_file(&s.tasks[0], "agg1", "logs/executor.log")
                .unwrap()
                .truncated
        );
        assert!(rollout_file(&s.tasks[0], "agg1", "../etc").is_none());
    }
}
