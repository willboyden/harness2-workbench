//! The configuration editor's two routes: `GET /system/config` sends the configuration file
//! to administrators only when it holds nothing that looks like a credential, and
//! `POST /system/config/validate` checks a text with the server's own loader without writing
//! anything.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::*;
use h2w_api::{ConfigValidation, ServerConfigFile};
use reqwest::StatusCode;
use serde_json::json;

/// A server whose configuration "was loaded" from `<root>/h2workbench.toml`, holding the text
/// the test server runs with plus `extra` (a comment, say).
async fn with_file(extra: &str) -> (TestServer, std::path::PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("h2workbench.toml");
    let mut setup = Setup::new(root.path());
    let text = format!("{}{extra}", toml::to_string(&setup.file).unwrap());
    std::fs::write(&path, text).unwrap();
    setup.config_source = Some(path.clone());
    (TestServer::start_in(root, setup).await, path)
}

async fn read(t: &TestServer, token: &str) -> (StatusCode, Option<ServerConfigFile>) {
    let r = t
        .get("/api/v1/system/config", Some(token))
        .send()
        .await
        .unwrap();
    let status = r.status();
    (
        status,
        if status.is_success() {
            r.json().await.ok()
        } else {
            None
        },
    )
}

#[tokio::test]
async fn admins_read_the_file_and_others_cannot() {
    let (t, path) = with_file("# edited by hand\n").await;
    let (status, file) = read(&t, &t.admin).await;
    assert_eq!(status, StatusCode::OK);
    let file = file.unwrap();
    assert_eq!(
        file.path.as_deref(),
        Some(path.display().to_string().as_str())
    );
    assert!(file.text.unwrap().contains("# edited by hand"));
    assert!(file.refused.is_none());
    assert!(!file.changed_since_start);
    assert_eq!(file.max_bytes, 256 * 1024);
    for token in [&t.operator, &t.viewer] {
        assert_eq!(read(&t, token).await.0, StatusCode::FORBIDDEN);
    }
    // The read is audited.
    let audit: serde_json::Value = t
        .get("/api/v1/audit", Some(&t.admin))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(audit.to_string().contains("config.read"), "{audit}");
}

#[tokio::test]
async fn a_file_with_a_stored_secret_value_is_refused() {
    // Plain words, so only the stored-secret check (not a shape rule) can refuse the file.
    let words = ["correct", "horse", "battery", "staple"].join("-");
    let (t, _) = with_file("").await;
    t.put_secret("GATEWAY_KEY", &words).await;
    let path = t.root.path().join("h2workbench.toml");
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str(&format!("# old {words}\n"));
    std::fs::write(&path, text).unwrap();
    let (status, file) = read(&t, &t.admin).await;
    assert_eq!(status, StatusCode::OK);
    let file = file.unwrap();
    assert!(file.text.is_none());
    let why = file.refused.unwrap();
    assert!(why.contains("stored secret"), "{why}");
    assert!(!why.contains("horse"), "{why}");
}

/// The shape allowlist is applied first: a file it withholds gives its reason whether or not
/// it also holds a stored secret, so the answer says nothing about the stored secrets.
#[tokio::test]
async fn the_shape_reason_comes_before_the_stored_secret_check() {
    let words = ["correct", "horse", "battery", "staple"].join("-");
    let (t, _) = with_file(&format!("[extra]\nnote = \"{words}\"\n")).await;
    let before = read(&t, &t.admin).await.1.unwrap().refused.unwrap();
    assert!(before.contains("unknown key"), "{before}");
    t.put_secret("GATEWAY_KEY", &words).await;
    let after = read(&t, &t.admin).await.1.unwrap().refused.unwrap();
    assert_eq!(before, after);
}

#[tokio::test]
async fn a_short_stored_secret_does_not_block_the_file() {
    let (t, _) = with_file("# listen on loopback\n").await;
    // "on" and "loo" occur in the file; values under 8 bytes are not matched.
    t.put_secret("WEAK_ONE", "on").await;
    t.put_secret("WEAK_TWO", "loo").await;
    let file = read(&t, &t.admin).await.1.unwrap();
    assert!(file.refused.is_none(), "{:?}", file.refused);
    assert!(file.text.unwrap().contains("listen on loopback"));
}

