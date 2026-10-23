# Supply-chain policy

Harness² Studio holds credentials and starts processes, so what goes into its build matters as
much as its own code. These are the rules and the tools that enforce them.

## 1. Dependency cooldown (14 days)

A release that was published minutes ago has had no time for anyone to notice a compromise. So
every crate version in `Cargo.lock` — direct and transitive — must have been published **at least
14 days** before it is adopted.

- Enforced by `cargo xtask cooldown`, which reads each locked version's `pubtime` from the
  crates.io sparse index and fails on anything younger. It runs in `cargo xtask check` and in CI.
- Picking versions: `cargo update -p <crate>@<version> --precise <older>` pins a younger
  transitive version back; direct dependencies are declared at a version that passed the cooldown.
- The same rule applies to GitHub Actions (pinned by full commit SHA to a release at least 14 days
  old), Dependabot (`cooldown: default-days: 14`), tool versions installed in CI
  (`wasm-bindgen-cli`, `cargo-deny`, `cargo-audit`, `cargo-cyclonedx`, `gitleaks`) and container
  base images (pinned by digest).

### Security fixes always win

A release that fixes a RustSec advisory is adopted immediately, even if it is younger than the
cooldown. Each such version is listed in [`supply-chain/cooldown-exceptions.toml`](../supply-chain/cooldown-exceptions.toml)
with the advisory it fixes and an expiry date (when it would pass the cooldown on its own);
`cargo xtask cooldown` honours the list and reports entries that are expired or no longer needed.
(This rule exists because the first lockfile of this project, pinned strictly by age, pulled in
`rustls`, `rustls-webpki`, `webpki-root-certs` and `tokio` versions with known advisories that
newer releases had already fixed.)

## 2. cargo-deny

[`deny.toml`](../deny.toml), checked by `cargo deny check` locally and in CI:

- **advisories**: every RustSec vulnerability, unsound, unmaintained or yanked advisory fails the
  build unless it is ignored with a reason and a review date (currently one: `paste`, an archived,
  compile-time-only macro crate pulled in by Leptos);
- **licenses**: an allow-list of permissive licences compatible with Apache-2.0 distribution;
- **bans**: no OpenSSL bindings (rustls only), no wildcard version requirements;
- **sources**: crates.io only; no git dependencies.

`cargo audit` additionally runs weekly in the Security workflow.

## 3. Upstream harness2

The Studio drives harness2 through its CLI and pins the commit it was tested against
(`TESTED_HARNESS2_COMMIT` in `crates/h2s-core/src/lib.rs`). The managed installer fetches exactly
that 40-hex commit with `git fetch --depth 1 origin <commit>`, so the content is identified by its
hash; a different commit can be installed deliberately by an administrator. harness2's own
benchmark setup pins its dependencies by commit too (`harness2/config.py`).

## 4. Releases you can verify

Each release ships, for every Linux target, a tarball with both binaries and the embedded UI, plus:

- `SHA256SUMS` — verify with `sha256sum -c SHA256SUMS --ignore-missing`;
- a CycloneDX SBOM of every crate in the build;
- a SLSA build-provenance attestation signed through GitHub's OIDC — verify with
  `gh attestation verify harness2-studio-<version>-<target>.tar.gz --repo willboyden/harness2-studio`.

Releases are created as drafts by CI from a version tag and published by a maintainer after the
artifacts and attestations are checked ([RELEASING.md](RELEASING.md)).

## 5. Repository hygiene

- Secrets scanning: the `pre-commit` hook scans every staged line for personal paths, private
  keys and common token formats; CI runs gitleaks over the full history.
- Workflows: least-privilege `permissions`, `persist-credentials: false`, no
  `pull_request_target`, no template expansion inside `run:` scripts; audited with
  [zizmor](https://docs.zizmor.sh) (`uvx zizmor==1.30.1 --persona auditor .github/workflows`) and
  CodeQL's `actions` analysis.
- OpenSSF Scorecard runs weekly.
