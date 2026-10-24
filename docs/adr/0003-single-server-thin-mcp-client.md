# ADR 0003: One server owns jobs, state and authentication; MCP is a thin client

- **Status:** Accepted. Part of the v1.0 contract ([ARCHITECTURE.md](../ARCHITECTURE.md) §2, §13, §16).
- **Applies to:** `h2s-daemon`, `h2s-mcp`.

## Context

AI agents need the same operations as people: set up a benchmark, preview and run a sweep, wait for
it, read the results. The people-facing path already needs authentication, roles, a job queue with
cancellation and budgets, secrets handling and an audit log. An agent-facing path is the more
dangerous of the two, because its input is shaped by model output and by documents the model reads
(prompt injection), so it is the last place to put extra authority.

## Decision

`h2studio serve` is the **only** component that authenticates, authorises, runs jobs, writes the
database and appends to the audit log. `h2studio-mcp` is a **thin client** of its REST API:

- it keeps no state of its own and runs no jobs;
- every call it makes carries an API token. In stdio mode that is the one token it was started with
  (from the environment or a 0600 file, never from the command line). Over streamable HTTP it is the
  **caller's own bearer token**, forwarded unchanged, so the server's role checks and audit records
  apply to each client individually;
- it exposes tools that are one REST capability each, and it **never** exposes writes to profiles,
  secrets, tokens or the audit log (contract addendum v1.1), because a prompt-injected agent that
  could edit a profile could repoint its endpoint at a hostile URL and receive the API key it
  references on the next job;
- it adds only limits that belong to being an agent front end: bounded waits (`wait_secs`, at most
  600 s; a few concurrent waits per token), a cap on concurrent HTTP requests, size limits on
  results, and refusal to send a token over plain `http` to a non-loopback host.

## Consequences

Good:

- **One enforcement point.** Roles, the Host and Origin rules, rate limits, validation and audit are
  implemented once. The MCP server cannot do anything the token's role forbids, and cannot be
  configured to.
- **Per-client attribution.** The audit log records the token that acted, not "the MCP server".
- The MCP server is stateless and disposable: restart it, run several, put it anywhere that can reach
  the Studio.
- Smaller privileged surface: the component that talks to an LLM holds no database, no secrets, no
  process control.

Bad, and how it is managed:

- **No offline mode.** MCP needs a reachable, running server; a connection failure is reported as
  `unavailable: cannot connect to the Harness² Studio server at ...`.
- **Every agent capability needs a REST route first**, and contract changes are additive in both.
- **An extra hop** and a second place where limits live (the MCP server's wait and request caps do not
  replace the server's own limits).
- **The HTTP transport sees every caller's token**, so it must sit behind TLS beyond loopback; it
  warns when it listens elsewhere and refuses an insecure `--server-url`.
- **The stateless transport** (no `Mcp-Session-Id`) gives up server-initiated sessions; long jobs are
  followed by `wait_for_job` with progress notifications instead.
- **A token with the operator role is still powerful** when an agent holds it: the structure bounds
  what an injected agent can reach, not what the token's role allows ([THREAT-MODEL.md](../THREAT-MODEL.md)).

## Alternatives considered

- **An MCP server with its own job runner** (rejected in the original decision table). It would be a
  second authority with its own authentication, queue and audit trail to keep consistent with the
  first, and it would put process-control power inside the component most exposed to model-shaped
  input.
- **Giving MCP an administrator token to act on behalf of users** (implicit in the rejection above, and
  a variation worth naming): it would collapse every agent into one identity in the audit log and turn
  any bug in the MCP layer into an admin compromise.

## Evidence in the repository

`crates/h2s-mcp/src/server.rs` (tools), `client.rs` (token handling), `http.rs` (guard and token
forwarding), `limits.rs`; the tool contract is frozen by
`crates/h2s-mcp/tests/snapshots/tool_schemas.json`; the role refusals are exercised in
`crates/h2s-e2e/tests/mcp.rs`.

## Revisit when

MCP gains first-class authorisation (scoped tokens, user consent flows) that the server can
verify, or a client needs server-initiated sessions that a stateless endpoint cannot provide.
