mod actions;
mod app;
mod client;
mod code;
mod doc;
mod herdr;
mod highlight;
mod inbox;
mod layout;
mod markdown;
mod tree;
mod ui;

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    ExitCode::from(match args.as_slice() {
        [] => app::run(herdr::var(actions::FILE_VAR).map(PathBuf::from)),
        [flag, name] if flag == "--action" => actions::run(name),
        [file] if !file.starts_with('-') => client::run(file),
        _ => {
            eprintln!("usage: {}", client::USAGE);
            eprintln!("       herdr-navigator [--action toggle|open|close]");
            2
        }
    })
}
