//! A read-only, traversal-proof view of one experiment directory (one `HARNESS2_RUNS_DIR`).
//!
//! All access goes through a `cap_std::fs::Dir` opened at the experiment root, so neither a
//! request parameter nor a path recorded inside a results file (harness2 records absolute
//! `run_dir`s) can reach outside it. Symlinks are listed but never followed. Absolute paths
//! found in results files are mapped back under the root by stripping a known prefix: the
//! experiment's host path, or [`crate::isolation::EXPERIMENT_MOUNT`] for jobs that ran in a
//! container; anything else is refused.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use h2w_api::{
    AnonId, AuditSession, BaseJudgedRow, BaseRun, Comparison, Experience, ExperimentKind,
    FileContent, FileEntry, FileKind, HarnessDiff, HarnessFile, HarnessRef, HarnessRole,
    HarnessTree, JudgedRow, Ledger, LibraryDomain, LibraryEntry, Mode, ModeRecord, ModeScore,
    ScoreReport, Scores, Selection, Similarity, Stage, Substrate, Surface, SweepIdentity,
    SweepSummary, TaskDelta, TaskDetail, TaskModeOutcome, TaskRow,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::lite::{self, BaseLite, JudgedLite, Loose, MetaLite, RecordLite, SummaryLite};
use crate::namespace::{self, Namespace};
use crate::safefs::{self, EntryKind};
use crate::{CoreError, Result, names, pyfmt, report, tree};

/// Default cap for [`ExperimentDir::rollout_file`] and audit text reads.
pub const DEFAULT_READ_CAP: u64 = 2 * 1024 * 1024;

// Memory bounds. harness2's files can be rewritten by the jobs that produce them, so every
// read below has a size cap, and every view that reads many files has a count cap and a
// total. Past a cap a view fails with `CoreError::TooLarge`, except where a single file is
// only flagged (an oversized summary) or only a display hint (a sweep record).

/// Most entries walked by [`ExperimentDir::size_bytes`].
const MAX_SIZE_ENTRIES: usize = 5_000_000;
/// Largest task prompt read for [`TaskDetail::task_prompt`].
const MAX_PROMPT_BYTES: u64 = 256 * 1024;
/// Largest `summary.json` read. Real ones are tens of KiB; a larger one is listed with
/// status `too-large` and never parsed, and its task detail is refused.
pub(crate) const MAX_SUMMARY_BYTES: u64 = 4 * 1024 * 1024;
/// Most task directories in one sweep.
pub(crate) const MAX_SWEEP_TASKS: usize = 10_000;
/// Most `summary.json` bytes one multi-task view reads in total.
pub(crate) const MAX_SWEEP_SUMMARY_BYTES: u64 = 256 * 1024 * 1024;
/// Largest `judged.json` / `base_judged.json`.
pub(crate) const MAX_JUDGED_BYTES: u64 = 32 * 1024 * 1024;
/// Most rows kept from one judged file.
pub(crate) const MAX_JUDGED_ROWS: usize = 100_000;
/// Largest sweep record or `demo.json` read for the stream order (a larger one is ignored:
/// tasks are then listed by id).
const MAX_RECORD_BYTES: u64 = 4 * 1024 * 1024;
/// Largest `config.json` read for a sweep identity.
const MAX_CONFIG_BYTES: u64 = 256 * 1024;
/// Most `config.json` bytes one manifest reads in total.
const MAX_IDENTITY_BYTES: u64 = 16 * 1024 * 1024;
/// Bytes of a library `INDEX.md` read for its title (the first line).
const MAX_INDEX_HEAD: u64 = 4096;
/// Largest `EXPERIENCE.md` parsed; a larger one is listed without its details.
pub(crate) const MAX_EXPERIENCE_BYTES: u64 = 256 * 1024;
/// Most `EXPERIENCE.md` bytes one library view reads in total.
pub(crate) const MAX_LIBRARY_BYTES: u64 = 32 * 1024 * 1024;
/// Largest audit `.meta.json`; a larger one reads as empty.
const MAX_META_BYTES: u64 = 64 * 1024;
/// Most `.meta.json` bytes one audit view reads in total.
const MAX_AUDIT_META_BYTES: u64 = 32 * 1024 * 1024;
/// Most audit sessions listed for one sweep.
pub(crate) const MAX_AUDIT_SESSIONS: usize = 50_000;
/// Most entries listed from one rollout directory.
pub(crate) const MAX_ROLLOUT_ENTRIES: usize = 20_000;
/// Most bytes [`ExperimentDir::results_digest`] reads in all.
pub(crate) const MAX_DIGEST_BYTES: u64 = 1024 * 1024 * 1024;

/// Bytes read so far by one view, refused past `limit`.
struct Budget {
    used: u64,
    limit: u64,
    what: &'static str,
}

impl Budget {
    fn new(limit: u64, what: &'static str) -> Self {
        Budget {
            used: 0,
            limit,
            what,
        }
    }

    fn take(&mut self, n: usize) -> Result<()> {
        self.used = self
            .used
            .saturating_add(u64::try_from(n).unwrap_or(u64::MAX));
        if self.used > self.limit {
            return Err(CoreError::TooLarge(format!(
                "{} total more than {} bytes",
                self.what, self.limit
            )));
        }
        Ok(())
    }
}

/// An open experiment directory.
pub struct ExperimentDir {
    dir: cap_std::fs::Dir,
    kind: ExperimentKind,
    /// Absolute path prefixes that refer to the root inside results files.
    roots: Vec<String>,
}

impl std::fmt::Debug for ExperimentDir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExperimentDir")
            .field("kind", &self.kind)
            .field("roots", &self.roots)
            .finish()
    }
}

// ---------------------------------------------------------------------------------------
// JSON helpers with Python's semantics for the values harness2 writes.
// ---------------------------------------------------------------------------------------

fn field<'a>(v: &'a Value, key: &str) -> &'a Value {
    v.get(key).unwrap_or(&Value::Null)
}

/// A text field: strings verbatim, `null`/missing empty, anything else as JSON.
fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Python truthiness.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `float(x or 0.0)`.
fn number(v: &Value) -> f64 {
    match v {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::Bool(true) => 1.0,
        Value::String(s) if !s.is_empty() => pyfmt::strip(s)
            .replace('_', "")
            .parse::<f64>()
            .unwrap_or(0.0),
        _ => 0.0,
    }
}

/// A non-negative whole number, else 0.
fn count(v: &Value) -> u32 {
    match v {
        Value::Number(n) => n
            .as_u64()
            .map(|x| u32::try_from(x).unwrap_or(u32::MAX))
            .or_else(|| n.as_f64().and_then(pyfmt::exact_u32))
            .unwrap_or(0),
        _ => 0,
    }
}

fn list(v: &Value) -> &[Value] {
    v.as_array().map_or(&[], Vec::as_slice)
}

fn usize_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Upstream `_modes_of`: the per-mode records (`modes`, or the legacy `arms`).
fn modes_of(record: &Value) -> Vec<(Mode, &Value)> {
    let map = if record.get("modes").is_some() {
        record.get("modes")
    } else {
        record.get("arms")
    };
    by_mode(map)
}

/// `{"agg": ..., "seq": ...}` -> entries in canonical mode order.
fn by_mode(map: Option<&Value>) -> Vec<(Mode, &Value)> {
    let Some(Value::Object(obj)) = map else {
        return Vec::new();
    };
    let mut out: Vec<(Mode, &Value)> = obj
        .iter()
        .filter_map(|(k, v)| Mode::from_token(k).map(|m| (m, v)))
        .collect();
    out.sort_by_key(|(m, _)| *m);
    out.dedup_by_key(|(m, _)| *m);
    out
}

fn mode_rec(record: &Value, mode: Mode) -> &Value {
    modes_of(record)
        .into_iter()
        .find(|(m, _)| *m == mode)
        .map_or(&Value::Null, |(_, v)| v)
}

fn selection_rec(record: &Value, mode: Mode) -> &Value {
    by_mode(record.get("selections"))
        .into_iter()
        .find(|(m, _)| *m == mode)
        .map_or(&Value::Null, |(_, v)| v)
}

/// Modes with a record or a selection, in canonical order.
fn modes_present(record: &Value) -> Vec<Mode> {
    let mut modes: Vec<Mode> = modes_of(record).into_iter().map(|(m, _)| m).collect();
    modes.extend(
        by_mode(record.get("selections"))
            .into_iter()
            .map(|(m, _)| m),
    );
    modes.sort();
    modes.dedup();
    modes
}

/// `library.has_experience`.
fn experience(v: &Value) -> Option<Experience> {
    let ok = ["cue", "lesson", "evidence"]
        .iter()
        .all(|k| matches!(v.get(*k), Some(Value::String(s)) if !pyfmt::strip(s).is_empty()));
    ok.then(|| Experience {
        cue: text(field(v, "cue")),
        lesson: text(field(v, "lesson")),
        evidence: text(field(v, "evidence")),
        confidence: text(field(v, "confidence")),
        edits: text(field(v, "edits")),
    })
}

