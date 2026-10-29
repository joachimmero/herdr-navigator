mod app;
mod doc;
mod markdown;
mod ui;

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    ExitCode::from(match args.as_slice() {
        [] => app::run(std::env::var_os("HERDR_MD_FILE").map(PathBuf::from)),
        _ => {
            eprintln!("usage: herdr-md");
            2
        }
    })
}
