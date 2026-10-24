# Harness² Studio — Architecture and Build Contract

Status: **v1.0 contract (LOCKED)**. Changes after fan-out are additive only and recorded in
§16 (Contract addenda) with a version bump (v1.1, v1.2, ...). The Rust source of truth for
wire types is `crates/h2s-api`; for domain logic, the public signatures of `crates/h2s-core`.

Harness² Studio is an unofficial GUI, REST API and MCP server for
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
   isolation with an egress allowlist, a tamper-evident audit log, cost guards.
4. **Better than the CLI for newcomers and experts**: onboarding, an environment doctor, cost
   estimates, harness diffs and exports, cross-sweep comparisons with a sign test,
   reproducibility manifests, OpenTelemetry and Prometheus.

Non-goals (v0.1): modifying harness2 itself; hosting models; multi-node job scheduling; native TLS
(use a reverse proxy, `docs/ADMIN-GUIDE.md`); Windows hosts (harness2 is Linux-only).

## 2. Decisions (ADR summary; full records in `docs/adr/`)

| # | Decision | Why | Rejected |
|---|---|---|---|
| 0001 | Wrap the harness2 **CLI** (argv, no shell) and read its **files**; never import its Python | harness2 exposes no library API; the CLI + on-disk layout is its contract; process boundary isolates crashes | Embedding Python (PyO3): couples to internals, no isolation |
| 0002 | Rust workspace: `h2s-api` (wire types) · `h2s-core` (domain) · `h2s-daemon` (server) · `h2s-web` (Leptos CSR → WASM) · `h2s-mcp` (rmcp) | One language, shared types end to end, memory safety, single static binaries | TS frontend: duplicated types; SSR: more attack surface |
| 0003 | One server process owns jobs, state and auth; the MCP server is a **thin client** that forwards the caller's token | One enforcement point for RBAC and audit; MCP holds no extra power | MCP server with its own job runner |
| 0004 | SQLite (bundled, WAL) for state; files for logs; experiments stay plain harness2 directories | Zero-ops, portable, harness2 tools keep working on the same dirs | Postgres: ops burden for a single-node tool |
| 0005 | Strict CSP; WASM loaded by an external module script; no inline script or style attributes | XSS defence in depth for an admin UI that can start processes | Trunk's inline bootstrap (needs `unsafe-inline`) |
| 0006 | Isolation modes: `host` (upstream behaviour, explicit acknowledgement) and `container` (hardened runner, egress allowlist proxy) | Upstream runs executors unsandboxed with network; enterprises need containment | Host only |
| 0007 | Dependency cooldown: direct and locked versions ≥ 14 days old, checked from the index's `pubtime` | Mitigates freshly-published malicious releases | No policy |
| 0008 | Tokens `h2s_<id>_<secret>`, SHA-256 of the secret stored, constant-time compare; browser sessions via HttpOnly SameSite=Strict cookie | Random 256-bit secrets need no slow KDF; id enables O(1) lookup without timing leaks | Passwords |

## 3. Components and process model

```
 Browser (WASM UI) ──HTTPS via reverse proxy or loopback HTTP──┐
                                                              ▼
 MCP clients ──stdio──> h2studio-mcp ──HTTP + caller token──> h2studio (server)
 (Claude Code, opencode,  (or streamable HTTP :8723)            │  REST /api/v1 · SSE · static UI · /metrics
  Codex, Cursor, ...)                                           │  auth · RBAC · audit · SQLite
                                                                │  job scheduler ─┬─ host: harness2 venv bins
                                                                │                 └─ container: docker/podman run
                                                                ▼                    runner image + egress proxy
                                     experiments/<name>/  (each = one HARNESS2_RUNS_DIR)
                                     harness2/            (managed checkout + .venv + dependencies/)
```

- `h2studio serve` — the server (default `127.0.0.1:8722`).
- `h2studio-mcp` — MCP server; stdio (default) or streamable HTTP (default `127.0.0.1:8723`).
- `h2studio egress-proxy` — allowlisting forward proxy used by container isolation.
- Other `h2studio` subcommands: `init`, `token create|list|revoke`, `doctor`, `config check`,
  `audit verify`, `openapi`, `version`.

## 4. Conventions

- JSON `snake_case`; enums are `snake_case` strings; timestamps RFC 3339 UTC strings; ids opaque
  strings (job ids are UUIDv7). Money: USD `f64`.
- Errors: every non-2xx JSON response is `ErrorBody` (`h2s_api::ErrorBody`) with an `ErrorCode`.
  Status mapping: 400 `bad_request`, 422 `validation` (with `details`), 401, 403, 404, 409
  `conflict`, 413, 421 `misdirected_request`, 429 `rate_limited`, 503 `unavailable`, 500.
