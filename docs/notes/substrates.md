# Notes: the Hermes Agent and Turnstone executors

Notes behind the executor sections of the user guide. Status of each claim is given explicitly.

harness2 calls the agent framework that runs rollouts a *substrate*; the Workbench calls it the
*executor*. Upstream harness2 at the commit this release tests (`1cfe254`) has two: OpenCode (`oc`)
and Codex (`cx`). A harness2 that also ships **Hermes Agent** (`hm`, Nous Research,
<https://github.com/NousResearch/hermes-agent>) and **Turnstone** (`ts`,
<https://github.com/turnstonelabs/turnstone>) as LAB executors can be driven by the Workbench the
same way. The Workbench offers the two only when the installed harness2 ships them, and changes
nothing for a harness2 that does not.

## What the Workbench needs from harness2

| harness2 provides | The Workbench uses it for | Status |
|---|---|---|
| `harness2/substrate/hermes.py` / `turnstone.py` in the checkout | detection: `harness2.substrates` in `GET /api/v1/system/info` lists `hm` / `ts` (`h2w_core::install::read_status`) | unit-tested; verified live |
| `--substrate hm` / `ts` on `harness2`, `harness2-check`, `harness2-setup lab`, `harness2-judge`, `harness2-report` | the same job kinds as for `oc` and `cx` | verified live (see below) |
| `HARNESS2_HERMES_{BASE_URL,API_KEY,MODEL}` and `HARNESS2_TURNSTONE_{BASE_URL,API_KEY,MODEL}` | set from an `openai_compatible` profile's endpoint, for jobs on that executor only ([MODELS.md](../MODELS.md)) | unit-tested; verified live |
| namespaces `lab-hm-...` / `lab-ts-...`, results under `results/hm` / `results/ts` | sweep lists, task pages, scores, comparisons | unit-tested; verified live |
| one session stream per rollout: `hermes_session.jsonl` / `turnstone_session.jsonl` | the rollout browser | verified live |
| the trees: Hermes `SOUL.md`, `memories/MEMORY.md`, `skills/`, `scripts/`; Turnstone `systemprompt.md`, `guardrails.md`, `skills/`, `scripts/` | the harness viewer, diffs, the validator, export ([CONCEPTS.md](../CONCEPTS.md)) | unit-tested against harness2's own messages |

Rules that come with them, all enforced before a job is created:

- **LAB only.** A cell with `hm` or `ts` on JobBench or WorkBuddy-Bench is refused (`422`), and so
  is a `setup` for those benchmarks.
- **The installed harness2 must ship the executor.** Otherwise `422` on `params.cell.substrate`
  (or `params.substrate` for `setup`).
- **An OpenAI-compatible endpoint.** Both frameworks call a Chat Completions endpoint directly, so a
  `run` or `pipeline` needs an `openai_compatible` profile, or the executor's `..._BASE_URL` in the
  profile's `extra_env` (`422` on `params.profile`).
- **The improver stays on OpenCode** for every executor (harness2's choice), so a profile still needs
  what OpenCode needs.

## What the Workbench adds

- **Validation in harness2's words.** `POST /api/v1/harness/validate` (and the MCP tool
  `validate_harness`) runs harness2's tree rules with each framework's caps (Hermes 20,000
  characters for `SOUL.md` and `MEMORY.md` together, Turnstone 24,000), then the framework's own
  static refusals: an empty `SOUL.md` or persona; a persona or loaded skill body over 32,768
  characters; Turnstone's skill-name rule, closed-frontmatter rule and `$1`/`$ARGUMENTS`
  placeholders; Hermes skill helpers named by a workspace path, or reached through
  `${HERMES_SKILL_DIR}` but not shipped. The messages were compared with the ones harness2 prints for
  the same trees (`tree::tests::substrate_refusals_match_harness2`). Not ported: Hermes Agent's
  prompt-injection scanner and Turnstone's skill risk scanner, which harness2 loads from each
  framework's install; the report's `notes` say so.
- **Export between all five layouts** (raw, OpenCode, Codex, Hermes Agent home, Turnstone bundle),
  moving the always-on text by role and skill-helper paths between `${HERMES_SKILL_DIR}` and
  project paths, and refusing what a target has nothing for ([USER-GUIDE.md](../USER-GUIDE.md#64-export)).
  Unit-tested in both directions for every pair; not imported into a running Hermes Agent or
  Turnstone.
- **Job logs repeat only the profile warnings about that job** (a LAB run on Hermes Agent does not
  repeat the Codex or WorkBuddy-Bench lines).

## Verification

- **Tests** (in `cargo xtask check`): detection in a checkout
  (`install::tests::substrates_follow_the_checkout`); `lab-hm-…` / `lab-ts-…` namespaces; endpoint
  variables only for jobs on that executor
  (`env::tests::hermes_and_turnstone_runs_get_the_endpoint_and_nothing_else_does`); the job
  refusals (`hermes_and_turnstone_need_lab_a_harness2_that_has_them_and_an_endpoint` in
  `crates/h2w-daemon/tests/jobs.rs`); validation messages compared with the ones harness2 prints
  for the same trees (`tree::tests::hermes_and_turnstone_messages_match_harness2`,
  `tree::tests::substrate_refusals_match_harness2`); export between every pair of frameworks
  (`export::tests::every_framework_exports_to_every_layout` and its neighbours); per-job warnings
  (`env::tests::a_job_hears_only_the_warnings_about_its_benchmark_and_executor`).
- **Live, by the maintainers, 2026-10-27.** A Workbench built from this change, in container
  isolation (rootless Docker, an internal jobs network, the allowlisting egress proxy), with a
  harness2 checkout that adds both executors and an OpenAI-compatible profile (GLM-5.3-Flash
  through a LiteLLM gateway, as executor and improver):
  - `setup` for LAB with `substrate` `hm`, then `ts`, installed each framework at harness2's pin
    through the proxy in setup mode;
  - with the proxy back to the gateway only, one LAB task
    (`employment-labor/identify-issues-in-counterparty-discovery-responses`, k = 1, parallel mode,
    seed 42) ran on each executor: 4 rollouts and 3 improver sessions each, `1 tasks ok, 0 failed`,
    in 62 minutes (Hermes Agent) and 59 minutes (Turnstone), both adopting the composed harness
    `agg1`;
  - harness2's Turnstone checks sent the improver's first domain proposal back (its skill body
    held dollar amounts such as `$206`, which Turnstone would have filled with nothing when loading
    the skill), and the retry passed: the feedback the Workbench's validator now reports too;
  - the sweeps, task pages, harness trees (Hermes Agent: `SOUL.md` and a skill with two helper
    scripts; Turnstone: a persona and a script), the validator and all five export layouts were
    read through the API and the web UI; the export dialog opened on each harness's own framework;
  - the proxy allowed only the gateway during the runs; its only refusals were the Hermes agent's
    own attempts to install a Python package from PyPI.
- **Not verified:** judging these sweeps (no judge job was run), a `pipeline` job, host isolation,
  importing an exported bundle into a running Hermes Agent or Turnstone, and any harness2 checkout
  other than the one above.
