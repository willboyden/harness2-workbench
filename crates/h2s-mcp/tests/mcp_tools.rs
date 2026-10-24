//! In-process MCP client <-> server tests against a mock Studio server: every tool's happy
//! path and error path, argument checking, waiting with progress notifications, resources and
//! prompts.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::collections::BTreeSet;

use common::*;
use h2s_api::JobSpec;
use rmcp::{
    model::{
        CallToolRequestParams, ClientRequest, GetPromptRequestParams, ReadResourceRequestParams,
        Request, ResourceContents,
    },
    service::PeerRequestOptions,
};
use serde_json::{Value, json};
use wiremock::{
    Mock, ResponseTemplate,
    matchers::{body_partial_json, method, path},
};

/// Every tool, sorted (tools/list sorts by name).
const EXPECTED_TOOLS: [&str; 42] = [
    "cancel_job",
    "check_benchmark",
    "check_profile",
    "compare_sweeps",
    "create_experiment",
    "create_split",
    "diff_harnesses",
    "doctor",
    "estimate_cost",
    "export_harness",
    "get_experiment",
    "get_harness",
    "get_job",
    "get_job_log",
    "get_library",
    "get_manifest",
    "get_report",
    "get_scores",
    "get_task",
    "install_harness2",
    "list_audit_sessions",
    "list_benchmarks",
    "list_experiments",
    "list_jobs",
    "list_profiles",
    "list_rollout_files",
    "list_splits",
    "list_tasks",
    "preview_run",
    "read_audit_session",
    "read_rollout_file",
    "run_demo",
    "server_info",
    "setup_benchmark",
    "start_bridge",
    "start_judge",
    "start_pipeline",
    "start_report",
    "start_run",
    "update_experiment",
    "validate_harness",
    "wait_for_job",
];

fn cell() -> Value {
    json!({"bench": "lab"})
}

fn options() -> Value {
    json!({"k": 1, "modes": ["parallel"], "max_tasks": 2})
}

fn sweep_args() -> Value {
    json!({"experiment": EXP, "sweep": SWEEP})
}

fn task_args(extra: Value) -> Value {
    let mut args = json!({"experiment": EXP, "sweep": SWEEP, "task": TASK});
    if let (Some(args), Value::Object(extra)) = (args.as_object_mut(), extra) {
        args.extend(extra);
    }
    args
}

/// One call per tool (a few twice, to cover their options), with valid arguments.
fn cases() -> Vec<(&'static str, Value)> {
    vec![
        ("server_info", json!({})),
        ("doctor", json!({})),
        ("doctor", json!({"refresh": true})),
        ("list_benchmarks", json!({})),
        ("setup_benchmark", json!({"bench": "lab", "dry_run": true})),
        ("check_benchmark", json!({"cell": cell()})),
        ("list_profiles", json!({})),
        ("check_profile", json!({"profile": "local"})),
        ("list_experiments", json!({})),
        ("list_experiments", json!({"include_archived": true})),
        (
            "create_experiment",
            json!({"name": "exp1", "profile": "local"}),
        ),
        ("get_experiment", json!({"experiment": EXP})),
        (
            "update_experiment",
            json!({"experiment": EXP, "archived": true}),
        ),
        ("list_splits", json!({})),
        ("get_manifest", json!({"experiment": EXP})),
        (
            "estimate_cost",
            json!({"tasks": 10, "k": 2, "modes": ["parallel"]}),
        ),
        (
            "preview_run",
            json!({"experiment": EXP, "cell": cell(), "options": options()}),
        ),
        (
            "start_run",
            json!({"experiment": "e1", "cell": cell(), "options": options(), "profile": "local",
                   "budget_usd": 5.0, "acknowledge_risks": true}),
        ),
        (
            "start_pipeline",
            json!({"experiment": "e1", "cell": cell(), "options": options(), "profile": "local",
                   "judge_bases": true, "budget_usd": 5.0}),
        ),
        (
            "run_demo",
            json!({"experiment": EXP, "k": 2, "mode": "both"}),
        ),
        (
            "start_judge",
            json!({"experiment": "e1", "cell": cell(), "k": 1, "modes": ["parallel"],
                   "profile": "local"}),
        ),
        (
            "start_report",
            json!({"experiment": "e1", "cell": cell(), "ks": [1], "modes": ["parallel"]}),
        ),
        ("create_split", json!({"bench": "lab", "name": "lab-half"})),
        ("install_harness2", json!({})),
        ("start_bridge", json!({"profile": "vertex"})),
        (
            "list_jobs",
            json!({"status": "running", "kind": "run", "limit": 5}),
        ),
        ("get_job", json!({"job_id": JOB_ID})),
        ("wait_for_job", json!({"job_id": JOB_ID, "timeout_secs": 5})),
        ("cancel_job", json!({"job_id": JOB_ID})),
        ("get_job_log", json!({"job_id": JOB_ID})),
        ("get_job_log", json!({"job_id": JOB_ID, "tail": 1})),
        ("list_tasks", sweep_args()),
        ("get_task", task_args(json!({}))),
        (
            "get_harness",
            task_args(json!({"mode": "parallel", "label": "agg1"})),
        ),
        (
            "diff_harnesses",
            task_args(json!({"mode": "parallel", "to": "agg1"})),
        ),
        (
            "export_harness",
            task_args(json!({"mode": "parallel", "label": "agg1", "target": "opencode"})),
        ),
        (
            "validate_harness",
            json!({"surface": "opencode",
                   "files": [{"path": "systemprompt.md", "content": "You are careful.\n"}]}),
        ),
        (
            "list_rollout_files",
            task_args(json!({"mode": "parallel", "label": "agg1"})),
        ),
        (
            "read_rollout_file",
            task_args(json!({"mode": "parallel", "label": "agg1", "path": "out/report.md"})),
        ),
        ("get_scores", sweep_args()),
        ("get_report", sweep_args()),
        ("get_library", sweep_args()),
        ("list_audit_sessions", sweep_args()),
        (
            "read_audit_session",
            json!({"experiment": EXP, "sweep": SWEEP, "session": SESSION, "part": "response"}),
        ),
        (
            "compare_sweeps",
            json!({"a": {"experiment": EXP, "sweep": SWEEP, "mode": "parallel"},
                   "b": {"experiment": EXP, "sweep": SWEEP, "mode": "sequential"}}),
        ),
    ]
}

