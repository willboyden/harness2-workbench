//! Experiments and everything harness2 writes into them.
//!
//! An **experiment** is one `HARNESS2_RUNS_DIR`. Inside it, a **sweep** is one
//! (benchmark cell, k, seed, tag) group, identified by its *summary namespace* — the
//! parallel-mode namespace name (`lab-oc-agg1-s7`), because harness2 keeps every task's
//! `summary.json` and the sweep's `judged.json` there for both modes. Each task has, per
//! mode, a set of **harnesses** (component trees, labelled `base`, `inv1`, `dom1`, `agg1`,
//! `c1`, ...) and **rollouts** (executions of those harnesses).

use serde::{Deserialize, Serialize};

use crate::{Cell, Mode, Substrate};

/// Experiments created by the demo have a different on-disk layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ExperimentKind {
    /// Benchmark runs: `<bench>[/<subset>]/adapt/<namespace>/...`.
    Standard,
    /// `harness2-demo` output: `runs/<namespace>/...`, `library/k<k>/...`.
    Demo,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Experiment {
    /// `^[a-z0-9][a-z0-9_-]{0,62}$`; also the directory name.
    pub name: String,
    /// Standard benchmark experiment or demo output.
    pub kind: ExperimentKind,
    /// Free-text description.
    pub description: String,
    /// Short labels for filtering.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Model profile pinned at creation. Runs with a different profile are refused, because
    /// harness2 refuses to resume or judge a namespace under different models.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Creation time (RFC 3339, UTC).
    pub created_at: String,
    /// Display name of the principal that created it.
    pub created_by: String,
    /// Hidden from default listings; nothing is deleted.
    #[serde(default)]
    pub archived: bool,
    /// Sweeps found on disk (cheap header data only).
    pub sweeps: Vec<SweepSummary>,
    /// Total bytes on disk, when computed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
}

/// Body of `POST /api/v1/experiments`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CreateExperimentRequest {
    /// Experiment name: `^[a-z0-9][a-z0-9_-]{0,62}$` (also the directory name).
    pub name: String,
    /// Free-text description.
    #[serde(default)]
    pub description: String,
    /// Short labels for filtering.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Pin a model profile now (else the first run pins it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// `standard` for benchmark runs, `demo` for harness2-demo output.
    #[serde(default = "default_kind")]
    pub kind: ExperimentKind,
}

fn default_kind() -> ExperimentKind {
    ExperimentKind::Standard
}

/// Body of `PATCH /api/v1/experiments/{exp}`. Absent fields are unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UpdateExperimentRequest {
    /// New description; absent = unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// New tag list (replaces the old one); absent = unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// Archive or unarchive; absent = unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
}

/// One sweep: a (cell, k, seed, tag) group within an experiment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SweepSummary {
    /// The summary namespace, e.g. `lab-oc-agg1-s7` or `demo-oc-agg2`.
    pub id: String,
    /// `jb`, `lab`, `wb-office`, `wb-web`, `wb-code` or `demo`.
    pub bench_name: String,
    /// `None` for the demo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cell: Option<Cell>,
    /// Executor framework (`oc` OpenCode or `cx` Codex).
    pub substrate: Substrate,
    /// Task-level recursion budget.
    pub k: u32,
    /// Stream-order seed; omit for upstream's sorted order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// Modes with at least one task record.
    pub modes: Vec<Mode>,
    /// Tasks with a summary on disk.
    pub tasks: u32,
    /// Task statuses from `summary.json`: `ok`/`skipped` count as ok.
    pub ok: u32,
    /// Tasks recorded as `partial`.
    pub partial: u32,
    /// Tasks recorded as failed (`base-failed` and other states).
    pub failed: u32,
    /// Improver + selection spend recorded in the summaries (USD).
    pub usd: f64,
    /// Present once `judged.json` exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headline: Option<Vec<ModeScore>>,
    /// Newest summary modification time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

/// One figure per mode for list views.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ModeScore {
    /// Recursion mode.
    pub mode: Mode,
    /// Task-level recursion budget.
    pub k: u32,
    /// Graded tasks (the denominator).
    pub graded: u32,
    /// Mean partial credit in percent (0..100); `None` when nothing graded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    /// All-criteria pass rate in percent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all_pass: Option<f64>,
}

