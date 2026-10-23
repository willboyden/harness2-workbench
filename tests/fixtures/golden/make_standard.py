#!/usr/bin/env python3
"""Synthesize STANDARD-layout experiments by running harness2's own CLI code.

Nothing here reimplements harness2: `harness2.cli.run_sweep`, `harness2.cli.judge`,
`harness2.cli.report` and `scripts/split_tasks.py` run unmodified. Only their outside world
is replaced: the benchmark adapter is a deterministic local one (the same ledger task and
script editor as `harness2-demo`, under a real benchmark's name and run-directory
convention), the improver is harness2's own `DemoProposer` plus an audit trail and a
recorded cost, and the judge reads the answer file. No model, network or benchmark checkout
is used.

    <harness2>/.venv/bin/python -I make_standard.py <harness2 checkout> <experiment dir> <stdout dir>

Writes one experiment (HARNESS2_RUNS_DIR) with LAB, JobBench and WorkBuddy-office sweeps,
and the captured stdout of every command into <stdout dir>.
"""

from __future__ import annotations

import contextlib
import csv
import importlib.util
import io
import os
import sys
from decimal import Decimal, InvalidOperation
from pathlib import Path

CHECKOUT = Path(sys.argv[1]).resolve()
EXP = Path(sys.argv[2]).resolve()
OUT = Path(sys.argv[3]).resolve()
WORK = EXP.parent / (EXP.name + "-local-bench")

# harness2.config reads these at import time.
os.environ.update(
    {
        "HARNESS2_RUNS_DIR": str(EXP),
        "HARNESS2_SOLVER_MODEL": "local/solver",
        "HARNESS2_IMPROVER_MODEL": "local/improver",
        "HARNESS2_JUDGE_MODEL": "local-judge",
    }
)
for name in (
    "GOOGLE_CLOUD_PROJECT",
    "HARNESS2_VERTEX_PROJECT",
    "HARNESS2_JUDGE_BASE_URL",
    "HARNESS2_JUDGE_API_KEY",
    "HARNESS2_CODEX_BASE_URL",
    "HARNESS2_CODEX_API_KEY",
):
    os.environ.pop(name, None)

from harness2 import preflight  # noqa: E402
from harness2.cli import _common as C  # noqa: E402
from harness2.cli import judge as judge_cli  # noqa: E402
from harness2.cli import report as report_cli  # noqa: E402
from harness2.cli import run_sweep  # noqa: E402
from harness2.core import proposer as P  # noqa: E402
from harness2.core import tree as HT  # noqa: E402
from harness2.core.spec import BenchPrompts, BenchSpec, ChannelSpec  # noqa: E402
from harness2.demo import BASE_SCRIPT, DemoAdapter, DemoProposer  # noqa: E402
from harness2.substrate.opencode import OpencodeSubstrate  # noqa: E402

CHANNEL = ChannelSpec("opencode", "local/improver")


class LocalBench(DemoAdapter):
    """harness2-demo's adapter under a real benchmark's name and run-directory convention."""

    def __init__(self, name, runs_root, ledgers, domains, run_dir_of, judge, domain_word):
        self.root = WORK / name
        self.substrate = OpencodeSubstrate()
        self.spec = BenchSpec(
            name=name,
            runs_root=runs_root,
            install_mode="workspace_config",
            prompts=BenchPrompts(work="reconcile signed amounts in a transaction ledger"),
            channels={"proposer": CHANNEL, "selector": CHANNEL},
            domain_word=domain_word,
            leak_globs=("reference_answer.txt",),
        )
        self.execution_count = 0
        self._ledgers = ledgers
        self._domains = domains
        self._run_dir_of = run_dir_of
        self._judge = judge
        base = {n: "" for n in self.substrate.surface().always_on}
        base["systemprompt.md"] = "Reconcile the ledger using python scripts/summarize.py.\n"
        base["scripts/summarize.py"] = BASE_SCRIPT
        HT.write_tree(self.substrate.surface(), base, self.base_harness(), stubs=False)
        for task_id, amounts in ledgers.items():
            src = self.task_files(task_id)
            src.mkdir(parents=True, exist_ok=True)
            (src / "ledger.csv").write_text("amount\n" + amounts, encoding="utf-8")

    def tasks(self, split="all"):
        return sorted(self._ledgers)

    def domain_of(self, task_id):
        return self._domains[task_id]

    def reference_domain_order(self):
        return sorted(set(self._domains.values()))

    def task_files(self, task_id):
        return self.root / "sources" / task_id.replace("/", "__")

    def task_prompt(self, task_id):
        return (
            f"# TASK {task_id}\n\nSum every signed amount in ledger.csv. "
            "Return Total: with two decimal places."
        )

    def execute(self, task_id, harness_dir, run_id, *, timeout=None):
        import subprocess

        self.execution_count += 1
        run_dir = self._run_dir_of(run_id, task_id)
        run_dir.mkdir(parents=True, exist_ok=True)
        script = (harness_dir or self.base_harness()) / "scripts/summarize.py"
        result = subprocess.run(
            [sys.executable, str(script), str(self.task_files(task_id) / "ledger.csv")],
            capture_output=True,
            text=True,
            check=True,
            timeout=timeout or 10,
        )
        (run_dir / "answer.txt").write_text(result.stdout, encoding="utf-8")
        (run_dir / "trajectory.md").write_text(
            "[ACTION] Run scripts/summarize.py on ledger.csv\n[OBSERVATION]\n" + result.stdout,
            encoding="utf-8",
        )
        return run_dir

    def expected(self, task_id):
        with (self.task_files(task_id) / "ledger.csv").open() as fh:
            return sum((Decimal(r["amount"]) for r in csv.DictReader(fh)), Decimal(0))

    def score(self, run_dir, task_id):
        return self._judge(self, Path(run_dir), task_id)


