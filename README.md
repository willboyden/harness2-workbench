# Harness² Workbench

**An unofficial GUI, REST API and MCP server for [google-research/harness2](https://github.com/google-research/harness2)**
("Harness²: Recursive Agent Harnessing for an Open World").

> **Not affiliated with Google.** harness2 is the work of its authors at Google Research. Harness²
> Workbench is an independent project that drives harness2 through its command line and reads what it
> writes; it is not endorsed by or supported by Google, and it does not modify harness2. All credit
> for the method belongs to the harness2 authors: see the
> [paper](https://arxiv.org/abs/2610.14224), the [project site](https://harness2.github.io) and
> the [repository](https://github.com/google-research/harness2).

harness2 improves an AI agent's *harness* (prompts, guardrails, memory, tool guidance, subagents,
skills, scripts, plugins) task by task, without touching model weights, and keeps what it learns for
later tasks ([the ideas in five minutes](docs/CONCEPTS.md)). It is a research framework driven from a
terminal: you set environment variables, start long runs by hand, watch them scroll, and read results
as files. **Harness² Workbench is what you put around it when more than one person, or one AI agent, needs
to use it safely**: sign-in and roles, a job queue with live logs, cancellation and cost guards, write-only
secrets with best-effort log redaction, a hash-chained audit trail, browsing of everything harness2 writes
(task summaries, harness trees and diffs, rollouts, experience libraries, reports), optional container
isolation with an egress allowlist, and an MCP server so coding agents can run and analyse sweeps
under the same rules as people.

> **Before you run it: host isolation is not a sandbox.** By default harness2's executors run as the
> user that runs the Workbench. A malicious or prompt-injected executor can read the Workbench's own
> database (stored secrets, token hashes) and files and rewrite them, so it is, in effect, a Workbench
> administrator plus that user. **Container isolation exists for this** ([quick start b](#quick-start-b-jobs-in-containers-with-an-egress-allowlist)).
> The compose deployment ([quick start c](#quick-start-c-the-compose-reference-deployment))
> contains the whole Workbench from your host and network, but **its jobs are not isolated from the
> Workbench itself**. MCP agents: give them a viewer token; an operator token only when they must start
> jobs; never an admin token; and treat everything a tool returns as untrusted.

<table>
<tr>
<td width="33%"><a href="docs/USER-GUIDE.md#4-the-dashboard-and-the-guided-tour"><img src="docs/images/dashboard.png" alt="The dashboard: harness2 status, environment checks, recent jobs and experiments"></a></td>
<td width="33%"><a href="docs/USER-GUIDE.md#6-a-task-in-depth"><img src="docs/images/task-diff.png" alt="The diff between a task's base harness and the harness harness2 adopted"></a></td>
<td width="33%"><a href="docs/USER-GUIDE.md#9-comparing-results"><img src="docs/images/mock-compare.png" alt="Comparing two results task by task with a sign test (fictional mock data)"></a></td>
</tr>
<tr>
<td>The dashboard (real server)</td>
<td>What changed in a harness (real demo run)</td>
<td>Comparing two results (fictional mock data)</td>
</tr>
</table>

## Status and how to read it

This is version 0.1.0, tested against harness2 commit
[`1cfe254`](https://github.com/google-research/harness2/commit/1cfe254e8d25b15f37048c3730134686abe60335).
Every feature below carries two words so you can see what is claimed and what is proven:

| Status | Meaning |
|---|---|
| **Implemented** | the code does it |
| **Partial** | part of it works; the Notes column says what is missing |
| **Not planned** | outside the project's scope |

| Verification | Meaning |
|---|---|
| **verified live** | run against a real server (and, where it matters, a real harness2 checkout), with the date |
| **fakes only** | covered by automated tests that use stand-ins (stub executables, mock servers, recorded output), not by a real run |
| **unverified** | no run of any kind recorded |

"Verified live" below means: by the documentation author on 2026-10-23 and 2026-10-24 on Linux x86_64
with harness2 `1cfe254`, or by the project maintainers, as stated. **No model-backed sweep has been run
through the Workbench**: the only real harness2 workflow exercised end to end is the free offline demo.

Most of this code and documentation was written by AI coding agents (Claude) under human direction
and review; the `Co-Authored-By` trailers of the commits record which.

| Feature | Status | Verification | Notes |
|---|---|---|---|
| REST API: 49 operations, OpenAPI 3.1 | Implemented | verified live (curl against a real server and the real demo) | [API.md](docs/API.md) |
| Sign-in with API tokens and browser sessions | Implemented | verified live | roles viewer < operator < admin; [ADR 0008](docs/adr/0008-api-tokens-and-sessions.md) |
| Single sign-on through a trusted reverse proxy | Implemented | verified live with simulated proxy headers; not with a real SSO proxy | headers plus a shared secret; a repeated identity header is refused; [ADMIN-GUIDE](docs/ADMIN-GUIDE.md#63-proxy-sso-mode) |
| Host, CSRF and Origin protection; strict security headers | Implemented | verified live | no CORS, ever; a repeated `Host` or `Origin` header is refused; behind an `https` public URL only https origins pass |
| Abuse limits: failed-authentication throttle (60 a minute per client), connection cap per address, event-stream cap per principal, job-log cap, free-disk floor | Implemented | verified live by the author on 2026-10-24 against a scratch server (job output from a stand-in harness2 script) | no general request-rate limit; the limiters are kept apart per proxy peer; [THREAT-MODEL](docs/THREAT-MODEL.md) rows D1, D3 and S1 |
| Bounded reads of what jobs write: files opened without blocking, per-request read budgets, cancellation, 64 MiB response cap, about 1 MiB log pages | Implemented | verified live by the author on 2026-10-24 against scratch servers (a FIFO swapped over `summary.json` 9 million times while tasks were listed; 1 TiB and 32 GiB sparse files; a 262 MB log); a negative control against the earlier code was not run | a hostile experiment can still make results views slow or refused (`413`), and a changed sweep can be shown as it was for one refresh; [THREAT-MODEL](docs/THREAT-MODEL.md) row T3 |
| Write-only secrets; best-effort redaction of logs, errors and audit | Implemented | fakes only for redaction (stub processes print planted secrets); set/list verified live | not encrypted at rest; a value a job transforms is not caught; every viewer reads job logs; the server and the egress proxy refuse `/proc` reads by a same-user job (verified live), the database file stays readable to it |
| Hash-chained audit log | Implemented | verified live (verification; an edit to a copy detected) | not tamper-proof: anyone who can write the data directory, host-mode jobs included, can rewrite the whole chain; failed authentications are recorded at most 10 a minute per client and refused requests, refused job starts and cancels included, 10 a minute per principal; actors name their kind (`token:<name>`, `proxy:<user>`); [THREAT-MODEL](docs/THREAT-MODEL.md) |
| Job engine: queue, live logs over SSE, cancel, timeout, budget guard, restart recovery | Implemented | verified live for queueing, SSE, cancelling a queued job and, with a stand-in harness2 script, restart recovery after `kill -9`, the log cap and the free-disk floor; fakes only for running-job cancel, timeout and budget | stub executables stand in for harness2 |
| Managed harness2 install (pinned commit) or an existing checkout | Implemented | verified live | a few seconds with warm caches; only the tested commit can be installed unless an administrator sets `paths.allow_untested_harness2_rev` |
| Offline install bundles: `bundle export`, `verify`, `import` | Implemented | verified live by the maintainers on 2026-10-26: a real LAB installation (7.3 GiB) exported, verified and imported into a second Workbench on the same host, where a LAB `check` and the demo ran with no network; runner-image loading, another machine and host isolation unverified | command line only; the SHA-256 must come from a channel you trust; [ADMIN-GUIDE §3.5](docs/ADMIN-GUIDE.md#35-offline-installation-bundles), [ADR 0009](docs/adr/0009-offline-install-bundles.md) |
| Free demo workflow (`harness2-demo`) | Implemented | verified live (real harness2) | also through MCP |
| Benchmark `check` and `setup` | Implemented | `check` verified live (failure path on an uninstalled benchmark); `setup` against recorded dry-run output only | real setup is network-heavy and unverified |
| Sweeps: `preview`, `run`, `judge`, `report`, `split`, `pipeline` | Implemented | fakes only | parsers checked against recorded harness2 output and output of upstream's own code run offline |
| Browsing results: tasks, harness trees, diffs, exports, rollouts, libraries, improver audit, comparisons with a sign test, manifests | Implemented | verified live on the demo; JobBench, LAB and WorkBuddy layouts against recorded fixtures only | read-only, path-checked, bounded (row above); a sweep list shows at most the newest 200 sweeps of an experiment |
| Cost estimates | Implemented | verified live (the arithmetic) | judging and retries excluded |
| Model profiles: Vertex AI | Implemented | fakes only | [MODELS](docs/MODELS.md) |
| Model profiles: OpenAI-compatible endpoints (LiteLLM, vLLM, SGLang, Ollama) | Implemented | fakes only; compatibility read from upstream sources | open questions listed |
| Codex executor and the Vertex bridge | Implemented | fakes only | host isolation only |
| WorkBuddy-Bench | Partial | unverified | Vertex-only upstream; host isolation only |
| Container isolation (Docker or Podman) | Implemented | verified live by the maintainers on rootless Docker, 2026-10-23; refusals and `doctor` verified live by the author; the 0600 environment file and the `container_network` and `runner_image` checks verified live by the author only against a stand-in `docker` script; rootful Docker and Podman unverified | [ADMIN-GUIDE §8](docs/ADMIN-GUIDE.md#8-container-isolation-step-by-step) |
| Egress allowlist proxy (hostnames, ports, address checks) | Implemented | verified live | listens on loopback unless configured; no client authentication; hostname-level only |
| Compose reference deployment (internal network, egress proxy, TLS ingress) | Implemented | verified live by the maintainers, 2026-10-23; not re-run by the author | [ADMIN-GUIDE §10](docs/ADMIN-GUIDE.md#10-the-compose-reference-deployment) |
| OpenTelemetry traces (OTLP/HTTP) and Prometheus metrics | Implemented | verified live (local collector; scrape with a bearer secret) | |
| MCP server: 42 tools, resources, prompts; stdio and streamable HTTP | Implemented | verified live (stdio session including a real demo run, HTTP guard); client configuration snippets unverified | [MCP.md](docs/MCP.md) |
| Web UI (Leptos, WebAssembly) | Implemented | verified live in headless Firefox on 2026-10-24 against a real server and the free demo (most pages), and against a fictional mock API (scored pages); not other browsers, not a model-backed run | build it with `cargo xtask web`; without it the server serves a placeholder page; [USER-GUIDE](docs/USER-GUIDE.md) says what was and was not exercised |
| systemd user unit | Implemented | syntax checked with `systemd-analyze verify`; service not started | |
| Native TLS, multi-node scheduling, hosting models, Postgres, per-experiment permissions, Windows or macOS job hosts | Not planned | n/a | use a reverse proxy; harness2's setup guide specifies Linux |

## Quick start (a): run it locally against a harness2 checkout

For one person on one machine, no containers. You need Linux, `git`, [`uv`](https://docs.astral.sh/uv/)
(Python 3.12 available to it) and a Rust toolchain. Steps 2 and 3 and everything after them were run as
written, with a scratch home directory, against an existing checkout of the pinned harness2 commit.
Step 1 repeats what the Workbench's `install_harness2` job runs (which was run live, with the same
`uv pip install` and cutoff); the manual commands themselves were not re-run for this page.

```sh
# 1. harness2 itself, at the commit this release was tested against
git clone https://github.com/google-research/harness2 /opt/harness2 && cd /opt/harness2   # any directory you can write
git checkout 1cfe254e8d25b15f37048c3730134686abe60335
uv venv --python 3.12 .venv
uv pip install --python .venv/bin/python --exclude-newer 2026-10-14T02:22:49Z -e '.[render]'   # the Workbench's own install uses this cutoff
cd -

# 2. the Workbench (API and command line; the web UI needs `cargo xtask web` first)
git clone https://github.com/willboyden/harness2-workbench.git && cd harness2-workbench
cargo build --release --locked -p h2w-daemon -p h2w-mcp
export PATH="$PWD/target/release:$PATH"

# 3. configure and start
h2workbench init              # writes ~/.config/harness2-workbench/h2workbench.toml, prints your admin token ONCE
```

Open `~/.config/harness2-workbench/h2workbench.toml`, and in `[paths]` point the Workbench at your checkout
(it will never modify it):

```toml
harness2_home = "/opt/harness2"   # the absolute path of your checkout
harness2_managed = false
```

(Leave both at their defaults instead and the Workbench fetches and installs harness2 itself when an admin
starts an `install_harness2` job.) Then:

```console
$ h2workbench doctor          # lists what is missing for each benchmark; harness2 should be OK
$ h2workbench serve
h2workbench listening on http://127.0.0.1:8722
```

In another terminal, run the demo, which needs no credentials and no network, and look at the result:

```console
$ TOKEN=$(cat ~/.config/harness2-workbench/admin.token)
$ curl -s -X POST -H "Authorization: Bearer $TOKEN" -H "x-h2w-request: 1" -H "content-type: application/json" \
       -d '{"kind":"demo","params":{"experiment":"demo","k":2}}' http://127.0.0.1:8722/api/v1/jobs
{"id":"01a16c44-216a-75cb-8c29-f250b6fdee83","kind":"demo","status":"queued", ...}
$ curl -s -H "Authorization: Bearer $TOKEN" http://127.0.0.1:8722/api/v1/jobs/01a16c44-216a-75cb-8c29-f250b6fdee83
{ ..."status":"succeeded", "result":{"kind":"demo","done":2,"failed":0,"execution_count":28,"editor_calls":32} ...}
```

The [API walkthrough](docs/API.md#10-a-curl-walkthrough) continues from there (diffs, exports, reports);
the web UI ([USER-GUIDE.md](docs/USER-GUIDE.md)) offers the same in a browser once it is built into the binary. Before you point it at a real
model, read [MODELS.md](docs/MODELS.md) and [THREAT-MODEL.md](docs/THREAT-MODEL.md): **in the default
host mode, executors run with your user's files and network.**

## Quick start (b): jobs in containers with an egress allowlist

For anything beyond your own trusted experiments. Each job runs in a hardened throw-away container on
an internal network whose only way out is an allowlisting proxy. Requires Docker (rootless is preferred)
or Podman. Verified live on rootless Docker by the maintainers; the commands come from the example
configuration's own comments.

```sh
docker build -f deploy/docker/runner.Dockerfile -t harness2-workbench-runner:0.1.0 .
docker build -f deploy/docker/Dockerfile        -t harness2-workbench:0.1.0 .
docker network create --internal h2w-jobs && docker network create h2w-uplink
# h2workbench.toml: [isolation] mode = "container"; [isolation.container] network = "h2w-jobs",
#   proxy = "http://h2w-egress:3128"; [egress] allow = ["github.com", "pypi.org", "files.pythonhosted.org", ...]
docker run -d --name h2w-egress --network h2w-jobs -v "$PWD/h2workbench.toml:/config/h2workbench.toml:ro" \
  harness2-workbench:0.1.0 egress-proxy --config /config/h2workbench.toml
docker network connect h2w-uplink h2w-egress
h2workbench serve    # then run an install_harness2 job (admin) in this mode, then the demo
```

The full procedure, the rootless/rootful user mapping, and the one mistake everyone makes (a harness2
installed on the host cannot run inside the container) are in
[ADMIN-GUIDE §8](docs/ADMIN-GUIDE.md#8-container-isolation-step-by-step).

## Quick start (c): the compose reference deployment

The whole Workbench contained from your host: a server on an internal network, an egress proxy as its only
way out, and a Caddy ingress with TLS. Jobs still run in host mode *inside* the Workbench's container, so
they are **not** isolated from the Workbench itself (see the note at the top). Verified live by the
maintainers on rootless Docker (2026-10-23).

```sh
docker build -f deploy/docker/runner.Dockerfile -t harness2-workbench-runner:0.1.0 .
docker build -f deploy/docker/Dockerfile        -t harness2-workbench:0.1.0 .
cp deploy/examples/h2workbench.compose.toml deploy/h2workbench.toml   # then edit the EDIT lines; set your domain in deploy/examples/Caddyfile
chmod 644 deploy/h2workbench.toml     # it holds no secrets; the container user must read it
docker compose -f deploy/compose.yaml run --rm workbench init      # prints the first admin token
docker compose -f deploy/compose.yaml up -d
docker compose -f deploy/compose.yaml exec workbench h2workbench doctor
```

The ingress binds to loopback unless you set `H2W_INGRESS_BIND`. Rootless Docker cannot publish
ports below 1024 without a host setting, and may need explicit network subnets: see
[ADMIN-GUIDE §10](docs/ADMIN-GUIDE.md#10-the-compose-reference-deployment).

## Security in brief

The Workbench starts processes, holds model-provider keys and serves an admin interface, so it is built
to fail closed: it binds to loopback, refuses requests for unknown `Host` names (DNS rebinding) and
state-changing requests without a custom header or from another origin (CSRF), sends a strict
Content-Security-Policy, authenticates with 256-bit tokens stored only as hashes, checks roles in every
route, throttles failed authentication, never returns a secret value and redacts known values from
logs (best effort), validates every
identifier and resolves every path without following symlinks, and appends every sensitive action to a
hash-chained audit log. Containers
drop all capabilities and reach only the hosts you list.

It does **not** make running model-written code safe. In host mode, anyone who can start a run can in
effect run commands as the server's user, and that user owns the Workbench's data: a host-mode job can
read the database (stored secrets, token hashes) and rewrite the audit chain. Secrets are plain text in
a 0600 database; redaction of logs is best effort; the audit chain is a SHA-256 hash chain that is
only as good as the head hash you keep elsewhere (not tamper-proof); MCP agents are prompt-injectable,
which is why MCP cannot write profiles, secrets or tokens. Two independent reviews of the
code by a separate AI agent found issues; the second, on 2026-10-24, confirmed the first round's
fixes (some are partial or by design, as documented) and found ten more, which are fixed in this
revision ([ADMIN-GUIDE §17](docs/ADMIN-GUIDE.md#17-what-changed-with-the-security-review)); **those
last fixes have not been re-reviewed independently**. No human penetration test has been done. Read
[THREAT-MODEL.md](docs/THREAT-MODEL.md) for the assets, trust boundaries, a STRIDE analysis with the
code that implements each control, and the residual risks in plain words. Report vulnerabilities
privately as described in [SECURITY.md](SECURITY.md).

## MCP in two lines

`h2workbench-mcp` gives Claude Code, opencode, Codex, Cursor, VS Code and other MCP clients 42 tools, 3
resources and 3 prompts for harness2, as a thin client that uses your API token, so server-side roles
and the audit log apply to every agent. **Use a viewer token by default, an operator token only when the
agent must start jobs, never an admin token**, and treat everything a tool returns (rollouts, logs,
prompts) as untrusted model-written content. A model cannot accept host-isolation risks on your
behalf unless you start the MCP server with `--allow-risk-acknowledgement`. Setup, client
configuration, the safety rules and every tool: [docs/MCP.md](docs/MCP.md).

## Documentation

| Document | For | What it covers |
|---|---|---|
| [docs/CONCEPTS.md](docs/CONCEPTS.md) | newcomers | harness2 in plain terms: recursion, the eight harness components, modes, budgets; a glossary mapping harness2's words to the Workbench |
| [docs/USER-GUIDE.md](docs/USER-GUIDE.md) | everyone who uses the web UI | a screen-by-screen walkthrough with screenshots: signing in, experiments and sweeps, a task's harnesses and diffs, starting a run, live jobs, comparing, settings, keyboard shortcuts, what each role sees |
| [docs/ADMIN-GUIDE.md](docs/ADMIN-GUIDE.md) | administrators | install, every configuration key, authentication, TLS behind a proxy, container isolation, the egress allowlist, compose, observability, backups, upgrades, the audit log, systemd, a hardening checklist |
| [docs/MODELS.md](docs/MODELS.md) | operators | model profiles for Vertex AI and OpenAI-compatible servers; which benchmark and executor works with which; judge rules; cost |
| [docs/API.md](docs/API.md) | integrators | authentication, the CSRF rules, errors, paging, jobs, server-sent events, roles per route, the OpenAPI document, curl recipes |
| [docs/MCP.md](docs/MCP.md) | agent builders | the MCP server: transports, client configuration, tools, resources, prompts, safety rules |
| [docs/THREAT-MODEL.md](docs/THREAT-MODEL.md) | security reviewers | assets, actors, trust boundaries, STRIDE with code references, residual risks |
| [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md) | everyone | indexed by the exact error message or code you see |
| [docs/adr/](docs/adr/README.md) | architects | the nine architecture decision records |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | contributors | the build contract: components, REST table, security model, job engine, configuration, addenda |
| [docs/SUPPLY-CHAIN.md](docs/SUPPLY-CHAIN.md), [docs/RELEASING.md](docs/RELEASING.md) | maintainers | dependency policy, release process |
| [SECURITY.md](SECURITY.md), [SUPPORT.md](SUPPORT.md), [CONTRIBUTING.md](CONTRIBUTING.md), [CHANGELOG.md](CHANGELOG.md) | | reporting, getting help, contributing, release notes |

## Repository layout

```text
crates/h2w-api      wire types shared by every component (compiles to native and WebAssembly)
crates/h2w-core     domain logic: harness2 command lines, parsers, results reader, trees, estimates
crates/h2w-daemon   the `h2workbench` binary: server, jobs, SQLite, egress proxy, command line
crates/h2w-web      the web UI (Leptos, compiled to WebAssembly); its static shell is in web/
crates/h2w-mcp      the `h2workbench-mcp` binary
crates/h2w-e2e      end-to-end suites against real binaries and a real harness2
tools/h2w-mock-api  fixture API with the production security headers, for UI development
deploy/             Dockerfiles, compose.yaml, example configuration, Caddyfile, systemd unit
tests/fixtures/     harness2 output captured by running harness2 itself (never edited by hand)
xtask/              build orchestration: `cargo xtask web | dist | check | docs | cooldown | e2e`
```

## Contributing

Bug reports, fixes and documentation are welcome; start with [CONTRIBUTING.md](CONTRIBUTING.md). The
quality gate is `cargo xtask check` (format, lints, tests, dependency policy, and a check that every
link in the Markdown files resolves: `cargo xtask docs`).

## License

[Apache License 2.0](LICENSE). harness2 is Apache-2.0 as well; the benchmarks it installs keep their own
licences (WorkBuddy-Bench has a custom licence with a territorial limitation; read it before use).
Parts of `h2w-core` are translated from harness2 (Copyright 2026 Google LLC, Apache-2.0) and
the test fixtures contain its output: see [NOTICE](NOTICE). Release archives and the all-in-one
image also carry `THIRD-PARTY-LICENSES.md`, the licences of the crates compiled in
(`cargo xtask licenses`).
