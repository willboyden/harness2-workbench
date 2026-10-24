# Model profiles: Vertex AI, OpenAI-compatible endpoints, and what works with what

A harness2 sweep calls language models in four places: the **executor** (the agent being
improved), the **improver** (which edits harnesses and selects), the **judge** (which grades
results) and, for Codex, a **Responses-API endpoint**. A Workbench **model profile** is the named
bundle of settings that tells every job which models to use and how to reach them. This page
covers how profiles are written, which benchmark and executor each kind can drive, the judge
rules that decide whether a local model can grade, and what a sweep costs.

> **Read this first.** harness2's own documentation only describes Google Vertex AI. Running it
> on OpenAI-compatible servers (LiteLLM, vLLM, SGLang, Ollama, a hosted gateway) rests on facts
> read from the pinned upstream sources and on the Workbench's own tests with fakes. **As far as the
> repository's tests and notes record, no model-backed sweep has been run through the Workbench, on
> either kind of profile**: the only real harness2 workflow exercised end to end is the offline
> demo. The compatibility table below says, row by row, how each fact was established.

Terms: [CONCEPTS.md](CONCEPTS.md). Administering profiles and secrets: [ADMIN-GUIDE.md](ADMIN-GUIDE.md).

## 1. What a profile contains

A profile is stored in the Workbench's database (`PUT /api/v1/profiles/{name}`, admin role) and
holds **no secret values**: keys are referenced by *secret name* and stored separately,
write-only (`PUT /api/v1/secrets/{name}`), or supplied to the server as `H2W_SECRET_<NAME>`
environment variables.

| Field | Meaning |
|---|---|
| `name` | `^[a-z0-9][a-z0-9_-]{0,62}$` |
| `kind` | `vertex`, `openai_compatible` or `custom` (below) |
| `description` | free text, up to 2000 characters |
| `solver_model` | the executor model: `provider/model` for `vertex`; the **bare** model id for `openai_compatible` (the server adds the provider prefix `h2w/`) |
| `improver_model` | optional; defaults to the solver model |
| `judge_model` | optional bare id (`HARNESS2_JUDGE_MODEL`) |
| `codex_model` | optional, for the Codex executor |
| `vertex` | `project`, `location` (default `global`), optional `credentials_secret` |
| `endpoint` | `base_url` and optional `api_key_secret` of the OpenAI-compatible server |
| `judge_endpoint` | separate judge server; for `openai_compatible` profiles with a judge model it defaults to `endpoint` |
| `codex_endpoint` | a Responses-API endpoint for the Codex executor |
| `extra_env` | more variables: each a literal `value` **or** a `secret` name |
| `prices` | USD per rollout and per improver session, low and high, for cost estimates |
| `egress_hosts` | hosts the profile's jobs reach (informational; see section 6) |

An experiment is **pinned** to the profile of its first writing job; later runs, judging and
reports must name the same profile (a different one is refused with 409, because harness2
refuses to resume or judge a sweep under different models). A profile cannot be deleted while an
experiment is pinned to it.

### What each kind puts into a job's environment

Jobs start from an empty environment (see [ADMIN-GUIDE.md](ADMIN-GUIDE.md)); these are the
profile-derived variables added to it. `HARNESS2_RUNS_DIR` is always set to the experiment.

| Variable | `vertex` | `openai_compatible` | From |
|---|---|---|---|
| `HARNESS2_SOLVER_MODEL` | as written | `h2w/<solver_model>` | `solver_model` |
| `HARNESS2_IMPROVER_MODEL` | improver, else solver | `h2w/<improver or solver>` | `improver_model` |
| `HARNESS2_JUDGE_MODEL` | as written | as written | `judge_model` |
| `GOOGLE_CLOUD_PROJECT`, `GOOGLE_CLOUD_LOCATION` | set | set when `vertex` is given (a judge on Vertex) | `vertex` |
| `GOOGLE_APPLICATION_CREDENTIALS` | path of a per-job 0600 key file | same | `vertex.credentials_secret` |
| `OPENCODE_CONFIG` | n/a | path of a generated config registering provider `h2w` (`@ai-sdk/openai-compatible`, `baseURL`, `apiKey` as `{env:H2W_PROVIDER_API_KEY}`) | `endpoint` |
| `H2W_PROVIDER_API_KEY` | n/a | the key | `endpoint.api_key_secret` |
| `HARNESS2_JUDGE_BASE_URL`, `HARNESS2_JUDGE_API_KEY` | when `judge_endpoint` is given | `judge_endpoint`, else `endpoint` | `judge_endpoint` |
| `OPENAI_BASE_URL`, `OPENAI_API_KEY` | when a `judge_endpoint` is given and the judge model starts with `gpt`, `o1`, `o3`, `o4` or `o5` | when the judge model starts with `gpt`, `o1`, `o3`, `o4` or `o5` | the judge endpoint |
| `HARNESS2_CODEX_BASE_URL`, `_API_KEY`, `HARNESS2_CODEX_MODEL` | from `codex_endpoint` / `codex_model` | same | `codex_endpoint` |

