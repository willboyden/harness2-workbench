# ADR 0005: A strict Content-Security-Policy; WebAssembly loaded by an external script; nothing inline

- **Status:** Accepted. Part of the v1.0 contract ([ARCHITECTURE.md](../ARCHITECTURE.md) §2, §9, §12).
- **Applies to:** `h2s-daemon` (`http/guard.rs`, `http/static_ui.rs`), `h2s-web`, `xtask web`.

## Context

The web UI is an administration console: a signed-in page can start processes, change model
endpoints and read secrets' metadata. It also renders text that nobody at the Studio wrote: model
transcripts, harness files proposed by an improver, benchmark documents, job output. Any script
injected into that page has the user's session and the power to start jobs. Defence in depth against
cross-site scripting is therefore a requirement, not a polish item.

## Decision

Every response, errors and static files included, carries:

```text
Content-Security-Policy: default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self';
  img-src 'self' data:; font-src 'self'; connect-src 'self'; manifest-src 'self'; base-uri 'none';
  form-action 'self'; frame-ancestors 'none'
```

plus `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`, `X-Frame-Options: DENY`,
`Permissions-Policy`, and same-origin cross-origin policies. To make that policy workable:

- The UI is **client-side rendered Leptos compiled to WebAssembly**, started by a small external
  module script (`/app.js`) that imports the `wasm-bindgen` glue and initialises the module from an
  external `.wasm` file. There is **no inline `<script>`** in `web/index.html` and no `style="..."`
  attribute in the UI's source; styling is the static `styles.css` and SVG attributes.
- Static assets are **embedded in the server binary** from `dist/web/` (built by `cargo xtask web`) and
  served from an **allowlist** (`/`, `/app.js`, `/styles.css`, `/favicon.svg`, `/pkg/*`); any other
  non-API path returns `index.html` for client-side routing. Content-hashed names are cached as
  immutable; everything else is revalidated.
- UI development without a server uses `tools/h2s-mock-api`, which serves fixture data **with the
  production policy**, so a violation shows up while developing.

## Consequences

Good:

- A bug that lets attacker-chosen text reach the DOM cannot run it as script: no inline script, no
  `eval`, no third-party origins, no framing, no form posts elsewhere.
- Nothing on disk is reachable by URL except the embedded assets.
- One binary serves the API and the UI; there is no separate web server to configure or secure.

Bad, and how it is managed:

- **`'wasm-unsafe-eval'` is required** to compile WebAssembly. It permits compiling Wasm, not
  evaluating JavaScript strings, but it is a hole the policy would not otherwise have.
- **Tooling constraints.** Trunk's default inline bootstrap needs `'unsafe-inline'`, so the build is
  hand-assembled (`cargo xtask web`: `cargo build` for `wasm32-unknown-unknown`, then `wasm-bindgen
  --target web`, then the static assets). The `wasm-bindgen` CLI version must match the crate.
- **No CDNs, web fonts, analytics or embedded widgets**; styling is static CSS and charts are SVG.
- **The CSP does not make the UI's own escaping optional.** Leptos escapes by default; any future use
  of raw HTML insertion needs review.
- Embedded assets mean a UI fix requires a new server binary.

## Alternatives considered

- **Trunk's inline bootstrap** (rejected in the original decision table): it requires
  `'unsafe-inline'` in `script-src`, which removes the main benefit of the policy for an interface that
  can start processes.

## Evidence in the repository

`crates/h2s-daemon/src/http/guard.rs` (`SECURITY_HEADERS`), `http/static_ui.rs` (allowlist, caching),
`xtask/src/main.rs` (`build_web`), `crates/h2s-daemon/tests/security.rs` and `static_ui.rs`,
`web/index.html` and `web/app.js` (the only script is the external module), `crates/h2s-web`,
`tools/h2s-mock-api`. The server's side of this decision (headers, allowlist, the `UI not built`
page) was exercised live by the documentation author. The UI source was read for inline scripts,
`style` attributes and raw-HTML insertion (none found); the UI's behaviour in a browser was **not**
exercised by the documentation author. The maintainers' notes (`docs/notes/web.md`) record scripted
headless-Firefox runs against the mock API and a real server.

## Revisit when

browsers allow Wasm compilation without `'wasm-unsafe-eval'` under a hash- or nonce-based mechanism,
or the UI needs a feature (inline editors, embedded viewers) that cannot live under this policy.
