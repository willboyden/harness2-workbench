# Security policy

Harness² Studio starts processes, holds model-provider credentials and serves an admin UI, so
security reports are welcome and taken seriously.

## Reporting a vulnerability

**Please do not open a public issue.** Report privately through GitHub:
**Security → Advisories → Report a vulnerability** on this repository
(<https://github.com/willboyden/harness2-studio/security/advisories/new>).

Include what an attacker needs (network position, role, configuration), the impact, and steps or
a proof of concept. Leave real tokens and keys out of the report.

What to expect (this is a volunteer project, so these are best-effort targets):

- acknowledgement within 7 days;
- an assessment and, for confirmed issues, a fix plan within 30 days;
- credit in the advisory and the changelog, unless you prefer otherwise.

Please give us a reasonable chance to release a fix before disclosing publicly.

## Supported versions

Until 1.0, only the latest minor release receives security fixes.

| Version | Supported |
|---|---|
| 0.1.x | yes |

## Scope

In scope: the `h2studio` server (REST API, authentication, authorization, job execution, the
egress proxy), the web UI, the `h2studio-mcp` MCP server, the deployment artifacts in `deploy/`,
and the release pipeline.

Out of scope here, but please report to the right place:

- harness2 itself → <https://github.com/google-research/harness2> (upstream states it is not an
  officially supported Google product);
- the benchmarks harness2 installs (JobBench, LAB, WorkBuddy-Bench) → their own repositories;
- third-party crates → the crate's maintainers and <https://rustsec.org> (and tell us, so we can
  update).

## Design notes for reviewers

- The threat model is in [docs/THREAT-MODEL.md](docs/THREAT-MODEL.md) and the security
  architecture in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) §9.
- **Host isolation is upstream's behaviour and is powerful by design:** harness2's executors
  (OpenCode, Codex with its sandbox disabled) run agent tool calls with the server user's files
  and network. The Studio requires an explicit acknowledgement for such runs and offers container
  isolation with an egress allowlist. A finding of the form "an executor in host mode can read the
  server user's files" is expected behaviour, not a vulnerability; a way around the
  acknowledgement, the container hardening or the egress allowlist is.
- Supply-chain controls are described in [docs/SUPPLY-CHAIN.md](docs/SUPPLY-CHAIN.md).
