//! `h2workbench egress-proxy` over real sockets: CONNECT tunnels, absolute-form forwarding,
//! the allowlist, the private-address policy, timeouts and the connection cap.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use h2w_daemon::config::AllowRule;
use h2w_daemon::egress::{ProxySettings, serve};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Listeners on 127.0.0.1 and ::1 with the same port (`localhost` may resolve to either).
async fn loopback_pair() -> (TcpListener, Option<TcpListener>, u16) {
    for _ in 0..50 {
        let v4 = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = v4.local_addr().unwrap().port();
        match TcpListener::bind(format!("[::1]:{port}")).await {
            Ok(v6) => return (v4, Some(v6), port),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => continue,
            // No IPv6 loopback on this host.
            Err(_) => return (v4, None, port),
        }
    }
    panic!("no free port pair");
}

fn spawn_accept<F, Fut>(l: TcpListener, f: F)
where
    F: Fn(TcpStream) -> Fut + Send + Sync + 'static + Clone,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        while let Ok((s, _)) = l.accept().await {
            tokio::spawn(f.clone()(s));
        }
    });
}

async fn echo_upstream() -> u16 {
    let (v4, v6, port) = loopback_pair().await;
    let echo = |mut s: TcpStream| async move {
        let mut buf = [0u8; 1024];
        while let Ok(n) = s.read(&mut buf).await {
            if n == 0 || s.write_all(&buf[..n]).await.is_err() {
                break;
            }
        }
    };
    spawn_accept(v4, echo);
    if let Some(v6) = v6 {
        spawn_accept(v6, echo);
    }
    port
}

/// An HTTP upstream that records request heads and answers `hello`.
async fn http_upstream() -> (u16, Arc<Mutex<Vec<String>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (v4, v6, port) = loopback_pair().await;
    let handler = {
        let seen = Arc::clone(&seen);
        move |mut s: TcpStream| {
            let seen = Arc::clone(&seen);
            async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 1024];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                seen.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buf).into_owned());
                let _ = s
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
                    )
                    .await;
            }
        }
    };
    spawn_accept(v4, handler.clone());
    if let Some(v6) = v6 {
        spawn_accept(v6, handler);
    }
    (port, seen)
}

struct Proxy {
    addr: SocketAddr,
    _stop: tokio::sync::watch::Sender<bool>,
}

async fn proxy(rules: Vec<AllowRule>, tweak: impl FnOnce(&mut ProxySettings)) -> Proxy {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let mut s = ProxySettings::new(rules);
    tweak(&mut s);
    let (tx, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(serve(l, s, rx));
    Proxy { addr, _stop: tx }
}

/// A rule for `host` on exactly `port` (the upstreams listen on random ports).
fn rule(host: &str, allow_private: bool, port: u16) -> AllowRule {
    AllowRule {
        host: host.into(),
        wildcard: false,
        allow_private,
        ports: vec![port],
    }
}

async fn read_head(s: &mut TcpStream) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        let n = tokio::time::timeout(Duration::from_secs(10), s.read(&mut chunk))
            .await
            .expect("no response head")
            .unwrap();
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    String::from_utf8_lossy(&buf).into_owned()
}

async fn request(addr: SocketAddr, raw: &str) -> String {
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(raw.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut out)).await;
    String::from_utf8_lossy(&out).into_owned()
}

