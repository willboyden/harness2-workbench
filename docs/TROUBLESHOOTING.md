# Troubleshooting

Find the **exact message** you are looking at (search this page for it), read what produces it, and
apply the fix. Messages quoted here were taken from a running server, or, where a line says
*(from the code)*, from the source because the situation could not be reproduced safely. For
configuration keys see [ADMIN-GUIDE.md](ADMIN-GUIDE.md#5-configuration-reference); for the HTTP
contract, [API.md](API.md).

Contents: [Where to look](#where-to-look) · [HTTP errors](#http-errors-by-status) ·
[Browser and proxy](#browser-and-reverse-proxy-symptoms) · [Starting the server](#starting-the-server) ·
[Jobs](#jobs) · [Containers, compose and egress](#containers-compose-and-egress) ·
[Vertex AI credentials](#vertex-ai-credentials) · [`h2workbench doctor` checks](#h2workbench-doctor-checks) ·
[MCP server](#mcp-server)

## Where to look

Every API error body carries a **`request_id`**, also in the `x-request-id` response header, the
server's log line for that request and the audit record. Start there.

| What | Where |
|---|---|
| Server log (start-up, one line per request, warnings) | standard error of `h2workbench serve`: `journalctl --user -u h2workbench -f` under systemd, `docker compose logs -f workbench` under compose. Format and verbosity: `telemetry.log_format`, `telemetry.log_level` (not `RUST_LOG`) |
| A job's output | `GET /api/v1/jobs/{id}/log` (the UI's job page, MCP `get_job_log`), or the file `<data_dir>/logs/jobs/<job-id>.log`, one JSON object per line. The job's `error` field is only a one-line summary, **often the last line the program printed**: read the log for the real cause |
| Who did what | `GET /api/v1/audit` (admin), `h2workbench audit verify`; failed sign-ins appear as `auth.failed` with a `detail` such as `bad_secret on /api/v1/auth/me`. The actor names its kind: `token:<name>`, `proxy:<user>`, `cli`, `server` or `anonymous` |
| Counters of refusals | `/metrics`: `h2w_auth_failures_total{reason=...}` with reasons `missing`, `malformed`, `unknown_token`, `bad_secret`, `expired`, `revoked`, `session_invalid`, `proxy_untrusted`, `proxy_secret`, `proxy_user`, `proxy_duplicate_header`, `proxy_no_role`, `forbidden`, `rate_limited`, `sse_per_principal`, `host`, `csrf`, `origin`, `unavailable`; `h2w_connections_refused_total{reason="total"\|"per_client"}` (connections closed at accept); `h2w_audit_suppressed_total` (failed-authentication and refused-request records not written to the audit log) |
| Egress proxy decisions | its standard error: `docker compose logs egress`; lines `decision="allow"` or `decision="deny"` with `host`, `port`, `reason` |
| Environment checks | `h2workbench doctor`, `GET /api/v1/system/doctor` |
| MCP server | its standard error (`RUST_LOG=h2w_mcp=debug` for more) |

## HTTP errors by status

### 401 `unauthorized`

| Message | Meaning and fix |
|---|---|
| `authentication required` | No credentials. Send `Authorization: Bearer h2w_<id>_<secret>` or sign in. |
| `invalid credentials` | A credential was sent but is not valid: not a well-formed token (`h2w_` + 8 lowercase letters or digits + `_` + 43 characters from `A-Za-z0-9_-`, ASCII only, so check for a truncated paste, a typographic dash or a trailing line break), an unknown or wrong token (tokens belong to one server's database: a token from another data directory fails), a header that is not `Bearer <token>` (for example `Basic ...`), or, in proxy mode, a wrong or missing shared secret, a request from a peer outside `trusted_proxies`, or a user, groups or secret header that arrives **more than once** (the proxy must replace client-supplied copies, not add to them). The audit record `auth.failed` says which (`malformed`, `unknown_token`, `bad_secret`, `proxy_untrusted`, `proxy_secret`, `proxy_user`, `proxy_duplicate_header`). |
| `invalid token` | The same, from `POST /auth/login` (the sign-in page). |
| `the token has expired` / `the token has been revoked` | Create a new token (`h2workbench token create`). `h2workbench token list` shows `revoked` and the expiry. |
| `the session has ended; sign in again` | The browser session expired (`auth.session_ttl_hours`), was logged out, or its token was revoked. |

### 403 `forbidden`

| Message | Meaning and fix |
|---|---|
| `state-changing requests need the header x-h2w-request: 1` | Any `POST`, `PUT`, `PATCH` or `DELETE` must send `x-h2w-request: 1`, including login and including calls with a bearer token. Add the header (`curl -H 'x-h2w-request: 1'`). The web UI and the MCP server send it themselves. |
| `cross-origin requests are refused` | A request carried an `Origin` that is neither `server.public_url`'s origin nor `https://` plus its own `Host` (nor, **only when `public_url` is plain `http`**, `http://` plus its own `Host`), or is `null`. Behind an `https` `public_url`, a page opened over plain http on the same host is refused too. From a browser: open the UI at the URL in `public_url`, or fix `public_url`. A script should not send `Origin` at all. Behind a proxy that rewrites `Host`, make it forward the original one. |
| `the request has more than one Origin header` | Two `Origin` lines: a browser sends one, so a proxy or client is adding a second. Fix the sender; the request is refused whatever the values say. |
| `this requires the operator role` / `...admin role` | The token's role is too low for the route ([API.md](API.md#4-roles-and-routes)). Use a token with the role, or ask an admin. The denial is audited (`access.denied`). |
| `starting a job of kind <kind> requires the <role> role` / `cancelling a job of kind <kind> requires the <role> role` | Job kinds have their own minimum: `install_harness2`, `setup` and `bridge` need admin. |
| `your groups do not grant a role on this server` | Proxy mode: `viewer_groups` is set and the user is in none of the admin, operator or viewer groups. Fix the groups the proxy sends, or the lists. |

### 404, 405

| Message | Meaning and fix |
|---|---|
| `no route for GET /api/v1/nope` | The path does not exist. Remember the prefix `/api/v1` (and none for `/healthz`, `/readyz`, `/metrics`). |
| `method not allowed` | Wrong method for the path; the `Allow` response header lists the right ones. |
| `no experiment named X`, `no sweep named X`, `no job X`, `no profile named X`, `no stored secret named X`, `no such token` | The name is wrong (or, for a job id, not a UUID). `GET /experiments`, `GET /jobs` list what exists. |
| `the directory of experiment X is missing` | The experiment is in the database but its directory under `paths.experiments_dir` is gone (moved, restored without it). Restore the directory or recreate the experiment. |

### 409 `conflict`

| Message | Meaning and fix |
|---|---|
| `WorkBuddy-Bench needs host isolation in this version: …` | `isolation.mode = "container"` refuses every WorkBuddy job (cells, `setup`, `split`); its cards say **needs host isolation**. Run WorkBuddy-Bench on a Workbench with host isolation, or use another benchmark. |
| `The Codex bridge needs host isolation in this version` | `isolation.mode = "container"` refuses the `bridge` job. Use host isolation for it. |
| The administrator's note for a benchmark, or `<benchmark> is not offered on this server.` | This server lists the benchmark under `[[benchmarks.not_offered]]` ([ADMIN-GUIDE.md](ADMIN-GUIDE.md#benchmarks-benchmarks-this-server-does-not-offer)): another Workbench runs it. Its card links there. |
| `harness2_home is an existing checkout (paths.harness2_managed = false); the server does not modify it` | `install_harness2` against an unmanaged checkout. Install into it yourself, or set `paths.harness2_managed = true`. |
| `experiment X is pinned to profile Y; harness2 refuses to resume or judge a sweep under different models` | An experiment is pinned to the profile of its first writing job. Use profile `Y`, or a new experiment. |
| `X is archived` | Unarchive it (`PATCH /experiments/X` with `{"archived": false}`) before running into it. |
| `X is a demo experiment; benchmark jobs need a standard experiment` / `X is not a demo experiment; the demo writes its own layout` | Demo and benchmark experiments have different layouts; create the right kind. |
| `an experiment named X already exists` | Choose another name. |
| `profile X is pinned by experiment(s): a, b` | A profile cannot be deleted while experiments are pinned to it. |
| `a split named X already exists` | Split names are unique; pick another. |
| `this is the last active admin token; create another admin token first` | The API will not revoke the last admin. Create another admin token, then revoke. |
| `the token is already revoked`, `job <id> already finished (succeeded)`, `the job changed state; try again` | Self-explanatory; for jobs, read `GET /jobs/{id}`. |
| `X is not a plain directory (symlinks are refused)` | The experiment's directory is a symlink or a file. Experiment directories must be real directories. |

### 413, 421, 429

| Message | Meaning and fix |
|---|---|
| `the request body is larger than N bytes` (413) | Over `server.max_body_bytes` (1 MiB). Raise it, or send less. |
| `too large: <what>` (413), for example `too large: summary.json is 5247749 bytes, over the 4194304-byte limit` | A results view hit one of the server's memory bounds: a `summary.json` over 4 MiB (task detail), more than 10,000 tasks in a sweep, a harness tree or export over 2,000 files or 64 MiB, a judged file over 32 MiB or 100,000 rows, a directory with over 250,000 entries. A job wrote something far larger than harness2 does; look at the experiment's files. In a task list, an oversized summary shows as status `too-large` (with one error), a `summary.json` that cannot be read at all (a symlink, a pipe, an unreadable file) as `unreadable`, and the other tasks still load; the task's own page answers this `413`. Filter on either with `status=too-large` or `status=unreadable`. Reproduced with a 5 MiB `summary.json` in a scratch experiment. |
| `too large: this request may read at most N bytes of results files` (413) (or `... at most N sweeps`) | One request would have read more of an experiment's files than its route allows (256 MiB for a task list or one experiment, 64 MiB for scores and the report, 640 MiB for a comparison, 1 GiB for the manifest; the table is in [API.md](API.md#6-paging-filters-and-limits)). Reproduced with 62 summaries of 4 MiB plus a 12 MiB judged file in one sweep. The experiment list and experiment pages do not fail this way: they show the sweeps read so far and the next call goes on. Look at what a job wrote into the sweep (`du` on the experiment directory), remove what should not be there, or read a smaller view. The budgets are fixed. |
| `too large: the response would be larger than 64 MiB` (413) | A results response (a harness tree, a diff, a JSON export) would exceed 64 MiB once serialized (a harness of control characters is six times larger as JSON). Reproduced with 12 MiB of control bytes in a harness: the view and the JSON export are refused, the `tar.gz` export (the same files, compressed) works. Read the files on disk, or use the tar.gz export. |
| `too large: the harness files total more than 67108864 bytes` or `too large: the results files total more than 1073741824 bytes` or `too large: namespace <name> has more than 10000 summaries` (413) *(from the code)* | The harness view or export counts more than 64 MiB of file contents, or the manifest's digest more than 1 GiB (or more than 10,000 summaries in one sweep). Files over their own cap (4 MiB for a harness file or a summary, 32 MiB for a judged file) are not read at all and appear in the digest as `<too large: N bytes>`. |
| `this server does not answer to that Host` (421) | The `Host` header is not on the allowlist: the host of `public_url`, `server.allowed_hosts`, or `127.0.0.1`/`localhost`/`[::1]` with the bound port. Typing the machine's LAN IP or a DNS name into the browser, or a proxy forwarding another name, causes it. A request whose target is a full URL (`GET http://other.example/...`) must name an allowed host too. Add the name to `allowed_hosts` (a name without a port matches any port) or fix the proxy. **Behind Caddy**, a request for a name Caddy does not serve gets an empty `200` from Caddy itself; the Workbench's 421 appears only for a Host Caddy forwards and the Workbench does not allow. |
| `too many attempts; try again in N s` (429) | Two causes, the same message. **Sign-in:** more than `server.login_attempts_per_minute` attempts from one client address (a token bucket refilled over a minute; IPv6 per /64). **Failed authentication:** more than 60 refused credentials in a minute from one client address (wrong, unknown, expired or revoked tokens, ended sessions, bad proxy headers); until the allowance refills, further requests that fail authentication get `429` instead of `401`, while valid credentials from the same address still work. Wait `Retry-After` seconds, and find the client that keeps sending a bad token (`h2w_auth_failures_total{reason="rate_limited"}` rising; the audit log keeps at most 10 `auth.failed` rows a minute per client, the rest are counted in `h2w_audit_suppressed_total`). Behind a proxy that is **not** in `server.trusted_proxies`, every user shares the proxy's address and one bucket: add the proxy. |
| `this caller already has 8 open event streams; close one first` (429, `Retry-After: 5`) | One token (or session, or proxy user) holds `server.sse_max_per_principal` job event streams (default 8; the number in the message is the configured one): browser tabs, or a script that opens a stream per job and never closes it. Close some, or raise the limit. `h2w_auth_failures_total{reason="sse_per_principal"}` counts them. |

### 400, 422 (bad request, validation)

| Message | Meaning and fix |
|---|---|
| `expected a JSON body with Content-Type: application/json` (400) | Set `Content-Type: application/json`. |
| `the request has more than one Host header` (400) | The request carried two `Host` lines, which HTTP forbids. A client or an intermediate proxy is adding a second one; fix the sender. (A single `Host` the server does not know is the `421` above.) |
| `invalid JSON body: ...` (400) | Malformed JSON or wrong types; the message never quotes your input. An unknown job `kind` lists the valid ones: `unknown variant `x`, expected one of `install_harness2`, `demo`, `setup`, `check`, `preview`, `run`, `judge`, `report`, `split`, `bridge`, `pipeline``. |
| `query: Failed to deserialize query string: ...` (422) | An unknown or unparsable query parameter (`limit=abc`, `bogus=1`). The message names the parameter and lists the valid ones. |
| `params.acknowledge_risks: jobs run on the server host without isolation (the executor gets the server user's files and network); set acknowledge_risks to true to proceed` (422) | Host isolation: `run` and `pipeline` need `"acknowledge_risks": true`, or the administrator sets `isolation.preacknowledge_host_risks = true`, or switches to container isolation. The flag exists so nobody runs executors on a host unknowingly; set it only with informed consent. |
| `params.profile: profile P needs secret S, which is not set` (422) | The profile references a secret that has no value. An admin sets it (`PUT /secrets/S`, the UI's Secrets page, or the server's `H2W_SECRET_S` variable). `GET /profiles` shows each referenced secret's `set` flag, and `GET /profiles/P/check` lists every missing secret at once (the web UI shows it after a profile is saved and in the run wizard). |
| `params.experiment: no experiment named X; create it first` (422) | Create the experiment (only `demo` creates its own). |
| `params.options.split_file: no split named X` / `split X was made for another benchmark` (422) | Create the split for this benchmark first (`split` job). |
| `params.k: must be between 1 and 8` (422) | Demo `k` is 1 to 8; sweeps allow 1 to 32. Other field problems read the same way: the field, then what is allowed. |
| `params.cell: WorkBuddy-Bench needs a subset: office, web or code` (422) | Add `"subset"` to a `wb` cell; do not add one to `jb` or `lab`. |
| `name: use 1-63 lowercase letters, digits, '_' or '-', starting with a letter or digit` (422) | Experiment and profile names. Secret names are `[A-Z][A-Z0-9_]{0,63}`; token names allow letters, digits, space and `._-@`. |
| `params.rev: only the tested harness2 commit <sha> can be installed; an administrator can allow others with paths.allow_untested_harness2_rev = true` (422) | `install_harness2` named a `rev` other than the commit this release was tested with. Leave `rev` out, or an administrator sets `paths.allow_untested_harness2_rev = true` and restarts the server (the job then builds whatever commit GitHub serves for that id: trust it first). |
| `names starting with LD_ are reserved ...`, `names starting with GIT_ are reserved ...`, `HTTPS_PROXY is managed by the server and cannot be set by a profile`, `SSL_CERT_FILE is managed by the server and cannot be set by a profile`, `set exactly one of value or secret`, `a vertex profile needs Vertex settings`, `use the bare model id; the server adds h2w/` (422) | Profile validation, [MODELS.md](MODELS.md#3-examples): the profile tried to set a variable the server owns (the list is in [MODELS.md](MODELS.md#reserved-environment-variables)). |

### 503 `unavailable`, 500 `internal`

| Message | Meaning and fix |
|---|---|
| `harness2 is not installed; an admin can run the install_harness2 job` (503) | No `<harness2_home>/.venv/bin/harness2`. An admin starts `install_harness2` (managed), or points `paths.harness2_home` at a prepared checkout ([ADMIN-GUIDE.md](ADMIN-GUIDE.md#4-first-run)). `h2workbench doctor` row `harness2`. |
| `the job queue is full (N jobs waiting); try again later` (503) | `jobs.max_queued` reached. Wait, cancel queued jobs, or raise the limit. |
| `too many open event streams; try again later` (503) | `server.sse_max_connections` streams are open (browser tabs, scripts that never close). Close some or raise the limit. |
| `the read was cancelled` (503) *(from the code)* | A results read noticed that the request it served had gone away (it timed out or the client left) and stopped at its next file or sweep. Normally nobody sees it; retry the request with a smaller view if it comes back. |
| `the request took too long` (503) | A request exceeded `server.request_timeout_secs` (60 s). Page results with smaller requests. |
| `the database is not available`, `authentication is temporarily unavailable` (503) *(from the code)* | The SQLite thread has stopped; read the server log and restart. `GET /readyz` reports `database`. |
| `internal database error`, `harness2 wrote a malformed X`, `I/O error reading results`, `internal error` (500) | A bug or an unreadable results file. The cause is in the server log under the `request_id`. `Malformed` means a file harness2 (or a job) wrote could not be parsed; look at the file. |

## Browser and reverse-proxy symptoms

| Symptom | Cause and fix |
|---|---|
| Every API call is `421` through the proxy | The proxy forwards a `Host` the server does not allow. Set `public_url` to the URL users type and let the proxy pass the original `Host`, or list the name in `allowed_hosts`. |
| Sign-in appears to work, then everything is `401` `authentication required` | With `public_url` set to `https`, the session cookie is named `__Host-h2w_session` and is `Secure`, so a browser that reached the server over plain `http` does not keep or send it back. Use the `https` URL, or fix `public_url`. (The server reads only the cookie name it sets: a plain `h2w_session` from an older setup is ignored.) |
| Sign-in `403` `cross-origin requests are refused` | `Origin` does not match `public_url` or the `Host`, or `public_url` is `https` and the page was opened over plain `http`. Same fix as above; a proxy that strips or rewrites `Origin` is the other usual cause. |
| An experiment page or the experiment list shows **fewer sweeps than are on disk** | Three causes. More than 200 sweeps: only the newest 200 (by the modification time of their directories) are listed, the rest stay reachable by id; the log says `the experiment has more sweeps than are listed; the newest are listed and the others stay reachable by id`. The first calls over an experiment of very large sweeps: each request reads within its budget, lists what it read and the next call goes on (verified: 8 sweeps of 160 MiB showed 2, 3, ... 8 on successive calls, then answered from the cache). A sweep that cannot be read (for example a `judged.json` over its cap, or a damaged one): it is left out and the log says `a sweep could not be read; it is left out of the experiment's list`; open it by id to see why. |
| A sweep still shows the **old counts** right after a job rewrote its files | By design: an entry whose files changed is served as it was while one background read refreshes it (at most five minutes old otherwise). Reload; verified: the call after a rewrite showed the old counts, the next the new. |
| A job log page has **fewer lines than `limit`** | By design: a page stops near 1 MiB (at least one line) and says where to go on in `next`; `complete` is true only when a finished job's log has been read to its end. A reconnecting live log replays at most one such page; the UI fetches the rest, a script pages with `GET /jobs/{id}/log?from=<next>`. |
| Everybody gets `429` | The proxy is not in `trusted_proxies`, so all users share one address: one login bucket, one failed-authentication allowance (60 a minute) and one connection limit (`server.max_connections_per_ip`). Add the proxy; the start-up warning `server.public_url is https but server.trusted_proxies is empty` says the same. |
| Connections are closed at once with no response (`curl: (56) Recv failure: Connection reset by peer`) while the server is up | The client address holds `server.max_connections_per_ip` connections (default 256), or the server its 1,024 in total; new ones are closed at accept. Look for leaked connections (a script that never closes them), or add the proxy to `trusted_proxies` (listed proxies are exempt from the per-address cap). The server logs `connection limit reached; closing new connections (counted in h2w_connections_refused_total)` at most once a minute, with `reason="per_client"` or `"total"`. |
| The job log does not update live, or arrives in bursts | The proxy buffers server-sent events. Caddy: `flush_interval -1`. nginx: `proxy_buffering off`. The server already sends `x-accel-buffering: no`. |
| A live log stops after about an hour, or ten idle minutes | By design: the stream ends and the client reconnects with `Last-Event-ID`. |
| A live log stops within 15 seconds of revoking the token, or after the browser session ended | By design: an open stream checks its credential at every heartbeat and closes when the token is revoked or expired or the session ended. Sign in again. |
| `curl` works but the page shows `UI not built — run cargo xtask web, then rebuild the server.` | This server was built without `dist/web`. Build the UI, then rebuild the server (`cargo xtask dist` does both in order). A **release** build notices `dist/web` itself: after `cargo xtask web`, `cargo build --release -p h2w-daemon` recompiles and embeds it. A **debug** build picks it up on the next `cargo build` too: `cargo xtask web` marks the embedding module changed (built without `cargo xtask`, run `touch crates/h2w-daemon/src/http/static_ui.rs`). The API works regardless. |
| The audit log shows the proxy's address for every user | `server.trusted_proxies` does not include the proxy. |
| The sign-in page says `The server did not accept this token. Check that it was copied completely and has not been revoked or expired.` | The server answered `401 invalid token` (unknown, wrong, expired or revoked token). Create a new one (`h2workbench token create`). |
| The sign-in page says `That does not look like a Workbench token: tokens start with “h2w_”.` or `Paste your API token first.` | Checked in the browser before anything is sent. |
| The sign-in page says `Too many sign-in attempts from this address. Wait a minute before trying again.` | `429` from the login rate limit (see the `429` row above). |
| You are sent back to the sign-in page with no message while using the UI | Your session ended: it expired (`auth.session_ttl_hours`), you signed out elsewhere, or the token was revoked. Sign in again; you return to the page you were on. |
| A page of the UI shows an error toast with a server message | Look the message up in [HTTP errors by status](#http-errors-by-status). The [user guide](USER-GUIDE.md#14-when-something-looks-wrong) lists the UI's own messages. |

## Starting the server

All of these appear as `h2workbench: <message>` on standard error, exit status 1.

| Message | Meaning and fix |
|---|---|
| `invalid configuration (<file>):` followed by `  - key: problem` lines | Every problem is listed at once; fix each (the keys and rules are in [the reference](ADMIN-GUIDE.md#5-configuration-reference)). `h2workbench config check` repeats the check without starting anything. |
| `<file>: TOML parse error at line N, column M` ... `unknown field `x`, expected one of ...` | A misspelled key, or a key outside its `[table]`. The message lists the valid keys. |
| `cannot read <file>: No such file or directory (os error 2): ...` | An explicit `--config` or `$H2W_CONFIG` that does not exist. (A missing file at the default location silently means the built-in defaults.) |
| `starting the server: cannot listen on 127.0.0.1:8722: Address already in use (os error 98): ...` | Another process (often another Workbench) holds the port. Stop it or change `server.listen`. (The second `os error` text is repeated by the message formatting.) |
| `starting the server: cannot create the data directories: <path>: ...` | `paths.data_dir` (or the experiments directory) cannot be created or written by this user. |
| `starting the server: auth.mode = "proxy" needs the shared secret in the environment variable H2W_PROXY_SECRET` | Proxy mode needs `$H2W_PROXY_SECRET` (the name is `auth.proxy.secret_env`) set where the server starts. |
| `starting the server: H2W_PROXY_SECRET must be at least 16 characters` | Use a longer random secret. |
| `auth.mode = "proxy" needs server.trusted_proxies (the reverse proxy's address)` | Part of the configuration list; add the proxy's address or CIDR. |
| `already initialised: an admin token exists in <db>. Create more with `h2workbench token create`, or pass --force for another bootstrap token` | `init` ran before. If you lost the token: `h2workbench init --force` (mints another bootstrap admin token and leaves the configuration alone), or `h2workbench token create --role admin`. |
| `the database was written by a newer Harness² Workbench (schema N, this build knows M)` | You started an older binary against a newer database. Run the newer binary, or restore the backup made before the upgrade. |
| `no database at <path>` (`audit verify`) | The data directory has no `workbench.db`; check `--config` and `paths.data_dir`. |
| `no active token with id X` (`token revoke`) | Wrong id, or already revoked (`h2workbench token list`). |
| `XDG_DATA_HOME points into a sandboxed application's directory; set --config or H2W_DATA_DIR if that is not intended` (warning) | The shell was started from a snap or Flatpak app (an IDE terminal) and its XDG variables point into the sandbox. Set explicit paths. |
| `warning: auth.mode = "proxy" but H2W_PROXY_SECRET is not set in this environment` (`config check`) | Only a warning: the shell running the check lacks the variable the service will have. |
| `warning: server.public_url is https but server.trusted_proxies is empty: when a reverse proxy terminates TLS in front of this server, every client has the proxy's address ...` (`config check`, and a warning in the log at start) | Not an error. If a reverse proxy sits in front, list its address in `server.trusted_proxies`, or every user shares one login limit, one failed-authentication allowance and one connection limit, and the audit log shows the proxy's address. |
| `SECURITY: network h2w-jobs is NOT internal: container jobs can reach the internet directly, around the egress proxy http://h2w-egress:3128` (a warning in the server log at start; the `container_network` row of `doctor`) | `isolation.container.proxy` is set but the jobs network has a route out, so a job can ignore the proxy. Recreate the network with `docker network create --internal <name>` and attach the egress proxy to it and to a network with internet access ([ADMIN-GUIDE.md](ADMIN-GUIDE.md#8-container-isolation-step-by-step)). The server warns; it does not stop. |
| `Warning: no active admin token remains; create one with `h2workbench token create --role admin`.` | You revoked the last admin from the command line. Do as it says. |

## Jobs

| Symptom or message | Meaning and fix |
|---|---|
| A job stays `queued` | `jobs.max_concurrent` jobs are running; or another job that **writes** (`demo`, `run`, `judge`, `pipeline`) holds the same experiment; or an `install_harness2`/`setup` job is running (they run alone and block everything behind them). Look at `GET /jobs?status=running`. |
| `exited with code 127: [FATAL tini (7)] exec /h2w/harness2/.venv/bin/harness2-demo failed: No such file or directory` | **Container isolation with a harness2 installed on the host.** The script exists but its first line names the host's Python, which the runner image does not have. Reinstall harness2 in container mode (`install_harness2` with `"reinstall": true` while the Workbench is in container mode) or return to host mode. |
| `exited with code 125: Run 'docker run --help' for more information` | The runtime refused to start the container. **The job log has the real reason**, for example `Unable to find image 'harness2-workbench-runner:9.9.9' locally` and `pull access denied for harness2-workbench-runner, repository does not exist or may require 'docker login'`: build or pull the image named by `isolation.container.image`. `h2workbench doctor` finds this earlier: its `runner_image` row fails with the build command. |
| Jobs cannot connect to the egress proxy (`Connection refused` to `http://h2w-egress:3128` or similar) although the proxy container runs | `egress.listen` defaults to `127.0.0.1:3128`, which only the proxy's own container can reach. A proxy that other containers use must listen on an address of the jobs network: the compose example configuration sets `0.0.0.0:3128` (keep the port unreachable from elsewhere); the plain example keeps the loopback default and its comment says to change it for a container. *(from the code and the defaults; not reproduced with real containers)* |
| `could not prepare the job: ...` | Setup before the first process: a secret that disappeared (`secret X is not set`), a mount source that is not a plain directory (`... must be a directory, not a symlink or file`), an unwritable job directory. The text after the colon is specific. |
| `could not prepare the job: the value of X contains a line break or NUL, which a container job cannot receive (its variables go through an env file, one per line); change the value or use host isolation` | Container jobs get their variables through a 0600 file with one `NAME=VALUE` per line. A profile value with a newline, carriage return or NUL cannot be written that way (`the value of X is too long for a container job (at most 64 KiB per variable)` is the size limit). Change the value, or use host isolation. |
| `profile <name> no longer passes validation (extra_env.GIT_SSH_COMMAND: names starting with GIT_ are reserved and cannot be set by a profile); an administrator must update it` (`422` when creating a job) | A profile stored by an earlier version sets a variable that this version reserves (`GIT_*`, `DOCKER_*`, `XDG_*`, `OPENCODE_*`, `PIP_*`, `NODE_PATH`, `SSL_CERT_FILE`, ...; list in [MODELS.md](MODELS.md#reserved-environment-variables)). Save the profile again without the variable. |
| `refused: a secret value would appear on the command line` | A step's argv would have contained a secret value; the step is refused rather than leaking it. Do not put keys in options. |
| `finished, but some tasks failed or stayed ungraded` (status `partial`) | harness2 exited with code 2: it finished, but some tasks failed or were not graded. That is a result, not a crash; read the report. |
| `exited with code N: <a line>` | The program failed. The line shown is the last non-empty line it wrote to standard error; read the log from the top for the cause. |
| `step <name> failed: ...` | A pipeline step failed; later steps are marked cancelled. |
| `timed out after 1h 30m` | `jobs.timeout_hours` passed (fractions allowed). Raise it for a long sweep and resume by repeating the command. |
| `budget reached` (status `cancelled`) | `budget_usd` was reached (improver spend only; [MODELS.md](MODELS.md#5-cost)). Resume with a higher budget; finished work is kept. |
| `server restarted` / `server stopped` (status `failed`) | The server was restarted or stopped while the job ran. After a restart the log's last line says what happened to its processes: `server restarted while this job was running; marked failed (terminated its process group N)`, `(stopped its container)`, `(its processes were recorded before a reboot; nothing signalled)` or `(its processes could not be identified (incomplete record); nothing signalled)`. Recovery signals a process group only while a live member still carries the job's `H2W_JOB_ID` marker, so a recycled process id is never hit; a program that removed the marker from its environment would be left running (the log then does not say it was terminated). Repeat the command to resume. |
| `stopped: less than N MiB free on the data directory` (status `failed`) | The file system under `paths.data_dir` had less than `jobs.min_free_disk_mb` (default 256) free when the job started or at one of the checks made every 15 seconds, so the job was stopped before it filled the disk. Free space (or move `paths.data_dir`), or lower the setting if the number is wrong, then repeat the command. |
| A log line `log limit of 256 MiB reached (jobs.max_log_bytes): further output of this job is not stored; the job keeps running`, later `N lines (X) of output were not stored (log limit)` | The job printed more than `jobs.max_log_bytes` (default 256 MiB). The server kept reading its output (so the program is never blocked and results are still parsed) but stored no more of it. Quiet the program, or raise the setting. Live streaming and the log API show only what was stored. |
| `internal error in the job runner` | A bug; the job's slot was freed. Report it with the request id and the server log. |
| `harness2 is not importable after the install` | The install finished but `import harness2` fails; read the `import` step in the log (usually a missing system library or an incompatible Python). |
| `Setup incomplete: N issue(s). See docs/BENCHMARKS.md.` with lines `FAIL: ...` (a `check` job, `ready: false`) | harness2's own readiness check: the benchmark is not installed, a tool is missing (`pandoc missing; install pandoc and poppler-utils`), and so on. Run the `setup` job for the cell (admin) and fix the listed tools. No model is called. |
| `uv` prints `warning: Failed to hardlink files; falling back to full copy` during install | Harmless: uv's cache and the checkout are on different file systems. |
| A job's log is empty or short after a long time | `jobs.log_retention_days` deleted the file (the job row stays). |
| Judging says some rows are *ungraded* | The judge failed or made no pick. Ungraded is not zero: it is left out of the denominators. Re-run the `judge` job later (a provider `429 RESOURCE_EXHAUSTED` in the judge log means quota). |

## Containers, compose and egress

| Symptom or message | Meaning and fix |
|---|---|
| `h2workbench doctor`: `container isolation is configured but neither docker nor podman answers` | The runtime is not running or the server cannot reach it (permissions, socket). `docker info` as the server's user must work. Under systemd set `DOCKER_HOST` (rootless) in the unit. |
| `docker is installed but `docker info` failed (daemon not running, or no permission for this user)` | Same, for the `docker` row. |
| Rootless Docker: files in the experiment belong to an unexpected owner, or `Permission denied` writing the experiment | The job user does not match the runtime's user mapping. Leave `isolation.container.user` empty (rootless runtimes get `0:0`, rootful ones the server's `uid:gid`), or set a numeric value deliberately. *(from the design notes; not reproduced)* |
| A LAB rollout's transcript shows `FileNotFoundError: [Errno 2] No such file or directory: 'podman'` (container isolation) | The runner image was built from 0.1.2 or earlier, without the `podman` stand-in: LAB's `read_doc.py` parses documents with `podman run`, which a container job cannot start, so each read failed until the agent found `--in host`. Rebuild the runner image (ADMIN-GUIDE §3.3). Base rollouts of such runs lost turns that refined harnesses, having learned the workaround, did not. |
| `podman: the Harness² Workbench runner has no podman; this stand-in only runs LAB's document parser` (exit status 125) | Something other than LAB's document reader called `podman` inside a container job. The runner image has no container runtime; a benchmark that starts containers (WorkBuddy-Bench) needs host isolation. |
| `uv`, `npm` or OpenCode fail writing a cache in a custom runner image | The job user has no writable `HOME`. The shipped runner image sets `HOME=/h2w/home` (mode 1777); a custom image must provide one. |
| Egress proxy answers `403` `denied: not on the allowlist` | The job reached a host that is not listed. `docker compose logs egress` shows `decision="deny" reason="not on the allowlist"` with the host. Add the host to `[egress] allow` and restart the egress container. A wildcard `*.example.net` does **not** match `example.net`. |
| `denied: port not allowed for this host` | The host is listed, but not on that port. Plain entries allow 443 and 80; give others in a table entry (`ports = [4000]`); a later entry for the same host can add a port. |
| `denied: resolves to a loopback address` (or `private address`, `link-local or cloud metadata address`, `CGNAT address`) | The name resolves to an address class that needs `allow_private = true` on its entry (an internal gateway). |
| `denied: only allowlisted host names are reachable` | The client used an IP address. IP literals are never allowed; use a name. |
| `denied: connect failed: Connection refused (os error 111)` (502), `denied: connect timeout` / `denied: DNS timeout` (504) | The host is allowed but unreachable from the proxy: check its uplink network, DNS and firewalls. |
| `only CONNECT and absolute http:// requests are proxied` (400) | Something spoke to the proxy as if it were a web server (a browser, a health check). Clients must use it as `HTTPS_PROXY`/`HTTP_PROXY`. |
| `pip`, `git` or `npm` in a job time out or fail on a corporate network | Their traffic is not going through the proxy, or the proxy's allowlist lacks the mirror. Jobs get the server's `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, `NO_PROXY`, `SSL_CERT_FILE`, `SSL_CERT_DIR`, `REQUESTS_CA_BUNDLE`, `NODE_EXTRA_CA_CERTS` and all `UV_*`, `PIP_*`, `NPM_CONFIG_*` variables in host mode, and `isolation.container.proxy` in container mode. |
| `docker compose up`: `all predefined address pools have been fully subnetted` | Docker has no free subnet for the three new networks. Give them explicit `ipam` subnets in an override file ([ADMIN-GUIDE.md](ADMIN-GUIDE.md#10-the-compose-reference-deployment)). *(Reported by the maintainers; the wording is Docker's and can differ between versions.)* |
| Rootless Docker: compose cannot bind `443`/`80` | Rootless Docker cannot publish ports below 1024 unless `net.ipv4.ip_unprivileged_port_start` allows it. Publish a high port (for example `8443:443`) through an override file. |
| The `egress` container shows `unhealthy` | Older images probed the Workbench's port; the compose file now probes the proxy's own port 3128. Update `compose.yaml`. |
| `init` in compose prints a warning that `admin.token` could not be saved | Expected: the mounted configuration directory is read-only. The token was printed first: copy it from the output. |
| An unknown Host through the ingress returns an empty `200` | Caddy answers hosts it has no site for; it is not the Workbench. The Workbench's `421` appears only for a Host Caddy forwards. |

## Vertex AI credentials

| Symptom | Fix |
|---|---|
| `h2workbench doctor`: `the Google Cloud CLI is not installed; harness2-check runs `gcloud auth application-default print-access-token` for Vertex AI profiles` | Install `gcloud` and put it on the **server's** `PATH` (a service has a smaller `PATH` than your shell). Required even with a service-account key. |
| `adc`: `not verified here (checking prints an access token); harness2-check verifies them before every run` | Only a reminder: the Workbench cannot check ADC without printing a token. Run a `check` job to test. Fix a failure with `gcloud auth application-default login` as the server's user (`--no-browser` on a headless host), or give the profile a `credentials_secret`. |
| A model call fails with permission or `not found` errors | The project, location (`global` by default) or model id is wrong, or the account lacks access to that model. The check does not test model availability; the first call does. |
| Judge log shows Vertex `429 RESOURCE_EXHAUSTED` | Quota. Re-run the `judge` job later (upstream's guidance). |
| `WorkBuddy-Bench cannot use this profile: its setup requires a google-vertex/gemini executor, a gemini judge and Vertex AI settings.` | A profile warning: it is in the job log, the audit detail of the save and `GET /profiles/{name}/check`. WorkBuddy is Vertex-only ([MODELS.md](MODELS.md#2-which-benchmark-and-executor-work-with-which-profile-kind)). |
| `LAB's judge does not recognise the judge model "X" ...` | Use a judge id LAB accepts (`gpt-...` alias for a local judge). |

## `h2workbench doctor` checks

Levels: `ok`, `warn` (works, but something deserves attention or only some features are affected),
`fail` (something you asked for cannot work), `skip` (does not apply). The probes run with the
**server's** environment, so a tool on your shell's `PATH` but not the service's is reported as
missing. Each probe has a 10-second limit.

| Id | When it is not `ok` | Fix |
|---|---|---|
| `harness2` | `fail`: `harness2 is not installed`. `warn`: `...; this release was tested with <commit>` | Install it (`install_harness2`), or point `paths.harness2_home` at a checkout at the tested commit. |
| `uv` | `fail`: `uv is not installed; it creates harness2's Python 3.12 environment` | Install uv. |
| `python312` | `fail`: `uv cannot find Python 3.12, which harness2's benchmarks require` (or `cannot look for Python 3.12 without uv`) | `uv python install 3.12`. |
| `git` | `fail`: `git is not installed; it fetches harness2 and the benchmark checkouts` | Install git. |
| `node20`, `npm` | `warn`: `Node.js is not installed; benchmark setup needs it`, `Node.js X is older than 20, which harness2-setup requires`, `could not read the Node.js version`; `npm is not installed; benchmark setup needs it` | Install Node.js 20 or newer with npm. Only benchmark setup needs them. |
| `curl` | `warn`: `curl is not installed; WorkBuddy setup downloads its datasets with it` | Install curl (WorkBuddy only). |
| `docker` | `warn`: `Docker is not installed; WorkBuddy-Bench runs its tasks in containers`, or `docker is installed but `docker info` failed (daemon not running, or no permission for this user)` | Start the daemon; make `docker info` work for the server's user. Needed for WorkBuddy and container isolation. |
| `podman` | `skip`: `Podman is not available (optional...)` | Nothing, unless you want Podman. |
| `pandoc`, `pdftoppm` | `warn`: `Pandoc is not installed; LAB converts documents with it`; `pdftoppm is not installed; LAB renders PDFs with it` | Install Pandoc and Poppler (`poppler-utils`); LAB only. |
| `gcloud`, `adc` | see [Vertex AI credentials](#vertex-ai-credentials); `skip` when no profile uses Vertex | |
| `container_runtime` | `fail` when container isolation is set and neither runtime answers; `skip` in host mode | Start the runtime or set `isolation.mode = "host"`. |
| `runner_image` | container isolation only. `fail`: `runner image <image> is not present for docker (`image inspect` exit 1); container jobs cannot start`. `warn`: `could not ask docker about the runner image <image>: ...` | Build it as the remedy line says (`docker build -f deploy/docker/runner.Dockerfile -t <image> .`) or pull it from your registry. |
| `container_network` | only when `isolation.container.proxy` is set and the network is not `none`. `warn`: `network <name> is NOT internal: container jobs can reach the internet directly, around the egress proxy <proxy>`, or `could not tell whether network <name> is internal: ...` | Recreate the network with `docker network create --internal <name>`; attach the egress proxy to it and to a network with internet access. `ok`: `network <name> is internal: container jobs reach the internet only through the egress proxy <proxy>`. |
| `disk` | `warn`: `N GiB free on the data directory's filesystem` (under 20 GiB) | Free space: setup downloads several GB and every rollout keeps its output. |
| `dotenv` | `warn`: `a .env file in the harness2 checkout supplies every variable a job does not set, bypassing Workbench's profiles and secrets` | Move the `.env` out of the checkout and put its values in a profile. |

`h2workbench doctor` prints one line per check, a `remedy:` line under a `warn` or `fail` that has
one, and then `OK` or `problems found`; the exit status is 1 only when a check is `fail` (from the
code: a report with `warn` rows alone ends in `OK`). The two container rows above were exercised
with a stand-in `docker` script that answered `image inspect` and `network inspect`, not with a
real runtime.

## MCP server

The server's own errors (`forbidden: ...`, `unauthorized: ...`, `validation: ...`) arrive as tool
errors with the Workbench's message and request id; the table lists what only the MCP layer says.

| Message | Meaning and fix |
|---|---|
| `h2workbench-mcp: no API token: set H2W_TOKEN, or point --token-file / H2W_TOKEN_FILE at a file readable only by you (create a token with `h2workbench token create`)` | No credential given in stdio mode. |
| `the token file <path> is accessible by group or others (mode 644); restrict it first: chmod 600 <path>` | Fix the permissions. |
| `both H2W_TOKEN and a token file (--token-file / H2W_TOKEN_FILE) are set; keep one` | Set only one. |
| `refusing to send API tokens over plain http to <host>: use an https URL (a TLS reverse proxy in front of h2workbench) or pass --allow-insecure-remote` | `--server-url` is plain `http` to a non-loopback host. Use `https`, or accept the risk explicitly. |
| `error: unexpected argument '--token' found` | Tokens are never command-line arguments. |
| `unavailable: cannot connect to the Harness² Workbench server at http://127.0.0.1:1/: is `h2workbench serve` running, and is H2W_SERVER_URL / --server-url right?` | The Workbench is not reachable. Start it or fix the URL. |
| `unauthorized: invalid credentials (the server rejected the API token: for stdio check H2W_TOKEN or H2W_TOKEN_FILE, over HTTP the bearer token your MCP client sends) [request id ...]` | The token is wrong, expired or revoked. |
| `forbidden: this requires the operator role (this tool needs the operator role or higher; ask an admin for a token with that role) [request id ...]` | The token's role is too low for the tool ([MCP.md](MCP.md#2-what-an-agent-gets)). |
| `unavailable: harness2 is not installed ... — install it with install_harness2 (admin role), then wait_for_job` | harness2 is missing on the server. |
| `failed to deserialize parameters: unknown argument `jobid` (allowed here: job_id). Check the spelling against the tool's input schema.` | A misspelled or unknown argument is an error, never ignored. |
| `forbidden: this MCP server does not let a model set acknowledge_risks=true, which accepts for the user that the executor runs with the server user's files and network (host isolation). ...` | A `start_run` or `start_pipeline` call carried `acknowledge_risks=true` and `h2workbench-mcp` was not started with `--allow-risk-acknowledgement` (or `H2W_MCP_ALLOW_RISK_ACK=1`). By design: a person starts the run in the web UI, or an administrator sets `isolation.preacknowledge_host_risks`, or the person who runs `h2workbench-mcp` turns the flag on. |
| `failed to deserialize parameters: unknown argument `rev` (allowed here: extras, reinstall, wait_secs). ...` | `install_harness2` over MCP takes no `rev`: it always installs the commit this Workbench was tested against. |
| `rate_limited: too many concurrent waits for this token (4 already running, the most allowed at once). Let one finish, or check the job with get_job instead of waiting.` | `--max-waits-per-token`. Poll with `get_job`. |
| `this MCP server does not answer to that Host; add it with --allowed-host if it is legitimate` (421) | HTTP transport: the name the client used is not allowed. |
| `cross-origin requests are not allowed; add the origin with --allowed-origin if it is legitimate` (403) | HTTP transport: a browser origin that is not allowed. |
| ``send `Authorization: Bearer h2w_...` with a Harness² Workbench API token; it is forwarded to the Workbench server, which checks it`` (401) | HTTP transport: the client sent no bearer token. |
| `this MCP server is already handling N requests, its maximum (--max-concurrent-requests); retry in a moment` (503) | Raise `--max-concurrent-requests` or slow the callers. |
| The client reports a timeout on long waits | The client's tool timeout is shorter than `wait_secs`/`timeout_secs` (up to 600 s): raise it (`MCP_TOOL_TIMEOUT` in Claude Code, `tool_timeout_sec` in Codex). |
| Over HTTP, the Workbench answers `421` to the MCP server's own calls | The Workbench does not allow the `Host` of `--server-url`: add that name to the Workbench's `server.allowed_hosts`. |