/// What each tool's JSON must say (fixtures in `common`).
fn check_payload(tool: &str, args: &Value, p: &Value) {
    let ok = match tool {
        "server_info" => {
            p["principal"]["name"] == "alice"
                && p["system"]["harness2"]["installed"] == true
                && p["mcp_server"]["name"] == "h2studio-mcp"
        }
        "doctor" => p["ok"] == true && p["checks"][1]["remedy"] == "install Docker Engine",
        "list_benchmarks" => p[0]["bench"] == "lab",
        "setup_benchmark" => p["kind"] == "setup" && p["result"]["dry_run"] == true,
        "check_benchmark" => p["result"]["ready"] == true,
        "list_profiles" => p[0]["name"] == "local",
        "check_profile" => p["missing_secrets"].as_array().is_some_and(Vec::is_empty),
        "list_experiments" => p[0]["name"] == EXP,
        "create_experiment" => p["name"] == "exp1",
        "get_experiment" => p["sweeps"][0]["id"] == SWEEP,
        "update_experiment" => p["archived"] == true,
        "list_splits" => p[0]["name"] == "lab-half",
        "get_manifest" => p["experiment"] == EXP,
        "estimate_cost" => p["rollouts"] == 80,
        "preview_run" => p["result"]["summary_namespace"] == "lab-oc-agg1",
        "start_run" => p["kind"] == "run" && p["spec"]["params"]["budget_usd"] == 5.0,
        "start_pipeline" => p["kind"] == "pipeline",
        "run_demo" => p["status"] == "succeeded" && p["result"]["done"] == 4,
        "start_judge" => p["kind"] == "judge",
        "start_report" => p["result"]["text"] == "lab parallel k=1 62.5",
        "create_split" => p["result"]["name"] == "lab-half",
        "install_harness2" => p["kind"] == "install_harness2",
        "start_bridge" => p["kind"] == "bridge",
        "list_jobs" => p["items"][0]["id"] == JOB_ID && p["next_cursor"] == "c2",
        "get_job" | "wait_for_job" => p["id"] == JOB_ID && p["status"] == "succeeded",
        "cancel_job" => p["status"] == "cancelled",
        "get_job_log" => p["lines"][1]["text"] == "[contracts] 1/4 ok t1",
        "list_tasks" => p["items"][0]["slug"] == TASK,
        "get_task" => p["modes"][0]["selection"]["chosen_label"] == "agg1",
        "get_harness" => p["files"][0]["path"] == "systemprompt.md",
        "diff_harnesses" => p["files"][0]["change"] == "modified",
        "export_harness" => p["files"][0]["path"] == "AGENTS.md",
        "validate_harness" => p["ok"] == true,
        "list_rollout_files" => p[1]["path"] == "out/report.md",
        "read_rollout_file" => p["text"] == "# Report\n",
        "get_scores" => p["rows"][0]["frac"] == 0.75,
        "get_report" => p["summaries"][0]["score"] == 62.5 && p["base_score"] == 50.0,
        "get_library" => p[0]["domain"] == "contracts",
        "list_audit_sessions" => p[0]["id"] == SESSION,
        "compare_sweeps" => p["mean_delta"] == -0.25,
        other => panic!("no check for {other}"),
    };
    assert!(ok, "{tool} {args}: unexpected payload {p:#}");
}

/// Paths of properties without a description, through nested objects, options and arrays.
fn undescribed_properties(schema: &Value, at: &str, out: &mut Vec<String>) {
    if let Some(Value::Object(props)) = schema.get("properties") {
        for (name, sub) in props {
            let path = format!("{at}.{name}");
            if sub.get("description").is_none() {
                out.push(path.clone());
            }
            undescribed_properties(sub, &path, out);
        }
    }
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(Value::Array(alternatives)) = schema.get(key) {
            for alternative in alternatives {
                undescribed_properties(alternative, at, out);
            }
        }
    }
    if let Some(items) = schema.get("items") {
        undescribed_properties(items, &format!("{at}[]"), out);
    }
}

#[tokio::test]
async fn initialize_reports_identity_capabilities_and_instructions() {
    let studio = Studio::start().await;
    let client = connect(&studio.url(), TOKEN).await;
    let info = client.peer_info().expect("initialized");
    let server = info.server_info.as_ref().expect("server info");
    assert_eq!(server.name, "h2studio-mcp");
    assert_eq!(server.version, env!("CARGO_PKG_VERSION"));
    assert!(info.capabilities.tools.is_some());
    assert!(info.capabilities.resources.is_some());
    assert!(info.capabilities.prompts.is_some());
    let instructions = info.instructions.as_deref().expect("instructions");
    for word in [
        "preview_run",
        "estimate_cost",
        "wait_for_job",
        "PERCENT",
        "FRACTIONS",
        "--allow-risk-acknowledgement",
    ] {
        assert!(instructions.contains(word), "instructions lack {word}");
    }
    // Nothing is sent to the Studio server just to initialize.
    assert!(studio.received().await.is_empty());
}

