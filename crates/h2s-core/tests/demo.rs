//! The results reader on REAL `harness2-demo` output (tests/fixtures/demo-k2, demo-k1-parallel;
//! see tests/fixtures/README.md). Recorded absolute paths start with `/fixture/<name>`.

// Test crate: helpers outside #[test] functions may unwrap and panic.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};

use h2s_api::{ChangeKind, Component, ExperimentKind, FileKind, HarnessRole, Mode, Substrate};
use h2s_core::namespace::Namespace;
use h2s_core::results::{DEFAULT_READ_CAP, ExperimentDir};
use h2s_core::{CoreError, parse};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

fn open_at(path: &Path, name: &str) -> ExperimentDir {
    ExperimentDir::open(path, ExperimentKind::Demo, &[format!("/fixture/{name}")]).unwrap()
}

fn open(name: &str) -> ExperimentDir {
    open_at(&fixtures().join(name), name)
}

fn sweep() -> Namespace {
    Namespace::parse("demo-oc-agg2").unwrap()
}

fn digests() -> serde_json::Value {
    let text = std::fs::read_to_string(fixtures().join("golden/digests.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// Copy a fixture experiment into a temporary directory (for tests that alter it).
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let target = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_tree(&e.path(), &target);
        } else {
            std::fs::copy(e.path(), target).unwrap();
        }
    }
}

#[test]
fn one_sweep_with_both_modes() {
    let exp = open("demo-k2");
    let sweeps = exp.sweeps().unwrap();
    assert_eq!(sweeps.len(), 1);
    let s = &sweeps[0];
    assert_eq!(s.id, "demo-oc-agg2");
    assert_eq!(s.bench_name, "demo");
    assert_eq!(s.cell, None);
    assert_eq!(s.substrate, Substrate::Oc);
    assert_eq!((s.k, s.seed, s.tag.as_deref()), (2, None, None));
    assert_eq!(s.modes, vec![Mode::Parallel, Mode::Sequential]);
    assert_eq!((s.tasks, s.ok, s.partial, s.failed), (2, 2, 0, 0));
    assert_eq!(s.usd, 0.0);
    assert_eq!(s.headline, None);
    assert!(s.updated_at.as_deref().is_some_and(|t| t.ends_with('Z')));
}

#[test]
fn task_rows_follow_the_demo_stream() {
    let exp = open("demo-k2");
    let rows = exp.tasks(&sweep()).unwrap();
    let ids: Vec<&str> = rows.iter().map(|r| r.task_id.as_str()).collect();
    assert_eq!(ids, vec!["ledger-1", "ledger-2"]);
    let r = &rows[0];
    assert_eq!(
        (r.slug.as_str(), r.domain.as_str(), r.status.as_str()),
        ("ledger-1", "accounting", "ok")
    );
    assert_eq!(r.errors, 0);
    assert_eq!(r.modes.len(), 2);
    assert!(
        r.modes
            .iter()
            .all(|m| m.ok && !m.chosen_is_base && m.score.is_none())
    );
    assert_eq!(r.modes[0].chosen_label.as_deref(), Some("agg1"));
    assert_eq!(r.modes[1].chosen_label.as_deref(), Some("c1"));
}