/// Anonymous selector ids in their natural order (`A`..`Z`, `AA`, ...).
fn anon_order(a: &str, b: &str) -> std::cmp::Ordering {
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

fn selection(v: &Value) -> Selection {
    let mut id_map: Vec<AnonId> = match v.get("id_map") {
        Some(Value::Object(m)) => m
            .iter()
            .map(|(id, label)| AnonId {
                id: id.clone(),
                label: text(label),
            })
            .collect(),
        _ => Vec::new(),
    };
    id_map.sort_by(|a, b| anon_order(&a.id, &b.id));
    let evidence = match v.get("evidence") {
        Some(Value::String(s)) => s.clone(),
        _ => "deliverable".to_owned(),
    };
    Selection {
        ok: truthy(field(v, "ok")),
        chosen_label: text(field(v, "chosen_label")),
        chosen_run_id: text(field(v, "chosen_run_id")),
        chosen_is_base: truthy(field(v, "chosen_is_base")),
        id_map,
        n_candidates: count(field(v, "n_candidates")),
        why: text(field(v, "why")),
        trivial: truthy(field(v, "trivial")),
        fallback: truthy(field(v, "fallback")),
        dropped: list(field(v, "dropped")).iter().map(text).collect(),
        usd: number(field(v, "usd")),
        error: text(field(v, "error")),
        evidence,
        harness_fingerprint: match v.get("harness_fingerprint") {
            Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
            _ => None,
        },
        experience: v.get("experience").and_then(experience),
    }
}

fn stage(v: &Value) -> Stage {
    Stage {
        label: text(field(v, "label")),
        dim: text(field(v, "dim")),
        fell_back: truthy(field(v, "fell_back")),
        kept_base: truthy(field(v, "kept_base")),
        attempts: count(field(v, "attempts")),
        usd: number(field(v, "usd")),
        cached: truthy(field(v, "cached")),
        notes: text(field(v, "notes")),
        error: text(field(v, "error")),
    }
}

fn ledger(v: &Value) -> Option<Ledger> {
    v.as_object()?;
    Some(Ledger {
        r: count(field(v, "R")),
        expected_r: count(field(v, "expected_R")),
        p: count(field(v, "P")),
        expected_p: count(field(v, "expected_P")),
        session_attempts: count(field(v, "session_attempts")),
        usd: number(field(v, "usd")),
        rollout_usd: number(field(v, "rollout_usd")),
        session_usd: number(field(v, "session_usd")),
    })
}

fn similarity(v: &Value) -> Option<Similarity> {
    v.as_object()?;
    Some(Similarity {
        n: count(field(v, "n")),
        n_distinct: count(field(v, "n_distinct")),
        mean_line_jaccard: number(field(v, "mean_line_jaccard")),
    })
}

fn role_of(label: &str) -> HarnessRole {
    if label.starts_with("base") {
        HarnessRole::Base
    } else if label.starts_with("inv") || label.starts_with("dom") {
        HarnessRole::Candidate
    } else {
        HarnessRole::Final
    }
}

/// A judged row as `core.judge._load_rows` rebuilds it: `graded` and `frac` are recomputed
/// from `n_passed`/`n_criteria`. JobBench's weighted judging writes float points, which the
/// `u32` wire fields cannot hold; the rows carry the raw values as `points_passed` /
/// `points_max`.
fn judged_row(l: JudgedLite) -> Option<JudgedRow> {
    let mode = Mode::from_token(l.mode.as_ref().or(l.arm.as_ref())?.as_str()?)?;
    let passed = l.n_passed.as_f64();
    let criteria = l.n_criteria.as_f64();
    let graded = criteria.is_some_and(|c| c > 0.0) && passed.is_some();
    let frac = match (graded, passed, criteria) {
        (true, Some(p), Some(c)) => Some(p / c),
        _ => None,
    };
    let metric = match &l.raw.0.metric {
        Loose::Str(s) => Some(s.clone()),
        m @ (Loose::Num(_) | Loose::Bool(true)) => Some(m.text()),
        _ => None,
    }
    .filter(|m| !m.is_empty());
    Some(JudgedRow {
        task_id: l.task_id.into_text(),
        mode,
        k: l.k.count(),
        label: l.label.into_text(),
        run_id: l.run_id.into_text(),
        chosen_is_base: l.chosen_is_base.truthy(),
        n_passed: passed.and_then(pyfmt::exact_u32),
        n_criteria: criteria.and_then(pyfmt::exact_u32),
        all_pass: l.all_pass.truthy(),
        frac,
        graded,
        error: l.error.0.into_text(),
        metric,
        points_passed: passed,
        points_max: criteria,
    })
}

/// A `base_judged.json` row (`{task, label, run_id, score}`), with the raw fraction.
fn base_row(l: BaseLite) -> (BaseJudgedRow, Option<f64>) {
    let score = l.score.0;
    let passed = score.n_passed.as_f64();
    let criteria = score.n_criteria.as_f64();
    let frac = match (passed, criteria) {
        (Some(p), Some(c)) if c > 0.0 => Some(p / c),
        _ => None,
    };
    let row = BaseJudgedRow {
        task_id: l.task.into_text(),
        label: l.label.into_text(),
        run_id: l.run_id.into_text(),
        n_passed: passed.and_then(pyfmt::exact_u32),
        n_criteria: criteria.and_then(pyfmt::exact_u32),
        all_pass: match score.all_pass {
            Some(Loose::Null) | None => None,
            Some(other) => Some(other.truthy()),
        },
        points_passed: passed,
        points_max: criteria,
    };
    (row, frac)
}

/// Rows of a judged file (empty when it does not exist), converted while they are read.
/// More than [`MAX_JUDGED_ROWS`] rows, or a file over [`MAX_JUDGED_BYTES`], is
/// [`CoreError::TooLarge`].
fn read_rows<L, T>(
    dir: &cap_std::fs::Dir,
    rel: &str,
    what: &str,
    convert: impl FnMut(L) -> Option<T>,
) -> Result<Vec<T>>
where
    L: for<'de> serde::Deserialize<'de>,
{
    let Some(bytes) = safefs::read_whole_opt(dir, rel, MAX_JUDGED_BYTES, what)? else {
        return Ok(Vec::new());
    };
    let (rows, more) =
        lite::rows(&bytes, MAX_JUDGED_ROWS, convert).map_err(|e| CoreError::Malformed {
            what: what.to_owned(),
            detail: e.to_string(),
        })?;
    if more {
        return Err(CoreError::TooLarge(format!(
            "{what} has more than {MAX_JUDGED_ROWS} rows"
        )));
    }
    Ok(rows)
}

fn rfc3339(secs: i64) -> Option<String> {
    let t = time::OffsetDateTime::from_unix_timestamp(secs).ok()?;
    t.format(&time::format_description::well_known::Rfc3339)
        .ok()
}

fn mime_for(name: &str, is_text: bool) -> String {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    let m = match ext.as_str() {
        "txt" | "log" | "out" | "err" => "text/plain",
        "md" => "text/markdown",
        "json" => "application/json",
        "jsonl" | "ndjson" => "application/x-ndjson",
        "csv" => "text/csv",
        "tsv" => "text/tab-separated-values",
        "html" | "htm" => "text/html",
        "py" => "text/x-python",
        "ts" => "text/typescript",
        "js" | "mjs" => "text/javascript",
        "sh" => "text/x-shellscript",
        "yaml" | "yml" => "application/yaml",
        "toml" => "application/toml",
        "xml" => "application/xml",
        "diff" | "patch" => "text/x-diff",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        _ if is_text => "text/plain",
        _ => "application/octet-stream",
    };
    m.to_owned()
}

/// Decode bytes as text for display: `None` when they look binary. A truncated read may end
/// inside a character; up to three trailing bytes are dropped for that.
fn display_text(bytes: &[u8], truncated: bool) -> Option<String> {
    let probe = bytes.get(..bytes.len().min(8192)).unwrap_or(bytes);
    if probe.contains(&0) {
        return None;
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => Some(s.to_owned()),
        Err(e) if truncated && e.error_len().is_none() && bytes.len() - e.valid_up_to() <= 3 => {
            std::str::from_utf8(bytes.get(..e.valid_up_to())?)
                .ok()
                .map(str::to_owned)
        }
        Err(_) => None,
    }
}

/// `re.sub(r"[^A-Za-z0-9_.-]", "-", s)` (WorkBuddy's `label_sanitize`).
fn label_sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// One task's whole `summary.json` (at most [`MAX_SUMMARY_BYTES`]) and the directory it
/// came from, for the single-task views.
struct TaskSummary {
    slug: String,
    raw: Value,
}

impl TaskSummary {
    fn task_id(&self) -> String {
        match self.raw.get("task_id") {
            Some(Value::String(s)) if !s.is_empty() => s.clone(),
            _ => self.slug.replace("__", "/"),
        }
    }
}

/// A task's `summary.json` as the multi-task views read it.
enum SummaryState {
    Parsed(SummaryLite),
    /// Not valid JSON, not an object, or not a regular file.
    Unreadable,
    /// Over [`MAX_SUMMARY_BYTES`]; never read.
    TooLarge,
}

/// The recorded `task_id`, else the directory name with `__` read as `/`.
fn task_id_of(s: &SummaryLite, slug: &str) -> String {
    match s.task_id.as_str() {
        Some(t) if !t.is_empty() => t.to_owned(),
        _ => slug.replace("__", "/"),
    }
}

/// Upstream `INDEX.md` title line `# Library — <domain>`.
fn index_title(head: &[u8]) -> Option<String> {
    String::from_utf8_lossy(head)
        .lines()
        .next()
        .and_then(|l| l.strip_prefix("# Library — "))
        .map(str::to_owned)
}

fn surface_of(substrate: Substrate) -> Surface {
    match substrate {
        Substrate::Oc => Surface::Opencode,
        Substrate::Cx => Surface::Codex,
    }
}

const AUDIT_PARTS: [&str; 3] = ["mission", "system", "response"];

/// Audit session ids are file stems harness2 builds from `[A-Za-z0-9_.-]`.
fn audit_id_ok(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 255
        && !id.starts_with('.')
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
}

/// `<label>.<16 hex>` -> `<label>`.
fn audit_label_of(stem: &str) -> String {
    match stem.rsplit_once('.') {
        Some((label, key)) if key.len() == 16 && key.bytes().all(|c| c.is_ascii_hexdigit()) => {
            label.to_owned()
        }
        _ => stem.to_owned(),
    }
}

/// Sweep record file name `<ns>-<YYYYmmddTHHMMSSZ>.json` -> stamp, when it belongs to `ns`.
fn record_stamp<'a>(file: &'a str, ns: &str) -> Option<&'a str> {
    let stem = file.strip_suffix(".json")?;
    let (prefix, stamp) = stem.rsplit_once('-')?;
    let b = stamp.as_bytes();
    let ok = b.len() == 16
        && b.get(8) == Some(&b'T')
        && b.get(15) == Some(&b'Z')
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 8 || i == 15 || c.is_ascii_digit());
    (ok && prefix == ns).then_some(stamp)
}

impl ExperimentDir {
    /// Open `path` (must be an existing directory). `extra_roots` lists other absolute
    /// prefixes results files may use for this root (the container mount point).
    ///
    /// The canonical form of `path` is recognised as well, since harness2 records resolved
    /// paths.
    pub fn open(path: &Path, kind: ExperimentKind, extra_roots: &[String]) -> Result<Self> {
        let dir = cap_std::fs::Dir::open_ambient_dir(path, cap_std::ambient_authority())?;
        let mut roots = vec![path.to_string_lossy().into_owned()];
        if let Ok(canon) = std::fs::canonicalize(path) {
            roots.push(canon.to_string_lossy().into_owned());
        }
        roots.extend(extra_roots.iter().cloned());
        let mut clean: Vec<String> = Vec::new();
        for r in roots {
            let r = if r.len() > 1 {
                r.trim_end_matches('/').to_owned()
            } else {
                r
            };
            if r.starts_with('/') && !clean.contains(&r) {
                clean.push(r);
            }
        }
        Ok(ExperimentDir {
            dir,
            kind,
            roots: clean,
        })
    }

    #[must_use]
    pub fn kind(&self) -> ExperimentKind {
        self.kind
    }

    /// The underlying capability handle (for modules in this crate).
    #[must_use]
    pub fn dir(&self) -> &cap_std::fs::Dir {
        &self.dir
    }

    /// Absolute prefixes recognised as this root.
    #[must_use]
    pub fn roots(&self) -> &[String] {
        &self.roots
    }

    /// Map an absolute path recorded by harness2 to a path relative to the root.
    fn map_abs(&self, abs: &str) -> Result<String> {
        let refused = || {
            CoreError::Refused(
                "a run directory recorded in summary.json lies outside this experiment".into(),
            )
        };
        for root in &self.roots {
            let rest = if root == "/" {
                abs.strip_prefix('/')
            } else if abs == root {
                Some("")
            } else {
                abs.strip_prefix(root.as_str())
                    .and_then(|r| r.strip_prefix('/'))
            };
            if let Some(rest) = rest {
                let rel = rest.trim_end_matches('/');
                names::relative_path(rel).map_err(|_| refused())?;
                return Ok(rel.to_owned());
            }
        }
        Err(refused())
    }

    /// The directory holding `bench_name`'s namespaces.
    fn ns_root(&self, bench_name: &str) -> Result<String> {
        namespace::namespaces_root(self.kind, bench_name)
            .ok_or_else(|| CoreError::NotFound(format!("sweep for benchmark {bench_name}")))
    }

    fn sweep_dir(&self, sweep: &Namespace) -> Result<String> {
        Ok(format!(
            "{}/{}",
            self.ns_root(&sweep.bench_name)?,
            sweep.summary()
        ))
    }

    fn mode_dir(&self, sweep: &Namespace, mode: Mode) -> Result<String> {
        Ok(format!(
            "{}/{}",
            self.ns_root(&sweep.bench_name)?,
            sweep.with_mode(mode)
        ))
    }

    /// `(bench name, namespaces root)` pairs this experiment kind can contain.
    fn bench_roots(&self) -> Vec<(&'static str, String)> {
        let benches: &[&'static str] = match self.kind {
            ExperimentKind::Demo => &["demo"],
            ExperimentKind::Standard => &["jb", "lab", "wb-office", "wb-web", "wb-code"],
        };
        benches
            .iter()
            .filter_map(|b| namespace::namespaces_root(self.kind, b).map(|r| (*b, r)))
            .collect()
    }

