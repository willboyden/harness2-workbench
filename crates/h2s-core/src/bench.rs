//! Benchmark catalogue and installation state.

use std::path::Path;

use h2s_api::{Bench, BenchmarkStatus, WbSubset};

use crate::safefs;

/// Upstream dependency pins from `harness2/config.py` at [`crate::TESTED_HARNESS2_COMMIT`].
pub const DEPENDENCY_PINS: &[(&str, &str)] = &[
    ("job-bench-eval", "fa03a17f9ff662f136798a274e10011c3aa00fd0"),
    (
        "workbuddy-bench",
        "b516950be5b56eb3be406c2f76ee1c5111dcb57f",
    ),
    ("harvey-labs", "b58a28ae9fde5ee4b810e53b4fd0a1915325a35d"),
    ("opencode", "23fb5e0516c99ac04a1aa46c193efda2e1b9bb24"),
];

/// Codex CLI version harness2 installs (`config.CODEX_VERSION`).
pub const CODEX_VERSION: &str = "0.144.6";

/// Hosts setup downloads from (upstream `docs/SETUP.md` "Network access during setup").
pub const SETUP_HOSTS: &[&str] = &[
    "github.com",
    "codeload.github.com",
    "registry.npmjs.org",
    "huggingface.co",
    "cdn-lfs.huggingface.co",
    "astral.sh",
    "pypi.org",
    "files.pythonhosted.org",
];

/// Hosts upstream recommends blocking for executors during evaluation, so agents cannot
/// fetch public rubrics (`docs/EXPERIMENTS.md` "Web access").
pub const EVAL_BLOCKLIST: &[&str] = &[
    "huggingface.co",
    "hf.co",
    "*.hf.co",
    "github.com",
    "raw.githubusercontent.com",
];

const SETUP_TOOLS: [&str; 3] = ["Node.js 20+ and npm", "uv", "git"];

fn entry(
    bench: Bench,
    subset: Option<WbSubset>,
    display_name: &str,
    extra_requirements: &[&str],
    license_note: &str,
    description: &str,
) -> BenchmarkStatus {
    let upstream_url = match bench {
        Bench::Jb => "https://github.com/Job-Bench/job-bench-eval",
        Bench::Lab => "https://github.com/harveyai/harvey-labs",
        Bench::Wb => "https://github.com/Tencent/workbuddy-bench",
    };
    BenchmarkStatus {
        bench,
        subset,
        display_name: display_name.to_owned(),
        installed: false,
        at_pinned_commit: None,
        data_present: false,
        domains: Vec::new(),
        requirements: extra_requirements
            .iter()
            .chain(SETUP_TOOLS.iter())
            .map(|s| (*s).to_owned())
            .collect(),
        upstream_url: upstream_url.to_owned(),
        license_note: license_note.to_owned(),
        description: description.to_owned(),
    }
}

const WB_LICENSE: &str = "Custom Tencent license with a territorial limitation (WorkBuddy Bench \
     is not intended for use within the European Union); harness2's derived runner files and \
     patches keep that license. Read it before downloading or using the benchmark.";

/// Static catalogue: one entry per benchmark (WorkBuddy: one per subset), not installed.
#[must_use]
pub fn catalog() -> Vec<BenchmarkStatus> {
    let mut out = vec![
        entry(
            Bench::Jb,
            None,
            "JobBench",
            &[],
            "job-bench-eval is Apache-2.0. The task data is the public JobBench/job-bench \
             dataset on Hugging Face (see its dataset card); its rubrics are public too, so \
             block executor web access during runs.",
            "Professional work tasks graded against weighted rubrics by an LLM judge; the \
             experience library groups tasks by occupation. Setup downloads the dataset from \
             Hugging Face and records its revision.",
        ),
        entry(
            Bench::Lab,
            None,
            "LAB",
            &["Pandoc", "Poppler (pdftoppm)"],
            "harvey-labs is MIT-licensed.",
            "Legal work tasks with source documents and rubric grading: reports partial credit \
             (fraction of criteria passed) and all-pass. The library groups tasks by practice \
             area; criteria never reach the improver.",
        ),
    ];
    let subsets = [
        (
            WbSubset::Office,
            "WorkBuddy-Bench (office)",
            "Office tasks (documents, spreadsheets, slides), each run in a fresh container and \
             scored by the benchmark's verifier; the library groups tasks by category.",
        ),
        (
            WbSubset::Web,
            "WorkBuddy-Bench (web)",
            "Web tasks, each run in a fresh container and scored by the benchmark's verifier \
             with a Gemini judge; the library groups tasks by category.",
        ),
        (
            WbSubset::Code,
            "WorkBuddy-Bench (code)",
            "Coding tasks, each run in a fresh container and scored by the benchmark's \
             verifier; the library groups tasks by role.",
        ),
    ];
    for (subset, name, description) in subsets {
        out.push(entry(
            Bench::Wb,
            Some(subset),
            name,
            &["Docker", "curl", "Vertex AI Gemini executor and judge"],
            WB_LICENSE,
            description,
        ));
    }
    out
}

