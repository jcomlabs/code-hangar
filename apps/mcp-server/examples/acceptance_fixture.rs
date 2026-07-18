#![cfg(windows)]

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hangar_agent::{AgentMethod, AgentRequest, PROTOCOL_VERSION};
use hangar_api::AppState;
use hangar_appconfig::{Host, ServerSpec};
use serde_json::{json, Value};

const FIXTURE_FILE: &str = "fixture.json";

fn main() {
    if let Err(error) = run() {
        eprintln!("MCP acceptance fixture failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.as_slice() {
        [action, root, server] if action == "prepare" => {
            prepare(Path::new(root), Path::new(server))
        }
        [action, fixture] if action == "audit" => audit(Path::new(fixture)),
        [action, fixture, host, methods @ ..]
            if action == "audit-host" && !methods.is_empty() =>
        {
            audit_host(Path::new(fixture), host, methods)
        }
        [action, fixture] if action == "disconnect" => disconnect(Path::new(fixture)),
        _ => Err(
            "usage: acceptance_fixture prepare <root> <server-exe> | audit <fixture.json> | audit-host <fixture.json> <host> <method...> | disconnect <fixture.json>"
                .to_string(),
        ),
    }
}

fn wait_for_ready(state: &AppState) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let status = hangar_api::startup_status(state);
        match status.state.as_str() {
            "ready" => return Ok(()),
            "failed" => return Err(format!("inventory failed to open: {}", status.message)),
            _ if Instant::now() >= deadline => {
                return Err("inventory did not become ready within 30 seconds".to_string())
            }
            _ => thread::sleep(Duration::from_millis(50)),
        }
    }
}

fn host_from_id(id: &str) -> Result<Host, String> {
    Host::from_id(id).ok_or_else(|| format!("unknown fixture host: {id}"))
}

fn prepare(root: &Path, server: &Path) -> Result<(), String> {
    let root = absolute(root)?;
    let fixture_path = root.join(FIXTURE_FILE);
    if fixture_path.exists() {
        return Err(format!(
            "refusing to overwrite an existing fixture: {}",
            fixture_path.display()
        ));
    }
    let server = absolute(server)?;
    if !server.is_file() {
        return Err(format!("MCP server not found: {}", server.display()));
    }
    if server.file_name().and_then(|name| name.to_str()) != Some(hangar_appconfig::SERVER_EXE_NAME)
    {
        return Err(format!(
            "expected {}, got {}",
            hangar_appconfig::SERVER_EXE_NAME,
            server.display()
        ));
    }

    fs::create_dir_all(&root).map_err(to_message)?;
    let home = root.join("home");
    let db_path = root.join("codehangar.sqlite3");
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(to_message)?
        .as_millis();
    let sentinel = format!("CODEHANGAR_SENTINEL_{timestamp_ms}");
    seed_sentinel_configs(&home, &sentinel)?;

    let state = AppState::open(&db_path)?;
    wait_for_ready(&state)?;
    hangar_api::start_local_automation(&state)?;
    let project = hangar_api::projects_list(&state)?
        .into_iter()
        .next()
        .ok_or_else(|| "fixture database contains no project".to_string())?;

    let mut clients = serde_json::Map::new();
    for host in Host::ALL {
        let credential = hangar_api::automation_register(
            &state,
            host.label().to_string(),
            vec!["read_structure".to_string()],
            vec![project.id],
        )?;
        let spec = ServerSpec {
            command: server.to_string_lossy().to_string(),
            args: Vec::new(),
            env: vec![
                ("CODEHANGAR_MCP_TOKEN".to_string(), credential.token.clone()),
                (
                    "CODEHANGAR_DB_PATH".to_string(),
                    db_path.to_string_lossy().to_string(),
                ),
            ],
            startup_timeout_sec: 20,
        };
        if let Err(error) = hangar_appconfig::register(host, &home, &spec) {
            let _ = hangar_api::automation_revoke(&state, credential.agent.id);
            return Err(format!("{} registration failed: {error}", host.label()));
        }
        clients.insert(
            host.id().to_string(),
            json!({
                "agentId": credential.agent.id,
                "agentName": credential.agent.name,
                "token": credential.token,
                "configPath": hangar_appconfig::host_config_path(host, &home),
            }),
        );
    }
    verify_config_state(&home, &sentinel, true)?;

    let fixture = json!({
        "schemaVersion": 1,
        "root": root,
        "home": home,
        "dbPath": db_path,
        "serverPath": server,
        "sentinel": sentinel,
        "project": { "id": project.id, "name": project.name },
        "clients": Value::Object(clients),
    });
    write_json(&fixture_path, &fixture)?;
    println!(
        "Prepared MCP acceptance fixture at {}",
        fixture_path.display()
    );
    Ok(())
}