    /// Every sweep's summary namespace on disk (sorted by name).
    fn summary_namespaces(&self) -> Result<Vec<Namespace>> {
        let mut out: BTreeMap<String, Namespace> = BTreeMap::new();
        for (bench, root) in self.bench_roots() {
            for e in safefs::list_opt(&self.dir, &root, "namespaces")? {
                if e.kind != EntryKind::Dir {
                    continue;
                }
                if let Some(ns) = Namespace::parse(&e.name)
                    && ns.bench_name == bench
                {
                    let s = ns.summary();
                    out.insert(s.to_string(), s);
                }
            }
        }
        Ok(out.into_values().collect())
    }

    /// Task directories of the sweep with their `summary.json` read into the typed view, in
    /// upstream's `sorted(glob("*/summary.json"))` order (task directories without one are
    /// left out).
    ///
    /// One bad summary never hides the others: it is listed as [`SummaryState::Unreadable`]
    /// or, over [`MAX_SUMMARY_BYTES`], [`SummaryState::TooLarge`] without being read. More
    /// than [`MAX_SWEEP_TASKS`] task directories, or more than [`MAX_SWEEP_SUMMARY_BYTES`]
    /// of summaries in total, is [`CoreError::TooLarge`].
    fn summaries(&self, sweep: &Namespace) -> Result<Vec<(String, SummaryState)>> {
        let dir = self.sweep_dir(sweep)?;
        let slugs: Vec<String> = safefs::list_opt(&self.dir, &dir, "sweep")?
            .into_iter()
            .filter(|e| e.kind == EntryKind::Dir && names::task_slug(&e.name).is_ok())
            .map(|e| e.name)
            .collect();
        if slugs.len() > MAX_SWEEP_TASKS {
            return Err(CoreError::TooLarge(format!(
                "the sweep has {} task directories, over the limit of {MAX_SWEEP_TASKS}",
                slugs.len()
            )));
        }
        let mut budget = Budget::new(MAX_SWEEP_SUMMARY_BYTES, "the sweep's summary.json files");
        let mut out = Vec::with_capacity(slugs.len());
        for slug in slugs {
            let rel = format!("{dir}/{slug}/summary.json");
            let state =
                match safefs::read_whole_opt(&self.dir, &rel, MAX_SUMMARY_BYTES, "summary.json") {
                    Ok(Some(bytes)) => {
                        budget.take(bytes.len())?;
                        lite::object::<SummaryLite>(&bytes)
                            .map_or(SummaryState::Unreadable, SummaryState::Parsed)
                    }
                    Ok(None) => continue,
                    Err(CoreError::TooLarge(_)) => SummaryState::TooLarge,
                    Err(CoreError::Refused(_)) => SummaryState::Unreadable,
                    Err(e) => return Err(e),
                };
            out.push((slug, state));
        }
        Ok(out)
    }

    /// One task's whole summary; over [`MAX_SUMMARY_BYTES`] is [`CoreError::TooLarge`].
    fn summary(&self, sweep: &Namespace, slug: &str) -> Result<TaskSummary> {
        names::task_slug(slug)?;
        let rel = format!("{}/{slug}/summary.json", self.sweep_dir(sweep)?);
        match safefs::read_json_opt(&self.dir, &rel, MAX_SUMMARY_BYTES, "summary.json")? {
            Some(raw) if raw.is_object() => Ok(TaskSummary {
                slug: slug.to_owned(),
                raw,
            }),
            Some(_) => Err(CoreError::Malformed {
                what: "summary.json".into(),
                detail: "expected a JSON object".into(),
            }),
            None => Err(CoreError::NotFound(format!("task {slug}"))),
        }
    }

    /// Rows of `judged.json` (empty when not judged yet).
    fn judged_rows(&self, sweep: &Namespace) -> Result<Vec<JudgedRow>> {
        let rel = format!("{}/judged.json", self.sweep_dir(sweep)?);
        read_rows(&self.dir, &rel, "judged.json", judged_row)
    }

    fn base_rows(&self, sweep: &Namespace) -> Result<Vec<(BaseJudgedRow, Option<f64>)>> {
        let rel = format!("{}/base_judged.json", self.sweep_dir(sweep)?);
        read_rows(&self.dir, &rel, "base_judged.json", |l: BaseLite| {
            Some(base_row(l))
        })
    }

    fn headline(rows: &[JudgedRow], k: u32) -> Vec<ModeScore> {
        let rows: Vec<JudgedRow> = rows.iter().filter(|r| r.k == k).cloned().collect();
        report::summarise(&rows, &BTreeMap::new(), &BTreeMap::new())
            .into_iter()
            .map(|s| ModeScore {
                mode: s.mode,
                k: s.k,
                graded: s.n,
                score: s.score,
                all_pass: s.all_pass,
            })
            .collect()
    }

    fn sweep_summary(&self, sweep: &Namespace) -> Result<SweepSummary> {
        let dir = self.sweep_dir(sweep)?;
        let summaries = self.summaries(sweep)?;
        let (mut ok, mut partial, mut failed) = (0_u32, 0_u32, 0_u32);
        let mut modes: Vec<Mode> = Vec::new();
        let mut usd = 0.0_f64;
        let mut newest: Option<i64> = None;
        for (slug, s) in &summaries {
            match s {
                SummaryState::Parsed(s) => {
                    match s.status.as_str() {
                        Some("ok" | "skipped") => ok += 1,
                        Some("partial") => partial += 1,
                        _ => failed += 1,
                    }
                    for m in s.modes_present() {
                        if !modes.contains(&m) {
                            modes.push(m);
                        }
                        let (_, spend, sel) = s.mode_spend(m);
                        usd += spend + sel;
                    }
                }
                SummaryState::Unreadable | SummaryState::TooLarge => failed += 1,
            }
            let rel = format!("{dir}/{slug}/summary.json");
            if let Some(t) = safefs::mtime(&self.dir, &rel, "summary.json")? {
                newest = Some(newest.map_or(t, |n| n.max(t)));
            }
        }
        modes.sort();
        let judged = self.judged_rows(sweep)?;
        let headline =
            if safefs::mtime(&self.dir, &format!("{dir}/judged.json"), "judged.json")?.is_some() {
                Some(Self::headline(&judged, sweep.k))
            } else {
                None
            };
        Ok(SweepSummary {
            id: sweep.summary().to_string(),
            bench_name: sweep.bench_name.clone(),
            cell: namespace::cell_of(&sweep.bench_name, sweep.substrate),
            substrate: sweep.substrate,
            k: sweep.k,
            seed: sweep.seed,
            tag: sweep.tag.clone(),
            modes,
            tasks: usize_u32(summaries.len()),
            ok,
            partial,
            failed,
            usd,
            headline,
            updated_at: newest.and_then(rfc3339),
        })
    }

    /// The summary namespaces of every sweep on disk, sorted by name: what [`Self::sweeps`]
    /// lists. With [`Self::sweep`] a caller can read them one at a time and keep listing the
    /// others when one fails (an oversized `judged.json`, too many task directories).
    /// Additive after contract v1.2.
    pub fn sweep_namespaces(&self) -> Result<Vec<Namespace>> {
        self.summary_namespaces()
    }

    /// One sweep's summary (`sweep` is its summary namespace, as from
    /// [`Self::sweep_namespaces`]), exactly as [`Self::sweeps`] lists it. Additive after
    /// contract v1.2.
    pub fn sweep(&self, sweep: &Namespace) -> Result<SweepSummary> {
        self.sweep_summary(sweep)
    }

    /// Every sweep on disk, newest first. Unparseable directory names are skipped.
    pub fn sweeps(&self) -> Result<Vec<SweepSummary>> {
        let mut out = Vec::new();
        for ns in self.summary_namespaces()? {
            out.push(self.sweep_summary(&ns)?);
        }
        out.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(out)
    }

    /// Task ids in stream order from the newest sweep record (`demo.json` for a demo).
    ///
    /// The order is only a display hint: a record that cannot be used (missing, over
    /// [`MAX_RECORD_BYTES`], malformed) gives no order, and the tasks are listed by id.
    fn stream_order(&self, sweep: &Namespace) -> Result<Vec<String>> {
        let rel = match self.kind {
            ExperimentKind::Demo => Some("demo.json".to_owned()),
            ExperimentKind::Standard => {
                let root = format!("{}/_sweeps", self.ns_root(&sweep.bench_name)?);
                let ns = sweep.summary().to_string();
                safefs::list_opt(&self.dir, &root, "sweep records")?
                    .into_iter()
                    .filter(|e| e.kind == EntryKind::File)
                    .filter_map(|e| {
                        record_stamp(&e.name, &ns).map(|s| (s.to_owned(), e.name.clone()))
                    })
                    .max()
                    .map(|(_, name)| format!("{root}/{name}"))
            }
        };
        let record = rel
            .and_then(|rel| {
                safefs::read_whole_opt(&self.dir, &rel, MAX_RECORD_BYTES, "sweep record")
                    .ok()
                    .flatten()
            })
            .and_then(|bytes| lite::object::<RecordLite>(&bytes).ok());
        Ok(record.map(RecordLite::order).unwrap_or_default())
    }

    /// Task rows of one sweep (`sweep` is the summary namespace), in stream order when a
    /// sweep record exists, else sorted by task id.
    pub fn tasks(&self, sweep: &Namespace) -> Result<Vec<TaskRow>> {
        let judged = self.judged_rows(sweep)?;
        let mut scores: HashMap<(&str, Mode), &JudgedRow> = HashMap::new();
        for r in judged.iter().filter(|r| r.k == sweep.k) {
            scores.insert((r.task_id.as_str(), r.mode), r);
        }
        let mut rows: Vec<TaskRow> = Vec::new();
        for (slug, state) in self.summaries(sweep)? {
            let s = match state {
                SummaryState::Parsed(s) => s,
                SummaryState::Unreadable | SummaryState::TooLarge => {
                    let status = if matches!(state, SummaryState::TooLarge) {
                        "too-large"
                    } else {
                        "unreadable"
                    };
                    rows.push(TaskRow {
                        task_id: slug.replace("__", "/"),
                        slug,
                        domain: String::new(),
                        status: status.into(),
                        modes: Vec::new(),
                        errors: 1,
                    });
                    continue;
                }
            };
            let task_id = task_id_of(&s, &slug);
            let modes = s
                .modes_present()
                .into_iter()
                .map(|mode| {
                    let rec = s.mode(mode);
                    let sel = s.selection(mode);
                    let (fell_back, spend, sel_usd) = s.mode_spend(mode);
                    let chosen = sel.map(|x| x.chosen_label.text()).unwrap_or_default();
                    let judged = scores.get(&(task_id.as_str(), mode));
                    TaskModeOutcome {
                        mode,
                        ok: rec.is_some_and(|r| r.ok.truthy())
                            && sel.is_some_and(|x| x.ok.truthy())
                            && !chosen.is_empty(),
                        chosen_label: (!chosen.is_empty()).then_some(chosen),
                        // "The adopted harness is an unchanged base": harness2 records
                        // that as `chosen_tree_is_base` (the selector's own
                        // `chosen_is_base` only says a reference run was picked).
                        chosen_is_base: sel.is_some_and(|x| {
                            x.chosen_tree_is_base
                                .as_ref()
                                .map_or_else(|| x.chosen_is_base.truthy(), Loose::truthy)
                        }),
                        fell_back,
                        usd: spend + sel_usd,
                        score: judged.and_then(|r| r.frac),
                        all_pass: judged.filter(|r| r.graded).map(|r| r.all_pass),
                    }
                })
                .collect();
            rows.push(TaskRow {
                domain: s.domain.text(),
                status: s.status.text(),
                errors: s.errors.len(),
                modes,
                slug,
                task_id,
            });
        }
        let order = self.stream_order(sweep)?;
        let position: HashMap<&str, usize> = order
            .iter()
            .enumerate()
            .map(|(i, t)| (t.as_str(), i))
            .rev()
            .collect();
        rows.sort_by(|a, b| {
            let pa = position
                .get(a.task_id.as_str())
                .copied()
                .unwrap_or(usize::MAX);
            let pb = position
                .get(b.task_id.as_str())
                .copied()
                .unwrap_or(usize::MAX);
            pa.cmp(&pb).then_with(|| a.task_id.cmp(&b.task_id))
        });
        Ok(rows)
    }

