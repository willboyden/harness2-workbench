//! Byte-for-byte checks of the private Python-compatibility helpers against values produced
//! by CPython 3.12 and upstream harness2 (`tests/fixtures/golden/make_goldens.py`).

use serde_json::Value;

use crate::{pyfmt, pyjson};

const EXPECTED: &str = include_str!("../../../tests/fixtures/golden/expected.json");

fn golden() -> Value {
    serde_json::from_str(EXPECTED).unwrap()
}

/// Floats are stored as Python `repr` strings; Rust's `str::parse` rounds correctly.
fn f(v: &Value) -> f64 {
    v.as_str().unwrap().parse().unwrap()
}

#[test]
fn neumaier_sum_and_exact_mean_match_cpython() {
    let g = golden();
    let cases = g["sums"].as_array().unwrap();
    assert!(cases.len() >= 100);
    for c in cases {
        let xs: Vec<f64> = c["xs"].as_array().unwrap().iter().map(f).collect();
        assert_eq!(
            pyfmt::sum(xs.iter().copied()).to_bits(),
            f(&c["sum"]).to_bits(),
            "sum {xs:?}"
        );
        assert_eq!(
            pyfmt::mean(&xs).unwrap().to_bits(),
            f(&c["mean"]).to_bits(),
            "mean {xs:?}"
        );
    }
}

#[test]
fn round_matches_cpython() {
    let g = golden();
    for c in g["rounds"].as_array().unwrap() {
        let x = f(&c["x"]);
        assert_eq!(pyfmt::round(x, 2), f(&c["r2"]), "round({x}, 2)");
        assert_eq!(pyfmt::round(x, 1), f(&c["r1"]), "round({x}, 1)");
        assert_eq!(pyfmt::round(x, 4), f(&c["r4"]), "round({x}, 4)");
    }
}

#[test]
fn frontmatter_matchers_match_python_re() {
    let g = golden();
    for c in g["frontmatter"].as_array().unwrap() {
        let input = c["input"].as_str().unwrap();
        assert_eq!(
            pyfmt::has_field(input, "description"),
            c["description"].as_bool().unwrap(),
            "description {input:?}"
        );
        assert_eq!(
            pyfmt::name_field(input),
            c["name"].as_str(),
            "name {input:?}"
        );
        assert_eq!(
            pyfmt::has_mode_subagent(input),
            c["mode_subagent"].as_bool().unwrap(),
            "mode {input:?}"
        );
    }
}

#[test]
fn repr_matches_python() {
    let g = golden();
    for c in g["reprs"].as_array().unwrap() {
        let input = c["input"].as_str().unwrap();
        assert_eq!(
            pyfmt::repr_str(input),
            c["repr"].as_str().unwrap(),
            "{input:?}"
        );
    }
}

#[test]
fn json_errors_match_cpython() {
    let g = golden();
    for c in g["json_errors"].as_array().unwrap() {
        let input = c["input"].as_str().unwrap();
        let got = pyjson::loads(input).err();
        assert_eq!(got.as_deref(), c["error"].as_str(), "json.loads({input:?})");
    }
}

#[test]
fn script_references_match_upstream_regex() {
    let g = golden();
    for c in g["script_refs"].as_array().unwrap() {
        let input = c["input"].as_str().unwrap();
        let want: Vec<&str> = c["refs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r.as_str().unwrap())
            .collect();
        assert_eq!(
            crate::tree::script_refs(h2s_api::Surface::Opencode, input),
            want,
            "{input:?}"
        );
    }
}
