mod markdown;

use std::process::ExitCode;

fn main() -> ExitCode {
    eprintln!("usage: herdr-md <file.md>");
    ExitCode::from(2)
}
