# h2s-core implementation notes

Notes from implementing `crates/h2s-core` against harness2 at
`1cfe254e8d25b15f37048c3730134686abe60335`. The doc comments in the source are the
interface; this file records what they rely on and where the port knowingly differs from
upstream.

## Byte-exact ports and how they are checked

| Port | Upstream | Checked against |
|---|---|---|
| `tree::validate`, `census` | `core/tree.py` + the two `SurfaceSpec`s | `tree.validate` on 13 trees covering every message (`tests/fixtures/golden/expected.json`) |
| `tree::fingerprint` | `core/runmark.fingerprint` | `runmark.fingerprint`, and every `harness_fingerprint` recorded in the demo fixtures |
| `report::summarise`, `render` | `core/report.py` | `summarise`/`render` on 6 row sets; `ExperimentDir::report().text` equals real `harness2-report` stdout (LAB `--k 1` and `--k 1,2`, seeded JobBench with mixed units, WorkBuddy) |
| `Namespace` | `core/schedule.ns`, `validate_tag` | `schedule.ns` |
| parsers | `cli/*.py` print statements | captured stdout in `tests/fixtures/cli/` |

Python semantics the ports reproduce (`src/pyfmt.rs`, `src/pyjson.rs`):

- `round(x, n)`: correctly rounded, ties to even on the binary value. Rust's `{:.n}` rounds
  the same way (checked on 6,030 values).
- `sum()` over floats: CPython 3.12 Neumaier compensation (`sum([0.1] * 10) == 1.0`).
- `statistics.mean`: exact rational mean, correctly rounded (a small big-integer helper).
- `str.isspace()` / `re` `\s`: Unicode `White_Space` plus U+001C..U+001F.
- `re.search(r"^name:\s*(\S+)\s*$", fm, re.M)` and friends: Python's backtracking, including
  `\s*` crossing newlines; `fm` is the first 600 code points.
- `json.loads` errors (`Expecting value: line 1 column 1 (char 0)`, the BOM error, dict
  semantics for repeated keys), because `.codex/hooks.json` problems quote them.
- Character counts are code points (Python `len`), and harness files are decoded like
  `Path.read_text(errors="replace")`, universal newlines included.
- Path order is `sorted(Path...)` order: component by component (`a/b` before `a-b/x`).

## Deliberate differences from upstream

- **No `compile()` check** of Python scripts in `tree::validate` (needs an interpreter;
  harness2 reports it itself).
- **Symlinks are never followed.** `read_tree` lists a symlink among the off-surface paths;
  `fingerprint` skips it (upstream would hash a linked file's target; harness2 never writes
  links into harness trees). Results browsing lists links as `FileKind::Symlink` and refuses
  to read through them; every path is walked one component at a time with an inode check.
- **`tree::validate` takes any file map**: it normalises paths like upstream `is_allowed`,
  reports off-surface files in `off_surface` (README stubs are ignored silently) and runs
  upstream's checks on the on-surface files, as harness2 would after reading them from disk.
- **An empty directory fingerprints to `da39a3ee5e6b4b0d`** (SHA-1 of nothing), as upstream
  does; only a missing directory is `absent` upstream, and a `Dir` handle always exists.
- **`render` orders NOTE/WARNING paragraphs by (mode, k)** whatever the input order, which is
  what `harness2-report` prints (its summaries arrive in that order); lines are (k, mode).
