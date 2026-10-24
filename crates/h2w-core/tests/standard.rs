//! The results reader on a STANDARD-layout experiment synthesized by running upstream's own
//! `harness2`, `harness2-judge` and `harness2-report` code with a deterministic local
//! adapter (tests/fixtures/std; see tests/fixtures/README.md).

// Test crate: helpers outside #[test] functions may unwrap and panic.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use h2w_api::{Bench, ExperimentKind, FileKind, Mode, Substrate, WbSubset};
use h2w_core::namespace::Namespace;
use h2w_core::results::{DEFAULT_READ_CAP, ExperimentDir, compare};
use h2w_core::{CoreError, parse};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

fn exp() -> ExperimentDir {
    ExperimentDir::open(
        &fixtures().join("std"),
        ExperimentKind::Standard,
        &["/fixture/std".to_owned()],
    )
    .unwrap()
}

fn ns(s: &str) -> Namespace {
    Namespace::parse(s).unwrap()
}

fn cli(name: &str) -> String {
    std::fs::read_to_string(fixtures().join("cli/std").join(name)).unwrap()
}

#[test]
fn every_benchmark_sweep_is_listed() {
    let sweeps = exp().sweeps().unwrap();
    let mut ids: Vec<&str> = sweeps.iter().map(|s| s.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        vec![
            "jb-oc-agg1-s3",
            "lab-oc-agg1",
            "lab-oc-agg2",
            "wb-office-oc-agg1"
        ]
    );
    let lab = sweeps.iter().find(|s| s.id == "lab-oc-agg1").unwrap();
    let cell = lab.cell.unwrap();
    assert_eq!(
        (cell.bench, cell.subset, cell.substrate),
        (Bench::Lab, None, Substrate::Oc)
    );
    assert_eq!(lab.modes, vec![Mode::Parallel, Mode::Sequential]);
    assert_eq!((lab.tasks, lab.ok, lab.partial, lab.failed), (3, 2, 0, 1));
    assert!(lab.usd > 0.1);
    let headline = lab.headline.as_ref().unwrap();
    assert_eq!(headline.len(), 2);
    assert_eq!((headline[0].mode, headline[0].graded), (Mode::Parallel, 2));
    assert_eq!(
        (headline[1].mode, headline[1].graded),
        (Mode::Sequential, 1)
    );
    assert_eq!(headline[0].score, Some(100.0));
    // A sequential-only sweep keeps its summaries in the parallel namespace.
    let wb = sweeps.iter().find(|s| s.id == "wb-office-oc-agg1").unwrap();
    assert_eq!(wb.modes, vec![Mode::Sequential]);
    assert_eq!(wb.cell.unwrap().subset, Some(WbSubset::Office));
    let jb = sweeps.iter().find(|s| s.id == "jb-oc-agg1-s3").unwrap();
    assert_eq!((jb.seed, jb.k), (Some(3), 1));
}

#[test]
fn reports_match_harness2_report_stdout() {
    let e = exp();
    for (sweep, captured) in [
        ("lab-oc-agg1", "lab-report-k1.stdout"),
        ("jb-oc-agg1-s3", "jb-report-k1-s3.stdout"),
        ("wb-office-oc-agg1", "wb-office-report-k1.stdout"),
    ] {
        let r = e.report(&ns(sweep)).unwrap();
        assert_eq!(r.text, cli(captured), "{sweep}");
    }
    let lab = e.report(&ns("lab-oc-agg1")).unwrap();
    assert_eq!(lab.title, "lab-oc modes=parallel,sequential");
    assert_eq!(lab.notes.len(), 2);
    assert!(lab.notes[1].starts_with("WARNING  parallel k=1: 1 stage(s) fell back"));
    // Reference executions: msa-redline and nda-review base1 scored 1/4, the
    // base-failed task's base runs are ungraded.
    assert_eq!(lab.base_score, Some(25.0));
    let jb = e.report(&ns("jb-oc-agg1-s3")).unwrap();
    assert_eq!(
        jb.summaries[0].metrics,
        vec!["unweighted-fallback", "weighted"]
    );
}

