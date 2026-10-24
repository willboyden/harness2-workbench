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
