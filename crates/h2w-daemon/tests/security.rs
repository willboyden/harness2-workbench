//! The outer middleware: Host allowlist, CSRF header and Origin, no CORS, security headers
//! and error bodies on every response, cache control, body limits, request ids.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::time::Duration;

use common::*;
use h2w_api::ErrorBody;
use h2w_daemon::http::guard::SECURITY_HEADERS;
use serde_json::json;

fn header_block(raw: &str) -> String {
    raw.split("\r\n\r\n")
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn status_of(raw: &str) -> u16 {
    raw.split_whitespace().nth(1).unwrap().parse().unwrap()
}

fn assert_security_headers_raw(raw: &str) {
    let h = header_block(raw);
    for (k, v) in SECURITY_HEADERS {
        assert!(
            h.contains(&format!("{k}: {}", v.to_ascii_lowercase())),
            "{k} missing in\n{h}"
        );
    }
    assert!(!h.contains("access-control-"), "CORS header sent:\n{h}");
}

fn assert_security_headers(r: &reqwest::Response) {
    for (k, v) in SECURITY_HEADERS {
        assert_eq!(
            r.headers().get(*k).map(|x| x.to_str().unwrap()),
            Some(*v),
            "{k} on {}",
            r.url()
        );
    }
    assert!(
        !r.headers()
            .keys()
            .any(|k| k.as_str().starts_with("access-control-")),
        "CORS header on {}",
        r.url()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn foreign_host_headers_are_misdirected() {
    let t = TestServer::start().await;
    let addr = t.srv().addr;
    let port = addr.port();
    for host in [
        "evil.example".to_owned(),
        format!("evil.example:{port}"),
        format!("127.0.0.1.nip.io:{port}"),
        "127.0.0.1".to_owned(),
        format!("localhost.:{port}"),
    ] {
        let raw = raw_http(
            addr,
            format!("GET /healthz HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await;
        assert_eq!(status_of(&raw), 421, "{host}: {raw}");
        assert_security_headers_raw(&raw);
        assert!(raw.contains("\"misdirected_request\""), "{raw}");
        assert!(raw.contains("\"request_id\""), "{raw}");
    }
    // Static files and the SPA fallback are protected the same way.
    let raw = raw_http(
        addr,
        b"GET / HTTP/1.1\r\nHost: rebind.example\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert_eq!(status_of(&raw), 421);
    // HTTP/1.0 without Host.
    let raw = raw_http(addr, b"GET /healthz HTTP/1.0\r\n\r\n").await;
    assert_eq!(status_of(&raw), 421, "{raw}");
    // Two Host headers are a 400 whatever their values (RFC 9112 §3.2), first allowed or not.
    for (first, second) in [
        (format!("127.0.0.1:{port}"), "evil.example".to_owned()),
        ("evil.example".to_owned(), format!("127.0.0.1:{port}")),
        (format!("127.0.0.1:{port}"), format!("127.0.0.1:{port}")),
    ] {
        let raw = raw_http(
            addr,
            format!(
                "GET /healthz HTTP/1.1\r\nHost: {first}\r\nHost: {second}\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await;
        assert_eq!(status_of(&raw), 400, "{first} + {second}: {raw}");
        assert_security_headers_raw(&raw);
        assert!(raw.contains("\"bad_request\""), "{raw}");
    }
    // An absolute-form target is checked as well as the Host header.
    let raw = raw_http(
        addr,
        format!(
            "GET http://evil.example/healthz HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
        )
        .as_bytes(),
    )
    .await;
    assert_eq!(status_of(&raw), 421, "{raw}");
    let raw = raw_http(
        addr,
        format!(
            "GET http://127.0.0.1:{port}/healthz HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
        )
        .as_bytes(),
    )
    .await;
    assert_eq!(status_of(&raw), 200, "{raw}");
    for host in [
        format!("127.0.0.1:{port}"),
        format!("localhost:{port}"),
        format!("LOCALHOST:{port}"),
    ] {
        let raw = raw_http(
            addr,
            format!("GET /healthz HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await;
        assert_eq!(status_of(&raw), 200, "{host}: {raw}");
    }
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsafe_methods_need_the_csrf_header_and_a_same_origin() {
    let t = TestServer::start().await;
    let port = t.port();
    // No header, wrong value: refused before authentication, even for an admin.
    for (method, path) in [
        (reqwest::Method::POST, "/api/v1/auth/login"),
        (reqwest::Method::PUT, "/api/v1/secrets/X_KEY"),
        (reqwest::Method::DELETE, "/api/v1/tokens/abcd1234"),
        (reqwest::Method::PATCH, "/api/v1/experiments/e1"),
        (reqwest::Method::POST, "/api/v1/jobs"),
    ] {
        let r = t
            .client
            .request(method.clone(), t.url(path))
            .bearer_auth(&t.admin)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 403, "{method} {path}");
        assert_security_headers(&r);
        let e: ErrorBody = r.json().await.unwrap();
        assert!(e.error.message.contains("x-h2w-request"));
        let r = t
            .client
            .request(method.clone(), t.url(path))
            .bearer_auth(&t.admin)
            .header("x-h2w-request", "0")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 403, "{method} {path} with value 0");
    }
    // Cross-site and opaque origins are refused; same-origin passes on to the handler.
    for origin in [
        "http://evil.example",
        "null",
        &format!("http://127.0.0.1:{}", port + 1),
    ] {
        let r = t
            .post("/api/v1/auth/login", None)
            .header("origin", origin)
            .json(&json!({"token": t.admin}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 403, "{origin}");
    }
    for origin in [format!("http://127.0.0.1:{port}")] {
        let r = t
            .post("/api/v1/auth/login", None)
            .header("origin", &origin)
            .json(&json!({"token": t.admin}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "{origin}");
    }
    let raw = raw_http(
        t.srv().addr,
        format!(
            "POST /api/v1/auth/logout HTTP/1.1\r\nHost: localhost:{port}\r\nOrigin: http://localhost:{port}\r\nx-h2w-request: 1\r\nAuthorization: Bearer {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            t.viewer
        )
        .as_bytes(),
    )
    .await;
    assert_eq!(status_of(&raw), 204, "{raw}");
    t.stop().await;
}

/// Behind an https public URL a plain-http origin of the same host is refused, and a request
/// with two Origin headers is refused whatever they say.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn https_deployments_accept_only_https_origins_and_one_origin_header() {
    let t = TestServer::start_with(|s| {
        s.file.server.public_url = "https://studio.example.com".into();
    })
    .await;
    let port = t.port();
    let login = |origins: Vec<String>| {
        let mut headers = String::new();
        for o in &origins {
            headers.push_str(&format!("Origin: {o}\r\n"));
        }
        let body = format!("{{\"token\":\"{}\"}}", t.viewer);
        format!(
            "POST /api/v1/auth/login HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{headers}x-h2w-request: 1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    };
    let addr = t.srv().addr;
    let https = format!("https://127.0.0.1:{port}");
    let http = format!("http://127.0.0.1:{port}");
    for (origins, status) in [
        (vec!["https://studio.example.com".to_owned()], 200),
        (vec![https.clone()], 200),
        (vec![http.clone()], 403),
        (vec![https.clone(), https.clone()], 403),
        (vec!["https://studio.example.com".to_owned(), http], 403),
    ] {
        let raw = raw_http(addr, login(origins.clone()).as_bytes()).await;
        assert_eq!(status_of(&raw), status, "{origins:?}: {raw}");
    }
    // Over plain http (the default public URL) both schemes of the Host still pass.
    let t2 = TestServer::start().await;
    let port2 = t2.port();
    for origin in [
        format!("http://127.0.0.1:{port2}"),
        format!("https://127.0.0.1:{port2}"),
    ] {
        let r = t2
            .post("/api/v1/auth/login", None)
            .header("origin", &origin)
            .json(&json!({"token": t2.viewer}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "{origin}");
    }
    t2.stop().await;
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_cors_headers_are_ever_sent() {
    let t = TestServer::start().await;
    let r = t
        .client
        .request(reqwest::Method::OPTIONS, t.url("/api/v1/jobs"))
        .header("origin", "http://evil.example")
        .header("access-control-request-method", "POST")
        .header("access-control-request-headers", "x-h2w-request")
        .send()
        .await
        .unwrap();
    assert_security_headers(&r);
    assert!(r.status().is_client_error());
    let r = t
        .get("/api/v1/auth/me", Some(&t.viewer))
        .header("origin", "http://evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_security_headers(&r);
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_response_carries_the_headers_and_errors_carry_request_ids() {
    let t = TestServer::start_with(|s| s.file.server.max_body_bytes = 1024).await;
    let big = "x".repeat(5000);
    let cases: Vec<(reqwest::RequestBuilder, u16)> = vec![
        (t.client.get(t.url("/healthz")), 200),
        (t.client.get(t.url("/api/v1/auth/me")), 401),
        (t.client.post(t.url("/api/v1/jobs")), 403),
        (t.get("/api/v1/does-not-exist", Some(&t.viewer)), 404),
        (t.send(reqwest::Method::DELETE, "/healthz", None), 405),
        (
            t.post("/api/v1/estimate", Some(&t.viewer))
                .header("content-type", "application/json")
                .body(big.clone()),
            413,
        ),
        (t.client.get(t.url("/")), 200),
        (t.client.get(t.url("/some/client/route")), 200),
        (t.get("/api/v1/system/info", Some(&t.viewer)), 200),
        (t.get("/api/v1/jobs?status=bogus", Some(&t.viewer)), 422),
    ];
    for (req, want) in cases {
        let r = req.send().await.unwrap();
        let url = r.url().path().to_owned();
        assert_eq!(r.status().as_u16(), want, "{url}");
        assert_security_headers(&r);
        let rid = r.headers()["x-request-id"].to_str().unwrap().to_owned();
        assert!(!rid.is_empty());
        let api = url.starts_with("/api/") || url == "/healthz";
        if api {
            assert_eq!(r.headers()["cache-control"], "no-store", "{url}");
        } else {
            assert_ne!(r.headers()["cache-control"], "no-store", "{url}");
        }
        if want >= 400 {
            assert!(
                r.headers()["content-type"]
                    .to_str()
                    .unwrap()
                    .starts_with("application/json")
            );
            let e: ErrorBody = r.json().await.unwrap();
            assert_eq!(e.error.request_id.as_deref(), Some(rid.as_str()), "{url}");
        }
    }
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn body_limits_apply_to_declared_and_chunked_bodies() {
    let t = TestServer::start_with(|s| s.file.server.max_body_bytes = 1024).await;
    let addr = t.srv().addr;
    let port = addr.port();
    let body = format!(
        "{{\"tasks\": 1, \"k\": 1, \"modes\": [\"parallel\"], \"profile\": \"{}\"}}",
        "a".repeat(3000)
    );
    let declared = format!(
        "POST /api/v1/estimate HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {}\r\nx-h2w-request: 1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        t.viewer,
        body.len()
    );
    let raw = raw_http(addr, declared.as_bytes()).await;
    assert_eq!(status_of(&raw), 413, "{raw}");
    assert!(raw.contains("\"payload_too_large\""));
    assert_security_headers_raw(&raw);
    let mut chunked = format!(
        "POST /api/v1/estimate HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {}\r\nx-h2w-request: 1\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
        t.viewer
    );
    for piece in body.as_bytes().chunks(500) {
        chunked.push_str(&format!(
            "{:x}\r\n{}\r\n",
            piece.len(),
            std::str::from_utf8(piece).unwrap()
        ));
    }
    chunked.push_str("0\r\n\r\n");
    let raw = raw_http(addr, chunked.as_bytes()).await;
    assert_eq!(status_of(&raw), 413, "{raw}");
    assert!(raw.contains("\"payload_too_large\""));
    // A small chunked body is fine.
    let small = "{\"tasks\": 1, \"k\": 1, \"modes\": [\"parallel\"]}";
    let raw = raw_http(
        addr,
        format!(
            "POST /api/v1/estimate HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {}\r\nx-h2w-request: 1\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{small}\r\n0\r\n\r\n",
            t.viewer,
            small.len()
        )
        .as_bytes(),
    )
    .await;
    assert_eq!(status_of(&raw), 200, "{raw}");
    // Non-JSON bodies are 400, not 413 or 500.
    let r = t
        .post("/api/v1/estimate", Some(&t.viewer))
        .header("content-type", "text/plain")
        .body("hello")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn request_ids_are_honoured_only_from_trusted_proxies() {
    let t = TestServer::start().await;
    let r = t
        .client
        .get(t.url("/healthz"))
        .header("x-request-id", "client-chosen-id")
        .send()
        .await
        .unwrap();
    assert_ne!(r.headers()["x-request-id"], "client-chosen-id");
    t.stop().await;

    let t =
        TestServer::start_with(|s| s.file.server.trusted_proxies = vec!["127.0.0.1".into()]).await;
    let r = t
        .client
        .get(t.url("/api/v1/auth/me"))
        .header("x-request-id", "proxy-id-123")
        .send()
        .await
        .unwrap();
    assert_eq!(r.headers()["x-request-id"], "proxy-id-123");
    let e: ErrorBody = r.json().await.unwrap();
    assert_eq!(e.error.request_id.as_deref(), Some("proxy-id-123"));
    // Invalid ids are replaced even from a trusted proxy.
    let r = t
        .client
        .get(t.url("/healthz"))
        .header("x-request-id", "bad id with spaces")
        .send()
        .await
        .unwrap();
    assert_ne!(r.headers()["x-request-id"], "bad id with spaces");
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn health_readiness_and_metrics() {
    let t = TestServer::start().await;
    let h: serde_json::Value = t
        .client
        .get(t.url("/healthz"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(h, json!({"status": "ok"}));
    let r: serde_json::Value = t
        .client
        .get(t.url("/readyz"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r, json!({"ready": true, "failing": []}));
    let r = t.get("/metrics", Some(&t.viewer)).send().await.unwrap();
    assert!(
        r.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("application/openmetrics-text")
    );
    let m = r.text().await.unwrap();
    for name in [
        "h2w_http_requests_total",
        "h2w_http_request_duration_seconds",
        "h2w_jobs_running",
        "h2w_sse_connections",
    ] {
        assert!(m.contains(name), "{name} missing:\n{m}");
    }
    // Route labels are templates, never raw paths; unknown methods fold into OTHER.
    t.get(
        "/api/v1/jobs/0190c5f0-0000-7000-8000-000000000000",
        Some(&t.viewer),
    )
    .send()
    .await
    .unwrap();
    t.client
        .request(
            reqwest::Method::from_bytes(b"BREW").unwrap(),
            t.url("/healthz"),
        )
        .header("x-h2w-request", "1")
        .send()
        .await
        .unwrap();
    let m = t
        .get("/metrics", Some(&t.viewer))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(m.contains("route=\"/api/v1/jobs/{id}\""), "{m}");
    assert!(!m.contains("0190c5f0"), "a raw path leaked into labels");
    assert!(m.contains("method=\"OTHER\""), "{m}");
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metrics_accept_the_configured_metrics_token() {
    let t = TestServer::start_with(|s| {
        s.file.telemetry.metrics_token_secret = String::new();
    })
    .await;
    // Without a configured metrics secret, a random bearer is just a bad credential.
    let r = t
        .client
        .get(t.url("/metrics"))
        .bearer_auth("scrape-secret-value")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stalled_requests_time_out_with_503() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let t = TestServer::start_with(|s| s.file.server.request_timeout_secs = 1).await;
    let port = t.port();
    let mut s = tokio::net::TcpStream::connect(t.srv().addr).await.unwrap();
    // Promise 100 bytes, send 10, then wait: the handler is still reading the body.
    s.write_all(
        format!(
            "POST /api/v1/auth/login HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nx-h2w-request: 1\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{{\"token\": "
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let started = std::time::Instant::now();
    let mut buf = vec![0u8; 4096];
    let n = tokio::time::timeout(std::time::Duration::from_secs(10), s.read(&mut buf))
        .await
        .expect("no response to a stalled request")
        .unwrap();
    let head = String::from_utf8_lossy(&buf[..n]).into_owned();
    assert!(head.starts_with("HTTP/1.1 503"), "{head}");
    assert!(head.contains("\"unavailable\""), "{head}");
    assert!(started.elapsed() >= std::time::Duration::from_millis(900));
    t.stop().await;
}

/// One client address cannot hold every connection slot: past
/// `server.max_connections_per_ip` its new connections are closed at once, while another
/// address is still served, and a closed connection gives its slot back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connections_are_capped_per_client_address() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let t = TestServer::start_with(|s| s.file.server.max_connections_per_ip = 4).await;
    let addr = t.srv().addr;
    let port = addr.port();
    let connect_from = |source: &'static str| async move {
        let sock = tokio::net::TcpSocket::new_v4().unwrap();
        sock.bind(format!("{source}:0").parse().unwrap()).unwrap();
        sock.connect(addr).await.unwrap()
    };
    let healthz = |mut s: tokio::net::TcpStream| async move {
        let req =
            format!("GET /healthz HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
        let _ = s.write_all(req.as_bytes()).await;
        let mut buf = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut buf)).await;
        String::from_utf8_lossy(&buf).into_owned()
    };
    // Four idle connections (no request yet) from 127.0.0.1 use its slots.
    let mut idle = Vec::new();
    for _ in 0..4 {
        idle.push(connect_from("127.0.0.1").await);
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    // The fifth is closed without an answer.
    loop {
        let raw = healthz(connect_from("127.0.0.1").await).await;
        if raw.is_empty() {
            break;
        }
        // The server may not have counted every idle connection yet.
        assert!(
            tokio::time::Instant::now() < deadline,
            "never refused: {raw}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // Another address is served.
    let raw = healthz(connect_from("127.0.0.2").await).await;
    assert!(raw.starts_with("HTTP/1.1 200"), "{raw}");
    // Closing one idle connection frees a slot for 127.0.0.1.
    drop(idle.pop());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let raw = healthz(connect_from("127.0.0.1").await).await;
        if raw.starts_with("HTTP/1.1 200") {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the slot was never released"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let m = t.srv().state.metrics.encode();
    assert!(
        m.contains("h2w_connections_refused_total{reason=\"per_client\"}"),
        "{m}"
    );
    drop(idle);
    t.stop().await;
}
