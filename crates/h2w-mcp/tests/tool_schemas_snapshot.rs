//! Snapshot of every tool's name, title, description, annotations and input schema, so any
//! change to what MCP clients see shows up in review.
//!
//! After an intentional change, regenerate and review the diff:
//! `UPDATE_SNAPSHOTS=1 cargo test -p h2w-mcp --test tool_schemas_snapshot`

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::path::Path;

use common::*;
use serde_json::{Value, json};

#[tokio::test]
async fn tool_schemas_match_the_snapshot() {
    // tools/list needs no Workbench server.
    let client = connect("http://127.0.0.1:9", TOKEN).await;
    let tools = client.list_all_tools().await.unwrap();
    let snapshot: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "title": t.title,
                "description": t.description,
                "annotations": t.annotations,
                "inputSchema": t.input_schema,
            })
        })
        .collect();
    let rendered = serde_json::to_string_pretty(&snapshot).unwrap() + "\n";
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/tool_schemas.json");
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, &rendered).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&file).unwrap_or_default();
    if expected != rendered {
        let first_difference = expected
            .lines()
            .zip(rendered.lines())
            .position(|(a, b)| a != b)
            .unwrap_or_else(|| expected.lines().count().min(rendered.lines().count()));
        panic!(
            "the tools MCP clients see changed (first difference at line {}). Review it and \
             accept it with: UPDATE_SNAPSHOTS=1 cargo test -p h2w-mcp --test \
             tool_schemas_snapshot",
            first_difference + 1
        );
    }
}
