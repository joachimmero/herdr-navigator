//! `herdr-navigator <file>`: show a file in the workspace's herdr-navigator pane, opening one beside
//! the calling pane if there is none, without taking the focus.

use std::path::PathBuf;

use crate::actions::{self, Pane};
use crate::herdr::var;
use crate::inbox::Inbox;

pub const USAGE: &str = "herdr-navigator <file>";

/// Show `file` and return the process exit code.
pub fn run(file: &str) -> u8 {
    match show(file) {
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

fn show(file: &str) -> Result<String, String> {
    let cwd = std::env::current_dir().map_err(|error| format!("no working directory: {error}"))?;
    let file = cwd.join(file);
    if !file.is_file() {
        return Err(format!("{} is not a file", file.display()));
    }
    let file: PathBuf = file.canonicalize().unwrap_or(file);
    let ws = var("HERDR_WORKSPACE_ID").ok_or("not inside herdr: HERDR_WORKSPACE_ID is unset")?;
    let beside = var("HERDR_PANE_ID").ok_or("not inside herdr: HERDR_PANE_ID is unset")?;

    match actions::find_or_open(&ws, &beside, &cwd, &file)? {
        Pane::Opened(pane) => Ok(format!("opened {} in {pane}", file.display())),
        Pane::Existing(pane) => {
            Inbox::for_pane(&pane)?.post(&file)?;
            Ok(format!("showing {} in {pane}", file.display()))
        }
    }
}