#[tokio::test]
async fn tools_list_is_complete_described_and_annotated() {
    let studio = Studio::start().await;
    let client = connect(&studio.url(), TOKEN).await;
    let tools = client.list_all_tools().await.unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert_eq!(names, EXPECTED_TOOLS);

    let read_only: BTreeSet<&str> = [
        "server_info",
        "doctor",
        "list_benchmarks",
        "list_profiles",
        "check_profile",
        "list_experiments",
        "get_experiment",
        "get_manifest",
        "list_splits",
        "estimate_cost",
        "list_jobs",
        "get_job",
        "wait_for_job",
        "get_job_log",
        "list_tasks",
        "get_task",
        "get_harness",
        "diff_harnesses",
        "export_harness",
        "validate_harness",
        "list_rollout_files",
        "read_rollout_file",
        "get_scores",
        "get_report",
        "get_library",
        "list_audit_sessions",
        "read_audit_session",
        "compare_sweeps",
    ]
    .into();
    let open_world: BTreeSet<&str> = [
        "setup_benchmark",
        "start_run",
        "start_pipeline",
        "start_judge",
        "install_harness2",
        "start_bridge",
    ]
    .into();
    let destructive: BTreeSet<&str> =
        ["cancel_job", "install_harness2", "update_experiment"].into();

    for tool in &tools {
        let name = tool.name.as_ref();
        let description = tool.description.as_deref().unwrap_or_default();
        assert!(
            (200..=1024).contains(&description.len()),
            "{name}: description is {} chars",
            description.len()
        );
        assert!(tool.title.is_some(), "{name}: no title");
        let a = tool.annotations.as_ref().expect("annotations");
        assert!(a.title.is_some(), "{name}: no annotation title");
        assert_eq!(
            a.read_only_hint,
            Some(read_only.contains(name)),
            "{name}: readOnlyHint"
        );
        assert_eq!(
            a.open_world_hint,
            Some(open_world.contains(name)),
            "{name}: openWorldHint"
        );
        assert_eq!(
            a.destructive_hint,
            Some(destructive.contains(name)),
            "{name}: destructiveHint"
        );
        assert!(a.idempotent_hint.is_some(), "{name}: idempotentHint");

        let schema = &tool.input_schema;
        assert_eq!(schema["type"], "object", "{name}");
        assert_eq!(schema["additionalProperties"], false, "{name}");
        let text = serde_json::to_string(&**schema).unwrap();
        assert!(!text.contains("$ref"), "{name}: schema has a $ref");
        // Every property, at any depth, says what it is.
        let mut undescribed = Vec::new();
        undescribed_properties(&Value::Object((**schema).clone()), name, &mut undescribed);
        assert!(undescribed.is_empty(), "no description: {undescribed:?}");
    }
    // Job tools that cost money say so.
    for name in ["start_run", "start_pipeline", "start_judge"] {
        let tool = tools.iter().find(|t| t.name == name).unwrap();
        assert!(
            tool.description.as_deref().unwrap().contains("COSTS MONEY")
                || tool
                    .description
                    .as_deref()
                    .unwrap()
                    .contains("COST WARNING"),
            "{name}"
        );
    }
}

