#!/usr/bin/env python3
"""Compute golden expected values with harness2's own code and Python itself.

    <harness2>/.venv/bin/python -I make_goldens.py <output json>

Every expected value below comes from calling upstream harness2 (tree validation, report
summarise/render, runmark fingerprint, namespace builder) or CPython (json.loads errors,
sum, statistics.mean, round, repr, re) on the inputs listed here. The Rust port must
reproduce each one byte for byte; see crates/h2w-core/tests/golden.rs.
"""

from __future__ import annotations

import json
import random
import re
import statistics
import sys
import tempfile
from pathlib import Path

from harness2.core import report as R
from harness2.core import runmark
from harness2.core import schedule
from harness2.core import tree as HT
from harness2.core.judge import Scored
from harness2.substrate.codex import CODEX_SURFACE
from harness2.substrate.opencode import OPENCODE_SURFACE

SURFACES = {"opencode": OPENCODE_SURFACE, "codex": CODEX_SURFACE}

OC_BASE = {
    "systemprompt.md": "You draft contracts. When reviewing, load the `review` skill.\n",
    "guardrails.md": "Never invent clauses.\n",
    "LongTermMEMORY.md": "",
    "tool_descriptions/tool_guidance.md": "Prefer python3 scripts/check_totals.py for sums.\n",
    "skills/review/SKILL.md": "---\nname: review\ndescription: review a draft\n---\nSteps.\n",
    "scripts/check_totals.py": "print('ok')\n",
}


def oc(changes):
    """OC_BASE with some files replaced (None removes one)."""
    t = dict(OC_BASE)
    for k, v in changes.items():
        if v is None:
            t.pop(k, None)
        else:
            t[k] = v
    return t


VALIDATION_CASES = [
    ("oc-valid", "opencode", OC_BASE, True),
    ("oc-valid-no-subagents-flag", "opencode", OC_BASE, False),
    (
        "oc-many-problems",
        "opencode",
        {
            "systemprompt.md": "Run python3 scripts/missing.py then python skills/tables/scripts/fmt.py "
            "and python3 scripts/read_doc.py. cpython3  scripts/also_missing.py",
            "guardrails.md": "",
            "skills/silent/SKILL.md": "---\nname: other\n---\nbody\n",
            "skills/tables/SKILL.md": "description:\n  formats tables\nname:   tables  \n",
            "skills/tables/scripts/fmt.py": "x = 1\n",
            "sub_agents/checker/AGENT.md": "---\ndescription: checks\n---\n",
            "sub_agents/writer/AGENT.md": "mode: subagent\n",
            "plugin/guard.ts": "export default {}\n",
            "skills/README.md": "stub",
            "notes.txt": "off surface",
            "Skills/x/SKILL.md": "off surface (case)",
            "scripts/Bad.py": "off surface (case)",
            "./tool_descriptions/tool_guidance.md": "Use python3 skills/silent/scripts/a.py",
        },
        False,
    ),
    ("oc-cap", "opencode", oc({"guardrails.md": "é" * 24_001}), True),
    (
        "oc-name-forms",
        "opencode",
        oc({
            "skills/review/SKILL.md": "name:\nreview\ndescription: x\n",
            "skills/alpha/SKILL.md": "---\nname: alpha beta\ndescription: x\n---\n",
            "skills/gamma/SKILL.md": "---\nname: gamma \ndescription:  x\n---\n",
            "systemprompt.md": "alpha gamma review",
        }),
        True,
    ),
    (
        "oc-frontmatter-window",
        "opencode",
        oc({"skills/review/SKILL.md": "x" * 599 + "\nname: review\ndescription: late\n"}),
        True,
    ),
    (
        "oc-mode-subagent-forms",
        "opencode",
        oc({
            "sub_agents/a/AGENT.md": "mode:subagent\ndescription: a\n",
            "sub_agents/b/AGENT.md": "mode: subagents\ndescription: b\n",
            "sub_agents/c/AGENT.md": "mode:\n subagent \ndescription: c\n",
            "sub_agents/d/AGENT.md": "description:\nmode: subagent x\n",
        }),
        True,
    ),
    ("cx-valid", "codex", {"AGENTS.md": "Read .agents/skills/doc/SKILL.md for docs."}, True),
    (
        "cx-problems",
        "codex",
        {
            "AGENTS.md": "\u001f \n\t",
            ".agents/skills/doc/SKILL.md": "name: docs\n",
            ".agents/skills/doc/scripts/run.py": "print(1)\n",
            "sub_agents/helper/AGENT.md": "no frontmatter\n",
            ".codex/hooks.json": json.dumps(
                {
                    "hooks": {
                        "postToolUse": [],
                        "PreToolUse": [
                            {"matcher": "Bash", "hooks": [{"type": "command", "command": "x"}]},
                            {"hooks": "nope"},
                            {"hooks": [{"type": "prompt"}, {"type": "command"}, {"type": "command", "command": None}, {"type": "command", "command": 0}, {"type": "command", "command": " \u001c"}]},
                            ["not", "a", "dict"],
                        ],
                        "Stop": {"x": 1},
                        "it's": [],
                    }
                }
            ),
        },
        True,
    ),
    ("cx-hooks-empty", "codex", {"AGENTS.md": "a", ".codex/hooks.json": '{"hooks": {}}'}, True),
    ("cx-hooks-not-object", "codex", {"AGENTS.md": "a", ".codex/hooks.json": "[1]"}, True),
    ("cx-cap", "codex", {"AGENTS.md": "x" * 8_001}, True),
    (
        "cx-dup-keys",
        "codex",
        {
            "AGENTS.md": "a",
            ".codex/hooks.json": '{"hooks": {"Stop": [], "Bad": [], "Stop": {"k": 1}}}',
        },
        True,
    ),
]