A `custom` profile sets nothing by itself: only its `extra_env`.

### Reserved environment variables

Names that control the loader, an interpreter or the shell, the server, the container runtime,
which certificates jobs trust, or credentials the server manages are **refused** in `extra_env`
(`422` when the profile is saved). Exact names: `PATH`, `HOME`, `SHELL`, `IFS`, `ENV`, `BASH_ENV`,
`SHELLOPTS`, `BASHOPTS`, `PS4`, `GCONV_PATH`, `GLIBC_TUNABLES`, `NODE_OPTIONS`, `NODE_PATH`,
`NODE_TLS_REJECT_UNAUTHORIZED`, `JAVA_TOOL_OPTIONS`, `JDK_JAVA_OPTIONS`, `_JAVA_OPTIONS`, `RUBYOPT`,
`RUBYLIB`, `PERL5OPT`, `PERL5LIB`, `PERLLIB`, `OPENCODE_CONFIG`, `OPENCODE_CONFIG_CONTENT`,
`OPENCODE_PERMISSION`, `HARNESS2_RUNS_DIR`, `GOOGLE_APPLICATION_CREDENTIALS`, `DOCKER_HOST`,
`SSH_AUTH_SOCK`, `CONTAINER_HOST`, `REGISTRY_AUTH_FILE`, the proxy variables (`HTTPS_PROXY`,
`HTTP_PROXY`, `ALL_PROXY`, `NO_PROXY`) and the certificate variables (`SSL_CERT_FILE`,
`SSL_CERT_DIR`, `REQUESTS_CA_BUNDLE`, `NODE_EXTRA_CA_CERTS`, `CURL_CA_BUNDLE`). Prefixes: any name
starting with `LD_`, `DYLD_`, `PYTHON`, `H2W_`, `UV_`, `PIP_`, `BUN_`, `NPM_CONFIG_`, `DOCKER_`,
`CONTAINERS_`, `XDG_`, `GIT_` or `OPENCODE_`. The source is `crates/h2w-core/src/env.rs`.

The list is a safety net for mistakes, not a security boundary: other variables that change what a
program does are accepted (`CC`, `MAKEFLAGS` or `CARGO_HOME`, for example), and only an admin can write a
profile, so treat the admin role accordingly. A profile that was stored before a name was reserved
is not rewritten: creating a job with it is refused (`422`, field `params.profile`, `profile <name>
no longer passes validation (...); an administrator must update it`); save the profile again
without that variable.

In container mode a job's variables, profile variables and secrets included, reach the container
through a 0600 environment file in the job's directory (one `NAME=value` per line, deleted with the
job), not through the container-runtime command's command line or environment. A value that
contains a line break or NUL cannot be written that way and fails the job (`the value of <NAME>
contains a line break or NUL, ...`); a line (`NAME=value`) is limited to 64 KiB.

## 2. Which benchmark and executor work with which profile kind

Status words: **source-read** means the fact was read from the pinned upstream source (harness2
`1cfe254`, OpenCode `23fb5e05` / v1.14.18); **upstream docs** means upstream's documentation says
so; **fakes only** means the Workbench's wiring is covered by tests with stub executables, not by a
real model call. Nothing in this table has been exercised against a real model through the
Workbench.