#[test]
fn task_detail_from_summary() {
    let exp = open("demo-k2");
    let t = exp.task(&sweep(), "ledger-1").unwrap();
    assert_eq!(
        (t.task_id.as_str(), t.k, t.seed, t.status.as_str()),
        ("ledger-1", 2, None, "ok")
    );
    assert_eq!(t.base.len(), 2);
    assert!(t.base.iter().all(|b| b.available));
    assert_eq!(t.base[0].run_id, "demo-oc-agg2/ledger-1/base1");
    assert!(t.errors.is_empty());
    assert!(
        t.task_prompt
            .as_deref()
            .is_some_and(|p| p.starts_with("Sum every signed amount in ledger.csv"))
    );

    let agg = &t.modes[0];
    assert_eq!(agg.mode, Mode::Parallel);
    assert!(agg.ok);
    assert_eq!(agg.finals, vec!["agg1", "agg2"]);
    let labels: Vec<&str> = agg.stages.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(labels, vec!["inv1", "dom1", "inv2", "dom2", "agg1", "agg2"]);
    assert_eq!(agg.stages[0].dim, "INV");
    assert!(agg.stages[0].notes.starts_with("Keep signed amounts"));
    let ledger = agg.ledger.as_ref().unwrap();
    assert_eq!(
        (ledger.r, ledger.expected_r, ledger.p, ledger.expected_p),
        (8, 8, 6, 6)
    );
    let sim = agg.similarity.as_ref().unwrap();
    assert_eq!((sim.n, sim.n_distinct), (2, 1));
    let sel = agg.selection.as_ref().unwrap();
    assert!(sel.ok && !sel.trivial && !sel.fallback);
    assert_eq!(sel.chosen_label, "agg1");
    assert_eq!(sel.id_map.len(), 2);
    assert_eq!(
        (sel.id_map[0].id.as_str(), sel.id_map[0].label.as_str()),
        ("A", "agg1")
    );
    assert_eq!(sel.evidence, "deliverable");
    assert_eq!(sel.harness_fingerprint.as_deref(), Some("851f1352f3589138"));
    let exp_lesson = sel.experience.as_ref().unwrap();
    assert_eq!(exp_lesson.cue, "When a ledger includes credits and debits");
    let harnesses: Vec<(&str, HarnessRole, bool)> = agg
        .harnesses
        .iter()
        .map(|h| (h.label.as_str(), h.role, h.chosen))
        .collect();
    assert_eq!(
        harnesses,
        vec![
            ("base", HarnessRole::Base, false),
            ("inv1", HarnessRole::Candidate, false),
            ("dom1", HarnessRole::Candidate, false),
            ("inv2", HarnessRole::Candidate, false),
            ("dom2", HarnessRole::Candidate, false),
            ("agg1", HarnessRole::Final, true),
            ("agg2", HarnessRole::Final, false),
        ]
    );
    let seq = &t.modes[1];
    assert_eq!(seq.mode, Mode::Sequential);
    assert_eq!(seq.similarity, None);
    let order: Vec<&str> = seq.harnesses.iter().map(|h| h.label.as_str()).collect();
    assert_eq!(
        order,
        vec!["base", "inv1", "dom1", "c1", "inv2", "dom2", "c2"]
    );
    let kept: Vec<bool> = seq.stages.iter().map(|s| s.kept_base).collect();
    assert_eq!(kept, vec![false, false, false, true, true, true]);
}

#[test]
fn harness_fingerprints_match_what_harness2_recorded() {
    let exp = open("demo-k2");
    for (slug, mode) in [
        ("ledger-1", Mode::Parallel),
        ("ledger-1", Mode::Sequential),
        ("ledger-2", Mode::Parallel),
        ("ledger-2", Mode::Sequential),
    ] {
        let t = exp.task(&sweep(), slug).unwrap();
        let sel = t
            .modes
            .iter()
            .find(|m| m.mode == mode)
            .and_then(|m| m.selection.clone())
            .unwrap();
        let h = exp
            .harness(&sweep(), slug, mode, &sel.chosen_label)
            .unwrap();
        assert_eq!(
            Some(h.fingerprint.clone()),
            sel.harness_fingerprint,
            "{slug} {mode:?}"
        );
        assert_eq!(h.role, HarnessRole::Final);
        // Upstream's library inventory says "1 scripts; 62 always-on chars" for this tree.
        assert_eq!(h.census.always_on_chars, 62);
        assert_eq!(h.census.scripts, 1);
        let paths: Vec<&str> = h.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "systemprompt.md",
                "guardrails.md",
                "LongTermMEMORY.md",
                "tool_descriptions/tool_guidance.md",
                "scripts/summarize.py"
            ]
        );
        assert!(h.files.iter().all(|f| f.content.is_some()));
        assert_eq!(h.files[4].component, Component::Scripts);
    }
}

#[test]
fn diff_shows_the_editor_change() {
    let exp = open("demo-k2");
    let d = exp
        .diff(&sweep(), "ledger-1", Mode::Parallel, "base", "agg1")
        .unwrap();
    assert_eq!((d.from.as_str(), d.to.as_str()), ("base", "agg1"));
    assert_eq!(d.files[0].path, "scripts/summarize.py");
    assert_eq!(d.files[0].change, ChangeKind::Modified);
    assert_eq!((d.files[0].lines_added, d.files[0].lines_removed), (1, 1));
    assert!(
        d.files[0]
            .unified
            .contains("-total = sum((amount for amount in amounts if amount > 0), Decimal(0))")
    );
    assert!(
        d.files[1..]
            .iter()
            .all(|f| f.change == ChangeKind::Unchanged && f.unified.is_empty())
    );
    assert_eq!(d.always_on_char_delta, 0);
    assert!(matches!(
        exp.diff(&sweep(), "ledger-1", Mode::Parallel, "base", "agg9"),
        Err(CoreError::NotFound(_))
    ));
}