#[tokio::test]
async fn a_file_with_credentials_in_a_url_is_refused_and_edits_are_noticed() {
    let (t, path) = with_file("").await;
    let mut text = std::fs::read_to_string(&path).unwrap();
    text = text.replace(
        "otlp_endpoint = \"\"",
        "otlp_endpoint = \"https://user:pw@otel.example.com\"",
    );
    std::fs::write(&path, &text).unwrap();
    let file = read(&t, &t.admin).await.1.unwrap();
    assert!(file.text.is_none());
    assert!(file.refused.unwrap().contains("URL"));
    assert!(file.changed_since_start);
}

#[tokio::test]
async fn only_known_settings_without_secret_shapes_are_sent() {
    for (extra, words) in [
        ("[extra]\nnote = \"x\"\n", "unknown key"),
        ("# db-password = hunter2\n", "looks like a password"),
        ("# AbCdEf0123456789XyZ0123\n", "token-like"),
    ] {
        let (t, _) = with_file(extra).await;
        let file = read(&t, &t.admin).await.1.unwrap();
        assert!(file.text.is_none(), "{extra}");
        let why = file.refused.unwrap();
        assert!(why.contains(words), "{extra}: {why}");
        assert!(!why.contains("hunter2") && !why.contains("AbCdEf"), "{why}");
    }
}

#[tokio::test]
async fn without_a_file_there_is_nothing_to_read() {
    let t = TestServer::start().await;
    let file = read(&t, &t.admin).await.1.unwrap();
    assert!(file.path.is_none() && file.text.is_none() && file.refused.is_none());
}

#[tokio::test]
async fn validate_runs_the_real_checks_and_writes_nothing() {
    let (t, path) = with_file("").await;
    let before = std::fs::read(&path).unwrap();
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    let check = |text: &str| {
        t.post("/api/v1/system/config/validate", Some(&t.admin))
            .json(&json!({ "text": text }))
            .send()
    };
    let ok: ConfigValidation = check("[jobs]\nmax_concurrent = 4\n")
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(ok.valid, "{ok:?}");
    let bad: ConfigValidation = check("[jobs]\nmax_concurrent = 0\n[auth]\nmode = \"proxy\"\n")
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!bad.valid);
    assert_eq!(bad.problems.len(), 2, "{bad:?}");
    let typo: ConfigValidation = check("[server]\nlistne = 1\n")
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(typo.parse_error.unwrap().line, Some(2));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        modified
    );
    // Too large: refused before parsing.
    let r = check(&"#".repeat(256 * 1024 + 1)).await.unwrap();
    assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
    // Unknown body fields, and no CSRF header: refused.
    let r = t
        .post("/api/v1/system/config/validate", Some(&t.admin))
        .json(&json!({ "text": "", "write": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    let r = t
        .client
        .post(t.url("/api/v1/system/config/validate"))
        .bearer_auth(&t.admin)
        .json(&json!({ "text": "" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    // Operators may not validate either.
    let r = t
        .post("/api/v1/system/config/validate", Some(&t.operator))
        .json(&json!({ "text": "" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn validation_never_echoes_a_value_and_cannot_confirm_a_secret() {
    // Built at run time so the source holds no key-shaped literal.
    let stored = format!("gk-{}{}", "0123456789", "abcdefghij");
    let guess = format!("gk-{}{}", "9876543210", "jihgfedcba");
    let t = TestServer::start().await;
    t.put_secret("GATEWAY_KEY", &stored).await;
    let check = |value: &str| {
        t.post("/api/v1/system/config/validate", Some(&t.admin))
            .json(&json!({ "text": format!("[server]\nlisten = \"{value}\"\n") }))
            .send()
    };
    let secret: ConfigValidation = check(&stored).await.unwrap().json().await.unwrap();
    let other: ConfigValidation = check(&guess).await.unwrap().json().await.unwrap();
    assert!(!secret.valid);
    let all = format!("{secret:?}");
    // Fixed messages: the answer is not printed, so a failure never logs the test secret.
    assert!(!all.contains(&stored), "the answer quotes the posted value");
    assert!(
        !all.contains("REDACTED"),
        "the answer shows a redaction marker"
    );
    assert!(
        all.contains("server.listen"),
        "the answer does not name the key"
    );
    // The stored secret's value and any other value get the same answer.
    assert_eq!(secret, other);
}
