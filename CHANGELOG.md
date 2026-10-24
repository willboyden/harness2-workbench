# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

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
  at the newest line; results responses are built inside their read permit and refused with `413`
  past 64 MiB; every route has a read budget of bytes and sweeps; a request that goes away cancels
  its reads (`503`); at most 200 sweeps are listed per experiment; the sweep cache evicts the least
  recently used entry and serves a changed sweep as it was while it is read again.
- Second review: records of refused requests (`access.denied`, and refused job starts and cancels)
  are limited to 10 a minute per principal, one shared allowance; audit actors name their kind (`token:<name>`, `proxy:<user>`, `cli`, `server`,
  `anonymous`); the rate limiters are kept per trusted-proxy peer (1,024 clients each, then that
  peer's own overflow bucket); behind an `https` `public_url` only https origins are accepted and a
  repeated `Origin` header is `403`; the MCP HTTP guard refuses a `Host` with userinfo.