#[test]
fn scores_keep_fractional_points_exact() {
    let e = exp();
    let lab = e.scores(&ns("lab-oc-agg1")).unwrap();
    assert_eq!(lab.rows.len(), 4);
    assert_eq!(lab.rows.iter().filter(|r| r.graded).count(), 3);
    let failed = lab.rows.iter().find(|r| !r.graded).unwrap();
    assert_eq!(
        (failed.task_id.as_str(), failed.mode),
        ("contracts/nda-review", Mode::Sequential)
    );
    assert!(
        failed
            .error
            .starts_with("RuntimeError: judge transport error")
    );
    assert_eq!(failed.frac, None);
    let ok = &lab.rows[0];
    assert_eq!(
        (ok.n_passed, ok.n_criteria, ok.frac),
        (Some(4), Some(4), Some(1.0))
    );
    assert_eq!(lab.base_rows.len(), 3);
    assert_eq!(lab.base_rows[2].n_criteria, None);
    let jb = e.scores(&ns("jb-oc-agg1-s3")).unwrap();
    let weighted = jb
        .rows
        .iter()
        .find(|r| r.task_id == "accountant/task1")
        .unwrap();
    // 7.5 / 10.0 points: not a u32, so the counts stay empty and the fraction is exact.
    assert_eq!((weighted.n_passed, weighted.n_criteria), (None, Some(10)));
    assert_eq!(weighted.frac, Some(0.75));
    assert_eq!(weighted.metric.as_deref(), Some("weighted"));
    let fallback = jb
        .rows
        .iter()
        .find(|r| r.task_id == "auditor/task1")
        .unwrap();
    assert_eq!((fallback.n_passed, fallback.frac), (Some(3), Some(0.6)));
}

#[test]
fn tasks_follow_the_sweep_record() {
    let e = exp();
    let rows = e.tasks(&ns("lab-oc-agg1")).unwrap();
    let ids: Vec<&str> = rows.iter().map(|r| r.task_id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "contracts/msa-redline",
            "contracts/nda-review",
            "litigation/motion-draft"
        ]
    );
    assert_eq!(rows[0].slug, "contracts__msa-redline");
    assert_eq!(rows[0].domain, "contracts");
    let par = &rows[0].modes[0];
    assert_eq!(
        (par.mode, par.fell_back, par.score),
        (Mode::Parallel, 1, Some(1.0))
    );
    assert_eq!(par.all_pass, Some(true));
    assert!(par.usd > 0.0);
    assert_eq!(rows[2].status, "base-failed");
    assert!(rows[2].modes.is_empty());
    assert_eq!(rows[2].errors, 1);
    // A seeded stream: the record's domain order, then each domain's task order.
    let jb: Vec<String> = e
        .tasks(&ns("jb-oc-agg1-s3"))
        .unwrap()
        .into_iter()
        .map(|r| r.task_id)
        .collect();
    assert_eq!(
        jb,
        vec!["auditor/task1", "accountant/task2", "accountant/task1"]
    );
    let total: f64 = rows
        .iter()
        .flat_map(|r| r.modes.iter())
        .map(|m| m.usd)
        .sum();
    let spend = e.spend_usd(&ns("lab-oc-agg1")).unwrap();
    assert!((total - spend).abs() < 1e-9, "{total} vs {spend}");
}

#[test]
fn fallen_back_stage_and_base_failed_task() {
    let e = exp();
    let t = e
        .task(&ns("lab-oc-agg1"), "contracts__msa-redline")
        .unwrap();
    let dom1 = t.modes[0]
        .stages
        .iter()
        .find(|s| s.label == "dom1")
        .unwrap();
    assert!(dom1.fell_back);
    assert_eq!(dom1.attempts, 2);
    assert!(!dom1.error.is_empty());
    assert!(
        t.task_prompt
            .as_deref()
            .is_some_and(|p| p.starts_with("# TASK contracts/msa-redline"))
    );
    let failed = e
        .task(&ns("lab-oc-agg1"), "litigation__motion-draft")
        .unwrap();
    assert_eq!(failed.status, "base-failed");
    assert_eq!(failed.errors, vec!["no base rollout produced anything"]);
    assert!(failed.base.iter().all(|b| !b.available));
    assert!(matches!(
        e.rollout_files(
            &ns("lab-oc-agg1"),
            "litigation__motion-draft",
            Mode::Parallel,
            "base1",
            ""
        ),
        Err(CoreError::NotFound(_))
    ));
}