#[test]
fn rollouts_for_recorded_and_derived_labels() {
    let exp = open("demo-k2");
    let s = sweep();
    let files = exp
        .rollout_files(&s, "ledger-1", Mode::Parallel, "base1", "")
        .unwrap();
    let names: Vec<(&str, FileKind)> = files.iter().map(|f| (f.name.as_str(), f.kind)).collect();
    assert_eq!(
        names,
        vec![
            ("answer.txt", FileKind::File),
            ("trajectory.md", FileKind::File)
        ]
    );
    let f = exp
        .rollout_file(
            &s,
            "ledger-1",
            Mode::Parallel,
            "base1",
            "answer.txt",
            DEFAULT_READ_CAP,
        )
        .unwrap();
    assert_eq!(f.text.as_deref(), Some("Total: 105.25\n"));
    assert_eq!(
        (f.bytes, f.truncated, f.mime.as_str()),
        (14, false, "text/plain")
    );
    // The chosen run (recorded), a candidate and a final (derived from the run id).
    for (mode, label, total) in [
        (Mode::Parallel, "agg1", "85.15"),
        (Mode::Parallel, "inv2", "85.15"),
        (Mode::Sequential, "c2", "85.15"),
        (Mode::Sequential, "base2", "105.25"),
    ] {
        let f = exp
            .rollout_file(&s, "ledger-1", mode, label, "answer.txt", DEFAULT_READ_CAP)
            .unwrap();
        assert_eq!(f.text.unwrap(), format!("Total: {total}\n"), "{label}");
    }
    let cut = exp
        .rollout_file(&s, "ledger-1", Mode::Parallel, "base1", "trajectory.md", 10)
        .unwrap();
    assert!(cut.truncated);
    assert_eq!(cut.text.as_deref(), Some("[ACTION] R"));
    for bad in ["../summary.json", "/etc/passwd", "a//b"] {
        assert!(matches!(
            exp.rollout_file(&s, "ledger-1", Mode::Parallel, "base1", bad, 100),
            Err(CoreError::Invalid { .. })
        ));
    }
    assert!(matches!(
        exp.rollout_files(&s, "../ledger-1", Mode::Parallel, "base1", ""),
        Err(CoreError::Invalid { .. })
    ));
    assert!(matches!(
        exp.rollout_files(&s, "ledger-1", Mode::Parallel, "../base1", ""),
        Err(CoreError::Invalid { .. })
    ));
    assert!(matches!(
        exp.rollout_files(&s, "ledger-9", Mode::Parallel, "base1", ""),
        Err(CoreError::NotFound(_))
    ));
}

#[test]
fn libraries_scores_audit_and_identity() {
    let exp = open("demo-k2");
    let s = sweep();
    let lib = exp.library(&s).unwrap();
    assert_eq!(lib.len(), 2);
    assert_eq!(
        (lib[0].mode, lib[0].domain.as_str()),
        (Mode::Parallel, "accounting")
    );
    assert_eq!(lib[1].mode, Mode::Sequential);
    let e = &lib[0].entries[0];
    assert_eq!(
        (e.task_slug.as_str(), e.adopted_label.as_str()),
        ("ledger-1", "agg1")
    );
    assert_eq!(e.run_id, "demo-oc-agg2/ledger-1/agg1");
    assert_eq!(e.components, "1 scripts; 62 always-on chars");
    assert_eq!(e.why, "The signed total matches the source ledger.");
    let x = e.experience.as_ref().unwrap();
    assert!(x.edits.starts_with("component: scripts/summarize.py"));
    assert_eq!(x.confidence, "high for this observed arithmetic contrast");

    let scores = exp.scores(&s).unwrap();
    assert!(scores.rows.is_empty() && scores.base_rows.is_empty());
    let report = exp.report(&s).unwrap();
    assert!(report.summaries.is_empty());
    assert_eq!(report.text, "nothing to report\n");
    assert_eq!(report.title, "demo-oc modes=");
    assert!(exp.audit(&s).unwrap().is_empty());
    assert_eq!(exp.spend_usd(&s).unwrap(), 0.0);
    assert!(exp.sweep_identities().unwrap().is_empty());
    let d = digests();
    assert_eq!(
        exp.results_digest().unwrap(),
        d["demo-k2"]["results_digest"].as_str().unwrap()
    );
    assert_eq!(
        exp.size_bytes().unwrap(),
        d["demo-k2"]["size_bytes"].as_u64().unwrap()
    );
}

