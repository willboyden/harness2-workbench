//! The public ports against golden values computed by upstream harness2 itself
//! (`tests/fixtures/golden/make_goldens.py`): tree validation, report summarise/render,
//! namespace names and the runmark fingerprint.

// Test crate: helpers outside #[test] functions may unwrap and panic.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use h2w_api::{JudgedRow, Mode, Surface};
use h2w_core::namespace::Namespace;
use h2w_core::{report, tree};
use serde_json::Value;

fn golden() -> Value {
    let p =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/golden/expected.json");
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_owned())
        .collect()
}

fn mode(token: &str) -> Mode {
    Mode::from_token(token).unwrap()
}

#[test]
fn validation_matches_upstream_tree_validate() {
    let g = golden();
    let cases = g["validation"].as_array().unwrap();
    assert!(cases.len() >= 10);
    for c in cases {
        let name = c["name"].as_str().unwrap();
        let surface = match c["surface"].as_str().unwrap() {
            "opencode" => Surface::Opencode,
            _ => Surface::Codex,
        };
        let t: tree::Tree = c["tree"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned()))
            .collect();
        let r = tree::validate(surface, &t, c["allow_subagents"].as_bool().unwrap());
        let errors = strings(&c["errors"]);
        assert_eq!(r.errors, errors, "{name}: errors");
        assert_eq!(r.ok, errors.is_empty(), "{name}: ok");
        assert_eq!(
            r.off_surface,
            strings(&c["off_surface"]),
            "{name}: off_surface"
        );
        if let Some(census) = c["census"].as_object() {
            let get = |k: &str| census.get(k).map_or(0, |v| v.as_u64().unwrap());
            assert_eq!(
                u64::from(r.census.always_on_chars),
                get("always_on_chars"),
                "{name}"
            );
            assert_eq!(u64::from(r.census.n_files), get("n_files"), "{name}");
            assert_eq!(u64::from(r.census.skills), get("skills"), "{name}");
            assert_eq!(u64::from(r.census.sub_agents), get("sub_agents"), "{name}");
            assert_eq!(u64::from(r.census.scripts), get("scripts"), "{name}");
            assert_eq!(u64::from(r.census.plugins), get("plugins"), "{name}");
            assert_eq!(u64::from(r.census.hooks), get("hooks"), "{name}");
            assert_eq!(r.census.always_on_max, tree::always_on_max(surface));
        }
    }
}

fn number(v: &Value) -> Option<f64> {
    v.as_f64()
}