- Approximations that only matter for non-ASCII input: script references use Rust's Unicode
  `\w` (Python's excludes combining marks); `repr()` treats non-ASCII as printable except
  separators and a few format characters; a lone surrogate in `hooks.json` becomes U+FFFD.

## Results layout conventions

- Summaries for both modes live in the parallel (`agg`) namespace, also for a
  sequential-only sweep (`wb-office-oc-agg1` holds the summaries of `wb-office-oc-seq1`).
- Rollout run directories: base runs and the selected run come from the absolute paths in
  `summary.json` (mapped under the experiment root, else `Refused`); other labels follow the
  benchmark's convention for the run id `<mode namespace>/<task id>/<label>`:
  demo `executions/<run id>`; LAB `lab/results/<oc|cx>/<run id>`; JobBench
  `jb/dataset/<task id>/model_output/<run id with / as __>`; WorkBuddy
  `wb/<subset>/rollouts/<label_sanitize(run id with / as __)>` (a directory of timestamped
  runs, each holding trial directories).
- Task order: the newest `_sweeps/<ns>-<stamp>.json` record's `domain_order` and per-domain
  task order (`demo.json` for a demo), then the rest by task id.
- `report()` reports the modes that have judged rows at the sweep's `k`; with no rows it
  returns an empty report whose text is upstream's `nothing to report`.
- Judged rows are rebuilt like upstream `_load_rows`: `graded`/`frac` are recomputed from
  `n_passed`/`n_criteria`. JobBench's weighted judging writes float points (7.5 of 10.0),
  which the `u32` wire fields cannot hold: those become `None` while `frac` stays exact. The
  raw floats are kept in `results::Points`, so optional `points_*` wire fields (planned by
  the coordinator) are a one-line change in `judged_row`/`base_row`.
- `Comparison` values are fractions 0..1 (contract v1.1); the sign test is the exact
  two-sided binomial test over wins and losses, ties dropped.

## Job environments (`env`)

Verified by the coordinator in the pinned sources:

- OpenCode 1.14.18 merges the file named by `OPENCODE_CONFIG` and bundles
  `@ai-sdk/openai-compatible`, so the generated provider `h2s` works offline and in
  containers. Every OpenCode process harness2 starts (LAB and JobBench runners, the improver)
  inherits the environment, so `OPENCODE_CONFIG` and `H2S_PROVIDER_API_KEY` reach them all.
- JobBench's judge reads `HARNESS2_JUDGE_BASE_URL`/`HARNESS2_JUDGE_API_KEY`. For an
  `openai_compatible` profile with a judge model and no judge endpoint, the profile's own
  endpoint is used.
- LAB's judge only accepts model ids starting with `claude`, `gemini`, `gpt`, `o1`, `o3`,
  `o4`, `o5`, `mistral` or `vertex/`, and sends `gpt*`/`o*` ids to the official OpenAI
  client, which reads `OPENAI_BASE_URL`/`OPENAI_API_KEY`: `build` sets both (values are
  secrets for the redactor) when the judge model is such an id.
- WorkBuddy's setup requires a `google-vertex/gemini...` executor, a `gemini...` judge and
  `GOOGLE_CLOUD_PROJECT`, so `openai_compatible` profiles cannot drive WorkBuddy.
- `profile_warnings` reports these limits (and missing executor/judge models and a missing
  Codex endpoint) without failing `validate_profile`.

## Other choices

- `doctor::evaluate` flags a `.env` in the harness2 checkout (`ctx.harness2.home`), because
  harness2's `config.load_dotenv` supplies every variable a job does not set. It checks the
  file exists and never reads it. ADC is reported as not verified: checking it would print an
  access token.
- `install::plan` verifies the checkout with `git merge-base --is-ancestor <commit> HEAD`
  (exit status only); in the depth-1 clone that means HEAD is the commit. A reinstall
  recreates the venv with `uv venv --clear` (uv 0.8 or newer).
- `isolation::run_argv` refuses the `latest` tag, the `host` network, uid 0 and a writable
  job-files mount, on top of the documented checks.
- `redact::Redactor` also redacts a secret's JSON-escaped form, each line of a multi-line
  value and each long string inside a JSON value (log lines are split, and a service-account
  key's `private_key` can appear on its own).
- serde_json parses floats with `float_roundtrip` (enabled workspace-wide by the
  coordinator), so values read from harness2's JSON equal Python's.

## Fixtures

See `tests/fixtures/README.md`: which files are real harness2 output, which were
synthesized by running upstream's own CLI code with a local adapter, and how to regenerate
them.
