#!/usr/bin/env python3
"""Regenerate everything under tests/fixtures/ from a harness2 checkout.

    <harness2>/.venv/bin/python -I tests/fixtures/golden/regenerate.py \\
        <harness2 checkout> <empty scratch dir> <repo>/tests/fixtures

The checkout must be google-research/harness2 at 1cfe254 with its `.venv` (Python 3.12)
installed. Nothing here calls a model or the network: the demo and the dry-run/check
commands are offline, and the STANDARD layouts come from make_standard.py. Absolute paths in
every output are rewritten to `/fixture/...` so no machine path is committed.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
CHECKOUT, WORK, OUT = (Path(p).resolve() for p in sys.argv[1:4])
VENV = CHECKOUT / ".venv" / "bin"
RAW = WORK / "raw"
HOME = WORK / "home"


def env() -> dict[str, str]:
    return {
        "PATH": "/usr/bin:/bin",
        "HOME": str(HOME),
        "LANG": "C.UTF-8",
        "HARNESS2_RUNS_DIR": str(WORK / "runs"),
    }


def write(rel: str, text: str) -> None:
    p = OUT / rel
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(text, encoding="utf-8")


def capture(name: str, argv: list[str]) -> None:
    """Run one harness2 command offline and keep its stdout, stderr and exit code."""
    p = subprocess.run(argv, cwd=WORK, env=env(), capture_output=True, text=True, timeout=600)
    write(f"cli/{name}.stdout", p.stdout)
    write(f"cli/{name}.stderr", p.stderr)
    write(f"cli/{name}.exit", f"{p.returncode}\n")


def prune_task(task_dir: Path, keep_prompt: bool) -> None:
    """Drop the improver/selector evidence copies the reader never opens; keep one task.md."""
    ctx = task_dir / "context"
    if ctx.is_dir():
        prompt = None
        for label in sorted(p for p in ctx.iterdir() if p.is_dir()):
            if (label / "task.md").is_file():
                prompt = (label.name, (label / "task.md").read_text(encoding="utf-8"))
                break
        shutil.rmtree(ctx)
        if keep_prompt and prompt:
            (ctx / prompt[0]).mkdir(parents=True)
            (ctx / prompt[0] / "task.md").write_text(prompt[1], encoding="utf-8")
    shutil.rmtree(task_dir / "reflection", ignore_errors=True)
    sel = task_dir / "selection"
    if sel.is_dir():
        for sub in sel.iterdir():
            for p in list(sub.iterdir()) if sub.is_dir() else []:
                if p.name != "selection.json":
                    shutil.rmtree(p) if p.is_dir() else p.unlink()


def prune_namespaces(ns_root: Path) -> None:
    for ns in sorted(p for p in ns_root.iterdir() if p.is_dir() and not p.name.startswith("_")):
        for task in sorted(p for p in ns.iterdir() if p.is_dir()):
            prune_task(task, keep_prompt="-agg" in ns.name)


def rewrite_tree(root: Path, replacements: list[tuple[str, str]]) -> None:
    for p in sorted(root.rglob("*")):
        if p.is_file() and not p.is_symlink():
            text = p.read_text(encoding="utf-8")
            new = text
            for old, rep in replacements:
                new = new.replace(old, rep)
            if new != text:
                p.write_text(new, encoding="utf-8")


def digest_and_size(exp: Path, ns_roots: list[str]) -> dict:
    """What ExperimentDir::results_digest and size_bytes must return for this fixture."""
    paths = []
    for root in ns_roots:
        base = exp / root
        if not base.is_dir():
            continue
        for ns in sorted(base.iterdir()):
            if not ns.is_dir() or ns.name.startswith("_"):
                continue
            for name in ("judged.json", "base_judged.json"):
                if (ns / name).is_file():
                    paths.append(f"{root}/{ns.name}/{name}")
            for task in sorted(ns.iterdir()):
                if task.is_dir() and (task / "summary.json").is_file():
                    paths.append(f"{root}/{ns.name}/{task.name}/summary.json")
    h = hashlib.sha256()
    for rel in sorted(paths, key=lambda s: s.encode()):
        h.update(rel.encode())
        h.update(b"\0")
        h.update((exp / rel).read_bytes())
        h.update(b"\0")
    size = sum(p.stat().st_size for p in exp.rglob("*") if p.is_file() and not p.is_symlink())
    return {"results_digest": h.hexdigest(), "size_bytes": size, "files": len(paths)}


def main() -> None:
    if RAW.exists() or (WORK / "runs").exists():
        sys.exit(f"{WORK} is not empty; use a fresh scratch directory")
    RAW.mkdir(parents=True)
    HOME.mkdir()
    (WORK / "runs").mkdir()
    for d in ("cli", "demo-k2", "demo-k1-parallel", "std"):
        shutil.rmtree(OUT / d, ignore_errors=True)
    (OUT / "golden" / "expected.json").unlink(missing_ok=True)

    capture("demo-k2", [str(VENV / "harness2-demo"), "--output", str(RAW / "demo-k2"), "--k", "2", "--mode", "both"])
    capture(
        "demo-k1-parallel",
        [str(VENV / "harness2-demo"), "--output", str(RAW / "demo-k1-parallel"), "--k", "1", "--mode", "parallel"],
    )
    for args in (["--bench", "lab"], ["--bench", "jb"], ["--bench", "wb", "--subset", "office"], ["--bench", "lab", "--substrate", "cx"]):
        capture("check-" + "-".join(a.lstrip("-") for a in args if a not in ("--bench", "--subset", "--substrate")), [str(VENV / "harness2-check"), *args])
    for args in (["lab"], ["jb"], ["wb"], ["wb", "--subset", "web"], ["lab", "--substrate", "cx"]):
        capture("setup-dry-run-" + "-".join(a for a in args if not a.startswith("--")), [str(VENV / "harness2-setup"), *args, "--dry-run"])

    subprocess.run(
        [str(VENV / "python"), "-I", str(HERE / "make_standard.py"), str(CHECKOUT), str(RAW / "std"), str(RAW / "std-stdout")],
        cwd=WORK,
        env=env(),
        check=True,
        timeout=900,
    )
    for p in sorted((RAW / "std-stdout").rglob("*")):
        if p.is_file():
            write(f"cli/std/{p.relative_to(RAW / 'std-stdout')}", p.read_text(encoding="utf-8"))
    subprocess.run(
        [str(VENV / "python"), "-I", str(HERE / "make_goldens.py"), str(OUT / "golden" / "expected.json")],
        cwd=WORK,
        env=env(),
        check=True,
        timeout=600,
    )

    prune_namespaces(RAW / "demo-k2" / "runs")
    prune_namespaces(RAW / "demo-k1-parallel" / "runs")
    for root in ("lab/adapt", "jb/adapt", "wb/office/adapt"):
        prune_namespaces(RAW / "std" / root)
    for exp in ("demo-k2", "demo-k1-parallel", "std"):
        shutil.copytree(RAW / exp, OUT / exp, symlinks=True)

    replacements = [
        (str(RAW / "std-stdout"), "/fixture/cli/std"),
        (str(RAW), "/fixture"),
        (str(HOME), "/fixture/home"),
        (str(WORK), "/fixture/work"),
        (str(CHECKOUT), "/fixture/harness2"),
    ]
    rewrite_tree(OUT, replacements)

    digests = {
        "demo-k2": digest_and_size(OUT / "demo-k2", ["runs"]),
        "demo-k1-parallel": digest_and_size(OUT / "demo-k1-parallel", ["runs"]),
        "std": digest_and_size(OUT / "std", ["jb/adapt", "lab/adapt", "wb/office/adapt", "wb/web/adapt", "wb/code/adapt"]),
    }
    write("golden/digests.json", json.dumps(digests, indent=1) + "\n")

    user = os.environ.get("USER", "") or Path.home().name
    forbidden = ["/home/", "/tmp/", "/Users/", str(Path.home())]
    if user:
        forbidden.append(user)
    bad = []
    for p in sorted(OUT.rglob("*")):
        rel = str(p.relative_to(OUT))
        if any(f in rel for f in forbidden):
            bad.append(rel)
        if p.is_file() and not (p.parent == HERE and p.suffix == ".py"):
            text = p.read_text(encoding="utf-8", errors="replace")
            if any(f in text for f in forbidden):
                bad.append(rel)
    if bad:
        sys.exit("machine-specific strings remain in: " + ", ".join(bad[:20]))
    print("fixtures written to", OUT)


if __name__ == "__main__":
    main()
