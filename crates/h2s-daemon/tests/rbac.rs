//! Every route of the router (built only from `ROUTES`) enforces the role recorded next to
//! it: unauthenticated -> 401, insufficient role -> 403, sufficient role -> neither.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::*;
use h2s_api::Role;
use h2s_daemon::http::{Access, ROUTES};
use reqwest::Method;
use serde_json::{Value, json};

fn sample_path(path: &str) -> String {
    let job = "0190c5f0-0000-7000-8000-000000000000";
    path.replace("{name}", "p1")
        .replace(
            "{id}",
            if path.starts_with("/api/v1/tokens") {
                "abcd1234"
            } else {
                job
            },
        )
        .replace("{exp}", "e1")
        .replace("{sweep}", "lab-oc-agg1")
        .replace("{task}", "t1")
        .replace("{mode}", "parallel")
        .replace("{label}", "base")
        .replace("{session}", "s1")
        .replace("{part}", "mission")
}

fn query_for(path: &str) -> &'static str {
    if path.ends_with("/diff") {
        "?from=base&to=c1"
    } else if path.ends_with("/export") {
        "?target=raw&format=json"
    } else if path.ends_with("/file") {
        "?path=a.txt"
    } else if path == "/api/v1/compare" {
        "?a=e1/lab-oc-agg1/parallel&b=e1/lab-oc-agg1/sequential"
    } else {
        ""
    }
}

fn body_for(method: &Method, path: &str, admin_job: bool) -> Option<Value> {
    if *method == Method::GET || *method == Method::DELETE {
        return None;
    }
    Some(match path {
        "/api/v1/auth/login" => json!({"token": "h2s_x"}),
        "/api/v1/profiles/{name}" => json!({"name": "p1", "kind": "custom", "solver_model": "m/x"}),
        "/api/v1/secrets/{name}" => json!({"value": "abcdefgh12345678"}),
        "/api/v1/tokens" => json!({"name": "t", "role": "viewer"}),
        "/api/v1/experiments" => json!({"name": "e1"}),
        "/api/v1/experiments/{exp}" => json!({}),
        "/api/v1/estimate" => json!({"tasks": 1, "k": 1, "modes": ["parallel"]}),
        "/api/v1/harness/validate" => json!({"surface": "opencode", "files": []}),
        "/api/v1/jobs" if admin_job => json!({"kind": "install_harness2", "params": {}}),
        "/api/v1/jobs" => demo_spec("ok-rbac"),
        _ => return None,
    })
}

async fn call(
    t: &TestServer,
    method: &Method,
    path: &str,
    token: Option<&str>,
    admin_job: bool,
) -> u16 {
    let url = format!("{}{}", sample_path(path), query_for(path));
    let mut req = t.client.request(method.clone(), t.url(&url));
    if *method != Method::GET {
        req = req.header("x-h2s-request", "1");
    }
    if let Some(tok) = token {
        req = req.bearer_auth(tok);
    }
    if let Some(b) = body_for(method, path, admin_job) {
        req = req.json(&b);
    }
    let r = req.send().await.unwrap();
    let status = r.status().as_u16();
    // Every error is an ErrorBody (the readiness probe has its own body).
    if status >= 400 && path != "/readyz" {
        let text = r.text().await.unwrap();
        let v: Value = serde_json::from_str(&text)
            .unwrap_or_else(|_| panic!("{method} {path}: not JSON: {text}"));
        assert!(v["error"]["code"].is_string(), "{method} {path}: {text}");
    }
    status
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_route_enforces_its_documented_role() {
    let t = TestServer::start().await;
    let mut checked = 0;
    for r in ROUTES {
        let m = &r.method;
        let anon = call(&t, m, r.path, None, false).await;
        let viewer = call(&t, m, r.path, Some(&t.viewer), false).await;
        let operator = call(&t, m, r.path, Some(&t.operator), false).await;
        let admin = call(&t, m, r.path, Some(&t.admin), false).await;
        let ok = |s: u16| s != 401 && s != 403;
        match r.access {
            Access::Public => {
                assert!(
                    ok(anon) || r.path == "/api/v1/auth/login",
                    "{m} {}: anon {anon}",
                    r.path
                );
            }
            Access::Authenticated | Access::MetricsOrViewer | Access::Role(Role::Viewer) => {
                assert_eq!(anon, 401, "{m} {}", r.path);
                assert!(
                    ok(viewer) && ok(operator) && ok(admin),
                    "{m} {}: {viewer} {operator} {admin}",
                    r.path
                );
            }
            Access::Role(Role::Operator) => {
                assert_eq!(anon, 401, "{m} {}", r.path);
                assert_eq!(viewer, 403, "{m} {}", r.path);
                assert!(
                    ok(operator) && ok(admin),
                    "{m} {}: {operator} {admin}",
                    r.path
                );
            }
            Access::Role(Role::Admin) => {
                assert_eq!(anon, 401, "{m} {}", r.path);
                assert_eq!(viewer, 403, "{m} {}", r.path);
                assert_eq!(operator, 403, "{m} {}", r.path);
                assert!(ok(admin), "{m} {}: {admin}", r.path);
            }
            Access::JobSpec => {
                assert_eq!(anon, 401);
                assert_eq!(viewer, 403);
                assert!(ok(operator), "operator demo: {operator}");
                let op_admin_kind = call(&t, m, r.path, Some(&t.operator), true).await;
                assert_eq!(
                    op_admin_kind, 403,
                    "operators must not start admin-only kinds"
                );
                let admin_kind = call(&t, m, r.path, Some(&t.admin), true).await;
                assert!(ok(admin_kind), "admin install: {admin_kind}");
            }
        }
        checked += 1;
    }
    assert_eq!(checked, ROUTES.len());
    t.stop().await;
}

/// The table is the router: every (path, method) answers, and other methods on the same
/// path are 405, never 404.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_router_serves_exactly_the_route_table() {
    let t = TestServer::start().await;
    let mut methods: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for r in ROUTES {
        methods
            .entry(r.path)
            .or_default()
            .insert(r.method.to_string());
    }
    for (path, registered) in &methods {
        let unregistered = ["GET", "PUT", "PATCH", "DELETE", "POST"]
            .into_iter()
            .find(|m| !registered.contains(*m))
            .unwrap();
        let m = Method::from_bytes(unregistered.as_bytes()).unwrap();
        let url = format!("{}{}", sample_path(path), query_for(path));
        let r = t
            .client
            .request(m.clone(), t.url(&url))
            .header("x-h2s-request", "1")
            .bearer_auth(&t.admin)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 405, "{m} {path} should be 405");
    }
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn static_ui_and_health_need_no_credentials() {
    let t = TestServer::start().await;
    for p in ["/", "/experiments/x", "/jobs/y", "/healthz", "/readyz"] {
        let r = t.client.get(t.url(p)).send().await.unwrap();
        assert_eq!(r.status(), 200, "{p}");
    }
    t.stop().await;
}
