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

use h2s_api::{
    AnonId, AuditSession, BaseJudgedRow, BaseRun, Comparison, Experience, ExperimentKind,
    FileContent, FileEntry, FileKind, HarnessDiff, HarnessFile, HarnessRef, HarnessRole,
    HarnessTree, JudgedRow, Ledger, LibraryDomain, LibraryEntry, Mode, ModeRecord, ModeScore,
    ScoreReport, Scores, Selection, Similarity, Stage, Substrate, Surface, SweepIdentity,
    SweepSummary, TaskDelta, TaskDetail, TaskModeOutcome, TaskRow,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::namespace::{self, Namespace};
use crate::safefs::{self, EntryKind, MAX_JSON_BYTES, MAX_TEXT_BYTES};
use crate::{CoreError, Result, names, pyfmt, report, tree};

/// Default cap for [`ExperimentDir::rollout_file`] and audit text reads.
pub const DEFAULT_READ_CAP: u64 = 2 * 1024 * 1024;

/// Most entries walked by [`ExperimentDir::size_bytes`].
const MAX_SIZE_ENTRIES: usize = 5_000_000;
/// Largest task prompt read for [`TaskDetail::task_prompt`].
const MAX_PROMPT_BYTES: u64 = 256 * 1024;

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

/// Upstream `_spend_and_fallbacks` for one summary and mode: `(fell_back, stage spend,
/// selection spend)`.
fn mode_spend(record: &Value, mode: Mode) -> (u32, f64, f64) {
    let rec = mode_rec(record, mode);
    let fell_back = usize_u32(list(field(rec, "fell_back")).len());
    let spend = pyfmt::sum(
        list(field(rec, "stages"))
            .iter()
            .map(|s| number(field(s, "usd"))),
    );
    let sel = number(field(selection_rec(record, mode), "usd"));
    (fell_back, spend, sel)
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

/// Judge points exactly as harness2 wrote them. JobBench's weighted judging writes floats
/// (`total_score`/`max_score`), which the `u32` wire fields cannot hold; the rows carry these
/// raw values as `points_passed`/`points_max`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Points {
    passed: Option<f64>,
    max: Option<f64>,
}

fn points(v: &Value) -> Points {
    Points {
        passed: v.get("n_passed").and_then(Value::as_f64),
        max: v.get("n_criteria").and_then(Value::as_f64),
    }
}

/// A judged row as `core.judge._load_rows` rebuilds it: `graded` and `frac` are recomputed
/// from `n_passed`/`n_criteria` (weighted JobBench scores are floats).
fn judged_row(v: &Value) -> Option<JudgedRow> {
    let mode_v = v.get("mode").or_else(|| v.get("arm"))?;
    let mode = Mode::from_token(mode_v.as_str()?)?;
    let Points {
        passed,
        max: criteria,
    } = points(v);
    let graded = criteria.is_some_and(|c| c > 0.0) && passed.is_some();
    let frac = match (graded, passed, criteria) {
        (true, Some(p), Some(c)) => Some(p / c),
        _ => None,
    };
    let metric = match field(field(v, "raw"), "metric") {
        Value::Null | Value::Bool(false) => None,
        Value::String(s) if s.is_empty() => None,
        other => Some(text(other)),
    };
    Some(JudgedRow {
        task_id: text(field(v, "task_id")),
        mode,
        k: count(field(v, "k")),
        label: text(field(v, "label")),
        run_id: text(field(v, "run_id")),
        chosen_is_base: truthy(field(v, "chosen_is_base")),
        n_passed: passed.and_then(pyfmt::exact_u32),
        n_criteria: criteria.and_then(pyfmt::exact_u32),
        all_pass: truthy(field(v, "all_pass")),
        frac,
        graded,
        error: text(field(v, "error")),
        metric,
        points_passed: passed,
        points_max: criteria,
    })
}

/// A `base_judged.json` row (`{task, label, run_id, score}`), with the raw fraction.
fn base_row(v: &Value) -> (BaseJudgedRow, Option<f64>) {
    let score = field(v, "score");
    let Points {
        passed,
        max: criteria,
    } = points(score);
    let frac = match (passed, criteria) {
        (Some(p), Some(c)) if c > 0.0 => Some(p / c),
        _ => None,
    };
    let row = BaseJudgedRow {
        task_id: text(field(v, "task")),
        label: text(field(v, "label")),
        run_id: text(field(v, "run_id")),
        n_passed: passed.and_then(pyfmt::exact_u32),
        n_criteria: criteria.and_then(pyfmt::exact_u32),
        all_pass: match score.get("all_pass") {
            Some(Value::Bool(b)) => Some(*b),
            Some(Value::Null) | None => None,
            Some(other) => Some(truthy(other)),
        },
        points_passed: passed,
        points_max: criteria,
    };
    (row, frac)
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

/// A parsed `summary.json` and the directory it came from.
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

    /// Task directories of the sweep with their parsed `summary.json`, in upstream's
    /// `sorted(glob("*/summary.json"))` order. Unreadable or malformed summaries are `None`.
    fn summaries(&self, sweep: &Namespace) -> Result<Vec<(String, Option<TaskSummary>)>> {
        let dir = self.sweep_dir(sweep)?;
        let mut out = Vec::new();
        for e in safefs::list_opt(&self.dir, &dir, "sweep")? {
            if e.kind != EntryKind::Dir || names::task_slug(&e.name).is_err() {
                continue;
            }
            let rel = format!("{dir}/{}/summary.json", e.name);
            match safefs::read_json_opt(&self.dir, &rel, "summary.json") {
                Ok(Some(raw)) if raw.is_object() => {
                    out.push((e.name.clone(), Some(TaskSummary { slug: e.name, raw })));
                }
                Ok(None) => {}
                Ok(Some(_)) | Err(CoreError::Malformed { .. }) => out.push((e.name, None)),
                Err(err) => return Err(err),
            }
        }
        Ok(out)
    }

    fn summary(&self, sweep: &Namespace, slug: &str) -> Result<TaskSummary> {
        names::task_slug(slug)?;
        let rel = format!("{}/{slug}/summary.json", self.sweep_dir(sweep)?);
        match safefs::read_json_opt(&self.dir, &rel, "summary.json")? {
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
        Ok(
            match safefs::read_json_opt(&self.dir, &rel, "judged.json")? {
                Some(Value::Array(rows)) => rows.iter().filter_map(judged_row).collect(),
                _ => Vec::new(),
            },
        )
    }

    fn base_rows(&self, sweep: &Namespace) -> Result<Vec<(BaseJudgedRow, Option<f64>)>> {
        let rel = format!("{}/base_judged.json", self.sweep_dir(sweep)?);
        Ok(
            match safefs::read_json_opt(&self.dir, &rel, "base_judged.json")? {
                Some(Value::Array(rows)) => rows.iter().map(base_row).collect(),
                _ => Vec::new(),
            },
        )
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
                Some(s) => {
                    match text(field(&s.raw, "status")).as_str() {
                        "ok" | "skipped" => ok += 1,
                        "partial" => partial += 1,
                        _ => failed += 1,
                    }
                    for m in modes_present(&s.raw) {
                        if !modes.contains(&m) {
                            modes.push(m);
                        }
                        let (_, spend, sel) = mode_spend(&s.raw, m);
                        usd += spend + sel;
                    }
                }
                None => failed += 1,
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
    fn stream_order(&self, sweep: &Namespace) -> Result<Vec<String>> {
        let record = match self.kind {
            ExperimentKind::Demo => safefs::read_json_opt(&self.dir, "demo.json", "demo.json")
                .ok()
                .flatten(),
            ExperimentKind::Standard => {
                let root = format!("{}/_sweeps", self.ns_root(&sweep.bench_name)?);
                let ns = sweep.summary().to_string();
                let newest = safefs::list_opt(&self.dir, &root, "sweep records")?
                    .into_iter()
                    .filter(|e| e.kind == EntryKind::File)
                    .filter_map(|e| {
                        record_stamp(&e.name, &ns).map(|s| (s.to_owned(), e.name.clone()))
                    })
                    .max();
                match newest {
                    Some((_, name)) => {
                        safefs::read_json_opt(&self.dir, &format!("{root}/{name}"), "sweep record")
                            .ok()
                            .flatten()
                    }
                    None => None,
                }
            }
        };
        let Some(rec) = record else {
            return Ok(Vec::new());
        };
        let mut order = Vec::new();
        if rec.get("tasks").is_some() {
            // demo.json: one run_domain report.
            for t in list(field(&rec, "tasks")) {
                order.push(text(field(t, "task")));
            }
            return Ok(order);
        }
        let domains = list(field(&rec, "domains"));
        let mut names_in_order: Vec<String> =
            list(field(&rec, "domain_order")).iter().map(text).collect();
        for d in domains {
            let name = text(field(d, "domain"));
            if !names_in_order.contains(&name) {
                names_in_order.push(name);
            }
        }
        for name in names_in_order {
            if let Some(d) = domains.iter().find(|d| text(field(d, "domain")) == name) {
                for t in list(field(d, "tasks")) {
                    order.push(text(field(t, "task")));
                }
            }
        }
        Ok(order)
    }

    /// Task rows of one sweep (`sweep` is the summary namespace), in stream order when a
    /// sweep record exists, else sorted by task id.
    pub fn tasks(&self, sweep: &Namespace) -> Result<Vec<TaskRow>> {
        let judged = self.judged_rows(sweep)?;
        let mut scores: HashMap<(String, Mode), &JudgedRow> = HashMap::new();
        for r in judged.iter().filter(|r| r.k == sweep.k) {
            scores.insert((r.task_id.clone(), r.mode), r);
        }
        let mut rows: Vec<TaskRow> = Vec::new();
        for (slug, s) in self.summaries(sweep)? {
            let Some(s) = s else {
                rows.push(TaskRow {
                    task_id: slug.replace("__", "/"),
                    slug,
                    domain: String::new(),
                    status: "unreadable".into(),
                    modes: Vec::new(),
                    errors: 1,
                });
                continue;
            };
            let task_id = s.task_id();
            let modes = modes_present(&s.raw)
                .into_iter()
                .map(|mode| {
                    let rec = mode_rec(&s.raw, mode);
                    let sel = selection_rec(&s.raw, mode);
                    let (fell_back, spend, sel_usd) = mode_spend(&s.raw, mode);
                    let chosen = text(field(sel, "chosen_label"));
                    let judged = scores.get(&(task_id.clone(), mode));
                    TaskModeOutcome {
                        mode,
                        ok: truthy(field(rec, "ok"))
                            && truthy(field(sel, "ok"))
                            && !chosen.is_empty(),
                        chosen_label: (!chosen.is_empty()).then_some(chosen),
                        // "The adopted harness is an unchanged base": harness2 records
                        // that as `chosen_tree_is_base` (the selector's own
                        // `chosen_is_base` only says a reference run was picked).
                        chosen_is_base: sel
                            .get("chosen_tree_is_base")
                            .map_or_else(|| truthy(field(sel, "chosen_is_base")), truthy),
                        fell_back,
                        usd: spend + sel_usd,
                        score: judged.and_then(|r| r.frac),
                        all_pass: judged.filter(|r| r.graded).map(|r| r.all_pass),
                    }
                })
                .collect();
            rows.push(TaskRow {
                domain: text(field(&s.raw, "domain")),
                status: text(field(&s.raw, "status")),
                errors: usize_u32(list(field(&s.raw, "errors")).len()),
                modes,
                slug: s.slug.clone(),
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
        let ledger_order: Vec<String> = list(field(field(rec, "ledger"), "rollouts"))
            .iter()
            .map(text)
            .collect();
        let key = |l: &str| {
            let base = u8::from(!l.starts_with("base"));
            let pos = ledger_order
                .iter()
                .position(|x| x == l)
                .unwrap_or(usize::MAX);
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
                    .unwrap_or(h2s_api::Component::SystemPrompt),
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
        let mut out: Vec<FileEntry> = safefs::list(&dir)?
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
            let Some(s) = s else { continue };
            for &mode in modes {
                let key = (mode, sweep.k);
                let (fb, spend, sel) = mode_spend(&s.raw, mode);
                *fell_back.entry(key).or_insert(0) += fb;
                let acc = usd.get(&key).copied().unwrap_or(0.0);
                usd.insert(key, acc + spend + sel);
            }
        }
        Ok((fell_back, usd))
    }

    /// Experience libraries of the sweep, one per (mode, domain).
    pub fn library(&self, sweep: &Namespace) -> Result<Vec<LibraryDomain>> {
        let Some(root) = namespace::library_root(self.kind, sweep) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for mode in Mode::ALL {
            let mode_root = format!("{root}/{}", mode.token());
            for d in safefs::list_opt(&self.dir, &mode_root, "library")? {
                if d.kind != EntryKind::Dir || names::task_slug(&d.name).is_err() {
                    continue;
                }
                let dom_rel = format!("{mode_root}/{}", d.name);
                let index = safefs::read_text_opt(
                    &self.dir,
                    &format!("{dom_rel}/INDEX.md"),
                    MAX_TEXT_BYTES,
                    "INDEX.md",
                )?;
                let domain = index
                    .as_deref()
                    .and_then(|t| t.lines().next())
                    .and_then(|l| l.strip_prefix("# Library — "))
                    .map_or_else(|| d.name.clone(), str::to_owned);
                let mut entries = Vec::new();
                let harnesses = format!("{dom_rel}/harnesses");
                for e in safefs::list_opt(&self.dir, &harnesses, "library entries")? {
                    if e.kind != EntryKind::Dir || names::task_slug(&e.name).is_err() {
                        continue;
                    }
                    let exp = safefs::read_text_opt(
                        &self.dir,
                        &format!("{harnesses}/{}/EXPERIENCE.md", e.name),
                        MAX_TEXT_BYTES,
                        "EXPERIENCE.md",
                    )?;
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
        let mut out = Vec::new();
        for (stem, (mut parts, has_meta, mtime)) in stems {
            parts.sort_by_key(|p| AUDIT_PARTS.iter().position(|x| x == p));
            let meta = if has_meta {
                safefs::read_json_opt(&self.dir, &format!("{dir}/{stem}.meta.json"), "meta.json")
                    .ok()
                    .flatten()
                    .unwrap_or(Value::Null)
            } else {
                Value::Null
            };
            let label = match meta.get("label") {
                Some(Value::String(l)) if !l.is_empty() => l.clone(),
                _ => audit_label_of(&stem),
            };
            out.push((
                mtime,
                AuditSession {
                    id: stem,
                    label,
                    ok: truthy(field(&meta, "ok")),
                    channel: text(field(&meta, "channel")),
                    model: text(field(&meta, "model")),
                    effort: text(field(&meta, "effort")),
                    attempts: count(field(&meta, "attempts")),
                    wall_s: number(field(&meta, "wall_s")),
                    cost_usd: number(field(&meta, "cost_usd")),
                    error: text(field(&meta, "error")),
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
            let Some(s) = s else { continue };
            for mode in modes_present(&s.raw) {
                let (_, spend, sel) = mode_spend(&s.raw, mode);
                total += spend + sel;
            }
        }
        Ok(total)
    }

    /// `config.json` identities of every sweep (for the manifest).
    pub fn sweep_identities(&self) -> Result<Vec<SweepIdentity>> {
        let mut out = Vec::new();
        for ns in self.summary_namespaces()? {
            let rel = format!("{}/config.json", self.sweep_dir(&ns)?);
            if let Some(text) =
                safefs::read_text_opt(&self.dir, &rel, MAX_JSON_BYTES, "config.json")?
            {
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
    pub fn results_digest(&self) -> Result<String> {
        let mut paths: Vec<String> = Vec::new();
        for (bench, root) in self.bench_roots() {
            for e in safefs::list_opt(&self.dir, &root, "namespaces")? {
                if e.kind != EntryKind::Dir
                    || Namespace::parse(&e.name).is_none_or(|ns| ns.bench_name != bench)
                {
                    continue;
                }
                let ns_rel = format!("{root}/{}", e.name);
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
        let mut buf = vec![0_u8; 64 * 1024];
        for p in paths {
            let (mut file, _) = safefs::open_file(&self.dir, &p, "results file")?;
            h.update(p.as_bytes());
            h.update(b"\0");
            loop {
                let n = std::io::Read::read(&mut file, &mut buf)?;
                if n == 0 {
                    break;
                }
                h.update(buf.get(..n).unwrap_or_default());
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
    summaries: &[h2s_api::ModeSummary],
    title: &str,
    bench: &str,
) -> (String, Vec<String>) {
    let (rendered, notes) = report::render(summaries, title, bench);
    let mut by_mode: Vec<&h2s_api::ModeSummary> = summaries.iter().collect();
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
    #[test]
    fn weighted_points_are_kept_exactly() {
        let v = serde_json::json!({
            "task_id": "t1", "mode": "agg", "k": 1, "label": "agg1", "run_id": "ns/t1/agg1",
            "chosen_is_base": false, "n_passed": 7.5, "n_criteria": 10.0, "all_pass": false,
        });
        let r = judged_row(&v).unwrap();
        assert_eq!((r.n_passed, r.n_criteria), (None, Some(10)));
        assert_eq!((r.points_passed, r.points_max), (Some(7.5), Some(10.0)));
        assert_eq!(r.frac, Some(0.75));
        let v = serde_json::json!({
            "task_id": "t2", "mode": "seq", "k": 1, "label": "seq1", "run_id": "ns/t2/seq1",
            "chosen_is_base": false, "n_passed": 3, "n_criteria": 4, "all_pass": false,
        });
        let r = judged_row(&v).unwrap();
        assert_eq!((r.n_passed, r.n_criteria), (Some(3), Some(4)));
        assert_eq!((r.points_passed, r.points_max), (Some(3.0), Some(4.0)));
        let (b, frac) = base_row(&serde_json::json!({
            "task": "t1", "label": "base", "run_id": "ns/t1/base",
            "score": {"n_passed": 2.25, "n_criteria": 3, "all_pass": false},
        }));
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
