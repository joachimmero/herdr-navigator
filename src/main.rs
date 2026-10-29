mod actions;
mod app;
mod doc;
mod herdr;
mod markdown;
mod picker;
mod ui;

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    ExitCode::from(match args.as_slice() {
        [] => app::run(herdr::var("HERDR_MD_FILE").map(PathBuf::from)),
        [flag, name] if flag == "--action" => actions::run(name),
        _ => {
            eprintln!("usage: herdr-md [--action toggle|open|close]");
            2
        }
    })
}