| Part of harness2 | Reads its model settings from | `vertex` profile | `openai_compatible` profile | Basis |
|---|---|---|---|---|
| Improver and selector sessions | OpenCode `--model provider/model`, environment copied from the server's job | yes | yes, through the generated provider `h2w` | source-read |
| Executor, LAB (`oc`) | OpenCode, environment copied | yes | yes, same provider | source-read |
| Executor, JobBench (`oc`) | OpenCode, `{**os.environ, ...}` | yes | yes, same provider | source-read |
| Executor, WorkBuddy (`oc`) | Harbor containers; setup refuses anything but `google-vertex/gemini...` | yes | **no** (upstream restriction) | source-read |
| Executor, Codex (`cx`), any benchmark | `HARNESS2_CODEX_BASE_URL` / `_API_KEY` / `_MODEL` | yes, with a Responses-API endpoint (`harness2-bridge` provides one on Vertex AI) | yes if the endpoint speaks the Responses API | upstream docs |
| Judge, JobBench | `HARNESS2_JUDGE_BASE_URL` / `HARNESS2_JUDGE_API_KEY`, or Vertex with a Gemini judge | yes | yes (judge endpoint and key) | upstream docs |
| Judge, LAB | `evaluation/judge.py` `_detect_provider()` by model-id prefix | yes (`gemini...`, `claude...`, `vertex/...`) | only through ids starting `gpt` or `o1`, `o3`, `o4`, `o5` (see below) | source-read |
| Judge, WorkBuddy | generated Vertex route | yes (`gemini...`) | **no** | source-read |

OpenCode merges the file named by `OPENCODE_CONFIG` and bundles `@ai-sdk/openai-compatible`
(2.0.41), so the generated provider needs no package download. That is why it works offline and
inside network-restricted containers (source-read).

### The judge rules that surprise people

- **LAB accepts only these judge model ids**: those starting with `claude`, `gemini`, `gpt`,
  `o1`, `o3`, `o4`, `o5`, `mistral` or `vertex/`. It sends `gpt*` and `o*` ids to the official
  OpenAI client, which reads `OPENAI_BASE_URL` and `OPENAI_API_KEY`; the Workbench sets both from the
  judge endpoint when the judge model is such an id. For a local judge on LAB, give the gateway a
  model *alias* that starts with `gpt` (for example `gpt-judge`) and use that as `judge_model`.
- **JobBench judging needs a judge endpoint with a key, or Vertex.** With a Gemini judge on
  Vertex it uses the project's Vertex OpenAI-compatible endpoint and Application Default
  Credentials. For any other judge it needs `HARNESS2_JUDGE_BASE_URL` and
  `HARNESS2_JUDGE_API_KEY`, so an Ollama endpoint that "needs no key" still needs a key *value*
  stored under a secret name (any placeholder string works for servers that ignore the key).
- **WorkBuddy is Vertex-only**: `setup` requires a `google-vertex/gemini...` executor, a bare
  `gemini...` judge and `GOOGLE_CLOUD_PROJECT`. WorkBuddy also needs Docker on the host, and the
  Workbench refuses WorkBuddy and the Codex bridge under container isolation in this version
  (409; [ADMIN-GUIDE.md](ADMIN-GUIDE.md)).
- **Changing the judge model changes the measurement.** Keep the judge fixed when you compare
  harnesses or budgets.

### Warnings the server raises

Saving a profile never fails because of these limits; the server lists them instead. They are
written to the server log and recorded in the **audit record** of the save (`profile.put`,
`detail: warnings: ...`), and the first lines of every job that uses the profile repeat them. They
are *not* part of the `PUT` response. To read them on demand, ask the server:
`GET /api/v1/profiles/{name}/check` (viewer role; MCP tool `check_profile`; the web UI shows the same
after a profile is saved and in the run wizard) returns `{"warnings": [...], "missing_secrets":
[...]}`, where `missing_secrets` names the referenced secrets that have no value yet (a job with
such a profile is refused with `422` until they are set). Run it after every profile change.
Some warnings are capability notes rather than faults: an `openai_compatible` profile always lists
the WorkBuddy and Codex lines below, because those need Vertex AI settings or a Responses-API
endpoint that the profile does not have. Exact texts:

- `No executor model: harness2 refuses to run without HARNESS2_SOLVER_MODEL.`
- `The executor model "X" is not in OpenCode's provider/model form, which harness2 requires.`
- `No judge model: harness2 starts no sweep and judges nothing without HARNESS2_JUDGE_MODEL.`
- `LAB's judge does not recognise the judge model "X" (it accepts ids starting with claude,
  gemini, gpt, o1, o3, o4, o5, mistral or vertex/), so this profile cannot judge LAB.`
