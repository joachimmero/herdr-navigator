//! The herdr CLI, which is the whole plugin API.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
#[cfg(unix)]
use std::time::Duration;

use serde_json::Value;

/// `HERDR_BIN_PATH` reaches the running herdr over its socket or pipe on every platform.
fn bin() -> OsString {
    std::env::var_os("HERDR_BIN_PATH").unwrap_or_else(|| "herdr".into())
}

/// The `result` of a herdr subcommand's JSON answer.
pub fn call(args: &[&str]) -> Result<Value, String> {
    let output = Command::new(bin())
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("could not run herdr: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("herdr {} failed: {}", args.join(" "), stderr.trim()));
    }
    let answer: Value = serde_json::from_slice(&output.stdout).map_err(|error| {
        format!("herdr {} answered in an unknown shape: {error}", args.join(" "))
    })?;
    Ok(answer.get("result").cloned().unwrap_or(Value::Null))
}

pub fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

pub fn plugin_id() -> String {
    var("HERDR_PLUGIN_ID").unwrap_or_else(|| "joachimmero.herdr-navigator".into())
}

/// The plugin's state dir. herdr hands it to the pane and to actions but not to a shell running
/// the binary directly, so there it is rebuilt from herdr's layout.
pub fn state_dir() -> Result<PathBuf, String> {
    if let Some(dir) = var("HERDR_PLUGIN_STATE_DIR") {
        return Ok(PathBuf::from(dir));
    }
    let state = var("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| home().map(|home| home.join(".local/state")))
        .ok_or("no plugin state dir: neither HERDR_PLUGIN_STATE_DIR nor a home directory is set")?;
    Ok(state.join("herdr/plugins").join(plugin_id()))
}

pub fn home() -> Option<PathBuf> {
    var("HOME").or_else(|| var("USERPROFILE")).map(PathBuf::from)
}

/// `id` keeping only the characters a file name can safely hold; herdr ids contain `:`.
pub fn file_safe(id: &str) -> String {
    id.chars().map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' }).collect()
}

/// An agent pane comments can be sent to, as the picker shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Agent {
    pub pane: String,
    pub name: String,
    pub state: String,
    /// The label of the tab it sits in, or empty.
    pub tab: String,
}

/// The agents in this pane's workspace other than this pane, in herdr's order.
pub fn agents() -> Result<Vec<Agent>, String> {
    let ws = var("HERDR_WORKSPACE_ID").ok_or("not inside herdr")?;
    let list = call(&["agent", "list"])?;
    let tabs = call(&["tab", "list", "--workspace", &ws]).unwrap_or_default();
    let labels: HashMap<&str, &str> = tabs["tabs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tab| Some((tab["tab_id"].as_str()?, tab["label"].as_str()?)))
        .collect();
    Ok(agents_in(&list, &ws, var("HERDR_PANE_ID").as_deref(), &labels))
}

/// `herdr agent list`'s panes running an agent in workspace `ws`, pane `me` left out.
fn agents_in(list: &Value, ws: &str, me: Option<&str>, tabs: &HashMap<&str, &str>) -> Vec<Agent> {
    fn text<'a>(entry: &'a Value, key: &str) -> Option<&'a str> {
        entry[key].as_str().filter(|text| !text.is_empty())
    }
    list["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| text(entry, "agent").is_some() && text(entry, "workspace_id") == Some(ws))
        .filter_map(|entry| {
            let pane = text(entry, "pane_id").filter(|pane| Some(*pane) != me)?;
            let name = ["name", "display_agent", "agent"]
                .into_iter()
                .find_map(|key| text(entry, key))
                .unwrap_or(pane);
            let status = text(entry, "agent_status").unwrap_or_default();
            let state = entry["state_labels"][status].as_str().filter(|label| !label.is_empty());
            let tab = text(entry, "tab_id").and_then(|tab| tabs.get(tab)).copied();
            Some(Agent {
                pane: pane.to_owned(),
                name: name.to_owned(),
                state: state.unwrap_or(status).to_owned(),
                tab: tab.unwrap_or_default().to_owned(),
            })
        })
        .collect()
}