#[test]
fn report_matches_upstream_summarise_and_render() {
    let g = golden();
    for c in g["reports"].as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        let rows: Vec<JudgedRow> = c["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                let passed = number(&r["n_passed"]);
                let criteria = number(&r["n_criteria"]);
                let graded = r["graded"].as_bool().unwrap();
                JudgedRow {
                    task_id: r["task_id"].as_str().unwrap().into(),
                    mode: mode(r["mode"].as_str().unwrap()),
                    k: u32::try_from(r["k"].as_u64().unwrap()).unwrap(),
                    label: r["label"].as_str().unwrap().into(),
                    run_id: String::new(),
                    chosen_is_base: r["chosen_is_base"].as_bool().unwrap(),
                    n_passed: None,
                    n_criteria: None,
                    all_pass: r["all_pass"].as_bool().unwrap(),
                    // Upstream's `frac` property, recomputed from the exact inputs.
                    frac: if graded {
                        Some(passed.unwrap() / criteria.unwrap())
                    } else {
                        None
                    },
                    graded,
                    error: String::new(),
                    metric: r["metric"].as_str().map(str::to_owned),
                    points_passed: passed,
                    points_max: criteria,
                }
            })
            .collect();
        let key = |e: &Value| {
            let a = e.as_array().unwrap();
            (
                mode(a[0].as_str().unwrap()),
                u32::try_from(a[1].as_u64().unwrap()).unwrap(),
            )
        };
        let fell_back: BTreeMap<(Mode, u32), u32> = c["fell_back"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| (key(e), u32::try_from(e[2].as_u64().unwrap()).unwrap()))
            .collect();
        let usd: BTreeMap<(Mode, u32), f64> = c["usd"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| (key(e), e[2].as_f64().unwrap()))
            .collect();
        let summaries = report::summarise(&rows, &fell_back, &usd);
        let want = c["summaries"].as_array().unwrap();
        assert_eq!(summaries.len(), want.len(), "{name}");
        for w in want {
            let (m, k) = (
                mode(w["mode"].as_str().unwrap()),
                u32::try_from(w["k"].as_u64().unwrap()).unwrap(),
            );
            let s = summaries
                .iter()
                .find(|s| s.mode == m && s.k == k)
                .unwrap_or_else(|| panic!("{name}: no summary for {m:?} k={k}"));
            assert_eq!(u64::from(s.n), w["n"].as_u64().unwrap(), "{name}");
            assert_eq!(
                u64::from(s.n_ungraded),
                w["n_ungraded"].as_u64().unwrap(),
                "{name}"
            );
            assert_eq!(s.score, w["score"].as_f64(), "{name}: score");
            assert_eq!(s.all_pass, w["all_pass"].as_f64(), "{name}: all_pass");
            assert_eq!(
                u64::from(s.kept_base),
                w["kept_base"].as_u64().unwrap(),
                "{name}"
            );
            assert_eq!(
                u64::from(s.chose_final),
                w["chose_final"].as_u64().unwrap(),
                "{name}"
            );
            assert_eq!(
                u64::from(s.chose_candidate),
                w["chose_candidate"].as_u64().unwrap(),
                "{name}"
            );
            assert_eq!(
                u64::from(s.fell_back),
                w["fell_back"].as_u64().unwrap(),
                "{name}"
            );
            assert_eq!(s.usd, w["usd"].as_f64().unwrap(), "{name}: usd");
            assert_eq!(s.metrics, strings(&w["metrics"]), "{name}: metrics");
        }
        // (k, mode) order, as the doc comment promises.
        let order: Vec<(u32, Mode)> = summaries.iter().map(|s| (s.k, s.mode)).collect();
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(order, sorted, "{name}: order");
        let (text, notes) = report::render(
            &summaries,
            c["title"].as_str().unwrap(),
            c["bench"].as_str().unwrap(),
        );
        assert_eq!(text, c["text"].as_str().unwrap(), "{name}: rendered text");
        let paragraphs = text.matches("\n\n  NOTE").count() + text.matches("\n\n  WARNING").count();
        assert_eq!(notes.len(), paragraphs, "{name}: notes");
    }
}

#[test]
fn namespaces_match_upstream_ns() {
    let g = golden();
    for c in g["namespaces"].as_array().unwrap() {
        let want = c["ns"].as_str().unwrap();
        let args = c["args"].as_array().unwrap();
        let ns = Namespace::parse(want).unwrap_or_else(|| panic!("{want} did not parse"));
        assert_eq!(ns.to_string(), want);
        assert_eq!(ns.bench_name, args[0].as_str().unwrap());
        assert_eq!(ns.substrate.as_cli(), args[1].as_str().unwrap());
        assert_eq!(ns.mode.token(), args[2].as_str().unwrap());
        assert_eq!(u64::from(ns.k), args[3].as_u64().unwrap());
        assert_eq!(ns.seed, args[4].as_u64());
        assert_eq!(ns.tag.as_deref().unwrap_or(""), args[5].as_str().unwrap());
    }
}

#[test]
fn fingerprint_matches_upstream_runmark() {
    let g = golden();
    let fp = &g["fingerprint"];
    let tmp = tempfile::tempdir().unwrap();
    for f in fp["files"].as_array().unwrap() {
        let rel = f[0].as_str().unwrap();
        let p = tmp.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, f[1].as_str().unwrap()).unwrap();
    }
    std::fs::create_dir_all(tmp.path().join("emptydir")).unwrap();
    let dir = cap_std::fs::Dir::open_ambient_dir(tmp.path(), cap_std::ambient_authority()).unwrap();
    assert_eq!(
        tree::fingerprint(&dir).unwrap(),
        fp["expected"].as_str().unwrap()
    );
    let empty = tempfile::tempdir().unwrap();
    let dir =
        cap_std::fs::Dir::open_ambient_dir(empty.path(), cap_std::ambient_authority()).unwrap();
    assert_eq!(
        tree::fingerprint(&dir).unwrap(),
        fp["empty_dir"].as_str().unwrap()
    );
}