class AuditedProposer(DemoProposer):
    """harness2's DemoProposer, plus the audit trail and costs a real improver records."""

    COST = {"select": 0.004, "experience": 0.006}

    def call(self, mission, context_dir, label, **kwargs):
        ctx = str(context_dir)
        # One stage of one task proposes an invalid tree twice, so it falls back.
        if "contracts__msa-redline" in ctx and "-agg1" in ctx and label.startswith("dom1."):
            ws = Path(context_dir) / "harness"
            (ws / "scripts" / "summarize.py").unlink(missing_ok=True)
            reply = P.Reply(
                label=label,
                text="<notes>Dropped the helper script.</notes>",
                ok=True,
                attempts=1,
                workspace=ws,
            )
            self._calls.append(reply)
        else:
            reply = super().call(mission, context_dir, label, **kwargs)
        reply.cost_usd = self.COST.get(label.split(".", 1)[0], 0.0125)
        reply.channel = self.spec.kind
        reply.wall_s = 1.5
        system = kwargs.get("system", "")
        self._write_audit(reply, mission, system, P._call_fingerprint(label, mission, system, context_dir))
        return reply


def lab_run_dir(run_id, task_id):
    return EXP / "lab" / "results" / "oc" / run_id


def jb_run_dir(run_id, task_id):
    return EXP / "jb" / "dataset" / task_id / "model_output" / run_id.replace("/", "__")


def wb_run_dir(run_id, task_id):
    label = run_id.replace("/", "__")
    safe = "".join(c if c.isalnum() or c in "_.-" else "-" for c in label)
    return EXP / "wb" / "office" / "rollouts" / safe / "2026-10-23__12-00-00" / f"{task_id}__a1b2c3"


def answer_ok(adapter, run_dir, task_id):
    try:
        got = (run_dir / "answer.txt").read_text(encoding="utf-8")
    except OSError:
        return False
    try:
        return f"Total: {adapter.expected(task_id):.2f}" in got
    except InvalidOperation:
        return False


def lab_judge(adapter, run_dir, task_id):
    if task_id == "contracts/nda-review" and "seq" in str(run_dir):
        raise RuntimeError("judge transport error (synthetic)")
    ok = answer_ok(adapter, run_dir, task_id)
    return {"n_passed": 4 if ok else 1, "n_criteria": 4, "all_pass": ok}


def jb_judge(adapter, run_dir, task_id):
    ok = answer_ok(adapter, run_dir, task_id)
    if task_id == "auditor/task1":
        return {
            "n_passed": 3 if ok else 1,
            "n_criteria": 5,
            "all_pass": False,
            "metric": "unweighted-fallback",
        }
    return {
        "n_passed": 7.5 if ok else 2.25,
        "n_criteria": 10.0,
        "all_pass": False,
        "metric": "weighted",
        "criteria_passed": 3,
        "criteria_count": 4,
        "judge_model": "local-judge",
    }


def wb_judge(adapter, run_dir, task_id):
    ok = answer_ok(adapter, run_dir, task_id)
    sc = 1.0 if ok else 0.35
    return {"n_passed": round(sc * 100), "n_criteria": 100, "all_pass": sc >= 1.0, "raw_score": sc}


