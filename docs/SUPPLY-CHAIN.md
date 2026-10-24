# Supply-chain policy

Harness² Workbench holds credentials and starts processes, so what goes into its build matters as
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
  old), Dependabot (`cooldown: default-days: 14`; Node majors are left to a person, because the
  cooldown alone offered an odd Node line past its end of life, and uv entirely, see
  [Bumping uv by hand](#bumping-uv-by-hand)), tool versions installed in CI (`wasm-bindgen-cli`,
  `cargo-deny`, `cargo-audit`, `cargo-cyclonedx`, `cargo-about`, `gitleaks`) and container base
  images (pinned by digest).

### Security fixes always win

A release that fixes a RustSec advisory is adopted immediately, even if it is younger than the
cooldown. Each such version is listed in [`supply-chain/cooldown-exceptions.toml`](../supply-chain/cooldown-exceptions.toml)
with the advisory it fixes and an expiry date (when it would pass the cooldown on its own);
`cargo xtask cooldown` honours the list and reports entries that are expired or no longer needed.
(This rule exists because the first lockfile of this project, pinned strictly by age, pulled in
`rustls`, `rustls-webpki`, `webpki-root-certs` and `tokio` versions with known advisories that
newer releases had already fixed.)

### Bumping uv by hand

The runner image copies `uv` from `ghcr.io/astral-sh/uv`. That registry gives Dependabot no
publication date, so the cooldown cannot apply: its first proposal was uv 0.13.5, two days after
the release. Dependabot therefore ignores uv ([`.github/dependabot.yml`](../.github/dependabot.yml)),
and a person moves it:

1. Pick the newest release published at least 14 days ago (GitHub release dates):
   ```console
   $ gh release view 0.12.24 -R astral-sh/uv --json publishedAt --jq .publishedAt
   2026-10-08T20:06:20Z
   ```
2. Look up the digest of its multi-platform image:
   ```console
   $ docker buildx imagetools inspect ghcr.io/astral-sh/uv:0.12.24
   Name:      ghcr.io/astral-sh/uv:0.12.24
   MediaType: application/vnd.oci.image.index.v1+json
   Digest:    sha256:3af4716e991d6956a41e573eab705d0ee08500cd829ed30293eb8472f372c65a
   ```
3. Put both in the `FROM ghcr.io/astral-sh/uv:<version>@sha256:<digest> AS uv` line of
   [`deploy/docker/runner.Dockerfile`](../deploy/docker/runner.Dockerfile), and replace
   `deploy/docker/licenses/uv/` with the `LICENSE-MIT` and `LICENSE-APACHE` of the same tag
   (`gh api "repos/astral-sh/uv/contents/LICENSE-MIT?ref=<version>" --jq .content | base64 -d`).
4. Rebuild the runner and all-in-one images
   ([ADMIN-GUIDE.md](ADMIN-GUIDE.md#33-container-images)) and run one container-isolated job (the
   free demo is enough, [ADMIN-GUIDE.md](ADMIN-GUIDE.md#8-container-isolation-step-by-step))
   before merging: CI builds and smoke-tests the images but runs no harness2 job inside them.

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

The Workbench drives harness2 through its CLI and pins the commit it was tested against
(`TESTED_HARNESS2_COMMIT` in `crates/h2w-core/src/lib.rs`). The managed installer fetches exactly
that 40-hex commit with `git fetch --depth 1 origin <commit>`, so the content is identified by its
hash; a different commit can be installed deliberately by an administrator. harness2's own
benchmark setup pins its dependencies by commit too (`harness2/config.py`).

## 4. Releases you can verify

Each release ships, for every Linux target, a tarball with both binaries and the embedded UI, plus:

- `SHA256SUMS` — verify with `sha256sum -c SHA256SUMS --ignore-missing`;
- a CycloneDX SBOM of every crate in the build;
- a SLSA build-provenance attestation signed through GitHub's OIDC — verify with
  `gh attestation verify harness2-workbench-<version>-<target>.tar.gz --repo willboyden/harness2-workbench`.

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