#[tokio::test]
async fn every_tool_happy_path_against_the_mock_server() {
    let studio = Studio::start().await;
    studio.mount_happy(TOKEN, "alice", "admin").await;
    // The run case sets acknowledge_risks: a server where a human allowed that.
    let client = connect_allowing_risk_acknowledgement(&studio.url(), TOKEN).await;

    let mut covered = BTreeSet::new();
    for (tool, args) in cases() {
        let result = call(&client, tool, args.clone()).await;
        assert_eq!(
            result.is_error,
            Some(false),
            "{tool} {args}: {}",
            text(&result)
        );
        if tool == "read_audit_session" {
            assert_eq!(text(&result), AUDIT_TEXT);
        } else {
            let p = payload(&result);
            check_payload(tool, &args, &p);
            if p.is_object() {
                assert_eq!(result.structured_content.as_ref(), Some(&p), "{tool}");
            }
        }
        covered.insert(tool);
    }
    let expected: BTreeSet<&str> = EXPECTED_TOOLS.into_iter().collect();
    assert_eq!(covered, expected, "every tool has a happy-path case");

    // The requests: the caller's token on every one, the CSRF header on every unsafe one.
    studio.assert_well_formed(TOKEN).await;
    let reqs = studio.received().await;
    let q = |verb: &str, route: String| -> Vec<Vec<(String, String)>> {
        requests_to(&reqs, verb, &route)
            .into_iter()
            .map(query)
            .collect()
    };
    let pairs = |list: &[(&str, &str)]| -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = list
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        v.sort();
        v
    };
    assert!(q("GET", api("/system/doctor")).contains(&pairs(&[("refresh", "true")])));
    assert!(q("GET", api("/experiments")).contains(&pairs(&[("archived", "true")])));
    assert_eq!(
        q("GET", api("/jobs")),
        vec![pairs(&[
            ("kind", "run"),
            ("limit", "5"),
            ("status", "running")
        ])]
    );
    let logs = q("GET", api(&format!("/jobs/{JOB_ID}/log")));
    assert!(
        logs.contains(&pairs(&[("from", "0"), ("limit", "200")])),
        "{logs:?}"
    );
    assert!(
        logs.contains(&pairs(&[("from", "1"), ("limit", "1")])),
        "{logs:?}"
    );
    assert_eq!(
        q("GET", sweep_route("/tasks")),
        vec![pairs(&[("limit", "50")])]
    );
    assert_eq!(
        q("GET", task_route("/modes/parallel/diff")),
        vec![pairs(&[("from", "base"), ("to", "agg1")])]
    );
    assert_eq!(
        q("GET", task_route("/modes/parallel/harnesses/agg1/export")),
        vec![pairs(&[("format", "json"), ("target", "opencode")])]
    );
    assert_eq!(
        q("GET", task_route("/modes/parallel/rollouts/agg1/files")),
        vec![pairs(&[])]
    );
    assert_eq!(
        q("GET", task_route("/modes/parallel/rollouts/agg1/file")),
        vec![pairs(&[("path", "out/report.md")])]
    );
    assert_eq!(
        q("GET", api("/compare")),
        vec![pairs(&[
            ("a", "demo/demo-oc-agg2/parallel"),
            ("b", "demo/demo-oc-agg2/sequential")
        ])]
    );

    // Bodies are the contract types.
    let body = |verb: &str, route: String| -> Value {
        requests_to(&reqs, verb, &route)
            .first()
            .map(|r| r.body_json::<Value>().unwrap())
            .unwrap_or(Value::Null)
    };
    let created = body("POST", api("/experiments"));
    assert_eq!(created["name"], "exp1");
    assert_eq!(created["profile"], "local");
    assert_eq!(created["kind"], "standard");
    assert_eq!(
        body("PATCH", api(&format!("/experiments/{EXP}"))),
        json!({"archived": true})
    );
    assert_eq!(
        body("POST", api("/estimate")),
        json!({"tasks": 10, "k": 2, "modes": ["parallel"]})
    );
    let validated = body("POST", api("/harness/validate"));
    assert_eq!(validated["surface"], "opencode");
    assert_eq!(validated["allow_subagents"], true);
    assert_eq!(validated["files"][0]["path"], "systemprompt.md");

    let specs: Vec<JobSpec> = requests_to(&reqs, "POST", &api("/jobs"))
        .into_iter()
        .map(|r| {
            r.body_json::<JobSpec>()
                .expect("every POST /jobs body is a JobSpec")
        })
        .collect();
    let kinds: BTreeSet<&str> = specs.iter().map(|s| s.kind().as_str()).collect();
    assert_eq!(
        kinds,
        [
            "bridge",
            "check",
            "demo",
            "install_harness2",
            "judge",
            "pipeline",
            "preview",
            "report",
            "run",
            "setup",
            "split"
        ]
        .into()
    );
    for spec in &specs {
        match spec {
            JobSpec::Run(p) => {
                assert_eq!(p.budget_usd, Some(5.0));
                assert!(p.acknowledge_risks);
                assert_eq!(p.options.max_tasks, Some(2));
            }
            JobSpec::Pipeline(p) => assert!(p.judge_bases),
            JobSpec::Demo(p) => assert_eq!((p.experiment.as_str(), p.k), (EXP, 2)),
            JobSpec::InstallHarness2(p) => assert_eq!(p.rev, None, "always the tested commit"),
            _ => {}
        }
    }
}

#[tokio::test]
async fn every_tool_renders_the_servers_refusal() {
    let studio = Studio::start().await;
    studio
        .mount_refusing(403, "forbidden", "your role is too low for this")
        .await;
    let client = connect_allowing_risk_acknowledgement(&studio.url(), TOKEN).await;
    for (tool, args) in cases() {
        let result = call(&client, tool, args.clone()).await;
        assert_eq!(result.is_error, Some(true), "{tool} {args}");
        let t = text(&result);
        assert!(
            t.starts_with("forbidden: your role is too low for this"),
            "{tool}: {t}"
        );
        assert!(t.contains("[request id req-1]"), "{tool}: {t}");
        let needs = match tool {
            "install_harness2" | "setup_benchmark" | "start_bridge" => "admin",
            "create_experiment" | "update_experiment" | "cancel_job" | "preview_run"
            | "start_run" | "start_pipeline" | "run_demo" | "start_judge" | "start_report"
            | "create_split" | "check_benchmark" => "operator",
            "doctor" if args["refresh"] == true => "operator",
            _ => "viewer",
        };
        assert!(
            t.contains(&format!("needs the {needs} role")),
            "{tool}: {t}"
        );
    }
    studio.assert_well_formed(TOKEN).await;
}

#[tokio::test]
async fn validation_unavailable_and_unauthorized_errors_are_explained() {
    let studio = Studio::start().await;
    Mock::given(method("POST"))
        .and(path(api("/jobs")))
        .and(body_partial_json(json!({"kind": "run"})))
        .respond_with(ResponseTemplate::new(422).set_body_json(error_body(
            "validation",
            "invalid run options",
            json!([{"field": "options.k", "problem": "must be between 1 and 32"}]),
        )))
        .mount(&studio.server)
        .await;
    Mock::given(method("POST"))
        .and(path(api("/jobs")))
        .and(body_partial_json(json!({"kind": "demo"})))
        .respond_with(ResponseTemplate::new(503).set_body_json(error_body(
            "unavailable",
            "harness2 is not installed",
            json!([]),
        )))
        .mount(&studio.server)
        .await;
    Mock::given(method("GET"))
        .and(path(api("/benchmarks")))
        .respond_with(ResponseTemplate::new(401).set_body_json(error_body(
            "unauthorized",
            "invalid token",
            json!([]),
        )))
        .mount(&studio.server)
        .await;
    Mock::given(method("GET"))
        .and(path(api("/profiles")))
        .respond_with(ResponseTemplate::new(502).set_body_string("<html>Bad Gateway</html>"))
        .mount(&studio.server)
        .await;
    let client = connect(&studio.url(), TOKEN).await;

    let run = call(
        &client,
        "start_run",
        json!({"experiment": "e1", "cell": cell(), "options": {"k": 99, "modes": ["parallel"]},
               "profile": "local"}),
    )
    .await;
    assert_eq!(run.is_error, Some(true));
    assert!(
        text(&run)
            .starts_with("validation: invalid run options; options.k: must be between 1 and 32"),
        "{}",
        text(&run)
    );

    let demo = call(&client, "run_demo", json!({"experiment": EXP})).await;
    let t = text(&demo);
    assert!(
        t.starts_with("unavailable: harness2 is not installed"),
        "{t}"
    );
    assert!(t.contains("install_harness2 (admin role)"), "{t}");

    let benches = call(&client, "list_benchmarks", json!({})).await;
    let t = text(&benches);
    assert!(t.starts_with("unauthorized: invalid token"), "{t}");
    assert!(t.contains("H2S_TOKEN"), "{t}");

    let profiles = call(&client, "list_profiles", json!({})).await;
    assert_eq!(
        text(&profiles),
        "unavailable: HTTP 502 from the server: <html>Bad Gateway</html>"
    );
}

