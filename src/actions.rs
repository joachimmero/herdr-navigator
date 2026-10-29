//! The plugin's toggle, open, and close actions on the invoking workspace's herdr-navigator pane,
//! and the find-or-open showing a file needs.

use std::fs::File;
use std::path::Path;

use crate::location::Location;

use serde_json::Value;

use crate::herdr::{self, var};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Toggle,
    Open,
    Close,
}

/// Run the action `name` and return the process exit code.
pub fn run(name: &str) -> u8 {
    let action = match name {
        "toggle" => Action::Toggle,
        "open" => Action::Open,
        "close" => Action::Close,
        _ => {
            eprintln!("herdr-navigator: unknown action '{name}' (toggle | open | close)");
            return 1;
        }
    };
    match act(action) {
        Ok(line) => {
            println!("herdr-navigator: {line}");
            0
        }
        Err(why) => {
            eprintln!("herdr-navigator: {why}");
            1
        }
    }
}

fn act(action: Action) -> Result<String, String> {
    let context: Value = var("HERDR_PLUGIN_CONTEXT_JSON")
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default();
    let ws = var("HERDR_WORKSPACE_ID")
        .or_else(|| text(&context, "/workspace_id"))
        .ok_or("no workspace context (invoke from inside herdr)")?;
    // Held through the close or open, so two quick toggles can't both open a pane.
    let _lock = lock(&ws)?;

    let (panes, open) = panes(&ws)?;
    match (action, open.is_empty()) {
        (Action::Toggle | Action::Close, false) => {
            open.iter().try_for_each(|pane| herdr::call(&["pane", "close", pane]).map(drop))?;
            Ok(format!("closed {}", open.join(" ")))
        }
        (Action::Open, false) => Ok(format!("already open ({})", open[0])),
        (Action::Close, true) => Ok("nothing open".into()),
        (Action::Toggle | Action::Open, true) => {
            let (target, cwd) = placement(&ws, &context, &panes)?;
            let pane = open_beside(&target, &cwd, true, None)?;
            Ok(format!("opened {pane} in {cwd}"))
        }
    }
}

pub enum Pane {
    /// Opened for the request, already showing its file.
    Opened(String),
    Existing(String),
}

/// The herdr-navigator pane in workspace `ws`, opened beside pane `beside` in `cwd` showing
/// `location` when there is none, without taking the focus.
pub fn find_or_open(
    ws: &str,
    beside: &str,
    cwd: &Path,
    location: &Location,
) -> Result<Pane, String> {
    let _lock = lock(ws)?;
    let (_, open) = panes(ws)?;
    match open.into_iter().next() {
        Some(pane) => Ok(Pane::Existing(pane)),
        None => {
            open_beside(beside, &cwd.to_string_lossy(), false, Some(location)).map(Pane::Opened)
        }
    }
}

/// Workspace `ws`'s panes, and the ids of those showing herdr-navigator.
fn panes(ws: &str) -> Result<(Vec<Value>, Vec<String>), String> {
    let panes = herdr::call(&["pane", "list", "--workspace", ws])?;
    let panes = panes["panes"].as_array().cloned().unwrap_or_default();
    let open = panes
        .iter()
        .filter_map(|pane| text(pane, "/pane_id"))
        .filter(|pane| shows_md(pane))
        .collect();
    Ok((panes, open))
}

/// The pane to split beside, and the directory the new pane starts in.
fn placement(ws: &str, context: &Value, panes: &[Value]) -> Result<(String, String), String> {
    let focused = text(context, "/focused_pane_id").or_else(|| var("HERDR_PANE_ID"));
    // The focused pane's live cwd wins over the workspace's.
    let live = focused.as_deref().and_then(|focused| {
        panes
            .iter()
            .find(|pane| text(pane, "/pane_id").as_deref() == Some(focused))
            .and_then(|pane| text(pane, "/foreground_cwd"))
    });
    let cwd = live
        .or_else(|| text(context, "/focused_pane_cwd"))
        .or_else(|| text(context, "/workspace_cwd"))
        .or_else(|| var("HOME"))
        .ok_or("no directory to open the pane in")?;
    let target = focused
        .or_else(|| panes.first().and_then(|pane| text(pane, "/pane_id")))
        .ok_or_else(|| format!("no pane to split in {ws}"))?;
    Ok((target, cwd))
}

/// Open a herdr-navigator pane in `cwd` as a split to the right of pane `target`, showing
/// `location` if given, and return its id.
fn open_beside(
    target: &str,
    cwd: &str,
    focus: bool,
    location: Option<&Location>,
) -> Result<String, String> {
    let plugin = herdr::plugin_id();
    let env = location.map(|location| format!("{FILE_VAR}={}", location.encode()));
    let mut args = vec![
        "plugin",
        "pane",
        "open",
        "--plugin",
        &plugin,
        "--entrypoint",
        "pane",
        "--placement",
        "split",
        "--target-pane",
        target,
        "--direction",
        "right",
        "--cwd",
        cwd,
    ];
    args.push(if focus { "--focus" } else { "--no-focus" });
    if let Some(env) = &env {
        args.extend(["--env", env]);
    }
    let opened = herdr::call(&args)?;
    text(&opened, "/plugin_pane/pane/pane_id")
        .ok_or_else(|| format!("herdr opened a pane in {cwd} without naming it"))
}

/// Whether pane `pane`'s foreground is the herdr-navigator pane itself, which runs with no
/// arguments, rather than an action or a hand-off.
fn shows_md(pane: &str) -> bool {
    let Ok(info) = herdr::call(&["pane", "process-info", "--pane", pane]) else {
        return false;
    };
    let processes = info.pointer("/process_info/foreground_processes").and_then(Value::as_array);
    processes.into_iter().flatten().any(|process| {
        let argv: Vec<&str> =
            process["argv"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        let exe = text(process, "/argv0").or_else(|| argv.first().map(|exe| (*exe).to_owned()));
        let named =
            exe.is_some_and(|exe| Path::new(&exe).file_name().is_some_and(|name| name == BINARY));
        named && argv.len() <= 1
    })
}

const BINARY: &str = env!("CARGO_PKG_NAME");

/// Names the location a newly opened pane shows first, as `Location::encode` writes it.
pub const FILE_VAR: &str = "HERDR_NAVIGATOR_FILE";

fn lock(ws: &str) -> Result<File, String> {
    let dir = herdr::state_dir()?;
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    let path = dir.join(format!("action-{}.lock", herdr::file_safe(ws)));
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    file.lock().map_err(|error| format!("cannot lock {}: {error}", path.display()))?;
    Ok(file)
}

fn text(value: &Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}
