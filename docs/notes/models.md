# Notes: running harness2 on OpenAI-compatible (local or self-hosted) models

Coordinator notes for the user guide. Status of each claim is given explicitly.

harness2's documentation only describes Google Vertex AI. These facts, read from the pinned
sources, show where an OpenAI-compatible endpoint (LiteLLM, vLLM, SGLang, Ollama, a gateway) can
replace Vertex, and how a Harness² Studio `openai_compatible` profile wires it.

| Part | Where it reads its model settings | OpenAI-compatible? | Status |
|---|---|---|---|
| Improver / selector sessions | `harness2/core/proposer.py` runs OpenCode with `--model <provider/model>`; env copied from `os.environ` | yes, via a custom OpenCode provider (`OPENCODE_CONFIG`) | verified by reading source |
| Executor, LAB | `third_party/lab_runner/opencode/run_opencode.py` `build_env()` copies `os.environ`; writes a project `opencode.json` without providers | yes, same provider | verified by reading source |
| Executor, JobBench | `harness2/benchmarks/jb_opencode_runner.py` passes `{**os.environ, ...}` | yes, same provider | verified by reading source |
| Executor, WorkBuddy | Harbor containers; `harness2/install.py::workbuddy_models` refuses anything but `google-vertex/gemini...` | **no** (upstream restriction) | verified by reading source |
| Executor, Codex (any bench) | `HARNESS2_CODEX_BASE_URL` / `_API_KEY` / `_MODEL`, a Responses-API endpoint | yes if the endpoint speaks the Responses API | upstream docs |
| Judge, JobBench | `HARNESS2_JUDGE_BASE_URL` / `HARNESS2_JUDGE_API_KEY` | yes | upstream docs |
| Judge, LAB | harvey-labs `evaluation/judge.py` `_detect_provider()`: `gpt*`/`o1`/`o3`/`o4`/`o5` → `openai.OpenAI()` (reads `OPENAI_BASE_URL`, `OPENAI_API_KEY`) | yes, if the judge model id starts with `gpt` or `o1`/`o3`/`o4`/`o5` (e.g. a gateway alias `gpt-judge`) | verified by reading source |
| Judge, WorkBuddy | generated Vertex route | no | verified by reading source |

## OpenCode facts (pinned commit `23fb5e05`, v1.14.18)

- `OPENCODE_CONFIG` is honoured and merged into the configuration (`packages/opencode/src/config/config.ts`).
- `@ai-sdk/openai-compatible` 2.0.41 is a dependency and is in `BUNDLED_PROVIDERS`
  (`packages/opencode/src/provider/provider.ts`), so no package is installed at run time — it
  works offline and inside network-restricted containers.

## Open questions (need a real model-backed run to settle)

- The LAB runner adds Gemini-specific `thinkingConfig` agent options when a reasoning effort is
  set; whether a given OpenAI-compatible endpoint accepts or drops them is untested.
- Tool-call fidelity of local models inside OpenCode (multi-step tool use over long tasks).
- Structured-output support of the judge endpoint (LAB's judge asks for a JSON schema).
