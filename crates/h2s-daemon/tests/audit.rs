//! The audit log: what is recorded, the hash chain, tamper detection by the API and by
//! `h2studio audit verify`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::*;
use h2s_api::{AuditEvent, AuditOutcome, AuditVerification, Job, JobStatus, Page, Role};
use serde_json::json;

async fn audit_page(t: &TestServer, query: &str) -> Page<AuditEvent> {
    t.get(&format!("/api/v1/audit{query}"), Some(&t.admin))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn verify(t: &TestServer) -> AuditVerification {
    t.get("/api/v1/audit/verify", Some(&t.admin))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn security_relevant_actions_form_a_verifiable_chain() {
    let t = TestServer::start().await;
    // Failed and successful logins.
    t.post("/api/v1/auth/login", None)
        .json(&json!({"token": "h2s_nope"}))
        .send()
        .await
        .unwrap();
    t.post("/api/v1/auth/login", None)
        .json(&json!({"token": t.viewer}))
        .send()
        .await
        .unwrap();
    // Job create (and the demo experiment it creates), cancel.
    let job: Job = t
        .create_job(&t.operator, demo_spec("sleep-aud"))
        .await
        .json()
        .await
        .unwrap();
    t.wait_job(&job.id, 30, |j| j.status == JobStatus::Running)
        .await;
    t.post(
        &format!("/api/v1/jobs/{}/cancel", job.id),
        Some(&t.operator),
    )
    .send()
    .await
    .unwrap();
    t.wait_terminal(&job.id, 30).await;
    // A refused job creation, an access denial, a token revocation.
    t.create_job(
        &t.operator,
        json!({"kind": "install_harness2", "params": {}}),
    )
    .await;
    t.get("/api/v1/audit", Some(&t.viewer))
        .send()
        .await
        .unwrap();
    let extra = t.token("extra", Role::Viewer, None).await;
    t.send(
        reqwest::Method::DELETE,
        &format!("/api/v1/tokens/{}", &extra[4..12]),
        Some(&t.admin),
    )
    .send()
    .await
    .unwrap();

    let page = audit_page(&t, "?limit=500").await;
    let has = |action: &str, outcome: AuditOutcome| {
        page.items
            .iter()
            .any(|e| e.action == action && e.outcome == outcome)
    };
    assert!(has("auth.login", AuditOutcome::Denied));
    assert!(has("auth.login", AuditOutcome::Success));
    assert!(has("job.create", AuditOutcome::Success));
    assert!(has("job.create", AuditOutcome::Denied));
    assert!(has("job.cancel", AuditOutcome::Success));
    assert!(has("experiment.create", AuditOutcome::Success));
    assert!(has("access.denied", AuditOutcome::Denied));
    assert!(has("token.revoke", AuditOutcome::Success));
    let create = page
        .items
        .iter()
        .find(|e| e.action == "job.create" && e.outcome == AuditOutcome::Success)
        .unwrap();
    assert_eq!(create.actor, "operator-t");
    assert_eq!(create.target, job.id);
    assert_eq!(create.ip.as_deref(), Some("127.0.0.1"));
    assert!(create.request_id.is_some());
    // Newest first, each record linked to the one before it.
    for w in page.items.windows(2) {
        assert_eq!(w[0].seq, w[1].seq + 1);
        assert_eq!(w[0].prev_hash, w[1].hash);
    }
    let v = verify(&t).await;
    assert!(v.ok);
    assert_eq!(v.entries, page.items.len() as u64);
    // Paging.
    let p1 = audit_page(&t, "?limit=2").await;
    assert_eq!(p1.items.len(), 2);
    let p2 = audit_page(&t, &format!("?limit=2&cursor={}", p1.next_cursor.unwrap())).await;
    assert_eq!(p2.items[0].seq, p1.items[1].seq - 1);
    let bad = t
        .get("/api/v1/audit?cursor=!!", Some(&t.admin))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 422);
    t.stop().await;
}

fn cli(config: &std::path::Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_h2studio"))
        .args(args)
        .arg("--config")
        .arg(config)
        .env_remove("H2S_CONFIG")
        .env_remove("H2S_DATA_DIR")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .output()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tampering_is_detected_by_the_api_and_the_cli() {
    let t = TestServer::start().await;
    for _ in 0..3 {
        t.post("/api/v1/auth/login", None)
            .json(&json!({"token": t.viewer}))
            .send()
            .await
            .unwrap();
    }
    assert!(verify(&t).await.ok);
    let root = t.root.path().to_path_buf();
    let config = root.join("h2studio.toml");
    std::fs::write(
        &config,
        format!("[paths]\ndata_dir = \"{}\"\n", root.join("data").display()),
    )
    .unwrap();
    let out = cli(&config, &["audit", "verify"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("audit chain OK"));

    let conn = h2s_daemon::db::open_connection(&root.join("data/studio.db")).unwrap();
    conn.execute("UPDATE audit SET actor = 'mallory' WHERE seq = 2", [])
        .unwrap();
    drop(conn);
    let v = verify(&t).await;
    assert!(!v.ok);
    assert_eq!(v.first_bad_seq, Some(2));
    let out = cli(&config, &["audit", "verify"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("BROKEN: first bad record 2"));
    t.stop().await;
}
