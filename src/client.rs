//! `herdr-navigator <file>[:line[-line]]`: show a place in the workspace's herdr-navigator pane,
//! opening one beside the calling pane if there is none, without taking the focus.

use crate::actions::{self, Pane};
use crate::herdr::var;
use crate::inbox::Inbox;
use crate::location::Location;

pub const USAGE: &str = "herdr-navigator <file>[:line[-line]]";

/// Show `arg`'s place and return the process exit code.
pub fn run(arg: &str) -> u8 {
    match show(arg) {
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

fn show(arg: &str) -> Result<String, String> {
    let cwd = std::env::current_dir().map_err(|error| format!("no working directory: {error}"))?;
    let location = Location::parse(arg, &cwd)?;
    let shown = location.label(&cwd);
    let ws = var("HERDR_WORKSPACE_ID").ok_or("not inside herdr: HERDR_WORKSPACE_ID is unset")?;
    let beside = var("HERDR_PANE_ID").ok_or("not inside herdr: HERDR_PANE_ID is unset")?;

    match actions::find_or_open(&ws, &beside, &cwd, &location)? {
        Pane::Opened(pane) => Ok(format!("opened {shown} in {pane}")),
        Pane::Existing(pane) => {
            Inbox::for_pane(&pane)?.post(&location)?;
            Ok(format!("showing {shown} in {pane}"))
        }
    }
}
