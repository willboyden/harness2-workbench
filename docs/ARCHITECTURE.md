# Harness² Workbench — Architecture and Build Contract

Status: **v1.0 contract**. Changes to the public interfaces are additive and recorded in §16
(Contract addenda) with a version bump (v1.1, v1.2, ...). The Rust source of truth for
wire types is `crates/h2w-api`; for domain logic, the public signatures of `crates/h2w-core`.

Harness² Workbench is an unofficial GUI, REST API and MCP server for
[google-research/harness2](https://github.com/google-research/harness2) ("Harness²: Recursive
Agent Harnessing for an Open World"). It is not affiliated with Google.

## 1. Goals

1. **Every harness2 use, without a terminal**: install, demo, benchmark setup and checks, dry-run
   previews, refinement sweeps (both recursion modes, every flag), judging (selected and base runs),
   reports, task splits, the Codex bridge — and browsing everything harness2 writes: task
   summaries, the c1..c8 harness trees, candidate/final diffs, rollouts, selections, reflections,
   experience libraries, improver audit trails.
2. **The same, for AI agents**: an MCP server exposing all of it as tools, resources and prompts.
3. **Safe by default for teams**: authentication, role-based access, CSRF/DNS-rebinding
   protection, secrets that are write-only and redacted from every log, optional container
   isolation with an egress allowlist, a hash-chained audit log (it shows edits by anyone who
   cannot rewrite the server's data directory; see SECURITY.md), cost guards.
4. **Better than the CLI for newcomers and experts**: onboarding, an environment doctor, cost
   estimates, harness diffs and exports, cross-sweep comparisons with a sign test,
   reproducibility manifests, OpenTelemetry and Prometheus.

Non-goals (v0.1): modifying harness2 itself; hosting models; multi-node job scheduling; native TLS
(use a reverse proxy, `docs/ADMIN-GUIDE.md`); Windows hosts (harness2 is Linux-only).

## 2. Decisions (ADR summary; full records in `docs/adr/`)

| # | Decision | Why | Rejected |
|---|---|---|---|
| 0001 | Wrap the harness2 **CLI** (argv, no shell) and read its **files**; never import its Python | harness2 exposes no library API; the CLI + on-disk layout is its contract; process boundary isolates crashes | Embedding Python (PyO3): couples to internals, no isolation |
| 0002 | Rust workspace: `h2w-api` (wire types) · `h2w-core` (domain) · `h2w-daemon` (server) · `h2w-web` (Leptos CSR → WASM) · `h2w-mcp` (rmcp) | One language, shared types end to end, memory safety, self-contained binaries (UI embedded; glibc-linked) | TS frontend: duplicated types; SSR: more attack surface |
| 0003 | One server process owns jobs, state and auth; the MCP server is a **thin client** that forwards the caller's token | One enforcement point for RBAC and audit; MCP holds no extra power | MCP server with its own job runner |
| 0004 | SQLite (bundled, WAL) for state; files for logs; experiments stay plain harness2 directories | Zero-ops, portable, harness2 tools keep working on the same dirs | Postgres: ops burden for a single-node tool |
| 0005 | Strict CSP; WASM loaded by an external module script; no inline script or style attributes | XSS defence in depth for an admin UI that can start processes | Trunk's inline bootstrap (needs `unsafe-inline`) |
| 0006 | Isolation modes: `host` (upstream behaviour, explicit acknowledgement) and `container` (hardened runner, egress allowlist proxy) | Upstream runs executors unsandboxed with network; enterprises need containment | Host only |
| 0007 | Dependency cooldown: direct and locked versions ≥ 14 days old, checked from the index's `pubtime` | Mitigates freshly-published malicious releases | No policy |
| 0008 | Tokens `h2w_<id>_<secret>`, SHA-256 of the secret stored, constant-time compare; browser sessions via HttpOnly SameSite=Strict cookie | Random 256-bit secrets need no slow KDF; id enables O(1) lookup without timing leaks | Passwords |

## 3. Components and process model

```
 Browser (WASM UI) ──HTTPS via reverse proxy or loopback HTTP──┐
                                                              ▼
 MCP clients ──stdio──> h2workbench-mcp ──HTTP + caller token──> h2workbench (server)
 (Claude Code, opencode,  (or streamable HTTP :8723)            │  REST /api/v1 · SSE · static UI · /metrics
  Codex, Cursor, ...)                                           │  auth · RBAC · audit · SQLite
                                                                │  job scheduler ─┬─ host: harness2 venv bins
                                                                │                 └─ container: docker/podman run
                                                                ▼                    runner image + egress proxy
                                     experiments/<name>/  (each = one HARNESS2_RUNS_DIR)
                                     harness2/            (managed checkout + .venv + dependencies/)
```

- `h2workbench serve` — the server (default `127.0.0.1:8722`).
- `h2workbench-mcp` — MCP server; stdio (default) or streamable HTTP (default `127.0.0.1:8723`).
- `h2workbench egress-proxy` — allowlisting forward proxy used by container isolation.
- Other `h2workbench` subcommands: `init`, `token create|list|revoke`, `doctor`, `config check`,
  `audit verify`, `openapi`, `version`.

## 4. Conventions

- JSON `snake_case`; enums are `snake_case` strings; timestamps RFC 3339 UTC strings; ids opaque
  strings (job ids are UUIDv7). Money: USD `f64`.
- Errors: every non-2xx JSON response is `ErrorBody` (`h2w_api::ErrorBody`) with an `ErrorCode`.
  Status mapping: 400 `bad_request`, 422 `validation` (with `details`), 401, 403, 404, 409
  `conflict`, 413, 421 `misdirected_request`, 429 `rate_limited`, 503 `unavailable`, 500.
- Lists that can grow without bound are `Page<T>` with an opaque `cursor`.
- Request ids: honour an incoming `x-request-id` only from trusted proxies, else generate; echo in
  the response header, error bodies, logs and audit records.
- No absolute host paths in responses to non-admins.
- Every identifier is validated with `h2w_core::names` before use; every filesystem path from a
  request or a results file is resolved through `cap_std` (never `Path::join` on user input).

## 5. Repository layout

| Path | What | Change rules |
|---|---|---|
| `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `deny.toml`, `.cargo/` | workspace, toolchain, dependency policy | new dependencies follow [SUPPLY-CHAIN.md](SUPPLY-CHAIN.md) (14-day cooldown, cargo-deny) |
| `crates/h2w-api/**` | wire types shared by every component | additive only; record in §16 |
| `crates/h2w-core/**` | domain logic | public signatures additive only; record in §16 |
| `crates/h2w-daemon/**` | the `h2workbench` server, jobs engine, egress proxy, CLI | |
| `crates/h2w-web/**`, `web/**`, `tools/h2w-mock-api/**` | web UI and its fixture API | the mock answers exactly like the server |
| `crates/h2w-mcp/**` | the `h2workbench-mcp` server | |
| `crates/h2w-e2e/**`, `xtask/**`, `.github/**`, `.githooks/**`, `deploy/**` | tests, tooling, CI, deployment | |
| `docs/**`, `README.md`, `CONTRIBUTING.md`, `SECURITY.md`, ... | documentation | `docs/notes/<area>.md` records implementation decisions per component |
| `tests/fixtures/**` | real harness2 output | regenerated by `tests/fixtures/golden/regenerate.py`, never edited by hand |

Security-sensitive paths need a maintainer's review (`.github/CODEOWNERS`).

## 6. `h2w-core` interface (stable)

See the doc comments in `crates/h2w-core/src/*.rs`. Summary:

| Module | Provides |
|---|---|
| `names` | validators: experiment/profile/secret/token/tag/domain/task slug/harness label/model id/commit/env/hostname/base URL/relative path |
| `namespace` | `Namespace` parse/display (`<bench>-<sub>-<mode><k>[-s<seed>][-<tag>]`), `namespaces_root`, `library_root` |
| `cli` | `Invocation { program, args }` for demo, setup, check, preview (`--dry-run`), run, judge, report, split, bridge |
| `parse` | check, preview, progress lines, run tail, judge tail, demo.json, setup plan, split file, bridge settings (secret!) |
| `results` | `ExperimentDir` (cap-std): sweeps, tasks, task detail, harness trees, diffs, rollout files, scores, report, library, audit, spend, identities, digest, size; `compare()` |
| `tree` | surfaces (OpenCode/Codex), component classification, upstream validation rules, census, diff, SHA-1 fingerprint |
| `report` | port of upstream `summarise`/`render` |
| `estimate` | rollouts/sessions/USD estimate from prices or history |
| `redact` | `Redactor` (secret values -> `[REDACTED:NAME]`) |
| `env` | `JobEnv` from a `ModelProfile` + secrets (+ generated OpenCode provider config) |
| `isolation` | container `run` argv with fixed hardening flags; label verify + kill argv |
| `export` | harness -> raw/OpenCode/Codex project files; deterministic tar.gz |
| `manifest` | reproducibility manifest assembly |
| `install` | managed install steps; installed-state reader |
| `doctor` | probe list; probe results -> checks with remedies |
| `bench` | benchmark catalogue, upstream pins, setup hosts, evaluation blocklist; install state |

## 7. On-disk layout harness2 produces (what `results` reads)

For an experiment directory `E` (= `HARNESS2_RUNS_DIR`) at harness2 `1cfe254`:

```
E/<bench_dir>/adapt/<ns>/<task_slug>/summary.json      # in the PARALLEL (agg) namespace, both modes
E/<bench_dir>/adapt/<ns>/<task_slug>/harnesses/<label>/ # per mode namespace (agg… or seq…)
E/<bench_dir>/adapt/<ns>/<task_slug>/{context,selection,reflection}/
E/<bench_dir>/adapt/<ns>/config.json                    # run identity (summary namespace)
E/<bench_dir>/adapt/<ns>/judged.json, base_judged.json  # summary namespace
E/<bench_dir>/adapt/_sweeps/<ns>-<stamp>.json           # sweep records
E/<bench_dir>/adapt/_audit/<ns>/<label>.<key>.{mission,system,response}.md, .meta.json, .workspace/
E/<bench_dir>/library/<bench_name>/k<k>-<sub>[-s<seed>]/<agg|seq>/<domain>/{INDEX.md,harnesses/<task>/{harness/,EXPERIENCE.md}}
E/lab/results/<oc|cx>/...                               # LAB rollout run dirs
```
`<bench_dir>` = `jb`, `lab`, `wb/<subset>`. Demo experiments (`harness2-demo --output E`):
`E/runs/<ns>/...` (no `adapt/`), `E/library/k<k>/<agg|seq>/<domain>/...`, `E/executions/<run_id>/`,
`E/demo.json`. Summaries record absolute `run_dir`s; map them under `E` (host path, or
`/h2w/experiment` for container jobs) or refuse.

## 8. REST API (`/api/v1`)

Unless noted, request/response bodies are `h2w_api` types. "Role" is the minimum role.
Unsafe methods (POST/PUT/PATCH/DELETE) require header `x-h2w-request: 1`.

| Method | Path | Role | Request | Response |
|---|---|---|---|---|
| GET | `/healthz` (root) | — | | `Health` |
| GET | `/readyz` (root) | — | | `Readiness` (503 when not ready) |
| GET | `/metrics` (root) | viewer or metrics token | | Prometheus text |
| POST | `/auth/login` | — (rate-limited) | `LoginRequest` | `Principal` + `Set-Cookie` |
| POST | `/auth/logout` | any | | 204, cookie cleared |
| GET | `/auth/me` | any | | `Principal` |
| GET | `/system/info` | viewer | | `SystemInfo` (admin-only fields filled for admins) |
| GET | `/system/doctor` | viewer | `?refresh=true` (operator) | `DoctorReport` (cached 60 s) |
| GET | `/benchmarks` | viewer | | `Vec<BenchmarkStatus>` |
| GET | `/profiles` | viewer | | `Vec<ModelProfile>` |
| PUT | `/profiles/{name}` | admin | `ModelProfile` | `ModelProfile` |
| DELETE | `/profiles/{name}` | admin | | 204 (409 if an experiment pins it) |
| GET | `/profiles/{name}/check` | viewer | | `ProfileCheck` (addendum v1.2) |
| GET | `/secrets` | admin | | `Vec<SecretInfo>` |
| PUT | `/secrets/{name}` | admin | `SetSecretRequest` | `SecretInfo` |
| DELETE | `/secrets/{name}` | admin | | 204 |
| GET | `/tokens` | admin | | `Vec<TokenInfo>` |
| POST | `/tokens` | admin | `CreateTokenRequest` | 201 `TokenCreated` |
| DELETE | `/tokens/{id}` | admin | | 204 (revokes; its sessions end) |
| GET | `/audit` | admin | `?limit&cursor` | `Page<AuditEvent>` (newest first) |
| GET | `/audit/verify` | admin | | `AuditVerification` |
| GET | `/experiments` | viewer | `?archived=true` | `Vec<Experiment>` |
| POST | `/experiments` | operator | `CreateExperimentRequest` | 201 `Experiment` (adopts an existing dir of that name) |
| GET | `/experiments/{exp}` | viewer | | `Experiment` |
| PATCH | `/experiments/{exp}` | operator | `UpdateExperimentRequest` | `Experiment` |
| GET | `/experiments/{exp}/manifest` | viewer | | `Manifest` |
| GET | `/experiments/{exp}/sweeps/{sweep}/tasks` | viewer | `?domain&status&limit&cursor` | `Page<TaskRow>` |
| GET | `/experiments/{exp}/sweeps/{sweep}/tasks/{task}` | viewer | | `TaskDetail` |
| GET | `…/tasks/{task}/modes/{mode}/harnesses/{label}` | viewer | | `HarnessTree` |
| GET | `…/tasks/{task}/modes/{mode}/diff` | viewer | `?from&to` | `HarnessDiff` |
| GET | `…/tasks/{task}/modes/{mode}/harnesses/{label}/export` | viewer | `?target=raw\|opencode\|codex\|hermes\|turnstone&format=json\|tar.gz` (v1.11) | `ExportBundle` or `application/gzip` attachment |
| GET | `…/tasks/{task}/modes/{mode}/rollouts/{label}/files` | viewer | `?path=` | `Vec<FileEntry>` |
| GET | `…/tasks/{task}/modes/{mode}/rollouts/{label}/file` | viewer | `?path=` | `FileContent` (2 MiB cap) |
| GET | `/experiments/{exp}/sweeps/{sweep}/scores` | viewer | | `Scores` |
| GET | `/experiments/{exp}/sweeps/{sweep}/report` | viewer | | `ScoreReport` |
| GET | `/experiments/{exp}/sweeps/{sweep}/library` | viewer | | `Vec<LibraryDomain>` |
| GET | `/experiments/{exp}/sweeps/{sweep}/audit` | viewer | | `Vec<AuditSession>` |
| GET | `/experiments/{exp}/sweeps/{sweep}/audit/{session}/{part}` | viewer | part = mission\|system\|response | `text/plain; charset=utf-8` |
| GET | `/compare` | viewer | `?a=<exp>/<sweep>/<mode>&b=…` | `Comparison` |
| POST | `/estimate` | viewer | `EstimateRequest` | `CostEstimate` |
| POST | `/harness/validate` | viewer | `ValidateHarnessRequest` | `ValidationReport` |
| GET | `/splits` | viewer | | `Vec<SplitInfo>` |
| GET | `/jobs` | viewer | `?status&kind&experiment&limit&cursor` | `Page<Job>` (newest first) |
| POST | `/jobs` | `JobSpec::required_role()` | `JobSpec` | 201 `Job` |
| GET | `/jobs/{id}` | viewer | | `Job` |
| POST | `/jobs/{id}/cancel` | operator | | `Job` (409 if terminal) |
| GET | `/jobs/{id}/log` | viewer | `?from=0&limit=500` (max 5000 lines; a page also stops near 1 MiB, v1.4) | `LogChunk` |
| GET | `/jobs/{id}/events` | viewer | | `text/event-stream`, each `data:` is a `JobEvent` |
| GET | `/openapi.json` | viewer | | OpenAPI 3.1 document |

Static UI: `GET /` (index.html), `/app.js`, `/styles.css`, `/favicon.svg`, `/pkg/*` (wasm-bindgen
output). Any other GET that is not `/api/*`, `/healthz`, `/readyz`, `/metrics` or a static file
returns `index.html` (client-side routes). Assets are embedded in the binary from `dist/web/`.

**Job validation the server performs before 201**: the spec's own `validate()` rules; names;
experiment exists (or for `demo`, is created as a demo experiment); the experiment's pinned profile
matches (409 otherwise); profile exists and its secrets are set (422 naming the secret); harness2 is
installed (503 `unavailable` otherwise, except `install_harness2`); `wb` and `bridge` refuse
container isolation in v0.1 (409); host isolation requires `acknowledge_risks: true` on
`run`/`pipeline` unless the server config pre-acknowledges; only one writing job
(`demo`/`run`/`judge`/`pipeline`) per experiment at a time (queued behind it).

## 9. Security model

**Transport/origin.** Bind loopback by default. `Host` must be in the allowlist (derived from
`server.public_url` + loopback forms of the listen port + `server.allowed_hosts`) else 421.
Unsafe methods need `x-h2w-request: 1` and, when an `Origin` header is present, it must be the
public origin or the request's own Host with the public URL's scheme (`http://` only when the
public URL is not https; a repeated `Origin` header is refused), else 403. No CORS headers, ever. `X-Forwarded-For`, `x-request-id` and (proxy mode)
the identity headers are honoured only from `server.trusted_proxies`.

**Authentication.** `Authorization: Bearer h2w_<id>_<secret>` or the `h2w_session` cookie
(HttpOnly, SameSite=Strict, Path=/, Secure when `public_url` is https; 32-byte random id, stored
hashed, TTL from config, revoked with its token). Proxy mode: identity from configured headers,
accepted only from `trusted_proxies` AND with the shared-secret header matching; roles from group
mapping. `h2workbench init` creates the first admin token (printed once, also written 0600 to
`<config_dir>/admin.token`). Login is rate-limited per IP.

**Authorization.** Role checks are route-level extractors (not only middleware); a test enumerates
the router's routes and asserts each declares a role. Job creation additionally checks
`JobSpec::required_role()`.

**Secrets.** Stored in the database (file mode 0600, directory 0700) or supplied as
`H2W_SECRET_<NAME>` environment variables (which take precedence and never touch disk). Write-only
API; responses carry `SecretInfo` only. Passed to jobs only through the child environment (never
argv) and only the secrets the job's profile references. Every job log line, error and audit detail
goes through the job's `Redactor`. The bridge key printed by `harness2-bridge` is captured into the
secret `HARNESS2_CODEX_API_KEY` and redacted before logging.

**Job processes.** argv only, never a shell; `env_clear()` + `BASE_ENV_PASSTHROUGH` + `JobEnv`;
own process group; cancellation = SIGTERM to the group, SIGKILL after 10 s; wall-clock timeout.
Generated files (OpenCode config, key file) live in a per-job 0700 dir, deleted when the job ends.

**Container isolation.** `docker|podman run` with `--rm --cap-drop ALL --security-opt
no-new-privileges --pids-limit`, CPU/memory limits, a non-root user, the job label, the configured
network (`none` or a pre-created network behind `h2workbench egress-proxy`), mounts limited to the
experiment (rw), the harness2 home (ro except install/setup), and the job-files dir (ro). Label is
verified before any kill. The image is pinned by tag or digest from configuration.

**Egress proxy.** HTTP CONNECT + absolute-form HTTP; hostname allowlist (exact or `*.suffix`);
resolves and pins the address it checked; refuses loopback, private, link-local, CGNAT and metadata
addresses unless the allowlist entry is marked `allow_private`; logs each decision without bodies.

**Audit.** Every authentication outcome, job create/cancel, experiment/profile/secret/token change
and access denial is appended to a SHA-256 hash chain in the database; `h2workbench audit verify`
re-walks it. No secret values or request bodies are recorded.

**Bounds.** Body limit (default 1 MiB), request timeout, SSE connection cap + 15 s heartbeat + idle
close, page/limit clamps, file read cap (2 MiB), queue cap, metric label cardinality bounded.

**Headers on every response** (including errors and static files): `Content-Security-Policy:
default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; img-src 'self' data:;
font-src 'self'; connect-src 'self'; manifest-src 'self'; base-uri 'none'; form-action 'self';
frame-ancestors 'none'`, `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`,
`X-Frame-Options: DENY`, `Permissions-Policy: camera=(), microphone=(), geolocation=()`,
`Cross-Origin-Opener-Policy: same-origin`, `Cross-Origin-Resource-Policy: same-origin`;
`Cache-Control: no-store` on `/api/*`; hashed assets `immutable`.

## 10. Jobs engine

States: `queued -> running -> succeeded | partial | failed | cancelled` (harness2 exit 2 =
`partial`). Scheduler: FIFO, `jobs.max_concurrent` (default 2), per-experiment single writer,
queue cap. Restart recovery: jobs `running` at startup become `failed` ("server restarted") and
their process groups are terminated if still alive (pid + start time recorded).

`run` and `pipeline` begin with an internal `preview` step (same options plus `--dry-run`) so the
stream and total are known; progress comes from `parse::progress_line`. `pipeline` steps:
`preview`, `check`, `run`, `judge`, [`judge-bases`], `report`; it stops at the first step that is
neither `succeeded` nor `partial`. Budget guard (`budget_usd`): every `jobs.budget_poll_secs`,
`ExperimentDir::spend_usd(summary ns)`; at or above the cap the job is cancelled with error
"budget reached" (harness2 records improver spend only; executor rollout spend is not recorded —
say so in the UI).

Logs: stdout/stderr line-split (max 64 KiB per line, truncated beyond), redacted, appended to
`<data_dir>/logs/jobs/<id>.log` (0600, one JSON `LogLine` per line), kept in a ring buffer for SSE,
paged by line number. Results: the kind's parser (`h2w_core::parse`) on the captured stdout and
the files the command writes; stored as JSON on the job row.

## 11. Configuration (`h2workbench.toml`)

Location: `--config`, else `$H2W_CONFIG`, else `$XDG_CONFIG_HOME/harness2-workbench/h2workbench.toml`
(created by `h2workbench init`). Unknown keys are an error. Environment overrides: `H2W_LISTEN`,
`H2W_DATA_DIR`, `H2W_PUBLIC_URL`, `H2W_LOG_FORMAT`, `H2W_OTLP_ENDPOINT`.

```toml
[server]
listen = "127.0.0.1:8722"
public_url = "http://127.0.0.1:8722"
allowed_hosts = []
trusted_proxies = []
max_body_bytes = 1048576
request_timeout_secs = 60
sse_max_connections = 64
sse_max_per_principal = 8            # a token and its sessions together (v1.3)
max_connections_per_ip = 256         # IPv6 per /64; trusted proxies exempt (v1.3)
login_attempts_per_minute = 10

[auth]
mode = "token"                       # token | proxy
session_ttl_hours = 12
[auth.proxy]
user_header = "x-forwarded-user"
groups_header = "x-forwarded-groups"
secret_header = "x-h2w-proxy-secret"
secret_env = "H2W_PROXY_SECRET"
admin_groups = []
operator_groups = []
viewer_groups = []                   # empty = any authenticated proxy user is a viewer

[paths]
data_dir = "~/.local/share/harness2-workbench"   # workbench.db, logs/, jobs/, splits/
experiments_dir = ""                 # default <data_dir>/experiments
harness2_home = ""                   # default <data_dir>/harness2
harness2_managed = true              # false = point at an existing checkout (never modified)
allow_untested_harness2_rev = false  # install_harness2 rev other than the tested commit (v1.3)

[jobs]
max_concurrent = 2
max_queued = 100
timeout_hours = 48
budget_poll_secs = 30
log_retention_days = 90
max_log_bytes = 268435456            # per job: one marker line, then output is counted, not stored (v1.3)
min_free_disk_mb = 256               # stop jobs when the data directory's filesystem is lower; 0 = off (v1.3)

[isolation]
mode = "host"                        # host | container
preacknowledge_host_risks = false
[isolation.container]
runtime = "docker"                   # docker | podman
image = "harness2-workbench-runner:0.1.4"
network = "none"
cpus = 4.0
memory_mb = 8192
pids_limit = 2048
user = ""
proxy = ""                           # http://host:port of the egress proxy on `network` (v1.2)

[egress]                             # used by `h2workbench egress-proxy`
listen = "127.0.0.1:3128"            # compose sets 0.0.0.0 explicitly (audit F14)
allow = []                           # ["aiplatform.googleapis.com", {host="litellm", allow_private=true, ports=[4000]}]; ports default 443, 80

[telemetry]
log_format = "pretty"                # pretty | json
log_level = "info"
otlp_endpoint = ""                   # OTLP/HTTP, e.g. http://127.0.0.1:4318
service_name = "harness2-workbench"
metrics_token_secret = ""            # secret NAME accepted as a bearer token on /metrics

[benchmarks]                         # v1.6
not_offered = []                     # [{bench="wb", note="Runs on the sandbox Workbench.", url="https://wb.example.com"}]
```

## 12. Web UI (`h2w-web`)

Leptos 0.8 CSR compiled to `wasm32-unknown-unknown`, `wasm-bindgen --target web`, loaded by
`/app.js` (`import init from "/pkg/h2w_web.js"; init({ module_or_path: "/pkg/h2w_web_bg.wasm" })`).
`cargo xtask web` builds `dist/web/`. No inline `<script>`, no `style="..."` attributes (CSP); dynamic
sizes via SVG attributes or `style:prop` (CSSOM). Same-origin `fetch` with cookies; every unsafe
request sends `x-h2w-request: 1`; errors render `ErrorBody.error.message`.

Routes: `/login`, `/` (dashboard), `/welcome` (onboarding), `/experiments`, `/experiments/:exp`,
`/experiments/:exp/sweeps/:sweep` (tabs: tasks · report · library · audit),
`/experiments/:exp/sweeps/:sweep/tasks/:task` (timeline · harnesses + diff · rollouts · selection &
lesson), `/new` (run wizard: cell → options → preview → estimate → confirm), `/jobs`, `/jobs/:id`
(live log over SSE), `/compare`, `/benchmarks`, `/settings/{profiles,secrets,tokens,audit,system}`,
`/help`. Shell: sidebar nav, role-aware actions (hidden when not allowed, and the server refuses
anyway), theme toggle (light/dark/system), command palette (Ctrl+K), toasts, keyboard shortcuts,
`aria-live` status, WCAG 2.1 AA contrast, `prefers-reduced-motion`.

## 13. MCP server (`h2w-mcp`)

rmcp 3.5.1. Transports: stdio (default) and streamable HTTP (`--transport http --listen
127.0.0.1:8723`, Host/Origin checks, bearer required). Backend: the server's REST API at
`--server-url` (`H2W_SERVER_URL`, default `http://127.0.0.1:8722`). Credentials: stdio uses
`H2W_TOKEN` or `--token-file`/`H2W_TOKEN_FILE` (never a CLI argument); HTTP forwards each caller's
own bearer token to the server, so the server's RBAC and audit apply per client.

Tools (snake_case; annotations: read-only / destructive / idempotent / open-world set correctly):
`server_info`, `doctor`, `list_benchmarks`, `setup_benchmark`, `check_benchmark`, `list_profiles`,
`list_experiments`, `create_experiment`, `get_experiment`, `estimate_cost`, `preview_run`,
`start_run`, `start_pipeline`, `run_demo`, `start_judge`, `start_report`, `create_split`,
`list_jobs`, `get_job`, `wait_for_job` (≤ 600 s, progress notifications), `cancel_job`,
`get_job_log`, `list_tasks`, `get_task`, `get_harness`, `diff_harnesses`, `export_harness`,
`validate_harness`, `list_rollout_files`, `read_rollout_file`, `get_scores`, `get_report`,
`get_library`, `list_audit_sessions`, `read_audit_session`, `compare_sweeps`, `get_manifest`,
`install_harness2`, `start_bridge`.
Resources: `h2w://jobs/{id}/log`, `h2w://experiments/{exp}/sweeps/{sweep}/report`,
`h2w://experiments/{exp}/manifest`. Prompts: `improve_agent_harness`, `analyze_sweep`,
`adopt_best_harness`. Tool outputs: JSON text (pretty, truncated with a note above 100 KB) plus
structured content where the SDK supports it; errors are tool errors with the server's message.

## 14. Observability

`tracing` everywhere (pretty or JSON); optional OTLP/HTTP trace export (`otel` feature); request
spans carry method, matched route, status, request id (never headers or bodies). Prometheus
`/metrics`: `h2w_http_requests_total{method,route,status}`, `h2w_http_request_duration_seconds`,
`h2w_jobs_total{kind,status}`, `h2w_jobs_running`, `h2w_job_duration_seconds{kind}`,
`h2w_spend_usd_total{experiment}` (bounded label set), `h2w_auth_failures_total{reason}`,
`h2w_sse_connections`, and since v1.3 `h2w_audit_suppressed_total` (audit records of failed
authentication or refused requests over the per-client or per-principal share, v1.4) and
`h2w_connections_refused_total{reason}` (per-IP connection cap).

## 15. Quality bar (every change)

- `cargo fmt --check`; `cargo clippy --workspace --all-targets -- -D warnings`;
  `cargo clippy -p h2w-web --target wasm32-unknown-unknown -- -D warnings`; `cargo test --workspace`;
  `cargo deny check`; `cargo xtask cooldown`.
- No `unsafe` (forbidden), no `unwrap()` outside tests, no `todo!`/`unimplemented!`, no
  `CoreError::Unimplemented` reachable at release.
- Tests: unit tests next to code; property tests (proptest) for validators, parsers, path handling,
  redaction; integration tests against real captured harness2 output and real subprocesses.
- Report honestly: what you verified by running it, what only with fakes, what not at all.

## 16. Contract addenda

**v1.1 (2026-10-23) — SSE framing.** `GET /api/v1/jobs/{id}/events` frames are UNNAMED: no
`event:` line, so browsers deliver them to `EventSource.onmessage`. Each frame carries exactly one
`JobEvent` JSON object in its `data:` field (axum `Event::default().json_data(..)`). The server
MAY set `id:` to the job-log line number of `log` events; clients may ignore it. Comment frames
(`: keep-alive`) are allowed in addition to `heartbeat` events.

**v1.1 (2026-10-23) — units.** `Comparison.tasks[].{a,b,delta}` and
`Comparison.{mean_a,mean_b,mean_delta}` are FRACTIONS in 0..1 (partial credit, the same unit as
`JudgedRow.frac` and `TaskModeOutcome.score`); `delta = b - a` in fraction units (show ×100 as
percentage points). Percent (0..100) is used only by `ModeScore`, `ModeSummary` and
`ScoreReport.base_score`, matching upstream's report.

**v1.1 (2026-10-23) — UI test hooks.** The end-to-end suite selects elements only by
`data-testid`: `login-token`, `login-submit`, `login-error`; `nav-dashboard`, `nav-experiments`,
`nav-jobs`, `nav-new-run`, `nav-compare`, `nav-benchmarks`, `nav-settings`, `nav-help`,
`current-user`, `theme-toggle`; `dashboard`, `harness2-status`; `experiment-row` (+`data-name`),
`experiment-create`; `sweep-row` (+`data-sweep`); `tab-tasks`, `tab-report`, `tab-library`,
`tab-audit`, `task-row` (+`data-task`), `report-view`, `library-view`; `mode-section`
(+`data-mode`), `harness-label` (+`data-label`), `diff-from`, `diff-to`, `diff-view`,
`export-button`; `job-row` (+`data-job-id`), `job-status` (text = the `JobStatus` string),
`job-log`, `job-cancel`; `toast`.

**v1.1 (2026-10-23) — server CLI for automation.** `h2workbench serve` accepts port 0 and prints one
STDOUT line `h2workbench listening on http://<ip>:<port>` once bound. `init` and `token create`
accept `--config` and print the new token as the only STDOUT line (explanations on stderr);
`init` never rewrites an existing configuration; when an admin token already exists it refuses
unless `--force`, which mints another bootstrap token.

**v1.1 (2026-10-23) — MCP argument names.** Tool arguments mirror the REST names:
`experiment`, `sweep` (summary namespace id), `task` (slug), `mode`, `label`, `from`/`to`, `path`,
`session` + `part`, `job_id`; `run_demo` takes the `DemoParams` fields flat; `wait_for_job` takes
`job_id` and `timeout_secs` (≤ 600) and returns the `Job`. Results are JSON (structured content or
the first text block); refusals set `is_error` and name the server's error code. Stdio
configuration: `H2W_SERVER_URL`, `H2W_TOKEN`.

**v1.1 (2026-10-23) — MCP tools.** Added `list_splits` (GET `/splits`, viewer) and
`update_experiment` (PATCH `/experiments/{exp}`, operator). Rule: MCP never exposes writes to
profiles, secrets or tokens, nor the audit log — a prompt-injected agent could repoint a profile's
endpoint at a hostile URL and exfiltrate the referenced API key on the next job. Job-starting tools
take an optional `wait_secs`; the HTTP transport is stateless at `/mcp`.

**v1.1 (2026-10-23) — request details.** `{mode}` path segments and `/compare` references use
`parallel`/`sequential`; servers also accept the on-disk tokens `agg`/`seq`. `POST
/jobs/{id}/cancel` takes no body. `Progress.done` counts tasks finished `ok`/`skipped`,
`Progress.failed` those finished with any other status (disjoint; finished = done + failed);
`Progress.total` comes from the job's internal preview step.

**v1.1 (2026-10-23) — dependencies.** `tools/h2w-mock-api` may use `tokio-stream` (already in the
lockfile) to stream SSE.

**v1.2 (2026-10-23) — job egress.** Host-mode jobs also receive the server's own proxy and CA
variables when set (`HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, `NO_PROXY` and their lowercase
forms; `SSL_CERT_FILE`, `SSL_CERT_DIR`, `REQUESTS_CA_BUNDLE`, `NODE_EXTRA_CA_CERTS`; and every
`UV_*`, `PIP_*` and `NPM_CONFIG_*` variable, such as the runner image's
`UV_PYTHON_INSTALL_DIR` or a corporate index mirror: `h2w_core::env::is_passed_through`), so the
compose deployment, where the Workbench sits on an internal network, reaches the allowlisted hosts
through `h2workbench egress-proxy`. Profiles cannot set the uppercase proxy names (`RESERVED_ENV`).
Container jobs get none of the server's variables; `[isolation.container] proxy =
"http://host:port"` (no credentials, needs a network other than `none`) sets
`HTTPS_PROXY`/`HTTP_PROXY`/`https_proxy`/`http_proxy` and `NO_PROXY`/`no_proxy` =
`localhost,127.0.0.1,::1` inside the container as fixed `-e NAME=VALUE` arguments
(`ContainerSpec::env_values`; never secrets), not by name, so the runtime CLI never sees a proxy
that exists only on the jobs network.

**v1.2 (2026-10-23) — container HOME.** On a rootless runtime jobs run as container uid 0 with
every capability dropped, which cannot write `/home/node`. The runner image therefore sets
`HOME=/h2w/home`, a sticky world-writable directory in the throwaway container layer; the
all-in-one image sets `HOME=/home/node` back for the server user. Custom runner images must give
the job user a writable `HOME`.

**v1.2 (2026-10-24) — raw judge points.** `JudgedRow` and `BaseJudgedRow` gain
`points_passed`/`points_max: Option<f64>`, the points exactly as the judge wrote them (always set
when present). `n_passed`/`n_criteria` stay integers and are empty when the value is fractional,
as with JobBench's weighted judging; `frac` is computed from the raw values.

**v1.2 (2026-10-24) — profile check.** `GET /api/v1/profiles/{name}/check` (viewer) returns
`ProfileCheck { warnings, missing_secrets }`: `h2w_core::env::profile_warnings` and the referenced
secrets with no value (stored or `H2W_SECRET_<NAME>`), which job creation refuses with 422.
404 for an unknown profile. MCP tool `check_profile` (`profile`; read-only, viewer).

**v1.3 (2026-10-24) — security audit fixes.** Additive contract changes from the first
independent audit (findings F1–F16):
- `CoreError::TooLarge(String)` → 413 `payload_too_large`. The results reader caps what it reads
  from harness2's output (summary.json 4 MiB; per call 10,000 tasks and 256 MiB of summaries;
  judged files 32 MiB / 100,000 rows; harness trees 2,000 files / 64 MiB / 4 MiB per file;
  docs/notes/core.md has the table). `TaskRow.status` may be `too-large` or `unreadable`, and the
  `?status=` filter accepts both. Core adds `ExperimentDir::sweep_namespaces()` and `sweep()`; the
  experiment list reads sweeps one at a time (a failing sweep is left out and logged) through a
  per-sweep cache and at most 4 concurrent blocking reads.
- `ContainerSpec::env_file`: container jobs receive their variables through a 0600 `--env-file`
  in the job directory; the runtime CLI process gets only the server pass-through and the runtime
  variables. Values with a newline, CR or NUL are refused. Every step process carries
  `H2W_JOB_ID=<job id>`; `jobs.pid_boot` (database migration 2) stores the boot id, and restart
  recovery signals only groups of this boot with a member that carries the marker.
- `SystemInfo.host_risks_preacknowledged: bool` (every role). More reserved env names and
  prefixes for profiles (`h2w_core::env`); stored profiles that use them are refused at job time.
- Auth: per client, 60 failed authentications a minute, then 429; at most 10 failed-auth audit rows
  a minute (the rest counted in `h2w_audit_suppressed_total`). Repeated proxy identity headers →
  401; a repeated `Host` header → 400. The session cookie is `__Host-h2w_session` when
  `public_url` is https. `install_harness2.rev` other than `TESTED_HARNESS2_COMMIT` → 422 unless
  `paths.allow_untested_harness2_rev`; the MCP tool has no `rev`, and `h2workbench-mcp` refuses a
  model's `acknowledge_risks=true` unless started with `--allow-risk-acknowledgement`.
- The server and the egress proxy are non-dumpable; the egress proxy listens on 127.0.0.1:3128 by
  default. Doctor adds `runner_image` and `container_network` (container mode).

**v1.4 (2026-10-24) — security re-audit fixes (N1–N10).**
- Results files are opened non-blocking without following links and checked by fstat (a regular
  file with the device and inode the lstat saw): a FIFO swapped in reads as `unreadable`, never
  hangs. The harness fingerprint and the manifest digest never read a file over its cap (a size
  marker) and refuse past their totals (`TooLarge`).
- `h2w_core::results::ReadBudget` (bytes, sweeps, an optional cancellation flag) and `*_with`
  variants of the results calls; `CoreError::Cancelled` → 503. The server gives each route a budget
  (docs/notes/daemon.md), cancels a request's reads when the request is abandoned, lists at most
  200 sweeps per experiment (newest first; older ones stay reachable by id) and keeps an LRU sweep
  cache that serves a stale entry while it re-reads.
- `GET /jobs/{id}/log` pages and event-stream replays stop near 1 MiB (`next` continues). Results
  responses are serialized inside the read permit and refused past 64 MiB (413). The MCP
  `get_job_log tail=N` and the `h2w://jobs/{id}/log` resource follow the pages and return the newest
  lines that fit their output cap (the resource cuts a newest line longer than its cap instead of
  dropping it).
- Refusal records (`access.denied`, and `job.create` / `job.cancel` denials) are limited per
  principal; limiter keys behind trusted proxies are kept per peer (1,024 clients per peer, then
  that peer's overflow bucket). Audit actors name their kind: `token:<name>`, `proxy:<user>`,
  `cli`, `server`, `anonymous`. The MCP HTTP Host guard refuses userinfo.


**v1.5 (2026-10-26) — offline install bundles ([ADR 0009](adr/0009-offline-install-bundles.md)).**
- `h2w_core::bundle`: `FORMAT`, `MANIFEST_NAME`, `RUNNER_IMAGE_NAME`, `TREE_PREFIX`;
  `BundleManifest`, `BundleBenchmark`, `BundleImage`, `TreeStats`; `survey`, `write`,
  `read_manifest`, `scan`, `unpack`, `compatibility`, `sha256_hex`, `sha256_line`. The module works
  on administrator-given paths (never a request's) and checks every archive name before it becomes
  a path.
- `h2workbench bundle export --output FILE [--with-runner-image] [--force]`,
  `bundle verify FILE [--sha256 HEX]`, `bundle import FILE --sha256 HEX`: command line only (no
  REST operation, no MCP tool). Stdout carries one line: the `sha256sum` line (export), `bundle OK:
  <hex>` or `problem:` lines (verify), `installed <hex>` (import). Audit actions `bundle.export`,
  `bundle.import` (actor `cli`).

**v1.6 (2026-10-27) — benchmark availability.**
- `BenchmarkStatus.unavailable: Option<BenchmarkUnavailable>` (additive; absent = the server
  sets the benchmark up and runs it). `BenchmarkUnavailable { reason, message, url? }` with
  `UnavailableReason` `not_offered` | `isolation`; `h2w_api::WB_NEEDS_HOST_ISOLATION` is the
  message for WorkBuddy-Bench under container isolation.
- Configuration `[benchmarks] not_offered = [{ bench, note, url }]` (each benchmark at most once;
  note at most 300 characters; url `http(s)` with a host, no credentials). An entry wins over the
  isolation rule.
- `h2w_core::bench`: `NotOffered`, `unavailable(bench, isolation, not_offered)`,
  `status_on(home, isolation, not_offered)`. `GET /benchmarks` and job creation use the same
  rule: a job whose cell, `setup` or `split` names an unavailable benchmark gets `409` with its
  `message`. The bridge's container refusal now reads `The Codex bridge needs host isolation in
  this version`.
- Web UI: such a card shows the reason as its badge (`not on this server`, `needs host
  isolation`), the message and the link, and no setup or check buttons; the New run wizard stops
  at its first step with the message; splits are offered only for benchmarks the server runs.
  The mock API mirrors the rule (`--container`, `--not-offered BENCH`).

**v1.7 (2026-10-27) — LAB documents under container isolation.**
- The runner image's `/usr/local/bin/podman` is `deploy/docker/podman-standin.sh`. It accepts only
  `podman run [--rm] [--network=none] [--cap-drop=ALL] [--security-opt=no-new-privileges]
  -v <file>:/doc/<name>:ro <image> parse-doc {docx|pdf|pptx|xlsx} /doc/<name>` (the call of LAB's
  `read_doc.py` in "sandbox" mode), with `<file>` an absolute regular file, and runs
  `python3 -I $LAB_BENCH_ROOT/sandbox/parsers/parse_doc.py <ext> <file>`; anything else exits 125.
- `cargo xtask runner-docs --harness2-home <home> [--image] [--runtime]` checks it against a real
  LAB checkout: each document read through the stand-in must equal `read_doc.py --in host`.

**v1.8 (2026-10-27) — comparing against the starting harness.**
- `h2w_api::CompareSide { Mode(Mode), Base }` (`parse`: `base` or a mode token; `as_param`).
  `GET /compare` accepts `<experiment>/<sweep>/base` for either side; anything else in the third
  segment is `422`.
- `Comparison.noise: Option<NoiseFloor { kind, n, mean_abs, max_abs }>` (additive; fractions),
  `NoiseKind` `across_sweeps` | `within_sweep`: across two sweeps of experiments pinned to the
  same profile when some task has graded reference rollouts in both, else the spread between the
  k reference rollouts of each task of A, then of B. `h2w_core::results::noise_floor`.
- `TaskRow.base: Option<BaseOutcome { graded, score, spread?, all_pass }>` and
  `SweepSummary.base_score: Option<f64>` (percent), both additive; a reference rollout's score is
  `points_passed / points_max` of its `base_judged.json` row.
- MCP `compare_sweeps`: a side's `mode` may be `base`. The web UI and the mock API follow.

**v1.9 (2026-10-27) — chosen splits.**
- `POST /splits` (operator) with `ChooseSplitRequest { name, bench, subset?, train, test }`
  (`deny_unknown_fields`) writes `<data_dir>/splits/<name>.json` (0600, created new) in harness2's
  split format (`benchmark` = the cell's bench name, `train`, `test` sorted, `seed` 0,
  `train_fraction` the train share, plus `chosen_by`) and records `SplitInfo` with the additive
  `chosen: true`. Checks: the name as experiment names; WorkBuddy needs a subset and nothing else
  takes one; at least one train id; at most `SPLIT_TASKS_MAX` (10,000) ids; each id 1..=255 bytes,
  no control characters or `\`, no surrounding whitespace, no empty, `.` or `..` part; no id twice
  across both sides. The file is data harness2 reads, never a path the server uses. Audit
  `split.choose`. Existence of the ids is harness2's check when a job uses the split.
- MCP `choose_split` (operator, 43 tools). The web UI's split dialog offers *Chosen tasks*.

**v1.10 (2026-10-27) — cloud judges.**
- `h2w_api::judge`: `JudgeSite { kind: JudgeSiteKind (local | cloud | unknown), provider?, hosts }`,
  `judge_site(profile, private_hosts)` (pure; the server, the web UI and the mock API share it),
  `cloud_judge_warning`, `CLOUD_JUDGE_RULE`, `CloudJudgePolicy` (`acknowledge` | `refuse`). Hosts
  come from the judge endpoint (or an OpenAI-compatible profile's own endpoint), `extra_env`
  literals for `HARNESS2_JUDGE_BASE_URL`, `OPENAI_BASE_URL` and `ANTHROPIC_BASE_URL`, and Vertex AI
  for `claude*`, `gemini*` and `vertex/` judges when the profile has Vertex settings. Loopback,
  private, link-local and CGNAT addresses, single-label names, local-only domains and
  `[judge] private_hosts` entries are local; an endpoint set through a secret is `unknown`.
- Additive fields: `ModelProfile.judge_site` and `ProfileCheck.judge_site` (response only, never
  stored), `SystemInfo.cloud_judges` and `SystemInfo.private_judge_hosts`,
  `JudgeParams.acknowledge_cloud_judge` and `PipelineParams.acknowledge_cloud_judge` (default
  false; the pipeline passes it to its judge steps).
- Job validation, after the profile's secrets: a `judge` or `pipeline` job whose profile's judge is
  not local is `409` under `[judge] cloud = "refuse"`, else `422` on
  `params.acknowledge_cloud_judge` without the acknowledgement. Audit detail on success:
  `acknowledge_cloud_judge: data is generated or synthetic`. The profile check lists the
  cloud-judge warning first, and `profile_warnings` adds two compatibility warnings: a `claude` or
  `gemini` judge without Vertex settings cannot judge LAB at the tested commit, and a Claude judge
  from 4.7 on fails JobBench's temperature-0 calls.
- Configuration `[judge] cloud` and `[judge] private_hosts`. MCP: `acknowledge_cloud_judge` on
  `start_judge` and `start_pipeline`, passed on only with `--allow-cloud-judge-acknowledgement`
  (`H2W_MCP_ALLOW_CLOUD_JUDGE_ACK`), reported in `server_info`'s policy. The web UI's profile
  editor has judge presets and a live judge-site notice; judging and pipelines with a cloud judge
  ask for the synthetic-data confirmation.

**v1.11 (2026-10-27) — Hermes Agent and Turnstone executors.**
- `Substrate` gains `hm` (Hermes Agent) and `ts` (Turnstone): `Substrate::ALL`, `upstream()` (only
  `oc` and `cx`), `lab_only()`, `surface()`. `Surface` gains `hermes` and `turnstone`;
  `ExportTarget` gains `hermes` and `turnstone` (`ExportTarget::ALL`, `as_param()`).
- `Harness2Status.substrates`: the executors the installed checkout supports, detected from
  `harness2/substrate/hermes.py` and `turnstone.py`; `serde` default `["oc", "cx"]` for a server that
  omits it. `ValidationReport.notes` (omitted when empty): what harness2 also checks that the
  Workbench cannot (scanners loaded from each framework's install).
- Job validation: `hm`/`ts` cells run LAB only (`Cell::validate`; `setup` likewise); a cell or setup
  naming an executor the installed harness2 lacks is `422` on `params.cell.substrate` or
  `params.substrate`; a `run` or `pipeline` on `hm`/`ts` needs an `openai_compatible` profile or the
  executor's `..._BASE_URL` in `extra_env` (`422` on `params.profile`). `EnvContext.substrate`
  limits `HARNESS2_HERMES_*` / `HARNESS2_TURNSTONE_*` to jobs on that executor.
- `h2w_core::tree` validates Hermes and Turnstone trees with harness2's rules and each framework's
  static refusals, in harness2's words; `h2w_core::export` converts any surface to any target and
  refuses what has no counterpart. `h2w_core::env::job_profile_warnings` filters a job log's profile
  warnings by benchmark and executor (`WarningScope`).
- Docs: [notes/substrates.md](notes/substrates.md).

**v1.12 (2026-10-27) — scoped profile checks.**
- `GET /profiles/{name}/check` takes optional `bench` (`jb` | `lab` | `wb`) and `substrate`
  (`oc` | `cx` | `hm` | `ts`) query parameters (`CheckQuery`, `deny_unknown_fields`; another
  value is `422` on that field) and then lists only the warnings
  `h2w_core::env::job_profile_warnings` keeps for those jobs; the cloud-judge warning stays first.
  Without them the answer is unchanged. MCP `check_profile` takes the same two optional fields;
  the run wizard sends its chosen benchmark and executor.
