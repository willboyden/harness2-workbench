# Releasing

Harness² Workbench follows [Semantic Versioning](https://semver.org). Before 1.0, a minor version
(0.**x**.0) may contain breaking changes; patch versions never do. The REST API and MCP tools are
versioned with the product.

## Repository visibility

Code scanning (the CodeQL and OpenSSF Scorecard jobs of `.github/workflows/security.yml`) and the
build-provenance attestations of the release workflow need a public repository on GitHub's free
plan (private repositories need GitHub Advanced Security or Enterprise Cloud). The two scanning jobs
therefore run only when the repository variable `CODE_SCANNING_ENABLED` is `true`
(`gh variable set CODE_SCANNING_ENABLED --body true`, done when the repository became public on
2026-10-24). Tag the first release only with that set.

Also set on 2026-10-24, as the governance files promise: private vulnerability reporting
(SECURITY.md), Discussions (SUPPORT.md), the `triage` label the issue forms use, Dependabot alerts
and security updates, secret scanning with push protection, deletion of merged branches, and a
ruleset for `main` (CONTRIBUTING.md): pull requests need green CI (the eight jobs of `ci.yml`, plus
`cargo audit` and `gitleaks` from `security.yml`, as reported by GitHub Actions) and an approving review
from a code owner, approvals are dismissed by new commits, force-pushes and deletion are blocked,
and the repository admin role may bypass it. If you fork this project, set the same.

## Build artifacts

Publish only what CI builds. A binary or WebAssembly bundle built on your machine embeds your
paths (`/home/<you>/...`, from panic locations and debug info); if you ever must, build with
`RUSTFLAGS="--remap-path-prefix=$HOME=/build --remap-path-prefix=$PWD=/src"`.

Release archives and the all-in-one image carry `THIRD-PARTY-LICENSES.md` next to `LICENSE` and
`NOTICE`: the licence texts of every crate compiled into the servers and the web UI (dev and build
dependencies excluded), written by `cargo xtask licenses` with cargo-about 0.9.2 (pinned and
cooldown-checked; `supply-chain/about.toml` holds the same allow-list as `deny.toml`). The runner
image carries uv's licence texts; its Debian packages keep theirs in `/usr/share/doc/*/copyright`.

## Checklist

1. **Changelog.** Move the *Unreleased* entries of [CHANGELOG.md](../CHANGELOG.md) under a new
   version heading with today's date.
2. **Version.** Bump `version` in `[workspace.package]` of the root `Cargo.toml`, then
   `cargo check --workspace` to refresh `Cargo.lock`.
3. **Gates.** `cargo xtask check` and the end-to-end suite (`crates/h2w-e2e/README.md`) pass
   locally, `cargo xtask licenses` writes the licence file; CI is green on `main`.
4. **Clean clone.** In a temporary directory outside the repository:
   `git clone <repo> && cd harness2-workbench && cargo xtask check && cargo xtask dist`. A working
   tree is not a clean clone: this is what catches files that exist only on your machine.
5. **Leak scan.** `git diff <previous-tag>..HEAD` contains no personal paths, tokens or
   machine-specific values (the hook and gitleaks check this too).
6. **Tag.** `git tag -s vX.Y.Z -m "Harness² Workbench vX.Y.Z"` and push the tag only:
   `git push origin vX.Y.Z`.
7. **CI builds a draft release** with tarballs for x86_64 and aarch64, `SHA256SUMS` and
   build-provenance attestations. Download one tarball, verify it
   (`sha256sum -c`, `gh attestation verify`), run `h2workbench version` and the demo, then publish
   the draft.

## Never move a published tag

If a release is broken, leave its tag and publish a fixed `vX.Y.Z+1`. Users and downstream
packagers rely on tags being immutable.

## Toolchain and pin bumps

- **Rust toolchain:** update `rust-toolchain.toml` and the Dockerfile's builder digest together,
  then run the full gate. Bump `rust-version` in `Cargo.toml` only to versions you test.
- **wasm-bindgen:** the CLI (CI, Dockerfile, CONTRIBUTING) and the crate must be the same version.
- **harness2:** update `TESTED_HARNESS2_COMMIT`, re-run the end-to-end suite against the new
  commit, re-check the CLI flags and output formats the parsers rely on, and note the bump in the
  changelog.