/// The largest request herdr reads in time.
const MAX_REQUEST_BYTES: usize = 256 * 1024;
/// herdr's read window, plus time to answer.
#[cfg(unix)]
const SEND_TIMEOUT: Duration = Duration::from_secs(6);

/// Paste `text` into agent `pane`'s input, unsubmitted, in one request, then focus the pane.
/// An agent waiting at a prompt refuses, since the prompt would swallow the paste.
pub fn send(pane: &str, text: &str) -> Result<(), String> {
    let socket = var("HERDR_SOCKET_PATH").ok_or("not inside herdr")?;
    // The paste's end marker inside the text would end the paste early.
    let body = text.replace("\x1b[201~", "");
    let request = serde_json::json!({
        "id": "herdr-navigator:send",
        "method": "pane.send_text",
        "params": {"pane_id": pane, "text": format!("\x1b[200~{body}\x1b[201~")},
    })
    .to_string();
    if request.len() > MAX_REQUEST_BYTES {
        return Err("too much to send".into());
    }
    let list = call(&["agent", "list"])?;
    let entries = list["agents"].as_array().into_iter().flatten();
    let agent = entries
        .filter(|entry| entry["agent"].as_str().is_some_and(|agent| !agent.is_empty()))
        .find(|entry| entry["pane_id"].as_str() == Some(pane))
        .ok_or("the agent closed")?;
    if agent["agent_status"].as_str() == Some("blocked") {
        return Err("answer the agent's prompt first".into());
    }
    let reply =
        exchange(&socket, &request).map_err(|error| format!("herdr didn't answer: {error}"))?;
    let reply: Value = serde_json::from_str(&reply).unwrap_or_default();
    if let Some(error) = reply.get("error") {
        return Err(format!("herdr refused the send: {error}"));
    }
    call(&["agent", "focus", pane]).map(drop)
}

/// Write `request` as one line to herdr's socket and read the one line it answers.
fn exchange(socket: &str, request: &str) -> std::io::Result<String> {
    let mut stream = connect(socket)?;
    stream.write_all(format!("{request}\n").as_bytes())?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply)?;
    Ok(reply)
}

#[cfg(unix)]
fn connect(socket: &str) -> std::io::Result<impl Read + Write> {
    let stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(SEND_TIMEOUT))?;
    stream.set_write_timeout(Some(SEND_TIMEOUT))?;
    Ok(stream)
}

#[cfg(windows)]
fn connect(socket: &str) -> std::io::Result<impl Read + Write> {
    let path = if socket.starts_with(r"\\.\pipe\") || socket.starts_with(r"\\?\pipe\") {
        socket.to_owned()
    } else {
        format!(r"\\.\pipe\{socket}")
    };
    std::fs::OpenOptions::new().read(true).write(true).open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agents_are_the_workspace_s_agent_panes_but_this_one() {
        let list = serde_json::json!({"agents": [
            {"agent": "claude", "agent_status": "working", "pane_id": "p1", "tab_id": "t1",
             "workspace_id": "w1", "name": "fixer", "state_labels": {"working": "busy"}},
            {"agent": "codex", "agent_status": "idle", "pane_id": "p2", "tab_id": "t2",
             "workspace_id": "w1", "display_agent": "", "name": null},
            {"agent": "claude", "agent_status": "idle", "pane_id": "me", "tab_id": "t1",
             "workspace_id": "w1"},
            {"agent": "", "agent_status": "idle", "pane_id": "shell", "tab_id": "t1",
             "workspace_id": "w1"},
            {"agent": "claude", "agent_status": "idle", "pane_id": "far", "tab_id": "t9",
             "workspace_id": "w2"},
        ]});
        let tabs = HashMap::from([("t1", "main")]);
        let agent = |pane: &str, name: &str, state: &str, tab: &str| Agent {
            pane: pane.into(),
            name: name.into(),
            state: state.into(),
            tab: tab.into(),
        };
        assert_eq!(
            agents_in(&list, "w1", Some("me"), &tabs),
            [agent("p1", "fixer", "busy", "main"), agent("p2", "codex", "idle", "")]
        );
    }
}
