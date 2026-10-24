# ADR 0008: API tokens `h2s_<id>_<secret>`, stored as a SHA-256, with cookie sessions for browsers

- **Status:** Accepted. Part of the v1.0 contract ([ARCHITECTURE.md](../ARCHITECTURE.md) §2, §9).
- **Applies to:** `h2s-daemon` (`auth.rs`, `db/tokens.rs`, `db/sessions.rs`, `http/extract.rs`, `http/routes/auth.rs`).

## Context

The Studio needs credentials for people, scripts, CI jobs and AI agents, each with a role, an
optional expiry and the possibility of instant revocation. It runs on a single machine for a small
team, usually without an identity provider; where there is one, a reverse proxy can bring it
([ADMIN-GUIDE.md](../ADMIN-GUIDE.md#63-proxy-sso-mode)). The credential check runs on every request
and must not leak information through timing or error text.

## Decision

- A token is **`h2s_` + an 8-character public id (`[a-z0-9]`) + `_` + a 256-bit random secret** in
  43 characters of unpadded base64url. It is shown once, at creation.
- The database stores the **id and the SHA-256 of the secret**, never the secret. Verification looks
  the row up **by id** (a single indexed read, no scan), hashes the presented secret and compares in
  **constant time**. An unknown id performs the same hashing work, and every failure returns the same
  message (`invalid credentials`); the reason is recorded only in the audit log and a metric.
- Tokens carry a **role** (viewer, operator, admin), an optional **expiry**, a `last_used_at`, and
  can be **revoked** at once; a check against the database happens on every request, so revocation
  needs no cache invalidation.
- **Browsers** exchange a token for a **session**: a 256-bit random id in an `HttpOnly`,
  `SameSite=Strict` cookie (`Secure` when the public URL is https), stored only as its hash, with a
  lifetime that never exceeds the token's. Revoking a token ends its sessions. Login is rate-limited
  per client address.
- There are **no passwords** and no user table. A proxy mode accepts a user and groups asserted by a
  trusted reverse proxy instead, behind a shared secret, for deployments with SSO.
- `h2studio init` mints the first admin token and prints it once.

## Consequences

Good:

- A 256-bit random secret cannot be guessed or brute-forced offline, so a fast hash is sufficient and
  no password-hashing cost or tuning is needed; the id makes the lookup O(1) without comparing against
  every stored hash (which would also leak timing).
- Nothing to reset or recover: lose a token, create another. Rotation is create-then-revoke.
- The same credential works for the CLI, curl, CI, MCP and the browser, and every action is
  attributable to a named token.

Bad, and how it is managed:

- **A token is a bearer credential**: whoever has the string has the access. Store tokens like
  passwords; give each person and each automation its own, with the lowest role and an expiry.
- **The bootstrap token is saved to `<config dir>/admin.token`** (mode 0600) for convenience; it, and
  any token created without `--expires-in-days`, never expires. The guide says to give every token an
  expiry and to remove the file or revoke the token after setup.
- **The id appears in logs and audit records** (it is public by design, not a secret).
- **Signing in means pasting a token**, which is less friendly than a password form and offers no
  single sign-on of its own; proxy mode is the answer for teams.
- **Three roles and nothing finer**: no scopes, no per-experiment permissions.
- Sessions are not bound to an address or user agent.

## Alternatives considered

- **Passwords** (rejected in the original decision table): human-chosen secrets are low-entropy, so
  they would need a slow key-derivation function, lockout and reset flows, a user table and
  a password policy, all to protect an admin tool that already has high-entropy credentials available.

## Evidence in the repository

`crates/h2s-daemon/src/auth.rs` (format, `mint`, `secret_matches`, `ct_eq`), `http/extract.rs`
(`verify_token`, `proxy_principal`), `http/routes/auth.rs`, `ratelimit.rs`, `db/tokens.rs`,
`db/sessions.rs`. Tests: `auth.rs` (bearer, expiry and revocation, cookie attributes, session end,
rate limit, proxy mode). Exercised live for this documentation: login, cookie attributes, the 401
messages, revocation, the 429.

## Revisit when

native OIDC or SAML sign-in is wanted without a proxy, scoped or delegated tokens are needed (for
example per-agent limits enforced by the server), or session binding becomes a requirement.