- `JobBench judging needs a judge endpoint or Vertex AI settings (harness2 reads
  HARNESS2_JUDGE_BASE_URL or GOOGLE_CLOUD_PROJECT).`
- `JobBench needs a judge endpoint with an API key for a non-Gemini judge
  (HARNESS2_JUDGE_BASE_URL and HARNESS2_JUDGE_API_KEY).`
- `WorkBuddy-Bench cannot use this profile: its setup requires a google-vertex/gemini executor,
  a gemini judge and Vertex AI settings.`
- `The Codex executor (substrate cx) needs a Responses-API endpoint: set the profile's Codex
  endpoint (for Vertex AI, run the bridge job and use its URL and key).`

## 3. Examples

All three were saved against a running server (`PUT /api/v1/profiles/{name}` returned 200) with
the commands below. That proves the profiles validate; it does not prove a sweep runs on them.
Model ids, hosts and project names are placeholders: use the ids your provider or gateway serves.
`BASE` is the server URL and `AUTH` an admin token header.

```bash
BASE=http://127.0.0.1:8722
AUTH="Authorization: Bearer $TOKEN"   # an admin token; see ADMIN-GUIDE.md
put() { curl -s -X PUT -H "$AUTH" -H 'x-h2w-request: 1' -H 'content-type: application/json' \
          -d "$2" "$BASE/api/v1/profiles/$1"; }
```

**Vertex AI** (Application Default Credentials of the server user; add `credentials_secret` to
use a service-account key stored as a secret instead):

```bash
put vertex-flash '{
  "name": "vertex-flash", "kind": "vertex",
  "solver_model": "google-vertex/gemini-3.5-flash",
  "judge_model": "gemini-3.5-flash",
  "vertex": { "project": "my-gcp-project", "location": "global" }
}'
```

**A self-hosted server behind an OpenAI-compatible API** (vLLM here; LiteLLM and SGLang look the
same). The key is referenced by name and set separately; the model id is bare:

```bash
put local-vllm '{
  "name": "local-vllm", "kind": "openai_compatible",
  "solver_model": "qwen3-coder",
  "judge_model": "gpt-judge",
  "endpoint": { "base_url": "http://vllm.internal.example:8000/v1",
                "api_key_secret": "VLLM_API_KEY" },
  "egress_hosts": ["vllm.internal.example"]
}'
curl -s -X PUT -H "$AUTH" -H 'x-h2w-request: 1' -H 'content-type: application/json' \
     -d '{"value": "the-real-key"}' "$BASE/api/v1/secrets/VLLM_API_KEY"
```

The response lists the secret as `{"name":"VLLM_API_KEY","set":false}` until the value is set, and
a job that needs it is refused with `422` `profile local-vllm needs secret VLLM_API_KEY, which is
not set`. `GET /api/v1/profiles/local-vllm/check` shows `"missing_secrets":["VLLM_API_KEY"]` until
then, and an empty list after the `PUT` above (checked live).

**Ollama on the same machine** (its OpenAI-compatible endpoint is `/v1`; prices of zero tell the
estimator that rollouts cost nothing):

```bash
put ollama-dev '{
  "name": "ollama-dev", "kind": "openai_compatible",
  "solver_model": "qwen3:32b", "judge_model": "gpt-judge",
  "endpoint": { "base_url": "http://127.0.0.1:11434/v1" },
  "prices": { "rollout_usd_low": 0, "rollout_usd_high": 0,
              "session_usd_low": 0, "session_usd_high": 0 }
}'
```

`127.0.0.1` in a profile means the host that runs the **job**. When the Workbench itself runs in a
container, or jobs run in containers, that is the container, not your workstation; use a name the
job's network can resolve and add it to the egress allowlist (section 6).

Validation errors name the field. Real examples (`422`): `a vertex profile needs Vertex settings`;
`use OpenCode's provider/model form, e.g. google-vertex/gemini-3.5-flash`; `use the bare model id;
the server adds h2w/`; `names starting with LD_ are reserved and cannot be set by a profile`;
`HTTPS_PROXY is managed by the server and cannot be set by a profile`; `set exactly one of value or
secret`. The same wording covers `GIT_*`, `DOCKER_*`, `XDG_*` and the other reserved names.

