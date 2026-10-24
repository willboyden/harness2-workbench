//! Streamable HTTP transport: the Host / Origin / bearer guard, and per-caller token
//! forwarding, on 127.0.0.1:0 against a mock Workbench server.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::net::SocketAddr;

use common::*;
use h2w_mcp::{
    ApiClient, ClientOptions,
    http::{self, HttpOptions},
};
use rmcp::{
    ServiceExt,
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use wiremock::ResponseTemplate;

const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"raw","version":"0"}}}"#;

/// A running `h2workbench-mcp --transport http`, stopped when dropped.
struct McpHttp {
    addr: SocketAddr,
    shutdown: CancellationToken,
}

impl McpHttp {
    async fn start(workbench_url: &str, options: HttpOptions) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let api = ApiClient::new(workbench_url, &ClientOptions::default()).unwrap();
        let shutdown = CancellationToken::new();
        let stop = shutdown.clone();
        tokio::spawn(async move { http::serve(listener, api, &options, stop).await });
        Self { addr, shutdown }
    }

    fn url(&self) -> String {
        format!("http://{}/mcp", self.addr)
    }

    /// An rmcp streamable-HTTP client session that sends `token`.
    async fn client(&self, token: &str) -> Client {
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let config = StreamableHttpClientTransportConfig::with_uri(self.url()).auth_header(token);
        let transport = StreamableHttpClientTransport::with_client(http, config);
        TestClient::default().serve(transport).await.unwrap()
    }
}

impl Drop for McpHttp {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

fn raw_client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

/// POST an `initialize` request with extra headers.
async fn post_initialize(mcp: &McpHttp, headers: &[(&str, String)]) -> reqwest::Response {
    let mut request = raw_client()
        .post(mcp.url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(INITIALIZE);
    for (name, value) in headers {
        request = request.header(*name, value);
    }
    request.send().await.unwrap()
}

async fn error_code(response: reqwest::Response) -> String {
    let body: Value = response.json().await.unwrap();
    body["error"]["code"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

fn bearer(token: &str) -> (&'static str, String) {
    ("authorization", format!("Bearer {token}"))
}

#[tokio::test]
async fn a_missing_or_malformed_bearer_is_401() {
    let workbench = Workbench::start().await;
    let mcp = McpHttp::start(&workbench.url(), HttpOptions::default()).await;

    let response = post_initialize(&mcp, &[]).await;
    assert_eq!(response.status(), 401);
    let challenge = response
        .headers()
        .get("www-authenticate")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    assert!(challenge.starts_with("Bearer"), "{challenge}");
    assert_eq!(error_code(response).await, "unauthorized");

    for value in [
        "Bearer not-a-token",
        "Basic aDJzX2FsaWNlX3MzY3JldA==",
        "h2w_alice_s3cret",
    ] {
        let response = post_initialize(&mcp, &[("authorization", value.to_owned())]).await;
        assert_eq!(response.status(), 401, "{value}");
    }
    assert!(
        workbench.received().await.is_empty(),
        "refused requests never reach the Workbench server"
    );
}

#[tokio::test]
async fn an_unknown_host_is_421_even_with_a_token() {
    let workbench = Workbench::start().await;
    let mcp = McpHttp::start(&workbench.url(), HttpOptions::default()).await;
    let port = mcp.addr.port();
    for host in [
        format!("evil.example:{port}"),
        format!("127.0.0.1:{}", port.wrapping_add(1)),
        "localhost".to_owned(),
    ] {
        let response = post_initialize(&mcp, &[bearer(TOKEN), ("host", host.clone())]).await;
        assert_eq!(response.status(), 421, "{host}");
        assert_eq!(error_code(response).await, "misdirected_request");
    }
    // The Host check comes first: no token and a bad Host is still 421.
    let response = post_initialize(&mcp, &[("host", "evil.example".to_owned())]).await;
    assert_eq!(response.status(), 421);
    // Loopback forms of the listen port pass.
    for host in [format!("localhost:{port}"), format!("127.0.0.1:{port}")] {
        let response = post_initialize(&mcp, &[bearer(TOKEN), ("host", host.clone())]).await;
        assert_eq!(response.status(), 200, "{host}");
    }
}

#[tokio::test]
async fn allowed_hosts_and_origins_extend_the_defaults() {
    let workbench = Workbench::start().await;
    let mcp = McpHttp::start(
        &workbench.url(),
        HttpOptions {
            allowed_hosts: vec!["mcp.example.com".into()],
            allowed_origins: vec!["https://inspector.example.com".into()],
            ..HttpOptions::default()
        },
    )
    .await;
    let response = post_initialize(
        &mcp,
        &[
            bearer(TOKEN),
            ("host", "mcp.example.com".into()),
            ("origin", "https://inspector.example.com".into()),
        ],
    )
    .await;
    assert_eq!(response.status(), 200);
}

#[tokio::test]
async fn a_foreign_origin_is_403() {
    let workbench = Workbench::start().await;
    let mcp = McpHttp::start(&workbench.url(), HttpOptions::default()).await;
    for origin in ["http://evil.example", "null", "http://localhost:6274"] {
        let response = post_initialize(&mcp, &[bearer(TOKEN), ("origin", origin.into())]).await;
        assert_eq!(response.status(), 403, "{origin}");
        assert_eq!(error_code(response).await, "forbidden");
    }
    let own = format!("http://127.0.0.1:{}", mcp.addr.port());
    let response = post_initialize(&mcp, &[bearer(TOKEN), ("origin", own)]).await;
    assert_eq!(response.status(), 200);
}

#[tokio::test]
async fn oversized_bodies_and_other_methods_are_refused() {
    let workbench = Workbench::start().await;
    let mcp = McpHttp::start(
        &workbench.url(),
        HttpOptions {
            max_body_bytes: 64 * 1024,
            ..HttpOptions::default()
        },
    )
    .await;
    let big = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"validate_harness","arguments":{{"surface":"opencode","files":[{{"path":"a","content":"{}"}}]}}}}}}"#,
        "x".repeat(128 * 1024)
    );
    let response = raw_client()
        .post(mcp.url())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("authorization", format!("Bearer {TOKEN}"))
        .body(big)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 413);

    // Stateless: there is no standalone event stream or session to delete.
    let response = raw_client()
        .get(mcp.url())
        .header("accept", "text/event-stream")
        .header("authorization", format!("Bearer {TOKEN}"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 405);
    assert!(workbench.received().await.is_empty());
}