fn audit(fixture_path: &Path) -> Result<(), String> {
    let fixture_path = absolute(fixture_path)?;
    let fixture = read_json(&fixture_path)?;
    let home = fixture_path_value(&fixture, "home")?;
    let db_path = fixture_path_value(&fixture, "dbPath")?;
    let sentinel = fixture_string(&fixture, "sentinel")?;
    verify_config_state(&home, sentinel, true)?;

    let state = AppState::open(&db_path)?;
    wait_for_ready(&state)?;
    let activity = hangar_api::automation_activity(&state, Some(1000))?;
    let clients = fixture_clients(&fixture)?;
    let mut host_results = serde_json::Map::new();
    let mut missing_reads = Vec::new();
    for (host_id, client) in clients {
        let agent_name = client
            .get("agentName")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{host_id} is missing agentName"))?;
        let read = activity.iter().find(|entry| {
            entry.agent_name.as_deref() == Some(agent_name)
                && entry.method == "list_catalog"
                && entry.status == "allowed"
        });
        host_results.insert(
            host_id.clone(),
            json!({
                "agentName": agent_name,
                "listCatalogAllowed": read.is_some(),
                "activityId": read.map(|entry| entry.id),
                "createdAt": read.map(|entry| entry.created_at.clone()),
            }),
        );
        if read.is_none() {
            missing_reads.push(host_id.clone());
        }
    }
    let report = json!({
        "schemaVersion": 1,
        "status": if missing_reads.is_empty() { "PASS" } else { "FAIL" },
        "fixture": fixture_path,
        "missingReads": missing_reads.clone(),
        "clients": Value::Object(host_results),
    });
    let output = fixture_path
        .parent()
        .ok_or_else(|| "fixture path has no parent".to_string())?
        .join("mcp-audit.json");
    write_json(&output, &report)?;
    if !missing_reads.is_empty() {
        return Err(format!(
            "missing allowed list_catalog audit entries for: {}. Partial evidence: {}",
            missing_reads.join(", "),
            output.display()
        ));
    }
    println!("MCP client reads verified at {}", output.display());
    Ok(())
}

