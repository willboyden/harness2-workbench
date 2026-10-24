# Troubleshooting

Find the **exact message** you are looking at (search this page for it), read what produces it, and
apply the fix. Messages quoted here were taken from a running server, or, where a line says
*(from the code)*, from the source because the situation could not be reproduced safely. For
configuration keys see [ADMIN-GUIDE.md](ADMIN-GUIDE.md#5-configuration-reference); for the HTTP
contract, [API.md](API.md).

Contents: [Where to look](#where-to-look) · [HTTP errors](#http-errors-by-status) ·
[Browser and proxy](#browser-and-reverse-proxy-symptoms) · [Starting the server](#starting-the-server) ·
[Jobs](#jobs) · [Containers, compose and egress](#containers-compose-and-egress) ·
[Vertex AI credentials](#vertex-ai-credentials) · [`h2studio doctor` checks](#h2studio-doctor-checks) ·
[MCP server](#mcp-server)

## Where to look

Every API error body carries a **`request_id`**, also in the `x-request-id` response header, the
server's log line for that request and the audit record. Start there.

| What | Where |
|---|---|
| Server log (start-up, one line per request, warnings) | standard error of `h2studio serve`: `journalctl --user -u h2studio -f` under systemd, `docker compose logs -f studio` under compose. Format and verbosity: `telemetry.log_format`, `telemetry.log_level` (not `RUST_LOG`) |
| A job's output | `GET /api/v1/jobs/{id}/log` (the UI's job page, MCP `get_job_log`), or the file `<data_dir>/logs/jobs/<job-id>.log`, one JSON object per line. The job's `error` field is only a one-line summary, **often the last line the program printed**: read the log for the real cause |
| Who did what | `GET /api/v1/audit` (admin), `h2studio audit verify`; failed sign-ins appear as `auth.failed` with a `detail` such as `bad_secret on /api/v1/auth/me` |
| Counters of refusals | `/metrics`: `h2s_auth_failures_total{reason=...}` with reasons `missing`, `malformed`, `unknown_token`, `bad_secret`, `expired`, `revoked`, `session_invalid`, `proxy_untrusted`, `proxy_secret`, `proxy_user`, `proxy_no_role`, `forbidden`, `rate_limited`, `host`, `csrf`, `origin`, `unavailable` |
| Egress proxy decisions | its standard error: `docker compose logs egress`; lines `decision="allow"` or `decision="deny"` with `host`, `port`, `reason` |
| Environment checks | `h2studio doctor`, `GET /api/v1/system/doctor` |
| MCP server | its standard error (`RUST_LOG=h2s_mcp=debug` for more) |

## HTTP errors by status

### 401 `unauthorized`

| Message | Meaning and fix |
|---|---|
| `authentication required` | No credentials. Send `Authorization: Bearer h2s_<id>_<secret>` or sign in. |
| `invalid credentials` | A credential was sent but is not valid: not a well-formed token (`h2s_` + 8 characters + `_` + 43 characters, so check for a truncated paste or a trailing line break), an unknown or wrong token (tokens belong to one server's database: a token from another data directory fails), a header that is not `Bearer <token>` (for example `Basic ...`), or, in proxy mode, a wrong or missing shared secret or a request from a peer outside `trusted_proxies`. The audit record `auth.failed` says which (`malformed`, `unknown_token`, `bad_secret`, `proxy_untrusted`, `proxy_secret`, `proxy_user`). |
| `invalid token` | The same, from `POST /auth/login` (the sign-in page). |
| `the token has expired` / `the token has been revoked` | Create a new token (`h2studio token create`). `h2studio token list` shows `revoked` and the expiry. |
| `the session has ended; sign in again` | The browser session expired (`auth.session_ttl_hours`), was logged out, or its token was revoked. |

### 403 `forbidden`

| Message | Meaning and fix |
|---|---|
| `state-changing requests need the header x-h2s-request: 1` | Any `POST`, `PUT`, `PATCH` or `DELETE` must send `x-h2s-request: 1`, including login and including calls with a bearer token. Add the header (`curl -H 'x-h2s-request: 1'`). The web UI and the MCP server send it themselves. |
| `cross-origin requests are refused` | A request carried an `Origin` that is neither `server.public_url`'s origin nor `http(s)://` plus its own `Host` (or is `null`). From a browser: open the UI at the URL in `public_url`, or fix `public_url`. A script should not send `Origin` at all. Behind a proxy that rewrites `Host`, make it forward the original one. |
| `this requires the operator role` / `...admin role` | The token's role is too low for the route ([API.md](API.md#4-roles-and-routes)). Use a token with the role, or ask an admin. The denial is audited (`access.denied`). |
| `starting a <kind> job requires the <role> role` / `cancelling a <kind> job requires the <role> role` | Job kinds have their own minimum: `install_harness2`, `setup` and `bridge` need admin. |
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
| `WorkBuddy-Bench and the Codex bridge need host isolation in this version` | `isolation.mode = "container"` refuses WorkBuddy cells, `setup`/`split` for `wb`, and the `bridge` job. Use host isolation for those, or another benchmark. |
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
| `this server does not answer to that Host` (421) | The `Host` header is not on the allowlist: the host of `public_url`, `server.allowed_hosts`, or `127.0.0.1`/`localhost`/`[::1]` with the bound port. Typing the machine's LAN IP or a DNS name into the browser, or a proxy forwarding another name, causes it. Add the name to `allowed_hosts` (a name without a port matches any port) or fix the proxy. **Behind Caddy**, a request for a name Caddy does not serve gets an empty `200` from Caddy itself; the Studio's 421 appears only for a Host Caddy forwards and the Studio does not allow. |
| `too many attempts; try again in N s` (429) | More than `server.login_attempts_per_minute` sign-in attempts from one client address (a token bucket refilled over a minute; IPv6 per /64). Wait `Retry-After` seconds. Behind a proxy that is **not** in `server.trusted_proxies`, every user shares the proxy's address and one bucket: add the proxy. |

### 400, 422 (bad request, validation)

| Message | Meaning and fix |
|---|---|
| `expected a JSON body with Content-Type: application/json` (400) | Set `Content-Type: application/json`. |
| `invalid JSON body: ...` (400) | Malformed JSON or wrong types; the message never quotes your input. An unknown job `kind` lists the valid ones: `unknown variant `x`, expected one of `install_harness2`, `demo`, `setup`, `check`, `preview`, `run`, `judge`, `report`, `split`, `bridge`, `pipeline``. |
| `query: Failed to deserialize query string: ...` (422) | An unknown or unparsable query parameter (`limit=abc`, `bogus=1`). The message names the parameter and lists the valid ones. |
| `params.acknowledge_risks: jobs run on the server host without isolation (the executor gets the server user's files and network); set acknowledge_risks to true to proceed` (422) | Host isolation: `run` and `pipeline` need `"acknowledge_risks": true`, or the administrator sets `isolation.preacknowledge_host_risks = true`, or switches to container isolation. The flag exists so nobody runs executors on a host unknowingly; set it only with informed consent. |
| `params.profile: profile P needs secret S, which is not set` (422) | The profile references a secret that has no value. An admin sets it (`PUT /secrets/S`, the UI's Secrets page, or the server's `H2S_SECRET_S` variable). `GET /profiles` shows each referenced secret's `set` flag, and `GET /profiles/P/check` lists every missing secret at once (the web UI shows it after a profile is saved and in the run wizard). |
| `params.experiment: no experiment named X; create it first` (422) | Create the experiment (only `demo` creates its own). |
| `params.options.split_file: no split named X` / `split X was made for another benchmark` (422) | Create the split for this benchmark first (`split` job). |
| `params.k: must be between 1 and 8` (422) | Demo `k` is 1 to 8; sweeps allow 1 to 32. Other field problems read the same way: the field, then what is allowed. |
| `params.cell: WorkBuddy-Bench needs a subset: office, web or code` (422) | Add `"subset"` to a `wb` cell; do not add one to `jb` or `lab`. |
| `name: use 1-63 lowercase letters, digits, '_' or '-', starting with a letter or digit` (422) | Experiment and profile names. Secret names are `[A-Z][A-Z0-9_]{0,63}`; token names allow letters, digits, space and `._-@`. |
| `names starting with LD_ are reserved ...`, `HTTPS_PROXY is managed by the server and cannot be set by a profile`, `set exactly one of value or secret`, `a vertex profile needs Vertex settings`, `use the bare model id; the server adds h2s/` (422) | Profile validation, [MODELS.md](MODELS.md#3-examples). |

### 503 `unavailable`, 500 `internal`

| Message | Meaning and fix |
|---|---|
| `harness2 is not installed; an admin can run the install_harness2 job` (503) | No `<harness2_home>/.venv/bin/harness2`. An admin starts `install_harness2` (managed), or points `paths.harness2_home` at a prepared checkout ([ADMIN-GUIDE.md](ADMIN-GUIDE.md#4-first-run)). `h2studio doctor` row `harness2`. |
| `the job queue is full (N jobs waiting); try again later` (503) | `jobs.max_queued` reached. Wait, cancel queued jobs, or raise the limit. |
| `too many open event streams; try again later` (503) | `server.sse_max_connections` streams are open (browser tabs, scripts that never close). Close some or raise the limit. |
| `the request took too long` (503) | A request exceeded `server.request_timeout_secs` (60 s). Page results with smaller requests. |
| `the database is not available`, `authentication is temporarily unavailable` (503) *(from the code)* | The SQLite thread has stopped; read the server log and restart. `GET /readyz` reports `database`. |
| `internal database error`, `harness2 wrote a malformed X`, `I/O error reading results`, `internal error` (500) | A bug or an unreadable results file. The cause is in the server log under the `request_id`. `Malformed` means a file harness2 (or a job) wrote could not be parsed; look at the file. |

## Browser and reverse-proxy symptoms

| Symptom | Cause and fix |
|---|---|
| Every API call is `421` through the proxy | The proxy forwards a `Host` the server does not allow. Set `public_url` to the URL users type and let the proxy pass the original `Host`, or list the name in `allowed_hosts`. |
| Sign-in appears to work, then everything is `401` `authentication required` | The session cookie is `Secure` (because `public_url` is `https`) but you reached the server over plain `http`, so the browser does not send it back. Use the `https` URL, or fix `public_url`. |
| Sign-in `403` `cross-origin requests are refused` | `Origin` does not match `public_url` or the `Host`. Same fix as above; a proxy that strips or rewrites `Origin` is the other usual cause. |
| Everybody gets `429` | The proxy is not in `trusted_proxies`, so all users share one login bucket. |
| The job log does not update live, or arrives in bursts | The proxy buffers server-sent events. Caddy: `flush_interval -1`. nginx: `proxy_buffering off`. The server already sends `x-accel-buffering: no`. |
| A live log stops after about an hour, or ten idle minutes | By design: the stream ends and the client reconnects with `Last-Event-ID`. |
| `curl` works but the page shows `UI not built — run cargo xtask web, then rebuild the server.` | This server was built without `dist/web`. Build the UI, then rebuild the server (`cargo xtask dist` does both in order; if you built the server first, `touch crates/h2s-daemon/src/http/static_ui.rs` so cargo rebuilds it: it does not notice `dist/web` appearing). The API works regardless. |
| The audit log shows the proxy's address for every user | `server.trusted_proxies` does not include the proxy. |
| The sign-in page says `The server did not accept this token. Check that it was copied completely and has not been revoked or expired.` | The server answered `401 invalid token` (unknown, wrong, expired or revoked token). Create a new one (`h2studio token create`). |
| The sign-in page says `That does not look like a Studio token: tokens start with “h2s_”.` or `Paste your API token first.` | Checked in the browser before anything is sent. |
| The sign-in page says `Too many sign-in attempts from this address. Wait a minute before trying again.` | `429` from the login rate limit (see the `429` row above). |
| You are sent back to the sign-in page with no message while using the UI | Your session ended: it expired (`auth.session_ttl_hours`), you signed out elsewhere, or the token was revoked. Sign in again; you return to the page you were on. |
| A page of the UI shows an error toast with a server message | Look the message up in [HTTP errors by status](#http-errors-by-status). The [user guide](USER-GUIDE.md#14-when-something-looks-wrong) lists the UI's own messages. |

## Starting the server

All of these appear as `h2studio: <message>` on standard error, exit status 1.

| Message | Meaning and fix |
|---|---|
| `invalid configuration (<file>):` followed by `  - key: problem` lines | Every problem is listed at once; fix each (the keys and rules are in [the reference](ADMIN-GUIDE.md#5-configuration-reference)). `h2studio config check` repeats the check without starting anything. |
| `<file>: TOML parse error at line N, column M` ... `unknown field `x`, expected one of ...` | A misspelled key, or a key outside its `[table]`. The message lists the valid keys. |
| `cannot read <file>: No such file or directory (os error 2): ...` | An explicit `--config` or `$H2S_CONFIG` that does not exist. (A missing file at the default location silently means the built-in defaults.) |
| `starting the server: cannot listen on 127.0.0.1:8722: Address already in use (os error 98): ...` | Another process (often another Studio) holds the port. Stop it or change `server.listen`. (The second `os error` text is repeated by the message formatting.) |
| `starting the server: cannot create the data directories: <path>: ...` | `paths.data_dir` (or the experiments directory) cannot be created or written by this user. |
| `starting the server: auth.mode = "proxy" needs the shared secret in the environment variable H2S_PROXY_SECRET` | Proxy mode needs `$H2S_PROXY_SECRET` (the name is `auth.proxy.secret_env`) set where the server starts. |
| `starting the server: H2S_PROXY_SECRET must be at least 16 characters` | Use a longer random secret. |
| `auth.mode = "proxy" needs server.trusted_proxies (the reverse proxy's address)` | Part of the configuration list; add the proxy's address or CIDR. |
| `already initialised: an admin token exists in <db>. Create more with `h2studio token create`, or pass --force for another bootstrap token` | `init` ran before. If you lost the token: `h2studio init --force` (mints another bootstrap admin token and leaves the configuration alone), or `h2studio token create --role admin`. |
| `the database was written by a newer Harness² Studio (schema N, this build knows M)` | You started an older binary against a newer database. Run the newer binary, or restore the backup made before the upgrade. |
| `no database at <path>` (`audit verify`) | The data directory has no `studio.db`; check `--config` and `paths.data_dir`. |
| `no active token with id X` (`token revoke`) | Wrong id, or already revoked (`h2studio token list`). |
| `XDG_DATA_HOME points into a sandboxed application's directory; set --config or H2S_DATA_DIR if that is not intended` (warning) | The shell was started from a snap or Flatpak app (an IDE terminal) and its XDG variables point into the sandbox. Set explicit paths. |
| `warning: auth.mode = "proxy" but H2S_PROXY_SECRET is not set in this environment` (`config check`) | Only a warning: the shell running the check lacks the variable the service will have. |
| `Warning: no active admin token remains; create one with `h2studio token create --role admin`.` | You revoked the last admin from the command line. Do as it says. |

## Jobs

| Symptom or message | Meaning and fix |
|---|---|
| A job stays `queued` | `jobs.max_concurrent` jobs are running; or another job that **writes** (`demo`, `run`, `judge`, `pipeline`) holds the same experiment; or an `install_harness2`/`setup` job is running (they run alone and block everything behind them). Look at `GET /jobs?status=running`. |
| `exited with code 127: [FATAL tini (7)] exec /h2s/harness2/.venv/bin/harness2-demo failed: No such file or directory` | **Container isolation with a harness2 installed on the host.** The script exists but its first line names the host's Python, which the runner image does not have. Reinstall harness2 in container mode (`install_harness2` with `"reinstall": true` while the Studio is in container mode) or return to host mode. |
| `exited with code 125: Run 'docker run --help' for more information` | The runtime refused to start the container. **The job log has the real reason**, for example `Unable to find image 'harness2-studio-runner:9.9.9' locally` and `pull access denied for harness2-studio-runner, repository does not exist or may require 'docker login'`: build or pull the image named by `isolation.container.image`. |
| `could not prepare the job: ...` | Setup before the first process: a secret that disappeared (`secret X is not set`), a mount source that is not a plain directory (`... must be a directory, not a symlink or file`), an unwritable job directory. The text after the colon is specific. |
| `refused: a secret value would appear on the command line` | A step's argv would have contained a secret value; the step is refused rather than leaking it. Do not put keys in options. |
| `finished, but some tasks failed or stayed ungraded` (status `partial`) | harness2 exited with code 2: it finished, but some tasks failed or were not graded. That is a result, not a crash; read the report. |
| `exited with code N: <a line>` | The program failed. The line shown is the last non-empty line it wrote to standard error; read the log from the top for the cause. |
| `step <name> failed: ...` | A pipeline step failed; later steps are marked cancelled. |
| `timed out after 1h 30m` | `jobs.timeout_hours` passed (fractions allowed). Raise it for a long sweep and resume by repeating the command. |
| `budget reached` (status `cancelled`) | `budget_usd` was reached (improver spend only; [MODELS.md](MODELS.md#5-cost)). Resume with a higher budget; finished work is kept. |
| `server restarted` / `server stopped` (status `failed`) | The server was restarted or stopped while the job ran; its processes were terminated. Repeat the command to resume. |
| `internal error in the job runner` | A bug; the job's slot was freed. Report it with the request id and the server log. |
| `harness2 is not importable after the install` | The install finished but `import harness2` fails; read the `import` step in the log (usually a missing system library or an incompatible Python). |
| `Setup incomplete: N issue(s). See docs/BENCHMARKS.md.` with lines `FAIL: ...` (a `check` job, `ready: false`) | harness2's own readiness check: the benchmark is not installed, a tool is missing (`pandoc missing; install pandoc and poppler-utils`), and so on. Run the `setup` job for the cell (admin) and fix the listed tools. No model is called. |
| `uv` prints `warning: Failed to hardlink files; falling back to full copy` during install | Harmless: uv's cache and the checkout are on different file systems. |
| A job's log is empty or short after a long time | `jobs.log_retention_days` deleted the file (the job row stays). |
| Judging says some rows are *ungraded* | The judge failed or made no pick. Ungraded is not zero: it is left out of the denominators. Re-run the `judge` job later (a provider `429 RESOURCE_EXHAUSTED` in the judge log means quota). |

## Containers, compose and egress

| Symptom or message | Meaning and fix |
|---|---|
| `h2studio doctor`: `container isolation is configured but neither docker nor podman answers` | The runtime is not running or the server cannot reach it (permissions, socket). `docker info` as the server's user must work. Under systemd set `DOCKER_HOST` (rootless) in the unit. |
| `docker is installed but `docker info` failed (daemon not running, or no permission for this user)` | Same, for the `docker` row. |
| Rootless Docker: files in the experiment belong to an unexpected owner, or `Permission denied` writing the experiment | The job user does not match the runtime's user mapping. Leave `isolation.container.user` empty (rootless runtimes get `0:0`, rootful ones the server's `uid:gid`), or set a numeric value deliberately. *(from the design notes; not reproduced)* |
| `uv`, `npm` or OpenCode fail writing a cache in a custom runner image | The job user has no writable `HOME`. The shipped runner image sets `HOME=/h2s/home` (mode 1777); a custom image must provide one. |
| Egress proxy answers `403` `denied: not on the allowlist` | The job reached a host that is not listed. `docker compose logs egress` shows `decision="deny" reason="not on the allowlist"` with the host. Add the host to `[egress] allow` and restart the egress container. A wildcard `*.example.net` does **not** match `example.net`. |
| `denied: port not allowed for this host` | The host is listed, but not on that port. Plain entries allow 443 and 80; give others in a table entry (`ports = [4000]`); a later entry for the same host can add a port. |
| `denied: resolves to a loopback address` (or `private address`, `link-local or cloud metadata address`, `CGNAT address`) | The name resolves to an address class that needs `allow_private = true` on its entry (an internal gateway). |
| `denied: only allowlisted host names are reachable` | The client used an IP address. IP literals are never allowed; use a name. |
| `denied: connect failed: Connection refused (os error 111)` (502), `denied: connect timeout` / `denied: DNS timeout` (504) | The host is allowed but unreachable from the proxy: check its uplink network, DNS and firewalls. |
| `only CONNECT and absolute http:// requests are proxied` (400) | Something spoke to the proxy as if it were a web server (a browser, a health check). Clients must use it as `HTTPS_PROXY`/`HTTP_PROXY`. |
| `pip`, `git` or `npm` in a job time out or fail on a corporate network | Their traffic is not going through the proxy, or the proxy's allowlist lacks the mirror. Jobs get the server's `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, `NO_PROXY`, `SSL_CERT_FILE`, `SSL_CERT_DIR`, `REQUESTS_CA_BUNDLE`, `NODE_EXTRA_CA_CERTS` and all `UV_*`, `PIP_*`, `NPM_CONFIG_*` variables in host mode, and `isolation.container.proxy` in container mode. |
| `docker compose up`: `all predefined address pools have been fully subnetted` | Docker has no free subnet for the three new networks. Give them explicit `ipam` subnets in an override file ([ADMIN-GUIDE.md](ADMIN-GUIDE.md#10-the-compose-reference-deployment)). *(Reported by the maintainers; the wording is Docker's and can differ between versions.)* |
| Rootless Docker: compose cannot bind `443`/`80` | Rootless Docker cannot publish ports below 1024 unless `net.ipv4.ip_unprivileged_port_start` allows it. Publish a high port (for example `8443:443`) through an override file. |
| The `egress` container shows `unhealthy` | Older images probed the Studio's port; the compose file now probes the proxy's own port 3128. Update `compose.yaml`. |
| `init` in compose prints a warning that `admin.token` could not be saved | Expected: the mounted configuration directory is read-only. The token was printed first: copy it from the output. |
| An unknown Host through the ingress returns an empty `200` | Caddy answers hosts it has no site for; it is not the Studio. The Studio's `421` appears only for a Host Caddy forwards. |

## Vertex AI credentials

| Symptom | Fix |
|---|---|
| `h2studio doctor`: `the Google Cloud CLI is not installed; harness2-check runs `gcloud auth application-default print-access-token` for Vertex AI profiles` | Install `gcloud` and put it on the **server's** `PATH` (a service has a smaller `PATH` than your shell). Required even with a service-account key. |
| `adc`: `not verified here (checking prints an access token); harness2-check verifies them before every run` | Only a reminder: the Studio cannot check ADC without printing a token. Run a `check` job to test. Fix a failure with `gcloud auth application-default login` as the server's user (`--no-browser` on a headless host), or give the profile a `credentials_secret`. |
| A model call fails with permission or `not found` errors | The project, location (`global` by default) or model id is wrong, or the account lacks access to that model. The check does not test model availability; the first call does. |
| Judge log shows Vertex `429 RESOURCE_EXHAUSTED` | Quota. Re-run the `judge` job later (upstream's guidance). |
| `WorkBuddy-Bench cannot use this profile: its setup requires a google-vertex/gemini executor, a gemini judge and Vertex AI settings.` | A profile warning: it is in the job log, the audit detail of the save and `GET /profiles/{name}/check`. WorkBuddy is Vertex-only ([MODELS.md](MODELS.md#2-which-benchmark-and-executor-work-with-which-profile-kind)). |
| `LAB's judge does not recognise the judge model "X" ...` | Use a judge id LAB accepts (`gpt-...` alias for a local judge). |

## `h2studio doctor` checks

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
| `disk` | `warn`: `N GiB free on the data directory's filesystem` (under 20 GiB) | Free space: setup downloads several GB and every rollout keeps its output. |
| `dotenv` | `warn`: `a .env file in the harness2 checkout supplies every variable a job does not set, bypassing Studio's profiles and secrets` | Move the `.env` out of the checkout and put its values in a profile. |

The `runner_image` id appears in the API's documentation of check ids, but this version does not
produce such a check; a missing runner image shows up as a failed job (`exited with code 125`,
above).

## MCP server

The server's own errors (`forbidden: ...`, `unauthorized: ...`, `validation: ...`) arrive as tool
errors with the Studio's message and request id; the table lists what only the MCP layer says.

| Message | Meaning and fix |
|---|---|
| `h2studio-mcp: no API token: set H2S_TOKEN, or point --token-file / H2S_TOKEN_FILE at a file readable only by you (create a token with `h2studio token create`)` | No credential given in stdio mode. |
| `the token file <path> is accessible by group or others (mode 644); restrict it first: chmod 600 <path>` | Fix the permissions. |
| `both H2S_TOKEN and a token file (--token-file / H2S_TOKEN_FILE) are set; keep one` | Set only one. |
| `refusing to send API tokens over plain http to <host>: use an https URL (a TLS reverse proxy in front of h2studio) or pass --allow-insecure-remote` | `--server-url` is plain `http` to a non-loopback host. Use `https`, or accept the risk explicitly. |
| `error: unexpected argument '--token' found` | Tokens are never command-line arguments. |
| `unavailable: cannot connect to the Harness² Studio server at http://127.0.0.1:1/: is `h2studio serve` running, and is H2S_SERVER_URL / --server-url right?` | The Studio is not reachable. Start it or fix the URL. |
| `unauthorized: invalid credentials (the server rejected the API token: for stdio check H2S_TOKEN or H2S_TOKEN_FILE, over HTTP the bearer token your MCP client sends) [request id ...]` | The token is wrong, expired or revoked. |
| `forbidden: this requires the operator role (this tool needs the operator role or higher; ask an admin for a token with that role) [request id ...]` | The token's role is too low for the tool ([MCP.md](MCP.md#2-what-an-agent-gets)). |
| `unavailable: harness2 is not installed ... — install it with install_harness2 (admin role), then wait_for_job` | harness2 is missing on the server. |
| `failed to deserialize parameters: unknown argument `jobid` (allowed here: job_id). Check the spelling against the tool's input schema.` | A misspelled or unknown argument is an error, never ignored. |
| `forbidden: this MCP server does not let a model set acknowledge_risks=true, which accepts for the user that the executor runs with the server user's files and network (host isolation). ...` | A `start_run` or `start_pipeline` call carried `acknowledge_risks=true` and `h2studio-mcp` was not started with `--allow-risk-acknowledgement` (or `H2S_MCP_ALLOW_RISK_ACK=1`). By design: a person starts the run in the web UI, or an administrator sets `isolation.preacknowledge_host_risks`, or the person who runs `h2studio-mcp` turns the flag on. |
| `failed to deserialize parameters: unknown argument `rev` (allowed here: extras, reinstall, wait_secs). ...` | `install_harness2` over MCP takes no `rev`: it always installs the commit this Studio was tested against. |
| `rate_limited: too many concurrent waits for this token (4 already running, the most allowed at once). Let one finish, or check the job with get_job instead of waiting.` | `--max-waits-per-token`. Poll with `get_job`. |
| `this MCP server does not answer to that Host; add it with --allowed-host if it is legitimate` (421) | HTTP transport: the name the client used is not allowed. |
| `cross-origin requests are not allowed; add the origin with --allowed-origin if it is legitimate` (403) | HTTP transport: a browser origin that is not allowed. |
| ``send `Authorization: Bearer h2s_...` with a Harness² Studio API token; it is forwarded to the Studio server, which checks it`` (401) | HTTP transport: the client sent no bearer token. |
| `this MCP server is already handling N requests, its maximum (--max-concurrent-requests); retry in a moment` (503) | Raise `--max-concurrent-requests` or slow the callers. |
| The client reports a timeout on long waits | The client's tool timeout is shorter than `wait_secs`/`timeout_secs` (up to 600 s): raise it (`MCP_TOOL_TIMEOUT` in Claude Code, `tool_timeout_sec` in Codex). |
| Over HTTP, the Studio answers `421` to the MCP server's own calls | The Studio does not allow the `Host` of `--server-url`: add that name to the Studio's `server.allowed_hosts`. |
