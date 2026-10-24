# ADR 0002: A Rust workspace with shared wire types

- **Status:** Accepted. Part of the v1.0 contract ([ARCHITECTURE.md](../ARCHITECTURE.md) §2).
- **Applies to:** the whole repository layout.

## Context

The product has four faces that must agree exactly: a server with a REST API, a browser UI, an MCP
server for agents, and the domain logic that reads harness2's results. The server handles
credentials, starts processes and parses files that model-driven programs write, so memory safety and
strict input handling matter. A mismatch between what the server sends and what the UI or the MCP
server expects would be a bug that only shows at run time.

## Decision

One Cargo workspace, one language, one set of types:

| Crate | Role |
|---|---|
| `h2w-api` | the wire types and the contract constants; compiles for native and `wasm32-unknown-unknown`; data and pure helpers only (no I/O) |
| `h2w-core` | domain logic: command lines, parsers, results reader, harness trees, estimates, redaction, container argv, doctor checks; no sockets, no processes |
| `h2w-daemon` | the `h2workbench` binary: HTTP server, authentication, job engine, SQLite, egress proxy, command line |
| `h2w-web` | the UI: Leptos (client-side rendered), compiled to WebAssembly |
| `h2w-mcp` | the `h2workbench-mcp` binary, on the `rmcp` SDK |

Around them: `h2w-e2e` (end-to-end suites against real binaries), `tools/h2w-mock-api` (fixture API
for UI work), `xtask` (build orchestration and gates). The toolchain is pinned in
`rust-toolchain.toml`; `unsafe` is forbidden workspace-wide; `unwrap()` is denied outside tests.
Wire types live in `h2w-api` and are the contract: changes after the first release are additive.
The server generates its OpenAPI 3.1 document from the same structs.

## Consequences

Good:

- The UI, the MCP server and the server cannot disagree about a field: they compile against the same
  definitions, and the OpenAPI document is generated, not written.
- Memory safety and exhaustive matching where input is untrusted (HTTP bodies, harness2's files,
  proxy requests, log lines).
- Self-contained binaries: SQLite is bundled and TLS (for outbound calls) is rustls, so there is no
  OpenSSL dependency. (They link the C library dynamically: building for glibc and running on an
  older glibc fails. The contract's phrase "single static binaries" is therefore stronger than the
  result; see the build notes in [ADMIN-GUIDE.md](../ADMIN-GUIDE.md#31-from-source-verified).)
- One toolchain for contributors and for CI gates: `cargo xtask check` runs format, lint, tests, the
  dependency policy and the docs link check.

Bad:

- **Contributors need Rust** (and a WebAssembly toolchain for the UI); the barrier is higher than
  for a TypeScript project.
- **Build times and size.** A release build of the two binaries took 1 m 17 s with warm caches;
  clean builds are much longer; the UI adds a WebAssembly compile and `wasm-bindgen`, whose CLI
  version must equal the crate's.
- **Leptos is a young framework**; the UI pins a specific minor version and carries its upgrade risk.
- **A large dependency tree**, which is the reason for [ADR 0007](0007-dependency-cooldown.md).

## Alternatives considered

- **A TypeScript front end** (rejected in the original decision table): it duplicates every wire
  type in a second language, needs a second dependency ecosystem to audit, and moves the "types
  agree" guarantee from the compiler to discipline.
- **Server-side rendering** (rejected): more attack surface on the server (rendering model-written
  content), more state, and no benefit for a tool whose users are logged in and want live pages.

## Evidence in the repository

`Cargo.toml` (members, lints, the `release` profile), `rust-toolchain.toml`,
`crates/*/Cargo.toml`, `xtask/src/main.rs`, `deny.toml` (rustls only), the CI jobs in
`.github/workflows/ci.yml`.

## Revisit when

the UI's framework or the MSRV policy becomes a maintenance burden, or a second client (a CLI
front end, a mobile client) wants a published, versioned client library instead of a path
dependency.