#[tokio::test]
async fn a_dead_server_is_reported_as_unavailable() {
    // Nothing listens on port 9 (discard) on a test machine; the call must fail cleanly.
    let client = connect("http://127.0.0.1:9", TOKEN).await;
    let result = call(&client, "list_experiments", json!({})).await;
    assert_eq!(result.is_error, Some(true));
    let t = text(&result);
    assert!(
        t.starts_with("unavailable: cannot connect to the Harness² Studio server"),
        "{t}"
    );
}

#[tokio::test]
async fn bad_arguments_are_tool_errors_and_never_reach_the_server() {
    let studio = Studio::start().await;
    let client = connect(&studio.url(), TOKEN).await;
    let cases = [
        (
            "start_run",
            json!({"experiment": "e1", "cell": cell(), "options": options(), "profile": "local",
                   "budget": 5}),
            "unknown argument `budget` (allowed here: acknowledge_risks, budget_usd, cell, \
             experiment, options, profile, wait_secs)",
        ),
        (
            "preview_run",
            json!({"experiment": EXP, "cell": cell(),
                   "options": {"k": 1, "modes": ["parallel"], "max_task": 2}}),
            "unknown argument `options.max_task`",
        ),
        (
            "get_task",
            json!({"experiment": EXP, "sweep": SWEEP}),
            "missing field `task`",
        ),
        (
            "get_harness",
            task_args(json!({"mode": "both", "label": "agg1"})),
            "unknown variant `both`",
        ),
        ("wait_for_job", json!({"job_id": 7}), "invalid type"),
        (
            "get_experiment",
            json!({"experiment": ".."}),
            "bad_request: invalid identifier \"..\"",
        ),
        (
            "install_harness2",
            json!({"rev": "0123456789abcdef0123456789abcdef01234567"}),
            "unknown argument `rev` (allowed here: extras, reinstall, wait_secs)",
        ),
    ];
    for (tool, args, expected) in cases {
        let result = call(&client, tool, args).await;
        assert_eq!(result.is_error, Some(true), "{tool}");
        let t = text(&result);
        assert!(t.contains(expected), "{tool}: {t}");
    }
    assert!(
        studio.received().await.is_empty(),
        "invalid calls must not reach the server"
    );
}

/// A running job: progress events on the stream, then success.
#[tokio::test]
async fn wait_for_job_follows_events_and_sends_progress() {
    let studio = Studio::start().await;
    let spec = demo_spec();
    let running = job(
        JOB_ID,
        &spec,
        "running",
        Some(json!({"done": 0, "failed": 0, "total": 3})),
    );
    let finished = job(JOB_ID, &spec, "succeeded", None);
    get_route(&api(&format!("/jobs/{JOB_ID}")), TOKEN)
        .respond_with(Sequence::new(vec![running.clone(), finished]))
        .mount(&studio.server)
        .await;
    let frames = [
        ": keep-alive".to_owned(),
        format!("data: {}", json!({"type": "snapshot", "job": running})),
        format!(
            "data: {}",
            json!({"type": "progress", "progress":
                   {"done": 1, "failed": 0, "total": 3, "current": "contracts / t1"}})
        ),
        format!(
            "id: 5\ndata: {}",
            json!({"type": "log", "line":
                   {"n": 5, "ts": NOW, "stream": "stdout", "text": "[contracts] 1/3 ok t1"}})
        ),
        format!(
            "data: {}",
            json!({"type": "step", "step": {"name": "demo", "status": "running"}})
        ),
        format!(
            "data: {}",
            json!({"type": "progress", "progress":
                   {"done": 3, "failed": 0, "total": 3, "current": "contracts / t3"}})
        ),
        format!(
            "data: {}",
            json!({"type": "status", "status": "succeeded", "exit_code": 0, "error": null})
        ),
    ];
    let body = frames.join("\n\n") + "\n\n";
    get_route(&api(&format!("/jobs/{JOB_ID}/events")), TOKEN)
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
        .mount(&studio.server)
        .await;

    let client = connect(&studio.url(), TOKEN).await;
    let result = call(
        &client,
        "wait_for_job",
        json!({"job_id": JOB_ID, "timeout_secs": 30}),
    )
    .await;
    assert_eq!(result.is_error, Some(false), "{}", text(&result));
    let p = payload(&result);
    assert_eq!(p["status"], "succeeded");
    assert_eq!(
        p["result"]["kind"], "demo",
        "the final job is re-read for its result"
    );
    let note = result.content[1].as_text().unwrap().text.clone();
    assert!(note.contains("succeeded"), "{note}");

    let progress = progress_after(&client, 4).await;
    assert!(progress.len() >= 4, "{progress:?}");
    for pair in progress.windows(2) {
        assert!(pair[1].progress > pair[0].progress, "{progress:?}");
    }
    let last = progress.last().unwrap();
    assert_eq!(last.progress, 3.0);
    assert_eq!(last.total, Some(3.0));
    assert!(
        progress.iter().any(|n| {
            n.message.as_deref().is_some_and(|m| {
                m.contains("1/3 tasks done") && m.contains("current: contracts / t1")
            })
        }),
        "{progress:?}"
    );
    studio.assert_well_formed(TOKEN).await;
}

