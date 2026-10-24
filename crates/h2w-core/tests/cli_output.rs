//! Parsers on REAL offline harness2 CLI output (`harness2-check` on an uninstalled
//! checkout, `harness2-setup --dry-run`), captured into tests/fixtures/cli/.

// Test crate: helpers outside #[test] functions may unwrap and panic.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use h2w_core::parse;

fn read(name: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/cli")
        .join(name);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

#[test]
fn check_reports_every_failure() {
    for (name, failures) in [
        ("check-lab", 9),
        ("check-jb", 9),
        ("check-wb-office", 11),
        ("check-lab-cx", 9),
    ] {
        let out = read(&format!("{name}.stdout"));
        let r = parse::check_output(&out);
        assert!(!r.ready, "{name}");
        assert_eq!(r.tasks, None, "{name}");
        assert_eq!(r.failures.len(), failures, "{name}");
        assert!(r.failures.iter().all(|f| !f.starts_with("FAIL")), "{name}");
        assert_eq!(read(&format!("{name}.exit")).trim(), "1");
    }
    let lab = parse::check_output(&read("check-lab.stdout"));
    assert!(
        lab.failures
            .iter()
            .any(|f| f.starts_with("pandoc missing; install pandoc"))
    );
    let ready = parse::check_output(
        "Ready: wb-office/oc; 120 tasks. Model access is not exercised; the first real run is the first model call.\n",
    );
    assert!(ready.ready);
    assert_eq!(ready.tasks, Some(120));
}

#[test]
fn setup_dry_run_plans() {
    for name in [
        "setup-dry-run-lab",
        "setup-dry-run-jb",
        "setup-dry-run-wb",
        "setup-dry-run-wb-web",
        "setup-dry-run-lab-cx",
    ] {
        let out = read(&format!("{name}.stdout"));
        let r = parse::setup_output(&out, true);
        assert!(r.dry_run);
        let lines: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).collect();
        let closing = usize::from(lines.last() == Some(&"Setup plan complete."));
        assert_eq!(
            r.plan.len() + r.notes.len() + closing,
            lines.len(),
            "{name}"
        );
        assert!(r.plan[0].starts_with("git init -q "), "{name}");
        assert!(
            r.plan.iter().all(|l| !l.starts_with("Setup plan")),
            "{name}"
        );
    }
    let wb = parse::setup_output(&read("setup-dry-run-wb.stdout"), true);
    assert_eq!(wb.notes.len(), 1);
    assert!(wb.notes[0].starts_with("Configure WorkBuddy solver and judge routes -- "));
    assert!(
        wb.plan
            .iter()
            .any(|l| l.ends_with("bash scripts/dataset/fetch-dataset.sh office web code"))
    );
    let web = parse::setup_output(&read("setup-dry-run-wb-web.stdout"), true);
    assert!(web.plan.iter().any(|l| l.ends_with("fetch-dataset.sh web")));
    let jb = parse::setup_output(&read("setup-dry-run-jb.stdout"), true);
    assert!(
        jb.plan.iter().any(|l| l
            == "Download JobBench/job-bench dataset/** from Hugging Face; record its revision")
    );
    // A failed setup reports on stderr; the plan printed so far is still parsed.
    assert_eq!(read("setup-dry-run-lab-cx.exit").trim(), "1");
    assert!(
        read("setup-dry-run-lab-cx.stderr").starts_with("setup failed: HARNESS2_CODEX_BASE_URL")
    );
    let cx = parse::setup_output(&read("setup-dry-run-lab-cx.stdout"), true);
    assert!(cx.plan.last().unwrap().contains("@openai/codex@0.144.6"));
}