    /// Harness labels on disk for one task and mode, in recursion order (base first).
    fn harness_refs(
        &self,
        sweep: &Namespace,
        slug: &str,
        mode: Mode,
        rec: &Value,
        chosen: &str,
    ) -> Result<Vec<HarnessRef>> {
        let rel = format!("{}/{slug}/harnesses", self.mode_dir(sweep, mode)?);
        let mut labels: Vec<String> = safefs::list_opt(&self.dir, &rel, "harnesses")?
            .into_iter()
            .filter(|e| e.kind == EntryKind::Dir && names::harness_label(&e.name).is_ok())
            .map(|e| e.name)
            .collect();
        if !labels.iter().any(|l| l == "base") {
            labels.push("base".to_owned());
        }
        // First position of each label in the ledger's rollout order.
        let mut ledger_pos: HashMap<String, usize> = HashMap::new();
        for (i, l) in list(field(field(rec, "ledger"), "rollouts"))
            .iter()
            .map(text)
            .enumerate()
        {
            ledger_pos.entry(l).or_insert(i);
        }
        let key = |l: &str| {
            let base = u8::from(!l.starts_with("base"));
            let pos = ledger_pos.get(l).copied().unwrap_or(usize::MAX);
            let digits: String = l.chars().filter(char::is_ascii_digit).collect();
            let n: u64 = digits.parse().unwrap_or(0);
            (base, pos, n, l.to_owned())
        };
        labels.sort_by_key(|l| key(l));
        Ok(labels
            .into_iter()
            .map(|label| HarnessRef {
                role: role_of(&label),
                chosen: label == chosen,
                label,
            })
            .collect())
    }

    fn task_prompt(&self, sweep: &Namespace, slug: &str) -> Result<Option<String>> {
        for mode in Mode::ALL {
            let task_dir = format!("{}/{slug}", self.mode_dir(sweep, mode)?);
            for sub in ["context", "selection"] {
                let base = format!("{task_dir}/{sub}");
                for e in safefs::list_opt(&self.dir, &base, "context")? {
                    if e.kind != EntryKind::Dir {
                        continue;
                    }
                    let rel = format!("{base}/{}/task.md", e.name);
                    match safefs::read_capped(&self.dir, &rel, MAX_PROMPT_BYTES, "task.md") {
                        Ok((bytes, _, _)) => {
                            return Ok(Some(String::from_utf8_lossy(&bytes).into_owned()));
                        }
                        Err(CoreError::NotFound(_) | CoreError::Refused(_)) => {}
                        Err(e) => return Err(e),
                    }
                }
            }
        }
        Ok(None)
    }

    /// Full detail of one task.
    pub fn task(&self, sweep: &Namespace, slug: &str) -> Result<TaskDetail> {
        let s = self.summary(sweep, slug)?;
        let task_id = s.task_id();
        let judged: Vec<JudgedRow> = self
            .judged_rows(sweep)?
            .into_iter()
            .filter(|r| r.task_id == task_id && r.k == sweep.k)
            .collect();
        let mut base = Vec::new();
        for entry in list(field(&s.raw, "base")) {
            let (label, run_id, run_dir) = match entry {
                Value::Object(_) => (
                    text(field(entry, "label")),
                    text(field(entry, "run_id")),
                    text(field(entry, "run_dir")),
                ),
                other => (String::new(), text(other), String::new()),
            };
            let available = !run_dir.is_empty()
                && self
                    .map_abs(&run_dir)
                    .is_ok_and(|rel| safefs::is_dir(&self.dir, &rel));
            base.push(BaseRun {
                label,
                run_id,
                available,
            });
        }
        let mut modes = Vec::new();
        for mode in modes_present(&s.raw) {
            let rec = mode_rec(&s.raw, mode);
            let sel_v = selection_rec(&s.raw, mode);
            let sel = sel_v.as_object().map(|_| selection(sel_v));
            let chosen = sel.as_ref().map_or("", |x| x.chosen_label.as_str());
            let mut errors: Vec<String> = list(field(rec, "errors")).iter().map(text).collect();
            if let Some(Value::String(e)) = rec.get("error")
                && !e.is_empty()
            {
                errors.push(e.clone());
            }
            modes.push(ModeRecord {
                mode,
                ok: truthy(field(rec, "ok")),
                finals: list(field(rec, "finals")).iter().map(text).collect(),
                stages: list(field(rec, "stages")).iter().map(stage).collect(),
                ledger: ledger(field(rec, "ledger")),
                similarity: similarity(field(rec, "similarity")),
                harnesses: self.harness_refs(sweep, slug, mode, rec, chosen)?,
                scores: judged.iter().filter(|r| r.mode == mode).cloned().collect(),
                selection: sel,
                errors,
            });
        }
        Ok(TaskDetail {
            slug: s.slug.clone(),
            domain: text(field(&s.raw, "domain")),
            k: count(field(&s.raw, "k")),
            seed: field(&s.raw, "seed").as_u64(),
            status: text(field(&s.raw, "status")),
            base,
            modes,
            errors: list(field(&s.raw, "errors")).iter().map(text).collect(),
            task_prompt: self.task_prompt(sweep, slug)?,
            task_id,
        })
    }

    fn read_harness(
        &self,
        sweep: &Namespace,
        slug: &str,
        mode: Mode,
        label: &str,
    ) -> Result<(cap_std::fs::Dir, tree::Tree)> {
        names::task_slug(slug)?;
        names::harness_label(label)?;
        let rel = format!("{}/{slug}/harnesses/{label}", self.mode_dir(sweep, mode)?);
        let dir = safefs::open_dir(&self.dir, &rel, "harness").map_err(|e| match e {
            CoreError::NotFound(_) => CoreError::NotFound(format!("harness {label}")),
            other => other,
        })?;
        let (t, _off) = tree::read_tree(&dir, surface_of(sweep.substrate))?;
        Ok((dir, t))
    }

    /// One harness tree with file contents.
    pub fn harness(
        &self,
        sweep: &Namespace,
        slug: &str,
        mode: Mode,
        label: &str,
    ) -> Result<HarnessTree> {
        let surface = surface_of(sweep.substrate);
        let (dir, t) = self.read_harness(sweep, slug, mode, label)?;
        let census = tree::census(surface, &t);
        let fingerprint = tree::fingerprint(&dir)?;
        let mut files: Vec<HarnessFile> = t
            .into_iter()
            .map(|(path, content)| HarnessFile {
                component: tree::classify(surface, &path)
                    .unwrap_or(h2w_api::Component::SystemPrompt),
                bytes: u64::try_from(content.len()).unwrap_or(u64::MAX),
                content: Some(content),
                path,
            })
            .collect();
        files.sort_by(|a, b| {
            a.component
                .cmp(&b.component)
                .then_with(|| pyfmt::cmp_paths(&a.path, &b.path))
        });
        Ok(HarnessTree {
            label: label.to_owned(),
            role: role_of(label),
            surface,
            files,
            census,
            fingerprint,
        })
    }

    /// Diff between two harness labels of the same task and mode.
    pub fn diff(
        &self,
        sweep: &Namespace,
        slug: &str,
        mode: Mode,
        from: &str,
        to: &str,
    ) -> Result<HarnessDiff> {
        let (_, a) = self.read_harness(sweep, slug, mode, from)?;
        let (_, b) = self.read_harness(sweep, slug, mode, to)?;
        Ok(tree::diff(surface_of(sweep.substrate), from, &a, to, &b))
    }

    /// The run directory (relative to the root) of one rollout.
    ///
    /// Base runs and the selected run come from the paths harness2 recorded in
    /// `summary.json`; other labels follow the benchmark's run-directory convention for the
    /// run id `<mode namespace>/<task id>/<label>`.
    fn rollout_dir(
        &self,
        sweep: &Namespace,
        slug: &str,
        mode: Mode,
        label: &str,
    ) -> Result<String> {
        names::harness_label(label)?;
        let s = self.summary(sweep, slug)?;
        let task_id = s.task_id();
        if label.starts_with("base")
            && let Some(entry) = list(field(&s.raw, "base"))
                .iter()
                .find(|e| text(field(e, "label")) == label)
        {
            let run_dir = text(field(entry, "run_dir"));
            if !run_dir.is_empty() {
                return self.map_abs(&run_dir);
            }
        }
        let sel = selection_rec(&s.raw, mode);
        let chosen_dir = text(field(sel, "chosen_run_dir"));
        if text(field(sel, "chosen_label")) == label && !chosen_dir.is_empty() {
            return self.map_abs(&chosen_dir);
        }
        // Base runs belong to the parallel (summary) namespace for both modes.
        let ns = if label.starts_with("base") {
            sweep.summary()
        } else {
            sweep.with_mode(mode)
        };
        let run_id = format!("{ns}/{task_id}/{label}");
        let rel = match sweep.bench_name.as_str() {
            "demo" => format!("executions/{run_id}"),
            "lab" => format!("lab/results/{}/{run_id}", sweep.substrate.as_cli()),
            "jb" => format!(
                "jb/dataset/{task_id}/model_output/{}",
                run_id.replace('/', "__")
            ),
            other => match namespace::bench_dir(other) {
                Some(d) if other.starts_with("wb-") => {
                    format!(
                        "{d}/rollouts/{}",
                        label_sanitize(&run_id.replace('/', "__"))
                    )
                }
                _ => return Err(CoreError::NotFound(format!("rollout {label}"))),
            },
        };
        names::relative_path(&rel).map_err(|_| {
            CoreError::Refused("the task id does not map to a safe run directory".into())
        })?;
        Ok(rel)
    }

