#![cfg(windows)]

use std::io::Write;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use hangar_api::AppState;
use serde_json::Value;

fn wait_for_ready(state: &AppState) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let status = hangar_api::startup_status(state);
        match status.state.as_str() {
            "ready" => return,
            "failed" => panic!("test inventory failed to open: {}", status.message),
            _ if Instant::now() >= deadline => panic!("test inventory did not become ready"),
            _ => thread::sleep(Duration::from_millis(50)),
        }
    }
}

#[test]
fn installed_binary_serves_authenticated_stdio_reads() {
    let temp = tempfile::tempdir().expect("temporary inventory directory");
    let db_path = temp.path().join("codehangar.sqlite3");
    let state = AppState::open(&db_path).expect("start encrypted inventory open");
    wait_for_ready(&state);

    let project_id = hangar_api::projects_list(&state)
        .expect("list seeded projects")
        .first()
        .expect("seeded project")
        .id;
    hangar_api::start_local_automation(&state).expect("start local automation boundary");
    let credential = hangar_api::automation_register(
        &state,
        "stdio-release-smoke".to_string(),
        vec!["read_structure".to_string()],
        vec![project_id],
    )
    .expect("register scoped connector credential");

    let mut child = Command::new(env!("CARGO_BIN_EXE_code-hangar-mcp"))
        .env("CODEHANGAR_MCP_TOKEN", credential.token)
        .env("CODEHANGAR_DB_PATH", &db_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn packaged MCP entry point");

    {
        let stdin = child.stdin.as_mut().expect("child stdin");
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2025-03-26"}}}}"#).unwrap();
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":2,"method":"tools/list"}}"#).unwrap();
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{{"name":"list_catalog","arguments":{{}}}}}}"#).unwrap();
    }
    drop(child.stdin.take());

    let output = child.wait_with_output().expect("collect MCP output");
    assert!(
        output.status.success(),
        "MCP process failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let responses: Vec<Value> = String::from_utf8(output.stdout)
        .expect("UTF-8 MCP output")
        .lines()
        .map(|line| serde_json::from_str(line).expect("JSON-RPC response"))
        .collect();

    assert_eq!(responses.len(), 3);
    assert_eq!(responses[0]["result"]["serverInfo"]["name"], "code-hangar");
    assert!(responses[1]["result"]["tools"]
        .as_array()
        .expect("tool catalog")
        .iter()
        .any(|tool| tool["name"] == "list_catalog"));
    assert_eq!(responses[2]["result"]["isError"], false);
    assert!(responses[2]["result"]["content"][0]["text"]
        .as_str()
        .expect("catalog response text")
        .contains("projects"));
}