#[tokio::test]
async fn each_caller_s_token_is_forwarded() {
    let workbench = Workbench::start().await;
    workbench.mount_happy(TOKEN, "alice", "operator").await;
    workbench.mount_happy(OTHER_TOKEN, "bob", "viewer").await;
    workbench
        .server
        .register(get_route(&api("/auth/me"), REVOKED_TOKEN).respond_with(
            ResponseTemplate::new(401).set_body_json(error_body(
                "unauthorized",
                "token revoked",
                json!([]),
            )),
        ))
        .await;
    let mcp = McpHttp::start(&workbench.url(), HttpOptions::default()).await;

    let alice = mcp.client(TOKEN).await;
    let bob = mcp.client(OTHER_TOKEN).await;
    let tools = alice.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), 42);

    let who = |result: &rmcp::model::CallToolResult| payload(result)["principal"]["name"].clone();
    assert_eq!(who(&call(&alice, "server_info", json!({})).await), "alice");
    assert_eq!(who(&call(&bob, "server_info", json!({})).await), "bob");
    assert_eq!(who(&call(&alice, "server_info", json!({})).await), "alice");

    let jobs = call(&bob, "list_jobs", json!({})).await;
    assert_eq!(jobs.is_error, Some(false), "{}", text(&jobs));

    let mallory = mcp.client(REVOKED_TOKEN).await;
    let refused = call(&mallory, "server_info", json!({})).await;
    assert_eq!(refused.is_error, Some(true));
    assert!(
        text(&refused).starts_with("unauthorized: token revoked"),
        "{}",
        text(&refused)
    );

    // Every request carried exactly the token of the MCP caller it served.
    let requests = workbench.received().await;
    let auth_of = |r: &wiremock::Request| {
        r.headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    };
    let me: Vec<String> = requests_to(&requests, "GET", &api("/auth/me"))
        .into_iter()
        .map(auth_of)
        .collect();
    assert_eq!(
        me,
        vec![
            format!("Bearer {TOKEN}"),
            format!("Bearer {OTHER_TOKEN}"),
            format!("Bearer {TOKEN}"),
            format!("Bearer {REVOKED_TOKEN}"),
        ]
    );
    let listed: Vec<String> = requests_to(&requests, "GET", &api("/jobs"))
        .into_iter()
        .map(auth_of)
        .collect();
    assert_eq!(listed, vec![format!("Bearer {OTHER_TOKEN}")]);
}