/// No event stream (404): poll until done.
#[tokio::test]
async fn wait_for_job_polls_when_events_are_unavailable() {
    let studio = Studio::start().await;
    let spec = demo_spec();
    let at = |done: u32| {
        job(
            JOB_ID,
            &spec,
            "running",
            Some(json!({"done": done, "failed": 0, "total": 2})),
        )
    };
    get_route(&api(&format!("/jobs/{JOB_ID}")), TOKEN)
        .respond_with(Sequence::new(vec![
            at(0),
            at(1),
            job(JOB_ID, &spec, "succeeded", None),
        ]))
        .mount(&studio.server)
        .await;
    let client = connect(&studio.url(), TOKEN).await;
    let result = call(
        &client,
        "wait_for_job",
        json!({"job_id": JOB_ID, "timeout_secs": 30, "poll_interval_secs": 1}),
    )
    .await;
    assert_eq!(payload(&result)["status"], "succeeded");
    // One notification per change of task progress; finishing adds none (the result follows).
    let progress = progress_after(&client, 2).await;
    let values: Vec<(f64, Option<f64>)> = progress.iter().map(|n| (n.progress, n.total)).collect();
    assert_eq!(values, vec![(0.0, Some(2.0)), (1.0, Some(2.0))]);
}

#[tokio::test]
async fn wait_for_job_returns_the_running_job_on_timeout() {
    let studio = Studio::start().await;
    let spec = demo_spec();
    get_route(&api(&format!("/jobs/{JOB_ID}")), TOKEN)
        .respond_with(ResponseTemplate::new(200).set_body_json(job(JOB_ID, &spec, "running", None)))
        .mount(&studio.server)
        .await;
    let client = connect(&studio.url(), TOKEN).await;
    let started = std::time::Instant::now();
    let result = call(
        &client,
        "wait_for_job",
        json!({"job_id": JOB_ID, "timeout_secs": 1, "poll_interval_secs": 1}),
    )
    .await;
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    assert_eq!(result.is_error, Some(false));
    assert_eq!(payload(&result)["status"], "running");
    let note = result.content[1].as_text().unwrap().text.clone();
    assert!(note.contains("still running"), "{note}");
    assert!(note.contains("wait_for_job"), "{note}");
}

#[tokio::test]
async fn job_tools_wait_when_asked() {
    let studio = Studio::start().await;
    unsafe_route("POST", &api("/jobs"), TOKEN)
        .respond_with(JobFactory { status: "queued" })
        .mount(&studio.server)
        .await;
    get_route(&api(&format!("/jobs/{JOB_ID}")), TOKEN)
        .respond_with(Sequence::new(vec![
            job(JOB_ID, &demo_spec(), "running", None),
            job(JOB_ID, &demo_spec(), "succeeded", None),
        ]))
        .mount(&studio.server)
        .await;
    let client = connect(&studio.url(), TOKEN).await;

    // wait_secs=0: the queued job at once, with a pointer to wait_for_job.
    let queued = call(
        &client,
        "run_demo",
        json!({"experiment": EXP, "wait_secs": 0}),
    )
    .await;
    assert_eq!(payload(&queued)["status"], "queued");
    let note = queued.content[1].as_text().unwrap().text.clone();
    assert!(note.contains("wait_for_job(job_id=\""), "{note}");

    // The default for the demo waits for the result.
    let done = call(&client, "run_demo", json!({"experiment": EXP})).await;
    let p = payload(&done);
    assert_eq!(p["status"], "succeeded", "{p:#}");
    let note = done.content[1].as_text().unwrap().text.clone();
    assert!(
        note.contains("get_experiment(experiment=\"demo\")"),
        "{note}"
    );
}

