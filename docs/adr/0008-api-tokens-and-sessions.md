# ADR 0008: API tokens `h2w_<id>_<secret>`, stored as a SHA-256, with cookie sessions for browsers

- **Status:** Accepted. Part of the v1.0 contract ([ARCHITECTURE.md](../ARCHITECTURE.md) §2, §9).
- **Applies to:** `h2w-daemon` (`auth.rs`, `db/tokens.rs`, `db/sessions.rs`, `http/extract.rs`, `http/routes/auth.rs`, `ratelimit.rs`).

## Context

The Workbench needs credentials for people, scripts, CI jobs and AI agents, each with a role, an
optional expiry and the possibility of instant revocation. It runs on a single machine for a small
team, usually without an identity provider; where there is one, a reverse proxy can bring it
([ADMIN-GUIDE.md](../ADMIN-GUIDE.md#63-proxy-sso-mode)). The credential check runs on every request
and must not leak information through timing or error text.

## Decision

- A token is **`h2w_` + an 8-character public id (`[a-z0-9]`) + `_` + a 256-bit random secret** in
  43 characters of unpadded base64url. It is shown once, at creation.
- The database stores the **id and the SHA-256 of the secret**, never the secret. Verification looks
  the row up **by id** (a single indexed read, no scan), hashes the presented secret and compares in
  **constant time**. An unknown id performs the same hashing work, and every failure returns the same
  message (`invalid credentials`); the reason is recorded only in the audit log and a metric.
- Tokens carry a **role** (viewer, operator, admin), an optional **expiry**, a `last_used_at`, and
  can be **revoked** at once; a check against the database happens on every request, so revocation
  needs no cache invalidation.
- **Browsers** exchange a token for a **session**: a 256-bit random id in an `HttpOnly`,
  `SameSite=Strict` cookie, stored only as its hash, with a lifetime that never exceeds the token's.
  When the public URL is https the cookie is `Secure` and named `__Host-h2w_session` (`Path=/`, no
  `Domain`, so a sibling subdomain cannot plant one); over plain http it is `h2w_session`, because a
  `__Host-` cookie needs `Secure`, which browsers drop on http. Revoking a token ends its sessions,
  and an open event stream notices at its next heartbeat. Login is rate-limited per client address.
- **Failed authentication is throttled per client address** (60 a minute, then `429`), and at most 10
  of those failures a minute per client reach the audit log, so presenting junk tokens can neither
  guess nor flood. Valid credentials from the same address are not affected. The limiters key each
  request on its TCP peer *and* its client (behind a trusted proxy, the `X-Forwarded-For` client):
  a peer holds at most 1,024 client keys and its other clients share one bucket of that peer, so a
  process that invents forwarded addresses only limits itself.
- The audit log names the **kind** of every actor: `token:<name>`, `proxy:<user>`, `cli`, `server`
  or `anonymous`, so a proxy user cannot pose as a token (a user called `initial admin` is
  `proxy:initial admin`, not the bootstrap token's `token:initial admin`).
- There are **no passwords** and no user table. A proxy mode accepts a user and groups asserted by a
  trusted reverse proxy instead, behind a shared secret, for deployments with SSO.
- `h2workbench init` mints the first admin token and prints it once.

## Consequences

Good:

- A 256-bit random secret cannot be guessed or brute-forced offline, so a fast hash is sufficient and
  no password-hashing cost or tuning is needed; the id makes the lookup O(1) without comparing against
  every stored hash (which would also leak timing).
- Nothing to reset or recover: lose a token, create another. Rotation is create-then-revoke.
- The same credential works for the CLI, curl, CI, MCP and the browser, and every action is
  attributable to a named token (or, in proxy mode, to a named proxy user).

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
- **Throttling is per client address**: an attacker with many addresses has an allowance on each, and
  behind a proxy that is not listed in `server.trusted_proxies` every user shares one allowance.
  Behind one that is, the clients of a single peer share that peer's bucket once it has used its
  1,024 keys, so whoever can make that proxy forward invented addresses can get its other users
  refused. Guessing 256 bits is infeasible either way; the throttle protects the audit log and the
  server's work, not the secret.

## Alternatives considered

- **Passwords** (rejected in the original decision table): human-chosen secrets are low-entropy, so
  they would need a slow key-derivation function, lockout and reset flows, a user table and
  a password policy, all to protect an admin tool that already has high-entropy credentials available.

## Evidence in the repository

`crates/h2w-daemon/src/auth.rs` (format, `mint`, `secret_matches`, `ct_eq`), `http/extract.rs`
(`verify_token`, `proxy_principal`), `http/routes/auth.rs`, `ratelimit.rs`, `db/tokens.rs`,
`db/sessions.rs`. Tests: `auth.rs` (bearer, expiry and revocation, cookie attributes, session end,
rate limit, proxy mode, and `failed_authentication_cannot_flood_the_audit_log`,
`https_deployments_use_a_host_prefixed_session_cookie`, `non_ascii_tokens_are_refused_with_401`,
`repeated_proxy_identity_headers_are_refused`); `ratelimit.rs::a_full_table_shares_overflow_buckets_instead_of_failing_open`,
`one_peer_cannot_fill_the_table_or_starve_another`; `auth.rs::a_peer_inventing_forwarded_clients_only_limits_itself`,
`audit_actors_name_their_kind`.
Exercised live for this documentation: login, the cookie attributes under both names, the 401
messages, revocation, and the 429s (sign-in, and 75 junk tokens: 60 `401`, then `429`).

## Revisit when

native OIDC or SAML sign-in is wanted without a proxy, scoped or delegated tokens are needed (for
example per-agent limits enforced by the server), or session binding becomes a requirement.
