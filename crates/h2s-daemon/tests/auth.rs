//! Authentication: bearer tokens, session cookies, expiry, revocation, login rate limit,
//! proxy mode.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::*;
use h2s_api::{AuditEvent, ErrorBody, ErrorCode, Page, Principal, PrincipalKind, Role, TokenInfo};
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
    let unknown = format!("h2s_zzzzzzzz_{}", &t.admin[13..]);
    for bad in [wrong.as_str(), unknown.as_str(), "nope", "h2s_", ""] {
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
        m.contains("h2s_auth_failures_total{reason=\"bad_secret\"}"),
        "{m}"
    );
    assert!(
        m.contains("h2s_auth_failures_total{reason=\"missing\"}"),
        "{m}"
    );
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expired_and_revoked_tokens_stop_working() {
    let t = TestServer::start().await;
    let expired = t
        .token("old", Role::Viewer, Some(h2s_daemon::util::now_unix() - 10))
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
        "h2s_session=",
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
        .header("x-h2s-request", "1")
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
        "h2s_session=abc",
        "h2s_session=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
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
        .json(&json!({"token": "h2s_nope"}))
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
    let (sid, hash) = h2s_daemon::auth::new_session().unwrap();
    let tid = id_of(&t.viewer).to_owned();
    let now = h2s_daemon::util::now_unix();
    t.srv()
        .state
        .db
        .call(move |c| h2s_daemon::db::sessions::insert(c, &hash, &tid, now - 100, now - 1))
        .await
        .unwrap();
    let r = t
        .client
        .get(t.url("/api/v1/auth/me"))
        .header("cookie", format!("h2s_session={}", sid.as_str()))
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
            .json(&json!({"token": "h2s_bad"}))
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

/// A token-length string whose multi-byte character straddles the id/secret split once
/// panicked the login handler (the connection was dropped instead of answered).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_ascii_tokens_are_refused_with_401() {
    let t = TestServer::start().await;
    let tokens = [
        format!("h2s_abcdefgé{}", "A".repeat(43)),
        format!("h2s_{}", "é".repeat(26)),
        format!("h2s_abcdefg😀{}", "B".repeat(41)),
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
    s.file.auth.mode = h2s_api::AuthMode::Proxy;
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
        .header("x-h2s-proxy-secret", secret)
        .send()
        .await
        .unwrap()
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
            .header("x-h2s-proxy-secret", PROXY_SECRET)
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
                .header("x-h2s-proxy-secret", "something-else-entirely")
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
    let cfg = h2s_daemon::config::Config::from_file(setup.file.clone(), None, &|_| None).unwrap();
    let mut opts = h2s_daemon::http::server::ServerOptions::new(std::sync::Arc::new(FakeBackend {
        home: root.path().join("harness2"),
        secret_value: String::new(),
        bridge_key: String::new(),
        sleep_secs: 1.0,
    }));
    opts.proxy_secret = Some("short".into());
    let err = h2s_daemon::http::server::Server::bind(cfg, opts)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("at least 16"), "{err}");
}
