//! Assist runs: one-off calls to the AI CLIs that already live on this
//! computer (claude / opencode / agy — picked in Settings → Assist). Kawruh
//! holds no keys and no provider accounts; the agents use their own.
//!
//! GUI launches don't inherit a user's PATH, so binaries are resolved through
//! a login shell and common install prefixes, then spawned by absolute path.
//! The working directory is a scratch folder under the data dir: everything
//! the agent needs travels inside the prompt, so it never has to touch the
//! user's files.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;

use regex::Regex;
use serde::Serialize;
use tauri::Manager;
use tauri::State;
use tauri::ipc::Channel;

use crate::commands::AppState;
use crate::error::{Error, Result};

/// An assist agent: its binary and the flags that run one prompt headlessly.
struct Agent {
    id: &'static str,
    label: &'static str,
    bin: &'static str,
    args: &'static [&'static str],
}

/// `claude -p` and `agy -p` answer in print mode; `opencode run` is its
/// non-interactive mode, and `--format json` keeps tool activity (it loves to
/// read skills and run shell commands) out of the answer we insert.
const AGENTS: &[Agent] = &[
    Agent {
        id: "claude",
        label: "Claude Code",
        bin: "claude",
        args: &["-p"],
    },
    Agent {
        id: "opencode",
        label: "OpenCode",
        bin: "opencode",
        args: &["run", "--format", "json"],
    },
    Agent {
        id: "agy",
        label: "Antigravity",
        bin: "agy",
        args: &["-p"],
    },
];

const TIMEOUT: Duration = Duration::from_secs(180);
/// Very long agent answers are cut so the webview gets a manageable string.
const MAX_OUTPUT: usize = 256 * 1024;

/// What the `assist_run` channel tells the webview: the run id first, then
/// either the agent's output or an error.
#[derive(Serialize, Clone)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AssistEvent {
    Start { id: String },
    Text { text: String },
    Error { message: String },
}

/// Live assist runs, keyed by id, so a closed panel can stop its agent.
#[derive(Default)]
pub struct Worker {
    pub jobs: Mutex<HashMap<String, tauri::async_runtime::JoinHandle<()>>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    id: &'static str,
    label: &'static str,
    path: String,
    available: bool,
}

/// The installed agents and where the one that will run Assist is.
#[tauri::command]
pub fn agents_list() -> Result<Vec<AgentInfo>> {
    Ok(AGENTS
        .iter()
        .map(|a| {
            let path = command_path(a.bin);
            AgentInfo {
                id: a.id,
                label: a.label,
                path: path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
                available: path.is_some(),
            }
        })
        .collect())
}

/// Runs one prompt through an agent and streams the outcome back on the
/// channel; returns the run id (pass it to `assist_cancel` to stop it).
#[tauri::command]
pub async fn assist_run(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    agent: String,
    prompt: String,
    channel: Channel<AssistEvent>,
) -> Result<String> {
    let a = AGENTS
        .iter()
        .find(|a| a.id == agent.trim())
        .ok_or_else(|| Error::msg(format!("Unknown agent: {agent}")))?;
    let path = command_path(a.bin).ok_or_else(|| {
        Error::msg(format!(
            "The agent isn't available: {} isn't installed, or isn't on the PATH of a login shell.",
            a.label
        ))
    })?;
    let cwd = state.data_dir.join("agent-sandbox");
    std::fs::create_dir_all(&cwd)
        .map_err(|e| Error::msg(format!("Couldn't prepare the agent sandbox: {e}")))?;

    let id = uuid::Uuid::new_v4().to_string();
    let handle = tauri::async_runtime::spawn({
        let app = app.clone();
        let id = id.clone();
        let path = path.clone();
        let agent = a;
        async move {
            let _ = channel.send(AssistEvent::Start { id: id.clone() });
            let res = run_agent(&path, agent, &prompt, &cwd, TIMEOUT).await;
            let _ = match res {
                Ok(text) => channel.send(AssistEvent::Text { text }),
                Err(e) => channel.send(AssistEvent::Error {
                    message: e.to_string(),
                }),
            };
            app.state::<AppState>()
                .assists
                .jobs
                .lock()
                .unwrap()
                .remove(&id);
        }
    });
    state
        .assists
        .jobs
        .lock()
        .unwrap()
        .insert(id.clone(), handle);
    Ok(id)
}