## 4. Vertex AI credentials

harness2 uses Application Default Credentials. Either way, the Google Cloud CLI must be on the
server's `PATH`: `harness2-check` runs `gcloud auth application-default print-access-token`
(upstream's setup guide says so even for the key-file route).

| Method | How |
|---|---|
| ADC of the server user | `gcloud auth application-default login` as the user that runs `h2workbench serve`; `--no-browser` on a headless host |
| Service-account key | store the key's JSON as a secret, set `vertex.credentials_secret` to its name; each job gets it as a 0600 file in its private job directory, deleted when the job ends |
| Attached service account | on a cloud VM with one attached, ADC picks it up |

`h2workbench doctor` shows a `gcloud` check (it fails if a Vertex profile exists and `gcloud` is
missing; it is skipped when no profile uses Vertex) and, while a Vertex profile exists, an `adc`
check that is always a **warning**: the Workbench does not verify ADC itself because the check
would print an access token. `harness2-check` verifies it before every run.

## 5. Cost

| Source of cost | Counted by the Workbench's estimate? |
|---|---|
| Executor rollouts: `T * (k + M * 3k)` | counted, in numbers; in dollars only with `prices` |
| Improver sessions: `T * M * (3k + [k>1] + 1)` | counted; dollars from `prices` or history |
| Judging (one call per selected run, and per reference run with `bases`) | **not included** |
| Retries (a rejected proposal gets one retry; a failed improver call one more attempt) | **not included** |
| Resumed sweeps | reuse finished rollouts and cached sessions at no cost |

`POST /api/v1/estimate` returns the counts and, when it can, a USD range with its `basis`:
`heuristic` (from the profile's `prices`) or `history` (the mean recorded spend per task and
mode of this experiment's earlier runs, widened when fewer than 10 records exist). Without prices
or history there is no dollar figure, by design.

**The budget guard sees improver spend only.** `budget_usd` on `run` and `pipeline` stops the job
when the spend recorded in task summaries reaches the cap; a task in flight finishes first.
harness2 records improver spend (editing, composing, selection, reflection) and does not record
executor rollout spend, so a sweep whose cost is mostly rollouts can exceed any figure the guard
watches. Judging is a separate job with no guard.

**Self-hosted models may record zero.** harness2 sums the `cost` OpenCode reports in each
improver session. The provider the Workbench generates for an `openai_compatible` profile declares
no model prices, so OpenCode has nothing to multiply and the recorded cost is most likely 0
(inferred from upstream's `_opencode_cost` and the generated configuration; not observed in a
live run). If so, `budget_usd` never trips and `history` estimates read $0. Use wall-clock limits
(`jobs.timeout_hours`) and the job queue limits to bound a self-hosted sweep, and treat GPU time
as the real cost.

A full LAB stream is 1,143 tasks (upstream); start with `only_domains` and `max_tasks` (a cap
**per domain**), and run a `preview` job first: it makes no model calls and returns the task
stream, the namespaces and the estimate.

## 6. Egress hosts

`egress_hosts` is documentation for the administrator: **the server does not apply it**. In
container isolation and in the compose deployment the only list that matters is `[egress] allow`
in the server's configuration, and every host the profile's endpoints use must be added there by
hand ([ADMIN-GUIDE.md](ADMIN-GUIDE.md)):

- a public host such as `aiplatform.googleapis.com` is reachable on ports 443 and 80 unless the
  entry lists others;
- an internal gateway needs `allow_private = true` (it resolves to a private address) **and** its
  port: `{ host = "vllm.internal.example", allow_private = true, ports = [8000] }`;
- the refusals are logged by the egress proxy with the reason (`not on the allowlist`,
  `port not allowed for this host`, `resolves to a ... address`).

## 7. Open questions

These need a real model-backed run to settle and are listed so nobody assumes otherwise
(from the maintainers' notes):

- The LAB runner adds Gemini-specific `thinkingConfig` agent options when a reasoning effort is
  set; whether a given OpenAI-compatible endpoint accepts or drops them is untested.
- Tool-call fidelity of local models inside OpenCode over long multi-step tasks.
- Structured-output support of the judge endpoint (LAB's judge asks for a JSON schema).
- Whether OpenCode records a non-zero cost for a custom provider (section 5).
