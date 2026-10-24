# ADR 0007: A 14-day dependency cooldown, with security fixes exempt

- **Status:** Accepted. Part of the v1.0 contract ([ARCHITECTURE.md](../ARCHITECTURE.md) §2, §15); policy text in [SUPPLY-CHAIN.md](../SUPPLY-CHAIN.md).
- **Applies to:** `Cargo.lock`, `xtask cooldown`, CI, container base images, Dependabot.

## Context

The Studio holds model-provider credentials and starts processes, so a malicious dependency release
is a direct route to those credentials. A release that was published minutes ago has had no time for
anyone to notice a compromise ([SUPPLY-CHAIN.md](../SUPPLY-CHAIN.md)); a project that adopts new
releases the moment they appear takes all of that exposure, and one that waits does not. A pure
wait-and-see rule, however, also delays fixes for real vulnerabilities, and the first lockfile of this
project showed it.

## Decision

- **Every locked registry package, direct or transitive, must have been published at least 14 days
  before it is adopted.** `cargo xtask cooldown` reads each locked version's `pubtime` from the
  crates.io sparse index and fails on anything younger; it runs in `cargo xtask check`.
- **Security fixes win.** A release that fixes a RustSec advisory is adopted at once and listed in
  `supply-chain/cooldown-exceptions.toml` with the advisory and an **expiry date** (when it would pass
  on its own); the check reports exceptions that are expired or no longer needed.
- The same rule applies to the other inputs: GitHub Actions pinned by full commit SHA to a release at
  least 14 days old, Dependabot configured with `cooldown: default-days: 14` for crates, actions and
  Docker, tools installed in CI pinned to versions that pass, and container base images pinned by
  digest.
- `cargo-deny` complements it: RustSec advisories fail the build unless ignored with a reason and a
  review date, licences are allow-listed, OpenSSL bindings and git dependencies are banned.

## Consequences

Good:

- A freshly published malicious release cannot enter through an update the day it appears, and the
  review of updates is spread over a fortnight in which the ecosystem may notice.
- The rule is mechanical and checked on every change, so it does not depend on someone remembering.

Bad, and how it is managed:

- **It delays good fixes**, which is why the exception list exists. The first lockfile, pinned strictly
  by age, pulled in `rustls`, `rustls-webpki`, `webpki-root-certs` and `tokio` versions with known
  advisories that newer releases had fixed; those four are the current exceptions, each with an expiry.
- **It does not protect against a compromise that has already aged**, nor against a malicious version
  nobody has noticed in 14 days. It narrows one window; it is not a review.
- **Its scope is the Studio's own build.** It covers the Rust crates, CI actions, container base
  images and Dependabot updates. It does **not** cover harness2's Python dependencies (resolved at
  install time by `uv pip install -e`, with no lock file and no `--exclude-newer`) or the npm and Bun
  packages that benchmark setup installs.
- **It needs the network** at check time (one request per locked crate to `index.crates.io`, through
  `curl`), and trusts the index's `pubtime`.
- **Maintenance cost**: pinning back transitive versions with `cargo update -p ... --precise ...`,
  tending the exceptions, and a policy contributors must understand
  ([CONTRIBUTING.md](../../CONTRIBUTING.md): new dependencies "must be at least 14 days old").

## Alternatives considered

- **No policy** (rejected in the original decision table): adopt whatever resolves, rely on
  advisories after the fact. That leaves the freshly-published window fully open.
- **Advisories alone** (`cargo-deny` / RustSec): kept, but it only knows about problems someone has
  already reported, so it cannot cover the window the cooldown is for.

## Evidence in the repository

`xtask/src/cooldown.rs`, `supply-chain/cooldown-exceptions.toml`, `deny.toml`,
`.github/dependabot.yml`, `.github/workflows/ci.yml` (the `supply-chain` job) and `security.yml`.

## Revisit when

the registry exposes a signed or attested publication time, a better ecosystem control (trusted
publishing with provenance verification) can replace the delay, or the exceptions list becomes a
routine rather than an exception.