/// Stops a run (its process dies with the task — see `kill_on_drop`).
#[tauri::command]
pub fn assist_cancel(state: State<'_, AppState>, run_id: String) -> Result<()> {
    if let Some(handle) = state.assists.jobs.lock().unwrap().remove(&run_id) {
        handle.abort();
    }
    Ok(())
}

async fn run_agent(
    path: &std::path::Path,
    agent: &Agent,
    prompt: &str,
    cwd: &std::path::Path,
    timeout: Duration,
) -> Result<String> {
    let label = agent.label;
    // Command lines built with argument arrays can't inject a shell; the
    // prompt only needs guarding against being read as the first flag, which
    // the fixed "You are …" prompt openings already prevent.
    #[cfg(target_os = "windows")]
    let mut command = tokio::process::Command::new("cmd");
    #[cfg(target_os = "windows")]
    command.arg("/C").arg(format!(
        "\"{}\" {} {}",
        path.display(),
        agent.args.join(" "),
        cmd_quote(prompt)
    ));
    #[cfg(not(target_os = "windows"))]
    let mut command = {
        let mut command = tokio::process::Command::new(path);
        command.args(agent.args.iter().copied()).arg(prompt);
        command
    };
    command
        .current_dir(cwd)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let out = tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| {
            Error::msg(format!(
                "{label} didn't answer within {} minutes. Try a shorter piece of text.",
                timeout.as_secs() / 60
            ))
        })?
        .map_err(|e| Error::msg(format!("Couldn't start {label}: {e}")))?;
    if !out.status.success() {
        let stderr = clean(&String::from_utf8_lossy(&out.stderr));
        let tail = stderr.lines().last().unwrap_or("").trim();
        let detail = if tail.is_empty() {
            "no output".into()
        } else {
            last_bytes(tail, 200)
        };
        return Err(Error::msg(format!("{label} failed: {detail}")));
    }
    let raw = last_bytes(
        clean(&String::from_utf8_lossy(&out.stdout)).trim(),
        MAX_OUTPUT,
    );
    Ok(if agent.id == "opencode" {
        // Its answer arrives as NDJSON message parts; tool activity (OpenCode
        // likes to read skills and run shell commands) never reaches the text
        // parts, so the markdown we insert stays clean.
        opencode_text(&raw).unwrap_or(raw)
    } else {
        raw
    })
}

/// OpenCode's `--format json`: one event per line, the answer being every
/// `text` of `text` message parts joined. `None` when there was no such part
/// — callers then fall back to the plain stdout.
fn opencode_text(stdout: &str) -> Option<String> {
    let mut texts = Vec::new();
    for line in stdout.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("type").and_then(|t| t.as_str()) != Some("text") {
            continue;
        }
        let text = value
            .get("part")
            .and_then(|p| p.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("");
        if !text.is_empty() {
            texts.push(text.to_string());
        }
    }
    (!texts.is_empty()).then(|| texts.join(""))
}

#[cfg(target_os = "windows")]
/// Quotes an argument for the `cmd /C` line npm shims (`.cmd`) need.
fn cmd_quote(args: &str) -> String {
    format!("\"{}\"", args.replace('"', "\"\""))
}

/// Fully resolves `bin`: the login shell PATH first (a GUI app starts with a
/// bare PATH), then the private prefixes installers like Homebrew write.
/// Shared with MCP agent registration.
pub(crate) fn command_path(bin: &str) -> Option<PathBuf> {
    via_shell(bin).or_else(|| fallback_bins(bin))
}

