# Releasing

Harness² Studio follows [Semantic Versioning](https://semver.org). Before 1.0, a minor version
(0.**x**.0) may contain breaking changes; patch versions never do. The REST API and MCP tools are
versioned with the product.

## Repository visibility

Code scanning (the CodeQL and OpenSSF Scorecard jobs of `.github/workflows/security.yml`) and the
build-provenance attestations of the release workflow need a public repository on GitHub's free
plan (private repositories need GitHub Advanced Security or Enterprise Cloud). The two scanning jobs
therefore run only when the repository variable `CODE_SCANNING_ENABLED` is `true`:
`gh variable set CODE_SCANNING_ENABLED --body true` once the repository is public. Tag the first
release only after that.

## Checklist

1. **Changelog.** Move the *Unreleased* entries of [CHANGELOG.md](../CHANGELOG.md) under a new
   version heading with today's date.
2. **Version.** Bump `version` in `[workspace.package]` of the root `Cargo.toml`, then
   `cargo check --workspace` to refresh `Cargo.lock`.
3. **Gates.** `cargo xtask check` and the end-to-end suite (`crates/h2s-e2e/README.md`) pass
   locally; CI is green on `main`.
4. **Clean clone.** In a temporary directory outside the repository:
   `git clone <repo> && cd harness2-studio && cargo xtask check && cargo xtask dist`. A working
   tree is not a clean clone: this is what catches files that exist only on your machine.
5. **Leak scan.** `git diff <previous-tag>..HEAD` contains no personal paths, tokens or
   machine-specific values (the hook and gitleaks check this too).
6. **Tag.** `git tag -s vX.Y.Z -m "Harness² Studio vX.Y.Z"` and push the tag only:
   `git push origin vX.Y.Z`.
7. **CI builds a draft release** with tarballs for x86_64 and aarch64, `SHA256SUMS` and
   build-provenance attestations. Download one tarball, verify it
   (`sha256sum -c`, `gh attestation verify`), run `h2studio version` and the demo, then publish
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
