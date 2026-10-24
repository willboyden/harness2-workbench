// Parses the output formats of harness2 (https://github.com/google-research/harness2) at
// commit 1cfe254, Copyright 2026 Google LLC, Apache License 2.0; see NOTICE.

//! Typed results from harness2's output.
//!
//! Every parser is total: unexpected input yields `None`/defaults or
//! [`CoreError::Malformed`], never a panic, because the text comes from a subprocess.
//! Formats are those of harness2 at [`crate::TESTED_HARNESS2_COMMIT`]; tests use captured
//! real output under `tests/fixtures/`.

use std::sync::LazyLock;

use h2s_api::{
    CheckResult, DemoResult, DomainCount, JudgeResult, Mode, ModeNamespace, RunResult, SetupResult,
    StreamEntry,
};
use regex::Regex;

use crate::namespace::Namespace;
use crate::{CoreError, Result};

fn lines(stdout: &str) -> impl Iterator<Item = &str> {
    stdout.lines().map(|l| l.trim_end_matches('\r'))
}

/// `harness2-check` stdout: `FAIL: ...` lines, then `Setup incomplete: ...` or
/// `Ready: <bench>/<sub>; N tasks. ...`.
#[must_use]
pub fn check_output(stdout: &str) -> CheckResult {
    let mut failures = Vec::new();
    let mut ready = false;
    let mut tasks = None;
    for line in lines(stdout) {
        if let Some(f) = line.strip_prefix("FAIL: ") {
            failures.push(f.to_owned());
        } else if let Some(rest) = line.strip_prefix("Ready: ") {
            ready = true;
            tasks = rest
                .split_once("; ")
                .and_then(|(_, r)| r.split_once(" tasks."))
                .and_then(|(n, _)| n.trim().parse::<u32>().ok());
        }
    }
    CheckResult {
        ready: ready && failures.is_empty(),
        tasks,
        failures,
    }
}

/// Parsed `harness2 --dry-run` stdout (the server adds the cost estimate).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    pub header: String,
    pub namespaces: Vec<ModeNamespace>,
    pub summary_namespace: String,
    pub library_root: Option<String>,
    pub workers: u32,
    pub stream: Vec<StreamEntry>,
    pub domains: Vec<DomainCount>,
    pub total_tasks: u32,
    pub total_domains: u32,
}

const DOMAINS_MARKER: &str =
    "# domains of this split, with task counts (the legal --only-domains names):";

/// `{n:>4}  {rest}` -> `(n, rest)`.
fn numbered(line: &str) -> Option<(u32, &str)> {
    let t = line.trim_start();
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let (n, rest) = t.split_at(digits);
    Some((n.parse().ok()?, rest.strip_prefix("  ")?))
}

/// Split `{domain:<40} {task}` using the domain names listed by the preview.
fn split_stream_rest<'a>(rest: &'a str, domains: &[&str]) -> Option<(String, &'a str)> {
    let mut known: Vec<&str> = domains.to_vec();
    known.sort_by_key(|d| std::cmp::Reverse(d.len()));
    for d in known {
        let padded = format!("{d:<40}");
        if let Some(task) = rest.strip_prefix(&padded).and_then(|r| r.strip_prefix(' '))
            && !task.is_empty()
        {
            return Some((d.to_owned(), task));
        }
    }
    // Unknown domain: the padded field is 40 characters, then one space.
    let cut = rest.char_indices().nth(40).map(|(i, _)| i);
    if let Some(i) = cut
        && rest.get(i..i + 1) == Some(" ")
    {
        let domain = rest.get(..i)?.trim_end();
        let task = rest.get(i + 1..)?;
        if !domain.is_empty() && !task.is_empty() && !domain.contains("  ") {
            return Some((domain.to_owned(), task));
        }
    }
    let (domain, task) = rest.split_once(' ')?;
    Some((domain.to_owned(), task.trim_start()))
}