fn pin_of(checkout: &str) -> Option<&'static str> {
    DEPENDENCY_PINS
        .iter()
        .find(|(n, _)| *n == checkout)
        .map(|(_, p)| *p)
}

/// The catalogue with installation state read from `harness2_home/dependencies/` (files
/// only: checkout HEAD vs pin, dataset markers).
///
/// Installed = the checkout has a `.git` directory. Data: JobBench's download record
/// (`dataset/main/.harness2-download.json`), LAB's `tasks/` directory, a WorkBuddy subset's
/// `datasets/wb-bench-<subset>-v1.0/tasks/` directory.
#[must_use]
pub fn status(harness2_home: &Path) -> Vec<BenchmarkStatus> {
    let mut out = catalog();
    let Ok(home) = cap_std::fs::Dir::open_ambient_dir(harness2_home, cap_std::ambient_authority())
    else {
        return out;
    };
    for b in &mut out {
        let checkout = b.bench.upstream_checkout();
        let repo = format!("dependencies/{checkout}");
        b.installed = safefs::is_dir(&home, &format!("{repo}/.git"));
        if b.installed {
            let head = safefs::open_dir(&home, &repo, "checkout")
                .ok()
                .and_then(|d| crate::install::read_head(&d));
            b.at_pinned_commit = Some(head.is_some() && head.as_deref() == pin_of(checkout));
        }
        b.data_present = match (b.bench, b.subset) {
            (Bench::Jb, _) => safefs::mtime(
                &home,
                &format!("{repo}/dataset/main/.harness2-download.json"),
                "dataset record",
            )
            .ok()
            .flatten()
            .is_some(),
            (Bench::Lab, _) => safefs::is_dir(&home, &format!("{repo}/tasks")),
            (Bench::Wb, Some(s)) => safefs::is_dir(
                &home,
                &format!("{repo}/datasets/wb-bench-{}-v1.0/tasks", s.as_cli()),
            ),
            (Bench::Wb, None) => false,
        };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_covers_every_cell() {
        let c = catalog();
        assert_eq!(c.len(), 5);
        assert!(
            c.iter()
                .all(|b| !b.installed && b.at_pinned_commit.is_none())
        );
        let lab = c.iter().find(|b| b.bench == Bench::Lab).unwrap();
        assert!(lab.requirements.iter().any(|r| r.contains("Pandoc")));
        let wb: Vec<_> = c.iter().filter(|b| b.bench == Bench::Wb).collect();
        assert_eq!(wb.len(), 3);
        assert!(wb.iter().all(|b| b.license_note.contains("territorial")));
    }

    #[test]
    fn status_reads_checkouts() {
        let tmp = tempfile::tempdir().unwrap();
        let deps = tmp.path().join("dependencies");
        let lab = deps.join("harvey-labs");
        std::fs::create_dir_all(lab.join(".git")).unwrap();
        std::fs::create_dir_all(lab.join("tasks/contracts/t1")).unwrap();
        std::fs::write(
            lab.join(".git/HEAD"),
            format!("{}\n", pin_of("harvey-labs").unwrap()),
        )
        .unwrap();
        let jb = deps.join("job-bench-eval");
        std::fs::create_dir_all(jb.join(".git")).unwrap();
        std::fs::write(jb.join(".git/HEAD"), format!("{}\n", "1".repeat(40))).unwrap();
        let wb = deps.join("workbuddy-bench");
        std::fs::create_dir_all(wb.join(".git")).unwrap();
        std::fs::create_dir_all(wb.join("datasets/wb-bench-web-v1.0/tasks")).unwrap();
        let s = status(tmp.path());
        let get = |b: Bench, sub: Option<WbSubset>| {
            s.iter().find(|x| x.bench == b && x.subset == sub).unwrap()
        };
        let l = get(Bench::Lab, None);
        assert!(l.installed && l.data_present);
        assert_eq!(l.at_pinned_commit, Some(true));
        let j = get(Bench::Jb, None);
        assert_eq!(
            (j.installed, j.at_pinned_commit, j.data_present),
            (true, Some(false), false)
        );
        assert!(get(Bench::Wb, Some(WbSubset::Web)).data_present);
        assert!(!get(Bench::Wb, Some(WbSubset::Office)).data_present);
        assert_eq!(status(&tmp.path().join("missing")).len(), 5);
    }
}