JSON_ERROR_INPUTS = [
    "",
    " ",
    "{",
    "}",
    "[",
    "[1,]",
    '{"a": 1,}',
    '{"a" 1}',
    '{"a": }',
    "[1 2]",
    '{"a": 1}\n{"b": 2}',
    '"abc',
    '"a\\qb"',
    '"\\u12"',
    '"\\u12G4"',
    '"\\ud800\\uZZZZ"',
    '"tab\tin string"',
    "nul",
    "tru",
    "-",
    "-x",
    "01",
    "1.",
    "1e",
    "[1e+]",
    "﻿{}",
    '{"a": [1, 2, {"b": nul}]}',
    '\n\n  {"x": 1,\n  "y" 2}',
    '{"é": "x"} é',
    "NaN x",
    "[Infinity, -Infinity, NaN]",
]

FRONTMATTER_INPUTS = [
    "---\ndescription: x\n",
    "description:\n\n  later",
    "description:   \n  ",
    "xdescription: y",
    "description y",
    "\ndescription: z",
    "description:\u001c\u001d",
    "---\nname: foo\n",
    "name: foo",
    "name:\nfoo\n",
    "name: foo bar\nname: baz\n",
    "name: foo bar",
    "name:   ",
    "name: foo \t\n",
    "name:foo \nx",
    "mode: subagent\n",
    "x\nmode:subagent",
    "mode: subagents\n",
    "mode: subagent x\n",
    "mode:\n\nsubagent\n\n",
    "mode: subagent ",
]


def oc_script_refs(src):
    return HT._rx(OPENCODE_SURFACE)["script_ref"].findall(src)


SCRIPT_REF_INPUTS = [
    "python3 scripts/a.py and python scripts/b.py",
    "cpython3  scripts/c.py",
    "python3\u001cscripts/d.py",
    "python3 skills/x-y/scripts/e.py.bak",
    "python3 scripts/f.pyc",
    "python 3 scripts/g.py",
    "python3 scripts/ünï.py",
    "python3 skills/a/scripts/b.py python3 scripts/c.py",
]


def scored(task, mode, k, label, n_passed, n_criteria, all_pass, chosen_is_base=False, metric=None):
    raw = {"metric": metric} if metric else {}
    return Scored(
        task_id=task,
        mode=mode,
        k=k,
        label=label,
        run_id=f"x/{task}/{label}",
        chosen_is_base=chosen_is_base,
        n_passed=n_passed,
        n_criteria=n_criteria,
        all_pass=all_pass,
        raw=raw,
    )


def random_rows(seed, n):
    rnd = random.Random(seed)
    rows = []
    for i in range(n):
        mode = rnd.choice(["agg", "seq"])
        k = rnd.choice([1, 1, 2, 3])
        if mode == "agg":
            label = rnd.choice(["agg1", "agg2", "inv1", "base1"])
        else:
            label = rnd.choice(["c1", "c2", "c3", "dom1", "base2"])
        crit = rnd.choice([None, 0, 3, 4, 7, 10, 11.5])
        passed = None if crit is None else rnd.choice([0, 1, 2, 3, 2.5, 7.25])
        if crit is not None and passed is not None and passed > crit:
            passed = crit
        rows.append(
            scored(
                f"t{i}",
                mode,
                k,
                label,
                passed,
                crit,
                rnd.random() < 0.3,
                chosen_is_base=label.startswith("base"),
                metric=rnd.choice([None, None, "weighted", "unweighted-fallback"]),
            )
        )
    return rows