/// Parse `harness2 --dry-run` stdout (see upstream `cli/run_sweep.py::_print_stream`).
pub fn preview_output(stdout: &str) -> Result<Preview> {
    let all: Vec<&str> = lines(stdout).collect();
    let Some(header_at) = all
        .iter()
        .position(|l| l.starts_with("# ") && l.contains(" split=") && l.contains(" modes="))
    else {
        return Err(CoreError::Malformed {
            what: "preview output".into(),
            detail: "no `# <bench>-<substrate> split=...` header line".into(),
        });
    };
    let body = all.get(header_at..).unwrap_or_default();
    let header = (*body.first().unwrap_or(&"")).to_owned();
    let mut namespaces = Vec::new();
    let mut summary_path: Option<&str> = None;
    let mut library_root = None;
    let mut workers = 1;
    let mut stream_lines: Vec<&str> = Vec::new();
    let mut domain_lines: Vec<&str> = Vec::new();
    let mut totals: Option<(u32, u32)> = None;
    let mut in_domains = false;
    for &line in body.iter().skip(1) {
        if let Some(rest) = line.strip_prefix("#   namespace[") {
            if let Some((label, ns)) = rest.split_once("]: ")
                && let Some(mode) = Mode::from_token(label)
            {
                namespaces.push(ModeNamespace {
                    mode,
                    namespace: ns.trim().to_owned(),
                });
            }
        } else if let Some(p) = line.strip_prefix("#   summaries under: ") {
            summary_path = Some(p.trim());
        } else if let Some(p) = line.strip_prefix("#   library root:") {
            library_root = Some(p.trim().to_owned());
        } else if let Some(w) = line.strip_prefix("#   domains parallel: ") {
            workers = w.trim().parse().unwrap_or(1);
        } else if line == DOMAINS_MARKER {
            in_domains = true;
        } else if let Some(rest) = line.strip_prefix("# ") {
            if let Some((n, tail)) = rest.split_once(" tasks over ")
                && let Some((m, _)) = tail.split_once(" domains")
                && let (Ok(n), Ok(m)) = (n.trim().parse(), m.trim().parse())
            {
                totals = Some((n, m));
            }
        } else if !line.starts_with('#') && !line.trim().is_empty() {
            if in_domains {
                domain_lines.push(line);
            } else {
                stream_lines.push(line);
            }
        }
    }
    let mut domains = Vec::new();
    for line in &domain_lines {
        if let Some((n, d)) = numbered(line) {
            domains.push(DomainCount {
                domain: d.to_owned(),
                tasks: n,
            });
        }
    }
    let names: Vec<&str> = domains.iter().map(|d| d.domain.as_str()).collect();
    let mut stream = Vec::new();
    for line in &stream_lines {
        if let Some((index, rest)) = numbered(line)
            && let Some((domain, task)) = split_stream_rest(rest, &names)
        {
            stream.push(StreamEntry {
                index,
                domain,
                task: task.to_owned(),
            });
        }
    }
    let summary_namespace = summary_path
        .and_then(|p| p.trim_end_matches('/').rsplit('/').next())
        .map(str::to_owned)
        .or_else(|| {
            namespaces
                .iter()
                .find_map(|m| Namespace::parse(&m.namespace))
                .map(|ns| ns.summary().to_string())
        })
        .unwrap_or_default();
    let (total_tasks, total_domains) = totals.unwrap_or((
        u32::try_from(stream.len()).unwrap_or(u32::MAX),
        u32::try_from(domains.len()).unwrap_or(u32::MAX),
    ));
    Ok(Preview {
        header,
        namespaces,
        summary_namespace,
        library_root,
        workers,
        stream,
        domains,
        total_tasks,
        total_domains,
    })
}

/// One `  [<domain>] i/n <status> <task>[  :: <error>]` progress line from a sweep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskProgress {
    pub domain: String,
    pub index: u32,
    pub total: u32,
    /// `ok`, `skipped`, `partial`, `base-failed`, `exception`, ...
    pub status: String,
    pub task: String,
    pub error: Option<String>,
}

static PROGRESS: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"^\s*\[(?P<domain>.+)\] (?P<i>\d+)/(?P<n>\d+) (?P<status>\S+) +(?P<rest>\S.*)$")
        .ok()
});

