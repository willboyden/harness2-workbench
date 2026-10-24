# Server (`h2studio`): behaviour notes

The server owns jobs, state, authentication and the audit log (ARCHITECTURE.md §3). This file
records what the contract leaves open and how the server decides it, so the web UI, the MCP
server, the end-to-end suite and administrators can rely on it. Everything here is tested in
`crates/h2s-daemon` (unit tests next to the code, integration tests in `tests/`) unless it says
otherwise.

Status (2026-10-23): every route of §8 is implemented against the real `h2s-core`. Tested with
real subprocesses (stub harness2 executables that print upstream's captured output formats),
real sockets, the real harness2 fixtures and a local OTLP/HTTP collector; the real
`harness2-demo` ran through `h2studio serve` against an unmodified harness2 checkout, by hand
and in the `h2s-e2e` server and MCP suites. Not exercised: container isolation end to end (the
`docker`/`podman run` argv is tested, no container was started) and TLS (served by a reverse
proxy, not by this server).

## Command line

All commands take `--config <path>` anywhere on the command line (else `$H2S_CONFIG`, else
`$XDG_CONFIG_HOME/harness2-studio/h2studio.toml`). Standard output carries only machine-readable
results; explanations and logs go to standard error.

| Command | Standard output |
|---|---|
| `h2studio serve` | exactly one line once the socket is bound: `h2studio listening on http://<ip>:<port>` (IPv6 as `http://[::1]:<port>`). Port 0 is allowed (`listen = "127.0.0.1:0"` or `H2S_LISTEN=127.0.0.1:0`). SIGINT/SIGTERM stop it gracefully: no new connections, event streams end, running jobs are stopped (SIGTERM, SIGKILL after 10 s) and marked failed ("server stopped"), queued jobs stay queued. |
| `h2studio init [--data-dir P] [--force]` | the new admin token as the only line. |
| `h2studio token create --name N --role R [--expires-in-days D]` | the new token as the only line (`TOKEN=$(h2studio token create ...)` works). |
| `h2studio token list [--json]` | a table, or JSON `TokenInfo`s (never secrets). |
| `h2studio token revoke ID` | nothing; exit 1 when no active token has that id. |
| `h2studio doctor [--json]` | the checks; exit 1 when one fails. |
| `h2studio config check` | `configuration OK` and the resolved paths; problems on stderr, exit 1. |
| `h2studio audit verify` | `audit chain OK: N entries; head <seq> <hash>` or `audit chain BROKEN: first bad record S of N entries` (exit 1). |
| `h2studio openapi` | the OpenAPI 3.1 document. |
| `h2studio version` | `h2studio <version>` (and the commit when built with `H2S_BUILD_COMMIT`). |
| `h2studio egress-proxy` | one line `h2studio egress proxy listening on <ip>:<port>`. |

`init`:

- creates the configuration file only when it does not exist (0600, its directory 0700), with
  `paths.data_dir` set to `--data-dir`, else `$H2S_DATA_DIR`, else the XDG data directory; an
  existing configuration is used as it is and **never rewritten** (not even with `--force`);
- creates the data directories and the database, then the bootstrap admin token
  (`initial admin`), printed once and written 0600 to `<config dir>/admin.token`;
- refuses (exit 1, nothing on stdout) when an active admin token already exists, unless
  `--force`, which mints another bootstrap token.

The CLI and a running server can use the same database at once (WAL, 10 s busy timeout). CLI
token actions are audited with the actor `cli`.

`serve` and `egress-proxy` first make themselves non-dumpable (`prctl(PR_SET_DUMPABLE, 0)`): no
core dumps, and no other process of the same user, host-mode jobs included, can read their
`/proc/<pid>/environ` (`H2S_SECRET_*`, the proxy secret) or memory, or ptrace them. Jobs
themselves stay dumpable (`execve` resets the attribute). This does not protect files: a
host-mode job runs as the server's user and can open `studio.db`; only container isolation
separates them. Every data directory (the data directory itself, `logs`, `logs/jobs`, `jobs`,
`splits`, `experiments`) is set to 0700 at startup, also when it already exists (a volume).

## Configuration

`h2studio.toml` as in §11, with these precisions: empty `paths.*` mean the XDG data directory
(`$XDG_DATA_HOME/harness2-studio`, else `~/.local/share/harness2-studio`), `<data>/experiments`
and `<data>/harness2`; relative XDG values are ignored (the XDG rule) and values pointing into a
snap or Flatpak sandbox are logged as warnings at startup, as are the resolved paths;
`jobs.timeout_hours` accepts fractions; `isolation.container.user` must be numeric (`uid` or
`uid:gid`) and `image` pinned (`name:tag` other than `latest`, or `name@sha256:<64 hex>`, name in
lowercase); `network` is `none` or a user-defined network (never `host`/`bridge`/`container:`).
Proxy mode needs `server.trusted_proxies` and the shared secret (at least 16 characters) in the
environment variable `auth.proxy.secret_env` when the server starts.

Keys beyond §11 (all optional, defaults shown):

| Key | Default | Meaning |
|---|---|---|
| `paths.allow_untested_harness2_rev` | `false` | `install_harness2` with a `rev` other than the tested commit (`h2s_core::TESTED_HARNESS2_COMMIT`) is 422 on `params.rev` unless `true`: a fork can make any commit fetchable from the upstream repository, and installing runs its build code. |

## HTTP

- **Host allowlist**: `public_url`'s host (with its port; without a port when it is the scheme's
  default), `server.allowed_hosts` (an entry without a port matches any port), and
  `127.0.0.1:P`, `localhost:P`, `[::1]:P` for the bound port P (plus the bound address when it
  is a specific non-loopback IP). Anything else is 421, static files included. A request with
  more than one `Host` header is 400 (RFC 9112 §3.2); an absolute-form target
  (`GET http://host/...`) must name an allowed host as well as its `Host` header.
