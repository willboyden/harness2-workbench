//! The real `h2studio-mcp` binary over stdio (rmcp's child-process client), and its refusals
//! of unsafe credential setups.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::process::{Command as StdCommand, Stdio};

use common::*;
use rmcp::{ServiceExt, transport::TokioChildProcess};
use serde_json::json;

const BINARY: &str = env!("CARGO_BIN_EXE_h2studio-mcp");

/// Run the binary to completion with only `env` set and stdin closed.
fn run_once(env: &[(&str, &str)], args: &[&str]) -> (bool, String, String) {
    let mut cmd = StdCommand::new(BINARY);
    cmd.env_clear()
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in env {
        cmd.env(key, value);
    }
    let out = cmd.output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[tokio::test]
async fn the_binary_serves_mcp_over_stdio() {
    let studio = Studio::start().await;
    studio.mount_happy(TOKEN, "alice", "operator").await;

    let mut cmd = tokio::process::Command::new(BINARY);
    cmd.env_clear()
        .env("H2S_SERVER_URL", studio.url())
        .env("H2S_TOKEN", TOKEN)
        .env("RUST_LOG", "debug");
    let transport = TokioChildProcess::new(cmd).unwrap();
    let client = TestClient::default().serve(transport).await.unwrap();

    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), 41);
    let info = call(&client, "server_info", json!({})).await;
    assert_eq!(payload(&info)["principal"]["name"], "alice");
    let demo = call(&client, "run_demo", json!({"experiment": EXP})).await;
    assert_eq!(payload(&demo)["status"], "succeeded");
    let refused = call(&client, "get_task", json!({"experiment": EXP})).await;
    assert_eq!(refused.is_error, Some(true));

    client.cancel().await.unwrap();
    studio.assert_well_formed(TOKEN).await;
}

#[test]
fn a_group_readable_token_file_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("mcp.token");
    std::fs::write(&file, format!("{TOKEN}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    let file = file.to_str().unwrap();
    let (ok, stdout, stderr) = run_once(&[("H2S_TOKEN_FILE", file)], &[]);
    assert!(!ok);
    assert!(
        stdout.is_empty(),
        "nothing on the protocol channel: {stdout}"
    );
    assert!(stderr.contains("chmod 600"), "{stderr}");
    assert!(!stderr.contains("s3cret"), "the token leaked: {stderr}");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600)).unwrap();
        // A private file is accepted: the server starts (and stops at once, because stdin is
        // already closed before any `initialize`).
        let (_, stdout, stderr) = run_once(&[("H2S_TOKEN_FILE", file)], &[]);
        assert!(stderr.contains("serving MCP over stdio"), "{stderr}");
        assert!(!stderr.contains("chmod 600"), "{stderr}");
        assert!(stdout.is_empty());
    }
}

#[test]
fn credential_mistakes_are_explained_without_echoing_tokens() {
    let (ok, _, stderr) = run_once(&[], &[]);
    assert!(!ok);
    assert!(stderr.contains("no API token"), "{stderr}");

    let (ok, _, stderr) = run_once(&[("H2S_TOKEN", "not-a-token")], &[]);
    assert!(!ok);
    assert!(
        stderr.contains("not a Harness² Studio API token"),
        "{stderr}"
    );
    assert!(!stderr.contains("not-a-token"), "{stderr}");

    let (ok, _, stderr) = run_once(&[("H2S_TOKEN", TOKEN)], &["--token", TOKEN]);
    assert!(!ok, "--token must not exist");
    assert!(stderr.contains("--token"), "{stderr}");

    let (ok, _, stderr) = run_once(
        &[
            ("H2S_TOKEN", TOKEN),
            ("H2S_SERVER_URL", "http://studio.example.com:8722"),
        ],
        &[],
    );
    assert!(!ok);
    assert!(
        stderr.contains("refusing to send API tokens over plain http"),
        "{stderr}"
    );
    assert!(!stderr.contains("s3cret"), "{stderr}");
}