#[tokio::test]
async fn resources_templates_listing_and_reads() {
    let studio = Studio::start().await;
    studio.mount_happy(TOKEN, "alice", "viewer").await;
    let client = connect(&studio.url(), TOKEN).await;

    let templates = client.list_resource_templates(None).await.unwrap();
    let uris: Vec<&str> = templates
        .resource_templates
        .iter()
        .map(|t| t.uri_template.as_str())
        .collect();
    assert_eq!(
        uris,
        vec![
            "h2s://jobs/{id}/log",
            "h2s://experiments/{exp}/sweeps/{sweep}/report",
            "h2s://experiments/{exp}/manifest"
        ]
    );

    let listed = client.list_resources(None).await.unwrap();
    let uris: BTreeSet<String> = listed.resources.iter().map(|r| r.uri.clone()).collect();
    assert!(uris.contains("h2s://experiments/demo/manifest"), "{uris:?}");
    assert!(
        uris.contains("h2s://experiments/demo/sweeps/demo-oc-agg2/report"),
        "{uris:?}"
    );
    assert!(
        !uris.contains("h2s://experiments/demo/sweeps/demo-oc-agg1/report"),
        "unjudged sweeps have no report: {uris:?}"
    );
    assert!(
        uris.contains(&format!("h2s://jobs/{JOB_ID}/log")),
        "{uris:?}"
    );

    let read = |uri: String| {
        let client = &client;
        async move {
            let result = client
                .read_resource(ReadResourceRequestParams::new(uri.clone()))
                .await
                .unwrap_or_else(|e| panic!("{uri}: {e}"));
            match result.contents.into_iter().next().unwrap() {
                ResourceContents::TextResourceContents {
                    text, mime_type, ..
                } => (text, mime_type),
                other => panic!("{uri}: {other:?}"),
            }
        }
    };
    let (log, mime) = read(format!("h2s://jobs/{JOB_ID}/log")).await;
    assert_eq!(mime.as_deref(), Some("text/plain"));
    assert!(log.contains("[out] [contracts] 1/4 ok t1"), "{log}");
    assert!(log.starts_with("# demo job"), "{log}");
    let (report, mime) = read(format!("h2s://experiments/{EXP}/sweeps/{SWEEP}/report")).await;
    assert_eq!(mime.as_deref(), Some("application/json"));
    assert_eq!(
        serde_json::from_str::<Value>(&report).unwrap()["summaries"][0]["score"],
        62.5
    );
    let (manifest, _) = read(format!("h2s://experiments/{EXP}/manifest")).await;
    assert_eq!(
        serde_json::from_str::<Value>(&manifest).unwrap()["experiment"],
        EXP
    );

    let missing = client
        .read_resource(ReadResourceRequestParams::new("h2s://nope"))
        .await;
    assert!(missing.is_err());
    studio.assert_well_formed(TOKEN).await;
}

#[tokio::test]
async fn prompts_list_and_get() {
    let studio = Studio::start().await;
    let client = connect(&studio.url(), TOKEN).await;
    let prompts = client.list_prompts(None).await.unwrap().prompts;
    let names: Vec<&str> = prompts.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "improve_agent_harness",
            "analyze_sweep",
            "adopt_best_harness"
        ]
    );
    let analyze = prompts.iter().find(|p| p.name == "analyze_sweep").unwrap();
    let required: Vec<&str> = analyze
        .arguments
        .as_deref()
        .unwrap()
        .iter()
        .filter(|a| a.required == Some(true))
        .map(|a| a.name.as_str())
        .collect();
    assert_eq!(required, vec!["experiment", "sweep"]);

    let args = |v: Value| match v {
        Value::Object(map) => map,
        _ => unreachable!(),
    };
    let improve = client
        .get_prompt(
            GetPromptRequestParams::new("improve_agent_harness")
                .with_arguments(args(json!({"bench": "lab", "k": "2", "budget_usd": "10"}))),
        )
        .await
        .unwrap();
    let text = improve.messages[0].content.as_text().unwrap().text.clone();
    for step in [
        "server_info",
        "doctor",
        "check_benchmark",
        "preview_run",
        "estimate_cost",
        "start_pipeline",
        "wait_for_job",
        "get_report",
        "diff_harnesses",
        "export_harness",
    ] {
        assert!(text.contains(step), "improve_agent_harness lacks {step}");
    }
    assert!(text.contains("LAB (bench lab)") && text.contains("10.00 USD"));

    let adopt = client
        .get_prompt(
            GetPromptRequestParams::new("adopt_best_harness").with_arguments(args(
                json!({"experiment": EXP, "sweep": SWEEP, "target": "codex"}),
            )),
        )
        .await
        .unwrap();
    let text = adopt.messages[0].content.as_text().unwrap().text.clone();
    assert!(text.contains("target=codex") && text.contains("validate_harness"));

    let missing = client
        .get_prompt(
            GetPromptRequestParams::new("analyze_sweep")
                .with_arguments(args(json!({"experiment": EXP}))),
        )
        .await;
    assert!(missing.is_err(), "sweep is required");
    assert!(studio.received().await.is_empty());
}

#[tokio::test]
async fn cancelling_a_wait_stops_the_server_side_polling() {
    let studio = Studio::start().await;
    get_route(&api(&format!("/jobs/{JOB_ID}")), TOKEN)
        .respond_with(ResponseTemplate::new(200).set_body_json(job(
            JOB_ID,
            &demo_spec(),
            "running",
            None,
        )))
        .mount(&studio.server)
        .await;
    let client = connect(&studio.url(), TOKEN).await;
    let args = json!({"job_id": JOB_ID, "timeout_secs": 120, "poll_interval_secs": 1});
    let Value::Object(args) = args else {
        unreachable!()
    };
    let request = ClientRequest::CallToolRequest(Request::new(
        CallToolRequestParams::new("wait_for_job").with_arguments(args),
    ));
    let handle = client
        .peer()
        .send_cancellable_request(request, PeerRequestOptions::no_options())
        .await
        .unwrap();
    let polls = || async {
        requests_to(
            &studio.received().await,
            "GET",
            &api(&format!("/jobs/{JOB_ID}")),
        )
        .len()
    };
    tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
    handle
        .cancel(Some("user stopped waiting".into()))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let at_cancel = polls().await;
    assert!(at_cancel >= 2, "the wait was polling: {at_cancel}");
    tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
    assert_eq!(
        polls().await,
        at_cancel,
        "polling continued after cancellation"
    );
}