fn audit_host(
    fixture_path: &Path,
    host_id: &str,
    required_methods: &[String],
) -> Result<(), String> {
    host_from_id(host_id)?;
    let fixture_path = absolute(fixture_path)?;
    let fixture = read_json(&fixture_path)?;
    let home = fixture_path_value(&fixture, "home")?;
    let db_path = fixture_path_value(&fixture, "dbPath")?;
    let sentinel = fixture_string(&fixture, "sentinel")?;
    verify_config_state(&home, sentinel, true)?;

    let client = fixture_clients(&fixture)?
        .get(host_id)
        .ok_or_else(|| format!("fixture has no client for {host_id}"))?;
    let agent_name = client
        .get("agentName")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{host_id} is missing agentName"))?;
    let state = AppState::open(&db_path)?;
    wait_for_ready(&state)?;
    let activity = hangar_api::automation_activity(&state, Some(1000))?;

    let mut method_results = serde_json::Map::new();
    let mut missing = Vec::new();
    for method in required_methods {
        let read = activity.iter().find(|entry| {
            entry.agent_name.as_deref() == Some(agent_name)
                && entry.method == *method
                && entry.status == "allowed"
        });
        method_results.insert(
            method.clone(),
            json!({
                "allowed": read.is_some(),
                "activityId": read.map(|entry| entry.id),
                "createdAt": read.map(|entry| entry.created_at.clone()),
            }),
        );
        if read.is_none() {
            missing.push(method.clone());
        }
    }

    let report = json!({
        "schemaVersion": 1,
        "status": if missing.is_empty() { "PASS" } else { "FAIL" },
        "fixture": fixture_path,
        "host": host_id,
        "agentName": agent_name,
        "requiredMethods": required_methods,
        "missingMethods": missing,
        "methods": Value::Object(method_results),
    });
    let output = fixture_path
        .parent()
        .ok_or_else(|| "fixture path has no parent".to_string())?
        .join(format!("mcp-audit-{host_id}.json"));
    write_json(&output, &report)?;
    if !missing.is_empty() {
        return Err(format!(
            "{host_id} did not complete required MCP methods: {}. Partial evidence: {}",
            missing.join(", "),
            output.display()
        ));
    }
    println!("MCP methods for {host_id} verified at {}", output.display());
    Ok(())
}

fn disconnect(fixture_path: &Path) -> Result<(), String> {
    let fixture_path = absolute(fixture_path)?;
    let fixture = read_json(&fixture_path)?;
    let home = fixture_path_value(&fixture, "home")?;
    let db_path = fixture_path_value(&fixture, "dbPath")?;
    let sentinel = fixture_string(&fixture, "sentinel")?;
    let clients = fixture_clients(&fixture)?;

    let state = AppState::open(&db_path)?;
    wait_for_ready(&state)?;
    let mut host_results = serde_json::Map::new();
    for (host_id, client) in clients {
        let host = host_from_id(host_id)?;
        let agent_id = client
            .get("agentId")
            .and_then(Value::as_i64)
            .ok_or_else(|| format!("{host_id} is missing agentId"))?;
        let token = client
            .get("token")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{host_id} is missing token"))?;

        let revoked = hangar_api::automation_revoke(&state, agent_id)?;
        let removed = hangar_appconfig::unregister(host, &home)?;
        let denied = !hangar_api::dispatch_agent_request(
            &state,
            AgentRequest {
                protocol: PROTOCOL_VERSION.to_string(),
                request_id: format!("revoked-{host_id}"),
                token: Some(token.to_string()),
                method: AgentMethod::ListCatalog,
                params: json!({}),
            },
        )
        .ok;
        if !revoked || !removed || !denied {
            return Err(format!(
                "{host_id} disconnect incomplete: revoked={revoked}, removed={removed}, oldTokenDenied={denied}"
            ));
        }
        host_results.insert(
            host_id.clone(),
            json!({
                "credentialRevoked": revoked,
                "configEntryRemoved": removed,
                "oldTokenDenied": denied,
            }),
        );
    }

    verify_config_state(&home, sentinel, false)?;
    let enabled_ids: Vec<i64> = hangar_api::automation_agents(&state)?
        .into_iter()
        .filter(|agent| agent.enabled)
        .map(|agent| agent.id)
        .collect();
    if !enabled_ids.is_empty() {
        return Err(format!(
            "fixture still has enabled agents after disconnect: {enabled_ids:?}"
        ));
    }

    let report = json!({
        "schemaVersion": 1,
        "status": "PASS",
        "fixture": fixture_path,
        "sentinelPreserved": true,
        "enabledAgents": 0,
        "clients": Value::Object(host_results),
    });
    let output = fixture_path
        .parent()
        .ok_or_else(|| "fixture path has no parent".to_string())?
        .join("mcp-disconnect.json");
    write_json(&output, &report)?;
    println!(
        "MCP clients disconnected and revoked at {}",
        output.display()
    );
    Ok(())
}

