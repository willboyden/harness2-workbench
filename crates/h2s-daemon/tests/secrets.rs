//! Secret values never leave the server: not in any response, not in the server's own logs,
//! not in job logs; files holding state are private.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex, OnceLock};

use common::*;
use h2s_api::{Job, SecretInfo};
use serde_json::json;

#[derive(Clone)]
struct Sink(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Sink {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Capture every log line the server writes (debug level) in this test process.
fn server_logs() -> Arc<Mutex<Vec<u8>>> {
    static LOGS: OnceLock<Arc<Mutex<Vec<u8>>>> = OnceLock::new();
    LOGS.get_or_init(|| {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let sink = Sink(Arc::clone(&buf));
        tracing_subscriber::fmt()
            .with_env_filter("debug")
            .with_ansi(false)
            .with_writer(move || sink.clone())
            .init();
        buf
    })
    .clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn planted_secrets_never_appear_in_responses_or_logs() {
    let logs = server_logs();
    let env_value = format!("envsecret-{}", random_text(24));
    let ev = env_value.clone();
    let t = TestServer::start_with(move |s| {
        s.secret_env = vec![("H2S_SECRET_ENV_KEY".into(), ev)];
    })
    .await;
    let db_value = format!("dbsecret-{}", random_text(24));
    t.put_secret("DB_KEY", &db_value).await;
    t.put_profile(json!({
        "name": "psec", "kind": "openai_compatible", "solver_model": "glm",
        "endpoint": {"base_url": "http://127.0.0.1:9/v1", "api_key_secret": "DB_KEY"},
        "judge_endpoint": {"base_url": "http://127.0.0.1:9/v1", "api_key_secret": "ENV_KEY"},
        "extra_env": [{"name": "X_TOKEN", "secret": "DB_KEY"}]
    }))
    .await;
    t.put_experiment("ok-sec", None).await;

    let mut seen = String::new();
    let grab = |label: &str, r: reqwest::Response| {
        let status = r.status();
        let headers = format!("{:?}", r.headers());
        (label.to_owned(), status, headers, r)
    };
    let mut responses = Vec::new();
    responses.push(grab(
        "secrets",
        t.get("/api/v1/secrets", Some(&t.admin))
            .send()
            .await
            .unwrap(),
    ));
    responses.push(grab(
        "profiles",
        t.get("/api/v1/profiles", Some(&t.viewer))
            .send()
            .await
            .unwrap(),
    ));
    responses.push(grab(
        "info",
        t.get("/api/v1/system/info", Some(&t.admin))
            .send()
            .await
            .unwrap(),
    ));
    responses.push(grab(
        "tokens",
        t.get("/api/v1/tokens", Some(&t.admin))
            .send()
            .await
            .unwrap(),
    ));
    responses.push(grab(
        "openapi",
        t.get("/api/v1/openapi.json", Some(&t.viewer))
            .send()
            .await
            .unwrap(),
    ));
    responses.push(grab(
        "metrics",
        t.get("/metrics", Some(&t.viewer)).send().await.unwrap(),
    ));
    let run = t
        .create_job(&t.operator, run_spec("ok-sec", "psec", None))
        .await;
    assert_eq!(run.status(), 201);
    let run: Job = run.json().await.unwrap();
    let demo: Job = t
        .create_job(&t.operator, demo_spec("ok-sec-demo"))
        .await
        .json()
        .await
        .unwrap();
    for id in [&run.id, &demo.id] {
        t.wait_terminal(id, 30).await;
        responses.push(grab(
            "job",
            t.get(&format!("/api/v1/jobs/{id}"), Some(&t.viewer))
                .send()
                .await
                .unwrap(),
        ));
        responses.push(grab(
            "log",
            t.get(
                &format!("/api/v1/jobs/{id}/log?limit=5000"),
                Some(&t.viewer),
            )
            .send()
            .await
            .unwrap(),
        ));
        responses.push(grab(
            "events",
            t.get(&format!("/api/v1/jobs/{id}/events"), Some(&t.viewer))
                .send()
                .await
                .unwrap(),
        ));
    }
    responses.push(grab(
        "jobs",
        t.get("/api/v1/jobs", Some(&t.viewer)).send().await.unwrap(),
    ));
    responses.push(grab(
        "audit",
        t.get("/api/v1/audit?limit=500", Some(&t.admin))
            .send()
            .await
            .unwrap(),
    ));
    responses.push(grab(
        "bad login",
        t.post("/api/v1/auth/login", None)
            .json(&json!({"token": db_value}))
            .send()
            .await
            .unwrap(),
    ));

    for (label, status, headers, r) in responses {
        let body = r.text().await.unwrap();
        seen.push_str(&body);
        for v in [&db_value, &env_value] {
            assert!(
                !body.contains(v.as_str()),
                "{label} ({status}) echoed a secret: {body}"
            );
            assert!(
                !headers.contains(v.as_str()),
                "{label} headers echoed a secret"
            );
        }
    }
    // Metadata is there, values are not.
    let infos: Vec<SecretInfo> = serde_json::from_str(
        &t.get("/api/v1/secrets", Some(&t.admin))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
    )
    .unwrap();
    let db = infos.iter().find(|i| i.name == "DB_KEY").unwrap();
    assert!(db.set && db.chars == Some(u32::try_from(db_value.len()).unwrap()));
    let env = infos.iter().find(|i| i.name == "ENV_KEY").unwrap();
    assert!(env.set && env.updated_at.is_none());

    // The server's own logs: no secret values and no bearer tokens.
    let text = String::from_utf8_lossy(&logs.lock().unwrap()).into_owned();
    assert!(text.contains("request"), "log capture is not working");
    for v in [&db_value, &env_value, &t.admin, &t.operator, &t.viewer] {
        assert!(
            !text.contains(v.as_str()),
            "a secret or token reached the server log"
        );
    }
    // Job log files on disk.
    let logs_dir = t.root.path().join("data/logs/jobs");
    for e in std::fs::read_dir(&logs_dir).unwrap() {
        let p = e.unwrap().path();
        let raw = std::fs::read_to_string(&p).unwrap();
        assert!(!raw.contains(&db_value) && !raw.contains(&env_value));
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{}", p.display());
    }
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn state_files_and_directories_are_private() {
    let t = TestServer::start().await;
    let data = t.root.path().join("data");
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&data), 0o700);
    for d in ["logs/jobs", "jobs", "splits", "experiments"] {
        assert_eq!(mode(&data.join(d)), 0o700, "{d}");
    }
    assert_eq!(mode(&data.join("studio.db")), 0o600);
    for suffix in ["studio.db-wal", "studio.db-shm"] {
        let p = data.join(suffix);
        if p.exists() {
            assert_eq!(mode(&p), 0o600, "{suffix}");
        }
    }
    t.stop().await;
}