    /// List `sub` (relative path, empty = root) of the rollout run directory of `label`.
    ///
    /// Directories are listed first, then files, each by name.
    pub fn rollout_files(
        &self,
        sweep: &Namespace,
        slug: &str,
        mode: Mode,
        label: &str,
        sub: &str,
    ) -> Result<Vec<FileEntry>> {
        names::relative_path(sub)?;
        let run = self.rollout_dir(sweep, slug, mode, label)?;
        let target = if sub.is_empty() {
            run
        } else {
            format!("{run}/{sub}")
        };
        let dir = safefs::open_dir(&self.dir, &target, "rollout directory")?;
        let mut out: Vec<FileEntry> = safefs::list_capped(&dir, MAX_ROLLOUT_ENTRIES)?
            .into_iter()
            .map(|e| FileEntry {
                path: if sub.is_empty() {
                    e.name.clone()
                } else {
                    format!("{sub}/{}", e.name)
                },
                kind: match e.kind {
                    EntryKind::File => FileKind::File,
                    EntryKind::Dir => FileKind::Dir,
                    EntryKind::Symlink => FileKind::Symlink,
                    EntryKind::Other => FileKind::Other,
                },
                bytes: if e.kind == EntryKind::File { e.len } else { 0 },
                name: e.name,
            })
            .collect();
        out.sort_by(|a, b| {
            (a.kind != FileKind::Dir)
                .cmp(&(b.kind != FileKind::Dir))
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(out)
    }

    /// Read one file of a rollout run directory, at most `max_bytes` of it.
    pub fn rollout_file(
        &self,
        sweep: &Namespace,
        slug: &str,
        mode: Mode,
        label: &str,
        sub: &str,
        max_bytes: u64,
    ) -> Result<FileContent> {
        names::relative_path(sub)?;
        if sub.is_empty() {
            return Err(CoreError::invalid(
                "path",
                "name a file inside the run directory",
            ));
        }
        let run = self.rollout_dir(sweep, slug, mode, label)?;
        let (bytes, len, truncated) = safefs::read_capped(
            &self.dir,
            &format!("{run}/{sub}"),
            max_bytes,
            "rollout file",
        )?;
        let text = display_text(&bytes, truncated);
        let name = sub.rsplit('/').next().unwrap_or(sub);
        Ok(FileContent {
            path: sub.to_owned(),
            bytes: len,
            mime: mime_for(name, text.is_some()),
            text,
            truncated,
        })
    }

    /// `judged.json` and `base_judged.json` of the sweep (empty when not judged yet).
    pub fn scores(&self, sweep: &Namespace) -> Result<Scores> {
        Ok(Scores {
            rows: self.judged_rows(sweep)?,
            base_rows: self.base_rows(sweep)?.into_iter().map(|(r, _)| r).collect(),
        })
    }

    /// The score report computed from the cached rows (port of `harness2-report`).
    ///
    /// Reports every mode that has judged rows, at the sweep's `k`, exactly as
    /// `harness2-report --k <k> --modes <those modes>` would; `text` is that command's
    /// stdout. With no judged rows the report is empty and says so.
    pub fn report(&self, sweep: &Namespace) -> Result<ScoreReport> {
        let rows = self.judged_rows(sweep)?;
        let mut modes: Vec<Mode> = rows.iter().map(|r| r.mode).collect();
        modes.sort();
        modes.dedup();
        let title = format!(
            "{}-{}{} modes={}",
            sweep.bench_name,
            sweep.substrate.as_cli(),
            sweep.seed.map(|s| format!(" seed={s}")).unwrap_or_default(),
            modes
                .iter()
                .map(|m| m.as_cli())
                .collect::<Vec<_>>()
                .join(",")
        );
        let base_fracs: Vec<f64> = self
            .base_rows(sweep)?
            .into_iter()
            .filter_map(|(_, f)| f)
            .collect();
        let base_score = pyfmt::mean(&base_fracs).map(|m| pyfmt::round(100.0 * m, 2));
        if rows.is_empty() {
            return Ok(ScoreReport {
                title,
                bench_name: sweep.bench_name.clone(),
                summaries: Vec::new(),
                notes: vec!["No judged rows yet: judge this sweep first.".to_owned()],
                base_score,
                text: "nothing to report\n".to_owned(),
            });
        }
        let (fell_back, usd) = self.spend_and_fallbacks(sweep, &modes)?;
        let summaries = report::summarise(&rows, &fell_back, &usd);
        let (text, notes) = report_text(&summaries, &title, &sweep.bench_name);
        Ok(ScoreReport {
            title,
            bench_name: sweep.bench_name.clone(),
            summaries,
            notes,
            base_score,
            text,
        })
    }

    /// Upstream `cli/report._spend_and_fallbacks` for one sweep: fell-back stage counts and
    /// improver spend keyed by `(mode, k)`, accumulated over the summaries in upstream's
    /// sorted order (the order matters for the float sums).
    fn spend_and_fallbacks(&self, sweep: &Namespace, modes: &[Mode]) -> Result<(FellBack, Spend)> {
        let mut fell_back = FellBack::new();
        let mut usd = Spend::new();
        for (_, s) in self.summaries(sweep)? {
            let SummaryState::Parsed(s) = s else { continue };
            for &mode in modes {
                let key = (mode, sweep.k);
                let (fb, spend, sel) = s.mode_spend(mode);
                *fell_back.entry(key).or_insert(0) += fb;
                let acc = usd.get(&key).copied().unwrap_or(0.0);
                usd.insert(key, acc + spend + sel);
            }
        }
        Ok((fell_back, usd))
    }

    /// Experience libraries of the sweep, one per (mode, domain).
    ///
    /// An entry whose `EXPERIENCE.md` is over [`MAX_EXPERIENCE_BYTES`] (or not a regular
    /// file) is listed without its details; more than [`MAX_LIBRARY_BYTES`] of them in
    /// total is [`CoreError::TooLarge`].
    pub fn library(&self, sweep: &Namespace) -> Result<Vec<LibraryDomain>> {
        let Some(root) = namespace::library_root(self.kind, sweep) else {
            return Ok(Vec::new());
        };
        let mut budget = Budget::new(MAX_LIBRARY_BYTES, "the library's EXPERIENCE.md files");
        let mut out = Vec::new();
        for mode in Mode::ALL {
            let mode_root = format!("{root}/{}", mode.token());
            for d in safefs::list_opt(&self.dir, &mode_root, "library")? {
                if d.kind != EntryKind::Dir || names::task_slug(&d.name).is_err() {
                    continue;
                }
                let dom_rel = format!("{mode_root}/{}", d.name);
                let index = format!("{dom_rel}/INDEX.md");
                let domain =
                    match safefs::read_capped(&self.dir, &index, MAX_INDEX_HEAD, "INDEX.md") {
                        Ok((head, _, _)) => index_title(&head),
                        Err(CoreError::NotFound(_) | CoreError::Refused(_)) => None,
                        Err(e) => return Err(e),
                    }
                    .unwrap_or_else(|| d.name.clone());
                let mut entries = Vec::new();
                let harnesses = format!("{dom_rel}/harnesses");
                for e in safefs::list_opt(&self.dir, &harnesses, "library entries")? {
                    if e.kind != EntryKind::Dir || names::task_slug(&e.name).is_err() {
                        continue;
                    }
                    let exp = match safefs::read_text_opt(
                        &self.dir,
                        &format!("{harnesses}/{}/EXPERIENCE.md", e.name),
                        MAX_EXPERIENCE_BYTES,
                        "EXPERIENCE.md",
                    ) {
                        Ok(text) => text,
                        Err(CoreError::TooLarge(_) | CoreError::Refused(_)) => None,
                        Err(e) => return Err(e),
                    };
                    if let Some(text) = &exp {
                        budget.take(text.len())?;
                    }
                    entries.push(library_entry(&e.name, exp.as_deref().unwrap_or("")));
                }
                out.push(LibraryDomain {
                    mode,
                    domain,
                    entries,
                });
            }
        }
        Ok(out)
    }

    fn audit_dir(&self, sweep: &Namespace) -> Option<String> {
        match self.kind {
            ExperimentKind::Demo => None,
            ExperimentKind::Standard => self
                .ns_root(&sweep.bench_name)
                .ok()
                .map(|r| format!("{r}/_audit/{}", sweep.summary())),
        }
    }

    /// Improver/selector sessions recorded under `adapt/_audit/<sweep>/`.
    ///
    /// One session per file stem (`<label>.<key>`), ordered by modification time then id.
    /// Demo experiments record no audit trail.
    pub fn audit(&self, sweep: &Namespace) -> Result<Vec<AuditSession>> {
        let Some(dir) = self.audit_dir(sweep) else {
            return Ok(Vec::new());
        };
        let mut stems: BTreeMap<String, (Vec<String>, bool, i64)> = BTreeMap::new();
        for e in safefs::list_opt(&self.dir, &dir, "audit")? {
            if e.kind != EntryKind::File {
                continue;
            }
            let parsed = AUDIT_PARTS
                .iter()
                .find_map(|p| {
                    e.name
                        .strip_suffix(&format!(".{p}.md"))
                        .map(|s| (s.to_owned(), Some(*p)))
                })
                .or_else(|| {
                    e.name
                        .strip_suffix(".meta.json")
                        .map(|s| (s.to_owned(), None))
                });
            let Some((stem, part)) = parsed else { continue };
            if !audit_id_ok(&stem) {
                continue;
            }
            let slot = stems.entry(stem).or_insert((Vec::new(), false, i64::MAX));
            match part {
                Some(p) => slot.0.push(p.to_owned()),
                None => slot.1 = true,
            }
            slot.2 = slot.2.min(e.mtime.unwrap_or(i64::MAX));
        }
        if stems.len() > MAX_AUDIT_SESSIONS {
            return Err(CoreError::TooLarge(format!(
                "the audit trail has {} sessions, over the limit of {MAX_AUDIT_SESSIONS}",
                stems.len()
            )));
        }
        let mut budget = Budget::new(MAX_AUDIT_META_BYTES, "the audit's .meta.json files");
        let mut out = Vec::new();
        for (stem, (mut parts, has_meta, mtime)) in stems {
            parts.sort_by_key(|p| AUDIT_PARTS.iter().position(|x| x == p));
            let meta_rel = format!("{dir}/{stem}.meta.json");
            // Metadata that cannot be read (oversized, a link, malformed) reads as empty.
            let meta: MetaLite = match has_meta
                .then(|| safefs::read_whole_opt(&self.dir, &meta_rel, MAX_META_BYTES, "meta.json"))
            {
                Some(Ok(Some(bytes))) => {
                    budget.take(bytes.len())?;
                    lite::object(&bytes).unwrap_or_default()
                }
                _ => MetaLite::default(),
            };
            let label = match meta.label.as_str() {
                Some(l) if !l.is_empty() => l.to_owned(),
                _ => audit_label_of(&stem),
            };
            out.push((
                mtime,
                AuditSession {
                    id: stem,
                    label,
                    ok: meta.ok.truthy(),
                    channel: meta.channel.into_text(),
                    model: meta.model.into_text(),
                    effort: meta.effort.into_text(),
                    attempts: meta.attempts.count(),
                    wall_s: meta.wall_s.number(),
                    cost_usd: meta.cost_usd.number(),
                    error: meta.error.0.into_text(),
                    parts,
                },
            ));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.id.cmp(&b.1.id)));
        Ok(out.into_iter().map(|(_, s)| s).collect())
    }

    /// One text part (`mission`, `system`, `response`) of an audit session.
    ///
    /// A part longer than `max_bytes` is cut there and ends with a `[truncated: ...]` line.
    pub fn audit_text(
        &self,
        sweep: &Namespace,
        id: &str,
        part: &str,
        max_bytes: u64,
    ) -> Result<String> {
        if !AUDIT_PARTS.contains(&part) {
            return Err(CoreError::invalid(
                "part",
                "expected mission, system or response",
            ));
        }
        if !audit_id_ok(id) {
            return Err(CoreError::invalid("session", "not an audit session id"));
        }
        let Some(dir) = self.audit_dir(sweep) else {
            return Err(CoreError::NotFound(format!("audit session {id}")));
        };
        let (bytes, len, truncated) = safefs::read_capped(
            &self.dir,
            &format!("{dir}/{id}.{part}.md"),
            max_bytes,
            "audit text",
        )
        .map_err(|e| match e {
            CoreError::NotFound(_) => CoreError::NotFound(format!("audit {id} {part}")),
            other => other,
        })?;
        let mut text = match display_text(&bytes, truncated) {
            Some(t) => t,
            None => String::from_utf8_lossy(&bytes).into_owned(),
        };
        if truncated {
            text.push_str(&format!(
                "\n[truncated: showing the first {} of {len} bytes]",
                bytes.len()
            ));
        }
        Ok(text)
    }

    /// Spend recorded in the sweep's summaries so far (stages + selections), USD. Used by
    /// the budget guard while a run is in progress; tolerant of half-written files.
    pub fn spend_usd(&self, sweep: &Namespace) -> Result<f64> {
        let mut total = 0.0;
        for (_, s) in self.summaries(sweep)? {
            let SummaryState::Parsed(s) = s else { continue };
            for mode in s.modes_present() {
                let (_, spend, sel) = s.mode_spend(mode);
                total += spend + sel;
            }
        }
        Ok(total)
    }

    /// `config.json` identities of every sweep (for the manifest).
    ///
    /// A `config.json` over [`MAX_CONFIG_BYTES`], or more than [`MAX_IDENTITY_BYTES`] of
    /// them in total, is [`CoreError::TooLarge`].
    pub fn sweep_identities(&self) -> Result<Vec<SweepIdentity>> {
        let mut budget = Budget::new(MAX_IDENTITY_BYTES, "the sweeps' config.json files");
        let mut out = Vec::new();
        for ns in self.summary_namespaces()? {
            let rel = format!("{}/config.json", self.sweep_dir(&ns)?);
            if let Some(text) =
                safefs::read_text_opt(&self.dir, &rel, MAX_CONFIG_BYTES, "config.json")?
            {
                budget.take(text.len())?;
                out.push(SweepIdentity {
                    sweep: ns.to_string(),
                    identity_json: text,
                });
            }
        }
        Ok(out)
    }

    /// SHA-256 (hex) over every `summary.json`, `judged.json` and `base_judged.json`,
    /// in sorted path order, each as `path\0bytes\0`.
    ///
    /// Paths are relative to the experiment root (`lab/adapt/lab-oc-agg1/judged.json`),
    /// sorted as byte strings; every namespace directory of every benchmark is included.
    ///
    /// Bounded like the views that read these files. A file over its cap (4 MiB for a
    /// summary, 32 MiB for a judged file), which every view refuses, is not read: its
    /// contents are recorded as `<too large: N bytes>`. A file that is no longer a regular
    /// file, or is gone, when it is opened is recorded as `<unreadable>`. More than 1 GiB in
    /// all, or more than 10,000 summaries in one namespace, is [`CoreError::TooLarge`].
    pub fn results_digest(&self) -> Result<String> {
        self.results_digest_capped(MAX_DIGEST_BYTES)
    }

    fn results_digest_capped(&self, max_total: u64) -> Result<String> {
        let mut paths: Vec<String> = Vec::new();
        for (bench, root) in self.bench_roots() {
            for e in safefs::list_opt(&self.dir, &root, "namespaces")? {
                if e.kind != EntryKind::Dir
                    || Namespace::parse(&e.name).is_none_or(|ns| ns.bench_name != bench)
                {
                    continue;
                }
                let ns_rel = format!("{root}/{}", e.name);
                let mut summaries = 0_usize;
                for t in safefs::list_opt(&self.dir, &ns_rel, "namespace")? {
                    match t.kind {
                        EntryKind::File
                            if t.name == "judged.json" || t.name == "base_judged.json" =>
                        {
                            paths.push(format!("{ns_rel}/{}", t.name));
                        }
                        EntryKind::Dir => {
                            let rel = format!("{ns_rel}/{}/summary.json", t.name);
                            if safefs::mtime(&self.dir, &rel, "summary.json")?.is_some() {
                                summaries += 1;
                                if summaries > MAX_SWEEP_TASKS {
                                    return Err(CoreError::TooLarge(format!(
                                        "namespace {} has more than {MAX_SWEEP_TASKS} summaries",
                                        e.name
                                    )));
                                }
                                paths.push(rel);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        paths.sort();
        let mut h = Sha256::new();
        let mut total = 0_u64;
        for p in paths {
            let cap = if p.ends_with("/summary.json") {
                MAX_SUMMARY_BYTES
            } else {
                MAX_JUDGED_BYTES
            };
            h.update(p.as_bytes());
            h.update(b"\0");
            match safefs::open_file(&self.dir, &p, "results file") {
                Ok((_, len)) if len > cap => h.update(safefs::too_large_marker(len).as_bytes()),
                Ok((file, len)) => {
                    if total.saturating_add(len) > max_total {
                        return Err(CoreError::TooLarge(format!(
                            "the results files total more than {max_total} bytes"
                        )));
                    }
                    let n = safefs::stream(file, cap, |b| h.update(b))?;
                    total = total.saturating_add(n);
                }
                Err(CoreError::Refused(_) | CoreError::NotFound(_)) => h.update(b"<unreadable>"),
                Err(e) => return Err(e),
            }
            h.update(b"\0");
        }
        Ok(hex::encode(h.finalize()))
    }

    /// Total size of regular files under the root (symlinks not followed).
    pub fn size_bytes(&self) -> Result<u64> {
        safefs::total_size(&self.dir, MAX_SIZE_ENTRIES)
    }
}

type FellBack = BTreeMap<(Mode, u32), u32>;
type Spend = BTreeMap<(Mode, u32), f64>;

/// What `harness2-report` prints for these summaries: the rendered block, then (when any
/// improver spend was recorded) the spend line. Also returns the NOTE/WARNING paragraphs.
fn report_text(
    summaries: &[h2w_api::ModeSummary],
    title: &str,
    bench: &str,
) -> (String, Vec<String>) {
    let (rendered, notes) = report::render(summaries, title, bench);
    let mut by_mode: Vec<&h2w_api::ModeSummary> = summaries.iter().collect();
    by_mode.sort_by_key(|s| (s.mode, s.k));
    let spend = pyfmt::sum(by_mode.iter().map(|s| s.usd));
    let mut text = format!("{rendered}\n");
    if spend != 0.0 {
        text.push_str(&format!(
            "\n  improver (editors, composer, selection, reflection) spend across reported modes: \
             ${spend:.2}\n"
        ));
    }
    (text, notes)
}

/// Parse a library entry's `EXPERIENCE.md` (upstream `Library.add` format).
fn library_entry(slug: &str, text: &str) -> LibraryEntry {
    let mut adopted_label = String::new();
    let mut run_id = String::new();
    let mut components = String::new();
    let mut section = "";
    let mut why: Vec<&str> = Vec::new();
    let mut edits: Vec<&str> = Vec::new();
    let mut lesson: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        match line {
            "## Why this was chosen" => {
                section = "why";
                continue;
            }
            "## What each edit did" => {
                section = "edits";
                continue;
            }
            "## Transferable lesson" => {
                section = "lesson";
                continue;
            }
            _ => {}
        }
        match section {
            "why" => why.push(line),
            "edits" => edits.push(line),
            "lesson" => {
                let field_line = ["cue", "lesson", "evidence", "confidence"]
                    .iter()
                    .find_map(|f| {
                        line.strip_prefix(&format!("- **{f}**:"))
                            .map(|v| ((*f).to_owned(), v.trim().to_owned()))
                    });
                match field_line {
                    Some(fv) => lesson.push(fv),
                    None => {
                        if let Some(last) = lesson.last_mut()
                            && !line.is_empty()
                        {
                            last.1.push('\n');
                            last.1.push_str(line);
                        }
                    }
                }
            }
            _ => {
                if let Some(rest) = line.strip_prefix("- adopted: `") {
                    if let Some((label, tail)) = rest.split_once('`') {
                        adopted_label = label.to_owned();
                        if let Some(r) = tail.trim().strip_prefix("(run `") {
                            run_id = r.trim_end_matches(')').trim_end_matches('`').to_owned();
                        }
                    }
                } else if let Some(rest) = line.strip_prefix("- components: ") {
                    components = rest.to_owned();
                }
            }
        }
    }
    let get = |k: &str| {
        lesson
            .iter()
            .find(|(f, _)| f == k)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    let exp = Experience {
        cue: get("cue"),
        lesson: get("lesson"),
        evidence: get("evidence"),
        confidence: get("confidence"),
        edits: edits.join("\n").trim().to_owned(),
    };
    let has = !exp.cue.is_empty() && !exp.lesson.is_empty() && !exp.evidence.is_empty();
    LibraryEntry {
        task_slug: slug.to_owned(),
        adopted_label,
        run_id,
        components,
        why: why.join("\n").trim().to_owned(),
        experience: has.then_some(exp),
    }
}

/// Exact two-sided sign test: `P(X <= min(w, l)) * 2`, capped at 1, with `X ~ Bin(w + l,
/// 1/2)`. Computed in log space so large task counts stay finite.
fn sign_test(wins: u32, losses: u32) -> Option<f64> {
    let n = wins.checked_add(losses)?;
    if n == 0 {
        return None;
    }
    let m = wins.min(losses);
    // log C(n, i) / 2^n accumulated iteratively.
    let ln2n = f64::from(n) * std::f64::consts::LN_2;
    let mut log_c = 0.0_f64; // ln C(n, 0)
    let mut terms = Vec::with_capacity(usize::try_from(m).unwrap_or(0) + 1);
    for i in 0..=m {
        if i > 0 {
            log_c += (f64::from(n - i + 1)).ln() - f64::from(i).ln();
        }
        terms.push(log_c - ln2n);
    }
    let max = terms.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let sum: f64 = terms.iter().map(|t| (t - max).exp()).sum();
    let tail = (max + sum.ln()).exp();
    Some((2.0 * tail).min(1.0))
}

/// Per-task deltas between two score sets. `a_mode`/`b_mode` pick the rows of each side;
/// `domains` maps task id to domain for display.
///
/// Scores are fractions 0..1 (partial credit, as `JudgedRow::frac`); `delta = b - a`. Means
/// are over the tasks graded on both sides. Wins count tasks where `b` scored higher. Tasks
/// are listed by task id, every task that has a row of the chosen mode on either side.
#[must_use]
pub fn compare(
    a_label: &str,
    a: &Scores,
    a_mode: Mode,
    b_label: &str,
    b: &Scores,
    b_mode: Mode,
    domains: &BTreeMap<String, String>,
) -> Comparison {
    let side = |s: &Scores, mode: Mode| -> BTreeMap<String, Option<f64>> {
        let mut m: BTreeMap<String, Option<f64>> = BTreeMap::new();
        for r in s.rows.iter().filter(|r| r.mode == mode) {
            let v = if r.graded { r.frac } else { None };
            let slot = m.entry(r.task_id.clone()).or_insert(None);
            if slot.is_none() {
                *slot = v;
            }
        }
        m
    };
    let sa = side(a, a_mode);
    let sb = side(b, b_mode);
    let mut ids: Vec<&String> = sa.keys().chain(sb.keys()).collect();
    ids.sort();
    ids.dedup();
    let mut tasks = Vec::with_capacity(ids.len());
    let (mut wins, mut losses, mut ties) = (0_u32, 0_u32, 0_u32);
    let (mut common_a, mut common_b, mut deltas) = (Vec::new(), Vec::new(), Vec::new());
    for id in ids {
        let va = sa.get(id).copied().flatten();
        let vb = sb.get(id).copied().flatten();
        let delta = match (va, vb) {
            (Some(x), Some(y)) => {
                common_a.push(x);
                common_b.push(y);
                deltas.push(y - x);
                if y > x {
                    wins += 1;
                } else if y < x {
                    losses += 1;
                } else {
                    ties += 1;
                }
                Some(y - x)
            }
            _ => None,
        };
        tasks.push(TaskDelta {
            task_id: id.clone(),
            domain: domains.get(id).cloned().unwrap_or_default(),
            a: va,
            b: vb,
            delta,
        });
    }
    let mean_a = pyfmt::mean(&common_a);
    let mean_b = pyfmt::mean(&common_b);
    Comparison {
        a: a_label.to_owned(),
        b: b_label.to_owned(),
        tasks,
        n_common: usize_u32(common_a.len()),
        mean_delta: pyfmt::mean(&deltas),
        mean_a,
        mean_b,
        wins,
        losses,
        ties,
        sign_test_p: sign_test(wins, losses),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combined_ks_match_harness2_report() {
        // `harness2-report --bench lab --k 1,2 --modes parallel,sequential`, captured from
        // upstream on the std fixture.
        let fx = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
        let exp = ExperimentDir::open(
            &fx.join("std"),
            ExperimentKind::Standard,
            &["/fixture/std".to_owned()],
        )
        .unwrap();
        let modes = [Mode::Parallel, Mode::Sequential];
        let mut rows = Vec::new();
        let mut fell_back = FellBack::new();
        let mut usd = Spend::new();
        for name in ["lab-oc-agg1", "lab-oc-agg2"] {
            let ns = Namespace::parse(name).unwrap();
            rows.extend(
                exp.judged_rows(&ns)
                    .unwrap()
                    .into_iter()
                    .filter(|r| modes.contains(&r.mode)),
            );
            let (f, u) = exp.spend_and_fallbacks(&ns, &modes).unwrap();
            fell_back.extend(f);
            usd.extend(u);
        }
        let summaries = report::summarise(&rows, &fell_back, &usd);
        let (text, notes) = report_text(&summaries, "lab-oc modes=parallel,sequential", "lab");
        let want = std::fs::read_to_string(fx.join("cli/std/lab-report-k1k2.stdout")).unwrap();
        assert_eq!(text, want);
        assert_eq!(notes.len(), 2);
    }

    #[test]
    fn sign_test_matches_exact_binomial() {
        assert_eq!(sign_test(0, 0), None);
        // n = 1: P(X <= 0) = 0.5 -> p = 1.
        assert!((sign_test(1, 0).unwrap() - 1.0).abs() < 1e-12);
        // n = 10, 9 wins: 2 * (1 + 10) / 1024.
        assert!((sign_test(9, 1).unwrap() - 22.0 / 1024.0).abs() < 1e-12);
        // n = 20, 15 vs 5: 2 * sum_{i<=5} C(20, i) / 2^20 = 0.041389...
        assert!((sign_test(15, 5).unwrap() - 0.041_389_465_332_031_25).abs() < 1e-12);
        assert!((sign_test(5, 5).unwrap() - 1.0).abs() < 1e-12);
        let big = sign_test(60_000, 40_000).unwrap();
        assert!((0.0..1e-200).contains(&big));
    }

    fn jr(task: &str, mode: Mode, frac: Option<f64>) -> JudgedRow {
        JudgedRow {
            task_id: task.into(),
            mode,
            k: 1,
            label: "agg1".into(),
            run_id: String::new(),
            chosen_is_base: false,
            n_passed: None,
            n_criteria: None,
            all_pass: false,
            frac,
            graded: frac.is_some(),
            error: String::new(),
            metric: None,
            points_passed: None,
            points_max: None,
        }
    }

    /// JobBench's weighted judging writes fractional points: the integer fields stay empty
    /// and the raw values travel as `points_*`, with `frac` computed from them.
    fn judged(v: &serde_json::Value) -> Option<JudgedRow> {
        judged_row(lite::object(v.to_string().as_bytes()).unwrap())
    }

    #[test]
    fn weighted_points_are_kept_exactly() {
        let v = serde_json::json!({
            "task_id": "t1", "mode": "agg", "k": 1, "label": "agg1", "run_id": "ns/t1/agg1",
            "chosen_is_base": false, "n_passed": 7.5, "n_criteria": 10.0, "all_pass": false,
        });
        let r = judged(&v).unwrap();
        assert_eq!((r.n_passed, r.n_criteria), (None, Some(10)));
        assert_eq!((r.points_passed, r.points_max), (Some(7.5), Some(10.0)));
        assert_eq!(r.frac, Some(0.75));
        let v = serde_json::json!({
            "task_id": "t2", "mode": "seq", "k": 1, "label": "seq1", "run_id": "ns/t2/seq1",
            "chosen_is_base": false, "n_passed": 3, "n_criteria": 4, "all_pass": false,
        });
        let r = judged(&v).unwrap();
        assert_eq!((r.n_passed, r.n_criteria), (Some(3), Some(4)));
        assert_eq!((r.points_passed, r.points_max), (Some(3.0), Some(4.0)));
        let base = serde_json::json!({
            "task": "t1", "label": "base", "run_id": "ns/t1/base",
            "score": {"n_passed": 2.25, "n_criteria": 3, "all_pass": false},
        });
        let (b, frac) = base_row(lite::object(base.to_string().as_bytes()).unwrap());
        assert_eq!(
            (b.n_passed, b.points_passed, b.points_max),
            (None, Some(2.25), Some(3.0))
        );
        assert_eq!(frac, Some(0.75));
    }

    #[test]
    fn compare_pairs_graded_rows() {
        let a = Scores {
            rows: vec![
                jr("t1", Mode::Parallel, Some(0.5)),
                jr("t2", Mode::Parallel, Some(0.5)),
                jr("t3", Mode::Parallel, None),
                jr("t4", Mode::Parallel, Some(1.0)),
                jr("t1", Mode::Sequential, Some(0.0)),
            ],
            base_rows: vec![],
        };
        let b = Scores {
            rows: vec![
                jr("t1", Mode::Sequential, Some(0.75)),
                jr("t2", Mode::Sequential, Some(0.5)),
                jr("t3", Mode::Sequential, Some(1.0)),
                jr("t4", Mode::Sequential, Some(0.25)),
            ],
            base_rows: vec![],
        };
        let domains = BTreeMap::from([("t1".to_owned(), "d".to_owned())]);
        let c = compare(
            "e/s/parallel",
            &a,
            Mode::Parallel,
            "e/s/sequential",
            &b,
            Mode::Sequential,
            &domains,
        );
        assert_eq!(c.n_common, 3);
        assert_eq!((c.wins, c.losses, c.ties), (1, 1, 1));
        assert_eq!(c.tasks.len(), 4);
        assert_eq!(c.tasks[0].delta, Some(0.25));
        assert_eq!(c.tasks[0].domain, "d");
        assert_eq!(c.tasks[2].delta, None);
        assert_eq!(c.mean_a, Some(2.0 / 3.0));
        assert_eq!(c.mean_b, Some(0.5));
        assert_eq!(c.sign_test_p, Some(1.0));
    }

    #[test]
    fn library_entry_parses_upstream_format() {
        let text = "# acct/ledger-1\n\n- adopted: `agg1`  (run `demo-oc-agg2/acct/ledger-1/agg1`)\n- components: 1 scripts; 62 always-on chars\n\n## Why this was chosen\nThe total matches.\n\n## What each edit did\ncomponent: scripts/summarize.py\nverdict: helped\n\n## Transferable lesson\n- **cue**: When credits appear\n- **lesson**: Sum both signs.\n- **evidence**: probe total\nsecond line\n- **confidence**: high\n";
        let e = library_entry("acct__ledger-1", text);
        assert_eq!(e.adopted_label, "agg1");
        assert_eq!(e.run_id, "demo-oc-agg2/acct/ledger-1/agg1");
        assert_eq!(e.components, "1 scripts; 62 always-on chars");
        assert_eq!(e.why, "The total matches.");
        let x = e.experience.unwrap();
        assert_eq!(x.edits, "component: scripts/summarize.py\nverdict: helped");
        assert_eq!(x.evidence, "probe total\nsecond line");
        assert_eq!(x.confidence, "high");
        let empty = library_entry("x", "");
        assert!(empty.experience.is_none());
    }

    /// A standard experiment with one LAB sweep (`lab-oc-agg1`) built in a temp dir.
    struct Lab {
        root: tempfile::TempDir,
    }

    impl Lab {
        const SWEEP: &'static str = "lab/adapt/lab-oc-agg1";

        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(root.path().join(Self::SWEEP)).unwrap();
            Lab { root }
        }

        fn path(&self, rel: &str) -> std::path::PathBuf {
            let p = self.root.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            p
        }

        fn write(&self, rel: &str, body: &[u8]) {
            std::fs::write(self.path(rel), body).unwrap();
        }

        /// A file of `len` zero bytes that takes no disk space.
        fn sparse(&self, rel: &str, len: u64) {
            std::fs::File::create(self.path(rel))
                .unwrap()
                .set_len(len)
                .unwrap();
        }

        fn summary(&self, slug: &str, usd: f64) {
            let body = serde_json::json!({
                "task_id": slug, "domain": "d", "status": "ok",
                "modes": {"agg": {"ok": true, "stages": [{"usd": usd}]}},
            });
            self.write(
                &format!("{}/{slug}/summary.json", Self::SWEEP),
                body.to_string().as_bytes(),
            );
        }

        fn open(&self) -> (ExperimentDir, Namespace) {
            let exp = ExperimentDir::open(self.root.path(), ExperimentKind::Standard, &[]).unwrap();
            (exp, Namespace::parse("lab-oc-agg1").unwrap())
        }
    }

    fn status_of(rows: &[TaskRow], slug: &str) -> String {
        rows.iter().find(|r| r.slug == slug).unwrap().status.clone()
    }

    fn sha256_hex(parts: &[(&str, &[u8])]) -> String {
        let mut h = Sha256::new();
        for (path, body) in parts {
            h.update(path.as_bytes());
            h.update(b"\0");
            h.update(body);
            h.update(b"\0");
        }
        hex::encode(h.finalize())
    }

    /// Sparse files far over the caps (32 GiB) are recorded in the manifest digest by their
    /// size, never read: the digest returns at once and is the same every time.
    #[test]
    fn the_digest_records_oversized_files_by_size() {
        let lab = Lab::new();
        lab.summary("t1", 0.5);
        lab.sparse(&format!("{}/t2/summary.json", Lab::SWEEP), 1 << 35);
        lab.sparse(&format!("{}/judged.json", Lab::SWEEP), MAX_JUDGED_BYTES + 1);
        let t1 = std::fs::read(
            lab.root
                .path()
                .join(format!("{}/t1/summary.json", Lab::SWEEP)),
        )
        .unwrap();
        let (exp, _) = lab.open();
        let digest = crate::safefs::tests::within(move || exp.results_digest().unwrap());
        assert_eq!(
            digest,
            sha256_hex(&[
                (
                    "lab/adapt/lab-oc-agg1/judged.json",
                    b"<too large: 33554433 bytes>"
                ),
                ("lab/adapt/lab-oc-agg1/t1/summary.json", &t1),
                (
                    "lab/adapt/lab-oc-agg1/t2/summary.json",
                    b"<too large: 34359738368 bytes>"
                ),
            ])
        );
    }

    #[test]
    fn the_digest_has_a_total_and_a_summary_count() {
        let lab = Lab::new();
        lab.write(&format!("{}/t1/summary.json", Lab::SWEEP), b"0123456789");
        lab.write(&format!("{}/t2/summary.json", Lab::SWEEP), b"0123456789");
        let (exp, _) = lab.open();
        assert!(exp.results_digest_capped(20).is_ok());
        let err = exp.results_digest_capped(19).unwrap_err();
        assert_eq!(
            err.to_string(),
            "too large: the results files total more than 19 bytes"
        );
        for i in 0..MAX_SWEEP_TASKS {
            lab.write(&format!("{}/n{i}/summary.json", Lab::SWEEP), b"{}");
        }
        let err = exp.results_digest().unwrap_err();
        assert_eq!(
            err.to_string(),
            "too large: namespace lab-oc-agg1 has more than 10000 summaries"
        );
    }

    /// A job swapping one task's `summary.json` for a FIFO and back, as fast as it can,
    /// while the task list is read over and over: every read returns (the FIFO is never
    /// waited on) and that task is either read or listed as unreadable.
    #[test]
    fn task_lists_survive_a_fifo_swapped_in_and_out() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        let lab = Lab::new();
        for i in 0..20 {
            lab.summary(&format!("t{i:02}"), 0.25);
        }
        let task = lab.root.path().join(format!("{}/t00", Lab::SWEEP));
        let body = std::fs::read(task.join("summary.json")).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let swapper = {
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                let mut swaps = 0_u32;
                while !stop.load(Ordering::Relaxed) {
                    crate::safefs::tests::mkfifo(&task.join("fifo"));
                    std::fs::rename(task.join("fifo"), task.join("summary.json")).unwrap();
                    std::fs::write(task.join("real"), &body).unwrap();
                    std::fs::rename(task.join("real"), task.join("summary.json")).unwrap();
                    swaps += 1;
                }
                swaps
            })
        };
        let (exp, ns) = lab.open();
        let seen = crate::safefs::tests::within(move || {
            let mut seen = std::collections::BTreeSet::new();
            let started = std::time::Instant::now();
            while started.elapsed() < std::time::Duration::from_secs(1) {
                let rows = exp.tasks(&ns).unwrap();
                assert_eq!(rows.len(), 20);
                seen.insert(status_of(&rows, "t00"));
                assert_eq!(status_of(&rows, "t19"), "ok");
            }
            seen
        });
        stop.store(true, Ordering::Relaxed);
        assert!(swapper.join().unwrap() > 0);
        assert!(
            seen.iter().all(|s| s == "ok" || s == "unreadable"),
            "{seen:?}"
        );
    }

    /// An oversized summary is flagged without being parsed (it is valid JSON: parsing it
    /// would give status `ok`), and every other summary of the sweep still reads.
    #[test]
    fn an_oversized_summary_is_flagged_and_the_others_still_read() {
        let lab = Lab::new();
        lab.summary("t1", 0.5);
        lab.summary("t3", 0.25);
        let head = br#"{"task_id": "t2", "status": "ok"}"#;
        let pad = |n: u64| {
            let mut b = head.to_vec();
            b.resize(usize::try_from(n).unwrap(), b' ');
            b
        };
        lab.write(
            &format!("{}/t2/summary.json", Lab::SWEEP),
            &pad(MAX_SUMMARY_BYTES + 1),
        );
        // Exactly at the cap is still read.
        lab.write(
            &format!("{}/t4/summary.json", Lab::SWEEP),
            &pad(MAX_SUMMARY_BYTES),
        );
        let (exp, ns) = lab.open();
        let rows = exp.tasks(&ns).unwrap();
        assert_eq!(rows.len(), 4);
        assert_eq!(status_of(&rows, "t2"), "too-large");
        assert_eq!(status_of(&rows, "t4"), "ok");
        assert_eq!(status_of(&rows, "t1"), "ok");
        assert_eq!(rows.iter().find(|r| r.slug == "t2").unwrap().errors, 1);
        assert_eq!(exp.spend_usd(&ns).unwrap(), 0.75);
        let sweeps = exp.sweeps().unwrap();
        assert_eq!((sweeps[0].tasks, sweeps[0].ok, sweeps[0].failed), (4, 3, 1));
        // The single-task views refuse it instead.
        assert!(matches!(exp.task(&ns, "t2"), Err(CoreError::TooLarge(_))));
        assert!(exp.task(&ns, "t1").is_ok());
    }

    /// The multi-task views have a total: 64 summaries of 4 MiB (the per-file cap) are
    /// 256 MiB and still read; one more is refused instead of read.
    #[test]
    fn a_sweep_reads_at_most_its_summary_budget() {
        let lab = Lab::new();
        let per_file = MAX_SUMMARY_BYTES;
        let fit = usize::try_from(MAX_SWEEP_SUMMARY_BYTES / per_file).unwrap();
        for i in 0..fit {
            lab.sparse(&format!("{}/t{i:03}/summary.json", Lab::SWEEP), per_file);
        }
        let (exp, ns) = lab.open();
        let rows = exp.tasks(&ns).unwrap();
        assert_eq!(rows.len(), fit);
        assert!(rows.iter().all(|r| r.status == "unreadable"));
        lab.sparse(&format!("{}/t{fit:03}/summary.json", Lab::SWEEP), per_file);
        let err = exp.tasks(&ns).unwrap_err();
        assert!(matches!(err, CoreError::TooLarge(_)), "{err}");
    }

    #[test]
    fn a_sweep_holds_at_most_its_task_cap() {
        let lab = Lab::new();
        for i in 0..MAX_SWEEP_TASKS {
            std::fs::create_dir(lab.root.path().join(format!("{}/t{i}", Lab::SWEEP))).unwrap();
        }
        lab.summary("t0", 1.0);
        let (exp, ns) = lab.open();
        assert_eq!(exp.tasks(&ns).unwrap().len(), 1);
        std::fs::create_dir(lab.root.path().join(format!("{}/one-more", Lab::SWEEP))).unwrap();
        let err = exp.tasks(&ns).unwrap_err();
        assert_eq!(
            err.to_string(),
            "too large: the sweep has 10001 task directories, over the limit of 10000"
        );
    }

    #[test]
    fn judged_files_are_capped_by_size_and_rows() {
        let lab = Lab::new();
        lab.summary("t1", 0.0);
        let judged = format!("{}/judged.json", Lab::SWEEP);
        let row = br#"{"task_id": "t1", "mode": "agg", "k": 1, "n_passed": 1, "n_criteria": 2}"#;
        let many = |n: usize| {
            let mut b = b"[".to_vec();
            for i in 0..n {
                if i > 0 {
                    b.push(b',');
                }
                b.extend_from_slice(row);
            }
            b.push(b']');
            b
        };
        lab.write(&judged, &many(MAX_JUDGED_ROWS));
        let (exp, ns) = lab.open();
        assert_eq!(exp.scores(&ns).unwrap().rows.len(), MAX_JUDGED_ROWS);
        lab.write(&judged, &many(MAX_JUDGED_ROWS + 1));
        let err = exp.scores(&ns).unwrap_err();
        assert_eq!(
            err.to_string(),
            "too large: judged.json has more than 100000 rows"
        );
        // Every view that needs the rows says so too, rather than showing partial scores.
        assert!(matches!(exp.tasks(&ns), Err(CoreError::TooLarge(_))));
        lab.sparse(&judged, MAX_JUDGED_BYTES + 1);
        assert!(matches!(exp.scores(&ns), Err(CoreError::TooLarge(_))));
        lab.sparse(
            &format!("{}/base_judged.json", Lab::SWEEP),
            MAX_JUDGED_BYTES + 1,
        );
        std::fs::remove_file(lab.root.path().join(&judged)).unwrap();
        assert!(matches!(exp.scores(&ns), Err(CoreError::TooLarge(_))));
    }

    /// `sweeps()` is `sweep_namespaces()` read one by one with `sweep()`, newest first; and
    /// read that way, a sweep that fails does not take the others with it.
    #[test]
    fn sweeps_are_the_namespaces_read_one_by_one() {
        let lab = Lab::new();
        lab.summary("t1", 0.5);
        lab.summary("t2", 0.25);
        let other = "lab/adapt/lab-oc-agg1-s7";
        let body = serde_json::json!({
            "task_id": "u1", "domain": "d", "status": "partial",
            "modes": {"agg": {"ok": false, "stages": [{"usd": 1.0}]}},
        });
        lab.write(
            &format!("{other}/u1/summary.json"),
            body.to_string().as_bytes(),
        );
        let newest_first = |v: &mut Vec<SweepSummary>| {
            v.sort_by(|a, b| {
                b.updated_at
                    .cmp(&a.updated_at)
                    .then_with(|| a.id.cmp(&b.id))
            });
        };
        let (exp, _) = lab.open();
        let names = exp.sweep_namespaces().unwrap();
        let ids: Vec<String> = names.iter().map(ToString::to_string).collect();
        assert_eq!(ids, vec!["lab-oc-agg1", "lab-oc-agg1-s7"]);
        let mut each: Vec<SweepSummary> = names.iter().map(|ns| exp.sweep(ns).unwrap()).collect();
        newest_first(&mut each);
        assert_eq!(each, exp.sweeps().unwrap());

        // One bad sweep: the whole list fails, the other sweep still reads on its own.
        let rows: Vec<serde_json::Value> = (0..=MAX_JUDGED_ROWS)
            .map(|_| serde_json::json!({"task_id": "u1", "mode": "agg", "k": 1}))
            .collect();
        lab.write(
            &format!("{other}/judged.json"),
            serde_json::to_vec(&rows).unwrap().as_slice(),
        );
        assert!(matches!(exp.sweeps(), Err(CoreError::TooLarge(_))));
        let bad = Namespace::parse("lab-oc-agg1-s7").unwrap();
        assert!(matches!(exp.sweep(&bad), Err(CoreError::TooLarge(_))));
        let good = exp.sweep(&names[0]).unwrap();
        assert_eq!(
            (good.id.as_str(), good.tasks, good.ok),
            ("lab-oc-agg1", 2, 2)
        );
    }

    /// The stream order is a display hint: an oversized sweep record is ignored, not fatal.
    #[test]
    fn an_oversized_sweep_record_only_loses_the_order() {
        let lab = Lab::new();
        lab.summary("b", 0.0);
        lab.summary("a", 0.0);
        let record = "lab/adapt/_sweeps/lab-oc-agg1-20261023T120000Z.json";
        lab.write(
            record,
            br#"{"domains": [{"domain": "d", "tasks": [{"task": "b"}, {"task": "a"}]}]}"#,
        );
        let (exp, ns) = lab.open();
        let order = |exp: &ExperimentDir| -> Vec<String> {
            exp.tasks(&ns)
                .unwrap()
                .into_iter()
                .map(|r| r.slug)
                .collect()
        };
        assert_eq!(order(&exp), vec!["b", "a"]);
        lab.sparse(record, MAX_RECORD_BYTES + 1);
        assert_eq!(order(&exp), vec!["a", "b"]);
    }

    #[test]
    fn library_and_audit_files_are_capped() {
        let lab = Lab::new();
        let lib = "lab/library/lab/k1-oc/agg/dom";
        lab.write(
            &format!("{lib}/INDEX.md"),
            "# Library — Domain\n".as_bytes(),
        );
        lab.write(
            &format!("{lib}/harnesses/small/EXPERIENCE.md"),
            b"- adopted: `agg1`\n",
        );
        lab.sparse(
            &format!("{lib}/harnesses/large/EXPERIENCE.md"),
            MAX_EXPERIENCE_BYTES + 1,
        );
        let (exp, ns) = lab.open();
        let doms = exp.library(&ns).unwrap();
        assert_eq!(doms[0].domain, "Domain");
        let entry = |slug: &str| {
            doms[0]
                .entries
                .iter()
                .find(|e| e.task_slug == slug)
                .unwrap()
        };
        assert_eq!(entry("small").adopted_label, "agg1");
        assert_eq!(entry("large").adopted_label, "");
        // Audit metadata over its cap reads as empty; the session is still listed.
        let audit = "lab/adapt/_audit/lab-oc-agg1";
        lab.write(
            &format!("{audit}/inv1.a1.0123456789abcdef.mission.md"),
            b"m",
        );
        lab.sparse(
            &format!("{audit}/inv1.a1.0123456789abcdef.meta.json"),
            MAX_META_BYTES + 1,
        );
        lab.write(
            &format!("{audit}/select.agg1.fedcba9876543210.meta.json"),
            br#"{"label": "select", "ok": true, "model": "m"}"#,
        );
        let sessions = exp.audit(&ns).unwrap();
        assert_eq!(sessions.len(), 2);
        let s = sessions.iter().find(|s| s.id.starts_with("inv1")).unwrap();
        assert_eq!((s.label.as_str(), s.ok), ("inv1.a1", false));
        let s = sessions
            .iter()
            .find(|s| s.id.starts_with("select"))
            .unwrap();
        assert_eq!(
            (s.label.as_str(), s.ok, s.model.as_str()),
            ("select", true, "m")
        );
    }

    #[test]
    fn small_helpers() {
        assert_eq!(
            record_stamp("lab-oc-agg1-20261023T120000Z.json", "lab-oc-agg1"),
            Some("20261023T120000Z")
        );
        assert_eq!(
            record_stamp("lab-oc-agg1-s7-20261023T120000Z.json", "lab-oc-agg1"),
            None
        );
        assert_eq!(audit_label_of("inv1.a1.3f9c0d2e11aa7b40"), "inv1.a1");
        assert_eq!(audit_label_of("select.agg2"), "select.agg2");
        assert!(audit_id_ok("select.agg2.0123456789abcdef"));
        assert!(!audit_id_ok("../x"));
        assert_eq!(
            label_sanitize("wb-office-oc-agg1__t 1__inv1"),
            "wb-office-oc-agg1__t-1__inv1"
        );
        assert_eq!(display_text(&[0xe2, 0x82], true).as_deref(), Some(""));
        assert_eq!(display_text(b"a\0b", false), None);
        assert_eq!(number(&serde_json::json!("1_000.5")), 1000.5);
        assert!(truthy(&serde_json::json!([0])));
        assert_eq!(count(&serde_json::json!(3.0)), 3);
        assert_eq!(count(&serde_json::json!(-1)), 0);
    }
}
