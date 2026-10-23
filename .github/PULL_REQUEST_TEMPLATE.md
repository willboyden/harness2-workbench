## What and why

<!-- One paragraph: the problem and the change. Link the issue it closes. -->

## How it was verified

<!-- Commands you ran and what you saw. Say plainly what you could NOT verify. -->

- [ ] `cargo xtask check` passes (fmt, clippy native + wasm, tests, cargo-deny, cooldown)
- [ ] New behaviour has tests (unit, property or integration as fits)
- [ ] UI changes: screenshots in light and dark, desktop and phone width

## Risk review

- [ ] No secrets, personal paths or machine-specific values in the diff
- [ ] Touches auth, RBAC, job execution, filesystem access, the egress proxy or the CSP? Explain the threat-model impact (docs/THREAT-MODEL.md)
- [ ] New or bumped dependency? It passes `cargo xtask cooldown` and `cargo deny check`; say why it is needed
- [ ] API or MCP contract change? Additive only, documented in docs/ARCHITECTURE.md §16 and CHANGELOG.md

The PR title follows Conventional Commits (`feat(daemon): ...`, `fix(web): ...`).
