//! `GET /api/v1/profiles/{name}/check`: the warnings and missing secrets of a profile, as a
//! viewer sees them before starting a job (contract addendum v1.2).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::*;
use h2w_api::{ErrorBody, ProfileCheck};
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
    let db = h2w_daemon::db::open_connection(&t.root.path().join("data/workbench.db")).unwrap();
    let mut p = h2w_daemon::db::profiles::get(&db, "old").unwrap().unwrap();
    p.extra_env.push(h2w_api::EnvVar {
        name: "GIT_SSH_COMMAND".into(),
        value: Some("ssh -o ProxyCommand=evil".into()),
        secret: None,
    });
    h2w_daemon::db::profiles::put(&db, &p, 0).unwrap();
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

/// `?bench=` and `?substrate=` keep the warnings about those jobs (contract v1.12): what the run
/// wizard shows for the benchmark and executor it is about to use.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_scoped_check_keeps_the_warnings_about_those_jobs() {
    let t = TestServer::start().await;
    t.put_profile(json!({"name": "local", "kind": "openai_compatible",
        "solver_model": "glm", "judge_model": "glm-judge",
        "endpoint": {"base_url": "http://127.0.0.1:9/v1"}}))
        .await;
    let check = |query: &str| {
        let req = t.get(
            &format!("/api/v1/profiles/local/check{query}"),
            Some(&t.viewer),
        );
        async move { req.send().await.unwrap() }
    };
    let warnings = |query: &str| {
        let r = check(query);
        async move {
            let r = r.await;
            assert_eq!(r.status(), 200);
            r.json::<ProfileCheck>().await.unwrap().warnings
        }
    };
    let has = |w: &[String], needle: &str| w.iter().any(|x| x.contains(needle));
    let all = warnings("").await;
    assert!(
        has(&all, "Codex executor") && has(&all, "WorkBuddy-Bench"),
        "{all:?}"
    );

    let lab_hm = warnings("?bench=lab&substrate=hm").await;
    assert!(has(&lab_hm, "LAB's judge does not recognise"), "{lab_hm:?}");
    assert!(!has(&lab_hm, "Codex executor") && !has(&lab_hm, "WorkBuddy-Bench"));
    assert!(!has(&lab_hm, "JobBench"), "{lab_hm:?}");
    assert!(lab_hm.iter().all(|w| all.contains(w)));

    assert!(has(
        &warnings("?bench=lab&substrate=cx").await,
        "Codex executor"
    ));
    assert!(has(&warnings("?bench=wb").await, "WorkBuddy-Bench"));

    for (query, field) in [("?bench=xx", "bench"), ("?substrate=zz", "substrate")] {
        let r = check(query).await;
        assert_eq!(r.status(), 422, "{query}");
        let e: ErrorBody = r.json().await.unwrap();
        assert!(e.error.details.iter().any(|d| d.field == field), "{e:?}");
    }
    assert_eq!(check("?other=1").await.status(), 422);
    t.stop().await;
}