#[tokio::test]
async fn progress_notifications_stream_over_http() {
    let workbench = Workbench::start().await;
    let spec = demo_spec();
    get_route(&api(&format!("/jobs/{JOB_ID}")), TOKEN)
        .respond_with(Sequence::new(vec![
            job(
                JOB_ID,
                &spec,
                "running",
                Some(json!({"done": 0, "failed": 0, "total": 2})),
            ),
            job(
                JOB_ID,
                &spec,
                "running",
                Some(json!({"done": 1, "failed": 0, "total": 2})),
            ),
            job(JOB_ID, &spec, "succeeded", None),
        ]))
        .mount(&workbench.server)
        .await;
    let mcp = McpHttp::start(&workbench.url(), HttpOptions::default()).await;
    let client = mcp.client(TOKEN).await;
    let result = call(
        &client,
        "wait_for_job",
        json!({"job_id": JOB_ID, "timeout_secs": 30, "poll_interval_secs": 1}),
    )
    .await;
    assert_eq!(payload(&result)["status"], "succeeded");
    let progress = progress_after(&client, 2).await;
    let values: Vec<f64> = progress.iter().map(|n| n.progress).collect();
    assert_eq!(values, vec![0.0, 1.0]);
    workbench.assert_well_formed(TOKEN).await;
}

#[tokio::test]
async fn every_response_carries_security_headers() {
    let workbench = Workbench::start().await;
    let mcp = McpHttp::start(&workbench.url(), HttpOptions::default()).await;
    let refused = post_initialize(&mcp, &[]).await;
    let served = post_initialize(&mcp, &[bearer(TOKEN)]).await;
    assert_eq!(
        (refused.status().as_u16(), served.status().as_u16()),
        (401, 200)
    );
    for response in [&refused, &served] {
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_owned()
        };
        assert_eq!(header("x-content-type-options"), "nosniff");
        assert_eq!(header("referrer-policy"), "no-referrer");
        assert_eq!(header("x-frame-options"), "DENY");
        assert_eq!(header("cache-control"), "no-store");
        assert!(header("content-security-policy").contains("frame-ancestors 'none'"));
        assert!(
            response
                .headers()
                .get("access-control-allow-origin")
                .is_none(),
            "no CORS, ever"
        );
    }
}

/// POST one self-contained `tools/call` and read the whole (event-stream) answer, so the
/// request stays in flight until the tool has finished.
async fn raw_tool_call(url: String, tool: &str, arguments: Value) -> (u16, String) {
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                      "params": {"name": tool, "arguments": arguments}});
    let response = raw_client()
        .post(url)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-06-18")
        .header("authorization", format!("Bearer {TOKEN}"))
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    (status, response.text().await.unwrap_or_default())
}

