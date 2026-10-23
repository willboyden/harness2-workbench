# Web UI notes (`h2s-web`, `tools/h2s-mock-api`)

## Layout

| Path | What |
|---|---|
| `crates/h2s-web/src/logic/` | Pure logic, unit-tested natively (`cargo test -p h2s-web`): formatting and RFC 3339 times, mirrors of the server's name rules, URL builders, diff-line classification, trajectory segmentation, the job view-model and live-event application, the ordered log buffer, the run wizard model and presets, the profile form, report and sign-test readings, task timelines, the glossary, the palette matcher, theme handling. |
| `crates/h2s-web/src/api.rs` | Same-origin `fetch`; `x-h2s-request: 1` on every unsafe method; non-2xx becomes `ApiError` from `ErrorBody`; a 401 sends the user to `/login?next=…`. |
| `crates/h2s-web/src/browser.rs` | Storage, clipboard, downloads, focus and a `setTimeout` future. Every throwing browser call goes through its `Result` binding. |
| `crates/h2s-web/src/state.rs` | Root contexts: session, toasts, dialogs/palette/theme, a coarse clock. |
| `crates/h2s-web/src/components/` | Shell, primitives, forms, portal-rendered modals, toasts, palette and shortcuts, SVG charts, domain widgets. |
| `crates/h2s-web/src/pages/` | One module per route of ARCHITECTURE.md §12. |
| `web/` | `index.html`, `app.js`, `styles.css` (design tokens, light and dark), `favicon.svg`. |

`#[allow(unreachable_pub, clippy::needless_pass_by_value)]` is set on the view modules only
(`app`, `components`, `pages`): Leptos' `#[component]` expands to `pub` items, and component
props are owned by design. Both lints stay on everywhere else.

## Build and run

```sh
cargo xtask web --dev          # or without --dev for the release-web profile
cargo run -p h2s-mock-api -- --port 8790 --dist dist/web
```

Sign in with any token that starts with `h2s_dev_` (`h2s_dev_viewer…` and
`h2s_dev_operator…` give those roles, anything else is an admin). Mock flags:

| Flag | Effect |
|---|---|
| `--fresh` | A brand-new server: harness2 not installed, no experiments or profiles. |
| `--container` | Container isolation (no risk acknowledgement needed for runs). |
| `--csp-report` | Appends `report-uri /__csp-report` to the CSP and prints every violation report. Enforcement is unchanged. |
| `--sse-off` | Answers `/jobs/{id}/events` with 503, to exercise the UI's reconnect and polling fallback. |
| `--task-ms`, `--live-task-ms` | Pace of simulated runs (the populated mock starts with one live run and one queued pipeline). |

The mock sends the exact CSP and security headers of §9, enforces the CSRF header, the
`Origin` check, loopback `Host` names and roles, streams job events over real SSE
(snapshot first, `id:` on log events, heartbeats and keep-alive comments), and serves
`dist/web` with the SPA fallback. All fixtures are fictional and built from `h2s_api` types.

## Contract assumptions the UI relies on

- Job events: unnamed SSE frames, one `JobEvent` per `data:` (addendum v1.1). The UI merges
  log lines by `n`, backfills gaps from `GET /log`, closes the stream on a terminal status,
  reconnects with backoff, refreshes once per failure, and polls after four failures.
- `Comparison` values are fractions 0..1 (addendum v1.1); the UI shows percent and
  percentage points.
- `{mode}` path segments use `parallel` / `sequential`; `/jobs?status=` takes one value.
- Downloads (manifest, `.tar.gz` export, judged rows) are plain same-origin links with
  `download`; report text and export JSON are offered as Blob downloads.

## Test hooks (addendum v1.1)

`login-token`, `login-submit`, `login-error`; `nav-dashboard`, `nav-experiments`, `nav-jobs`,
`nav-new-run`, `nav-compare`, `nav-benchmarks`, `nav-settings`, `nav-help`, `current-user`,
`theme-toggle`; `dashboard`, `harness2-status`; `experiment-row[data-name]`,
`experiment-create`; `sweep-row[data-sweep]`; `tab-tasks`, `tab-report`, `tab-library`,
`tab-audit`, `task-row[data-task]`, `report-view`, `library-view`;
`mode-section[data-mode]`, `harness-label[data-label]`, `diff-from`, `diff-to`, `diff-view`,
`export-button`; `job-row[data-job-id]`, `job-status` (text is exactly the status string),
`job-log`, `job-cancel`; `toast`. Extra ids used by the UI's own checks include
`confirm-dialog`, `confirm-accept`, `command-palette`, `create-experiment-dialog`,
`experiment-create-submit`, `token-created`, `run-start`.

## Known gaps

- Task tables at phone width scroll horizontally inside their container rather than
  reflowing into cards.
- Glossary tooltips open on hover and focus; on touch screens the term links to Help.
- There are no browser (`wasm-bindgen-test`) tests; behaviour that needs a browser was
  checked with scripted headless Firefox runs against the mock.