- **Unsafe methods** (everything but GET/HEAD/OPTIONS) need `x-h2s-request: 1`. An `Origin`, when
  sent, must be `public_url`'s origin or `http(s)://<the request's Host>` (so a UI opened at
  `http://localhost:P` works); `null` is refused. No CORS headers are ever sent.
- **Errors**: every error response is `ErrorBody` with `request_id`, including 404/405 for unknown
  routes and methods, 413, 421 and rejections from extractors. Status mapping beyond §4: unknown
  query parameters, unknown enum values and unparsable numbers in a query are **422** (field
  `query`, or the parameter's name); a body that is not JSON (or not `application/json`) is 400;
  `CoreError::Refused` is 400; `CoreError::NotFound` 404; a request over
  `server.request_timeout_secs` is 503.
- **Request ids**: `x-request-id` is honoured only from `trusted_proxies` and only when it is
  1..=128 characters of `[A-Za-z0-9._:-]`; otherwise a new id is generated.
- **Static UI**: `/`, `/app.js`, `/styles.css`, `/favicon.svg`, `/pkg/**` from `dist/web` embedded
  at build time; other non-API GETs return `index.html`. Without `dist/web` the server builds and
  serves a page saying `UI not built — run cargo xtask web`. `Cache-Control: no-cache` with an
  `ETag` for everything except content-hashed names (`name-<8+ hex>.ext`), which are `immutable`.
- **Metrics** route labels are matched route templates (`/api/v1/jobs/{id}`), `unmatched` or
  `static`; unknown methods are `OTHER`; at most 50 `experiment` labels, then `other`.

## Authentication and authorization

- Order: `Authorization: Bearer` (a malformed header is 401, never a fall-through), then the
  `h2s_session` cookie, then (proxy mode) the proxy headers. Tokens keep working in proxy mode.
- A presented token must be exactly `h2s_` + 8 of `[a-z0-9]` + `_` + 43 of `[A-Za-z0-9_-]`;
  anything else, including any non-ASCII character, is malformed (401), at login and as a bearer.
- Sessions last `auth.session_ttl_hours` and never outlive their token; revoking a token ends its
  sessions; logout deletes the session and clears the cookie (`Max-Age=0`).
- Login is rate-limited per client address (IPv6 per /64): `server.login_attempts_per_minute`
  attempts, successful or not, then 429 with `Retry-After`. End-to-end suites that sign in more
  often should raise the limit in their configuration. Each limiter tracks up to 10,000 clients;
  when its table is full, clients whose bucket has refilled are forgotten first and newcomers
  that still do not fit share one overflow bucket, so the limit never fails open.
- Proxy mode roles: any `admin_groups` member is admin, else any `operator_groups` member is
  operator, else viewer when `viewer_groups` is empty or matches, else 403. Requests carrying the
  user header from an address outside `trusted_proxies`, or with a wrong shared secret, are 401.
  So is a request in which the user, groups or secret header appears more than once: the proxy
  must remove whatever the client sent under those names and set each exactly once.
- `POST /jobs` needs at least operator, then `JobSpec::required_role()`. **Cancelling** a job
  needs the role that could start it (operator for most kinds, admin for install, setup and the
  bridge).
- `DELETE /tokens/{id}` refuses (409) to revoke the last active admin token; the CLI can.
- **Failed authentication** with credentials (malformed, unknown or wrong tokens, rejected proxy
  headers): per client address (IPv6 per /64), 60 a minute are answered 401 and the rest 429
  with `Retry-After`; valid credentials from the same address keep working. Of those failures
  and of denied logins, at most 10 per client and minute are written to the audit log; the rest
  are counted in `h2s_audit_suppressed_total`, so junk credentials cannot flood the hash-chained
  log. Requests without credentials are never audited. Every refused request is counted once in
  `h2s_auth_failures_total{reason}` (`rate_limited` for the 429s).
- Audited: `auth.login` (success; denied and rate-limited attempts within the limit above),
  `auth.logout`, `auth.failed` (within the limit above), `access.denied`, `job.create`
  (success and denied, with the reason), `job.cancel`, `experiment.create|update`,
  `profile.put|delete` (with `env::profile_warnings` in the detail), `secret.set|delete|capture`
  (the length, never the value), `token.create|revoke`.

## Jobs

- **Scheduling**: FIFO within `jobs.max_concurrent`; one writing job (demo, run, judge, pipeline)
  per experiment, later writers of the same experiment wait; `install_harness2` and `setup` run
  alone (nothing starts beside them, and once one waits nothing later starts first); bridges are
  services: at most one at a time, not counted against `max_concurrent`. The queue holds
  `jobs.max_queued` jobs; more is 503.
- **Validation before 201** follows §8 in this order: role, structure, identifiers (h2s-core),
  isolation rules (WorkBuddy and the bridge refuse container isolation: 409; host isolation needs
  `acknowledge_risks` on run/pipeline unless pre-acknowledged: 422), harness2 installed
  (`<home>/.venv/bin/harness2` exists; 503), experiment (missing: 422, except the demo, which
  creates a demo experiment; a demo on a standard experiment or a benchmark job on a demo
  experiment: 409; writing to an archived experiment: 409; another pinned profile: 409), profile
  and its secrets (422 naming the secret), split file (exists and matches the cell), queue cap.
  The first writing job with a profile pins it to an experiment that has none.
- `check` without a profile uses the profile named `default` when one exists.
- **Processes**: argv only; `env_clear()` + `BASE_ENV_PASSTHROUGH` and the `UV_`/`PIP_`/`NPM_CONFIG_`
  prefixes (`h2s_core::env::is_passed_through`) + the job environment (runtime
  CLIs also get `DOCKER_HOST`, `DOCKER_CONTEXT`, `DOCKER_CONFIG`, `XDG_RUNTIME_DIR`,
  `CONTAINER_HOST`, `CONTAINERS_CONF`, `REGISTRY_AUTH_FILE` when set); each step is its own
  process group; a step is refused when a secret value would appear in its argv. When a step's
  leader exits, members left in its group are killed, so no background process outlives it.
- **Steps**: `run` = preview, run; `pipeline` = preview, check, run, judge, [judge-bases], report;
  a pipeline stops at the first step that is neither succeeded nor partial and marks the rest
  cancelled. `Progress.done`/`failed`/`total` as in contract addendum v1.1; the demo reports
  done/failed from its own `[domain] i/n` lines (no total, it has no preview).
- **Budget guard**: polls `ExperimentDir::spend_usd` every `jobs.budget_poll_secs` while the
  `run` step runs and counts spend **since that step started** (an earlier run of a resumed
  sweep does not count); `spent_usd` is that amount.
- **Cancellation reasons**: user (cancelled, "cancelled by <name>"), budget (cancelled, "budget
  reached"), wall clock (failed, "timed out after 1h 30m"), shutdown (failed, "server stopped"),
  restart (failed, "server restarted").
- A runner that panics (a bug) fails its job ("internal error in the job runner"), terminates
  its recorded process group and frees its slot.
- **Restart recovery** signals a recorded process group only after positive identification:
  the job row records the leader's pid, its start time (`/proc/<pid>/stat` field 22) and the
  boot id (`/proc/sys/kernel/random/boot_id`); every step process gets `H2S_JOB_ID=<job id>` in
  its environment. A record from another boot signals nothing; otherwise only a group with a
  live member (started no earlier than the leader) whose `/proc/<pid>/environ` holds that
  marker is sent SIGTERM, and SIGKILL after the grace period only while such a member remains.
  A process that removed the marker from its environment is not found (it is left running and
  the log says nothing was signalled). The same rule applies when a runner panics. Container
  jobs are also stopped through the label-verified stop.
- **Logs**: one JSON `LogLine` per line in `<data>/logs/jobs/<id>.log` (0600), lines cut at
  64 KiB with ` … [line truncated]`, every line through the job's redactor (built from all
  stored and environment secrets plus the job's own), then path-masked: `<job>`, `<experiment>`,
  `<experiments>`, `<harness2>`, `<data>`, `~` replace the server's absolute paths (failure
  reasons too). The bridge's `HARNESS2_CODEX_API_KEY=` line is always replaced by the server
  itself, independently of the parser. Logs of jobs older than `jobs.log_retention_days` are
  deleted (the job rows stay).
- **Event streams** (contract addendum v1.1): unnamed frames, one `JobEvent` JSON per `data:`;
  `log` events carry `id: <line number>`; a reconnect with `Last-Event-ID: N` replays lines
  `N+1..` up to the snapshot (from the ring buffer or the file); a `heartbeat` event at least every
  15 s; the stream ends after the terminal `status` event, after 10 minutes without events, after
  1 hour, or at shutdown (clients reconnect). A subscriber that falls behind gets a fresh
  `snapshot`. `server.sse_max_connections` streams at once, then 503.

## Container isolation

- **User**: `isolation.container.user` when set; otherwise detected once (at startup, in the
  background): rootless Docker (`docker info` security options contain `name=rootless`) or
  rootless Podman (`podman info` `Host.Security.Rootless`) run jobs as `--user 0:0`, which maps to
  the server's own host user; a rootful runtime gets `--user <server uid>:<server gid>`.
  `--cap-drop ALL` and `no-new-privileges` apply in every case (fixed flags of h2s-core).
- **Mounts**: harness2 at `/h2s/harness2` (read-write only for install and setup), the
  experiment at `/h2s/experiment`, the job files at `/h2s/job` (always read-only). Split jobs
  write their file into `<job dir>/out`, mounted at `/h2s/experiment` (they have no experiment),
  and the server moves it to `<data>/splits/<name>.json`.
- **Install inside the container**: with container isolation the install job runs in the runner
  image, so the venv points at the image's interpreter. `install::plan` inspects the checkout on
  the host; the server maps the host paths in its steps to `/h2s/harness2`.
- **Network** (contract addendum v1.2): the container sees none of the server's environment.
  With `isolation.container.proxy` set, each `docker run` gets fixed `-e HTTPS_PROXY=<proxy>`,
  `HTTP_PROXY`, `https_proxy`, `http_proxy` and `NO_PROXY`/`no_proxy` = `localhost,127.0.0.1,::1`
  values; the runtime CLI's own environment does not get them. The install job (git, uv) needs
  this: with `network = "none"` only the offline demo works.
- **HOME**: the runner image sets `HOME=/h2s/home` (mode 1777), because container uid 0 without
  capabilities cannot write `/home/node` on a rootless runtime.
- **Stopping**: the job label of `h2s-job-<id>` is read first and the container is stopped only
  when it equals the job id.
- `SystemInfo.container_runtime` is set once the runtime answered (user detection or a passing
  doctor check).

## Egress proxy

As §9, with: `egress.listen` defaults to `127.0.0.1:3128` (also in the file `init` writes): the
proxy has no client access list, so listening beyond loopback (`0.0.0.0:3128` on a jobs network,
as in the compose example) must be configured explicitly; subdomain-only wildcards (`*.example.com` does not match `example.com`); a host that
resolves to any refused address is refused as a whole; unspecified, multicast, broadcast,
documentation, benchmarking and other reserved addresses are refused even with `allow_private`;
IPv4-mapped, NAT64 and 6to4 IPv6 addresses are judged by their IPv4 address; refusals answer
with `Connection: close`. Fixed limits: 10 s header read, 5 s DNS, 10 s connect, 300 s idle,
512 connections. The allowlist is `[egress].allow` only: hosts in profiles' `egress_hosts` must be
added there by the administrator.

## For contributors

`jobs::backend::JobBackend` is the one seam between the generic job engine and harness2's CLI
contract (identifier rules, argv, parsers, spend). Production uses `CoreBackend` (h2s-core); the
integration tests also drive the engine with a backend that plans stub scripts in a temporary
harness2 home (`tests/common/mod.rs`), to exercise process groups, cancellation, timeouts,
budgets and recovery without depending on harness2 itself.
