//! End to end over MCP: a real MCP client (rmcp) spawns the real `h2workbench-mcp` over stdio,
//! which drives the real server, which runs the real harness2 demo. Run with
//! `cargo xtask e2e`.

// Test-only crate: a panic with a clear message IS how a failed expectation is reported here.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::time::Duration;

use common::Workbench;
use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::TokioChildProcess;
use serde_json::{Value, json};

/// Every tool the contract (ARCHITECTURE.md §13) promises.
const EXPECTED_TOOLS: &[&str] = &[
    "server_info",
    "doctor",
    "list_benchmarks",
    "setup_benchmark",
    "check_benchmark",
    "list_profiles",
    "check_profile",
    "list_experiments",
    "create_experiment",
    "get_experiment",
    "estimate_cost",
    "preview_run",
    "start_run",
    "start_pipeline",
    "run_demo",
    "start_judge",
    "start_report",
    "create_split",
    "list_jobs",
    "get_job",
    "wait_for_job",
    "cancel_job",
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
    "get_manifest",
    "install_harness2",
    "start_bridge",
    "list_splits",
    "update_experiment",
];

async fn connect(workbench: &Workbench, token: &str) -> RunningService<RoleClient, ()> {
    let mut cmd = tokio::process::Command::new(workbench.env.mcp());
    cmd.env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("H2W_SERVER_URL", &workbench.base)
        .env("H2W_TOKEN", token)
        .env("RUST_LOG", "warn");
    let transport = TokioChildProcess::new(cmd).expect("spawn h2workbench-mcp");
    ().serve(transport).await.expect("MCP initialize")
}

fn args(v: Value) -> serde_json::Map<String, Value> {
    match v {
        Value::Object(m) => m,
        other => panic!("tool arguments must be an object, got {other}"),
    }
}

/// The JSON a tool returned: structured content when present, else the first text block.
fn payload(result: &CallToolResult) -> Value {
    if let Some(v) = &result.structured_content {
        return v.clone();
    }
    let text = result
        .content
        .iter()
        .find_map(|c| c.as_text().map(|t| t.text.clone()))
        .expect("tool returned no text content");
    serde_json::from_str(&text).unwrap_or(Value::String(text))
}

async fn call(
    client: &RunningService<RoleClient, ()>,
    name: &'static str,
    a: Value,
) -> CallToolResult {
    client
        .call_tool(CallToolRequestParams::new(name).with_arguments(args(a)))
        .await
        .unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[tokio::test]
#[ignore = "end-to-end: run with `cargo xtask e2e`"]
async fn mcp_lists_every_tool_and_runs_the_demo() {
    let workbench = Workbench::start();
    let client = connect(&workbench, &workbench.admin_token).await;

    // The contract's tool set, each with a description and an object input schema.
    let tools = client.list_all_tools().await.expect("tools/list");
    let names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    for want in EXPECTED_TOOLS {
        assert!(
            names.iter().any(|n| n == want),
            "missing MCP tool {want}; have {names:?}"
        );
    }
    for t in &tools {
        assert!(
            t.description.as_ref().is_some_and(|d| d.len() >= 20),
            "tool {} needs a real description",
            t.name
        );
    }

    // server_info reflects the live server.
    let info = payload(&call(&client, "server_info", json!({})).await);
    assert!(info.to_string().contains("harness2"), "server_info: {info}");

    // Run the demo through MCP and wait for it with the MCP tool.
    let started = call(
        &client,
        "run_demo",
        json!({ "experiment": "mcp-demo", "k": 1, "mode": "parallel" }),
    )
    .await;
    assert_ne!(
        started.is_error,
        Some(true),
        "run_demo failed: {:?}",
        started.content
    );
    let job = payload(&started);
    let id = job["id"].as_str().expect("job id").to_owned();
    let waited = tokio::time::timeout(
        Duration::from_secs(180),
        call(
            &client,
            "wait_for_job",
            json!({ "job_id": id, "timeout_secs": 170 }),
        ),
    )
    .await
    .expect("wait_for_job returned in time");
    let done = payload(&waited);
    assert_eq!(done["status"], "succeeded", "job: {done}");

    // Results through MCP match what the REST API reports.
    let task = payload(
        &call(
            &client,
            "get_task",
            json!({ "experiment": "mcp-demo", "sweep": "demo-oc-agg1", "task": "ledger-1" }),
        )
        .await,
    );
    assert_eq!(task["domain"], "accounting", "get_task: {task}");
    let diff = payload(
        &call(
            &client,
            "diff_harnesses",
            json!({ "experiment": "mcp-demo", "sweep": "demo-oc-agg1", "task": "ledger-1",
                    "mode": "parallel", "from": "base", "to": "agg1" }),
        )
        .await,
    );
    assert!(
        diff.to_string().contains("summarize.py"),
        "diff_harnesses: {diff}"
    );

    client.cancel().await.expect("close the MCP session");
}

#[tokio::test]
#[ignore = "end-to-end: run with `cargo xtask e2e`"]
async fn mcp_tools_respect_the_callers_role() {
    let workbench = Workbench::start();
    let viewer = workbench.token("e2e-mcp-viewer", "viewer");
    let client = connect(&workbench, &viewer).await;

    // Reading works for a viewer...
    let listed = call(&client, "list_experiments", json!({})).await;
    assert_ne!(listed.is_error, Some(true));

    // ...but starting a job is refused by the server, and the tool says why.
    let refused = client
        .call_tool(CallToolRequestParams::new("run_demo").with_arguments(args(
            json!({ "experiment": "nope", "k": 1, "mode": "parallel" }),
        )))
        .await;
    let refused_text = match refused {
        Ok(r) => {
            assert_eq!(
                r.is_error,
                Some(true),
                "a viewer must not start jobs: {:?}",
                r.content
            );
            r.content
                .iter()
                .filter_map(|c| c.as_text().map(|t| t.text.clone()))
                .collect::<Vec<_>>()
                .join(" ")
        }
        Err(e) => e.to_string(),
    };
    assert!(
        refused_text.to_lowercase().contains("forbidden"),
        "explains the refusal: {refused_text}"
    );

    client.cancel().await.expect("close the MCP session");
}
