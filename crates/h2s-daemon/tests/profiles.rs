//! `GET /api/v1/profiles/{name}/check`: the warnings and missing secrets of a profile, as a
//! viewer sees them before starting a job (contract addendum v1.2).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::*;
use h2s_api::{ErrorBody, ProfileCheck};
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn profile_check_reports_warnings_and_missing_secrets() {
    let t = TestServer::start().await;
    // An OpenAI-compatible profile whose key is not set yet, and without a judge model.
    t.put_profile(
        json!({"name": "local", "kind": "openai_compatible", "solver_model": "glm",
        "endpoint": {"base_url": "http://127.0.0.1:9/v1", "api_key_secret": "LOCAL_KEY"}}),
    )
    .await;
    let check = |token: String| {
        let req = t.get("/api/v1/profiles/local/check", Some(&token));
        async move { req.send().await.unwrap() }
    };
    let r = check(t.viewer.clone()).await;
    assert_eq!(r.status(), 200);
    let c: ProfileCheck = r.json().await.unwrap();
    assert_eq!(c.missing_secrets, vec!["LOCAL_KEY".to_owned()]);
    assert!(
        c.warnings.iter().any(|w| w.contains("judge")),
        "a profile without a judge is warned about: {:?}",
        c.warnings
    );

    // Once the secret is set it is no longer missing; the value never appears.
    t.put_secret("LOCAL_KEY", "sk-local-0123456789abcdef").await;
    let r = check(t.viewer.clone()).await;
    let text = r.text().await.unwrap();
    assert!(!text.contains("sk-local-0123456789abcdef"));
    let c: ProfileCheck = serde_json::from_str(&text).unwrap();
    assert!(c.missing_secrets.is_empty(), "{c:?}");

    // Unknown and malformed names.
    let r = t
        .get("/api/v1/profiles/nope/check", Some(&t.viewer))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    r.json::<ErrorBody>().await.unwrap();
    let r = t
        .get("/api/v1/profiles/Bad%20Name/check", Some(&t.viewer))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 422);
    t.stop().await;
}

/// A profile stored before a rule tightened (here: an environment name reserved later) is
/// refused when a job is created (422), not accepted and failed while the job prepares.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stored_profile_that_breaks_current_rules_is_refused_at_job_creation() {
    let t = TestServer::start().await;
    t.put_profile(json!({"name": "old", "kind": "custom", "solver_model": "m/x"}))
        .await;
    // Rewrite it as an earlier release could have stored it.
    let db = h2s_daemon::db::open_connection(&t.root.path().join("data/studio.db")).unwrap();
    let mut p = h2s_daemon::db::profiles::get(&db, "old").unwrap().unwrap();
    p.extra_env.push(h2s_api::EnvVar {
        name: "GIT_SSH_COMMAND".into(),
        value: Some("ssh -o ProxyCommand=evil".into()),
        secret: None,
    });
    h2s_daemon::db::profiles::put(&db, &p, 0).unwrap();
    drop(db);
    t.put_experiment("e-old", None).await;
    let r = t
        .create_job(&t.operator, run_spec("e-old", "old", None))
        .await;
    assert_eq!(r.status(), 422);
    let e: ErrorBody = r.json().await.unwrap();
    let problem = &e.error.details[0];
    assert_eq!(problem.field, "params.profile");
    assert!(
        problem.problem.contains("no longer passes validation") && problem.problem.contains("GIT_"),
        "{}",
        problem.problem
    );
    t.stop().await;
}
