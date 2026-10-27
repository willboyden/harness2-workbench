# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Judging with a cloud model: the profile editor's **Judge** part fills the judge from a provider
  with one click (**Claude (Anthropic API)**, **OpenAI**, **Google Gemini API**, **OpenRouter**, or
  the executor's own endpoint) and says what works with each benchmark.
- Every profile now says where its judge runs (`judge_site`: local, cloud with its provider and
  hosts, or unknown), shared by the server, the web UI and the MCP server. A cloud judge sees
  every task and output it grades, so the app warns that it should be used **only with generated
  or synthetic data**: in the profile editor as you type, on the profile's card, in the profile
  check, and before judging.
- Judge and pipeline jobs whose profile has a cloud judge need `acknowledge_cloud_judge: true`,
  the caller's confirmation that the experiment's data is generated or synthetic (`422` without
  it); the web UI asks with a checkbox, and the audit log records the confirmation. `[judge] cloud
  = "refuse"` refuses such jobs outright (`409`), and `[judge] private_hosts` declares internal
  gateways with public-looking names local. Run jobs never call the judge and are unaffected.
- `h2workbench-mcp --allow-cloud-judge-acknowledgement` (`H2W_MCP_ALLOW_CLOUD_JUDGE_ACK=1`): without
  it the MCP server refuses `acknowledge_cloud_judge=true`, because only the user knows what the
  data is.
- The profile check warns that harness2 judges `claude` and `gemini` ids for LAB on Vertex AI (so
  such a judge needs Vertex settings to judge LAB), and that JobBench's judge sends temperature 0,
  which Claude models from 4.7 on refuse.

### Changed

- `judge` and `pipeline` jobs with a profile whose judge is not local (including the upstream
  Vertex AI setup) are refused until `acknowledge_cloud_judge` is set. API clients that judge with
  a cloud model must add it.

## [0.1.3] - 2026-10-27

Measuring harness2 on self-hosted models under container isolation found an environment fault
that biased every LAB measurement and showed that single runs cannot tell refinement from noise;
this release fixes the first and makes both visible.

### Added

- `cargo xtask runner-docs --harness2-home <home>`: reads one .docx, .pptx and .xlsx document of
  the LAB checkout, and a generated PDF (no LAB task has shipped a PDF yet), with LAB's
  `read_doc.py` inside the runner image the way a container-isolated rollout does, and requires
  each result to equal in-process parsing. [RELEASING.md](docs/RELEASING.md) runs it after a
  harness2 bump.
- Compare against the starting harness: a comparison side may be a sweep's starting harness
  (`/compare?a=<experiment>/<sweep>/base`, the mean of its graded reference rollouts per task), so
  `a = base, b = parallel` asks whether refinement beat the harness it started from. The page
  offers it once the reference rollouts are judged, and a sweep's **Compare…** opens on it.
- A measured noise floor: a comparison carries `noise`, how far the starting harness moved between
  runs on the compared tasks (between two sweeps of experiments pinned to the same profile, or
  between the k reference rollouts of each task), and the page says whether the mean difference is
  within it. One rerun of a ten-task sweep showed why it matters: the same starting harness moved
  3.9 points per task on average, and the two refinements of it differed by up to 78 points on a
  task.
- Task rows carry `base` (the starting harness on that task: graded rollouts, mean, spread) and
  sweep summaries `base_score`; the sweep page shows a **Base score** column, each mode's
  difference from it, and a **base** chip in the headline. The MCP tool `compare_sweeps` accepts
  `base` as a side's mode.

- Choose exactly which tasks run: `POST /api/v1/splits` saves a split of tasks chosen by id
  (`train`, and optionally `test`), in harness2's split format, and a run with `split: train` and
  that `split_file` runs exactly those tasks. The Benchmarks page's **New split…** offers *Chosen
  tasks* beside *Random*; the MCP tool `choose_split` does the same. harness2 has no task-id
  option at the tested commit, and with a seed `max_tasks` takes the first tasks of the shuffled
  order, so this was the missing way to run specific tasks (for example JobBench's tasks that need
  no internet).

### Changed

- The default runner image is `harness2-workbench-runner:0.1.3` (the tag follows the Workbench
  version), and the documentation, Dockerfiles, compose file and CI build that tag. **Rebuild the
  runner image**: it gained the `podman` stand-in (see Fixed).

### Fixed

- LAB under container isolation: LAB's document reader (`scripts/read_doc.py`) parses .docx,
  .pdf, .pptx and .xlsx files with `podman run`, which a job inside a container cannot do, so every
  such read failed (`FileNotFoundError: [Errno 2] No such file or directory: 'podman'`) until the
  agent found `--in host` by itself. In one container-isolated experiment, 29 of 37 base rollouts
  lost turns this way, against 22 of 108 rollouts of candidate and refined harnesses, which had
  learned the workaround; LAB results from container isolation therefore understated the base
  harness, so compare them with care. The runner image now ships a `podman` stand-in
  (`deploy/docker/podman-standin.sh`) that serves exactly that call, running LAB's own parser on
  the mounted file inside the job container (itself the sandbox LAB asks for: no capabilities, no
  network but the egress proxy), and refuses every other call with podman's status 125.
  **Rebuild the runner image.**
- The run options described `max_tasks` as "the first N tasks of every domain"; it takes them in
  stream order, which `seed` shuffles, so with a seed they are usually not the lowest-numbered
  tasks. The API description, the New run wizard and USER-GUIDE now say so.
- JobBench judging that rate limits turn into ungraded rows is now explained in TROUBLESHOOTING:
  one judging step opens about 64 judge requests at once.

## [0.1.2] - 2026-10-27

Running the benchmarks on two Workbenches, one with container isolation and one with host isolation
for WorkBuddy-Bench, showed each server listing the other's benchmarks as "not set up"; this release
says where each benchmark runs.

### Added

- Benchmark availability: an entry of `GET /api/v1/benchmarks` carries `unavailable` (reason
  `not_offered` or `isolation`, a message, and an optional link) when the server does not set up or
  run that benchmark, and jobs for it are refused with the same message (`409`). Administrators list
  the benchmarks a server does not offer under `[[benchmarks.not_offered]]`, with a note and a link
  to the Workbench that runs them
  ([ADMIN-GUIDE §5](docs/ADMIN-GUIDE.md#benchmarks-benchmarks-this-server-does-not-offer)), so two
  Workbenches that split the benchmarks between them point at each other.

### Changed

- The Benchmarks page shows a benchmark the server does not run as **needs host isolation**
  (WorkBuddy-Bench under container isolation) or **not on this server**, with the reason and the
  link, instead of "not set up" with setup and check buttons the server would refuse. The New run
  wizard stops at its first step for such a benchmark, train/test splits are offered only for
  benchmarks the server runs, and Getting started counts only those.
- Container isolation's refusals are separate now: WorkBuddy jobs get `WorkBuddy-Bench needs host
  isolation in this version: …`, the bridge `The Codex bridge needs host isolation in this
  version` (one message covered both before).
- The MCP tool `list_benchmarks` describes `unavailable`.
- The default runner image is `harness2-workbench-runner:0.1.2` (the tag follows the Workbench
  version), and the documentation, Dockerfiles, compose file and CI build that tag. The runner's
  Dockerfile did not change since 0.1.1, so `docker tag harness2-workbench-runner:0.1.1
  harness2-workbench-runner:0.1.2` is enough; or rebuild it, or keep `isolation.container.image`
  pointing at an image you built yourself.

## [0.1.1] - 2026-10-26

The first model-backed run through the Workbench (one LAB task on self-hosted models behind LiteLLM,
in container isolation) found the fixes below; this release also adds offline install bundles.

### Added

- Offline install bundles ([ADR 0009](docs/adr/0009-offline-install-bundles.md),
  [ADMIN-GUIDE §3.5](docs/ADMIN-GUIDE.md#35-offline-installation-bundles)): `h2workbench bundle
  export` writes the managed harness2 installation (checkout, virtual environment, benchmark
  dependencies and data), optionally with the runner image, to one archive with a manifest and a
  `.sha256` file; `bundle verify` checks one without changing anything; `bundle import` installs one
  after comparing the SHA-256 the administrator supplies, checking every entry (no absolute or `..`
  paths, nothing below a link, links that stay inside, no hard links or devices, no setuid bits) and
  the bundle's fit (architecture, harness2 commit, interpreter), and keeps the previous installation.
  Command line only; audited as `bundle.export` and `bundle.import`.

### Changed

- The default runner image is `harness2-workbench-runner:<Workbench version>`
  (`harness2-workbench-runner:0.1.1` here, `0.1.0` before), and the documentation, Dockerfiles,
  compose file and CI build that tag: the image changed (it now ships ripgrep). **Rebuild the runner
  image** for 0.1.1 (`docker build -f deploy/docker/runner.Dockerfile -t
  harness2-workbench-runner:0.1.1 .`), or keep `isolation.container.image` pointing at an image you
  built yourself.

### Fixed

- ADMIN-GUIDE §3.2 still called the release archives unpublished; it now says what was verified for
  v0.1.0.

Found by the first model-backed run (LAB, container isolation, an egress allowlist holding only an
OpenAI-compatible gateway):

- OpenCode sessions of `openai_compatible` profiles exited before their first turn when they could
  not reach `models.dev`: harness2 runs OpenCode from source, which has no bundled model catalogue,
  fetches one at start-up and JSON-parses the egress proxy's refusal. The server now sets
  `OPENCODE_DISABLE_MODELS_FETCH=1` and `OPENCODE_DISABLE_LSP_DOWNLOAD=1` for these profiles; the
  generated provider needs nothing from the catalogue. Vertex profiles keep the fetch
  (docs/MODELS.md says to allow `models.dev` for them in container isolation).
- The runner image ships ripgrep, so OpenCode's grep, glob and list tools no longer try to download
  it from GitHub.
- The setup-host catalogue lists `api.github.com` and `pkg.pr.new`, which OpenCode's lockfile needs
  during LAB setup and upstream's list leaves out.
- `doctor` no longer fails a container-isolated server for a host without uv, Python 3.12, git,
  Node.js, npm, curl, Pandoc or Poppler: jobs use the runner image's copies, so those checks are
  skipped with the reason (the `runner_image` check covers the image).

## [0.1.0] - 2026-10-24

The first release.

### Added

- Initial workspace: wire-type contract (`h2w-api`), domain library (`h2w-core`), server
  (`h2workbench`), web UI (Leptos/WebAssembly), MCP server (`h2workbench-mcp`), end-to-end tests and
  build tooling (`cargo xtask`).
- Supply-chain controls: 14-day dependency cooldown with security-fix exceptions, cargo-deny
  policy, SHA-pinned GitHub Actions, gitleaks, CodeQL, OpenSSF Scorecard, SBOM and build
  provenance for releases.
- Deployment artifacts: runner and all-in-one container images, a contained reference Compose
  stack with an egress allowlist proxy and TLS ingress, and a systemd user unit.
- Documentation: README with a feature matrix that separates what is implemented from what was
  verified live; web UI user guide with screenshots; administrator's guide with the full
  configuration reference; REST API and MCP guides; model-profile guide (Vertex AI and
  OpenAI-compatible endpoints); concepts and glossary; threat model (STRIDE, trust boundaries,
  residual risks); troubleshooting indexed by exact message; eight architecture decision records.
- `cargo xtask docs`: fails on a broken relative link or `#anchor` in any Markdown file (GitHub's
  heading-anchor rules) and on a missing required document (the README that release archives
  contain, the guides the governance files point to); it is also one of the gates of
  `cargo xtask check`.
- Third-party licences: release archives and the all-in-one image carry
  `THIRD-PARTY-LICENSES.md`, the licence texts of every crate compiled into the servers and the
  web UI (`cargo xtask licenses`, cargo-about 0.9.2); the runner image carries uv's licence texts.

### Security

Changes made after two independent reviews of the code by a separate AI agent (no human penetration
test has been done): the first of the initial implementation, the second (2026-10-24) of the fixed
code. The second confirmed the first round's fixes (some are partial or by design, as the threat
model says) and found ten more issues; **those fixes have not been re-reviewed independently**.
Each change, its tests and how it was checked are in
[docs/ADMIN-GUIDE.md](docs/ADMIN-GUIDE.md#17-what-changed-with-the-security-review) and
[docs/THREAT-MODEL.md](docs/THREAT-MODEL.md).

- Sign-in: failed authentication is throttled per client address (60 a minute, then `429`); at most
  10 failures a minute per client are written to the audit log (the rest are counted in
  `h2w_audit_suppressed_total`); the rate-limit table never fails open; a token must match the exact
  format (ASCII only); a repeated proxy identity header is `401`; a repeated `Host` header is `400`
  and an absolute-form request target must name an allowed host; over https the session cookie is
  `__Host-h2w_session`; an open event stream re-checks its credential at every heartbeat.
- Limits: connections per client address (`server.max_connections_per_ip`, default 256, trusted
  proxies exempt) and event streams per principal (`server.sse_max_per_principal`, default 8);
  at most four results reads at once; a stored-log cap (`jobs.max_log_bytes`, default 256 MiB) and
  a free-disk floor that stops running jobs (`jobs.min_free_disk_mb`, default 256).
- Jobs: container jobs receive their variables through a 0600 `--env-file` instead of the runtime
  command's environment; the server and the egress proxy are non-dumpable, so a same-user process can
  no longer read their `/proc` environment; more environment names and prefixes are reserved for
  profiles; restart recovery signals a process group only after matching its boot id and an
  `H2W_JOB_ID` marker; `install_harness2` installs only the tested commit unless
  `paths.allow_untested_harness2_rev = true`, and resolves harness2's Python dependencies with
  `--exclude-newer`.
- Output: host paths in job results and `file://` URLs are masked; result files written by jobs are
  read without following symlinks and with a size limit.
- Egress and containers: the egress proxy listens on `127.0.0.1:3128` unless configured (the
  compose example sets `0.0.0.0:3128`); the server warns at start-up, and `doctor` reports
  (`container_network`), when the jobs network is not internal; `doctor` also checks the runner
  image (`runner_image`).
- Second review: results files are opened without blocking (`O_NONBLOCK`, `O_NOFOLLOW`, then an
  `fstat` check of type and inode), so a FIFO swapped in after the check is refused instead of
  hanging a read; the harness fingerprint and the manifest digest never read a file over its cap (it
  is recorded as `<too large: N bytes>`) and refuse more than 64 MiB and 1 GiB in all.
- Second review: job-log pages and event-stream replays stop near 1 MiB (`next` says where to go
  on), and the MCP `get_job_log tail=N` and job-log resource follow those pages, so they still end
  at the newest line; the web UI's job page fetches only the lines it keeps (the newest 20,000, at
  most about 16 MiB), so it reaches the end of a long log; results responses are built inside their
  read permit and refused with `413`
  past 64 MiB; every route has a read budget of bytes and sweeps; a request that goes away cancels
  its reads (`503`); at most 200 sweeps are listed per experiment; the sweep cache evicts the least
  recently used entry and serves a changed sweep as it was while it is read again.
- Second review: records of refused requests (`access.denied`, and refused job starts and cancels)
  are limited to 10 a minute per principal, one shared allowance; audit actors name their kind
  (`token:<name>`, `proxy:<user>`, `cli`, `server`, `anonymous`); the rate limiters are kept per
  trusted-proxy peer (1,024 clients each, then that peer's own overflow bucket); behind an `https`
  `public_url` only https origins are accepted and a repeated `Origin` header is `403`; the MCP
  HTTP guard refuses a `Host` with userinfo.
- Restart recovery no longer counts a job process caught in the middle of an exec as unmarked:
  an environment that reads empty or denied for that moment is read again for up to about 0.25 s
  (found by a test that failed once on a CI runner).

[Unreleased]: https://github.com/willboyden/harness2-workbench/compare/v0.1.3...HEAD
[0.1.3]: https://github.com/willboyden/harness2-workbench/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/willboyden/harness2-workbench/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/willboyden/harness2-workbench/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/willboyden/harness2-workbench/releases/tag/v0.1.0
