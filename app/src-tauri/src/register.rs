//! Wiring this computer's agent CLIs to the local MCP endpoint. Registration
//! writes only Mac-local config files (claude's `~/.claude.json`, opencode's
//! `opencode.json`, agy's own CLI), and only ever touches this app's server
//! entry — the agents' other servers stay as they are. Status checks are
//! read-only; connect/disconnect run when the user clicks, so E2E drives
//! neither.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde::Serialize;
use serde_json::Value;
use tauri::State;

use crate::commands::AppState;
use crate::error::{Error, Result};
use crate::mcp_server::port_configured;

/// The one server we manage in every agent config.
pub const SERVER_NAME: &str = "kawruh-local";

#[derive(Clone, Copy)]
struct Agent {
    id: &'static str,
    label: &'static str,
    bin: &'static str,
}

const AGENTS: &[Agent] = &[
    Agent {
        id: "claude",
        label: "Claude Code",
        bin: "claude",
    },
    Agent {
        id: "opencode",
        label: "OpenCode",
        bin: "opencode",
    },
    Agent {
        id: "agy",
        label: "Antigravity",
        bin: "agy",
    },
];

fn agent_by_id(id: &str) -> Result<&'static Agent> {
    AGENTS
        .iter()
        .find(|a| a.id == id)
        .ok_or_else(|| Error::msg(format!("no agent named \"{id}\"")))
}

/// A registration the Settings tab shows a Connect button for.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Registration {
    pub id: &'static str,
    pub label: &'static str,
    /// The CLI exists on this computer.
    pub available: bool,
    /// Its config points this server at our endpoint right now.
    pub connected: bool,
}

fn for_agent(a: &Agent, url: &str) -> Registration {
    let available = crate::agents::command_path(a.bin).is_some();
    let connected = match a.id {
        "claude" => claude_connected(url),
        "opencode" => opencode_connected(url),
        "agy" => agy_connected(url),
        _ => false,
    };
    Registration {
        id: a.id,
        label: a.label,
        available,
        connected,
    }
}

// ---------------------------------------------------------------- status

fn claude_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|h| h.join(".claude.json"))
}

fn opencode_path() -> Option<PathBuf> {
    let dir = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(d) => PathBuf::from(d),
        None => PathBuf::from(std::env::var_os("HOME")?).join(".config"),
    };
    Some(dir.join("opencode").join("opencode.json"))
}

fn read_json(path: &PathBuf) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Whether the claude config still carries a kawruh-local entry of any kind.
fn claude_entry_exists() -> bool {
    claude_path()
        .and_then(|p| read_json(&p))
        .and_then(|v| Some(v.get("mcpServers")?.get(SERVER_NAME).is_some()))
        .unwrap_or(false)
}

/// claude keeps user-scope MCP servers in `mcpServers` of `~/.claude.json`.
fn claude_connected(url: &str) -> bool {
    claude_path()
        .and_then(|p| read_json(&p))
        .and_then(|v| {
            let entry = v.get("mcpServers")?.get(SERVER_NAME)?;
            Some(entry.get("url").and_then(|u| u.as_str()) == Some(url))
        })
        .unwrap_or(false)
}

/// opencode (v2) nests remote servers under `mcp.servers`; older configs had
/// them directly under `mcp` — counted either way, written only the new way.
fn opencode_entry<'v>(v: &'v Value) -> Option<&'v Value> {
    v.pointer("/mcp/servers")?
        .get(SERVER_NAME)
        .or_else(|| v.get("mcp")?.get(SERVER_NAME))
}

fn opencode_connected(url: &str) -> bool {
    opencode_path()
        .and_then(|p| read_json(&p))
        .and_then(|v| Some(opencode_entry(&v)?.get("url").and_then(|u| u.as_str()) == Some(url)))
        .unwrap_or(false)
}

/// agy's only status surface is a plain `mcp list`.
fn agy_connected(url: &str) -> bool {
    crate::agents::command_path("agy")
        .and_then(|path| {
            Command::new(path)
                .args(["mcp", "list"])
                .stdin(Stdio::null())
                .output()
                .ok()
        })
        .map(|out| {
            let out = String::from_utf8_lossy(&out.stdout);
            out.contains(url) || out.contains(SERVER_NAME)
        })
        .unwrap_or(false)
}

// ---------------------------------------------------------------- connect

fn run(bin: &str, args: &[&str]) -> Result<()> {
    let path = crate::agents::command_path(bin)
        .ok_or_else(|| Error::msg(format!("{bin} CLI not found on this computer")))?;
    let out = Command::new(path)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| Error::msg(format!("running {bin} failed: {e}")))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(Error::msg(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ))
    }
}

fn claude_add(url: &str, token: &str) -> Result<()> {
    run(
        "claude",
        &[
            "mcp",
            "add",
            "--scope",
            "user",
            "--transport",
            "http",
            SERVER_NAME,
            url,
            "--header",
            &format!("Authorization: Bearer {token}"),
        ],
    )
}