#[tokio::test]
async fn connect_tunnels_to_an_allowlisted_private_host() {
    let port = echo_upstream().await;
    let p = proxy(vec![rule("localhost", true, port)], |_| {}).await;
    let mut s = TcpStream::connect(p.addr).await.unwrap();
    s.write_all(
        format!("CONNECT localhost:{port} HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n").as_bytes(),
    )
    .await
    .unwrap();
    let head = read_head(&mut s).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    s.write_all(b"ping through the tunnel").await.unwrap();
    let mut buf = [0u8; 64];
    let n = tokio::time::timeout(Duration::from_secs(5), s.read(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buf[..n], b"ping through the tunnel");
}

#[tokio::test]
async fn loopback_is_refused_without_allow_private() {
    let port = echo_upstream().await;
    let p = proxy(vec![rule("localhost", false, port)], |_| {}).await;
    let resp = request(
        p.addr,
        &format!("CONNECT localhost:{port} HTTP/1.1\r\nHost: x\r\n\r\n"),
    )
    .await;
    assert!(resp.starts_with("HTTP/1.1 403"), "{resp}");
    assert!(resp.contains("loopback"), "{resp}");
}

#[tokio::test]
async fn ports_outside_the_rule_are_refused() {
    let port = echo_upstream().await;
    // The host is allowlisted, but on another port.
    let other = if port == u16::MAX { port - 1 } else { port + 1 };
    let p = proxy(vec![rule("localhost", true, other)], |_| {}).await;
    let resp = request(
        p.addr,
        &format!("CONNECT localhost:{port} HTTP/1.1\r\nHost: x\r\n\r\n"),
    )
    .await;
    assert!(resp.starts_with("HTTP/1.1 403"), "{resp}");
    assert!(resp.contains("port not allowed"), "{resp}");
}

#[tokio::test]
async fn ip_literals_and_unlisted_hosts_are_refused() {
    let port = echo_upstream().await;
    let p = proxy(vec![rule("localhost", true, port)], |_| {}).await;
    for target in [
        format!("127.0.0.1:{port}"),
        format!("[::1]:{port}"),
        "169.254.169.254:80".to_owned(),
        "metadata.google.internal:80".to_owned(),
        "example.com:443".to_owned(),
    ] {
        let resp = request(
            p.addr,
            &format!("CONNECT {target} HTTP/1.1\r\nHost: x\r\n\r\n"),
        )
        .await;
        assert!(resp.starts_with("HTTP/1.1 403"), "{target}: {resp}");
    }
    // Absolute-form HTTP to an IP literal is refused as well.
    let resp = request(
        p.addr,
        &format!("GET http://127.0.0.1:{port}/ HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
    )
    .await;
    assert!(resp.starts_with("HTTP/1.1 403"), "{resp}");
}

#[tokio::test]
async fn absolute_form_http_is_forwarded_in_origin_form() {
    let (port, seen) = http_upstream().await;
    let p = proxy(vec![rule("localhost", true, port)], |_| {}).await;
    let resp = request(
        p.addr,
        &format!(
            "GET http://localhost:{port}/hello?x=1 HTTP/1.1\r\nHost: localhost:{port}\r\nProxy-Authorization: Basic c2VjcmV0\r\nProxy-Connection: keep-alive\r\nX-Kept: yes\r\nConnection: close\r\n\r\n"
        ),
    )
    .await;
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    assert!(resp.ends_with("hello"), "{resp}");
    let heads = seen.lock().unwrap().clone();
    assert_eq!(heads.len(), 1);
    let h = heads[0].to_ascii_lowercase();
    assert!(h.starts_with("get /hello?x=1 http/1.1"), "{h}");
    assert!(h.contains(&format!("host: localhost:{port}")), "{h}");
    assert!(h.contains("x-kept: yes"), "{h}");
    assert!(
        !h.contains("proxy-authorization") && !h.contains("proxy-connection"),
        "{h}"
    );
}

#[tokio::test]
async fn only_proxy_requests_are_accepted() {
    let p = proxy(vec![rule("localhost", true, 1)], |_| {}).await;
    let resp = request(p.addr, "GET / HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(resp.starts_with("HTTP/1.1 400"), "{resp}");
    let resp = request(
        p.addr,
        "GET https://localhost:1/ HTTP/1.1\r\nHost: localhost:1\r\n\r\n",
    )
    .await;
    assert!(resp.starts_with("HTTP/1.1 400"), "{resp}");
}

#[tokio::test]
async fn idle_tunnels_are_closed() {
    let port = echo_upstream().await;
    let p = proxy(vec![rule("localhost", true, port)], |s| {
        s.idle_timeout = Duration::from_millis(400);
    })
    .await;
    let mut s = TcpStream::connect(p.addr).await.unwrap();
    s.write_all(format!("CONNECT localhost:{port} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())
        .await
        .unwrap();
    assert!(read_head(&mut s).await.starts_with("HTTP/1.1 200"));
    let started = std::time::Instant::now();
    let mut buf = [0u8; 8];
    let n = tokio::time::timeout(Duration::from_secs(10), s.read(&mut buf))
        .await
        .expect("the idle tunnel was never closed")
        .unwrap_or(0);
    assert_eq!(n, 0);
    assert!(started.elapsed() >= Duration::from_millis(300));
}

#[tokio::test]
async fn connections_beyond_the_cap_are_closed() {
    let p = proxy(vec![rule("localhost", true, 1)], |s| s.max_connections = 1).await;
    let _held = TcpStream::connect(p.addr).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let mut second = TcpStream::connect(p.addr).await.unwrap();
    let _ = second
        .write_all(b"CONNECT localhost:1 HTTP/1.1\r\nHost: x\r\n\r\n")
        .await;
    let mut buf = Vec::new();
    let n = tokio::time::timeout(Duration::from_secs(5), second.read_to_end(&mut buf))
        .await
        .expect("the extra connection was not closed");
    assert!(
        n.is_err() || buf.is_empty(),
        "the extra connection was served: {buf:?}"
    );
}