#[test]
fn rollout_conventions_per_benchmark() {
    let e = exp();
    let read = |sweep: &str, slug: &str, mode: Mode, label: &str, sub: &str| {
        e.rollout_file(&ns(sweep), slug, mode, label, sub, DEFAULT_READ_CAP)
            .unwrap()
            .text
            .unwrap()
    };
    // LAB: lab/results/oc/<run id>, task ids with a '/'.
    assert_eq!(
        read(
            "lab-oc-agg1",
            "contracts__nda-review",
            Mode::Parallel,
            "inv1",
            "answer.txt"
        ),
        "Total: 85.15\n"
    );
    assert_eq!(
        read(
            "lab-oc-agg1",
            "contracts__nda-review",
            Mode::Sequential,
            "dom1",
            "answer.txt"
        ),
        "Total: 85.15\n"
    );
    assert_eq!(
        read(
            "lab-oc-agg1",
            "contracts__nda-review",
            Mode::Sequential,
            "base1",
            "answer.txt"
        ),
        "Total: 105.25\n"
    );
    // JobBench: jb/dataset/<task id>/model_output/<run id with / as __>.
    assert_eq!(
        read(
            "jb-oc-agg1-s3",
            "accountant__task1",
            Mode::Parallel,
            "dom1",
            "answer.txt"
        ),
        "Total: 7.50\n"
    );
    // WorkBuddy: the label directory of timestamped runs, and recorded trial dirs.
    let s = ns("wb-office-oc-agg1");
    let listing = e
        .rollout_files(&s, "deck-outline", Mode::Sequential, "inv1", "")
        .unwrap();
    assert_eq!(listing.len(), 1);
    assert_eq!(listing[0].kind, FileKind::Dir);
    let trial = format!("{}/deck-outline__a1b2c3", listing[0].name);
    assert_eq!(
        read(
            "wb-office-oc-agg1",
            "deck-outline",
            Mode::Sequential,
            "inv1",
            &format!("{trial}/answer.txt")
        ),
        "Total: 4.00\n"
    );
    assert_eq!(
        read(
            "wb-office-oc-agg1",
            "deck-outline",
            Mode::Sequential,
            "base1",
            "answer.txt"
        ),
        "Total: 4.00\n"
    );
}

#[test]
fn audit_trail_sessions_and_text() {
    let e = exp();
    let s = ns("lab-oc-agg1");
    let sessions = e.audit(&s).unwrap();
    assert_eq!(sessions.len(), 17);
    let retry = sessions.iter().find(|x| x.label == "dom1.a2").unwrap();
    assert!(retry.ok);
    assert_eq!(
        (retry.channel.as_str(), retry.model.as_str()),
        ("opencode", "local/improver")
    );
    assert_eq!(retry.parts, vec!["mission", "response"]);
    assert!((retry.cost_usd - 0.0125).abs() < 1e-12);
    assert!(sessions.iter().any(|x| x.label == "experience.seq1"));
    let text = e
        .audit_text(&s, &retry.id, "response", DEFAULT_READ_CAP)
        .unwrap();
    assert_eq!(text, "<notes>Dropped the helper script.</notes>");
    let cut = e.audit_text(&s, &retry.id, "mission", 20).unwrap();
    assert!(cut.contains("\n[truncated: showing the first 20 of "));
    assert!(matches!(
        e.audit_text(&s, &retry.id, "secrets", 10),
        Err(CoreError::Invalid { .. })
    ));
    assert!(matches!(
        e.audit_text(&s, "../x", "mission", 10),
        Err(CoreError::Invalid { .. })
    ));
    assert!(matches!(
        e.audit_text(&s, &retry.id, "system", 10),
        Err(CoreError::NotFound(_))
    ));
    assert_eq!(e.audit(&ns("lab-oc-agg2")).unwrap().len(), 16);
}

#[test]
fn library_identities_digest_and_size() {
    let e = exp();
    let lib = e.library(&ns("lab-oc-agg1")).unwrap();
    let doms: Vec<(Mode, &str, usize)> = lib
        .iter()
        .map(|d| (d.mode, d.domain.as_str(), d.entries.len()))
        .collect();
    assert_eq!(
        doms,
        vec![
            (Mode::Parallel, "contracts", 2),
            (Mode::Sequential, "contracts", 2)
        ]
    );
    assert_eq!(lib[0].entries[0].task_slug, "contracts__msa-redline");
    let jb_lib = e.library(&ns("jb-oc-agg1-s3")).unwrap();
    assert_eq!(jb_lib.len(), 2);
    let ids = e.sweep_identities().unwrap();
    let names: Vec<&str> = ids.iter().map(|i| i.sweep.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "jb-oc-agg1-s3",
            "lab-oc-agg1",
            "lab-oc-agg2",
            "wb-office-oc-agg1"
        ]
    );
    let id: serde_json::Value = serde_json::from_str(&ids[1].identity_json).unwrap();
    assert_eq!(id["executor"], "local/solver");
    assert_eq!(id["tasks"].as_array().unwrap().len(), 3);
    let d: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fixtures().join("golden/digests.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        e.results_digest().unwrap(),
        d["std"]["results_digest"].as_str().unwrap()
    );
    assert_eq!(
        e.size_bytes().unwrap(),
        d["std"]["size_bytes"].as_u64().unwrap()
    );
}