#[must_use]
pub fn progress_line(line: &str) -> Option<TaskProgress> {
    let line = line.trim_end_matches(['\r', '\n']);
    let caps = PROGRESS.as_ref()?.captures(line)?;
    let index: u32 = caps.name("i")?.as_str().parse().ok()?;
    let total: u32 = caps.name("n")?.as_str().parse().ok()?;
    if index == 0 || total == 0 || index > total {
        return None;
    }
    let rest = caps.name("rest")?.as_str();
    let (task, error) = match rest.split_once("  :: ") {
        Some((t, e)) => (t.trim_end(), Some(e.to_owned())),
        None => (rest.trim_end(), None),
    };
    Some(TaskProgress {
        domain: caps.name("domain")?.as_str().to_owned(),
        index,
        total,
        status: caps.name("status")?.as_str().to_owned(),
        task: task.to_owned(),
        error,
    })
}

static SWEEP_DONE: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"^sweep (\S+): (\d+) tasks ok, (\d+) failed, (\d+) domain\(s\) died(?:: (.*))?$")
        .ok()
});

/// The tail of a sweep: `sweep <ns>: X tasks ok, Y failed, Z domain(s) died[: a, b]`,
/// `record: <path>`, `next:   <command>`. Also recognises the zero-tasks refusal.
#[must_use]
pub fn run_output(stdout: &str) -> Option<RunResult> {
    let mut result: Option<RunResult> = None;
    for line in lines(stdout) {
        if let Some(c) = SWEEP_DONE.as_ref().and_then(|r| r.captures(line)) {
            let dead = c
                .get(5)
                .map(|m| {
                    m.as_str()
                        .split(", ")
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            result = Some(RunResult {
                namespace: c.get(1).map_or("", |m| m.as_str()).to_owned(),
                done: c.get(2).and_then(|m| m.as_str().parse().ok()).unwrap_or(0),
                failed: c.get(3).and_then(|m| m.as_str().parse().ok()).unwrap_or(0),
                dead_domains: dead,
                record: None,
                next: None,
            });
        } else if let Some(rest) = line.strip_prefix("sweep ")
            && let Some((ns, tail)) = rest.split_once(": zero tasks scheduled")
            && tail.ends_with("refusing to report success")
        {
            result = Some(RunResult {
                namespace: ns.to_owned(),
                done: 0,
                failed: 0,
                dead_domains: Vec::new(),
                record: None,
                next: None,
            });
        } else if let Some(path) = line.strip_prefix("record: ") {
            if let Some(r) = result.as_mut() {
                r.record = path
                    .trim()
                    .rsplit('/')
                    .next()
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned);
            }
        } else if let Some(cmd) = line.strip_prefix("next:")
            && let Some(r) = result.as_mut()
        {
            r.next = Some(cmd.trim().to_owned()).filter(|s| !s.is_empty());
        }
    }
    result
}

static JUDGED: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"^judged\.json: (\d+) row\(s\), (\d+) graded, (\d+) ungraded").ok()
});
static BASES: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^wrote (.+): (\d+) row\(s\), (\d+) graded$").ok());

fn num(c: &regex::Captures<'_>, i: usize) -> u32 {
    c.get(i).and_then(|m| m.as_str().parse().ok()).unwrap_or(0)
}

/// The tail of `harness2-judge`: `judged.json: N row(s), G graded, U ungraded (...)` or,
/// with `--bases`, `wrote <path>: N row(s), G graded`.
///
/// With `--bases`, `no reference executions recorded under ...; wrote nothing` yields zero
/// rows.
#[must_use]
pub fn judge_output(stdout: &str, bases: bool) -> Option<JudgeResult> {
    let mut out = None;
    for line in lines(stdout) {
        if bases {
            if let Some(c) = BASES.as_ref().and_then(|r| r.captures(line)) {
                let rows = num(&c, 2);
                let graded = num(&c, 3);
                out = Some(JudgeResult {
                    bases: true,
                    rows,
                    graded,
                    ungraded: rows.saturating_sub(graded),
                });
            } else if line.starts_with("no reference executions recorded under ")
                && line.ends_with("; wrote nothing")
            {
                out = Some(JudgeResult {
                    bases: true,
                    rows: 0,
                    graded: 0,
                    ungraded: 0,
                });
            }
        } else if let Some(c) = JUDGED.as_ref().and_then(|r| r.captures(line)) {
            out = Some(JudgeResult {
                bases: false,
                rows: num(&c, 1),
                graded: num(&c, 2),
                ungraded: num(&c, 3),
            });
        }
    }
    out
}