fn claude_remove() -> Result<()> {
    let out = run("claude", &["mcp", "remove", SERVER_NAME, "--scope", "user"]);
    // A remove that only complains the entry is not there is a success: the
    // config landed (and stays) where we wanted it.
    if out.is_ok() || !claude_entry_exists() {
        Ok(())
    } else {
        out
    }
}

fn agy_add(url: &str, token: &str) -> Result<()> {
    // agy takes flags before the server name; the http URL needs no --type.
    run(
        "agy",
        &[
            "mcp",
            "add",
            "--header",
            &format!("Authorization: Bearer {token}"),
            SERVER_NAME,
            url,
        ],
    )
}

fn agy_remove() -> Result<()> {
    run("agy", &["mcp", "remove", SERVER_NAME])
}

/// Merges the kawruh-local entry into (or out of) the opencode config,
/// preserving everything else in the file.
fn opencode_write(url: &str, token: &str, connect: bool) -> Result<()> {
    let path =
        opencode_path().ok_or_else(|| Error::msg("cannot find the OpenCode config directory"))?;
    let mut root = read_json(&path).unwrap_or_else(|| Value::Object(Default::default()));
    let obj = root
        .as_object_mut()
        .ok_or_else(|| Error::msg("opencode.json is not a JSON object"))?;
    // The legacy direct mapping is gone upstream: our name lives only under
    // `mcp.servers` from here on.
    let mcp = obj
        .entry("mcp")
        .or_insert_with(|| Value::Object(Default::default()));
    let Some(mcp) = mcp.as_object_mut() else {
        return Err(Error::msg(
            "the \"mcp\" entry in opencode.json is not an object",
        ));
    };
    mcp.remove(SERVER_NAME);
    let servers = match mcp.get_mut("servers") {
        Some(Value::Object(s)) => s,
        None => {
            mcp.insert("servers".into(), Value::Object(Default::default()));
            mcp.get_mut("servers")
                .and_then(Value::as_object_mut)
                .expect("just inserted")
        }
        Some(_) => {
            return Err(Error::msg(
                "the \"mcp.servers\" entry in opencode.json is not an object",
            ));
        }
    };
    if connect {
        servers.insert(
            SERVER_NAME.into(),
            serde_json::json!({
                "type": "remote",
                "url": url,
                "headers": { "Authorization": format!("Bearer {token}") },
                "oauth": false,
                "disabled": false,
            }),
        );
    } else {
        servers.remove(SERVER_NAME);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error::msg(format!("mkdir failed: {e}")))?;
    }
    std::fs::write(
        &path,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&root)
                .map_err(|e| Error::msg(format!("rewriting opencode.json failed: {e}")))?
        ),
    )
    .map_err(|e| Error::msg(format!("writing opencode.json failed: {e}")))?;
    Ok(())
}

fn add(a: &Agent, url: &str, token: &str) -> Result<()> {
    match a.id {
        "claude" => claude_add(url, token),
        "opencode" => opencode_write(url, token, true),
        "agy" => agy_add(url, token),
        _ => Ok(()),
    }
}

fn remove(a: &Agent) -> Result<()> {
    match a.id {
        "claude" => claude_remove(),
        "opencode" => opencode_write("", "", false),
        "agy" => agy_remove(),
        _ => Ok(()),
    }
}

fn url_for(state: &AppState) -> String {
    format!(
        "http://127.0.0.1:{}/mcp",
        state.mcp.port().unwrap_or_else(port_configured)
    )
}

// ------------------------------------------------------------ tauri commands

/// How every known agent stands, cheap reads only.
#[tauri::command]
pub async fn mcp_status(state: State<'_, AppState>) -> Result<Vec<Registration>> {
    let url = url_for(&state);
    tauri::async_runtime::spawn_blocking(move || {
        AGENTS.iter().map(|a| for_agent(a, &url)).collect()
    })
    .await
    .map_err(|e| Error::msg(format!("status check failed: {e}")))
}

/// Connects one agent to (or disconnects it from) the local endpoint, then
/// returns the fresh standings of all of them.
#[tauri::command]
pub async fn mcp_register(
    state: State<'_, AppState>,
    agent: String,
    connect: bool,
) -> Result<Vec<Registration>> {
    let a = agent_by_id(&agent)?;
    let label = a.label;
    let url = url_for(&state);
    let token = state.mcp.token()?;
    tauri::async_runtime::spawn_blocking(move || {
        if connect {
            add(a, &url, &token)
        } else {
            remove(a)
        }
    })
    .await
    .map_err(|e| Error::msg(format!("registration failed: {e}")))?
    .map_err(|e| Error::msg(format!("Cannot update {label}: {e}")))?;
    // Not the spawn_blocking's return: a fresh read of whatever the CLI wrote.
    mcp_status(state).await
}