/// A row in the task list of a sweep.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TaskRow {
    /// Task id as harness2 reports it.
    pub task_id: String,
    /// Directory name (`/` replaced by `__`); use this in URLs.
    pub slug: String,
    /// Domain (practice area, occupation or category) the task belongs to.
    pub domain: String,
    /// `ok`, `skipped`, `partial`, `base-failed`, `running` (as harness2 records it).
    pub status: String,
    /// Per-mode outcome of the task.
    pub modes: Vec<TaskModeOutcome>,
    /// Number of errors recorded for the task.
    pub errors: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TaskModeOutcome {
    /// Recursion mode.
    pub mode: Mode,
    /// The mode finished and a harness was adopted.
    pub ok: bool,
    /// Label of the adopted harness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chosen_label: Option<String>,
    /// The adopted harness is an unchanged base.
    pub chosen_is_base: bool,
    /// Stages that fell back to the inherited harness.
    pub fell_back: u32,
    /// Recorded spend in US dollars.
    pub usd: f64,
    /// Judged partial credit 0..1 for the chosen run, when judged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    /// Every criterion passed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all_pass: Option<bool>,
}

/// Everything known about one task, from `summary.json` plus the judge rows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TaskDetail {
    /// Task id as harness2 reports it.
    pub task_id: String,
    /// Task directory name (`/` replaced by `__`); use in URLs.
    pub slug: String,
    /// Domain (practice area, occupation or category) the task belongs to.
    pub domain: String,
    /// Task-level recursion budget.
    pub k: u32,
    /// Stream-order seed; omit for upstream's sorted order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// Status.
    pub status: String,
    /// Reference executions on the unmodified harness (labels `base1..basek`).
    pub base: Vec<BaseRun>,
    /// Recursion modes, non-empty and without duplicates.
    pub modes: Vec<ModeRecord>,
    /// Error messages recorded for it.
    pub errors: Vec<String>,
    /// The task prompt as the agent saw it (from the improver context), when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_prompt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BaseRun {
    /// Harness label (`base`, `inv1`, `dom1`, `agg1`, `c1`, ...).
    pub label: String,
    /// harness2 run id (`<namespace>/<task>/<label>`).
    pub run_id: String,
    /// Whether the run directory exists inside the experiment.
    pub available: bool,
}

/// One recursion mode's record for a task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ModeRecord {
    /// Recursion mode.
    pub mode: Mode,
    /// Whether it completed successfully.
    pub ok: bool,
    /// Labels of this mode's composed finals (`agg1..aggk` or `c1..ck`).
    pub finals: Vec<String>,
    /// Improvement stages in execution order.
    pub stages: Vec<Stage>,
    /// Rollout and session counts against the budget identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ledger: Option<Ledger>,
    /// Pairwise line-Jaccard over the finals (parallel mode only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub similarity: Option<Similarity>,
    /// The blind selector's decision for this mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<Selection>,
    /// Error messages recorded for it.
    pub errors: Vec<String>,
    /// Harness labels available on disk for this mode (always includes `base`).
    pub harnesses: Vec<HarnessRef>,
    /// Judge rows for this task and mode.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scores: Vec<JudgedRow>,
}