- Lists that can grow without bound are `Page<T>` with an opaque `cursor`.
- Request ids: honour an incoming `x-request-id` only from trusted proxies, else generate; echo in
  the response header, error bodies, logs and audit records.
- No absolute host paths in responses to non-admins.
- Every identifier is validated with `h2s_core::names` before use; every filesystem path from a
  request or a results file is resolved through `cap_std` (never `Path::join` on user input).

## 5. Repository layout and ownership

One owner per path during the parallel build. Do not edit paths you do not own; ask the
coordinator (the `main` session) for any change to a LOCKED interface or to `Cargo.toml`
dependency lists.

| Path | Owner | Notes |
|---|---|---|
| `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `deny.toml`, `.cargo/` | coordinator | add dependencies only via the coordinator (cooldown check) |
| `crates/h2s-api/**` | coordinator | LOCKED contract |
| `crates/h2s-core/**` | **core agent** | public signatures LOCKED; implement bodies, add private helpers/tests freely |
| `crates/h2s-daemon/**` | **daemon agent** | |
| `crates/h2s-web/**`, `web/**`, `tools/h2s-mock-api/**` | **web agent** | |
| `crates/h2s-mcp/**` | **mcp agent** | |
| `crates/h2s-e2e/**`, `xtask/**`, `.github/**`, `.githooks/**`, `deploy/**` | coordinator | |
| `docs/**`, `README.md`, `CONTRIBUTING.md`, `SECURITY.md`, ... | docs agent (later) | agents may add notes to `docs/notes/<area>.md` |
| `tests/fixtures/**` | core agent (adds), others read | real harness2 output captured from the demo |

## 6. `h2s-core` interface (LOCKED)

See the doc comments in `crates/h2s-core/src/*.rs`. Summary:

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
`/h2s/experiment` for container jobs) or refuse.

## 8. REST API (`/api/v1`)

Unless noted, request/response bodies are `h2s_api` types. "Role" is the minimum role.
Unsafe methods (POST/PUT/PATCH/DELETE) require header `x-h2s-request: 1`.

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
| GET | `…/tasks/{task}/modes/{mode}/harnesses/{label}/export` | viewer | `?target=raw\|opencode\|codex&format=json\|tar.gz` | `ExportBundle` or `application/gzip` attachment |
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
| GET | `/jobs/{id}/log` | viewer | `?from=0&limit=500` (max 5000) | `LogChunk` |
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
Unsafe methods need `x-h2s-request: 1` and, when an `Origin` header is present, it must equal the
public origin, else 403. No CORS headers, ever. `X-Forwarded-*` honoured only from
`server.trusted_proxies`.

**Authentication.** `Authorization: Bearer h2s_<id>_<secret>` or the `h2s_session` cookie
(HttpOnly, SameSite=Strict, Path=/, Secure when `public_url` is https; 32-byte random id, stored
hashed, TTL from config, revoked with its token). Proxy mode: identity from configured headers,
accepted only from `trusted_proxies` AND with the shared-secret header matching; roles from group
mapping. `h2studio init` creates the first admin token (printed once, also written 0600 to
`<config_dir>/admin.token`). Login is rate-limited per IP.

**Authorization.** Role checks are route-level extractors (not only middleware); a test enumerates
the router's routes and asserts each declares a role. Job creation additionally checks
`JobSpec::required_role()`.

**Secrets.** Stored in the database (file mode 0600, directory 0700) or supplied as
`H2S_SECRET_<NAME>` environment variables (which take precedence and never touch disk). Write-only
API; responses carry `SecretInfo` only. Passed to jobs only through the child environment (never
argv) and only the secrets the job's profile references. Every job log line, error and audit detail
goes through the job's `Redactor`. The bridge key printed by `harness2-bridge` is captured into the
secret `HARNESS2_CODEX_API_KEY` and redacted before logging.

**Job processes.** argv only, never a shell; `env_clear()` + `BASE_ENV_PASSTHROUGH` + `JobEnv`;
own process group; cancellation = SIGTERM to the group, SIGKILL after 10 s; wall-clock timeout.
Generated files (OpenCode config, key file) live in a per-job 0700 dir, deleted when the job ends.

**Container isolation.** `docker|podman run` with `--rm --cap-drop ALL --security-opt
no-new-privileges --pids-limit`, CPU/memory limits, a non-root user, the job label, the configured
network (`none` or a pre-created network behind `h2studio egress-proxy`), mounts limited to the
experiment (rw), the harness2 home (ro except install/setup), and the job-files dir (ro). Label is
verified before any kill. The image is pinned by tag or digest from configuration.

**Egress proxy.** HTTP CONNECT + absolute-form HTTP; hostname allowlist (exact or `*.suffix`);
resolves and pins the address it checked; refuses loopback, private, link-local, CGNAT and metadata
addresses unless the allowlist entry is marked `allow_private`; logs each decision without bodies.

**Audit.** Every authentication outcome, job create/cancel, experiment/profile/secret/token change
and access denial is appended to a SHA-256 hash chain in the database; `h2studio audit verify`
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
paged by line number. Results: the kind's parser (`h2s_core::parse`) on the captured stdout and
the files the command writes; stored as JSON on the job row.

## 11. Configuration (`h2studio.toml`)

Location: `--config`, else `$H2S_CONFIG`, else `$XDG_CONFIG_HOME/harness2-studio/h2studio.toml`
(created by `h2studio init`). Unknown keys are an error. Environment overrides: `H2S_LISTEN`,
`H2S_DATA_DIR`, `H2S_PUBLIC_URL`, `H2S_LOG_FORMAT`, `H2S_OTLP_ENDPOINT`.

```toml
[server]
listen = "127.0.0.1:8722"
public_url = "http://127.0.0.1:8722"
allowed_hosts = []
trusted_proxies = []
max_body_bytes = 1048576
request_timeout_secs = 60
sse_max_connections = 64
login_attempts_per_minute = 10

[auth]
mode = "token"                       # token | proxy
session_ttl_hours = 12
[auth.proxy]
user_header = "x-forwarded-user"
groups_header = "x-forwarded-groups"
secret_header = "x-h2s-proxy-secret"
secret_env = "H2S_PROXY_SECRET"
admin_groups = []
operator_groups = []
viewer_groups = []                   # empty = any authenticated proxy user is a viewer

[paths]
data_dir = "~/.local/share/harness2-studio"   # studio.db, logs/, jobs/, splits/
experiments_dir = ""                 # default <data_dir>/experiments
harness2_home = ""                   # default <data_dir>/harness2
harness2_managed = true              # false = point at an existing checkout (never modified)

[jobs]
max_concurrent = 2
max_queued = 100
timeout_hours = 48
budget_poll_secs = 30
log_retention_days = 90

[isolation]
mode = "host"                        # host | container
preacknowledge_host_risks = false
[isolation.container]
runtime = "docker"                   # docker | podman
image = "harness2-studio-runner:0.1.0"
network = "none"
cpus = 4.0
memory_mb = 8192
pids_limit = 2048
user = ""

[egress]                             # used by `h2studio egress-proxy`
listen = "0.0.0.0:3128"
allow = []                           # ["aiplatform.googleapis.com", "*.googleapis.com", {host="litellm", allow_private=true}]

[telemetry]
log_format = "pretty"                # pretty | json
log_level = "info"
otlp_endpoint = ""                   # OTLP/HTTP, e.g. http://127.0.0.1:4318
service_name = "harness2-studio"
metrics_token_secret = ""            # secret NAME accepted as a bearer token on /metrics
```

## 12. Web UI (`h2s-web`)

Leptos 0.8 CSR compiled to `wasm32-unknown-unknown`, `wasm-bindgen --target web`, loaded by
`/app.js` (`import init from "/pkg/h2s_web.js"; init({ module_or_path: "/pkg/h2s_web_bg.wasm" })`).
`cargo xtask web` builds `dist/web/`. No inline `<script>`, no `style="..."` attributes (CSP); dynamic
sizes via SVG attributes or `style:prop` (CSSOM). Same-origin `fetch` with cookies; every unsafe
request sends `x-h2s-request: 1`; errors render `ErrorBody.error.message`.

Routes: `/login`, `/` (dashboard), `/welcome` (onboarding), `/experiments`, `/experiments/:exp`,
`/experiments/:exp/sweeps/:sweep` (tabs: tasks · report · library · audit),
`/experiments/:exp/sweeps/:sweep/tasks/:task` (timeline · harnesses + diff · rollouts · selection &
lesson), `/new` (run wizard: cell → options → preview → estimate → confirm), `/jobs`, `/jobs/:id`
(live log over SSE), `/compare`, `/benchmarks`, `/settings/{profiles,secrets,tokens,audit,system}`,
`/help`. Shell: sidebar nav, role-aware actions (hidden when not allowed, and the server refuses
anyway), theme toggle (light/dark/system), command palette (Ctrl+K), toasts, keyboard shortcuts,
`aria-live` status, WCAG 2.1 AA contrast, `prefers-reduced-motion`.

## 13. MCP server (`h2s-mcp`)

rmcp 3.5.1. Transports: stdio (default) and streamable HTTP (`--transport http --listen
127.0.0.1:8723`, Host/Origin checks, bearer required). Backend: the server's REST API at
`--server-url` (`H2S_SERVER_URL`, default `http://127.0.0.1:8722`). Credentials: stdio uses
`H2S_TOKEN` or `--token-file`/`H2S_TOKEN_FILE` (never a CLI argument); HTTP forwards each caller's
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
Resources: `h2s://jobs/{id}/log`, `h2s://experiments/{exp}/sweeps/{sweep}/report`,
`h2s://experiments/{exp}/manifest`. Prompts: `improve_agent_harness`, `analyze_sweep`,
`adopt_best_harness`. Tool outputs: JSON text (pretty, truncated with a note above 100 KB) plus
structured content where the SDK supports it; errors are tool errors with the server's message.

## 14. Observability

`tracing` everywhere (pretty or JSON); optional OTLP/HTTP trace export (`otel` feature); request
spans carry method, matched route, status, request id (never headers or bodies). Prometheus
`/metrics`: `h2s_http_requests_total{method,route,status}`, `h2s_http_request_duration_seconds`,
`h2s_jobs_total{kind,status}`, `h2s_jobs_running`, `h2s_job_duration_seconds{kind}`,
`h2s_spend_usd_total{experiment}` (bounded label set), `h2s_auth_failures_total{reason}`,
`h2s_sse_connections`.

## 15. Quality bar (every agent, every merge)

- `cargo fmt --check`; `cargo clippy --workspace --all-targets -- -D warnings`;
  `cargo clippy -p h2s-web --target wasm32-unknown-unknown -- -D warnings`; `cargo test --workspace`;
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

**v1.1 (2026-10-23) — server CLI for automation.** `h2studio serve` accepts port 0 and prints one
STDOUT line `h2studio listening on http://<ip>:<port>` once bound. `init` and `token create`
accept `--config` and print the new token as the only STDOUT line (explanations on stderr);
`init` refuses to overwrite an existing configuration unless `--force`.

**v1.1 (2026-10-23) — MCP argument names.** Tool arguments mirror the REST names:
`experiment`, `sweep` (summary namespace id), `task` (slug), `mode`, `label`, `from`/`to`, `path`,
`session` + `part`, `job_id`; `run_demo` takes the `DemoParams` fields flat; `wait_for_job` takes
`job_id` and `timeout_secs` (≤ 600) and returns the `Job`. Results are JSON (structured content or
the first text block); refusals set `is_error` and name the server's error code. Stdio
configuration: `H2S_SERVER_URL`, `H2S_TOKEN`.

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

**v1.1 (2026-10-23) — dependencies.** `tools/h2s-mock-api` may use `tokio-stream` (already in the
lockfile) to stream SSE.

**v1.2 (2026-10-23) — job egress.** Host-mode jobs also receive the server's own proxy and CA
variables when set (`HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, `NO_PROXY` and their lowercase
forms; `SSL_CERT_FILE`, `SSL_CERT_DIR`, `REQUESTS_CA_BUNDLE`, `NODE_EXTRA_CA_CERTS`; and every
`UV_*`, `PIP_*` and `NPM_CONFIG_*` variable, such as the runner image's
`UV_PYTHON_INSTALL_DIR` or a corporate index mirror: `h2s_core::env::is_passed_through`), so the
compose deployment, where the Studio sits on an internal network, reaches the allowlisted hosts
through `h2studio egress-proxy`. Profiles cannot set the uppercase proxy names (`RESERVED_ENV`).
Container jobs get none of the server's variables; `[isolation.container] proxy =
"http://host:port"` (no credentials, needs a network other than `none`) sets
`HTTPS_PROXY`/`HTTP_PROXY`/`https_proxy`/`http_proxy` and `NO_PROXY`/`no_proxy` =
`localhost,127.0.0.1,::1` inside the container as fixed `-e NAME=VALUE` arguments
(`ContainerSpec::env_values`; never secrets), not by name, so the runtime CLI never sees a proxy
that exists only on the jobs network.

**v1.2 (2026-10-23) — container HOME.** On a rootless runtime jobs run as container uid 0 with
every capability dropped, which cannot write `/home/node`. The runner image therefore sets
`HOME=/h2s/home`, a sticky world-writable directory in the throwaway container layer; the
all-in-one image sets `HOME=/home/node` back for the server user. Custom runner images must give
the job user a writable `HOME`.

**v1.2 (2026-10-24) — raw judge points.** `JudgedRow` and `BaseJudgedRow` gain
`points_passed`/`points_max: Option<f64>`, the points exactly as the judge wrote them (always set
when present). `n_passed`/`n_criteria` stay integers and are empty when the value is fractional,
as with JobBench's weighted judging; `frac` is computed from the raw values.

**v1.2 (2026-10-24) — profile check.** `GET /api/v1/profiles/{name}/check` (viewer) returns
`ProfileCheck { warnings, missing_secrets }`: `h2s_core::env::profile_warnings` and the referenced
secrets with no value (stored or `H2S_SECRET_<NAME>`), which job creation refuses with 422.
404 for an unknown profile. MCP tool `check_profile` (`profile`; read-only, viewer).