#[cfg(not(target_os = "windows"))]
fn via_shell(bin: &str) -> Option<PathBuf> {
    // The names come from the fixed table above, never from user input.
    let script = format!("command -v {bin}");
    for shell in ["zsh", "bash", "sh"] {
        let Ok(out) = std::process::Command::new(shell)
            .arg("-l")
            .arg("-c")
            .arg(&script)
            .stdin(Stdio::null())
            .output()
        else {
            continue;
        };
        if !out.status.success() {
            continue;
        }
        let found = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if found.contains('/') {
            return Some(found.into());
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn via_shell(bin: &str) -> Option<PathBuf> {
    let Ok(out) = std::process::Command::new("cmd")
        .args(["/C", "where", bin])
        .stdin(Stdio::null())
        .output()
    else {
        return None;
    };
    if !out.status.success() {
        return None;
    }
    let first = String::from_utf8_lossy(&out.stdout).lines().next()?.trim();
    if first.contains('\\') {
        Some(first.into())
    } else {
        None
    }
}

/// Prefixes a bare app PATH misses: Homebrew, cargo, npm, pnpm, bun shims.
fn fallback_bins(bin: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)?;
    let mut dirs = vec!["/opt/homebrew/bin".into(), "/usr/local/bin".into()];
    for rel in [".cargo/bin", ".bun/bin", ".local/bin", ".npm-global/bin"] {
        dirs.push(home.join(rel));
    }
    #[cfg(target_os = "windows")]
    if let Some(appdata) = std::env::var_os("APPDATA") {
        dirs.push(PathBuf::from(appdata).join("npm"));
    }
    for dir in dirs {
        for ext in ["", std::env::consts::EXE_SUFFIX] {
            let path = dir.join(format!("{bin}{ext}"));
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

/// Drops ANSI escapes and bells; some agents colour stdout even when piped.
fn clean(text: &str) -> String {
    static CSI: Mutex<Option<Regex>> = Mutex::new(None);
    let guard = &mut *CSI.lock().unwrap();
    let re = guard.get_or_insert_with(|| {
        Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x07").unwrap()
    });
    re.replace_all(text, "").to_string()
}

/// Keeps at most `max` bytes from the end, on a character boundary.
fn last_bytes(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut start = text.len() - max;
    while start > 0 && !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_string()
}

#[cfg(test)]
mod tests {
    use super::{AGENTS, clean, last_bytes, opencode_text};

    #[test]
    fn opencode_text_pulls_the_answer_only() {
        let events = concat!(
            r#"{"type":"step_start","part":{"type":"step-start"}}"#,
            "\n",
            r#"{"type":"text","part":{"type":"text","text":"Saya suka masakan Padang."}}"#,
            "\n",
            r#"{"type":"step_finish","part":{"type":"step-finish"}}"#,
            "\n",
        );
        assert_eq!(
            opencode_text(events).as_deref(),
            Some("Saya suka masakan Padang.")
        );
        assert_eq!(opencode_text("not json"), None);
    }

    #[test]
    fn clean_drops_escapes_but_keeps_text() {
        assert_eq!(clean("\x1b[31mred\x1b[0m"), "red");
        assert_eq!(clean("a\x07b\x1b]0;title\x07c"), "abc");
        assert_eq!(clean("plain text"), "plain text");
    }

    #[test]
    fn last_bytes_cut_not_mid_character() {
        assert_eq!(last_bytes("hello", 3), "llo");
        let e = "é".repeat(150);
        let cut = last_bytes(&e, 5);
        // A char-boundary cut stays a multiple of the é byte length (2).
        assert_eq!(cut.len() % 2, 0);
        assert!(e.ends_with(&cut));
    }

    #[test]
    fn table_has_the_three_clis() {
        let ids: Vec<_> = AGENTS.iter().map(|a| a.id).collect();
        assert_eq!(ids, ["claude", "opencode", "agy"]);
    }
}
