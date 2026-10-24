//! Authentication: bearer tokens, session cookies, expiry, revocation, login rate limit,
//! proxy mode.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::*;
use h2w_api::{AuditEvent, ErrorBody, ErrorCode, Page, Principal, PrincipalKind, Role, TokenInfo};
use serde_json::json;

async fn me(t: &TestServer, token: &str) -> reqwest::Response {
    t.get("/api/v1/auth/me", Some(token)).send().await.unwrap()
}

fn id_of(token: &str) -> &str {
    &token[4..12]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bearer_tokens_authenticate_and_bad_ones_are_refused() {
    let t = TestServer::start().await;
    let r = me(&t, &t.admin).await;
    assert_eq!(r.status(), 200);
    let p: Principal = r.json().await.unwrap();
    assert_eq!(
        (p.role, p.kind, p.name.as_str()),
        (Role::Admin, PrincipalKind::Token, "admin-t")
    );
    assert_eq!(p.id, id_of(&t.admin));

    let mut wrong = t.admin.clone();
    let last = wrong.pop().unwrap();
    wrong.push(if last == 'A' { 'B' } else { 'A' });
    let unknown = format!("h2w_zzzzzzzz_{}", &t.admin[13..]);
    for bad in [wrong.as_str(), unknown.as_str(), "nope", "h2w_", ""] {
        let r = me(&t, bad).await;
        assert_eq!(r.status(), 401, "{bad}");
        let e: ErrorBody = r.json().await.unwrap();
        assert_eq!(e.error.code, ErrorCode::Unauthorized);
    }
    let r = t
        .client
        .get(t.url("/api/v1/auth/me"))
        .header("authorization", "Basic YWJj")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let r = t.client.get(t.url("/api/v1/auth/me")).send().await.unwrap();
    assert_eq!(r.status(), 401);

    // Failures with credentials are audited (with the token id), and counted.
    let audit: Page<AuditEvent> = t
        .get("/api/v1/audit?limit=100", Some(&t.admin))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let failed: Vec<&AuditEvent> = audit
        .items
        .iter()
        .filter(|e| e.action == "auth.failed")
        .collect();
    assert!(
        failed.iter().any(|e| e.target == id_of(&t.admin)
            && e.detail.as_deref().unwrap().starts_with("bad_secret"))
    );
    assert!(failed.iter().any(|e| e.target == "zzzzzzzz"));
    let m = t
        .get("/metrics", Some(&t.viewer))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        m.contains("h2w_auth_failures_total{reason=\"bad_secret\"}"),
        "{m}"
    );
    assert!(
        m.contains("h2w_auth_failures_total{reason=\"missing\"}"),
        "{m}"
    );
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expired_and_revoked_tokens_stop_working() {
    let t = TestServer::start().await;
    let expired = t
        .token("old", Role::Viewer, Some(h2w_daemon::util::now_unix() - 10))
        .await;
    let r = me(&t, &expired).await;
    assert_eq!(r.status(), 401);
    assert!(
        r.json::<ErrorBody>()
            .await
            .unwrap()
            .error
            .message
            .contains("expired")
    );

    let victim = t.token("victim", Role::Operator, None).await;
    assert_eq!(me(&t, &victim).await.status(), 200);
    let r = t
        .send(
            reqwest::Method::DELETE,
            &format!("/api/v1/tokens/{}", id_of(&victim)),
            Some(&t.admin),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    let r = me(&t, &victim).await;
    assert_eq!(r.status(), 401);
    assert!(
        r.json::<ErrorBody>()
            .await
            .unwrap()
            .error
            .message
            .contains("revoked")
    );
    let list: Vec<TokenInfo> = t
        .get("/api/v1/tokens", Some(&t.admin))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(list.iter().any(|i| i.id == id_of(&victim) && i.revoked));
    assert!(
        list.iter()
            .any(|i| i.id == id_of(&t.admin) && i.last_used_at.is_some())
    );
    // Revoking twice, unknown and malformed ids.
    let r = t
        .send(
            reqwest::Method::DELETE,
            &format!("/api/v1/tokens/{}", id_of(&victim)),
            Some(&t.admin),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 409);
    for id in ["zzzzzzzz", "../etc", "ABCDEFGH"] {
        let r = t
            .send(
                reqwest::Method::DELETE,
                &format!("/api/v1/tokens/{id}"),
                Some(&t.admin),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 404, "{id}");
    }
    // The last active admin token cannot be revoked through the API.
    let r = t
        .send(
            reqwest::Method::DELETE,
            &format!("/api/v1/tokens/{}", id_of(&t.admin)),
            Some(&t.admin),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 409);
    let other = t.token("second-admin", Role::Admin, None).await;
    let r = t
        .send(
            reqwest::Method::DELETE,
            &format!("/api/v1/tokens/{}", id_of(&t.admin)),
            Some(&other),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    t.stop().await;
}

fn cookie_of(r: &reqwest::Response) -> String {
    r.headers()["set-cookie"].to_str().unwrap().to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_sets_a_strict_session_cookie_and_logout_ends_it() {
    let t = TestServer::start().await;
    let r = t
        .post("/api/v1/auth/login", None)
        .json(&json!({"token": t.viewer}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let set = cookie_of(&r);
    for attr in [
        "h2w_session=",
        "HttpOnly",
        "SameSite=Strict",
        "Path=/",
        "Max-Age=",
    ] {
        assert!(set.contains(attr), "{attr} missing in {set}");
    }
    assert!(
        !set.contains("Secure"),
        "plain-http public_url must not set Secure: {set}"
    );
    let p: Principal = r.json().await.unwrap();
    assert_eq!((p.kind, p.role), (PrincipalKind::Session, Role::Viewer));
    let cookie = set.split(';').next().unwrap().to_owned();

    let r = t
        .client
        .get(t.url("/api/v1/auth/me"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(
        r.json::<Principal>().await.unwrap().kind,
        PrincipalKind::Session
    );

    let r = t
        .client
        .post(t.url("/api/v1/auth/logout"))
        .header("cookie", &cookie)
        .header("x-h2w-request", "1")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    assert!(cookie_of(&r).contains("Max-Age=0"));
    let r = t
        .client
        .get(t.url("/api/v1/auth/me"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);

    // Garbage and forged cookies.
    for c in [
        "h2w_session=abc",
        "h2w_session=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    ] {
        let r = t
            .client
            .get(t.url("/api/v1/auth/me"))
            .header("cookie", c)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 401, "{c}");
    }
    // A bad token cannot log in.
    let r = t
        .post("/api/v1/auth/login", None)
        .json(&json!({"token": "h2w_nope"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revoking_a_token_ends_its_sessions_and_expired_sessions_fail() {
    let t = TestServer::start().await;
    let tok = t.token("browser", Role::Operator, None).await;
    let r = t
        .post("/api/v1/auth/login", None)
        .json(&json!({"token": tok}))
        .send()
        .await
        .unwrap();
    let cookie = cookie_of(&r).split(';').next().unwrap().to_owned();
    assert_eq!(
        t.client
            .get(t.url("/api/v1/auth/me"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let r = t
        .send(
            reqwest::Method::DELETE,
            &format!("/api/v1/tokens/{}", id_of(&tok)),
            Some(&t.admin),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    assert_eq!(
        t.client
            .get(t.url("/api/v1/auth/me"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );

    // A session past its expiry is refused even though its token is fine.
    let (sid, hash) = h2w_daemon::auth::new_session().unwrap();
    let tid = id_of(&t.viewer).to_owned();
    let now = h2w_daemon::util::now_unix();
    t.srv()
        .state
        .db
        .call(move |c| h2w_daemon::db::sessions::insert(c, &hash, &tid, now - 100, now - 1))
        .await
        .unwrap();
    let r = t
        .client
        .get(t.url("/api/v1/auth/me"))
        .header("cookie", format!("h2w_session={}", sid.as_str()))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_attempts_are_rate_limited_per_client() {
    let t = TestServer::start_with(|s| s.file.server.login_attempts_per_minute = 3).await;
    for _ in 0..3 {
        let r = t
            .post("/api/v1/auth/login", None)
            .json(&json!({"token": "h2w_bad"}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 401);
    }
    let r = t
        .post("/api/v1/auth/login", None)
        .json(&json!({"token": t.admin}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 429);
    let retry: u64 = r.headers()["retry-after"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=60).contains(&retry));
    assert_eq!(
        r.json::<ErrorBody>().await.unwrap().error.code,
        ErrorCode::RateLimited
    );
    // Bearer authentication is not affected.
    assert_eq!(me(&t, &t.admin).await.status(), 200);
    t.stop().await;
}

/// Behind HTTPS the session cookie carries the `__Host-` prefix (Secure, Path=/, no Domain:
/// a sibling subdomain cannot plant one), and a plain `h2w_session` cookie is ignored.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn https_deployments_use_a_host_prefixed_session_cookie() {
    let t = TestServer::start_with(|s| {
        s.file.server.public_url = "https://workbench.example.com".into();
    })
    .await;
    let r = t
        .post("/api/v1/auth/login", None)
        .json(&json!({ "token": t.viewer }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let set = r.headers()["set-cookie"].to_str().unwrap().to_owned();
    assert!(set.starts_with("__Host-h2w_session="), "{set}");
    for attr in ["HttpOnly", "SameSite=Strict", "Secure", "Path=/"] {
        assert!(set.contains(attr), "{attr} missing: {set}");
    }
    assert!(!set.to_ascii_lowercase().contains("domain="), "{set}");
    let pair = set.split(';').next().unwrap().to_owned();
    let value = pair.split_once('=').unwrap().1.to_owned();
    let with_cookie = |cookie: String| {
        t.client
            .get(t.url("/api/v1/auth/me"))
            .header("cookie", cookie)
            .send()
    };
    assert_eq!(with_cookie(pair.clone()).await.unwrap().status(), 200);
    // The same session under the plain name is not accepted over HTTPS.
    assert_eq!(
        with_cookie(format!("h2w_session={value}"))
            .await
            .unwrap()
            .status(),
        401
    );
    // Logout clears the prefixed cookie.
    let r = t
        .client
        .post(t.url("/api/v1/auth/logout"))
        .header("cookie", &pair)
        .header("x-h2w-request", "1")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    let cleared = r.headers()["set-cookie"].to_str().unwrap();
    assert!(
        cleared.starts_with("__Host-h2w_session=;") && cleared.contains("Max-Age=0"),
        "{cleared}"
    );
    assert_eq!(with_cookie(pair).await.unwrap().status(), 401);
    t.stop().await;
}

async fn audit_rows(t: &TestServer) -> u64 {
    t.srv()
        .state
        .db
        .call(|c| h2w_daemon::db::audit::head(c))
        .await
        .unwrap()
        .map_or(0, |(seq, _)| seq)
}

/// What a limiter of `per_minute` hands back over `elapsed`, rounded up: its buckets refill
/// continuously, so a slow run (a busy CI runner) earns that many attempts beyond the quota.
fn refilled(per_minute: u32, elapsed: std::time::Duration) -> u32 {
    let ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
    u32::try_from(u64::from(per_minute).saturating_mul(ms) / 60_000 + 1).unwrap_or(u32::MAX)
}

/// Junk credentials cannot flood the hash-chained audit log: per client, a few failures a
/// minute are recorded, the rest only counted, and past a limit failures are 429.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_authentication_cannot_flood_the_audit_log() {
    let t = TestServer::start().await;
    let before = audit_rows(&t).await;
    let audit_quota = h2w_daemon::http::extract::AUDITED_FAILURES_PER_MINUTE;
    let started = std::time::Instant::now();
    let mut statuses = std::collections::BTreeMap::new();
    for i in 0..500 {
        // Malformed, unknown-id and wrong-secret tokens, in turn.
        let junk = match i % 3 {
            0 => format!("junk-{i}"),
            1 => format!("h2w_{:08}_{}", i, "A".repeat(43)),
            _ => format!("{}{}", &t.viewer[..13], "B".repeat(43)),
        };
        let r = me(&t, &junk).await;
        *statuses.entry(r.status().as_u16()).or_insert(0) += 1;
        if r.status() == 429 {
            assert!(r.headers().contains_key("retry-after"));
        }
    }
    let elapsed = started.elapsed();
    let audited = audit_rows(&t).await - before;
    assert!(
        (1..=u64::from(audit_quota + refilled(audit_quota, elapsed))).contains(&audited),
        "{audited} audit rows for 500 failures in {elapsed:?}"
    );
    // The quota answered 401 (plus what refilled while the requests ran), the rest 429.
    let limit = h2w_daemon::http::extract::AUTH_FAILURES_PER_MINUTE;
    let answered = statuses.get(&401).copied().unwrap_or(0);
    assert!(
        (limit..=limit + refilled(limit, elapsed)).contains(&answered),
        "{statuses:?} in {elapsed:?}"
    );
    assert_eq!(
        statuses.get(&429).copied(),
        Some(500 - answered),
        "{statuses:?}"
    );
    // Failed logins share the record budget: at most what refilled meanwhile.
    for _ in 0..5 {
        let r = t
            .post("/api/v1/auth/login", None)
            .json(&json!({"token": "h2w_junk"}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 401);
    }
    let elapsed = started.elapsed();
    let audited = audit_rows(&t).await - before;
    assert!(
        audited <= u64::from(audit_quota + refilled(audit_quota, elapsed)),
        "{audited} audit rows after the logins, in {elapsed:?}"
    );
    // Valid credentials from the same client still work, and every failure was counted.
    assert_eq!(me(&t, &t.admin).await.status(), 200);
    let m = t
        .get("/metrics", Some(&t.viewer))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let count = |reason: &str| -> u64 {
        let key = format!("h2w_auth_failures_total{{reason=\"{reason}\"}} ");
        m.lines()
            .find_map(|l| l.strip_prefix(key.as_str()))
            .map_or(0, |v| v.trim().parse().unwrap())
    };
    // One reason per refused request: those answered 401 plus the 5 logins, then 429s.
    assert_eq!(
        count("malformed") + count("unknown_token") + count("bad_secret"),
        u64::from(answered) + 5,
        "{m}"
    );
    assert_eq!(count("rate_limited"), u64::from(500 - answered), "{m}");
    let suppressed = m
        .lines()
        .find_map(|l| l.strip_prefix("h2w_audit_suppressed_total "))
        .map_or(0, |v| v.trim().parse::<u64>().unwrap());
    assert_eq!(suppressed, u64::from(answered) + 5 - audited, "{m}");
    t.stop().await;
}

/// An authenticated caller cannot flood the audit log with requests it may not make: a
/// principal's refusals are recorded a few a minute, the rest only counted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refused_requests_cannot_flood_the_audit_log() {
    let t = TestServer::start().await;
    let before = audit_rows(&t).await;
    let started = std::time::Instant::now();
    for _ in 0..500 {
        let r = t
            .get("/api/v1/tokens", Some(&t.viewer))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 403);
    }
    let (elapsed, quota) = (
        started.elapsed(),
        h2w_daemon::http::extract::AUDITED_FAILURES_PER_MINUTE,
    );
    let audited = audit_rows(&t).await - before;
    assert!(
        (1..=u64::from(quota + refilled(quota, elapsed))).contains(&audited),
        "{audited} audit rows for 500 refusals in {elapsed:?}"
    );
    // Another principal's refusals are recorded on their own share.
    let r = t
        .get("/api/v1/tokens", Some(&t.operator))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    assert_eq!(audit_rows(&t).await - before, audited + 1);
    let m = t.srv().state.metrics.encode();
    let suppressed = m
        .lines()
        .find_map(|l| l.strip_prefix("h2w_audit_suppressed_total "))
        .map_or(0, |v| v.trim().parse::<u64>().unwrap());
    assert_eq!(suppressed, 500 - audited, "{m}");
    t.stop().await;
}

/// Refused job requests (a kind the role may not start, a cancel of a finished job) share
/// the principal's quota of refusal records with `access.denied`, so an operator cannot
/// flood the audit log either.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refused_job_requests_cannot_flood_the_audit_log() {
    let t = TestServer::start().await;
    let before = audit_rows(&t).await;
    let started = std::time::Instant::now();
    for _ in 0..200 {
        let r = t
            .create_job(
                &t.operator,
                serde_json::json!({"kind": "install_harness2", "params": {}}),
            )
            .await;
        assert_eq!(r.status(), 403);
    }
    let (elapsed, quota) = (
        started.elapsed(),
        h2w_daemon::http::extract::AUDITED_FAILURES_PER_MINUTE,
    );
    let audited = audit_rows(&t).await - before;
    assert!(
        (1..=u64::from(quota + refilled(quota, elapsed))).contains(&audited),
        "{audited} audit rows for 200 refused job requests in {elapsed:?}"
    );
    t.stop().await;
}

/// Behind trusted proxies the client comes from X-Forwarded-For, which any process on the
/// proxy's network can invent. One peer cannot fill the limiter table that way: past its
/// quota of clients it only limits itself, and a fresh client behind another peer still
/// signs in.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_peer_inventing_forwarded_clients_only_limits_itself() {
    let t = TestServer::start_with(|s| {
        s.file.server.trusted_proxies = vec!["127.0.0.0/8".into()];
    })
    .await;
    let from = |source: &str| {
        reqwest::Client::builder()
            .no_proxy()
            .local_address(Some(source.parse().unwrap()))
            .build()
            .unwrap()
    };
    let flooder = from("127.0.0.2");
    let login = |c: &reqwest::Client, xff: String, token: String| {
        c.post(t.url("/api/v1/auth/login"))
            .header("x-h2w-request", "1")
            .header("x-forwarded-for", xff)
            .json(&json!({ "token": token }))
            .send()
    };
    let per_peer = u32::try_from(h2w_daemon::ratelimit::MAX_PER_PEER).unwrap();
    let started = std::time::Instant::now();
    let mut statuses = std::collections::BTreeMap::new();
    for i in 0..(per_peer + 100) {
        let xff = format!("198.18.{}.{}", i / 250, i % 250 + 1);
        let r = login(&flooder, xff, "h2w_junk".into()).await.unwrap();
        *statuses.entry(r.status().as_u16()).or_insert(0_u32) += 1;
    }
    // Every invented client up to the quota had a bucket; the rest shared one of this peer
    // (plus what it refilled while the requests ran).
    let elapsed = started.elapsed();
    let limit = t.srv().state.cfg.file.server.login_attempts_per_minute;
    let answered = statuses.get(&401).copied().unwrap_or(0);
    assert!(
        (per_peer + limit..=per_peer + limit + refilled(limit, elapsed)).contains(&answered),
        "{statuses:?} in {elapsed:?}"
    );
    assert_eq!(
        statuses.get(&429).copied(),
        Some(per_peer + 100 - answered),
        "{statuses:?}"
    );
    // The flooding peer limited itself, also for a valid token from a new address...
    let r = login(&flooder, "203.0.113.7".into(), t.admin.clone())
        .await
        .unwrap();
    assert_eq!(r.status(), 429);
    // ...while a fresh client behind another peer signs in.
    let honest = from("127.0.0.3");
    let r = login(&honest, "203.0.113.8".into(), t.admin.clone())
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    t.stop().await;
}

/// A token-length string whose multi-byte character straddles the id/secret split once
/// panicked the login handler (the connection was dropped instead of answered).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_ascii_tokens_are_refused_with_401() {
    let t = TestServer::start().await;
    let tokens = [
        format!("h2w_abcdefgé{}", "A".repeat(43)),
        format!("h2w_{}", "é".repeat(26)),
        format!("h2w_abcdefg😀{}", "B".repeat(41)),
    ];
    for token in &tokens {
        let r = t
            .post("/api/v1/auth/login", None)
            .json(&json!({ "token": token }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 401, "{token}");
        assert_eq!(
            r.json::<ErrorBody>().await.unwrap().error.code,
            ErrorCode::Unauthorized
        );
        // The same bytes as a bearer token (obs-text in the header) are refused too.
        let header =
            reqwest::header::HeaderValue::from_bytes(format!("Bearer {token}").as_bytes()).unwrap();
        let r = t
            .client
            .get(t.url("/api/v1/auth/me"))
            .header("authorization", header)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 401, "{token}");
    }
    // The server is still serving.
    assert_eq!(me(&t, &t.admin).await.status(), 200);
    t.stop().await;
}

const PROXY_SECRET: &str = "proxy-shared-secret-0123456789";

fn proxy_setup(s: &mut Setup, trusted: &str) {
    s.file.auth.mode = h2w_api::AuthMode::Proxy;
    s.file.server.trusted_proxies = vec![trusted.to_owned()];
    s.file.auth.proxy.admin_groups = vec!["admins".into()];
    s.file.auth.proxy.operator_groups = vec!["ops".into()];
    s.file.auth.proxy.viewer_groups = vec!["staff".into()];
    s.proxy_secret = Some(PROXY_SECRET.to_owned());
}

async fn proxied(t: &TestServer, user: &str, groups: &str, secret: &str) -> reqwest::Response {
    t.client
        .get(t.url("/api/v1/auth/me"))
        .header("x-forwarded-user", user)
        .header("x-forwarded-groups", groups)
        .header("x-h2w-proxy-secret", secret)
        .send()
        .await
        .unwrap()
}

/// Audit actors carry their kind: a proxy user who calls itself `initial admin` is recorded
/// as `proxy:initial admin`, never as the bootstrap token (`token:initial admin`). The
/// current-user view keeps the plain name.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audit_actors_name_their_kind() {
    let t = TestServer::start_with(|s| proxy_setup(s, "127.0.0.1")).await;
    let r = t
        .client
        .put(t.url("/api/v1/secrets/POSE_KEY"))
        .header("x-forwarded-user", "initial admin")
        .header("x-forwarded-groups", "admins")
        .header("x-h2w-proxy-secret", PROXY_SECRET)
        .header("x-h2w-request", "1")
        .json(&json!({"value": "pose-value-0123456789"}))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.text().await.unwrap());
    let r = proxied(&t, "initial admin", "admins", PROXY_SECRET).await;
    assert_eq!(r.json::<Principal>().await.unwrap().name, "initial admin");
    let r = t
        .send(
            reqwest::Method::PUT,
            "/api/v1/secrets/TOKEN_KEY",
            Some(&t.admin),
        )
        .json(&json!({"value": "token-value-0123456789"}))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());
    let r = t
        .get("/api/v1/tokens", Some(&t.viewer))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    let audit: Page<AuditEvent> = t
        .get("/api/v1/audit?limit=100", Some(&t.admin))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let actor_of = |action: &str, target: &str| {
        audit
            .items
            .iter()
            .find(|e| e.action == action && e.target == target)
            .map(|e| e.actor.clone())
            .unwrap_or_else(|| panic!("no {action} {target} in {:#?}", audit.items))
    };
    assert_eq!(actor_of("secret.set", "POSE_KEY"), "proxy:initial admin");
    assert_eq!(actor_of("secret.set", "TOKEN_KEY"), "token:admin-t");
    let denied = audit
        .items
        .iter()
        .find(|e| e.action == "access.denied")
        .unwrap();
    assert_eq!(denied.actor, "token:viewer-t");
    assert!(
        denied
            .detail
            .as_deref()
            .unwrap()
            .starts_with("token:viewer-t holds viewer"),
        "{denied:?}"
    );
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proxy_mode_maps_groups_to_roles() {
    let t = TestServer::start_with(|s| proxy_setup(s, "127.0.0.1")).await;
    for (groups, role) in [
        ("admins", Role::Admin),
        ("staff, ops", Role::Operator),
        ("staff", Role::Viewer),
    ] {
        let r = proxied(&t, "alice", groups, PROXY_SECRET).await;
        assert_eq!(r.status(), 200, "{groups}");
        let p: Principal = r.json().await.unwrap();
        assert_eq!(
            (p.role, p.kind, p.name.as_str()),
            (role, PrincipalKind::Proxy, "alice")
        );
    }
    let r = proxied(&t, "bob", "visitors", PROXY_SECRET).await;
    assert_eq!(r.status(), 403);
    let r = proxied(&t, "mallory", "admins", "guessed-secret").await;
    assert_eq!(r.status(), 401);
    // Tokens keep working in proxy mode.
    assert_eq!(me(&t, &t.viewer).await.status(), 200);
    let audit: Page<AuditEvent> = t
        .get("/api/v1/audit?limit=100", Some(&t.admin))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        audit
            .items
            .iter()
            .any(|e| e.action == "auth.failed" && e.target == "mallory")
    );
    t.stop().await;
}

/// A client value the proxy failed to strip must not pick the identity or add a group.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repeated_proxy_identity_headers_are_refused() {
    let t = TestServer::start_with(|s| proxy_setup(s, "127.0.0.1")).await;
    let base = || {
        t.client
            .get(t.url("/api/v1/auth/me"))
            .header("x-h2w-proxy-secret", PROXY_SECRET)
    };
    let cases = [
        (
            "groups",
            base()
                .header("x-forwarded-user", "alice")
                .header("x-forwarded-groups", "staff")
                .header("x-forwarded-groups", "admins"),
        ),
        (
            "user",
            base()
                .header("x-forwarded-user", "mallory")
                .header("x-forwarded-user", "alice")
                .header("x-forwarded-groups", "staff"),
        ),
        (
            "secret",
            base()
                .header("x-h2w-proxy-secret", "something-else-entirely")
                .header("x-forwarded-user", "alice")
                .header("x-forwarded-groups", "staff"),
        ),
    ];
    for (what, req) in cases {
        let r = req.send().await.unwrap();
        assert_eq!(r.status(), 401, "repeated {what} header");
        assert_eq!(
            r.json::<ErrorBody>().await.unwrap().error.code,
            ErrorCode::Unauthorized
        );
    }
    // One of each still works, and the refusals were audited with their reason.
    let r = proxied(&t, "alice", "staff, admins", PROXY_SECRET).await;
    assert_eq!(r.status(), 200);
    assert_eq!(r.json::<Principal>().await.unwrap().role, Role::Admin);
    let audit: Page<AuditEvent> = t
        .get("/api/v1/audit?limit=100", Some(&t.admin))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let dup = audit
        .items
        .iter()
        .filter(|e| {
            e.action == "auth.failed"
                && e.detail
                    .as_deref()
                    .is_some_and(|d| d.starts_with("proxy_duplicate_header"))
        })
        .count();
    assert_eq!(dup, 3, "{:#?}", audit.items);
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proxy_headers_from_untrusted_peers_are_refused() {
    let t = TestServer::start_with(|s| proxy_setup(s, "10.9.8.7/32")).await;
    let r = proxied(&t, "alice", "admins", PROXY_SECRET).await;
    assert_eq!(r.status(), 401);
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proxy_mode_refuses_to_start_without_its_secret() {
    let root = tempfile::tempdir().unwrap();
    let mut setup = Setup::new(root.path());
    proxy_setup(&mut setup, "127.0.0.1");
    let cfg = h2w_daemon::config::Config::from_file(setup.file.clone(), None, &|_| None).unwrap();
    let mut opts = h2w_daemon::http::server::ServerOptions::new(std::sync::Arc::new(FakeBackend {
        home: root.path().join("harness2"),
        secret_value: String::new(),
        bridge_key: String::new(),
        sleep_secs: 1.0,
    }));
    opts.proxy_secret = Some("short".into());
    let err = h2w_daemon::http::server::Server::bind(cfg, opts)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("at least 16"), "{err}");
}