#[tokio::test]
async fn requests_over_the_concurrency_cap_get_503_until_a_slot_frees() {
    let workbench = Workbench::start().await;
    get_route(&api(&format!("/jobs/{JOB_ID}")), TOKEN)
        .respond_with(ResponseTemplate::new(200).set_body_json(job(
            JOB_ID,
            &demo_spec(),
            "running",
            None,
        )))
        .mount(&workbench.server)
        .await;
    let mcp = McpHttp::start(
        &workbench.url(),
        HttpOptions {
            max_concurrent_requests: 2,
            ..HttpOptions::default()
        },
    )
    .await;

    // Two three-second waits take both slots.
    let wait = json!({"job_id": JOB_ID, "timeout_secs": 3, "poll_interval_secs": 1});
    let first = tokio::spawn(raw_tool_call(mcp.url(), "wait_for_job", wait.clone()));
    let second = tokio::spawn(raw_tool_call(mcp.url(), "wait_for_job", wait));
    until_requests(&workbench, "GET", &api(&format!("/jobs/{JOB_ID}")), 2).await;

    let refused = post_initialize(&mcp, &[bearer(TOKEN)]).await;
    assert_eq!(refused.status(), 503);
    let header = |name: &str| {
        refused
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    };
    assert_eq!(header("retry-after"), "1");
    assert_eq!(header("x-content-type-options"), "nosniff");
    assert_eq!(header("cache-control"), "no-store");
    assert_eq!(error_code(refused).await, "unavailable");

    // Authentication still comes first: no token is 401, not 503.
    assert_eq!(post_initialize(&mcp, &[]).await.status(), 401);

    for task in [first, second] {
        let (status, body) = task.await.unwrap();
        assert_eq!(status, 200);
        assert!(body.contains("still running"), "{body}");
    }
    // Both slots are free again.
    assert_eq!(post_initialize(&mcp, &[bearer(TOKEN)]).await.status(), 200);
    assert_eq!(post_initialize(&mcp, &[bearer(TOKEN)]).await.status(), 200);
}

#[tokio::test]
async fn each_token_has_its_own_cap_on_concurrent_waits() {
    let workbench = Workbench::start().await;
    for token in [TOKEN, OTHER_TOKEN] {
        get_route(&api(&format!("/jobs/{JOB_ID}")), token)
            .respond_with(ResponseTemplate::new(200).set_body_json(job(
                JOB_ID,
                &demo_spec(),
                "running",
                None,
            )))
            .mount(&workbench.server)
            .await;
    }
    let mcp = McpHttp::start(
        &workbench.url(),
        HttpOptions {
            max_waits_per_token: 1,
            ..HttpOptions::default()
        },
    )
    .await;
    let alice = std::sync::Arc::new(mcp.client(TOKEN).await);
    let bob = mcp.client(OTHER_TOKEN).await;

    let long_wait = {
        let alice = std::sync::Arc::clone(&alice);
        tokio::spawn(async move {
            call(
                &alice,
                "wait_for_job",
                json!({"job_id": JOB_ID, "timeout_secs": 3, "poll_interval_secs": 1}),
            )
            .await
        })
    };
    until_requests(&workbench, "GET", &api(&format!("/jobs/{JOB_ID}")), 1).await;

    let short = json!({"job_id": JOB_ID, "timeout_secs": 1, "poll_interval_secs": 1});
    let refused = call(&alice, "wait_for_job", short.clone()).await;
    assert_eq!(refused.is_error, Some(true));
    assert!(
        text(&refused).starts_with("rate_limited: too many concurrent waits for this token"),
        "{}",
        text(&refused)
    );
    // Another caller is not affected.
    let other = call(&bob, "wait_for_job", short.clone()).await;
    assert_eq!(other.is_error, Some(false), "{}", text(&other));
    assert_eq!(payload(&other)["status"], "running");

    let first = long_wait.await.unwrap();
    assert_eq!(first.is_error, Some(false), "{}", text(&first));
    // The slot came back when the first wait ended.
    let again = call(&alice, "wait_for_job", short).await;
    assert_eq!(again.is_error, Some(false), "{}", text(&again));
}

#[tokio::test]
async fn the_risk_acknowledgement_setting_reaches_http_handlers() {
    let workbench = Workbench::start().await;
    workbench.mount_happy(TOKEN, "alice", "operator").await;
    for allow in [false, true] {
        let mcp = McpHttp::start(
            &workbench.url(),
            HttpOptions {
                allow_risk_acknowledgement: allow,
                ..HttpOptions::default()
            },
        )
        .await;
        let client = mcp.client(TOKEN).await;
        let info = payload(&call(&client, "server_info", json!({})).await);
        assert_eq!(
            info["mcp_server"]["policy"]["allow_risk_acknowledgement"],
            allow
        );
        let run = call(
            &client,
            "start_run",
            json!({"experiment": "e1", "cell": {"bench": "lab"},
                   "options": {"k": 1, "modes": ["parallel"]}, "profile": "local",
                   "acknowledge_risks": true}),
        )
        .await;
        assert_eq!(run.is_error, Some(!allow), "{}", text(&run));
    }
}
