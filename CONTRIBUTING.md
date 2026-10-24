# Contributing to Harness² Workbench

Thanks for helping. This guide covers the development setup, the git workflow and the bar every
change has to meet. By contributing you agree that your contributions are licensed under the
[Apache License 2.0](LICENSE) (section 5) and that you follow the
[Code of Conduct](CODE_OF_CONDUCT.md).

## Development setup

Linux is required: for harness2 itself, and for the Workbench's server (it reads `/proc` and uses
Linux process controls). CI builds and tests on Linux x86_64.

```bash
# Rust: the exact toolchain is pinned in rust-toolchain.toml and installed automatically.
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # or your distro's rustup
cargo install --locked wasm-bindgen-cli --version 0.2.129        # must match the wasm-bindgen crate
cargo install --locked cargo-deny --version 0.20.2

cargo xtask hooks      # use the repository's git hooks (rustfmt + leak scan, commit messages)
cargo xtask check      # every quality gate; run before you push
```

Run it locally:

```bash
cargo xtask web                                  # build the UI into dist/web (embedded by the server)
cargo run -p h2w-daemon -- init                  # config + first admin token (printed once)
cargo run -p h2w-daemon -- serve                 # http://127.0.0.1:8722
```

UI work without a server: `cargo run -p h2w-mock-api -- --port 8799 --dist dist/web` serves
fixture data with the production Content-Security-Policy.

End-to-end tests need a harness2 checkout with its virtualenv, Firefox and geckodriver; see
`crates/h2w-e2e/README.md`.

## Git workflow

- `main` is always releasable and protected: changes arrive through pull requests with green CI
  and at least one maintainer review.
- Branch names: `feat/<topic>`, `fix/<topic>`, `docs/<topic>`, `chore/<topic>`, `security/<topic>`.
- Commits follow [Conventional Commits](https://www.conventionalcommits.org/):
  `<type>(<scope>): <description>`, with types `build chore ci docs feat fix perf refactor
  revert security style test` and scopes such as `api core daemon web mcp e2e xtask deploy`.
  The `commit-msg` hook enforces the subject line. Mark breaking changes with `!` and a
  `BREAKING CHANGE:` footer.
- Keep pull requests focused. They are squash-merged with the PR title as the commit subject;
  large multi-part features may be merged with a merge commit after review.
- Stage explicit paths (`git add crates/h2w-core/src/tree.rs`), so you see exactly what you
  commit. Never commit `.env` files, databases, tokens or generated `dist/` output.
- Signed commits and tags are encouraged (`git commit -S`, `git tag -s`).

## The bar for every change

- `cargo xtask check` passes: rustfmt, the documentation link check (`cargo xtask docs`), clippy with
  `-D warnings` (native and wasm32), all tests, `cargo deny check`, and the dependency cooldown.
- No `unsafe` (forbidden workspace-wide), no `unwrap()` outside tests, no `todo!()`.
- Tests next to the code; property tests (proptest) for anything that parses or validates input;
  integration tests for HTTP behaviour; UI changes come with light/dark screenshots at desktop
  and phone widths in the PR.
- Errors are actionable sentences for the user; logs never contain secrets, tokens, cookies or
  request bodies.
- Security-sensitive changes (authentication, roles, job execution, filesystem access, the egress
  proxy, the CSP) update [docs/THREAT-MODEL.md](docs/THREAT-MODEL.md) and explain the impact in
  the PR.
- The REST API and MCP tool contract change only additively within a minor version; record
  changes in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) §16 and the
  [changelog](CHANGELOG.md) under *Unreleased*.
- New dependencies must be justified, pass `cargo deny check` and be at least 14 days old
  ([docs/SUPPLY-CHAIN.md](docs/SUPPLY-CHAIN.md)).
- Documentation moves with the code. A change that alters a configuration key, an error message, a
  route, an MCP tool or a default updates [docs/ADMIN-GUIDE.md](docs/ADMIN-GUIDE.md),
  [docs/API.md](docs/API.md), [docs/MCP.md](docs/MCP.md) or
  [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md) in the same pull request. State what you ran: write
  *verified live* (and against what), *fakes only* or *unverified*, never a date for something you did not
  run. Paste real command output, trimmed, with tokens shown as `h2w_xxxxxxxx_<redacted>`, and use
  `example.com`, `192.0.2.x` and `2001:db8::` for example addresses; no personal paths, names or hosts.
  `cargo xtask docs` fails on any broken relative link or `#anchor` in a Markdown file, and on a
  missing guide (`README.md`, `docs/USER-GUIDE.md`, `docs/THREAT-MODEL.md`, and so on). A decision that
  changes the architecture gets an [ADR](docs/adr/README.md). A change to what the web UI shows updates
  [docs/USER-GUIDE.md](docs/USER-GUIDE.md); screenshots live in `docs/images/` as PNG files under 300 KB,
  come from a real server on the free demo (or from the mock API, and are labelled so), and may show
  demo data only: no real paths, host names or tokens.

## Reporting bugs and security issues

Bugs: open an issue with the bug template. Security problems: never in public — see
[SECURITY.md](SECURITY.md).