REPORT_CASES = [
    (
        "lab-noise-floor",
        "lab-oc modes=parallel,sequential",
        "lab",
        [
            scored("a", "agg", 1, "agg1", 3, 4, False),
            scored("b", "agg", 1, "agg1", 4, 4, True),
            scored("c", "agg", 1, "agg1", None, None, False),
            scored("a", "seq", 1, "c1", 1, 3, False),
            scored("b", "seq", 1, "inv1", 2, 3, False),
        ],
        {("seq", 1): 2},
        {("agg", 1): 1.005, ("seq", 1): 0.125},
    ),
    (
        "jb-mixed-units",
        "jb-oc seed=3 modes=parallel",
        "jb",
        [
            scored("a", "agg", 1, "agg1", 7.5, 10.0, False, metric="weighted"),
            scored("b", "agg", 1, "base1", 3, 5, False, chosen_is_base=True, metric="unweighted-fallback"),
            scored("c", "agg", 1, "inv2", 2.25, 10.0, False, metric="weighted"),
        ],
        {},
        {("agg", 1): 0.0},
    ),
    (
        "multi-k-ordering",
        "lab-oc modes=parallel,sequential",
        "lab",
        [
            scored("a", "seq", 2, "c2", 1, 2, False),
            scored("a", "agg", 2, "agg2", 2, 2, True),
            scored("a", "agg", 1, "base1", 0, 2, False, chosen_is_base=True),
            scored("b", "agg", 1, "base2", 1, 2, False, chosen_is_base=True),
            scored("a", "seq", 1, "c1", 2, 2, True),
        ],
        {("agg", 1): 1, ("agg", 2): 0, ("seq", 2): 3},
        {},
    ),
    ("random-1", "wb-office-cx modes=parallel,sequential", "wb-office", random_rows(1, 40), {}, {}),
    ("random-2", "lab-cx modes=parallel,sequential", "lab", random_rows(2, 200), {("agg", 1): 5}, {("seq", 3): 12.3456}),
    ("no-title", "", "jb", [scored("a", "agg", 1, "agg1", 1, 3, False)], {}, {}),
]

FINGERPRINT_TREE = [
    ["a/b", "one"],
    ["a-b/x", "two"],
    ["a.b", "three"],
    ["A", "upper"],
    [".DS_Store", "ignored"],
    ["sub/.DS_Store", "ignored"],
    ["x.pyc", "ignored"],
    [".pyc", "kept: a leading dot is not a suffix"],
    ["__pycache__/y.txt", "ignored"],
    ["deep/__pycache__/z", "ignored"],
    [".adapt_complete", "ignored"],
    ["deep/.adapt_complete", "ignored"],
    ["é/ü.md", "unicode"],
    ["empty", ""],
]


def summary_dict(s):
    return {
        "mode": s.mode,
        "k": s.k,
        "n": s.n,
        "n_ungraded": s.n_ungraded,
        "score": s.score,
        "all_pass": s.all_pass,
        "kept_base": s.kept_base,
        "chose_final": s.chose_final,
        "chose_candidate": s.chose_candidate,
        "fell_back": s.fell_back,
        "usd": s.usd,
        "metrics": list(s.metrics),
    }