ADAPTERS = {
    "lab": lambda: LocalBench(
        "lab",
        EXP / "lab",
        {
            "contracts/nda-review": "100.25\n-20.10\n5.00\n",
            "contracts/msa-redline": "35.50\n-10.25\n-5.00\n",
            "litigation/motion-draft": "12.00\nnot-a-number\n",
        },
        {
            "contracts/nda-review": "contracts",
            "contracts/msa-redline": "contracts",
            "litigation/motion-draft": "litigation",
        },
        lab_run_dir,
        lab_judge,
        "practice area",
    ),
    "jb": lambda: LocalBench(
        "jb",
        EXP / "jb",
        {
            "accountant/task1": "10.00\n-2.50\n",
            "accountant/task2": "3.00\n4.00\n-1.00\n",
            "auditor/task1": "7.25\n-0.25\n",
        },
        {
            "accountant/task1": "accountant",
            "accountant/task2": "accountant",
            "auditor/task1": "auditor",
        },
        jb_run_dir,
        jb_judge,
        "profession",
    ),
    "wb": lambda: LocalBench(
        "wb-office",
        EXP / "wb" / "office",
        {"sheet-merge": "1.50\n-0.50\n", "deck-outline": "2.00\n2.00\n"},
        {"sheet-merge": "spreadsheets", "deck-outline": "presentations"},
        wb_run_dir,
        wb_judge,
        "category",
    ),
}
_instances = {}


def make_adapter(bench, subset, substrate, split_file=None):
    if bench not in _instances:
        _instances[bench] = ADAPTERS[bench]()
    return _instances[bench]


C.make_adapter = make_adapter
preflight.require_ready = lambda adapter, judge=False: None
run_sweep.Proposer = AuditedProposer


def capture(name, fn, argv):
    buf = io.StringIO()
    with contextlib.redirect_stdout(buf):
        try:
            code = fn(argv)
        except SystemExit as exc:  # report/judge exit through SystemExit on refusal
            code = exc.code
    (OUT / f"{name}.stdout").write_text(buf.getvalue(), encoding="utf-8")
    (OUT / f"{name}.exit").write_text(f"{code}\n", encoding="utf-8")
    print(f"{name}: exit {code}", file=sys.stderr)


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    lab = ["--bench", "lab"]
    capture("lab-preview-k1", run_sweep.main, lab + ["--k", "1", "--modes", "parallel,sequential", "--dry-run"])
    capture("lab-run-k1", run_sweep.main, lab + ["--k", "1", "--modes", "parallel,sequential"])
    capture("lab-judge-k1", judge_cli.main, lab + ["--k", "1", "--modes", "parallel,sequential"])
    capture("lab-judge-bases-k1", judge_cli.main, lab + ["--k", "1", "--bases"])
    capture("lab-report-k1", report_cli.main, lab + ["--k", "1", "--modes", "parallel,sequential"])
    capture("lab-run-k2", run_sweep.main, lab + ["--k", "2", "--modes", "parallel"])
    capture("lab-judge-k2", judge_cli.main, lab + ["--k", "2", "--modes", "parallel"])
    capture("lab-report-k1k2", report_cli.main, lab + ["--k", "1,2", "--modes", "parallel,sequential"])
    jb = ["--bench", "jb"]
    capture("jb-preview-k1-s3", run_sweep.main, jb + ["--k", "1", "--seed", "3", "--max-tasks", "5", "--dry-run"])
    capture("jb-run-k1-s3", run_sweep.main, jb + ["--k", "1", "--seed", "3", "--modes", "parallel"])
    capture("jb-judge-k1-s3", judge_cli.main, jb + ["--k", "1", "--seed", "3", "--modes", "parallel"])
    capture("jb-report-k1-s3", report_cli.main, jb + ["--k", "1", "--seed", "3", "--modes", "parallel"])
    wb = ["--bench", "wb", "--subset", "office"]
    capture("wb-office-run-k1", run_sweep.main, wb + ["--k", "1", "--modes", "sequential"])
    capture("wb-office-judge-k1", judge_cli.main, wb + ["--k", "1", "--modes", "sequential"])
    capture("wb-office-report-k1", report_cli.main, wb + ["--k", "1", "--modes", "sequential"])

    # scripts/split_tasks.py, with the task discovery answered by the local adapter.
    spec = importlib.util.spec_from_file_location("split_tasks", CHECKOUT / "scripts" / "split_tasks.py")
    split_tasks = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(split_tasks)
    lab_bench = make_adapter("lab", None, "oc")
    split_tasks.discover = lambda bench, subset=None: {t: lab_bench.domain_of(t) for t in lab_bench.tasks()}
    (OUT / "splits").mkdir(exist_ok=True)
    capture(
        "split-lab",
        split_tasks.main,
        ["--bench", "lab", "--seed", "42", "--train-fraction", "0.5", "--output", str(OUT / "splits" / "lab-half.json")],
    )


if __name__ == "__main__":
    main()
