//! OTLP/HTTP trace export (feature `otel`): request spans reach a collector.

#![cfg(feature = "otel")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A tiny collector: records `(request line, content-type, body length)` of each POST.
async fn collector() -> (u16, Arc<Mutex<Vec<(String, String, usize)>>>) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s2 = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            let seen = Arc::clone(&s2);
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                let head_end = loop {
                    match s.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i + 4;
                    }
                };
                let head = String::from_utf8_lossy(&buf[..head_end]).to_ascii_lowercase();
                let len: usize = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(0);
                while buf.len() < head_end + len {
                    match s.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let ct = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-type:"))
                    .unwrap_or("")
                    .trim()
                    .to_owned();
                let line = head.lines().next().unwrap_or("").to_owned();
                seen.lock().unwrap().push((line, ct, len));
                let _ = s
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                    .await;
            });
        }
    });
    (port, seen)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn request_spans_are_exported_over_otlp_http() {
    let (port, seen) = collector().await;
    let root = tempfile::tempdir().unwrap();
    let mut setup = Setup::new(root.path());
    setup.file.telemetry.otlp_endpoint = format!("http://127.0.0.1:{port}");
    setup.file.telemetry.log_level = "info".into();
    let cfg = h2w_daemon::config::Config::from_file(setup.file.clone(), None, &|_| None).unwrap();
    assert_eq!(
        cfg.otlp_traces_url.as_deref(),
        Some(format!("http://127.0.0.1:{port}/v1/traces").as_str())
    );
    let guard = h2w_daemon::telemetry::init(&cfg).unwrap();
    let t = TestServer::start_in(root, setup).await;
    for _ in 0..3 {
        t.client.get(t.url("/api/v1/auth/me")).send().await.unwrap();
    }
    t.stop().await;
    // Shutting the provider down flushes the batch.
    tokio::task::spawn_blocking(move || drop(guard))
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let got = seen.lock().unwrap().clone();
        if let Some((line, ct, len)) = got.first() {
            assert!(line.starts_with("post /v1/traces "), "{line}");
            assert_eq!(ct, "application/x-protobuf");
            assert!(*len > 0);
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no spans were exported"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