#[tokio::test]
async fn long_results_are_truncated_with_a_note() {
    let studio = Studio::start().await;
    let long = "a line of rollout output\n".repeat(6_000); // 150 KB
    let content = typed::<h2s_api::FileContent>(json!({
        "path": "out/log.txt", "bytes": long.len(), "text": long, "truncated": false,
        "mime": "text/plain"
    }));
    get_route(&task_route("/modes/parallel/rollouts/agg1/file"), TOKEN)
        .respond_with(ResponseTemplate::new(200).set_body_json(content))
        .mount(&studio.server)
        .await;
    let client = connect(&studio.url(), TOKEN).await;
    let result = call(
        &client,
        "read_rollout_file",
        task_args(json!({"mode": "parallel", "label": "agg1", "path": "out/log.txt"})),
    )
    .await;
    assert_eq!(result.is_error, Some(false));
    let t = text(&result);
    assert!(t.len() < 100_500, "{} bytes", t.len());
    assert!(
        t.contains("[output truncated: the full result is"),
        "{}",
        &t[t.len() - 300..]
    );
    assert!(t.contains("structured content holds the complete value"));
    let full = result
        .structured_content
        .expect("structured content under 256 KiB");
    assert_eq!(full["text"].as_str().unwrap().len(), long.len());
}

#[tokio::test]
async fn job_tools_return_the_job_instead_of_waiting_past_the_cap() {
    let studio = Studio::start().await;
    unsafe_route("POST", &api("/jobs"), TOKEN)
        .respond_with(JobFactory { status: "queued" })
        .mount(&studio.server)
        .await;
    get_route(&api(&format!("/jobs/{JOB_ID}")), TOKEN)
        .respond_with(ResponseTemplate::new(200).set_body_json(job(
            JOB_ID,
            &demo_spec(),
            "running",
            None,
        )))
        .mount(&studio.server)
        .await;
    let client =
        std::sync::Arc::new(connect_with(&studio.url(), TOKEN, h2s_mcp::WaitLimiter::new(1)).await);
    let long_wait = {
        let client = std::sync::Arc::clone(&client);
        tokio::spawn(async move {
            call(
                &client,
                "wait_for_job",
                json!({"job_id": JOB_ID, "timeout_secs": 3, "poll_interval_secs": 1}),
            )
            .await
        })
    };
    until_requests(&studio, "GET", &api(&format!("/jobs/{JOB_ID}")), 1).await;

    // The job is created; the call returns it without waiting, and says why.
    let demo = call(
        &client,
        "run_demo",
        json!({"experiment": EXP, "wait_secs": 30}),
    )
    .await;
    assert_eq!(demo.is_error, Some(false), "{}", text(&demo));
    assert_eq!(payload(&demo)["status"], "queued");
    let note = demo.content[1].as_text().unwrap().text.clone();
    assert!(
        note.starts_with("Not waiting: this token already runs 1 waits"),
        "{note}"
    );
    assert!(note.contains("wait_for_job(job_id="), "{note}");

    let refused = call(&client, "wait_for_job", json!({"job_id": JOB_ID})).await;
    assert!(
        text(&refused).starts_with("rate_limited: "),
        "{}",
        text(&refused)
    );
    assert_eq!(long_wait.await.unwrap().is_error, Some(false));
}

#[tokio::test]
async fn models_cannot_acknowledge_host_risks_unless_a_human_allowed_it() {
    let studio = Studio::start().await;
    studio.mount_happy(TOKEN, "alice", "operator").await;
    let run = json!({"experiment": "e1", "cell": cell(), "options": options(),
                     "profile": "local", "acknowledge_risks": true});
    let pipeline = json!({"experiment": "e1", "cell": cell(), "options": options(),
                          "profile": "local", "acknowledge_risks": true});

    let client = connect(&studio.url(), TOKEN).await;
    let policy =
        payload(&call(&client, "server_info", json!({})).await)["mcp_server"]["policy"].clone();
    assert_eq!(policy["allow_risk_acknowledgement"], false);
    assert_eq!(policy["max_waits_per_token"], 4);
    for (tool, args) in [("start_run", &run), ("start_pipeline", &pipeline)] {
        let refused = call(&client, tool, args.clone()).await;
        assert_eq!(refused.is_error, Some(true), "{tool}");
        let t = text(&refused);
        assert!(
            t.starts_with("forbidden: this MCP server does not let a model set acknowledge_risks"),
            "{tool}: {t}"
        );
        for way in [
            "web UI",
            "pre-acknowledge",
            "--allow-risk-acknowledgement",
            "H2S_MCP_ALLOW_RISK_ACK=1",
        ] {
            assert!(t.contains(way), "{tool}: no mention of {way}: {t}");
        }
    }
    assert!(
        requests_to(&studio.received().await, "POST", &api("/jobs")).is_empty(),
        "refused calls must not reach the server"
    );

    // Without the acknowledgement the call goes through unchanged; the server decides.
    let mut plain = run.clone();
    plain["acknowledge_risks"] = json!(false);
    let started = call(&client, "start_run", plain).await;
    assert_eq!(started.is_error, Some(false), "{}", text(&started));

    // With the server-side opt-in, the acknowledgement is passed on.
    let allowed = connect_allowing_risk_acknowledgement(&studio.url(), TOKEN).await;
    let policy =
        payload(&call(&allowed, "server_info", json!({})).await)["mcp_server"]["policy"].clone();
    assert_eq!(policy["allow_risk_acknowledgement"], true);
    let started = call(&allowed, "start_pipeline", pipeline).await;
    assert_eq!(started.is_error, Some(false), "{}", text(&started));
    let specs: Vec<JobSpec> = requests_to(&studio.received().await, "POST", &api("/jobs"))
        .into_iter()
        .map(|r| r.body_json::<JobSpec>().unwrap())
        .collect();
    assert!(
        matches!(&specs[..], [JobSpec::Run(r), JobSpec::Pipeline(p)]
            if !r.acknowledge_risks && p.acknowledge_risks),
        "{specs:?}"
    );
}
