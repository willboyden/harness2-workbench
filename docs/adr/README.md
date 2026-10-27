# Architecture decision records

Each record states the context a decision was made in, the decision, what it costs, and what was
rejected. The one-line summaries of the original eight decisions are in
[ARCHITECTURE.md](../ARCHITECTURE.md) §2 (the build contract); these are the full records.

| # | Decision | Status |
|---|---|---|
| [0001](0001-wrap-the-harness2-cli.md) | Wrap the harness2 CLI (argv, no shell) and read its files; never import its Python | Accepted |
| [0002](0002-rust-workspace.md) | A Rust workspace: `h2w-api`, `h2w-core`, `h2w-daemon`, `h2w-web` (Leptos to WASM), `h2w-mcp` | Accepted |
| [0003](0003-single-server-thin-mcp-client.md) | One server owns jobs, state and authentication; the MCP server is a thin client forwarding the caller's token | Accepted |
| [0004](0004-sqlite-and-plain-experiment-directories.md) | SQLite (bundled, WAL) for state; files for logs; experiments stay plain harness2 directories | Accepted |
| [0005](0005-strict-csp-wasm-ui.md) | A strict Content-Security-Policy; WebAssembly loaded by an external script; nothing inline | Accepted |
| [0006](0006-isolation-modes.md) | Two isolation modes: `host` (with explicit acknowledgement) and `container` (hardened runner, egress allowlist proxy) | Accepted |
| [0007](0007-dependency-cooldown.md) | Dependency cooldown: locked versions at least 14 days old, security fixes exempt with an expiry | Accepted |
| [0008](0008-api-tokens-and-sessions.md) | Tokens `h2w_<id>_<secret>` stored as SHA-256 and compared in constant time; HttpOnly SameSite=Strict cookie sessions | Accepted |
| [0009](0009-offline-install-bundles.md) | Offline install bundles: export, verify and import the managed installation as one checked archive, from the command line only | Accepted |

## Format

Every record has the same sections: **Status** (and what it is part of), **Context**, **Decision**,
**Consequences** (good, then bad with how it is managed), **Alternatives considered** (with why each
was rejected, and the ones the contract recorded kept distinct from others), **Evidence in the
repository** (files and tests), and **Revisit when**.

## Changing a decision

Do not edit an accepted record to say something different. Add a new record that supersedes it,
change the old record's status to `Superseded by NNNN`, and update the summary table in
[ARCHITECTURE.md](../ARCHITECTURE.md) §2 and this index. The contract rules still apply: the REST API
and MCP tools change only additively within a minor version, recorded in ARCHITECTURE.md §16 and the
[changelog](../../CHANGELOG.md).