/// An improvement stage: one improver session (with at most one retry).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Stage {
    /// Harness label (`base`, `inv1`, `dom1`, `agg1`, `c1`, ...).
    pub label: String,
    /// `INV` (general edit), `DOM` (domain edit), `AGG` (parallel composition) or
    /// `COMPOSE` (sequential composition).
    pub dim: String,
    /// The proposal was rejected twice and the stage inherited the current harness.
    pub fell_back: bool,
    /// The accepted proposal changed nothing.
    pub kept_base: bool,
    pub attempts: u32,
    /// Recorded spend in US dollars.
    pub usd: f64,
    pub cached: bool,
    /// The improver's `<notes>`: what it changed and why.
    pub notes: String,
    /// Error message, empty when there was none.
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Ledger {
    /// Rollouts recorded; must equal `expected_r` (4k).
    pub r: u32,
    pub expected_r: u32,
    /// Improver sessions recorded; must equal `expected_p` (3k).
    pub p: u32,
    pub expected_p: u32,
    pub session_attempts: u32,
    /// Recorded spend in US dollars.
    pub usd: f64,
    pub rollout_usd: f64,
    pub session_usd: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Similarity {
    pub n: u32,
    pub n_distinct: u32,
    pub mean_line_jaccard: f64,
}

/// The blind selector's decision and the reflection that followed it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Selection {
    /// Whether it completed successfully.
    pub ok: bool,
    pub chosen_label: String,
    pub chosen_run_id: String,
    pub chosen_is_base: bool,
    /// The selector saw anonymised candidates; this maps its ids (`A`, `B`, ...) to labels.
    pub id_map: Vec<AnonId>,
    pub n_candidates: u32,
    pub why: String,
    /// k = 1: the single final was adopted without a selector session.
    pub trivial: bool,
    pub fallback: bool,
    pub dropped: Vec<String>,
    /// Recorded spend in US dollars.
    pub usd: f64,
    /// Error message, empty when there was none.
    pub error: String,
    /// `deliverable` or `trajectory`: what the selector compared.
    pub evidence: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_fingerprint: Option<String>,
    /// The lesson the reflector distilled, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experience: Option<Experience>,
}

/// One entry of the selector's anonymisation map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AnonId {
    /// What the selector saw (`A`, `B`, ...).
    pub id: String,
    /// The harness label behind it.
    pub label: String,
}

/// A lesson the reflector distilled from an adoption (stored in the library).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Experience {
    /// When the lesson applies.
    pub cue: String,
    /// What to do differently.
    pub lesson: String,
    /// The observed contrast that supports the lesson.
    pub evidence: String,
    /// How sure the reflector is, in its own words.
    #[serde(default)]
    pub confidence: String,
    /// Per-edit account (`component`, `change`, `observed`, `verdict` blocks).
    #[serde(default)]
    pub edits: String,
}

/// Role a harness label plays in the recursion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum HarnessRole {
    /// The unmodified starting harness (`base`).
    Base,
    /// A probed candidate (`inv*`, `dom*`).
    Candidate,
    /// A composed final (`agg*`, `c*`).
    Final,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct HarnessRef {
    /// Harness label (`base`, `inv1`, `dom1`, `agg1`, `c1`, ...).
    pub label: String,
    /// Role.
    pub role: HarnessRole,
    /// The selector adopted this one.
    pub chosen: bool,
}

/// Agent framework surface a harness tree targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    /// `systemprompt.md`, `guardrails.md`, `LongTermMEMORY.md`,
    /// `tool_descriptions/tool_guidance.md`, `skills/`, `sub_agents/`, `scripts/`, `plugin/`.
    Opencode,
    /// `AGENTS.md`, `.agents/skills/`, `sub_agents/`, `scripts/`, `.codex/hooks.json`.
    Codex,
}

