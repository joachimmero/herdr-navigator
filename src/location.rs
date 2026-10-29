//! A place an agent points the pane at: a file, and maybe the lines in it.

use std::path::{Path, PathBuf};

use crate::layout::Lines;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub file: PathBuf,
    pub lines: Option<Lines>,
}

impl Location {
    /// `path`, `path:line` or `path:start-end`, the path taken from `cwd`. A file whose name
    /// itself ends in `:3` is still that file.
    pub fn parse(arg: &str, cwd: &Path) -> Result<Self, String> {
        let (path, lines) = match arg.rsplit_once(':').map(|(path, range)| (path, lines(range))) {
            Some((path, Some(lines))) if !cwd.join(arg).is_file() => (path, Some(lines)),
            _ => (arg, None),
        };
        let file = cwd.join(path);
        if !file.is_file() {
            return Err(format!("{} is not a file", file.display()));
        }
        Ok(Self { file: file.canonicalize().unwrap_or(file), lines })
    }

    /// The location as one string for handing to another process, which `decode` reads back.
    pub fn encode(&self) -> String {
        match self.lines {
            Some(Lines { first, last }) => format!("{}\t{first}-{last}", self.file.display()),
            None => self.file.display().to_string(),
        }
    }

    pub fn decode(text: &str) -> Self {
        match text.rsplit_once('\t').map(|(path, range)| (path, lines(range))) {
            Some((path, Some(lines))) => Self { file: path.into(), lines: Some(lines) },
            _ => Self { file: text.into(), lines: None },
        }
    }

    /// `path:start-end`, or `path:line` for one line, the path relative to `root` when inside it.
    pub fn label(&self, root: &Path) -> String {
        let path = self.file.strip_prefix(root).unwrap_or(&self.file).display();
        match self.lines {
            Some(Lines { first, last }) if first == last => format!("{path}:{first}"),
            Some(Lines { first, last }) => format!("{path}:{first}-{last}"),
            None => path.to_string(),
        }
    }
}

/// `3` or `3-5` as the lines they name; lines count from 1 and a range runs forward.
fn lines(range: &str) -> Option<Lines> {
    let (first, last) = range.split_once('-').unwrap_or((range, range));
    let lines = Lines { first: first.parse().ok()?, last: last.parse().ok()? };
    (1 <= lines.first && lines.first <= lines.last).then_some(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_location_names_a_file_and_maybe_its_lines() {
        let dir = std::env::temp_dir().join(format!("herdr-navigator-loc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.rs"), "").unwrap();
        std::fs::write(dir.join("odd:7"), "").unwrap();
        let dir = dir.canonicalize().unwrap();
        let at = |arg: &str| Location::parse(arg, &dir).map(|location| location.label(&dir));

        assert_eq!(at("a.rs"), Ok("a.rs".into()));
        assert_eq!(at("a.rs:3"), Ok("a.rs:3".into()));
        assert_eq!(at("a.rs:3-5"), Ok("a.rs:3-5".into()));
        assert_eq!(at("odd:7"), Ok("odd:7".into()));
        assert!(at("a.rs:5-3").is_err());
        assert!(at("a.rs:0").is_err());
        assert!(at("b.rs:3").is_err());
    }

    #[test]
    fn a_location_survives_encoding() {
        let location =
            Location { file: "/p/a:b.rs".into(), lines: Some(Lines { first: 3, last: 5 }) };
        assert_eq!(Location::decode(&location.encode()), location);
        let whole = Location { file: "/p/a:3".into(), lines: None };
        assert_eq!(Location::decode(&whole.encode()), whole);
    }
}
