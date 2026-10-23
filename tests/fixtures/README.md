# Test fixtures

Everything here was produced by **harness2 itself** at commit
`1cfe254e8d25b15f37048c3730134686abe60335` (Python 3.12), offline: no model, no network, no
benchmark checkout. `golden/regenerate.py` rebuilds the whole directory (see below). The
`h2s-core` tests read these files; nothing here is executed.

Every absolute path harness2 wrote was rewritten to a neutral prefix:

| Prefix in the files | Was |
|---|---|
| `/fixture/<name>` | the experiment directory `<name>` (`demo-k2`, `demo-k1-parallel`, `std`) |
| `/fixture/cli/std` | the directory the STANDARD commands' outputs were written to |
| `/fixture/harness2` | the harness2 checkout |
| `/fixture/work`, `/fixture/home` | the scratch `HARNESS2_RUNS_DIR` and `HOME` of the CLI captures |

Tests open the experiments with the matching `/fixture/<name>` as an extra root, exactly as
the server maps a container mount point.

## Real captured output

| Path | Command |
|---|---|
| `demo-k2/` | `harness2-demo --output <dir> --k 2 --mode both` |
| `demo-k1-parallel/` | `harness2-demo --output <dir> --k 1 --mode parallel` |
| `cli/demo-*.{stdout,stderr,exit}` | the same two commands' output |
| `cli/check-*.{stdout,stderr,exit}` | `harness2-check --bench lab`, `--bench jb`, `--bench wb --subset office`, `--bench lab --substrate cx` on an uninstalled checkout (every check fails) |
| `cli/setup-dry-run-*.{stdout,stderr,exit}` | `harness2-setup lab|jb|wb|wb --subset web|lab --substrate cx --dry-run` |

The demo experiments are pruned to what the reader uses: per task directory `summary.json`,
`summary.lock`, every `harnesses/<label>/` tree (complete, so fingerprints still match the
recorded `harness_fingerprint`), `selection/<mode><k>/selection.json`, and one
`context/<label>/task.md` in the parallel namespace. The improver evidence copies
(`context/`, `reflection/`, selection candidates) are dropped. `executions/`, `library/`,
`base/`, `sources/` and `demo.json` are kept whole.

## Synthesized with upstream code

These formats need models or installed benchmarks, so they were produced by running
upstream's **own, unmodified** CLI modules with their outside world replaced
(`golden/make_standard.py`):

| Path | How |
|---|---|
| `std/` | A STANDARD experiment: `harness2.cli.run_sweep.main`, `harness2.cli.judge.main` and `harness2.cli.report.main` run with a deterministic local adapter (harness2-demo's ledger task and script editor under the names and run-directory conventions of LAB, JobBench and WorkBuddy-office), harness2's `DemoProposer` extended to write the improver audit trail and a fixed cost per session, and a judge that checks the answer file (LAB: 4 criteria; JobBench: weighted float points plus one unweighted-fallback row; WorkBuddy: a 0..1 score as 100 points). One LAB task has an unparseable ledger (status `base-failed`), one stage proposes an invalid tree twice (falls back), one judge call raises (ungraded row). Layout: `lab/adapt/{lab-oc-agg1,lab-oc-seq1,lab-oc-agg2,_sweeps,_audit}`, `lab/results/oc/...`, `lab/library/lab/{k1-oc,k2-oc}`, `jb/adapt/jb-oc-agg1-s3`, `jb/dataset/<task>/model_output/<run id>`, `wb/office/adapt/{wb-office-oc-agg1,wb-office-oc-seq1}`, `wb/office/rollouts/<label>/<stamp>/<trial>`. Pruned like the demo. |
| `cli/std/*.stdout`, `*.exit` | The captured stdout of those commands: `--dry-run` previews, sweep progress and tails, `harness2-judge` (selected and `--bases`), `harness2-report` (`--k 1`, `--k 1,2`, seeded JobBench, WorkBuddy). |
| `cli/std/splits/lab-half.json`, `cli/std/split-lab.stdout` | `scripts/split_tasks.py` with its task discovery answered by the local adapter. |
| `golden/expected.json` | `golden/make_goldens.py`: upstream `tree.validate` on 13 trees (every validation message), `report.summarise`/`render` on 6 row sets (incl. 240 random rows), `runmark.fingerprint`, `schedule.ns`, and CPython's `json.loads` errors, `sum`, `statistics.mean`, `round`, `repr` and the frontmatter/script-reference regexes. Floats travel as `repr()` strings. |
| `golden/digests.json` | `ExperimentDir::results_digest` and `size_bytes` for each experiment, computed in Python over the final files. |

Not covered by a fixture file: the `harness2-bridge` banner (it starts LiteLLM on Vertex AI);
`parse::bridge_line` is tested on lines written from the format in `harness2/cli/bridge.py`.

## Regenerating

From a harness2 checkout at the commit above with its `.venv` installed:

```bash
<harness2>/.venv/bin/python -I tests/fixtures/golden/regenerate.py \
    <harness2 checkout> <new empty scratch dir> tests/fixtures
```

The script refuses a non-empty scratch directory, rewrites the paths, and fails if a home
directory, `/tmp/` path or the user's name is left in any file. Between runs only these
change: sweep-record names and timestamps, audit session keys (harness2 hashes the absolute
context path into them) and file modification times.