/// The eight editable harness components (c1..c8 upstream).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Component {
    /// c1. On Codex, `AGENTS.md` carries c1-c4.
    SystemPrompt,
    /// c2
    Guardrails,
    /// c3
    Memory,
    /// c4
    ToolGuidance,
    /// c5: delegated agents with their own context.
    Subagents,
    /// c6: on-demand workflows.
    Skills,
    /// c7: deterministic helper scripts.
    Scripts,
    /// c8: OpenCode plugins / Codex hooks, acting inside the tool loop.
    Plugins,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct HarnessFile {
    /// Path relative to the harness root, `/`-separated.
    pub path: String,
    /// Which of the eight harness components the file belongs to.
    pub component: Component,
    /// Size in bytes.
    pub bytes: u64,
    /// UTF-8 text (lossy). Omitted in listings; present in `GET .../harnesses/{label}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// Component counts, mirroring upstream `tree.validate()`'s census.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Census {
    pub always_on_chars: u32,
    /// The surface's cap (24,000 OpenCode, 8,000 Codex).
    pub always_on_max: u32,
    pub n_files: u32,
    pub skills: u32,
    pub sub_agents: u32,
    pub scripts: u32,
    pub plugins: u32,
    pub hooks: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct HarnessTree {
    /// Harness label (`base`, `inv1`, `dom1`, `agg1`, `c1`, ...).
    pub label: String,
    /// Role.
    pub role: HarnessRole,
    /// Agent framework the harness targets.
    pub surface: Surface,
    /// Files of the harness.
    pub files: Vec<HarnessFile>,
    /// Component counts and the always-on budget.
    pub census: Census,
    /// harness2's 16-hex fingerprint of the tree (as in `harness_fingerprint`).
    pub fingerprint: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Removed,
    Modified,
    Unchanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FileDiff {
    /// Relative, `/`-separated path.
    pub path: String,
    /// Which of the eight harness components the file belongs to.
    pub component: Component,
    /// How the file changed.
    pub change: ChangeKind,
    /// Unified diff (3 lines of context) for `modified`; full text for added/removed.
    pub unified: String,
    /// Lines added.
    pub lines_added: u32,
    /// Lines removed.
    pub lines_removed: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct HarnessDiff {
    /// Label diffed from.
    pub from: String,
    /// Label diffed to.
    pub to: String,
    /// Changed files first, then unchanged ones (with empty `unified`).
    pub files: Vec<FileDiff>,
    pub always_on_char_delta: i64,
}

/// Body of `POST /api/v1/harness/validate`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ValidateHarnessRequest {
    /// Agent framework the harness targets.
    pub surface: Surface,
    /// Relative path and content of every file.
    pub files: Vec<crate::FileText>,
    /// Whether sub-agents are allowed on this deployment (upstream: false on LAB).
    #[serde(default = "default_true")]
    pub allow_subagents: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ValidationReport {
    /// Whether it completed successfully.
    pub ok: bool,
    /// Upstream's actionable messages, one per problem.
    pub errors: Vec<String>,
    /// Files outside the editable surface (ignored by harness2).
    pub off_surface: Vec<String>,
    /// Component counts and the always-on budget.
    pub census: Census,
}

/// Target layout for exporting a harness into a project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ExportTarget {
    /// The harness tree exactly as harness2 stores it.
    Raw,
    /// An OpenCode project: `AGENTS.md` (always-on text), `.opencode/skills/`,
    /// `.opencode/agent/`, `.opencode/plugin/`, `scripts/`.
    Opencode,
    /// A Codex project: `AGENTS.md`, `.agents/skills/`, `.codex/hooks.json`, `scripts/`.
    Codex,
}

/// `GET .../harnesses/{label}/export?target=...` (JSON form).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ExportBundle {
    /// Export layout.
    pub target: ExportTarget,
    /// Harness label (`base`, `inv1`, `dom1`, `agg1`, `c1`, ...).
    pub label: String,
    /// Relative path and content, ready to write into a project root.
    pub files: Vec<crate::FileText>,
    /// What the user should know (e.g. "plugins need OpenCode >= 1.14").
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    File,
    Dir,
    /// Symlinks are listed but never followed.
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FileEntry {
    /// Name.
    pub name: String,
    /// Path relative to the browsed root.
    pub path: String,
    /// Kind.
    pub kind: FileKind,
    /// Size in bytes.
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FileContent {
    /// Relative, `/`-separated path.
    pub path: String,
    /// Size in bytes.
    pub bytes: u64,
    /// `None` for binary files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// The file was larger than the read cap and `text` holds only the start.
    pub truncated: bool,
    /// Best-effort MIME type from the extension.
    pub mime: String,
}

/// One row of `judged.json` (the selected run of a task, scored after selection).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct JudgedRow {
    /// Task id as harness2 reports it.
    pub task_id: String,
    /// Recursion mode.
    pub mode: Mode,
    /// Task-level recursion budget.
    pub k: u32,
    /// Harness label (`base`, `inv1`, `dom1`, `agg1`, `c1`, ...).
    pub label: String,
    /// harness2 run id (`<namespace>/<task>/<label>`).
    pub run_id: String,
    pub chosen_is_base: bool,
    /// Criteria passed (integer judges only; see `frac`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n_passed: Option<u32>,
    /// Criteria graded (integer judges only; see `frac`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n_criteria: Option<u32>,
    /// Every criterion passed.
    pub all_pass: bool,
    /// `n_passed / n_criteria`, `None` when ungraded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frac: Option<f64>,
    /// The judge succeeded (not: the run did well).
    pub graded: bool,
    /// Error message, empty when there was none.
    #[serde(default)]
    pub error: String,
    /// `metric` unit from the judge (`points`, `criteria`), when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metric: Option<String>,
}

/// One row of `base_judged.json` (a reference execution, scored with `--bases`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BaseJudgedRow {
    /// Task id as harness2 reports it.
    pub task_id: String,
    /// Harness label (`base`, `inv1`, `dom1`, `agg1`, `c1`, ...).
    pub label: String,
    /// harness2 run id (`<namespace>/<task>/<label>`).
    pub run_id: String,
    /// Criteria passed (integer judges only; see `frac`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n_passed: Option<u32>,
    /// Criteria graded (integer judges only; see `frac`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n_criteria: Option<u32>,
    /// Every criterion passed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all_pass: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Scores {
    /// Rows.
    pub rows: Vec<JudgedRow>,
    pub base_rows: Vec<BaseJudgedRow>,
}

/// Port of upstream `core.report.Summary`: one per (mode, k).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ModeSummary {
    /// Recursion mode.
    pub mode: Mode,
    /// Task-level recursion budget.
    pub k: u32,
    /// Graded tasks (the denominator).
    pub n: u32,
    /// Rows the judge could not grade (excluded from the score, not zero).
    pub n_ungraded: u32,
    /// Mean partial credit, percent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    /// All-criteria pass rate, percent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all_pass: Option<f64>,
    /// Count where the adopted harness was the unchanged base.
    pub kept_base: u32,
    /// Tasks where the selector picked a composed final.
    pub chose_final: u32,
    /// Tasks where the selector picked a probed candidate.
    pub chose_candidate: u32,
    /// Stages that fell back to the inherited harness.
    pub fell_back: u32,
    /// Recorded spend in US dollars.
    pub usd: f64,
    /// Scoring units the judge reported (`points`, `criteria`).
    pub metrics: Vec<String>,
}