#[test]
fn k1_parallel_demo_adopts_without_a_selector() {
    let exp = open("demo-k1-parallel");
    let s = Namespace::parse("demo-oc-agg1").unwrap();
    let sweeps = exp.sweeps().unwrap();
    assert_eq!(sweeps.len(), 1);
    assert_eq!(sweeps[0].modes, vec![Mode::Parallel]);
    let t = exp.task(&s, "ledger-1").unwrap();
    assert_eq!(t.modes.len(), 1);
    let sel = t.modes[0].selection.as_ref().unwrap();
    assert!(sel.trivial);
    assert_eq!(sel.chosen_label, "agg1");
    let h = exp.harness(&s, "ledger-1", Mode::Parallel, "agg1").unwrap();
    assert_eq!(Some(h.fingerprint), sel.harness_fingerprint.clone());
    let d = digests();
    assert_eq!(
        exp.results_digest().unwrap(),
        d["demo-k1-parallel"]["results_digest"].as_str().unwrap()
    );
}

#[test]
fn demo_json_and_stdout_parse() {
    let fx = fixtures();
    let text = std::fs::read_to_string(fx.join("demo-k2/demo.json")).unwrap();
    let d = parse::demo_json(&text).unwrap();
    assert_eq!(
        (d.done, d.failed, d.execution_count, d.editor_calls),
        (2, 0, 28, 32)
    );
    let out = std::fs::read_to_string(fx.join("cli/demo-k2.stdout")).unwrap();
    let progress: Vec<_> = out.lines().filter_map(parse::progress_line).collect();
    assert_eq!(progress.len(), 2);
    assert_eq!(
        (
            progress[1].domain.as_str(),
            progress[1].index,
            progress[1].total,
            progress[1].status.as_str(),
            progress[1].task.as_str()
        ),
        ("accounting", 2, 2, "ok", "ledger-2")
    );
}

#[test]
fn symlinks_and_foreign_paths_are_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("demo-k2");
    copy_tree(&fixtures().join("demo-k2"), &root);
    let run = root.join("executions/demo-oc-agg2/ledger-1/base1");
    std::os::unix::fs::symlink("/etc", run.join("etc-link")).unwrap();
    std::os::unix::fs::symlink("answer.txt", run.join("answer-link")).unwrap();
    let exp = open_at(&root, "demo-k2");
    let s = sweep();
    let files = exp
        .rollout_files(&s, "ledger-1", Mode::Parallel, "base1", "")
        .unwrap();
    let links: Vec<&str> = files
        .iter()
        .filter(|f| f.kind == FileKind::Symlink)
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(links, vec!["answer-link", "etc-link"]);
    for sub in ["etc-link", "answer-link", "etc-link/passwd"] {
        assert!(
            matches!(
                exp.rollout_file(&s, "ledger-1", Mode::Parallel, "base1", sub, 100),
                Err(CoreError::Refused(_))
            ),
            "{sub}"
        );
    }
    assert!(matches!(
        exp.rollout_files(&s, "ledger-1", Mode::Parallel, "base1", "etc-link"),
        Err(CoreError::Refused(_))
    ));
    // A recorded run_dir outside every known root is refused, not followed.
    let summary = root.join("runs/demo-oc-agg2/ledger-2/summary.json");
    let text = std::fs::read_to_string(&summary).unwrap();
    std::fs::write(
        &summary,
        text.replace("/fixture/demo-k2/executions", "/etc/executions"),
    )
    .unwrap();
    assert!(matches!(
        exp.rollout_files(&s, "ledger-2", Mode::Parallel, "base1", ""),
        Err(CoreError::Refused(_))
    ));
    let t = exp.task(&s, "ledger-2").unwrap();
    assert!(t.base.iter().all(|b| !b.available));
    // A path that climbs out after the known prefix is refused too.
    std::fs::write(
        &summary,
        text.replace("/fixture/demo-k2/executions", "/fixture/demo-k2/../x"),
    )
    .unwrap();
    assert!(matches!(
        exp.rollout_files(&s, "ledger-2", Mode::Parallel, "base1", ""),
        Err(CoreError::Refused(_))
    ));
}

#[test]
fn chosen_is_base_reports_an_unchanged_adopted_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("demo-k2");
    copy_tree(&fixtures().join("demo-k2"), &root);
    let summary = root.join("runs/demo-oc-agg2/ledger-1/summary.json");
    let text = std::fs::read_to_string(&summary).unwrap();
    assert_eq!(text.matches("\"chosen_tree_is_base\": false").count(), 2);
    std::fs::write(
        &summary,
        text.replacen(
            "\"chosen_tree_is_base\": false",
            "\"chosen_tree_is_base\": true",
            1,
        ),
    )
    .unwrap();
    let rows = open_at(&root, "demo-k2").tasks(&sweep()).unwrap();
    let flags: Vec<bool> = rows[0].modes.iter().map(|m| m.chosen_is_base).collect();
    assert_eq!(flags, vec![true, false]);
}
