# REST API

Everything the web UI and the MCP server do goes through this API, and anything they can do you
can do with `curl`. Base path: **`/api/v1`**. The wire types are Rust structs in the
`h2w-api` crate; the machine-readable description is an **OpenAPI 3.1** document the server
generates from the same code (section 9).

The examples below were run against `h2workbench` 0.1.0 and a real harness2 checkout (the offline
demo), and their output is pasted from those runs, trimmed. The server was bound to a random
port; they are shown against the default `http://127.0.0.1:8722`. Request ids, timestamps and
job ids differ on every run. Tokens are written `h2w_xxxxxxxx_<redacted>`.

```bash
BASE=http://127.0.0.1:8722
TOKEN=$(cat ~/.config/harness2-workbench/admin.token)   # printed by `h2workbench init`; see ADMIN-GUIDE.md
```

Contents: [conventions](#1-conventions) · [authentication](#2-authentication) ·
[headers and the CSRF rules](#3-headers-origin-and-csrf) · [roles and routes](#4-roles-and-routes) ·
[errors](#5-errors) · [paging](#6-paging-filters-and-limits) · [jobs](#7-jobs) ·
[live events](#8-live-events-sse) · [OpenAPI](#9-the-openapi-document) ·
[a curl walkthrough](#10-a-curl-walkthrough)

## 1. Conventions

- JSON in and out; field names and enum values are `snake_case`. Requests with a body need
  `Content-Type: application/json` (or `application/...+json`).
- Timestamps are RFC 3339 in UTC (`2026-10-24T02:44:07Z`); ids are opaque strings (job ids are
  UUIDv7); money is US dollars as a floating-point number.
- The design is that absolute paths of the server's file system appear only in admin-only fields
  (for example `data_dir` in `GET /system/info`), and job logs replace the server's directories
  with placeholders such as `<experiment>` and `<harness2>`. **The masking is incomplete**: the
  parsed `result` of a `check` job can carry the harness2 checkout's absolute path, and `file://`
  URLs in a log are not masked. See [THREAT-MODEL.md](THREAT-MODEL.md) (I4) for what it does and
  does not cover.
- Mode path segments and `/compare` references accept `parallel` and `sequential`, and also the
  on-disk tokens `agg` and `seq`.
- Scores: percent (0..100) in reports, fractions (0..1) in judge rows, task rows and comparisons
  (`Comparison.delta = b - a`, in fraction units). Judge rows carry `n_passed` / `n_criteria` as
  integers; when the judge wrote fractional points (JobBench's weighted judging) those are empty
  and `points_passed` / `points_max` hold the points exactly as written (contract addendum v1.2).
- Within a minor version the API changes only additively. Changes are recorded in
  [ARCHITECTURE.md](ARCHITECTURE.md) §16 and the [changelog](../CHANGELOG.md).

## 2. Authentication

Two credentials are accepted, checked in this order:

1. **`Authorization: Bearer h2w_<id>_<secret>`**: an API token. A malformed `Authorization`
   header is a `401`; it never falls through to the cookie.
2. **The session cookie**: `h2w_session`, or `__Host-h2w_session` when `server.public_url` is
   `https` (the server reads only the name it sets). `POST /auth/login` sets it; it exchanges a
   token for a browser session.

In `auth.mode = "proxy"` a third credential exists, identity headers from a trusted reverse
proxy; tokens keep working there. See [ADMIN-GUIDE.md](ADMIN-GUIDE.md).

A token is `h2w_` + an 8-character id (`a-z`, `0-9`) + `_` + a 43-character secret (`A-Z`, `a-z`,
`0-9`, `-`, `_`); anything else, any non-ASCII character included, is a `401` before the database is
asked. The server stores only the SHA-256 of the secret and compares in constant time. Create tokens with `h2workbench token create`
or `POST /tokens` (admin); the secret is shown **once**.

```console
$ curl -s -H "Authorization: Bearer $TOKEN" $BASE/api/v1/auth/me
{"id":"a7jbu6x4","name":"initial admin","role":"admin","kind":"token"}
```

**Sessions.** `POST /auth/login` with `{"token": "..."}` returns the same `Principal` with
`"kind":"session"` and sets a cookie that is `HttpOnly`, `SameSite=Strict` and `Path=/`. Over plain
http it is named `h2w_session`; when `server.public_url` is `https` it is named
`__Host-h2w_session` and is also `Secure` (browsers then refuse a `Domain` attribute or a cookie set
by a sibling subdomain, and the server ignores a plain `h2w_session`). It lasts
`auth.session_ttl_hours` (default 12) and never outlives its token; revoking a token ends its
sessions; `POST /auth/logout` deletes the session and clears the cookie. Logins are rate-limited
per client address (`server.login_attempts_per_minute`, a token bucket; IPv6 clients per /64):
beyond it the answer is `429` with `Retry-After`.

**Failed authentication is throttled.** Every refused credential (a malformed, unknown, wrong,
expired or revoked token, an ended session, bad proxy headers) is counted against the client's
address (IPv6 per /64). After 60 failures in a minute, further failing requests from that address get
`429` `rate_limited` (`too many attempts; try again in N s`) with `Retry-After`; requests with valid
credentials from the same address still succeed. A request that carries no credentials at all is
answered `401` `authentication required` and does not count towards the limit.

```console
$ curl -si -c cookies.txt -H "x-h2w-request: 1" -H "content-type: application/json" \
       -d "{\"token\":\"$TOKEN\"}" $BASE/api/v1/auth/login
HTTP/1.1 200 OK
content-type: application/json
set-cookie: h2w_session=<session-id>; HttpOnly; SameSite=Strict; Path=/; Max-Age=43200
x-request-id: aeaa2dc2f6a54a99bae06cb7bdc5f897
cache-control: no-store

{"id":"a7jbu6x4","name":"initial admin","role":"admin","kind":"session"}
```

Behind `server.public_url = "https://..."` the same header reads
`set-cookie: __Host-h2w_session=<session-id>; HttpOnly; SameSite=Strict; Secure; Path=/; Max-Age=43200`.

Prefer a bearer token for scripts: no cookie jar, and no Origin rule to think about.

## 3. Headers, Origin and CSRF

Every request goes through the same outer checks, in this order. They apply to the API, the
event streams and the static UI alike.

| Check | Rule | Refusal |
|---|---|---|
| `Host` | must be one the server answers to: the host of `server.public_url`, `server.allowed_hosts`, and `127.0.0.1:P`, `localhost:P`, `[::1]:P` for the bound port P. An absolute-form request target (`GET http://host/...`) must name an allowed host as well. More than one `Host` header is refused | `421` `misdirected_request`: `this server does not answer to that Host`; two `Host` headers: `400` `bad_request`: `the request has more than one Host header` |
| `x-h2w-request` | every method except `GET`, `HEAD` and `OPTIONS` must send `x-h2w-request: 1`, **including login, logout and requests that carry a bearer token** | `403` `forbidden`: `state-changing requests need the header x-h2w-request: 1` |
| `Origin` | when present on such a request, it must be the origin of `public_url`, or `http(s)://` + the request's own `Host`; `null` is refused. `curl` and SDKs send none | `403` `forbidden`: `cross-origin requests are refused` |
| body size | declared or streamed body at most `server.max_body_bytes` (1 MiB) | `413` `payload_too_large`: `the request body is larger than N bytes` |
| request time | at most `server.request_timeout_secs` (60 s) | `503` `unavailable`: `the request took too long` |

No CORS headers are ever sent, so a page on another origin cannot read responses or send the
custom header. DNS rebinding is blocked by the `Host` rule. Every response, errors and static
files included, carries `x-request-id` (honoured from the client only when it comes from a
trusted proxy), a strict `Content-Security-Policy` and the other security headers; `/api/*`,
`/healthz`, `/readyz` and `/metrics` are `Cache-Control: no-store`.

```console
$ curl -si -X POST -H "Authorization: Bearer $TOKEN" $BASE/api/v1/experiments -d '{"name":"x","kind":"standard"}' \
       -H "content-type: application/json"
HTTP/1.1 403 Forbidden
{"error":{"code":"forbidden","message":"state-changing requests need the header x-h2w-request: 1","request_id":"82d0fd7a6ca4443ba8bae21278fc60cb"}}
```

## 4. Roles and routes

Roles are ordered **viewer < operator < admin**; a role may do everything the roles below it may.

| Role | Can |
|---|---|
| viewer | read everything except secrets metadata, tokens and the audit log: experiments, results, jobs and their logs, profiles (without secret values), benchmarks, system info |
| operator | viewer, plus create and edit experiments, start jobs that use already-installed tooling (`demo`, `check`, `preview`, `run`, `judge`, `report`, `split`, `pipeline`), cancel them, refresh the doctor |
| admin | operator, plus install harness2, set up benchmarks, run the Codex bridge, manage profiles, secrets and tokens, read the audit log |

A job needs at least operator **and** the role its kind requires (`install_harness2`, `setup` and
`bridge` need admin). **Cancelling** a job needs the role that could have started it. The server
refuses to revoke the last active admin token through the API (`409`); the command line can.

Paths below are relative to `/api/v1`, except the three root routes. The table is generated from
the server's route table and OpenAPI document; a test walks the same table and proves each route
enforces the role written here.

### System, health and metrics

| Method | Path | Minimum role | Body | Success | What it does |
|---|---|---|---|---|---|
| GET | `/healthz` | none (public) |  | 200 Health | Liveness: the process answers. |
| GET | `/readyz` | none (public) |  | 200 Readiness | Readiness: the database answers and the job runner runs. |
| GET | `/metrics` | viewer, or the metrics token |  | 200 openmetrics-text | Prometheus metrics (OpenMetrics text). |
| GET | `/system/info` | viewer |  | 200 SystemInfo | Server status (admin-only fields filled for admins). |
| GET | `/system/doctor` | viewer |  | 200 DoctorReport | Environment checks (cached for 60 s). |
| GET | `/benchmarks` | viewer |  | 200 [BenchmarkStatus] | Benchmarks and their installation state. |
| GET | `/openapi.json` | viewer |  | 200 json | This API's OpenAPI 3.1 document. |

### Authentication

| Method | Path | Minimum role | Body | Success | What it does |
|---|---|---|---|---|---|
| POST | `/auth/login` | none (public) | `LoginRequest` | 200 Principal | Exchange a token for a session cookie (rate-limited per client address). |
| POST | `/auth/logout` | any signed-in caller |  | 204 | End the browser session (the cookie is cleared either way). |
| GET | `/auth/me` | any signed-in caller |  | 200 Principal | The authenticated caller. |

### Model profiles

| Method | Path | Minimum role | Body | Success | What it does |
|---|---|---|---|---|---|
| GET | `/profiles` | viewer |  | 200 [ModelProfile] | Every model profile (with the state of the secrets each references). |
| PUT | `/profiles/{name}` | admin | `ModelProfile` | 200 ModelProfile | Create or replace a profile. |
| DELETE | `/profiles/{name}` | admin |  | 204 | Delete a profile (409 while an experiment is pinned to it). |
| GET | `/profiles/{name}/check` | viewer |  | 200 ProfileCheck | What jobs with this profile would lack: its warnings and the referenced secrets that have no value (contract addendum v1.2). |

### Secrets (write-only)

| Method | Path | Minimum role | Body | Success | What it does |
|---|---|---|---|---|---|
| GET | `/secrets` | admin |  | 200 [SecretInfo] | Metadata of every secret (stored, from the environment, or referenced but unset). |
| PUT | `/secrets/{name}` | admin | `SetSecretRequest` | 200 SecretInfo | Set a secret value (write-only: only its metadata comes back). |
| DELETE | `/secrets/{name}` | admin |  | 204 | Delete a stored secret. |

### API tokens

| Method | Path | Minimum role | Body | Success | What it does |
|---|---|---|---|---|---|
| GET | `/tokens` | admin |  | 200 [TokenInfo] | Every token, newest first (never the secret). |
| POST | `/tokens` | admin | `CreateTokenRequest` | 201 TokenCreated | Create a token. |
| DELETE | `/tokens/{id}` | admin |  | 204 | Revoke a token; its sessions end at once. |

### Audit log

| Method | Path | Minimum role | Body | Success | What it does |
|---|---|---|---|---|---|
| GET | `/audit` | admin |  | 200 Page_AuditEvent | The audit log, newest first. |
| GET | `/audit/verify` | admin |  | 200 AuditVerification | Re-walk the audit hash chain. |

### Experiments

| Method | Path | Minimum role | Body | Success | What it does |
|---|---|---|---|---|---|
| GET | `/experiments` | viewer |  | 200 [Experiment] | Experiments with their sweeps (archived ones with `archived=true`). |
| POST | `/experiments` | operator | `CreateExperimentRequest` | 201 Experiment | Create an experiment (adopting an existing directory of that name). |
| GET | `/experiments/{exp}` | viewer |  | 200 Experiment | One experiment, with its sweeps and size. |
| PATCH | `/experiments/{exp}` | operator | `UpdateExperimentRequest` | 200 Experiment | Change description, tags or the archived flag. |
| GET | `/experiments/{exp}/manifest` | viewer |  | 200 Manifest | The experiment's reproducibility manifest. |

### Results of a sweep

| Method | Path | Minimum role | Body | Success | What it does |
|---|---|---|---|---|---|
| GET | `/experiments/{exp}/sweeps/{sweep}/tasks` | viewer |  | 200 Page_TaskRow | Task rows of a sweep. |
| GET | `/experiments/{exp}/sweeps/{sweep}/tasks/{task}` | viewer |  | 200 TaskDetail | Everything known about one task. |
| GET | `/experiments/{exp}/sweeps/{sweep}/tasks/{task}/modes/{mode}/harnesses/{label}` | viewer |  | 200 HarnessTree | One harness tree with file contents. |
| GET | `/experiments/{exp}/sweeps/{sweep}/tasks/{task}/modes/{mode}/diff` | viewer |  | 200 HarnessDiff | File-by-file diff between two harness labels. |
| GET | `/experiments/{exp}/sweeps/{sweep}/tasks/{task}/modes/{mode}/harnesses/{label}/export` | viewer |  | 200 gzip | A harness as project files: JSON, or a deterministic `.tar.gz` attachment. |
| GET | `/experiments/{exp}/sweeps/{sweep}/tasks/{task}/modes/{mode}/rollouts/{label}/files` | viewer |  | 200 [FileEntry] | List a rollout run directory. |
| GET | `/experiments/{exp}/sweeps/{sweep}/tasks/{task}/modes/{mode}/rollouts/{label}/file` | viewer |  | 200 FileContent | Read one rollout file (2 MiB cap). |
| GET | `/experiments/{exp}/sweeps/{sweep}/scores` | viewer |  | 200 Scores | `judged.json` and `base_judged.json`. |
| GET | `/experiments/{exp}/sweeps/{sweep}/report` | viewer |  | 200 ScoreReport | The score report computed from cached judge rows. |
| GET | `/experiments/{exp}/sweeps/{sweep}/library` | viewer |  | 200 [LibraryDomain] | Experience libraries of a sweep. |
| GET | `/experiments/{exp}/sweeps/{sweep}/audit` | viewer |  | 200 [AuditSession] | Improver and selector sessions of a sweep. |
| GET | `/experiments/{exp}/sweeps/{sweep}/audit/{session}/{part}` | viewer |  | 200 plain; charset=utf-8 | One text part (`mission`, `system`, `response`) of an audit session, as plain text. |
| GET | `/compare` | viewer |  | 200 Comparison | Per-task deltas between two (sweep, mode) results, with a sign test. |

### Planning and validation

| Method | Path | Minimum role | Body | Success | What it does |
|---|---|---|---|---|---|
| POST | `/estimate` | viewer | `EstimateRequest` | 200 CostEstimate | Size and cost of a sweep before running it. |
| POST | `/harness/validate` | viewer | `ValidateHarnessRequest` | 200 ValidationReport | Check a harness tree against harness2's own validation rules. |
| GET | `/splits` | viewer |  | 200 [SplitInfo] | Train/test split files created by `split` jobs. |

### Jobs

| Method | Path | Minimum role | Body | Success | What it does |
|---|---|---|---|---|---|
| GET | `/jobs` | viewer |  | 200 Page_Job | Jobs, newest first. |
| POST | `/jobs` | operator, plus the role the job kind needs | `JobSpec` | 201 Job | Start a job. Needs at least operator, and the role the job kind requires. |
| GET | `/jobs/{id}` | viewer |  | 200 Job | One job. |
| POST | `/jobs/{id}/cancel` | operator |  | 200 Job | Cancel a queued or running job (409 when it already finished). No request body. |
| GET | `/jobs/{id}/log` | viewer |  | 200 LogChunk | A page of the job's log. |
| GET | `/jobs/{id}/events` | viewer |  | 200 event-stream | Live job events as server-sent events. |

Notes on single routes:

- `GET /system/doctor?refresh=true` runs the probes now instead of using the 60-second cache
  and needs operator.
- `GET /system/info` includes `isolation` (`host` or `container`), `container_runtime` (set when
  container isolation is configured and the runtime answers) and `host_risks_preacknowledged`
  (`true` when the administrator set `isolation.preacknowledge_host_risks`, so jobs need no
  `acknowledge_risks`).
- `GET /metrics` accepts a viewer credential, or, when `telemetry.metrics_token_secret` names a
  secret, that secret's value as a bearer token. The metrics token unlocks `/metrics` and nothing
  else.
- `POST /experiments` adopts an existing directory of that name under the experiments directory
  (it must be a real directory, not a symlink).
- `GET /profiles/{name}/check` (viewer) tells you, before you start a job, what the profile
  cannot do (`warnings`: the same texts the server logs when a profile is saved, see
  [MODELS.md](MODELS.md#warnings-the-server-raises)) and which referenced secrets have no value
  (`missing_secrets`; a job with such a profile is refused with `422`). It never returns a secret
  value. `404` for an unknown profile, `422` for a malformed name. A real answer for an
  OpenAI-compatible profile whose key was not set yet:

  ```console
  $ curl -s -H "Authorization: Bearer $TOKEN" $BASE/api/v1/profiles/local-vllm/check
  {"warnings":["WorkBuddy-Bench cannot use this profile: its setup requires a google-vertex/gemini executor, a gemini judge and Vertex AI settings.","The Codex executor (substrate cx) needs a Responses-API endpoint: set the profile's Codex endpoint (for Vertex AI, run the bridge job and use its URL and key)."],"missing_secrets":["VLLM_API_KEY"]}
  ```
- `GET .../tasks/{task}/modes/{mode}/harnesses/{label}/export` takes `target=raw|opencode|codex`
  and `format=json|tar.gz`; the archive is deterministic and arrives as `application/gzip` with a
  `Content-Disposition` attachment name.
- `GET .../rollouts/{label}/file` returns at most 2 MiB; rollout browsing never follows symlinks.

## 5. Errors

Every non-2xx response with a JSON body, including `404` and `405` for unknown routes and
methods, has this shape:

```json
{
  "error": {
    "code": "validation",
    "message": "params.k: must be between 1 and 8",
    "details": [{ "field": "params.k", "problem": "must be between 1 and 8" }],
    "request_id": "4a99dffe6ceb497fa24f49b99acb49b7"
  }
}
```

`details` appears for validation errors (one entry per problem); `request_id` is also the
`x-request-id` response header and appears in the server's log and the audit record. Messages
are written for people and never contain secret values or quoted request input.

| Status | `code` | Typical causes (exact messages are in [TROUBLESHOOTING.md](TROUBLESHOOTING.md)) |
|---|---|---|
| 400 | `bad_request` | body is not JSON or not `application/json`; unknown job `kind`; more than one `Host` header |
| 405 | `bad_request` | the path exists but not for that method (the message is `method not allowed`; an `Allow` header lists the right ones) |
| 401 | `unauthorized` | no or invalid credentials; expired or revoked token; ended session; in proxy mode a user, groups or secret header sent more than once |
| 403 | `forbidden` | role too low (a job start says `starting a job of kind <kind> requires the <role> role`); missing `x-h2w-request`; foreign `Origin`; a proxy user whose groups grant no role |
| 404 | `not_found` | unknown route, experiment, sweep, job, token |
| 409 | `conflict` | experiment exists; wrong profile for a pinned experiment; archived experiment; job already finished; container isolation refuses WorkBuddy and the bridge |
| 413 | `payload_too_large` | body over `server.max_body_bytes`; or a results read past one of the server's memory bounds (`too large: ...`, see section 6) |
| 421 | `misdirected_request` | unknown `Host` |
| 422 | `validation` | a field is wrong: `details` names it. Also unknown or unparsable **query parameters** (field `query`) |
| 429 | `rate_limited` | too many login attempts; too many failed authentications from one address; a caller that already holds `server.sse_max_per_principal` event streams; `Retry-After` says how long |
| 503 | `unavailable` | harness2 not installed; job queue full; too many event streams; database unavailable; request timed out |
| 500 | `internal` | a bug or an I/O failure; the message is generic, the cause is in the server log under the request id |

```console
$ curl -s -X POST -H "Authorization: Bearer $OPERATOR" -H "x-h2w-request: 1" -H "content-type: application/json" \
       -d '{"kind":"run","params":{"experiment":"lab-glm-k1","cell":{"bench":"lab"},"options":{"k":1,"modes":["parallel"]},"profile":"local-glm"}}' \
       $BASE/api/v1/jobs
{"error":{"code":"validation","message":"params.acknowledge_risks: jobs run on the server host without isolation (the executor gets the server user's files and network); set acknowledge_risks to true to proceed","details":[{"field":"params.acknowledge_risks","problem":"jobs run on the server host without isolation (the executor gets the server user's files and network); set acknowledge_risks to true to proceed"}],"request_id":"de7c630e310042e6955e73812d33babb"}}
```

## 6. Paging, filters and limits

Lists that can grow without bound return a **page**:

```json
{ "items": [ ... ], "next_cursor": "Mzc" }
```

Pass `next_cursor` back as `cursor` for the next page; it is absent on the last page. Cursors are
opaque: do not build them. An invalid one is `422` (`cursor: invalid cursor`). Query parameters
are strict: an unknown name or a value that does not parse is a `422` on the field `query`.

| Route | Filters | `limit` default / maximum |
|---|---|---|
| `GET /jobs` | `status`, `kind`, `experiment` | 50 / 500 |
| `GET /audit` | none | 100 / 500 |
| `GET .../tasks` | `domain`, `status` (`ok`, `skipped`, `partial`, `base-failed`, `running`, `exception`, `too-large`, `unreadable`) | 200 / 1000 |
| `GET /jobs/{id}/log` | `from` (0-based line), `limit` | 500 / 5000 |

Other bounds: request body 1 MiB (configurable); rollout and audit-text files 2 MiB; a job log line
64 KiB (longer lines are cut with ` … [line truncated]`); **reads of files that jobs may rewrite**
(`413` `too large: ...` past a bound, nothing partial): a `summary.json` 4 MiB (in a task list a
larger one appears as a row with status `too-large`, and a file that cannot be read as
`unreadable`), 10,000 tasks per sweep, 256 MiB of summaries per list view, 100,000 judged rows, a
harness tree or export 2,000 files and 64 MiB, any directory listing 250,000 entries (the full list
is in `docs/notes/core.md`); `POST /harness/validate` at most 5,000
files; secret values at most 64 KiB; experiment descriptions 4,000 characters and 32 tags. At most
four results reads run at once; the others wait for a permit within the request timeout. A sweep's
task list is cached until its files change (five minutes at most), and a sweep that cannot be read
is left out of the sweep list instead of failing it.

## 7. Jobs

Every harness2 operation is a **job**: it is queued, runs as separate processes with its own log,
and ends with a status and a parsed result. `POST /jobs` takes `{"kind": ..., "params": {...}}`
and returns `201` with the job in state `queued`.

| `kind` | Runs | Role | `params` (besides `experiment`) |
|---|---|---|---|
| `demo` | `harness2-demo`: the real workflow with deterministic local editors; no credentials, no network | operator | `k` (1..8, default 2), `mode` (`parallel`, `sequential`, `both`) |
| `check` | `harness2-check`: offline readiness | operator | `cell`, `profile` (default: the profile named `default`, if any) |
| `preview` | `harness2 --dry-run`: task stream, namespaces, estimate; no model calls | operator | `cell`, `options`, `profile` |
| `run` | `harness2`: the refinement sweep (**costs money** with cloud models) | operator | `cell`, `options`, `profile`, `budget_usd`, `acknowledge_risks` |
| `judge` | `harness2-judge` | operator | `cell`, `k`, `seed`, `modes`, `bases`, `profile` |
| `report` | `harness2-report` | operator | `cell`, `ks`, `seed`, `modes` |
| `pipeline` | preview, check, run, judge, optionally judge `bases`, report; stops at the first failing step | operator | like `run`, plus `judge_bases` |
| `split` | a reproducible train/test partition | operator | `bench`, `subset`, `seed`, `train_fraction`, `name` |
| `setup` | `harness2-setup`: clone and build a benchmark (network-heavy) | admin | `bench`, `subset`, `substrate`, `dry_run`, `profile` |
| `install_harness2` | fetch harness2 at the tested commit (or the commit `rev` names) and install it | admin | `rev`, `extras`, `reinstall` |
| `bridge` | `harness2-bridge`: a Responses-API endpoint on Vertex AI for Codex; runs until cancelled | admin | `profile`, `model`, `port`, `expose_to_containers` |

`rev` must be the tested commit (the default) unless an administrator has set
`paths.allow_untested_harness2_rev = true` in the configuration; otherwise any other value is a `422`
on `params.rev`: `only the tested harness2 commit <sha> can be installed; an administrator can
allow others with paths.allow_untested_harness2_rev = true`. With the switch on, `rev` accepts any
full 40-character commit id and the job builds what it finds there (use only commits you trust).

`cell` is `{"bench": "lab"}` (also `"substrate": "oc"|"cx"`, and `"subset"` for `wb`);
`options` is `{"k": 1, "modes": ["parallel"], "seed": 7, "split": "all", "workers": 1,
"only_domains": [], "max_tasks": 10}` where only `k` and `modes` are required. Defaults and ranges:
`k` 1..32, `workers` 1..64, `max_tasks` 1..100000.

**Validation before `201`**, in this order: role; the spec's own rules; identifier rules;
container-isolation limits (`409`); host isolation needs `acknowledge_risks: true` on `run` and
`pipeline` unless the server pre-acknowledges (`422`); harness2 installed (`503`); the experiment
exists (`422`; a `demo` creates its experiment), is the right kind (`409`), is not archived
(`409`) and is pinned to the same profile (`409`); the profile exists and its secrets are set
(`422` naming the secret); the split file exists and matches the cell; the queue has room (`503`).

**Scheduling.** First in, first out within `jobs.max_concurrent` (default 2); one job that
writes (`demo`, `run`, `judge`, `pipeline`) per experiment at a time, later ones wait;
`install_harness2` and `setup` run alone; the bridge is a service, one at a time, outside the
concurrency count.

**Status.** `queued`, `running`, then one of `succeeded`, `partial` (harness2 exit code 2:
finished, but some tasks failed or stayed ungraded), `failed`, `cancelled`. A job's `error` is a
one-line reason: `cancelled by <name>`, `budget reached`, `timed out after 1h 30m`,
`server restarted`, `server stopped`, `stopped: less than N MiB free on the data directory` (see
`jobs.min_free_disk_mb`), or `exited with code N: <last stderr line>` (`exited with a signal` when a
signal ended it; a pipeline prefixes `step <name> failed: `).

A job's stored log stops at `jobs.max_log_bytes` (256 MiB by default): the log then holds one line
saying so, the job goes on, and its last line says how many lines and bytes of output were not
stored (`N lines (X) of output were not stored (log limit)`).

A real demo job, created and read back (trimmed):

```console
$ curl -s -X POST -H "Authorization: Bearer $TOKEN" -H "x-h2w-request: 1" -H "content-type: application/json" \
       -d '{"kind":"demo","params":{"experiment":"demo","k":2,"mode":"both"}}' $BASE/api/v1/jobs
{"id":"01a16bcb-4ae0-7322-b121-6bf7509bb725","kind":"demo","spec":{"kind":"demo","params":{"experiment":"demo","k":2,"mode":"both"}},"status":"queued","created_by":"ci operator","created_at":"2026-10-24T02:44:07Z","isolation":"host","steps":[],"log_lines":0}

$ curl -s -H "Authorization: Bearer $TOKEN" $BASE/api/v1/jobs/01a16bcb-4ae0-7322-b121-6bf7509bb725
{ "id": "01a16bcb-...", "kind": "demo", "status": "succeeded", "exit_code": 0, "isolation": "host",
  "steps": [{ "name": "demo", "status": "succeeded", "exit_code": 0 }],
  "progress": { "done": 2, "failed": 0, "current": "accounting / ledger-2" },
  "result": { "kind": "demo", "done": 2, "failed": 0, "execution_count": 28, "editor_calls": 32 },
  "log_lines": 7 }
```

The **log** pages by line number; `next` is the `from` to continue with, and `complete` is true
once the job is finished and every line has been returned. Secret values were replaced with
`[REDACTED:<NAME>]` before the line was stored; that is best effort (a value a job transforms is not
recognised), and **every viewer can read every job's log**.

```console
$ curl -s -H "Authorization: Bearer $TOKEN" "$BASE/api/v1/jobs/$ID/log?from=0&limit=3"
{ "lines": [
    { "n": 0, "ts": "2026-10-24T02:44:07.521Z", "stream": "system", "text": "job 01a16bcb-... (demo) started; isolation: host" },
    { "n": 1, "ts": "2026-10-24T02:44:07.521Z", "stream": "system", "text": "▶ demo: harness2-demo --output <experiment> --k 2 --mode both" },
    { "n": 2, "ts": "2026-10-24T02:44:07.916Z", "stream": "stdout", "text": "  [accounting] 1/2 ok           ledger-1" } ],
  "next": 3, "complete": false }
```

`stream` is `stdout`, `stderr`, or `system` (lines the server writes itself: step boundaries,
isolation notes, cancellation).

**Cancelling.** `POST /jobs/{id}/cancel` (no body). A queued job is cancelled at once; a running
job gets `SIGTERM` to its process group, `SIGKILL` after 10 seconds (a container is stopped by
label). A finished job answers `409` `job <id> already finished (succeeded)`.

**Estimating before running.**

```console
$ curl -s -X POST -H "Authorization: Bearer $TOKEN" -H "x-h2w-request: 1" -H "content-type: application/json" \
       -d '{"tasks":20,"k":2,"modes":["parallel","sequential"],"profile":"local-glm"}' $BASE/api/v1/estimate
{ "tasks": 20, "modes": 2, "k": 2, "rollouts": 280, "improver_sessions": 320, "basis": "heuristic",
  "notes": [ "Per task: 2 reference run(s) shared by the modes; per mode 6 candidate and final runs and 6 improver sessions.",
             "Each task and mode adds one selector session (k > 1).", ...,
             "No prices in the profile and no earlier runs in this experiment: no cost figure." ] }
```

## 8. Live events (SSE)

`GET /jobs/{id}/events` streams a job's progress as server-sent events (contract addendum v1.1).

- **Frames are unnamed**: there is no `event:` line, so a browser `EventSource.onmessage`
  receives every frame. Each frame carries exactly **one `JobEvent` JSON object** in its `data:`.
- The object's `type` is one of `snapshot`, `status`, `log`, `progress`, `step`, `heartbeat`.
- The **first frame is always a `snapshot`** of the whole job. A `heartbeat` follows at least
  every 15 seconds. `log` frames carry `id: <line number>`.
- **Resuming**: reconnect with `Last-Event-ID: N` to get a fresh snapshot and then the log lines
  `N+1` onward (from memory or from the log file).
- The stream **ends** after the terminal `status` frame, after 10 minutes with nothing but
  heartbeats, after 1 hour, or when the server shuts down; clients reconnect. A job that has
  already finished gets a snapshot (and any replay) and the stream closes. A client that falls
  behind gets a fresh snapshot; page the log from its `log_lines`.
- At most `server.sse_max_connections` streams (default 64) are open at once; more get `503`
  `too many open event streams; try again later`. One caller (a token, a session's token, a proxy
  user) may hold `server.sse_max_per_principal` of them (default 8); a ninth gets `429`
  `this caller already has 8 open event streams; close one first` with `Retry-After: 5`.
- The credential is checked when the stream opens and again at every heartbeat: if the token is
  revoked, expires or the session ends, the stream closes at the next heartbeat (within 15 seconds).
  Proxy-mode users have nothing to revoke; their streams end at the idle or lifetime limit.
- The response carries `x-accel-buffering: no`; a reverse proxy must not buffer it (Caddy:
  `flush_interval -1`; nginx: `proxy_buffering off`). Unknown comment lines (`: ...`) should be
  ignored.

A real stream of the demo job above (long lines cut):

```console
$ curl -sN -H "Authorization: Bearer $TOKEN" $BASE/api/v1/jobs/$ID/events
data: {"type":"snapshot","job":{"id":"01a16bcb-4ae0-7322-b121-6bf7509bb725","kind":"demo","status":"running", ...}}

data: {"type":"progress","progress":{"done":1,"failed":0,"current":"accounting / ledger-1"}}

data: {"type":"log","line":{"n":2,"ts":"2026-10-24T02:44:07.916Z","stream":"stdout","text":"  [accounting] 1/2 ok           ledger-1"}}
id: 2

data: {"type":"log","line":{"n":4,"ts":"2026-10-24T02:44:08.283Z","stream":"stdout","text":"Local demo: 2 tasks completed; 0 failed. <experiment>/demo.json"}}
id: 4

data: {"type":"step","step":{"name":"demo","status":"succeeded","exit_code":0,"started_at":"2026-10-24T02:44:07Z","finished_at":"2026-10-24T02:44:08Z"}}

data: {"type":"status","status":"succeeded","exit_code":0,"error":null}
```

Resuming a finished job's stream after line 3 (`Last-Event-ID: 3`) returns the snapshot and then
only lines 4, 5 and 6, and closes.

`curl -N` is enough for a script. `EventSource` cannot set an `Authorization` header, which is
why the web UI uses the session cookie; from a script, use a bearer token and read the stream
with any HTTP client that does not buffer the whole body.

## 9. The OpenAPI document

```console
$ h2workbench openapi > openapi.json            # needs no configuration and no running server
$ python3 -c 'import json; d=json.load(open("openapi.json")); print(d["openapi"], len(d["paths"]), "paths")'
3.1.0 43 paths
```

The running server serves the same document at `GET /api/v1/openapi.json` (viewer). It describes
the 49 operations of the table above, 134 schemas, and the two security schemes (`bearer`, and the
session cookie, `h2w_session` or `__Host-h2w_session` over https). No Swagger UI is bundled, because the usual one loads a script from a CDN
and the server's Content-Security-Policy forbids that; open the file in any OpenAPI viewer or
feed it to a client generator (not tried by the authors of this page).

The OpenAPI document does not list the `x-h2w-request` header per operation; it states the rule
in its description. Generated clients must add the header to every state-changing call.

## 10. A curl walkthrough

```bash
BASE=http://127.0.0.1:8722
AUTH="Authorization: Bearer $TOKEN"      # an operator or admin token
H='x-h2w-request: 1'; J='content-type: application/json'

# 1. who am I, and what is the server like?
curl -s -H "$AUTH" $BASE/api/v1/auth/me
curl -s -H "$AUTH" $BASE/api/v1/system/info

# 2. run the free demo and wait for it
ID=$(curl -s -X POST -H "$AUTH" -H "$H" -H "$J" \
       -d '{"kind":"demo","params":{"experiment":"demo","k":2}}' $BASE/api/v1/jobs | python3 -c 'import sys,json; print(json.load(sys.stdin)["id"])')
curl -sN -H "$AUTH" $BASE/api/v1/jobs/$ID/events | grep -m1 '"type":"status"'

# 3. look at what harness2 wrote
curl -s -H "$AUTH" $BASE/api/v1/experiments
curl -s -H "$AUTH" $BASE/api/v1/experiments/demo/sweeps/demo-oc-agg2/tasks

# 4. what changed in the harness, and take it with you
curl -s -H "$AUTH" "$BASE/api/v1/experiments/demo/sweeps/demo-oc-agg2/tasks/ledger-1/modes/parallel/diff?from=base&to=agg1"
curl -s -H "$AUTH" -o harness.tar.gz \
  "$BASE/api/v1/experiments/demo/sweeps/demo-oc-agg2/tasks/ledger-1/modes/parallel/harnesses/agg1/export?target=opencode&format=tar.gz"
tar tzf harness.tar.gz
```

The last command printed, in the run these examples come from:

```text
harness-agg1/AGENTS.md
harness-agg1/scripts/summarize.py
```

and the diff of the demo's `scripts/summarize.py` between `base` and `agg1` showed the demo editor
removing an `if amount > 0` filter, with every other file `unchanged`:

```json
{ "path": "scripts/summarize.py", "component": "scripts", "change": "modified",
  "unified": "--- base/scripts/summarize.py\n+++ agg1/scripts/summarize.py\n@@ -4,5 +4,5 @@ ...",
  "lines_added": 1, "lines_removed": 1 }
```

A `GET /experiments` after that run lists the demo experiment with its sweep
(`id: demo-oc-agg2`, `modes: [parallel, sequential]`, `tasks: 2`, `ok: 2`, `usd: 0.0`). The
sweep id is the summary namespace; see [CONCEPTS.md](CONCEPTS.md).
