# End-to-end tests

These suites run the **real** binaries against a **real** harness2 checkout — no mocks:

| Suite | What it proves |
|---|---|
| `tests/server_demo.rs` | `h2studio` runs `harness2-demo` as a job, streams it over SSE, and every results endpoint (sweeps, tasks, harness trees, diffs, export, library, manifest) reads what harness2 wrote; RBAC, CSRF, Origin, Host and header rules hold on the live server; secrets never reach responses, the audit log or server logs. |
| `tests/browser.rs` | Headless Firefox logs in with a token, browses the demo down to a harness diff and a job log, and the dashboard fits a phone screen. Screenshots land in `target/e2e-artifacts/`. |
| `tests/mcp.rs` | An MCP client drives `h2studio-mcp` over stdio against the live server. |

The tests are `#[ignore]`d in the normal `cargo test` run because they need external tools. A
missing tool **fails** the suite with an explanation; nothing is skipped silently.

## Prerequisites

- A harness2 checkout with its virtualenv, e.g.:
  ```bash
  git clone https://github.com/google-research/harness2 ~/harness2 && cd ~/harness2
  git checkout 1cfe254e8d25b15f37048c3730134686abe60335   # the tested commit
  uv venv --python 3.12 .venv
  # The same dependency versions the Studio installs (h2s_core::install::TESTED_HARNESS2_DEPS_CUTOFF):
  uv pip install --python .venv/bin/python --exclude-newer 2026-10-14T02:22:49Z -e '.[render]'
  ```
- Firefox and geckodriver on `PATH` (or `GECKODRIVER=/path/to/geckodriver`; optional
  `FIREFOX_BIN`). The browser runs headless with a throwaway profile.
- `wasm-bindgen-cli` 0.2.129 (for the UI build).

## Run

```bash
cargo xtask e2e --harness2-home ~/harness2                     # everything
cargo xtask e2e --harness2-home ~/harness2 --suite browser     # one file: server_demo, browser, mcp
cargo xtask e2e --harness2-home ~/harness2 --filter security   # tests whose NAME contains "security"
```

A run in which no test matched fails instead of reporting success.

Each test starts its own server on a free loopback port with a throwaway data directory, and
stops it afterwards. No model credentials and no network are needed: the harness2 demo uses
deterministic local editors.