fn seed_sentinel_configs(home: &Path, sentinel: &str) -> Result<(), String> {
    fs::create_dir_all(home.join(".cursor")).map_err(to_message)?;
    fs::create_dir_all(home.join(".codex")).map_err(to_message)?;
    write_json(
        &home.join(".claude.json"),
        &json!({
            "codehangarAcceptanceSentinel": sentinel,
            "unrelated": { "keep": true, "count": 17 },
            "mcpServers": {
                "sentinel-existing": {
                    "command": r"C:\Windows\System32\where.exe",
                    "args": ["codehangar-sentinel-never-run"]
                }
            }
        }),
    )?;
    write_json(
        &home.join(".cursor").join("mcp.json"),
        &json!({
            "codehangarAcceptanceSentinel": sentinel,
            "unrelated": { "keep": true, "count": 23 },
            "mcpServers": {}
        }),
    )?;
    fs::write(
        home.join(".codex").join("config.toml"),
        format!(
            "# {sentinel}\ncodehangar_acceptance_sentinel = \"{sentinel}\"\nunrelated_number = 29\n\n[mcp_servers.sentinel-existing]\ncommand = \"C:\\\\Windows\\\\System32\\\\where.exe\"\nargs = [\"codehangar-sentinel-never-run\"]\nenabled = false\n"
        ),
    )
    .map_err(to_message)?;
    Ok(())
}

fn verify_config_state(home: &Path, sentinel: &str, registered: bool) -> Result<(), String> {
    for host in Host::ALL {
        let status = hangar_appconfig::status(host, home);
        if !status.readable || status.registered != registered {
            return Err(format!(
                "{} config state mismatch: readable={}, registered={}, expected={registered}",
                host.label(),
                status.readable,
                status.registered
            ));
        }
    }

    let claude = read_json(&home.join(".claude.json"))?;
    let cursor = read_json(&home.join(".cursor").join("mcp.json"))?;
    for (label, value, count) in [("Claude", claude, 17), ("Cursor", cursor, 23)] {
        if value["codehangarAcceptanceSentinel"] != sentinel
            || value["unrelated"]["keep"] != true
            || value["unrelated"]["count"] != count
        {
            return Err(format!("{label} sentinel content changed"));
        }
    }
    let codex = fs::read_to_string(home.join(".codex").join("config.toml")).map_err(to_message)?;
    for expected in [
        format!("# {sentinel}"),
        format!("codehangar_acceptance_sentinel = \"{sentinel}\""),
        "unrelated_number = 29".to_string(),
        "[mcp_servers.sentinel-existing]".to_string(),
        "codehangar-sentinel-never-run".to_string(),
    ] {
        if !codex.contains(&expected) {
            return Err(format!("Codex sentinel content lost: {expected}"));
        }
    }
    Ok(())
}

fn fixture_clients(fixture: &Value) -> Result<&serde_json::Map<String, Value>, String> {
    fixture
        .get("clients")
        .and_then(Value::as_object)
        .ok_or_else(|| "fixture is missing clients".to_string())
}

fn fixture_string<'a>(fixture: &'a Value, key: &str) -> Result<&'a str, String> {
    fixture
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("fixture is missing {key}"))
}

fn fixture_path_value(fixture: &Value, key: &str) -> Result<PathBuf, String> {
    Ok(PathBuf::from(fixture_string(fixture, key)?))
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text = fs::read_to_string(path).map_err(to_message)?;
    serde_json::from_str(&text).map_err(to_message)
}

fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(to_message)?;
    }
    let mut text = serde_json::to_string_pretty(value).map_err(to_message)?;
    text.push('\n');
    fs::write(path, text).map_err(to_message)
}

fn absolute(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        env::current_dir()
            .map_err(to_message)
            .map(|cwd| cwd.join(path))
    }
}

fn to_message(error: impl std::fmt::Display) -> String {
    error.to_string()
}