fn malformed(what: &str, detail: impl Into<String>) -> CoreError {
    CoreError::Malformed {
        what: what.to_owned(),
        detail: detail.into(),
    }
}

fn count_field(v: &serde_json::Value, key: &str) -> Option<u32> {
    let n = v.get(key)?;
    match n.as_u64() {
        Some(x) => u32::try_from(x).ok(),
        None => n.as_f64().and_then(crate::pyfmt::exact_u32),
    }
}

/// `demo.json` written by `harness2-demo` (`done`, `failed`, `execution_count`,
/// `editor_calls`).
pub fn demo_json(text: &str) -> Result<DemoResult> {
    let v: serde_json::Value =
        serde_json::from_str(text).map_err(|e| malformed("demo.json", e.to_string()))?;
    if !v.is_object() {
        return Err(malformed("demo.json", "expected a JSON object"));
    }
    let need = |key: &str| {
        count_field(&v, key)
            .ok_or_else(|| malformed("demo.json", format!("missing or invalid `{key}`")))
    };
    Ok(DemoResult {
        done: need("done")?,
        failed: need("failed")?,
        execution_count: count_field(&v, "execution_count").unwrap_or(0),
        editor_calls: count_field(&v, "editor_calls").unwrap_or(0),
    })
}

/// `harness2-setup` stdout: each printed command (`[cwd] cmd ...` or `cmd ...`) becomes a
/// plan line; `Note: ...` and `Configure WorkBuddy ...` lines become notes.
///
/// The closing `Setup plan complete.` / `Installation complete. ...` line is neither.
#[must_use]
pub fn setup_output(stdout: &str, dry_run: bool) -> SetupResult {
    let mut plan = Vec::new();
    let mut notes = Vec::new();
    for line in lines(stdout) {
        if line.trim().is_empty()
            || line == "Setup plan complete."
            || line.starts_with("Installation complete.")
        {
            continue;
        }
        if line.starts_with("Note: ")
            || line.starts_with("Configure WorkBuddy ")
            || line.starts_with("setup failed: ")
        {
            notes.push(line.to_owned());
        } else {
            plan.push(line.to_owned());
        }
    }
    SetupResult {
        dry_run,
        plan,
        notes,
    }
}

/// The split file `scripts/split_tasks.py` wrote: `(train, test)` counts.
pub fn split_file(text: &str) -> Result<(u32, u32)> {
    let v: serde_json::Value =
        serde_json::from_str(text).map_err(|e| malformed("split file", e.to_string()))?;
    let side = |key: &str| -> Result<u32> {
        let list = v
            .get(key)
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| malformed("split file", format!("`{key}` must be a list")))?;
        if !list.iter().all(serde_json::Value::is_string) {
            return Err(malformed(
                "split file",
                format!("`{key}` must list task ids"),
            ));
        }
        Ok(u32::try_from(list.len()).unwrap_or(u32::MAX))
    };
    Ok((side("train")?, side("test")?))
}

/// Settings `harness2-bridge` prints before it starts serving. **Contains the bridge's
/// API key**: the server must store it as a secret and add it to the job's redactor
/// before the line reaches any log. `Debug` must not print the key.
#[derive(Clone, PartialEq, Eq)]
pub struct BridgeSettings {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub container_url: Option<String>,
}

impl std::fmt::Debug for BridgeSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BridgeSettings")
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
            .field("container_url", &self.container_url)
            .finish()
    }
}

