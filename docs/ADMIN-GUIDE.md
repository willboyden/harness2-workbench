# Administrator's guide

How to install, configure, secure, run and maintain a Harness² Workbench server (`h2workbench`) and its
MCP server (`h2workbench-mcp`). For what the pieces are, start with the [README](../README.md); for
the HTTP interface, [API.md](API.md); for what the security controls do and do not stop,
[THREAT-MODEL.md](THREAT-MODEL.md); for a message you are looking at, [TROUBLESHOOTING.md](TROUBLESHOOTING.md).

**What was verified, and how.** Commands and output on this page marked as a transcript were run
against `h2workbench` 0.1.0 and harness2 `1cfe254` on Linux x86_64 on 2026-10-23 and 2026-10-24 (the
documentation author's runs: init, serve, tokens, doctor, a managed `install_harness2`, the demo,
proxy authentication with simulated proxy headers, the egress proxy on loopback, OTLP export to a
local collector, metrics, the MCP server). The container-isolation and compose-deployment
sections describe behaviour that the project maintainers **verified live on rootless Docker on
2026-10-23** (an `install_harness2` job through the egress proxy, the demo as a container job, a
cancelled container job stopped by label, and the compose stack behind a TLS ingress: `init`,
`/healthz`, the UI page and its CSP, a `421` for a forwarded unknown Host, install, demo, results,
SSE, `audit verify` and `doctor`); the author of this page did not repeat them, except where a
transcript says otherwise. **Not verified anywhere**: rootful Docker, Podman, a real SSO proxy
(only simulated headers), nginx or any proxy other than the compose stack's Caddy, the systemd
unit beyond a syntax check, and any model-backed run.

