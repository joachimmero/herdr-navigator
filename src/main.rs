mod actions;
mod app;
mod client;
mod code;
mod comment;
mod composer;
mod doc;
mod field;
mod herdr;
mod highlight;
mod inbox;
mod jumps;
mod layout;
mod location;
mod markdown;
mod tree;
mod ui;
mod watch;

use std::process::ExitCode;

use location::Location;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    ExitCode::from(match args.as_slice() {
        [] => app::run(herdr::var(actions::FILE_VAR).map(|text| Location::decode(&text))),
        [flag, name] if flag == "--action" => actions::run(name),
        [file] if !file.starts_with('-') => client::run(file),
        _ => {
            eprintln!("usage: {}", client::USAGE);
            eprintln!("       herdr-navigator [--action toggle|open|close]");
            2
        }
    })
}
