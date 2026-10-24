# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Initial workspace: wire-type contract (`h2s-api`), domain library (`h2s-core`), server
  (`h2studio`), web UI (Leptos/WebAssembly), MCP server (`h2studio-mcp`), end-to-end tests and
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

Changes made after an independent review of the first implementation by a separate AI agent (no
human penetration test has been done). The fixed code has not been reviewed independently again.
Each change, its tests and how it was checked are in
[docs/ADMIN-GUIDE.md](docs/ADMIN-GUIDE.md#17-what-changed-with-the-security-review) and
[docs/THREAT-MODEL.md](docs/THREAT-MODEL.md).

- Sign-in: failed authentication is throttled per client address (60 a minute, then `429`); at most
  10 failures a minute per client are written to the audit log (the rest are counted in
  `h2s_audit_suppressed_total`); the rate-limit table never fails open; a token must match the exact
  format (ASCII only); a repeated proxy identity header is `401`; a repeated `Host` header is `400`
  and an absolute-form request target must name an allowed host; over https the session cookie is
  `__Host-h2s_session`; an open event stream re-checks its credential at every heartbeat.
- Limits: connections per client address (`server.max_connections_per_ip`, default 256, trusted
  proxies exempt) and event streams per principal (`server.sse_max_per_principal`, default 8);
  at most four results reads at once; a stored-log cap (`jobs.max_log_bytes`, default 256 MiB) and
  a free-disk floor that stops running jobs (`jobs.min_free_disk_mb`, default 256).
- Jobs: container jobs receive their variables through a 0600 `--env-file` instead of the runtime
  command's environment; the server and the egress proxy are non-dumpable, so a same-user process can
  no longer read their `/proc` environment; more environment names and prefixes are reserved for
  profiles; restart recovery signals a process group only after matching its boot id and an
  `H2S_JOB_ID` marker; `install_harness2` installs only the tested commit unless
  `paths.allow_untested_harness2_rev = true`, and resolves harness2's Python dependencies with
  `--exclude-newer`.
- Output: host paths in job results and `file://` URLs are masked; result files written by jobs are
  read without following symlinks and with a size limit.
- Egress and containers: the egress proxy listens on `127.0.0.1:3128` unless configured (the
  compose example sets `0.0.0.0:3128`); the server warns at start-up, and `doctor` reports
  (`container_network`), when the jobs network is not internal; `doctor` also checks the runner
  image (`runner_image`).