/// `GET .../sweeps/{sweep}/report`: computed natively from cached rows (read-only).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ScoreReport {
    /// Title line.
    pub title: String,
    /// Benchmark name in namespaces (`jb`, `lab`, `wb-office`, ..., `demo`).
    pub bench_name: String,
    /// One summary per (mode, k).
    pub summaries: Vec<ModeSummary>,
    /// Upstream's NOTE/WARNING lines (noise floor at k=1, base picks, fallbacks, mixed units).
    pub notes: Vec<String>,
    /// Mean base-execution score (percent) from `base_judged.json`, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_score: Option<f64>,
    /// The block in harness2-report's text format, for copying into papers or tickets.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LibraryEntry {
    pub task_slug: String,
    pub adopted_label: String,
    /// harness2 run id (`<namespace>/<task>/<label>`).
    #[serde(default)]
    pub run_id: String,
    /// One-line census (`2 skills, 1 scripts; 3,104 always-on chars`).
    pub components: String,
    pub why: String,
    /// The lesson the reflector distilled, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experience: Option<Experience>,
}

/// The experience library of one (mode, domain) in a sweep.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LibraryDomain {
    /// Recursion mode.
    pub mode: Mode,
    /// Domain (practice area, occupation or category) the task belongs to.
    pub domain: String,
    /// Entries.
    pub entries: Vec<LibraryEntry>,
}

/// One improver/selector session from the sweep's `_audit` trail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AuditSession {
    /// File stem, e.g. `inv1.a1.3f9c0d2e11aa7b40`; use in `.../audit/{session}`.
    pub id: String,
    /// Harness label (`base`, `inv1`, `dom1`, `agg1`, `c1`, ...).
    pub label: String,
    /// Whether it completed successfully.
    pub ok: bool,
    pub channel: String,
    /// Model id.
    pub model: String,
    pub effort: String,
    pub attempts: u32,
    pub wall_s: f64,
    pub cost_usd: f64,
    /// Error message, empty when there was none.
    pub error: String,
    /// Which text parts exist: `mission`, `system`, `response`.
    pub parts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TaskDelta {
    /// Task id as harness2 reports it.
    pub task_id: String,
    /// Domain (practice area, occupation or category) the task belongs to.
    pub domain: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub a: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub b: Option<f64>,
    /// `b - a` when both graded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<f64>,
}

