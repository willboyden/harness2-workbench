//! The embedded web UI: allowlisted files, SPA fallback, JSON 404s for the API, headers.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::*;
use h2w_api::ErrorBody;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn index_and_client_routes_serve_the_ui() {
    let t = TestServer::start().await;
    let built = h2w_daemon::http::static_ui::ui_built();
    let index = t.client.get(t.url("/")).send().await.unwrap();
    assert_eq!(index.status(), 200);
    assert_eq!(index.headers()["content-type"], "text/html; charset=utf-8");
    assert_eq!(index.headers()["cache-control"], "no-cache");
    assert!(
        index.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("default-src 'none'")
    );
    let body = index.text().await.unwrap();
    if built {
        assert!(body.to_lowercase().contains("<html"), "{body}");
    } else {
        assert_eq!(body, h2w_daemon::http::static_ui::NOT_BUILT_PAGE);
        assert!(body.contains("cargo xtask web"));
    }
    for p in [
        "/experiments/demo",
        "/jobs/0190c5f0-0000-7000-8000-000000000000",
        "/settings/tokens",
        "/secret.txt",
        "/Cargo.toml",
    ] {
        let r = t.client.get(t.url(p)).send().await.unwrap();
        assert_eq!(r.status(), 200, "{p}");
        assert_eq!(
            r.text().await.unwrap(),
            body,
            "{p} must fall back to the index"
        );
    }
    let head = t.client.head(t.url("/")).send().await.unwrap();
    assert_eq!(head.status(), 200);
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_allowlisted_files_are_served_and_traversal_goes_nowhere() {
    let t = TestServer::start().await;
    let addr = t.srv().addr;
    let port = addr.port();
    for path in [
        "/pkg/../../Cargo.toml",
        "/pkg/%2e%2e/%2e%2e/Cargo.toml",
        "/../../etc/passwd",
    ] {
        let raw = raw_http(
            addr,
            format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await;
        assert!(
            !raw.contains("[workspace]") && !raw.contains("root:x:0:0"),
            "{path} leaked a file"
        );
    }
    if !h2w_daemon::http::static_ui::ui_built() {
        let r = t.client.get(t.url("/app.js")).send().await.unwrap();
        assert_eq!(r.status(), 404);
        let e: ErrorBody = r.json().await.unwrap();
        assert!(e.error.message.contains("not part of this build"));
    }
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_api_paths_and_methods_are_json_errors() {
    let t = TestServer::start().await;
    let r = t
        .get("/api/v1/nothing-here", Some(&t.viewer))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    let e: ErrorBody = r.json().await.unwrap();
    assert_eq!(e.error.code, h2w_api::ErrorCode::NotFound);
    let r = t.client.get(t.url("/api")).send().await.unwrap();
    assert_eq!(r.status(), 404);
    let r = t.post("/not-a-page", Some(&t.viewer)).send().await.unwrap();
    assert_eq!(r.status(), 404);
    assert!(r.json::<ErrorBody>().await.is_ok());
    t.stop().await;
}