The behaviours added after an independent security review (collected in
[section 17](#17-what-changed-with-the-security-review)) were run by the author on 2026-10-24
against scratch servers: throttling of failed authentication and the cap on its audit records,
repeated `Host` and proxy headers, the per-client connection and event-stream limits, the log size
cap and the free-space floor, restart recovery after a `kill -9`, the non-dumpable server and
egress proxy, the `__Host-` session cookie, the install restriction, and a container job's
variables through `--env-file` together with the `runner_image` and `container_network` checks.
The container checks used a **stand-in `docker` script** that records what it is given and answers
the probes, not a real container runtime. What the compose image's PID 1 holds was read from its
Dockerfile, not run.

> **Read this first: host isolation is not a security boundary.** In the default `host` isolation,
> harness2's executors run as the server's user. A malicious or prompt-injected executor can
> therefore read the Workbench's database (stored secrets, token hashes), its files and its
> environment, and rewrite them: it is, in effect, a Workbench administrator plus the server user.
> For anything but your own trusted experiments use container isolation
> ([section 8](#8-container-isolation-step-by-step)); that is what it is for. The compose reference
> deployment ([section 10](#10-the-compose-reference-deployment)) contains the whole Workbench from
> your host and network, but **the jobs inside it are not isolated from the Workbench itself**. The
> server and the egress proxy make themselves non-dumpable, so a same-user job cannot read their
> `/proc/<pid>/environ` or memory, but it can still open the database file and everything else the
> user can read: the caveat stands.

Contents: [1 Architecture](#1-architecture-in-one-page) · [2 Requirements](#2-requirements) ·
[3 Installation](#3-installation) · [4 First run](#4-first-run) ·
[5 Configuration reference](#5-configuration-reference) ·
[6 Authentication, roles and tokens](#6-authentication-roles-and-tokens) ·
[7 Reverse proxy and TLS](#7-reverse-proxy-and-tls) ·
[8 Container isolation](#8-container-isolation-step-by-step) · [9 The egress allowlist](#9-the-egress-allowlist) ·
[10 The compose reference deployment](#10-the-compose-reference-deployment) ·
[11 Observability](#11-observability) · [12 Data, backups and upgrades](#12-data-backups-and-upgrades) ·
[13 Audit log](#13-audit-log) · [14 systemd](#14-running-as-a-systemd-user-service) ·
[15 Hardening checklist](#15-hardening-checklist) · [16 Command reference](#16-command-reference) ·
[17 What changed with the security review](#17-what-changed-with-the-security-review)

## 1. Architecture in one page

```mermaid
flowchart TB
  user[Browser / curl] -->|HTTPS| proxy[Reverse proxy<br/>TLS, SSO]
  agent[MCP client] -->|stdio| mcp[h2workbench-mcp]
  mcp -->|REST + token| srv
  proxy -->|HTTP, trusted peer| srv[h2workbench serve<br/>REST, SSE, UI, /metrics<br/>auth, roles, audit, SQLite]
  srv -->|argv, no shell| jobs{{harness2 job processes}}
  jobs -.->|host mode| host[(server user's files<br/>and network)]
  jobs -.->|container mode| ctr[runner container<br/>no caps, limits, own network]
  ctr -->|"HTTP(S)_PROXY"| egress[h2workbench egress-proxy<br/>hostname allowlist]
  egress --> net((allowed hosts))
  srv --- data[(data dir: workbench.db, logs, experiments, harness2)]
```

*This Mermaid diagram was syntax-checked with the Mermaid 12.1.0 parser (`mermaid.parse`). It has not been rendered to an image, so its layout is unseen.*

- **`h2workbench serve`** is the only component with authority. It owns authentication, roles, the
  job queue, the SQLite database and the audit log, and serves the REST API, server-sent events,
  `/metrics` and the web UI from one port (default `127.0.0.1:8722`). It speaks plain HTTP/1.1:
  **there is no native TLS**; put a reverse proxy in front for anything beyond loopback.
- **Jobs** are harness2's own commands, started as child processes without a shell, each in its own
  process group, with an environment built from scratch. In **host** isolation they run as the
  server's user; in **container** isolation each runs in a hardened container
  ([section 8](#8-container-isolation-step-by-step)).
- **`h2workbench-mcp`** is a thin client that gives AI agents the same operations ([MCP.md](MCP.md)).
- **`h2workbench egress-proxy`** is an allowlisting forward proxy for container jobs
  ([section 9](#9-the-egress-allowlist)).
- **Data** lives under one directory: the database, job logs, per-job temporary files, split files,
  the experiments (each one a plain harness2 results directory) and, when the Workbench manages it, the
  harness2 checkout.

Only Linux is supported for running jobs (harness2's setup guide specifies Linux for benchmark runs,
and the server uses process groups and `/proc` for cancellation and restart recovery).

## 2. Requirements

| Needed for | Requirement | Checked by |
|---|---|---|
| everything that runs harness2 | `uv`, Python 3.12 available to uv, `git` | `h2workbench doctor` rows `uv`, `python312`, `git` |
| benchmark setup (JobBench, LAB, WorkBuddy) | Node.js 20 or newer and `npm` | `node20`, `npm` |
| LAB | Pandoc and Poppler (`pdftoppm`) | `pandoc`, `pdftoppm` |
| WorkBuddy-Bench | Docker with permission to run `docker info`; `curl` | `docker`, `curl` |
| Vertex AI profiles | the Google Cloud CLI on `PATH`; Application Default Credentials | `gcloud`, `adc` |
| container isolation | Docker or Podman, and the runner image | `container_runtime` |
| building from source | the Rust toolchain pinned in `rust-toolchain.toml` (1.99.1; `rustup` installs it), and for the web UI `wasm-bindgen-cli` 0.2.129 | |
| free disk | 20 GiB or more recommended; benchmark setup downloads several GB and every rollout keeps its output | `disk` warns below 20 GiB |

Debian/Ubuntu: `sudo apt-get install -y git curl unzip ca-certificates pandoc poppler-utils`, then
install [uv](https://docs.astral.sh/uv/getting-started/installation/) and Node.js 20+.

## 3. Installation

### 3.1 From source (verified)

```console
$ git clone https://github.com/willboyden/harness2-workbench.git && cd harness2-workbench
$ cargo build --release --locked -p h2w-daemon -p h2w-mcp
    Finished `release` profile [optimized] target(s) in 1m 07s
$ ls -l target/release/h2workbench target/release/h2workbench-mcp | awk '{print $5, $9}'
21509776 target/release/h2workbench
13997640 target/release/h2workbench-mcp
```

This builds the two binaries (`h2workbench`, `h2workbench-mcp`) with SQLite bundled and TLS via rustls
(no OpenSSL). They are self-contained but **dynamically linked against glibc** (`libc`, `libm`,
`libgcc_s`). A binary needs a glibc at least as new as the one it was built against, so build on the
oldest distribution you will run it on (or on one with the same glibc). (The
times and sizes above are from a run with warm dependency caches, with the web UI embedded.) The server
embeds the web UI when `dist/web/` exists; build it first
with `cargo xtask web` (needs `wasm-bindgen-cli` 0.2.129 and the `wasm32-unknown-unknown` target,
which `rust-toolchain.toml` installs). `cargo xtask dist` does both steps. **Without `dist/web/` the
server still builds and runs**: the API works and `/` shows a page saying `UI not built`.

- **Release builds notice the UI.** The crate's build script watches `dist/web`, so `cargo xtask web`
  followed by `cargo build --release` recompiles the server with it, with nothing to touch.
  Verified on 2026-10-24: a release server built without `dist/web` served the placeholder
  (`/app.js` was `404`); after `cargo xtask web` the next release build recompiled `h2w-daemon`
  and served the UI (`/app.js` `200`, the 3.4 MB `.wasm` as `application/wasm`); the build after that
  was fresh.
- **Debug builds read the files from disk at run time, but only if `dist/web/` existed when the
  crate was compiled.** `cargo xtask web` therefore marks the embedding module changed, so the next
  plain `cargo build` recompiles it. Verified on 2026-10-24 with a fresh target directory: a debug
  server built without `dist/web` served the placeholder; after `cargo xtask web --dev` and a plain
  `cargo build` it served the UI. With the UI present at compile time, an edit to
  `dist/web/styles.css` was served at once, without a rebuild (verified).

`cargo xtask web` builds a release-size UI (a 3.4 MB `.wasm` here); `cargo xtask web --dev` builds
a faster debug one (about 150 MB).

### 3.2 Release archives

Each tagged release provides, per Linux target (`x86_64-unknown-linux-gnu`,
`aarch64-unknown-linux-gnu`), `harness2-workbench-<tag>-<target>.tar.gz` holding `bin/h2workbench`,
`bin/h2workbench-mcp`, the documentation and the deployment files, plus `SHA256SUMS`, a CycloneDX SBOM
and a build-provenance attestation. Verify before use:

```sh
sha256sum -c SHA256SUMS --ignore-missing
gh attestation verify harness2-workbench-<tag>-<target>.tar.gz --repo willboyden/harness2-workbench
```

See [SUPPLY-CHAIN.md](SUPPLY-CHAIN.md) and [RELEASING.md](RELEASING.md). Verified for v0.1.0 by the
maintainers (2026-10-24): the three files match `SHA256SUMS`, the provenance record names the release
workflow at the tag, and the x86_64 binary served the web UI and ran the demo. The attestation's
signature was not checked with `gh attestation verify` (the `gh` at hand had no such command).

### 3.3 Container images

Two images are built from the repository root (the CI builds them too; they are **not published**
to a registry):

```sh
docker build -f deploy/docker/runner.Dockerfile -t harness2-workbench-runner:0.1.2 .
docker build -f deploy/docker/Dockerfile        -t harness2-workbench:0.1.2 .
```

- **`harness2-workbench-runner`**: Node 22, uv with a managed CPython 3.12, git, curl, unzip, Pandoc,
  Poppler and `tini`; no harness2 code (the checkout is mounted, so a harness2 bump never needs a
  rebuild). It is the image of container-isolated jobs and the base of the next one.
- **`harness2-workbench`**: the runner plus `h2workbench` and `h2workbench-mcp`, running as user `node`
  with `H2W_DATA_DIR=/data` and `H2W_CONFIG=/config/h2workbench.toml`, listening on 8722, with a
  `/healthz` health check. It is what [section 10](#10-the-compose-reference-deployment) runs.

Every base image is pinned by digest; Dependabot proposes updates with the same 14-day cooldown as
the crates, except Node majors, which are chosen by hand from the LTS lines, and uv, which is
bumped by hand ([SUPPLY-CHAIN.md](SUPPLY-CHAIN.md#bumping-uv-by-hand)). Licence texts:
`/usr/share/doc/harness2-workbench/` in the all-in-one image (`LICENSE`, `NOTICE` and
`THIRD-PARTY-LICENSES.md`, the crates compiled in), `/usr/share/doc/uv/` in both, and each Debian
package's `/usr/share/doc/<package>/copyright`. The all-in-one image
builds on the runner image named by the `RUNNER_IMAGE` build argument (default
`harness2-workbench-runner:0.1.2`), so `--build-arg RUNNER_IMAGE=<name:tag>` puts it on your own
runner image.

### 3.4 Put the binaries on `PATH`

```sh
install -Dm755 target/release/h2workbench target/release/h2workbench-mcp -t ~/.local/bin
h2workbench version
```

### 3.5 Offline installation: bundles

Installing harness2 and setting up a benchmark download from GitHub, npm, PyPI and (JobBench) Hugging
Face ([section 9](#9-the-egress-allowlist) lists the hosts). Where the Workbench host or its jobs must
not reach the internet, do that once on a machine that may, then carry the result over as a
**bundle**: one archive of the managed installation (the harness2 checkout, its virtual environment,
and `dependencies/` with the benchmarks' checkouts, environments and data), with a manifest. The
design and its limits are [ADR 0009](adr/0009-offline-install-bundles.md).

**1. On the connected machine**, with the same isolation mode, runner image and architecture as the
target: install harness2 and set up the benchmarks you need (as usual), then

```console
$ h2workbench bundle export --output harness2-bundle.tar.gz --with-runner-image
<sha256>  harness2-bundle.tar.gz
```

The SHA-256 line is printed on stdout and written to `harness2-bundle.tar.gz.sha256`. Export refuses
while an `install_harness2` or `setup` job runs. `--with-runner-image` adds the configured runner image
(`docker save`); leave it out when the target already has that exact image or uses host isolation.

**2. Carry the archive over, and the SHA-256 separately** through a channel you trust: the sidecar
file travels with the archive and proves nothing about it.

**3. On the target**, check it, then install it:

```console
$ h2workbench bundle verify harness2-bundle.tar.gz --sha256 <sha256>
bundle OK: <sha256>
$ h2workbench bundle import harness2-bundle.tar.gz --sha256 <sha256>
installed <sha256>
$ h2workbench doctor
```

`verify` changes nothing: it compares the SHA-256, checks every entry, and names whatever keeps this
Workbench from using the bundle (another architecture, a harness2 commit other than the tested one
unless `paths.allow_untested_harness2_rev`, or a Python environment whose interpreter is missing where
jobs run). `import` does the same checks, then needs `paths.harness2_managed = true`, no queued or running job and
enough free space; it unpacks next to `paths.harness2_home`, loads the runner image if the bundle has
one (container isolation; the image id must match), and swaps the installation in, keeping the
previous one as `<harness2_home>.previous`. Both are command-line only, and both are audited
(`bundle.export`, `bundle.import`). Then run a `check` job for each benchmark: in container isolation it
works with `isolation.container.network = "none"`, because nothing is downloaded any more.

A bundle is **code**: a matching SHA-256 proves you have the bundle that was exported, not that the
machine that exported it was trustworthy. Experiments are not part of a bundle
([section 12](#12-data-backups-and-upgrades) covers backups).

**Verified live by the maintainers on 2026-10-26**, with a real LAB installation in container
isolation: export of 211,677 files (7.3 GiB) and the runner image (220 MiB) took 163 s and wrote a
5.2 GiB archive, whose `.sha256` line `sha256sum -c` confirmed; `verify` took 13 s. Import into a
second Workbench on the same host (its own data directory, `network = "none"`) refused a wrong
SHA-256 without changing anything, then took 17 s; the two installations were identical (248,754
entries compared by type, mode, size and link target, 213 sampled files byte for byte); there,
`doctor` was ok, a LAB `check` job answered `Ready: lab/oc; 1143 tasks` in a container with network
mode `none`, and the demo succeeded. **Not verified**: loading the runner image (that host already had
it, with the same id), an import on another machine, host isolation, Podman.

## 4. First run

All commands take `--config PATH`; without it the server uses `$H2W_CONFIG`, then
`$XDG_CONFIG_HOME/harness2-workbench/h2workbench.toml` (normally `~/.config/harness2-workbench/`). A
transcript from a scratch directory (paths shortened to `/tmp/h2w-demo`):

```console
$ h2workbench --config /tmp/h2w-demo/h2workbench.toml init --data-dir /tmp/h2w-demo/data
h2w_xxxxxxxx_<redacted>                  <- stdout: the first admin token, the ONLY line on stdout
Wrote /tmp/h2w-demo/h2workbench.toml.       <- stderr from here on
Created admin token enn02d72 (also saved to /tmp/h2w-demo/admin.token, mode 0600).
Start the server with `h2workbench serve` and sign in with this token.
```

`init` creates the configuration (mode 0600, in a 0700 directory), the data directories and the
database, then mints a bootstrap token with the **admin** role. The token is printed once on
standard output (so `TOKEN=$(h2workbench init)` works) and also saved to `admin.token` next to the
configuration; the saved copy is best effort, and a warning on standard error says when it cannot
be written (for example a read-only configuration directory in a container).

- `init` **never rewrites an existing configuration**; it uses it as it is. A second `init` refuses
  while an active admin token exists:

  ```console
  $ h2workbench --config /tmp/h2w-demo/h2workbench.toml init
  Using the existing configuration /tmp/h2w-demo/h2workbench.toml (it is not modified).
  h2workbench: already initialised: an admin token exists in /tmp/h2w-demo/data/workbench.db. Create more with `h2workbench token create`, or pass --force for another bootstrap token
  ```

  `--force` mints another bootstrap admin token (it does not touch the configuration).
- `--data-dir` is written into a **new** configuration only; otherwise `$H2W_DATA_DIR`, else the XDG
  data directory (`~/.local/share/harness2-workbench`).

Then check the configuration, look at the environment, and start the server:

```console
$ h2workbench config check
configuration OK
  config file: /tmp/h2w-demo/h2workbench.toml
  data dir: /tmp/h2w-demo/data
  database: /tmp/h2w-demo/data/workbench.db
  experiments: /tmp/h2w-demo/data/experiments
  harness2 home: /tmp/h2w-demo/data/harness2 (managed)
  listen: 127.0.0.1:8722
  public url: http://127.0.0.1:8722/

$ h2workbench doctor
FAIL  harness2           harness2 is not installed
      remedy: Install harness2 (Settings → System → Install harness2, an install_harness2 job).
OK    uv                 uv 0.11.25 (x86_64-unknown-linux-gnu)
OK    python312          Python 3.12 is available to uv
OK    git                git version 2.53.0
OK    node20             Node.js v22.23.1
OK    npm                10.9.8
OK    curl               curl 8.18.0 (x86_64-pc-linux-gnu) libcurl/8.18.0 OpenSSL/3.5.5 zlib/1.3.1 ...
OK    docker             Docker daemon 29.9.1
SKIP  podman             Podman is not available (optional: an alternative runtime for container isolation)
WARN  pandoc             Pandoc is not installed; LAB converts documents with it
      remedy: Install Pandoc (Debian/Ubuntu: sudo apt-get install -y pandoc).
OK    pdftoppm           pdftoppm version 26.01.0
SKIP  gcloud             no model profile uses Vertex AI
SKIP  adc                no model profile uses Vertex AI
SKIP  container_runtime  jobs run on the host (isolation.mode = "host")
OK    disk               109 GiB free
OK    dotenv             no .env file in the harness2 checkout
problems found
```

`doctor` exits 1 while any check is `FAIL`. Check levels: `ok`, `warn` (works, but something
deserves attention or only some benchmarks are affected), `fail` (something you asked for cannot
work), `skip` (does not apply). `h2workbench doctor --json` prints the same report as JSON. The same
report is served at `GET /api/v1/system/doctor` (cached for 60 s). The checks are in
[section 16](#16-command-reference) and [TROUBLESHOOTING.md](TROUBLESHOOTING.md#h2workbench-doctor-checks).

**Start the server.** It prints exactly one line on standard output once the socket is bound, and
logs to standard error:

```console
$ h2workbench serve
h2workbench listening on http://127.0.0.1:8722
```

(`listen = "127.0.0.1:0"` picks a free port; the line shows which.) Stop it with Ctrl-C or
`SIGTERM`: it stops accepting connections, ends event streams, stops running jobs (`SIGTERM`,
`SIGKILL` after 10 s, marked `failed: server stopped`), and leaves queued jobs queued.

**Install harness2.** The Workbench can manage the harness2 checkout (the default) or use one you
already have.

- *Managed:* sign in as an admin and start an `install_harness2` job (web UI: Settings → System).
  It fetches google-research/harness2 at the commit this release was tested against by commit hash
  (`git fetch --depth 1 origin <commit>`, then verifies it), creates a Python 3.12 virtual
  environment with `uv`, installs harness2 with its `render` extra, and checks that it imports. A
  real run took a few seconds with a warm uv cache:

  ```console
  $ curl -s -X POST -H "Authorization: Bearer $TOKEN" -H 'x-h2w-request: 1' -H 'content-type: application/json' \
         -d '{"kind":"install_harness2","params":{}}' http://127.0.0.1:8722/api/v1/jobs
  {"id": "01a16c2f-2130-70af-8585-5a8a65343ca3", "kind": "install_harness2", "status": "queued", ...}
  $ # a few seconds later, GET /api/v1/jobs/<id> (summarised):
  steps: init=succeeded, remote=succeeded, fetch=succeeded, checkout=succeeded, verify-commit=succeeded, venv=succeeded, install=succeeded, import=succeeded
  result: {"kind": "install", "commit": "1cfe254e8d25b15f37048c3730134686abe60335", "version": "0.1.0", "python": "3.12", "extras": ["render"]}
  $ h2workbench doctor | head -1
  OK    harness2           harness2 0.1.0 at 1cfe254e8d25, Python 3.12
  ```

  The `install` step runs `uv pip install --python .venv/bin/python --exclude-newer
  2026-10-14T02:22:49Z -e .[render]` (seen in the job log; the same run, repeated on 2026-10-24 with
  warm caches, took 2 seconds). harness2 has no lock file, so the Workbench resolves its Python
  dependencies as of the tested commit's date: a commit that needs newer packages fails to install
  instead of pulling releases nobody tested.

  Options: `rev` (a full 40-hex commit of google-research/harness2), `extras` (`bridge` adds the
  LiteLLM proxy the Codex bridge needs), `reinstall` (re-create the virtual environment).
- *Existing checkout:* set `paths.harness2_home` to it and `paths.harness2_managed = false`. The
  Workbench **never modifies** such a checkout (an `install_harness2` job is refused with `409`
  `harness2_home is an existing checkout (paths.harness2_managed = false); the server does not
  modify it`). It must contain `.venv/bin/harness2`; install harness2 into it yourself:

  ```sh
  git clone https://github.com/google-research/harness2 ~/harness2 && cd ~/harness2
  git checkout 1cfe254e8d25b15f37048c3730134686abe60335    # the tested commit
  uv venv --python 3.12 .venv
  uv pip install --python .venv/bin/python --exclude-newer 2026-10-14T02:22:49Z -e '.[render]'
  ```

  (`--exclude-newer` gives you the dependency versions the Workbench's own install uses.) `doctor` warns when the installed commit differs from the tested one, and warns about a `.env`
  file in the checkout: harness2 loads it for every variable a job does not set, bypassing the
  Workbench's profiles and secrets.

**Run the free demo** to prove the whole path works (no model, no network, about a second). With
the tokens created above:

```console
$ curl -s -X POST -H "Authorization: Bearer $TOKEN" -H 'x-h2w-request: 1' -H 'content-type: application/json' \
       -d '{"kind":"demo","params":{"experiment":"demo","k":2}}' http://127.0.0.1:8722/api/v1/jobs
```

```console
{"kind": "demo", "done": 2, "failed": 0, "execution_count": 28, "editor_calls": 32}      <- the job's result
```

then follow [API.md](API.md#10-a-curl-walkthrough) or open the web UI.

## 5. Configuration reference

`h2workbench.toml` is TOML with seven tables. **Unknown keys are errors**, so a typo cannot silently
fall back to a default, and a key outside its table is an error too. Every value is validated once
at start-up and **all problems are reported together**. `~` and `~/` are expanded in paths (other
`~user` forms are refused).

```console
$ h2workbench config check
h2workbench: invalid configuration (/tmp/h2w-demo/bad/h2workbench.toml):
  - server.listen: "localhost" is not an address like 127.0.0.1:8722
  - server.public_url: scheme must be http or https; query and fragment are not allowed; serving under a path prefix is not supported
  - server.trusted_proxies[1]: "not-an-ip" is not an IP address or CIDR
  - server.max_body_bytes: 10 is outside 1024..=1073741824
  - jobs.timeout_hours: -1 must be > 0 and <= 8760
  - isolation.container.image: "harness2-workbench-runner:latest" must be pinned as name:tag (not latest) or name@sha256:<digest>
  - isolation.container.network: "host" must be "none" or a user-defined network (never host)
  - isolation.container.user: "root" must be a numeric uid or uid:gid
  - egress.allow[0]: "192.0.2.5" must be a host name or *.suffix (IP literals are not allowed)
  - egress.allow[2].ports: 0 is not a port
```

**Where the file is.** `--config PATH`, else `$H2W_CONFIG`, else
`$XDG_CONFIG_HOME/harness2-workbench/h2workbench.toml` (an absolute `XDG_CONFIG_HOME`, else
`~/.config`). A missing file at the *default* location means the built-in defaults; a missing file
at an *explicit* location is an error. Values pointing into a snap or Flatpak sandbox are logged as
warnings at start-up, as are the resolved paths.

**Environment overrides** (a non-empty value wins over the file): `H2W_LISTEN` (`server.listen`),
`H2W_DATA_DIR` (`paths.data_dir`), `H2W_PUBLIC_URL` (`server.public_url`), `H2W_LOG_FORMAT`
(`telemetry.log_format`: `pretty` or `json`), `H2W_OTLP_ENDPOINT` (`telemetry.otlp_endpoint`).
Other environment variables the server reads: `H2W_CONFIG` (the configuration path),
`H2W_SECRET_<NAME>` (secret values, [section 6.4](#64-secrets)), the variable named by
`auth.proxy.secret_env` (default `H2W_PROXY_SECRET`), and `HOME`, `XDG_CONFIG_HOME`,
`XDG_DATA_HOME` for default locations. The server does **not** read `RUST_LOG`; use
`telemetry.log_level`.

### `[server]`: the HTTP listener

| Key | Type | Default | Validation | Effect |
|---|---|---|---|---|
| `server.listen` | string | `"127.0.0.1:8722"` | `ip:port` (an IP, not a name; `[::1]:8722` for IPv6); port 0 picks a free port | Where the server binds. Loopback by default; the server prints the bound address once listening. Override: `H2W_LISTEN` |
| `server.public_url` | string | `"http://127.0.0.1:8722"` | `http` or `https` URL with a host; no credentials, query, fragment or path prefix | The URL people type. Its host (and port) join the `Host` allowlist, its origin is the expected `Origin` of browser requests (the exact rule is in [section 7](#7-reverse-proxy-and-tls)), and `https` makes the session cookie `Secure` and names it `__Host-h2w_session` (`h2w_session` over plain http). Override: `H2W_PUBLIC_URL`. An `https` URL with no `server.trusted_proxies` draws a warning |
| `server.allowed_hosts` | list of strings | `[]` | each `host`, `host:port`, `[v6]` or `[v6]:port` with a valid name or IP | Extra `Host` values answered (a LAN name, or the name a reverse proxy forwards). An entry without a port matches any port. Anything else is `421` |
| `server.trusted_proxies` | list of strings | `[]` | each an IP address or CIDR | Peers whose `X-Forwarded-For` and `x-request-id` are believed (the client is the right-most address that is not itself trusted) and, in proxy mode, whose identity headers are accepted. They are also exempt from `server.max_connections_per_ip` (a reverse proxy limits its own clients). Empty = none trusted |
| `server.max_body_bytes` | integer | `1048576` | 1024 to 1073741824 | Largest request body; more is `413` |
| `server.request_timeout_secs` | integer | `60` | 1 to 3600 | Longest a request may take; more is `503` |
| `server.sse_max_connections` | integer | `64` | 1 to 10000 | Open job event streams at once; more is `503` |
| `server.sse_max_per_principal` | integer | `8` | 1 to 10000 | Event streams one principal may hold: a token together with all its browser sessions, or one proxy user. More is `429` `this caller already has N open event streams; close one first` (`Retry-After: 5`) |
| `server.login_attempts_per_minute` | integer | `10` | 1 to 10000 | Login attempts, successful or not, per client address (IPv6 per /64): a token bucket of that size refilled over a minute; empty is `429` with `Retry-After` |
| `server.max_connections_per_ip` | integer | `256` | 1 to 1024 | Open connections one client address (IPv6: one /64) may hold, out of the server's 1,024 in total. Further connections are closed at accept, counted in `h2w_connections_refused_total{reason="per_client"}` and logged at most once a minute. Peers in `server.trusted_proxies` are exempt |

Fixed limits that are not configurable: 1024 concurrent connections in total, 10 s to send request
headers, SSE heartbeats every 15 s (the caller's credential is checked again at each one), 10 minutes
idle and 1 hour maximum per event stream. Failed authentication has its own fixed limits
([section 6.5](#65-failed-authentication-is-throttled)). Reads of what jobs write are bounded too:
at most 4 blocking reads of results or job logs at once, a log page or event-stream replay of about
1 MiB, a results response of at most 64 MiB, and a read budget per request that depends on the route
(the numbers are in [API.md](API.md#6-paging-filters-and-limits)).

### `[auth]` and `[auth.proxy]`: who may call

| Key | Type | Default | Validation | Effect |
|---|---|---|---|---|
| `auth.mode` | string | `"token"` | `token` or `proxy` | `token`: API tokens (the web UI exchanges one for a session cookie). `proxy`: a trusted reverse proxy asserts the user and groups in headers; tokens keep working. `proxy` needs a non-empty `server.trusted_proxies`, and the shared secret in the environment when the server starts |
| `auth.session_ttl_hours` | integer | `12` | 1 to 8760 | Lifetime of a browser session; never longer than its token |
| `auth.proxy.user_header` | string | `"x-forwarded-user"` | a valid HTTP header name | Header carrying the authenticated user name |
| `auth.proxy.groups_header` | string | `"x-forwarded-groups"` | a valid HTTP header name | Header carrying the user's groups, comma-separated (several header lines are merged) |
| `auth.proxy.secret_header` | string | `"x-h2w-proxy-secret"` | a valid HTTP header name | Header in which the proxy presents the shared secret |
| `auth.proxy.secret_env` | string | `"H2W_PROXY_SECRET"` | `[A-Z_][A-Z0-9_]*`, at most 128 characters | Name of the environment variable holding the shared secret; its value must be at least 16 characters |
| `auth.proxy.admin_groups` | list of strings | `[]` | | Members become `admin` |
| `auth.proxy.operator_groups` | list of strings | `[]` | | Members become `operator` (if not admin) |
| `auth.proxy.viewer_groups` | list of strings | `[]` | | Members become `viewer`. **Empty = every authenticated proxy user is a viewer**; non-empty = users in none of the three lists get `403` |

### `[paths]`: where things live

| Key | Type | Default | Validation | Effect |
|---|---|---|---|---|
| `paths.data_dir` | string | `""` = `$XDG_DATA_HOME/harness2-workbench` (normally `~/.local/share/harness2-workbench`) | absolute or `~/...` | The database `workbench.db`, `logs/jobs/`, `jobs/` (per-job temporary files), `splits/`. Created mode 0700. Override: `H2W_DATA_DIR` |
| `paths.experiments_dir` | string | `""` = `<data_dir>/experiments` | absolute or `~/...` | Each experiment is a directory here, one `HARNESS2_RUNS_DIR` |
| `paths.harness2_home` | string | `""` = `<data_dir>/harness2` | absolute or `~/...` | The harness2 checkout with its `.venv` and `dependencies/` |
| `paths.harness2_managed` | boolean | `true` | | `true`: the Workbench installs and updates harness2 there. `false`: an existing checkout that is never modified; `install_harness2` is refused |
| `paths.allow_untested_harness2_rev` | boolean | `false` | | `false`: `install_harness2` with a `rev` other than the tested commit is refused, `422` on `params.rev` (`only the tested harness2 commit <sha> can be installed; an administrator can allow others with paths.allow_untested_harness2_rev = true`), because a fork can make any commit fetchable from the upstream repository and installing runs its build code. `true` accepts any 40-hex commit |

### `[jobs]`: the job engine

| Key | Type | Default | Validation | Effect |
|---|---|---|---|---|
| `jobs.max_concurrent` | integer | `2` | 1 to 64 | Jobs running at once (the bridge is outside the count) |
| `jobs.max_queued` | integer | `100` | 1 to 100000 | Queue length; more is `503` |
| `jobs.timeout_hours` | number | `48.0` | greater than 0, at most 8760; fractions allowed | Wall-clock limit per job; exceeding it ends the job `failed` with `timed out after ...` |
| `jobs.budget_poll_secs` | integer | `30` | 1 to 3600 | How often the budget guard reads recorded spend while a `run` step runs |
| `jobs.log_retention_days` | integer | `90` | 1 to 36500 | Job log files older than this are deleted (the job rows stay) |
| `jobs.max_log_bytes` | integer | `268435456` (256 MiB) | 65536 to 1099511627776 | Largest log stored per job. Past it the log gets one marker line, `log limit of N reached (jobs.max_log_bytes): further output of this job is not stored; the job keeps running`; output is still read (the job never blocks) and parsed for progress, results and the bridge, but not stored, and each step's end says how many lines were not stored. The server's own lines may use 64 KiB more |
| `jobs.min_free_disk_mb` | integer | `256` | 0 to 1099511627776 | A running job is stopped, status `failed` with `stopped: less than N MiB free on the data directory`, when the filesystem of `paths.data_dir` has less free space than this; checked when the job starts and every 15 s. `0` = never |

### `[isolation]` and `[isolation.container]`: where jobs run

| Key | Type | Default | Validation | Effect |
|---|---|---|---|---|
| `isolation.mode` | string | `"host"` | `host` or `container` | `host`: jobs run as the server's user with its files and network (upstream's behaviour). `container`: each job runs in a hardened container ([section 8](#8-container-isolation-step-by-step)) |
| `isolation.preacknowledge_host_risks` | boolean | `false` | | When `false`, `run` and `pipeline` jobs in host mode must carry `acknowledge_risks: true` or are refused (`422`). `true` accepts every such run without the flag, **including runs started by MCP agents** (their own acknowledgement guard has nothing left to refuse), and the web UI's run wizard then shows a note instead of asking: set it only on a machine where every operator has been told and no agent holds an operator token |
| `isolation.container.runtime` | string | `"docker"` | `docker` or `podman` | The container CLI |
| `isolation.container.image` | string | `"harness2-workbench-runner:0.1.2"` | pinned: `name:tag` with a tag other than `latest`, or `name@sha256:<64 hex>`; the repository name in lowercase | The runner image |
| `isolation.container.network` | string | `"none"` | `none`, or a user-defined network name (`[A-Za-z0-9][A-Za-z0-9_.-]*`, at most 64 characters; never `host`, `bridge` or `default`) | `none`: no network (the demo works; model calls and installs do not). A network name puts jobs on it, normally an internal one behind an egress proxy |
| `isolation.container.proxy` | string | `""` | empty, or `http://host:port` with an explicit port and no credentials, path or query; needs a `network` other than `none` | The egress proxy as the job containers reach it. Set in each container as `HTTPS_PROXY`, `HTTP_PROXY`, `https_proxy`, `http_proxy`, with `NO_PROXY` and `no_proxy` = `localhost,127.0.0.1,::1`, as fixed `-e NAME=VALUE` arguments (never secrets) |
| `isolation.container.cpus` | number | `4.0` | greater than 0, at most 1024 | `--cpus` |
| `isolation.container.memory_mb` | integer | `8192` | 128 to 4194304 | `--memory` |
| `isolation.container.pids_limit` | integer | `2048` | 16 to 1000000 | `--pids-limit` |
| `isolation.container.user` | string | `""` = detected | empty, or numeric `uid` or `uid:gid` | The container user. Empty: `0:0` on a rootless runtime, the server's own `uid:gid` on a rootful one |

### `[egress]`: the allowlisting proxy (used only by `h2workbench egress-proxy`)

| Key | Type | Default | Validation | Effect |
|---|---|---|---|---|
| `egress.listen` | string | `"127.0.0.1:3128"` | `ip:port` | Where the proxy binds. Loopback by default (also in the file `init` writes), because the proxy has no client access list: listening beyond loopback must be an explicit choice. Job containers must reach it, so a deployment sets an address on the jobs network, such as `0.0.0.0:3128` inside the proxy's own container, as the compose example configuration does (the plain example keeps the loopback default and says in a comment to change it for a container) |
| `egress.allow` | list | `[]` | each entry a host name, a `*.suffix` wildcard, or a table; IP literals are refused; names are validated; `ports` may not contain 0 | The only hosts the proxy will contact. **Empty refuses every request** (the proxy warns at start-up). Entry forms and rules: [section 9](#9-the-egress-allowlist) |

An `allow` entry is `"host"`, `"*.suffix"`, or `{ host = "...", allow_private = true, ports = [4000] }`.

### `[telemetry]`: logs, traces, metrics

| Key | Type | Default | Validation | Effect |
|---|---|---|---|---|
| `telemetry.log_format` | string | `"pretty"` | `pretty` or `json` | Log lines on standard error: human-readable or one JSON object per line. Override: `H2W_LOG_FORMAT` |
| `telemetry.log_level` | string | `"info"` | a level or filter directive such as `info` or `h2w_daemon=debug,warn` | Log verbosity |
| `telemetry.otlp_endpoint` | string | `""` | empty, or an `http(s)` URL with a host | OTLP/HTTP trace export. A URL without a path gets `/v1/traces` appended. Empty = no export. Override: `H2W_OTLP_ENDPOINT` |
| `telemetry.service_name` | string | `"harness2-workbench"` | 1 to 128 characters | `service.name` of exported traces |
| `telemetry.metrics_token_secret` | string | `""` | empty, or a secret name `[A-Z][A-Z0-9_]{0,63}` | The secret whose value is accepted as a bearer token on `/metrics` ([section 11](#11-observability)) |

### `[benchmarks]`: benchmarks this server does not offer

| Key | Type | Default | Validation | Effect |
|---|---|---|---|---|
| `benchmarks.not_offered` | list of tables | `[]` | each benchmark at most once | Benchmarks this server neither sets up nor runs, for example because another Workbench runs them. Their cards say **not on this server** with the note and a link, the New run wizard will not start them, and every job for them (setup and splits included) is refused with `409` and the note |
| `benchmarks.not_offered[].bench` | string | required | `jb`, `lab` or `wb` (all three WorkBuddy-Bench subsets) | Which benchmark |
| `benchmarks.not_offered[].note` | string | `""` | at most 300 characters, no control characters; trimmed | One sentence for people, such as where it runs instead. Empty: "*\<benchmark\> is not offered on this server.*" |
| `benchmarks.not_offered[].url` | string | `""` | empty, or an `http(s)` URL with a host, no credentials, at most 2,048 characters | The Workbench that runs it, shown as the link *Open the Workbench that runs it* |

```toml
[[benchmarks.not_offered]]
bench = "wb"
note = "WorkBuddy-Bench runs on the team's host-isolation Workbench."
url = "https://wb-workbench.example.com"
```

Without an entry, a server with `isolation.mode = "container"` marks WorkBuddy-Bench **needs host
isolation** by itself and refuses its jobs ([section 8](#8-container-isolation-step-by-step)); an
entry replaces that with your note and link. Two Workbenches that split the benchmarks between them
can each list the other's, so every card says where its benchmark runs.

Two annotated examples ship with the code: `deploy/examples/h2workbench.toml` (every key, with
comments) and `deploy/examples/h2workbench.compose.toml` (the compose deployment). `init` writes every
key with its default.

## 6. Authentication, roles and tokens

### 6.1 Roles

**viewer < operator < admin**, each including the ones below it. The full per-route table is in
[API.md](API.md#4-roles-and-routes). In one line each: a viewer reads everything except secrets
metadata, tokens and the audit log; an operator also creates experiments and starts and cancels jobs
that use installed tooling; an admin also installs harness2, sets up benchmarks, runs the Codex
bridge and manages profiles, secrets, tokens and the audit log. Role checks are in every route
handler (not just a middleware), and a test walks the route table to prove each route enforces its
documented role.

### 6.2 Token mode

A token is `h2w_<id>_<secret>`: an 8-character public id and a 256-bit random secret. The server
stores only the SHA-256 of the secret and compares in constant time (ADR
[0008](adr/0008-api-tokens-and-sessions.md)). Browser users paste a token into the sign-in page,
which exchanges it for an `HttpOnly`, `SameSite=Strict` session cookie. Over plain http it is
named `h2w_session`; when `public_url` is `https` it is `Secure` and named `__Host-h2w_session`
(`Path=/`, no `Domain`), so a browser refuses a cookie of that name planted by a sibling subdomain,
and a plain `h2w_session` cookie is ignored. Verified with `public_url = "https://workbench.example.com"`
and the proxy's headers simulated: the login response carried
`set-cookie: __Host-h2w_session=<id>; HttpOnly; SameSite=Strict; Secure; Path=/; Max-Age=43200`.

```console
$ h2workbench token create --name "ci viewer" --role viewer --expires-in-days 30 > viewer.token
Created viewer token ffpqjvis (ci viewer), expires 2026-11-23T04:33:53Z.
$ h2workbench token list
ID         ROLE      STATE    CREATED               EXPIRES               NAME
ffpqjvis   viewer    active   2026-10-24T04:33:53Z  2026-11-23T04:33:53Z  ci viewer
jettl84k   admin     active   2026-10-24T04:30:38Z  never                 initial admin
$ h2workbench token revoke ffpqjvis
Revoked token ffpqjvis.
$ h2workbench token revoke nosuchid                      # exit status 1
h2workbench: no active token with id nosuchid
```

- Names are 1 to 64 characters of letters, digits, space and `._-@`; `--expires-in-days` is 1 to
  3650. **A token created without an expiry never expires**, and neither does the bootstrap admin
  token: give every token an expiry unless you have a reason not to.
- `token list --json` adds `last_used_at` for tokens that have been used; the API equivalent is
  `GET /api/v1/tokens`.
- Revoking a token ends its browser sessions at once. The API refuses to revoke the **last active
  admin token** (`409`); the command line does not, and warns:
  `Warning: no active admin token remains; create one with `h2workbench token create --role admin`.`
- The command line and a running server share the database safely (SQLite WAL, 10 s busy timeout);
  command-line token actions are audited with the actor `cli`.

**Lifecycle advice.** Treat the bootstrap token as a break-glass credential: create named admin
tokens for the people who administer the server, short-lived operator tokens for scripts and
agents, viewer tokens for dashboards, then store the bootstrap token offline or revoke it. Rotate by
creating the new token, switching clients, then revoking the old one.

### 6.3 Proxy (SSO) mode

For a team behind an identity-aware proxy (oauth2-proxy, Pomerium, an organisation's gateway), set
`auth.mode = "proxy"` and describe the headers the proxy sets. The Workbench accepts a user from those
headers **only when all of these hold**: the connection comes from an address in
`server.trusted_proxies`; the request carries `auth.proxy.secret_header` with a value equal to the
shared secret (`$H2W_PROXY_SECRET`, at least 16 characters, compared in constant time); and the user
header is non-empty, at most 256 characters and has no control characters. The role comes from the
groups:

| Group membership | Role |
|---|---|
| any of `admin_groups` | admin |
| else any of `operator_groups` | operator |
| else `viewer_groups` empty, or any of `viewer_groups` | viewer |
| otherwise | `403` `your groups do not grant a role on this server` |

Start-up refuses a proxy configuration without the secret (`auth.mode = "proxy" needs the shared
secret in the environment variable H2W_PROXY_SECRET`) or with a short one (`H2W_PROXY_SECRET must be
at least 16 characters`). A transcript against a server with
`trusted_proxies = ["127.0.0.1"]` and the headers sent by `curl` standing in for a proxy (**not**
tested with a real SSO proxy):

```console
$ curl -H 'x-forwarded-user: alice' -H 'x-forwarded-groups: h2w-operators,staff' -H "x-h2w-proxy-secret: $SECRET" $BASE/api/v1/auth/me
{"id":"alice","name":"alice","role":"operator","kind":"proxy"}
$ ... -H 'x-forwarded-groups: contractors'            # viewer_groups is set and none matches
{"error":{"code":"forbidden","message":"your groups do not grant a role on this server", ...}}
$ ... -H 'x-h2w-proxy-secret: guess'                  # wrong shared secret
{"error":{"code":"unauthorized","message":"invalid credentials", ...}}
```

Configure the proxy to (1) authenticate users, (2) **strip any copy of the identity and secret
headers sent by the client** and set its own, (3) add the shared-secret header on every request it
forwards. In proxy mode tokens still work, so scripts keep using bearer tokens. A proxy user is
recorded in the audit log as `proxy:<the user header's value>`, never as a token: a user who calls
itself `initial admin` appears as `proxy:initial admin`, the bootstrap token as `token:initial admin`
([section 13](#13-audit-log)). The UI contract is unchanged: mutating requests still need
`x-h2w-request: 1` and, when they carry an `Origin`, a matching one
([section 7](#7-reverse-proxy-and-tls)).

**Repeated headers are refused.** A request in which the user, groups or secret header appears more
than once is `401` (`invalid credentials`; the audit record and the metric say
`proxy_duplicate_header`), because it means a value written by the client got past the proxy: taking
the first user, or the union of the groups, would let a client choose its identity or add itself to
an administrators group. The proxy must still delete whatever the client sent under those names and
set each of them exactly once; a single groups header may hold a comma-separated list. Verified: a
request with a repeated user, groups or secret header got `401` (three audit records, metric
`h2w_auth_failures_total{reason="proxy_duplicate_header"} 3`); the same request with single headers
got `200`, and `staff,h2w-operators` in one header gave the operator role.

**If `trusted_proxies` is broad**, for example all private ranges as in the compose example, any
host in those ranges can *try* the headers, and only the shared secret stands between it and an
admin identity. A same-user job cannot read the secret from the server's `/proc/<pid>/environ` (the
server is non-dumpable), but it can read it wherever else you keep it: an `EnvironmentFile=` of the
server's user is readable by a host-mode job, and in the compose image PID 1 is `tini`, which holds
the container's environment and which a job in the same container can read at `/proc/1/environ`
(from the Dockerfile; not run). Keep the secret in a secret store, make it long and random, and
narrow `trusted_proxies` to the proxy's address as soon as it is known.

### 6.4 Secrets

Model-provider keys are **secrets**: write-only values stored either in the database
(`PUT /api/v1/secrets/{name}`, admin) or supplied as `H2W_SECRET_<NAME>` environment variables of
the server. An environment value **takes precedence** over a stored one with the same name and never
touches disk. Names are `[A-Z][A-Z0-9_]{0,63}`; a value is non-empty, at most 64 KiB, with no NUL.
No endpoint ever returns a value: the API shows only `{"name","set","chars","updated_at"}`, and the
audit log records the length ("36 chars"), never the value.

```console
$ curl -s -H "$AUTH" $BASE/api/v1/secrets
[{"name":"LITELLM_API_KEY","set":true,"chars":36,"updated_at":"2026-10-24T03:24:12Z"}]
```

A secret is handed to a job only when the job's profile references it, and only through the child's
environment (never the command line: a step whose argv would contain a secret value is refused; a
container job gets it through a private env file, [section 8](#8-container-isolation-step-by-step)).
Every log line, error message and audit detail passes through a redactor that replaces values with
`[REDACTED:<NAME>]` (including a value's JSON-escaped form and each line of a multi-line value).
**Redaction is best effort**: it matches the literal value, its JSON-escaped form and its standard
and URL-safe base64 spellings (padded or not), so a value that a job reverses, splits, spaces out,
hex- or percent-encodes, or base64-encodes together with other text (an HTTP Basic `user:password`
header) is not recognised, and **job logs are readable by every viewer**. Do not give a job a secret you cannot afford to see in a log.
**Secrets stored in the database are not encrypted at rest**; the file is mode 0600 in a 0700
directory. See [THREAT-MODEL.md](THREAT-MODEL.md) for what that implies, in particular for host-mode
jobs and for backups.

### 6.5 Failed authentication is throttled

A client address (IPv6: one /64; behind a reverse proxy, the address found through
`server.trusted_proxies`) that presents credentials which are refused (a malformed, unknown, wrong,
expired or revoked token or session, or rejected proxy headers) may do so **60 times a minute**
(a token bucket refilled over the minute). Further failures are answered `429`
`rate_limited` (`too many attempts; try again in N s`, with `Retry-After`) and counted as
`h2w_auth_failures_total{reason="rate_limited"}`. **Valid credentials from the same address keep
working.** A request with no credentials at all is never throttled or audited (it is counted as
`missing`). The login endpoint has its own, smaller limit (`server.login_attempts_per_minute`).

Of those failures (and of denied logins) **at most 10 per client and minute are written to the audit
log**; the rest are counted in `h2w_audit_suppressed_total`, so junk credentials cannot flood the
hash-chained log. Verified with 75 requests carrying a made-up bearer token: the first 60 were `401`,
the other 15 `429` (`too many attempts; try again in 1 s`); a valid token from the same address got
`200` in between; the audit log held 10 `auth.failed` records (`unknown_token on /api/v1/auth/me`);
the metrics read `h2w_auth_failures_total{reason="unknown_token"} 60`,
`h2w_auth_failures_total{reason="rate_limited"} 15` and `h2w_audit_suppressed_total 50`.

**Who counts as a client.** The limiters (sign-in attempts, failed authentication, and the audit
records of failures) key every request on its TCP peer and its client: the peer itself, or, when the
peer is in `server.trusted_proxies`, the client found in `X-Forwarded-For`, which any process on the
proxy's network can invent. A peer may hold at most 1,024 client keys of its own (16 for the audit
records of failures); its further clients share one bucket of that peer, so a peer that invents
addresses only limits itself. A limiter tracks at most 10,000 keys in all. When the table is full,
buckets that have refilled completely are dropped first, and a newcomer that still does not fit uses
the overflow bucket of its own peer, so one peer cannot starve another and the limit never fails
open (unit tests `ratelimit.rs::one_peer_cannot_fill_the_table_or_starve_another` and
`a_full_table_shares_overflow_buckets_instead_of_failing_open`; also
`auth.rs::a_peer_inventing_forwarded_clients_only_limits_itself`).

**What remains:** clients that arrive through the *same* trusted peer share that peer's overflow
bucket once the peer has used its 1,024 keys, so someone who can make a proxy forward invented
`X-Forwarded-For` values can get the other users of that proxy refused, at sign-in and for failed
authentication. A process that connects directly (a job, another container) is its own peer and
limits only itself. Verified with the trusted peer `127.0.0.2`: 1,124 sign-in attempts, each with a
different invented client, got 1,034 `401` and 90 `429`; a valid admin token presented for one more
invented client behind the same peer then got `429`, while the same token from the peer `127.0.0.3`
got `200`. Behind a reverse proxy list the proxy in `trusted_proxies`; otherwise every user shares the
proxy's address and one person's mistakes lock out the others.

## 7. Reverse proxy and TLS

The server does not terminate TLS. For anything beyond loopback, run a reverse proxy that does, and
tell the server about it with three settings that must agree:

```toml
[server]
listen = "127.0.0.1:8722"                       # or 0.0.0.0:8722 inside a container network
public_url = "https://workbench.example.com"        # what people type: Host, Origin and Secure cookies derive from it
allowed_hosts = []                               # extra Host values if the proxy forwards another name
trusted_proxies = ["203.0.113.10"]               # the proxy's address(es): X-Forwarded-For is believed only from here
```

- **`Host` allowlist.** A request whose `Host` is not the host of `public_url`, in `allowed_hosts`,
  or a loopback form of the bound port gets `421 this server does not answer to that Host`. This
  stops DNS rebinding. When `public_url` uses the default port for its scheme (443 or 80), that name
  is accepted on any port. Make your proxy forward the original `Host` (or list the name it sends).
  A request with **more than one `Host` header is `400`** (`the request has more than one Host
  header`, RFC 9112 section 3.2), and an absolute-form request target (`GET http://host/...`) must
  name an allowed host as well. Verified with raw requests: two allowed `Host` lines `400`; an
  absolute-form target for `evil.example` with an allowed `Host` `421`.
- **`public_url`** also fixes the expected `Origin` of browser requests and the session cookie:
  `https` makes it `Secure` and names it `__Host-h2w_session`. **The Origin rule** applies to every
  request that needs `x-h2w-request: 1` (all methods except `GET`, `HEAD` and `OPTIONS`). Without an
  `Origin` header (`curl`, SDKs) the request passes this check. With one, it must carry exactly one,
  and it must be the origin of `public_url` or `https://` plus the request's own `Host`; `http://` plus
  the `Host` also passes, but **only when `public_url` itself is plain `http`** (so a UI opened at
  `http://localhost:8722` works). Behind an `https` public URL a page served over plain http on the
  same host is a different, weaker origin and is refused, as are `null` and a repeated `Origin`
  header: `403` with `cross-origin requests are refused` or
  `the request has more than one Origin header`. Verified with raw requests under
  `public_url = "https://workbench.example.com"`: the public origin and `https://<Host>` `200`;
  `http://<Host>`, `null`, a foreign origin, two identical `Origin` headers, and the public origin plus
  `http://<Host>` all `403`; under a plain-http `public_url` both `http://<Host>` and `https://<Host>`
  `200` and two `Origin` headers `403`. The server ignores
  `X-Forwarded-Proto` and `X-Forwarded-Host`: `public_url` is the only source of truth for the scheme
  and host. With an `https` URL and an empty `trusted_proxies`, `serve` and `config check` warn
  (verified): `server.public_url is https but server.trusted_proxies is empty: when a reverse proxy
  terminates TLS in front of this server, every client has the proxy's address (one login rate limit
  and one connection limit for all, the proxy's address in the audit log); list the proxy's address
  in server.trusted_proxies`.
- **`trusted_proxies`.** Only peers in this list may set `X-Forwarded-For` and `x-request-id`, and
  (proxy mode) identity headers. The client address is found by walking `X-Forwarded-For` from the
  right and taking the first entry that is not itself a trusted proxy; an entry that is not an IP
  address ends the walk (everything left of it was written by the client), and no usable entry
  means the peer's own address. The client address feeds the rate limits (kept apart per peer,
  [section 6.5](#65-failed-authentication-is-throttled)) and the audit log.
  Verified live with a trusted peer at `127.0.0.1`:

  ```text
  X-Forwarded-For: 203.0.113.7                      -> audit ip 203.0.113.7
  X-Forwarded-For: garbage, 198.51.100.9            -> audit ip 198.51.100.9
  X-Forwarded-For: 203.0.113.50, junk, 127.0.0.1    -> audit ip 127.0.0.1
  X-Forwarded-For: 198.51.100.20, 127.0.0.1         -> audit ip 198.51.100.20
  X-Forwarded-For: (no header)                      -> audit ip 127.0.0.1
  ```

  If the proxy is not listed, every client appears with the proxy's address, shares one login
  rate-limit bucket, one failed-authentication bucket and one connection limit, and the audit log
  is far less useful. A listed proxy is exempt from `server.max_connections_per_ip`: it must limit
  its own clients (connections and request rates), because the server's global caps (1,024
  connections, 64 event streams) are what is left.
- **Streaming.** Job logs stream over server-sent events. The proxy must not buffer responses
  (the server sends `x-accel-buffering: no`; Caddy needs `flush_interval -1`, nginx
  `proxy_buffering off`). Allow responses to stay open for up to an hour.
- **Headers.** The server sets a strict Content-Security-Policy, `nosniff`, `X-Frame-Options:
  DENY`, `Referrer-Policy`, `Permissions-Policy` and cross-origin isolation headers itself; the
  proxy only needs to add HSTS.

The shipped `deploy/examples/Caddyfile`:

```caddyfile
workbench.example.com {
	encode zstd gzip
	header Strict-Transport-Security "max-age=31536000; includeSubDomains"
	reverse_proxy workbench:8722 {
		header_up X-Forwarded-Proto {scheme}
		flush_interval -1
	}
}
```

Caddy obtains and renews the certificate for the name in the site address (public DNS reachable on
ports 80 and 443 for the default ACME flow; for a private name use Caddy's own options, which this
guide does not cover). **Caddy answers requests for names it does not serve itself**, with an
empty `200`: the Workbench's `421` appears only for a `Host` that Caddy forwards but the Workbench does
not allow. Verified by the maintainers through the compose stack. Other proxies (nginx, Traefik,
cloud load balancers) follow the same rules; none was tested.

Keep the plain listener unreachable from outside: bind it to loopback or to an internal network,
and firewall the port.

## 8. Container isolation, step by step

In `isolation.mode = "container"` each job runs in a throw-away container from the runner image with
fixed hardening. This is the mode to use when executors might run code you do not trust, which, with
language models writing and running code, is always a question worth asking
([THREAT-MODEL.md](THREAT-MODEL.md)).

**What a container job gets.** Always: `--rm`, a name `h2w-job-<id>` and a label
`io.github.harness2-workbench.job=<id>`, `--cap-drop ALL`, `--security-opt no-new-privileges`,
`--pids-limit`, `--cpus`, `--memory`, the configured `--network`, and three bind mounts at fixed
points:

| Mount | Host side | Mode |
|---|---|---|
| `/h2w/harness2` | the harness2 checkout | read-only, except for `install_harness2` and `setup` |
| `/h2w/experiment` | the experiment directory | read-write |
| `/h2w/job` | a per-job directory with generated files (OpenCode config, a service-account key) | read-only |

**Variables.** Every variable a job needs (profile variables, secret values, `HARNESS2_RUNS_DIR`)
is written as `NAME=VALUE` lines to `<data_dir>/jobs/<id>/container.env` (mode 0600, deleted with the
job directory) and given to the runtime with `--env-file`. The runtime CLI itself runs with the
server's pass-through variables, its own settings (`DOCKER_HOST`, `DOCKER_CONTEXT`, `DOCKER_CONFIG`,
`XDG_RUNTIME_DIR`, `CONTAINER_HOST`, `CONTAINERS_CONF`, `REGISTRY_AUTH_FILE`) and `H2W_JOB_ID` only,
so no profile variable (one could steer the CLI, such as `DOCKER_CONTEXT`) and no secret value
reaches its environment or command line. The proxy settings of `isolation.container.proxy` are fixed
`-e NAME=VALUE` arguments (never secrets). The file format has no escapes: a value with a line
break or NUL, or a variable over 64 KiB, fails the job with an error that names the variable (never
the value). Checked with a stand-in `docker` script (not a real runtime) that records what it is
given: the `run` call held `--env-file .../container.env` and the fixed proxy settings and no
`-e MY_KEY`; the file, mode 600, held the profile variable and the secret; the script's environment
names held neither; the secret appeared nowhere in its log; the file was gone after the job.

**Hardening.** The image is pinned by tag or digest (`latest` is refused); the `host` network, a
writable job-files mount and a non-numeric user are refused; and the container's label is read and
compared with the job id **before** any stop, so a cancelled job stops its own container and nothing
else. Nothing else of the server's environment enters the container. Not set: a read-only root file
system, ulimits or a disk quota, a custom seccomp or AppArmor profile (the runtime's defaults apply).
On a **rootful** runtime the server user, who controls the runtime's socket, is root-equivalent on
the host: prefer rootless.

**The jobs network must be internal, and the server now checks.** With `isolation.container.proxy`
set, the server asks the runtime at start-up whether the network is internal
(`network inspect --format {{.Internal}}`) and logs, when it is not (or cannot be inspected), a
warning that starts `SECURITY:`; the same finding is the doctor check `container_network`
(`warn`; `h2workbench doctor` exits 1 while any check is `FAIL`, not for a warning). A job on a network
with a route out can ignore the proxy. Verified with the stand-in runtime answering `true` and then
`false`:

```text
INFO network h2w-jobs is internal: container jobs reach the internet only through the egress proxy http://h2w-egress:3128
WARN SECURITY: network h2w-jobs is NOT internal: container jobs can reach the internet directly, around the egress proxy http://h2w-egress:3128 remedy="Recreate it with `docker network create --internal h2w-jobs` and attach the egress proxy to it and to a network with internet access."
```

**The runner image is checked too.** The doctor check `runner_image` asks the runtime
(`image inspect`) for the configured image: `ok` when present, `fail` (the report is then not ok)
with the build command when not, `warn` when the runtime cannot be asked. Verified with the stand-in
runtime: `runner image harness2-workbench-runner:0.1.0 is not present for docker (`image inspect` exit
1); container jobs cannot start`, remedy `docker build -f deploy/docker/runner.Dockerfile -t
harness2-workbench-runner:0.1.0 .`. Because jobs use the image's toolchain, the host checks for uv,
Python 3.12, git, Node.js, npm, curl, Pandoc and Poppler are `skip` under container isolation, with
the detail `container isolation: jobs use the runner image's <tool>, not this host's`.

**Rootless and rootful runtimes** need different users. The experiment directory is bind-mounted
and must be writable by the job:

- *Rootless Docker or Podman*: container uid 0 **is** the server's unprivileged host user, and the
  only id that can write the mount (other ids map to subordinate ids that cannot). The server
  detects a rootless runtime (`docker info` security options contain `name=rootless`; Podman's
  `Host.Security.Rootless`) and runs jobs as `--user 0:0`, with every capability dropped.
- *Rootful runtime*: jobs run as the server's own `uid:gid`.
- Override with `isolation.container.user` (numeric). Because uid 0 without capabilities cannot
  write `/home/node`, the runner image sets `HOME=/h2w/home`, a sticky world-writable directory in
  the throw-away layer; **a custom runner image must give the job user a writable `HOME`** (uv, npm,
  Bun and OpenCode write caches there).

**Steps**, with the egress proxy on an internal network (the configuration the maintainers verified
on rootless Docker). The commands are from the example configuration's own comments; names
(`h2w-jobs`, `h2w-uplink`, `h2w-egress`) are yours to choose.

1. *Build the images* ([section 3.3](#33-container-images)).
2. *Create two networks*: an internal one with no route out for the jobs, and an uplink for the proxy.

   ```sh
   docker network create --internal h2w-jobs
   docker network create h2w-uplink
   ```

3. *Write the configuration* that both the Workbench and the proxy read, with the allowlist (everything
   the jobs may reach; [section 9](#9-the-egress-allowlist)). It holds no secrets; the proxy
   container's user must be able to read it (`chmod 644`).

   ```toml
   [isolation]
   mode = "container"

   [isolation.container]
   runtime = "docker"
   image = "harness2-workbench-runner:0.1.2"
   network = "h2w-jobs"
   proxy = "http://h2w-egress:3128"

   [egress]
   listen = "0.0.0.0:3128"            # the job containers must reach it: not the loopback default
   allow = ["github.com", "pypi.org", "files.pythonhosted.org",
            "aiplatform.googleapis.com", "oauth2.googleapis.com"]
   ```

4. *Start the egress proxy* on the internal network, then give it the uplink:

   ```sh
   docker run -d --name h2w-egress --network h2w-jobs \
     -v "$PWD/h2workbench.toml:/config/h2workbench.toml:ro" \
     harness2-workbench:0.1.2 egress-proxy --config /config/h2workbench.toml
   docker network connect h2w-uplink h2w-egress
   ```

5. *Start the Workbench* (on the host, as usual) with that configuration. `h2workbench doctor` should show
   `container_runtime`, `runner_image` and `container_network` ok, and `GET /api/v1/system/info`
   should report `"isolation": "container"` and `"container_runtime": "docker"`.
6. *Install harness2 in container mode.* A harness2 environment created on the host points at the
   **host's** Python and cannot run inside the runner image (see below), so run the `install_harness2`
   job (admin) **while the Workbench is in container mode**: the install then runs in the runner image
   and the virtual environment points at the image's interpreter. It needs `github.com`,
   `pypi.org` and `files.pythonhosted.org` on the allowlist. (Do not point a container-mode Workbench
   at a host-installed checkout.)
7. *Run the demo* as a container job and check `isolation` in the job, the `h2w-job-<id>` container
   while it runs, and the proxy's log (`docker logs h2w-egress`: `decision="allow"` and
   `decision="deny"` lines).

Restrictions in this version: WorkBuddy-Bench and the Codex bridge refuse container isolation,
and `network = "none"` allows only the offline demo. WorkBuddy-Bench starts a Docker container for
every trial, which a job inside a container cannot do: its cards say **needs host isolation** (the
Benchmarks API sets `unavailable` with reason `isolation`), and its jobs (setup, check, preview,
run, judge, report, pipeline, split) get `409` `WorkBuddy-Bench needs host isolation in this
version: …`. The bridge gets `409` `The Codex bridge needs host isolation in this version`. To
point people at a host-isolation Workbench that runs WorkBuddy-Bench, list it under
[`[benchmarks]`](#benchmarks-benchmarks-this-server-does-not-offer).

**What was verified.** The maintainers ran steps 2 to 7 on rootless Docker on 2026-10-23: the install
job fetched harness2 from GitHub and its packages from PyPI **through a real `egress-proxy`
container** (the proxy log showed only `github.com`, `pypi.org` and `files.pythonhosted.org`
allowed; `example.com`, `localhost` and an IP literal refused); the demo then ran as a container
job with its files owned by the server's user and read back through the API; a cancelled container
job was stopped by label and left no container. The documentation author verified, with the real
server and Docker's read-only commands only: that `isolation.mode = "container"` reports
`container_runtime: docker` and passes `doctor`, that `check` for WorkBuddy and `bridge` jobs get the
`409` above, and the failure mode below. **Not verified**: rootful Docker, Podman, an uplink-less
`none` network with the install job.

**The failure you will meet if you skip step 6.** With a harness2 installed on the host
(managed or not) and the Workbench switched to container mode, a job fails like this (real output):

```text
status: failed
error: exited with code 127: [FATAL tini (7)] exec /h2w/harness2/.venv/bin/harness2-demo failed: No such file or directory
log: system  ▶ demo: harness2-demo --output /h2w/experiment --k 1 --mode both
     stderr  [FATAL tini (7)] exec /h2w/harness2/.venv/bin/harness2-demo failed: No such file or directory
```

The script exists, but its first line names the host's Python, which the image does not have.
Reinstall harness2 in container mode (`install_harness2` with `reinstall: true`) or switch back to
host mode.

## 9. The egress allowlist

`h2workbench egress-proxy` is a forward proxy that lets job containers reach **only** the hosts and
ports you list. It speaks `CONNECT host:port` tunnels (HTTPS and anything else over TCP) and
absolute-form `http://` requests; every other request is `400`
`only CONNECT and absolute http:// requests are proxied`.

**Entries** (`[egress] allow`, first-match semantics per host and port):

| Entry | Meaning |
|---|---|
| `"pypi.org"` | exactly that host name, on ports **443 and 80** |
| `"*.example.net"` | any subdomain of `example.net` (not `example.net` itself), on 443 and 80 |
| `{ host = "litellm", allow_private = true, ports = [4000] }` | that host on port 4000 only; `allow_private` lets it resolve to a private address; an empty or missing `ports` means 443 and 80 |

- Host names are matched case-insensitively and without a trailing dot. **IP literals never match**
  and are rejected in the configuration.
- A host listed on some ports but requested on another is refused with `denied: port not allowed for
  this host`. A **later entry for the same host** can open another port, and each entry carries its
  own `allow_private`.
- `ports` may not contain 0 (`egress.allow[N].ports: 0 is not a port`).

**What the proxy checks before it connects**: it resolves the name, classifies **every** address
returned, and refuses the whole request if any is refused; it then dials only addresses it checked
(never a second lookup an attacker could race), in order, within the 10-second connect timeout, so an
unroutable IPv6 answer no longer fails a request that an IPv4 address could serve.

| Address class | Examples | Rule |
|---|---|---|
| public | everything else | allowed |
| private | RFC 1918, loopback, link-local (including cloud metadata addresses), CGNAT 100.64.0.0/10, unique-local and site-local IPv6 | refused unless the entry has `allow_private = true` |
| never | unspecified, multicast, broadcast, documentation, benchmarking, Teredo, IETF-reserved | always refused |

IPv4-mapped, NAT64 and 6to4 IPv6 addresses are judged by the IPv4 address they embed.

**What it tells the client** (real responses, verified on loopback with a local upstream):

| Situation | Status and body |
|---|---|
| host not listed (including a wildcard's apex) | `403` `denied: not on the allowlist` |
| host listed, port not | `403` `denied: port not allowed for this host` |
| IP literal | `403` `denied: only allowlisted host names are reachable` |
| resolves to a private address without `allow_private` | `403` `denied: resolves to a loopback address` (or the class, such as `private address`) |
| allowed, but the connection fails | `502` `denied: connect failed: Connection refused (os error 111)`; `504` for timeouts |
| not a proxy request | `400` `only CONNECT and absolute http:// requests are proxied` |

Refused clients get their answer and the connection closes. **Every decision is logged**, never a
body: `client`, `host`, `port`, `decision` (`allow`/`deny`) and `reason`. Start-up logs each
allowlist entry with its ports (`egress allowlist entry host=litellm ports=4000
allow_private=true`; verified, and the process is non-dumpable and exits 0 on SIGTERM):

```text
INFO egress client=127.0.0.1 host="localhost" port=47811 decision="allow" reason="allowlisted"
WARN egress client=127.0.0.1 host="localhost" port=47812 decision="deny" reason="port not allowed for this host"
WARN egress client=127.0.0.1 host="unlisted.example.org" port=443 decision="deny" reason="not on the allowlist"
WARN egress client=127.0.0.1 host="127.0.0.1" port=47811 decision="deny" reason="not a host name"
```

Fixed limits: 10 s to read a request head, 5 s DNS, 10 s connect, 300 s idle tunnel, 512
connections.

**What it is not.**

- It does **not authenticate clients**: anything that can reach its port can use it for the allowed
  hosts. Keep it on the internal jobs network, never on a shared one. That is why `egress.listen`
  defaults to `127.0.0.1:3128` (also in the file `init` writes): reaching it from job containers is
  an explicit choice, made with an address on the jobs network (`0.0.0.0:3128` inside the proxy's
  own container, as the compose example configuration does).
- It sees host names and ports, not content: HTTPS tunnels are opaque, so it cannot stop data
  leaving to a host you allowed (`*.googleapis.com` includes everyone's storage buckets). Prefer
  exact host names.
- It constrains only what is **forced through it**: a job on an `--internal` network cannot reach
  anything else, but a process on a network with a route out can ignore `HTTPS_PROXY`. Host-mode
  jobs have no such network control.
- Profiles' `egress_hosts` are documentation only; the server does not add them to this list.

**The allowlist is one list for install and for evaluation.** harness2's setup needs hosts that its
experiment guide says an evaluating executor should *not* reach, because public benchmark rubrics sit
there: the Workbench's own catalogue records the setup hosts (`github.com`, `codeload.github.com`,
`api.github.com`, `registry.npmjs.org`, `pkg.pr.new`, `huggingface.co`, `cdn-lfs.huggingface.co`,
`astral.sh`, `pypi.org`, `files.pythonhosted.org`; `api.github.com` and `pkg.pr.new` are missing
from upstream's list, and a LAB setup behind the proxy needed both for OpenCode's lockfile) and the
hosts upstream recommends blocking during evaluation
(`huggingface.co`, `hf.co`, `*.hf.co`, `github.com`, `raw.githubusercontent.com`). Allow what setup
needs, run setup, then **remove the blocked hosts and restart the proxy** before evaluation runs
(an experiment that reaches its own rubrics measures nothing). Record the restrictions you used
alongside any published result.

## 10. The compose reference deployment

`deploy/compose.yaml` runs the whole Workbench contained **from your host and network**: three
services, three networks. **It does not isolate the jobs from the Workbench**: they run in host
isolation inside the same container, as the same user, so a malicious or prompt-injected executor
can read and change the Workbench's data volume (the database, stored secrets, token hashes, the audit
chain) and, through the image's PID 1, the container's environment, and so can act as a Workbench
administrator. (The server makes itself non-dumpable, but the image starts it under `tini`
(`ENTRYPOINT ["/usr/bin/tini", "--", "h2workbench"]`), which stays dumpable and holds the container's
environment; a job of the same user can read `/proc/1/environ`. Read from the Dockerfile and the
Linux rules, not run.) What the container does is keep that executor away from your host's files,
your other containers and any network except the hosts on the allowlist. To isolate jobs from the Workbench, run `h2workbench` on a host with
container isolation ([section 8](#8-container-isolation-step-by-step)).

```text
browser --https--> ingress (Caddy, TLS) --> workbench --> egress proxy --> allowed hosts only
                   [edge + jobs nets]       [jobs net, internal]   [jobs + uplink nets]
```

- **`workbench`** (the all-in-one image) sits only on the `jobs` network, which is `internal: true`
  (no route out). Its jobs inherit `HTTP(S)_PROXY` and can reach exactly the hosts in `[egress]
  allow`. It runs as `node`, with `cap_drop: [ALL]`, `no-new-privileges`, 4096 pids, 16 GB and 8
  CPUs, and the volume `workbench-data` at `/data`.
- **`egress`** (the same image, command `egress-proxy`) is on `jobs` and `uplink`, read-only, with
  its own health check on port 3128 (the image's default check probes the Workbench's port). Its
  configuration sets `[egress] listen = "0.0.0.0:3128"` explicitly: the default is loopback.
- **`ingress`** (Caddy, pinned by digest) publishes 443 and 80 on **loopback by default**
  (`H2W_INGRESS_BIND=0.0.0.0` on a real server) and terminates TLS using `deploy/examples/Caddyfile`.

Jobs here run in **host** isolation *inside* the workbench container: the container is the boundary
around the Workbench and its jobs together, and the egress proxy is the only way out. That differs from
[section 8](#8-container-isolation-step-by-step), where each job gets its own container; see
[THREAT-MODEL.md](THREAT-MODEL.md) for what it means that a job and the server share a user and the
data volume. The example configuration's `trusted_proxies` covers the private ranges, which include
the jobs network: a job can therefore set the client address that the Workbench records in the audit
log and rate-limits by (narrow the list to the ingress's address once Docker has assigned it).

**Commands** (from the repository root; they are in the usage comment at the top of
`deploy/compose.yaml`):

```sh
cp deploy/examples/h2workbench.compose.toml deploy/h2workbench.toml   # then edit the EDIT lines
chmod 644 deploy/h2workbench.toml     # it holds no secrets; the container user (uid 1000) must read it
docker compose -f deploy/compose.yaml run --rm workbench init      # prints the first admin token
docker compose -f deploy/compose.yaml up -d
docker compose -f deploy/compose.yaml exec workbench h2workbench doctor
```

`deploy/examples/h2workbench.compose.toml` sets `listen = "0.0.0.0:8722"` (inside the container), your
`public_url` (the site address in the Caddyfile), `trusted_proxies` covering the private ranges
(narrow it to the ingress's subnet once known: `docker network inspect`), `isolation.mode = "host"`
and the egress allowlist, which **must include `github.com`, `pypi.org` and
`files.pythonhosted.org`** so harness2 can be installed, plus your model endpoints. Edit the domain
in `deploy/examples/Caddyfile` too. The image sets `H2W_CONFIG=/config/h2workbench.toml`, so `init`,
`doctor`, `audit verify` and `token create` work in the container without `--config`.

Notes:

- `init` prints the token **first** and treats the saved `admin.token` copy as best effort: the
  configuration is mounted read-only, so the warning `could not be saved to ...` is expected there. Copy
  the token from the output.
- Jobs and `doctor` probes also receive every `UV_*`, `PIP_*` and `NPM_CONFIG_*` variable of the
  server's environment, so the runner image's `UV_PYTHON_INSTALL_DIR` and a corporate package mirror
  (`UV_INDEX_URL`, `PIP_INDEX_URL`, `NPM_CONFIG_REGISTRY`) work. **A URL with credentials in those
  variables is readable by jobs.**
- Rootless Docker cannot publish ports below 1024 unless the host allows it
  (`net.ipv4.ip_unprivileged_port_start`); publish for example `8443:443` through an override file.
- If `up` fails with `all predefined address pools have been fully subnetted`, give the three
  networks explicit `ipam` subnets in an override file (choose unused /24 networks inside the ranges
  `trusted_proxies` covers):

  ```yaml
  # deploy/compose.override.yaml (suggested by the maintainers; not run by the author of this page)
  networks:
    jobs:   { ipam: { config: [ { subnet: <unused-private-/24-for-jobs> } ] } }
    uplink: { ipam: { config: [ { subnet: <unused-private-/24-for-uplink> } ] } }
    edge:   { ipam: { config: [ { subnet: <unused-private-/24-for-edge> } ] } }
  ```

- Refused egress shows in `docker compose logs egress` (`decision="deny"`); benchmark setup needs more
  hosts than installing harness2 does.

**Verified live by the maintainers** (rootless Docker, loopback, Caddy TLS ingress, 2026-10-23):
`compose run --rm workbench init` printed the token with a warning instead of locking out; through the
ingress `/healthz`, the UI page with its Content-Security-Policy, `421` for a Host that Caddy
forwards but the Workbench does not allow, `install_harness2` (git from `github.com`, packages from
PyPI, all through the egress proxy; no Python download), the demo, results, the SSE stream,
`audit verify` and `doctor` inside the container. Not re-run by the author of this page.

## 11. Observability

**Logs** go to standard error (standard output carries only machine-readable results).
`telemetry.log_format` is `pretty` (default) or `json` (one object per line); the verbosity is
`telemetry.log_level`. Start-up logs the resolved paths and a warning that host isolation runs jobs
as this user. Each request is one line: method, matched route, status, request id and latency;
never headers, cookies or bodies. `healthz`, `readyz` and `metrics` are logged at debug level.

```json
{"timestamp":"2026-10-24T03:48:38.983896Z","level":"INFO","fields":{"message":"request","latency_ms":1.0},"target":"h2w_daemon::http::guard","span":{"method":"GET","request_id":"1254656d1fbb421384b784a771f67b33","route":"/api/v1/auth/me","status":200,"name":"http"},"spans":[]}
```

Under systemd, `journalctl --user -u h2workbench -f`; under compose, `docker compose logs -f workbench`.
**Job logs** are files: `<data_dir>/logs/jobs/<job-id>.log`, one JSON object per line, mode 0600, at
most `jobs.max_log_bytes` each, secrets already redacted (best effort) and server paths masked
(also inside `file://` URLs, and in the text a result carries from the job's output); also served by
`GET /api/v1/jobs/{id}/log`. Verified with a stand-in job that printed 32 MiB against
`jobs.max_log_bytes = 65536`: the job still succeeded in about a second, the log file was 65,862
bytes, and it held the marker line and, at the end, `299639 lines (32.1 MiB) of output were not
stored (log limit)`.

**Traces.** With `telemetry.otlp_endpoint` set, request spans (method, matched route, status,
request id) are exported over OTLP/HTTP (protobuf) to `<endpoint>/v1/traces` with
`service.name = telemetry.service_name`. Verified by pointing the server at a local collector:
after a few requests it received `POST /v1/traces` with `application/x-protobuf`. Export needs the
`otel` build feature, which is on by default (`features` in `GET /api/v1/system/info` lists it); build
with `--no-default-features` to remove it.

**Metrics.** `GET /metrics` is Prometheus text (OpenMetrics). Auth: any viewer credential, **or**,
when `telemetry.metrics_token_secret` names a secret, that secret's value as a bearer token (it
unlocks `/metrics` and nothing else). Verified live:

```console
$ curl -s -o /dev/null -w '%{http_code}\n' $BASE/metrics                                  # no credentials
401
$ curl -s -o /dev/null -w '%{http_code}\n' -H "Authorization: Bearer $SCRAPE_SECRET" $BASE/metrics
200
$ curl -s -H "Authorization: Bearer $SCRAPE_SECRET" $BASE/api/v1/auth/me                 # it works nowhere else
{"error":{"code":"unauthorized","message":"invalid credentials", ...}}
```

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `h2w_http_requests_total` | counter | `method`, `route`, `status` | requests; `route` is the matched template (`/api/v1/jobs/{id}`), `unmatched` or `static` |
| `h2w_http_request_duration_seconds` | histogram | `method`, `route` | latency until the response head |
| `h2w_jobs_total` | counter | `kind`, `status` | jobs that reached a terminal status |
| `h2w_jobs_running` | gauge | | jobs running now |
| `h2w_job_duration_seconds` | histogram | `kind` | wall-clock duration of finished jobs |
| `h2w_spend_usd_total` | counter | `experiment` | improver spend recorded while jobs ran (at most 50 experiments are labelled; the rest are `other`) |
| `h2w_auth_failures_total` | counter | `reason` | refused requests: `missing`, `malformed`, `unknown_token`, `bad_secret`, `expired`, `revoked`, `session_invalid`, `proxy_untrusted`, `proxy_secret`, `proxy_user`, `proxy_duplicate_header`, `proxy_no_role`, `forbidden`, `rate_limited` (login limit, failed-authentication limit), `sse_per_principal`, `host`, `csrf`, `origin`, `unavailable` |
| `h2w_audit_suppressed_total` | counter | | records not written to the audit log because the client (failed authentication) or the principal (refused requests, `access.denied`) had used its 10 per minute |
| `h2w_connections_refused_total` | counter | `reason` | connections closed at accept: `total` (the server's 1,024) or `per_client` (`server.max_connections_per_ip`) |
| `h2w_sse_connections` | gauge | | open job event streams |

A family appears once it has a sample. Label sets are bounded by construction. A scrape job (not
exercised by the author of this page):

```yaml
scrape_configs:
  - job_name: harness2-workbench
    metrics_path: /metrics
    authorization: { credentials_file: /etc/prometheus/h2w-metrics.token }   # the metrics secret's value
    static_configs: [ { targets: ["workbench.internal.example:8722"] } ]
```

(Add the target's name to `server.allowed_hosts` if you scrape it by a name other than the ones
above.) Useful alerts: a rising `h2w_auth_failures_total{reason!="missing"}` or
`h2w_audit_suppressed_total`, any `h2w_connections_refused_total`,
`h2w_jobs_total{status="failed"}`, `h2w_sse_connections` near `server.sse_max_connections`, and
`/readyz` returning `503`.

**Health.** `GET /healthz` is `200 {"status":"ok"}` whenever the process answers. `GET /readyz`
is `200 {"ready":true,"failing":[]}` when the database answers and the job runner is alive, `503`
with the failing parts (`database`, `job_runner`) otherwise. Neither needs credentials and neither
reveals paths or versions.

## 12. Data, backups and upgrades

```text
<config dir>/h2workbench.toml            configuration (0600)   <config dir>/admin.token   bootstrap token (0600)
<data_dir>/workbench.db (+ -wal, -shm)   tokens (hashed), sessions, jobs, experiments, profiles, SECRETS, audit chain
<data_dir>/logs/jobs/<id>.log         job logs (0600), at most jobs.max_log_bytes each, deleted after jobs.log_retention_days
<data_dir>/jobs/<id>/                 per-job generated files (0700): OpenCode config, key file, container.env; deleted when the job ends
<data_dir>/splits/<name>.json         train/test split files
<experiments_dir>/<name>/             one harness2 results directory per experiment
<harness2_home>/                      the harness2 checkout, its .venv and dependencies/ (reproducible)
```

The data directory and every directory below it that the server writes (`logs`, `logs/jobs`, `jobs`,
`splits`, `experiments`) are set to mode 0700 at start-up, also when they already existed, for
example a volume mounted at `/data`.

**What to back up.** The database, the experiments directory, the configuration file, and any
`H2W_SECRET_*` values from wherever you keep them. Not the harness2 checkout (reinstall it; the
benchmark downloads are large) and not job logs unless you need them. **The database contains
secrets in plain text**, so a backup must be encrypted and access-controlled like the secrets
themselves.

**Backing up the database while the server runs.** SQLite's online-backup API gives a consistent
copy of a WAL database (copying `workbench.db` alone does not). This was run, with Python's standard
library, against a live server's database:

```sh
python3 - <<'EOF'
import sqlite3
src = sqlite3.connect("file:/var/lib/h2workbench/workbench.db?mode=ro", uri=True)
dst = sqlite3.connect("/backups/workbench-2026-10-24.db")
with dst:
    src.backup(dst)
EOF
chmod 600 /backups/workbench-2026-10-24.db
```

The `sqlite3` command-line equivalent is `sqlite3 workbench.db ".backup 'backup.db'"` (not run here: the
test machine had no `sqlite3` binary). Experiments are ordinary directories: `rsync -a` or `tar`
them; a running job writes into its experiment, so back up between jobs for a consistent copy.

**Restoring**: stop the server, put the database file in place (remove any stale `workbench.db-wal`
and `workbench.db-shm`), restore the experiments directory, start, and run `h2workbench audit verify`.
The backup copy above opened and verified (`audit chain OK: 39 entries; head 39 57e6450f...`).

**Upgrading.**

1. Back up (above) and read the [changelog](../CHANGELOG.md) for breaking changes: before 1.0 a
   minor version may break; patch versions do not.
2. Stop the server (jobs running are stopped and marked failed; plan a quiet moment), replace the
   binaries, start. The database **migrates itself** on start (`PRAGMA user_version`; verified on a
   copy of an older database: schema 1 became 2, which adds the boot id that restart recovery
   uses, and every row survived).
3. A database written by a *newer* build is refused: `the database was written by a newer
   Harness² Workbench (schema N, this build knows M)`. To go back, restore the pre-upgrade backup.
4. Each release is tested against one harness2 commit; `doctor` warns when yours differs. To move
   harness2, start `install_harness2` with `reinstall: true` in managed mode, or update your own
   checkout and re-run its setup. A `rev` other than the tested commit is refused (`422` on
   `params.rev`) unless `paths.allow_untested_harness2_rev = true`; with it, the job builds
   whatever it finds at that commit, so use only commits you trust (verified: with the flag on, a
   made-up commit started the job, which failed at `git fetch` with `not our ref`). The dependency cutoff stays at the tested commit's date, so a newer commit that
   needs newer packages fails to install in managed mode: install it yourself in an unmanaged
   checkout. Completed sweeps are unaffected, but a resumed sweep should use the harness2 it started
   with.
5. Restart `h2workbench-mcp` and re-check client configuration if the MCP tool list changed.

## 13. Audit log

Every authentication outcome, job create and cancel, experiment, profile, secret and token change,
and access denial is appended to a **SHA-256 hash chain** in the database: `hash = sha256(prev_hash
|| canonical JSON of the record)`. Records hold the actor, action, target, outcome, client address,
request id and a short detail (never a secret value or a request body). **The actor names its
kind:** `token:<name>` (an API token and its browser sessions), `proxy:<user>` (a user the reverse
proxy authenticated), `cli` (the command line), `server` (the server itself, for example when it
captures the bridge's key) or `anonymous` (a failed sign-in or authentication). A proxy user who
calls itself `initial admin` is therefore `proxy:initial admin`, never the bootstrap token's
`token:initial admin`. (`GET /auth/me` and the `created_by` of jobs and experiments keep the plain
name.) A request with **no**
credentials at all is only counted in `h2w_auth_failures_total`. A request that **presents** a
credential that is refused (even a junk bearer token) is recorded, but **at most 10 such records
per client and minute** are written (`auth.failed`, and denied `auth.login`); the rest are counted in
`h2w_audit_suppressed_total`, and past 60 failures a minute the client gets `429`
([section 6.5](#65-failed-authentication-is-throttled)). Before this limit, the independent review
had measured about 33 MB of audit rows for 140,000 junk requests; with it a client adds at most 600
rows an hour. Distinct client addresses each have their own allowance, so limit who can reach the
listener, and let the reverse proxy limit request rates.

**Refused requests** are limited the same way, per principal (a token with its sessions, or one proxy
user): at most 10 records a minute, the rest counted in `h2w_audit_suppressed_total`. That covers
`access.denied` (an authenticated caller without the role a route needs) and the job engine's
refusals, `job.create` and `job.cancel` with outcome `denied` (a spec that fails validation, a kind
above the caller's role, a cancel of a finished or unknown job); they share one allowance per
principal. Verified: 500 refused requests from a viewer token left 10 records and raised the counter
by 490; from an operator token, 300 refused `install_harness2` starts left 10 records, and 300
refused cancels and 5 refused requests after them added none (counter 590), while an administrator's
refusal was still recorded. Before this limit, 300 refused job requests added 300 records.

Successful actions are always recorded: a principal allowed to start jobs or change experiments adds
one record per change, as fast as the server accepts them. Give operator and admin tokens only to
people and tools you trust, and let the reverse proxy limit request rates.

```console
$ h2workbench audit verify
audit chain OK: 39 entries; head 39 57e6450ff2086c6118b46a3997a5f0356fc80c6e9805131864e512a9aca9adf6
$ # after editing the actor of record 5 in a copy of the database:
$ h2workbench audit verify
audit chain BROKEN: first bad record 5 of 39 entries          # exit status 1
```

The same check is `GET /api/v1/audit/verify` (admin); read the log with `GET /api/v1/audit` (newest
first, paged). **The log is a hash chain, not a tamper-proof record.** The chain is **unkeyed**: it
exposes edits to a record, a deleted record, a reordered record and a truncated chain **only if you
have seen the head hash before**, and only when the editor could not rewrite the whole chain.
Anyone with write access to the server's data directory (which includes every host-mode job, since
it runs as the server's user) can rewrite every record, recompute every hash, and `audit verify`
will report an intact chain. So record the head hash somewhere the server's user
cannot write: the server logs `audit chain head seq=... head=...` at every start, and `audit verify`
prints it. A cron job that runs `h2workbench audit verify` and ships its output to your log system is
enough. Exit status is 0 for an intact chain and 1 for a broken one.

## 14. Running as a systemd user service

`deploy/systemd/h2workbench.service` runs the server as you (no root) with modest hardening that is safe
in a *user* unit (`NoNewPrivileges`, `LockPersonality`, `RestrictRealtime`, `RestrictSUIDSGID`,
`SystemCallArchitectures=native`; `ProtectSystem`, `ProtectHome` and `PrivateTmp` are left out on
purpose, because in a user unit they can cut the server off from the harness2 checkout, your
experiments or a container runtime's socket).

```sh
install -Dm644 deploy/systemd/h2workbench.service ~/.config/systemd/user/h2workbench.service
systemctl --user daemon-reload
systemctl --user enable --now h2workbench
journalctl --user -u h2workbench -f
loginctl enable-linger "$USER"        # keep it running after you log out (needs admin rights)
```

`ExecStart=%h/.local/bin/h2workbench serve`, `KillMode=mixed`, `TimeoutStopSec=30`, `UMask=0077`,
`Restart=on-failure`. The unit parsed without errors under `systemd-analyze verify --user` (systemd
259); **the service itself was not started** by the author of this page. With container isolation on
rootless Docker add the runtime's socket to the unit, for example
`Environment=DOCKER_HOST=unix://%t/docker.sock` (the server passes `DOCKER_HOST`, `DOCKER_CONTEXT`,
`DOCKER_CONFIG`, `XDG_RUNTIME_DIR`, `CONTAINER_HOST`, `CONTAINERS_CONF` and `REGISTRY_AUTH_FILE` to
the runtime CLI); that line is untested. Secrets for the service belong in a file read by
`EnvironmentFile=` (mode 0600), not in the unit.

## 15. Hardening checklist

- [ ] The listener is on loopback or an internal network; TLS ends at a reverse proxy; `public_url`
      is the real URL; the proxy streams SSE without buffering.
- [ ] `trusted_proxies` lists the proxy's address(es), no wider.
- [ ] Every person and automation has its own token with the lowest role and an expiry; the bootstrap
      token is stored offline or revoked once named admins exist.
- [ ] Anything that runs code you did not write (any model-driven executor, which means all of
      them) uses container isolation with an internal network and the egress proxy, or a dedicated
      machine. In host mode, `acknowledge_risks` is required (`preacknowledge_host_risks = false`).
- [ ] The egress allowlist names exact hosts, has no broad wildcards, and drops the setup hosts
      (GitHub, Hugging Face) before evaluation runs.
- [ ] The egress proxy's port is reachable only from the jobs network (its default bind is
      loopback; you widened it on purpose).
- [ ] Secrets are supplied as `H2W_SECRET_*` from a secret store where possible; the database and
      its backups are encrypted and access-controlled; short-lived cloud credentials are preferred.
- [ ] The head hash of the audit chain is recorded off the server on a schedule.
- [ ] `/metrics` is scraped with the dedicated bearer secret; alerts exist for auth failures, failed
      jobs and `/readyz`.
- [ ] MCP clients get viewer tokens unless they must run jobs; an operator token only when the agent
      must start them, short-lived; **never an admin token**; and the cloud account behind the models
      has its own spend limit ([MCP.md](MCP.md#3-safety-rules)).
- [ ] The jobs network really is `--internal`: `h2workbench doctor` shows `container_network` ok (the
      server warns at start-up and in the doctor when it is not) and `runner_image` ok.
- [ ] Proxy mode: the proxy deletes every client-supplied identity and secret header and sets
      exactly one of each (the server refuses repeats with `401`); `trusted_proxies` is the proxy's
      own address.
- [ ] The reverse proxy limits connections and request rates per client: the server limits each
      client address (`server.max_connections_per_ip`, event streams per principal, failed
      authentication) but exempts the proxies in `trusted_proxies`, and its global caps (1,024
      connections, 64 event streams) are what is left.
- [ ] `server.public_url` is `https` only together with `trusted_proxies` (the server warns
      otherwise); `h2workbench config check` prints no warning you have not read.
- [ ] Every token has an expiry, the bootstrap token included.
- [ ] `jobs.timeout_hours`, `jobs.max_concurrent`, `jobs.max_log_bytes`, `jobs.min_free_disk_mb` and
      disk space are sized; `doctor` is clean.
- [ ] No `.env` file in the harness2 checkout (it bypasses profiles and secrets).
- [ ] The server runs as an unprivileged user, never root.

## 16. Command reference

| Command | Standard output | Notes |
|---|---|---|
| `h2workbench serve` | `h2workbench listening on http://<ip>:<port>` once bound | graceful on `SIGINT`/`SIGTERM` (the handlers are installed before that line is printed); makes itself non-dumpable first |
| `h2workbench init [--data-dir P] [--force]` | the new admin token, alone | see [section 4](#4-first-run) |
| `h2workbench token create --name N --role R [--expires-in-days D]` | the new token, alone | `R` is `viewer`, `operator` or `admin`; `TOKEN=$(h2workbench token create ...)` works |
| `h2workbench token list [--json]` | a table, or JSON | never secrets |
| `h2workbench token revoke ID` | nothing | exit 1 when no active token has that id |
| `h2workbench doctor [--json]` | the checks | exit 1 while a check is `FAIL` |
| `h2workbench config check` | `configuration OK` and the resolved paths | problems on standard error, exit 1; warns (exit 0) when proxy mode lacks its secret in the environment, or `public_url` is https with no `trusted_proxies` |
| `h2workbench audit verify` | `audit chain OK: N entries; head <seq> <hash>` or `audit chain BROKEN: ...` | exit 1 when broken; `no database at <path>` if absent |
| `h2workbench openapi` | the OpenAPI 3.1 document | needs neither configuration nor a server |
| `h2workbench version` | `h2workbench <version>` (and the commit when built with `H2W_BUILD_COMMIT`) | |
| `h2workbench egress-proxy` | `h2workbench egress proxy listening on <ip>:<port>` | [section 9](#9-the-egress-allowlist); non-dumpable; logs each allowlist entry with its ports |
| `h2workbench bundle export --output F [--with-runner-image] [--force]` | `<sha256>  <file name>` (also written to `F.sha256`) | [section 3.5](#35-offline-installation-bundles); refused while an install or setup job runs |
| `h2workbench bundle verify F [--sha256 HEX]` | `bundle OK: <sha256>`, or `problem: ...` lines | changes nothing; exit 1 when the bundle cannot be used here |
| `h2workbench bundle import F --sha256 HEX` | `installed <sha256>` | refused while any job is queued or running; the previous installation stays in `<harness2_home>.previous` |
| `h2workbench-mcp ...` | | [MCP.md](MCP.md) |

Every `h2workbench` command accepts `--config PATH` anywhere on the line. Explanations and logs go to
standard error; standard output is for results you can capture. A failure is one line,
`h2workbench: <what>: <cause>`, with each cause printed once.

The `doctor` checks (id: what it looks for): `harness2` (installed, at the tested commit), `uv`,
`python312`, `git`, `node20`, `npm`, `curl`, `docker`, `podman`, `pandoc`, `pdftoppm`, `gcloud`,
`adc`, `container_runtime`, `disk`, `dotenv`, and, in container isolation, `runner_image` and
`container_network`. What each means and how to fix it:
[TROUBLESHOOTING.md](TROUBLESHOOTING.md#h2workbench-doctor-checks).

## 17. What changed with the security review

An independent code review (by a separate AI agent, not a human penetration test) led to the changes
below before the first release, and a second review of the fixed code (2026-10-24) led to the rows
marked *second review*. This table is for anyone who ran an earlier build; each row says where this
guide describes the new behaviour. The exact messages are in
[TROUBLESHOOTING.md](TROUBLESHOOTING.md).

| Area | Before | Now |
|---|---|---|
| Failed authentication | every refused credential was audited and answered `401` | 60 a minute per client, then `429`; at most 10 audit records a minute per client; `h2w_audit_suppressed_total` ([6.5](#65-failed-authentication-is-throttled)) |
| Repeated proxy headers | the first user, the union of the groups | `401` ([6.3](#63-proxy-sso-mode)) |
| Repeated `Host` header | the first was checked | `400`; an absolute-form target must name an allowed host too ([7](#7-reverse-proxy-and-tls)) |
| Session cookie over https | `h2w_session` | `__Host-h2w_session`, so browsers sign in again after the switch ([6.2](#62-token-mode)) |
| `install_harness2` with a `rev` | any commit | the tested commit only, unless `paths.allow_untested_harness2_rev = true` ([5](#5-configuration-reference), [12](#12-data-backups-and-upgrades)) |
| harness2's Python dependencies | newest at install time | as of the tested commit's date (`--exclude-newer`) ([4](#4-first-run)) |
| Egress proxy default bind | `0.0.0.0:3128` | `127.0.0.1:3128`; the compose example sets `0.0.0.0` explicitly ([9](#9-the-egress-allowlist)) |
| Container job variables | `-e NAME`, values in the runtime CLI's environment | a 0600 `--env-file`; the CLI's environment holds no profile variable or secret ([8](#8-container-isolation-step-by-step)) |
| Jobs network | not checked | warning at start-up and doctor check `container_network`; doctor check `runner_image` ([8](#8-container-isolation-step-by-step)) |
| Job log size | unbounded | `jobs.max_log_bytes` (256 MiB) ([5](#5-configuration-reference)) |
| Free disk | not watched | running jobs stopped below `jobs.min_free_disk_mb` (256) ([5](#5-configuration-reference)) |
| Connections | 1,024 in total | also 256 per client address (`server.max_connections_per_ip`) ([5](#5-configuration-reference)) |
| Event streams | 64 in total, authorised once | also 8 per principal (`429`), credential checked at every heartbeat ([5](#5-configuration-reference)) |
| Restart recovery | recorded pid and start time | boot id, and `H2W_JOB_ID` in the group's environment ([THREAT-MODEL.md](THREAT-MODEL.md) D7) |
| The server's memory and environment | readable by a same-user job through `/proc` | the server and the egress proxy are non-dumpable (the note at the top, [10](#10-the-compose-reference-deployment)) |
| Results of hostile experiments | per-file read caps | bounded per request, 4 reads at once, `413 too large` ([API.md](API.md#6-paging-filters-and-limits)); *second review*: every results file is opened without blocking (a FIFO swapped in is refused, not waited on), a request has a read budget of bytes and sweeps, an abandoned request cancels its reads, a response is capped at 64 MiB, at most 200 sweeps are listed per experiment and a changed sweep may be shown as it was for one refresh |
| Harness fingerprint and manifest digest (*second review*) | read every file whatever its size | a file over its cap (4 MiB in the fingerprint; 4 MiB for a summary and 32 MiB for a judged file in the digest) is recorded as `<too large: N bytes>` and never read; more than 64 MiB (fingerprint) or 1 GiB (digest) in all is `413` |
| Job log pages and event-stream replay (*second review*) | up to 5,000 lines whatever their size | about 1 MiB (at least one line), with `next`; a reconnecting stream replays at most one page |
| Refused requests in the audit log (*second review*) | one record each | at most 10 a minute per principal, refused job starts and cancels included, the rest counted in `h2w_audit_suppressed_total` ([13](#13-audit-log)) |
| Audit actors (*second review*) | the plain name | `token:<name>`, `proxy:<user>`, `cli`, `server`, `anonymous` ([13](#13-audit-log)) |
| Limiter keys behind a trusted proxy (*second review*) | one table, 10,000 clients, one shared overflow bucket | per peer: 1,024 clients each, then that peer's own bucket ([6.5](#65-failed-authentication-is-throttled)) |
| Origin rule behind an `https` `public_url` (*second review*) | `http://<Host>` also passed; only the first `Origin` was read | only https origins; a repeated `Origin` header is `403` ([7](#7-reverse-proxy-and-tls)) |
| Profile `extra_env` | a short list of names | many more reserved ([MODELS.md](MODELS.md#reserved-environment-variables)); saving a profile that uses one is `422`, and creating a job with a profile stored earlier that uses one is refused (`422` on `params.profile`: `profile <name> no longer passes validation (...); an administrator must update it`) |
| Secret redaction | the literal value | also its base64 spellings ([6.4](#64-secrets)) |
| Host paths in job results and `file://` URLs | shown | masked ([THREAT-MODEL.md](THREAT-MODEL.md) I4) |
| Result files a job leaves (`demo.json`, split files) | read as they were | a symlink, FIFO or device is refused, over 16 MiB is refused |
| `GET /system/info` | | gains `host_risks_preacknowledged` ([API.md](API.md#4-roles-and-routes)); the web UI's run wizard asks nothing when it is `true` and its System page says so |
| Database schema | 1 | 2 (migrates itself, [12](#12-data-backups-and-upgrades)) |
| Data directories | 0700 when created | forced to 0700 at every start |