def main():
    out = {}

    cases = []
    for name, surf, tree, allow in VALIDATION_CASES:
        spec = SURFACES[surf]
        stubs = {rel for rel, _ in spec.stubs}
        on, off = {}, []
        for rel in sorted(tree, key=lambda r: Path(r).parts):
            if not HT.is_allowed(spec, rel):
                if rel not in stubs:
                    off.append(rel)
                continue
            n = HT.normalize_rel(rel)
            if n in stubs:
                continue
            on[n] = tree[rel]
        on = dict(sorted(on.items(), key=lambda kv: Path(kv[0]).parts))
        try:
            census = HT.validate(spec, on, allow_subagents=allow)
            errors = []
        except ValueError as exc:
            census = None
            errors = str(exc).split(" | ")
        cases.append(
            {
                "name": name,
                "surface": surf,
                "allow_subagents": allow,
                "tree": tree,
                "errors": errors,
                "off_surface": off,
                "census": census,
            }
        )
    out["validation"] = cases

    json_errors = []
    for text in JSON_ERROR_INPUTS:
        try:
            json.loads(text)
            json_errors.append({"input": text, "error": None})
        except json.JSONDecodeError as exc:
            json_errors.append({"input": text, "error": str(exc)})
    out["json_errors"] = json_errors

    fm = []
    for text in FRONTMATTER_INPUTS:
        m = re.search(r"^name:\s*(\S+)\s*$", text, re.M)
        fm.append(
            {
                "input": text,
                "description": bool(re.search(r"^description:\s*\S", text, re.M)),
                "name": m.group(1) if m else None,
                "mode_subagent": bool(re.search(r"^mode:\s*subagent\s*$", text, re.M)),
            }
        )
    out["frontmatter"] = fm

    out["script_refs"] = [{"input": s, "refs": oc_script_refs(s)} for s in SCRIPT_REF_INPUTS]

    reports = []
    for name, title, bench, rows, fell_back, usd in REPORT_CASES:
        summaries = R.summarise(rows, fell_back=fell_back, usd=usd)
        reports.append(
            {
                "name": name,
                "title": title,
                "bench": bench,
                "rows": [
                    {
                        "task_id": r.task_id,
                        "mode": r.mode,
                        "k": r.k,
                        "label": r.label,
                        "chosen_is_base": r.chosen_is_base,
                        "n_passed": r.n_passed,
                        "n_criteria": r.n_criteria,
                        "all_pass": r.all_pass,
                        "frac": r.frac,
                        "graded": r.graded,
                        "metric": r.raw.get("metric"),
                    }
                    for r in rows
                ],
                "fell_back": [[m, k, v] for (m, k), v in fell_back.items()],
                "usd": [[m, k, v] for (m, k), v in usd.items()],
                "summaries": [summary_dict(s) for s in summaries],
                "text": R.render(summaries, title=title, bench=bench),
            }
        )
    out["reports"] = reports

    rnd = random.Random(11)
    floats = []
    for _ in range(300):
        n = rnd.randint(1, 12)
        xs = [rnd.choice([rnd.random(), rnd.randint(0, 40) / rnd.randint(1, 40), 0.1, 1e-17, 1e16, -0.3]) for _ in range(n)]
        # Floats travel as repr() strings: JSON readers need not round-trip 17-digit values.
        floats.append(
            {"xs": [repr(x) for x in xs], "sum": repr(sum(xs)), "mean": repr(statistics.mean(xs))}
        )
    out["sums"] = floats
    rounds = []
    for _ in range(2000):
        x = rnd.choice([rnd.random() * 100, rnd.randint(0, 100000) / 1000, rnd.randint(0, 999) / 8])
        rounds.append({"x": repr(x), "r2": repr(round(x, 2)), "r1": repr(round(x, 1)), "r4": repr(round(x, 4))})
    for x in [0.125, 0.375, 2.675, 1.005, 66.665, 0.5, 1.5, 2.5, -0.0, 1e-9]:
        rounds.append({"x": repr(x), "r2": repr(round(x, 2)), "r1": repr(round(x, 1)), "r4": repr(round(x, 4))})
    out["rounds"] = rounds

    reprs = ["PostToolUse", "it's", "a'b\"c", "tab\there\n", "\x01é", "back\\slash", "\x7f", "日本"]
    out["reprs"] = [{"input": s, "repr": repr(s)} for s in reprs]

    out["namespaces"] = [
        {"args": [b, s, m, k, seed, tag], "ns": schedule.ns(b, s, m, k, seed=seed, tag=tag)}
        for (b, s, m, k, seed, tag) in [
            ("lab", "oc", "agg", 1, None, ""),
            ("wb-office", "cx", "seq", 12, 7, ""),
            ("jb", "oc", "agg", 3, 0, "v2.b"),
            ("demo", "oc", "seq", 2, None, "s7x"),
        ]
    ]

    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        for rel, content in FINGERPRINT_TREE:
            p = root / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(content, encoding="utf-8")
        (root / "emptydir").mkdir()
        fp = runmark.fingerprint(root)
        (root / "only").mkdir()
        empty = runmark.fingerprint(root / "only")
    out["fingerprint"] = {"files": FINGERPRINT_TREE, "expected": fp, "empty_dir": empty}

    Path(sys.argv[1]).write_text(json.dumps(out, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