#[test]
fn compare_two_modes_of_one_sweep() {
    let e = exp();
    let s = e.scores(&ns("lab-oc-agg1")).unwrap();
    let domains: BTreeMap<String, String> = e
        .tasks(&ns("lab-oc-agg1"))
        .unwrap()
        .into_iter()
        .map(|r| (r.task_id, r.domain))
        .collect();
    let c = compare(
        "std/lab-oc-agg1/parallel",
        &s,
        Mode::Parallel,
        "std/lab-oc-agg1/sequential",
        &s,
        Mode::Sequential,
        &domains,
    );
    assert_eq!(c.tasks.len(), 2);
    assert_eq!(c.n_common, 1);
    assert_eq!((c.wins, c.losses, c.ties), (0, 0, 1));
    assert_eq!(c.sign_test_p, None);
    assert_eq!(c.tasks[1].b, None);
    assert_eq!(c.tasks[0].domain, "contracts");
}

#[test]
fn captured_cli_output_parses() {
    let p = parse::preview_output(&cli("lab-preview-k1.stdout")).unwrap();
    assert_eq!(
        p.header,
        "# lab-oc split=all seed=None k=1 modes=parallel,sequential"
    );
    assert_eq!(p.summary_namespace, "lab-oc-agg1");
    assert_eq!(
        p.library_root.as_deref(),
        Some("/fixture/std/lab/library/lab/k1-oc")
    );
    assert_eq!(p.namespaces.len(), 2);
    assert_eq!(
        (p.namespaces[1].mode, p.namespaces[1].namespace.as_str()),
        (Mode::Sequential, "lab-oc-seq1")
    );
    assert_eq!((p.workers, p.total_tasks, p.total_domains), (1, 3, 2));
    let stream: Vec<(u32, &str, &str)> = p
        .stream
        .iter()
        .map(|s| (s.index, s.domain.as_str(), s.task.as_str()))
        .collect();
    assert_eq!(
        stream,
        vec![
            (1, "contracts", "contracts/msa-redline"),
            (2, "contracts", "contracts/nda-review"),
            (3, "litigation", "litigation/motion-draft")
        ]
    );
    // The stream equals what harness2 recorded as the sweep identity's task list.
    let identity: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fixtures().join("std/lab/adapt/lab-oc-agg1/config.json")).unwrap(),
    )
    .unwrap();
    let recorded: Vec<(String, String)> = identity["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            (
                t[0].as_str().unwrap().to_owned(),
                t[1].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let parsed: Vec<(String, String)> = p
        .stream
        .iter()
        .map(|s| (s.domain.clone(), s.task.clone()))
        .collect();
    assert_eq!(parsed, recorded);
    let jb = parse::preview_output(&cli("jb-preview-k1-s3.stdout")).unwrap();
    assert_eq!(jb.summary_namespace, "jb-oc-agg1-s3");
    assert_eq!(jb.stream[0].task, "auditor/task1");
    assert_eq!(jb.domains[1].domain, "accountant");

    let run = parse::run_output(&cli("lab-run-k1.stdout")).unwrap();
    assert_eq!(
        (run.namespace.as_str(), run.done, run.failed),
        ("lab-oc-agg1", 2, 1)
    );
    assert!(run.dead_domains.is_empty());
    let record = run.record.unwrap();
    assert!(
        fixtures()
            .join("std/lab/adapt/_sweeps")
            .join(&record)
            .is_file(),
        "{record}"
    );
    assert!(
        run.next
            .unwrap()
            .starts_with("python3 -m harness2.cli.judge --bench lab")
    );
    let progress: Vec<_> = cli("lab-run-k1.stdout")
        .lines()
        .filter_map(parse::progress_line)
        .collect();
    assert_eq!(progress.len(), 3);
    assert_eq!(progress[2].status, "base-failed");
    assert_eq!(
        progress[2].error.as_deref(),
        Some("no base rollout produced anything")
    );

    let j = parse::judge_output(&cli("lab-judge-k1.stdout"), false).unwrap();
    assert_eq!((j.rows, j.graded, j.ungraded), (4, 3, 1));
    let b = parse::judge_output(&cli("lab-judge-bases-k1.stdout"), true).unwrap();
    assert_eq!((b.rows, b.graded, b.ungraded), (3, 2, 1));
    assert_eq!(
        parse::split_file(&cli("splits/lab-half.json")).unwrap(),
        (1, 2)
    );
}