/// `GET /api/v1/compare`: per-task deltas between two (sweep, mode) results.
///
/// Units (contract v1.1): every score here is a FRACTION in 0..1 (partial credit, as
/// [`JudgedRow::frac`]); `delta = b - a` in the same unit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Comparison {
    /// `<experiment>/<sweep>/<mode>`
    pub a: String,
    pub b: String,
    /// Number of tasks.
    pub tasks: Vec<TaskDelta>,
    /// Tasks graded on both sides.
    pub n_common: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mean_a: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mean_b: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mean_delta: Option<f64>,
    pub wins: u32,
    pub losses: u32,
    pub ties: u32,
    /// Sign-test two-sided p-value over wins vs losses (ties dropped), when n >= 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sign_test_p: Option<f64>,
}

/// Reproducibility manifest for an experiment (`GET .../manifest`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Manifest {
    pub schema_version: u32,
    /// When this was generated (RFC 3339, UTC).
    pub generated_at: String,
    pub studio_version: String,
    /// Experiment name (one HARNESS2_RUNS_DIR managed by the server).
    pub experiment: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness2_commit: Option<String>,
    /// Upstream dependency pins (benchmark checkouts, OpenCode, Codex version).
    pub dependency_pins: Vec<crate::KeyValue>,
    /// The profile's model ids (no secrets).
    pub models: Vec<crate::KeyValue>,
    /// Names (never values) of the environment variables passed to harness2.
    pub env_names: Vec<String>,
    /// Where the job's processes run.
    pub isolation: crate::Isolation,
    /// Network policy description (e.g. "container, egress allowlist: aiplatform.googleapis.com").
    pub network_policy: String,
    /// Every job that touched this experiment, in order.
    pub jobs: Vec<ManifestJob>,
    /// The `config.json` identity harness2 recorded per sweep.
    pub sweep_identities: Vec<SweepIdentity>,
    /// SHA-256 over every `summary.json` and `judged.json`, sorted by path.
    pub results_digest: String,
    /// JobBench dataset revision, when recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jobbench_revision: Option<String>,
}

/// The run identity harness2 recorded in a sweep's `config.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SweepIdentity {
    pub sweep: String,
    /// The file's JSON, verbatim (it holds model ids and the task list, no secrets).
    pub identity_json: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ManifestJob {
    /// Opaque identifier.
    pub id: String,
    /// Kind.
    pub kind: crate::JobKind,
    /// Status.
    pub status: crate::JobStatus,
    /// The exact argv, with secret-bearing values replaced.
    pub argv: Vec<String>,
    /// Creation time (RFC 3339, UTC).
    pub created_at: String,
    /// End time (RFC 3339, UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_id_map_is_ordered_pairs() {
        let s = Selection {
            ok: true,
            chosen_label: "agg1".into(),
            chosen_run_id: "x".into(),
            chosen_is_base: false,
            id_map: vec![
                AnonId {
                    id: "A".into(),
                    label: "agg1".into(),
                },
                AnonId {
                    id: "B".into(),
                    label: "agg2".into(),
                },
            ],
            n_candidates: 2,
            why: String::new(),
            trivial: false,
            fallback: false,
            dropped: vec![],
            usd: 0.0,
            error: String::new(),
            evidence: "deliverable".into(),
            harness_fingerprint: None,
            experience: None,
        };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["id_map"][0]["id"], "A");
        assert_eq!(v["id_map"][0]["label"], "agg1");
    }

    #[test]
    fn components_are_ordered_c1_to_c8() {
        assert!(Component::SystemPrompt < Component::Plugins);
        assert_eq!(
            serde_json::to_value(Component::ToolGuidance).unwrap(),
            "tool_guidance"
        );
    }
}