/// Parse one stdout line of `harness2-bridge` into the settings accumulated so far.
/// Returns `true` when the line carried a setting (so the caller can keep it out of logs).
pub fn bridge_line(line: &str, acc: &mut BridgeSettings) -> bool {
    let line = line.trim_end_matches(['\r', '\n']);
    let Some((name, value)) = line.split_once('=') else {
        return false;
    };
    match name {
        "HARNESS2_CODEX_BASE_URL" => acc.base_url = value.to_owned(),
        "HARNESS2_CODEX_API_KEY" => acc.api_key = value.to_owned(),
        "HARNESS2_CODEX_MODEL" => acc.model = value.to_owned(),
        "HARNESS2_CODEX_CONTAINER_URL" => acc.container_url = Some(value.to_owned()),
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn check_lines() {
        let r = check_output(
            "FAIL: pandoc missing; install pandoc and poppler-utils (README.md)\n\
             Setup incomplete: 1 issue(s). See docs/BENCHMARKS.md.\n",
        );
        assert!(!r.ready);
        assert_eq!(
            r.failures,
            vec!["pandoc missing; install pandoc and poppler-utils (README.md)"]
        );
        let r = check_output(
            "Ready: lab/oc; 1253 tasks. Model access is not exercised; the first real run is \
             the first model call.\n",
        );
        assert!(r.ready);
        assert_eq!(r.tasks, Some(1253));
    }

    #[test]
    fn progress_lines() {
        let p = progress_line("  [accounting] 1/2 ok           ledger-1").unwrap();
        assert_eq!(
            (
                p.domain.as_str(),
                p.index,
                p.total,
                p.status.as_str(),
                p.task.as_str()
            ),
            ("accounting", 1, 2, "ok", "ledger-1")
        );
        assert_eq!(p.error, None);
        let p = progress_line(
            "  [practice area] 3/9 base-failed  contracts/nda-7  :: no base rollout produced anything",
        )
        .unwrap();
        assert_eq!(p.domain, "practice area");
        assert_eq!(p.status, "base-failed");
        assert_eq!(p.task, "contracts/nda-7");
        assert_eq!(
            p.error.as_deref(),
            Some("no base rollout produced anything")
        );
        assert!(progress_line("  [ledger-1] auto-triggered skill(s): x").is_none());
        assert!(progress_line("  [lab-oc] re-rolling lab-oc-agg1/a/base1: reason").is_none());
        assert!(progress_line("  [d] 3/2 ok t").is_none());
    }

    #[test]
    fn run_and_judge_tails() {
        let out = "sweep lab-oc-agg1: 1 domain(s) in parallel, tasks within a domain in stream order\n  [a] 1/1 ok  a/t\n\nsweep lab-oc-agg1: 3 tasks ok, 1 failed, 2 domain(s) died: x, y z\nrecord: /runs/lab/adapt/_sweeps/lab-oc-agg1-20261023T120000Z.json\nnext:   python3 -m harness2.cli.judge --bench lab --substrate oc --k 1 --modes parallel\n";
        let r = run_output(out).unwrap();
        assert_eq!(r.namespace, "lab-oc-agg1");
        assert_eq!((r.done, r.failed), (3, 1));
        assert_eq!(r.dead_domains, vec!["x", "y z"]);
        assert_eq!(
            r.record.as_deref(),
            Some("lab-oc-agg1-20261023T120000Z.json")
        );
        assert_eq!(
            r.next.as_deref(),
            Some("python3 -m harness2.cli.judge --bench lab --substrate oc --k 1 --modes parallel")
        );
        let zero = "\nsweep jb-oc-agg2-s7: zero tasks scheduled (split='all', only_domains=('x',), max_tasks=None); refusing to report success\nrecord: /r/jb/adapt/_sweeps/jb-oc-agg2-s7-20261023T000000Z.json\n";
        let r = run_output(zero).unwrap();
        assert_eq!((r.namespace.as_str(), r.done), ("jb-oc-agg2-s7", 0));
        assert!(r.record.is_some());
        assert!(run_output("nothing").is_none());
        let j = judge_output(
            "judging 4 selection(s) under lab-oc-agg1 ...\njudged.json: 4 row(s), 3 graded, 1 ungraded (judge failure or no pick — NOT zeros)\nwrote /x/judged.json\n",
            false,
        )
        .unwrap();
        assert_eq!((j.rows, j.graded, j.ungraded, j.bases), (4, 3, 1, false));
        let b = judge_output(
            "  t base1 1/2\nwrote /x: y/base_judged.json: 2 row(s), 1 graded\n",
            true,
        )
        .unwrap();
        assert_eq!((b.rows, b.graded, b.ungraded, b.bases), (2, 1, 1, true));
        assert!(judge_output("", true).is_none());
    }

    #[test]
    fn demo_split_bridge() {
        let d = demo_json(r#"{"done": 2, "failed": 0, "execution_count": 28, "editor_calls": 32}"#)
            .unwrap();
        assert_eq!(
            (d.done, d.failed, d.execution_count, d.editor_calls),
            (2, 0, 28, 32)
        );
        assert!(demo_json("[]").is_err());
        assert!(demo_json(r#"{"done": -1, "failed": 0}"#).is_err());
        assert_eq!(
            split_file(r#"{"train": ["a","b"], "test": ["c"]}"#).unwrap(),
            (2, 1)
        );
        assert!(split_file(r#"{"train": [1]}"#).is_err());
        let mut acc = BridgeSettings {
            base_url: String::new(),
            api_key: String::new(),
            model: String::new(),
            container_url: None,
        };
        assert!(!bridge_line(
            "Add these to .env, then run harness2-setup <bench> --substrate cx:",
            &mut acc
        ));
        assert!(bridge_line(
            "HARNESS2_CODEX_BASE_URL=http://127.0.0.1:4100/v1",
            &mut acc
        ));
        assert!(bridge_line(
            "HARNESS2_CODEX_API_KEY=abc-secret-123",
            &mut acc
        ));
        assert!(bridge_line(
            "HARNESS2_CODEX_MODEL=gemini-3.5-flash",
            &mut acc
        ));
        assert!(bridge_line(
            "HARNESS2_CODEX_CONTAINER_URL=http://host.docker.internal:4100/v1",
            &mut acc
        ));
        assert_eq!(acc.api_key, "abc-secret-123");
        assert!(!format!("{acc:?}").contains("abc-secret"));
    }

    #[test]
    fn setup_lines() {
        let s = setup_output(
            "Note: runners missing\ngit init -q /d/opencode\n[/d/opencode] bun install\nConfigure WorkBuddy solver: gemini\nSetup plan complete.\n",
            true,
        );
        assert_eq!(
            s.plan,
            vec!["git init -q /d/opencode", "[/d/opencode] bun install"]
        );
        assert_eq!(
            s.notes,
            vec![
                "Note: runners missing",
                "Configure WorkBuddy solver: gemini"
            ]
        );
    }

    proptest! {
        #[test]
        fn parsers_never_panic(s in "\\PC{0,200}") {
            let _ = check_output(&s);
            let _ = preview_output(&s);
            let _ = progress_line(&s);
            let _ = run_output(&s);
            let _ = judge_output(&s, true);
            let _ = judge_output(&s, false);
            let _ = demo_json(&s);
            let _ = setup_output(&s, false);
            let _ = split_file(&s);
            let mut acc = BridgeSettings {
                base_url: String::new(), api_key: String::new(), model: String::new(), container_url: None,
            };
            let _ = bridge_line(&s, &mut acc);
        }

        #[test]
        fn progress_round_trips(domain in "[a-z][a-z ]{0,20}[a-z]", i in 1u32..50, extra in 0u32..50, status in "(ok|skipped|partial|base-failed|exception)", task in "[a-z0-9/_-]{1,30}") {
            let n = i + extra;
            let line = format!("  [{domain}] {i}/{n} {status:<12} {task}");
            let p = progress_line(&line).unwrap();
            prop_assert_eq!(p.domain, domain);
            prop_assert_eq!((p.index, p.total), (i, n));
            prop_assert_eq!(p.status, status);
            prop_assert_eq!(p.task, task);
        }
    }
}
