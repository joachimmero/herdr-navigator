//! How `herdr-navigator <file>` hands a place to a pane that is already open: a file per pane
//! naming the place to show, which the pane takes and removes.

use std::fs;
use std::path::PathBuf;

use crate::herdr;
use crate::location::Location;

pub struct Inbox {
    path: PathBuf,
}

impl Inbox {
    /// Pane `pane`'s inbox, under the plugin's state dir.
    pub fn for_pane(pane: &str) -> Result<Self, String> {
        Ok(Self::at(herdr::state_dir()?.join("inbox").join(herdr::file_safe(pane))))
    }

    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    /// Ask the pane to show `location`, replacing any request it hasn't taken yet. Written
    /// through a temporary file, so the pane never reads half of it.
    pub fn post(&self, location: &Location) -> Result<(), String> {
        let dir = self.path.parent().ok_or("the inbox has no directory")?;
        fs::create_dir_all(dir)
            .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
        let partial = self.path.with_extension("partial");
        fs::write(&partial, location.encode())
            .and_then(|()| fs::rename(&partial, &self.path))
            .map_err(|error| format!("cannot write {}: {error}", self.path.display()))
    }

    /// The place last posted and not yet taken.
    pub fn take(&self) -> Option<Location> {
        let text = fs::read_to_string(&self.path).ok()?;
        fs::remove_file(&self.path).ok()?;
        Some(Location::decode(&text))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::layout::Lines;

    fn at(file: &str) -> Location {
        Location { file: file.into(), lines: Some(Lines::one(3)) }
    }

    #[test]
    fn a_posted_place_is_taken_once_and_the_latest_post_wins() {
        let dir =
            std::env::temp_dir().join(format!("herdr-navigator-inbox-{}", std::process::id()));
        let inbox = Inbox::at(dir.join("w1_p1"));
        assert_eq!(inbox.take(), None);
        inbox.post(&at("/a.md")).unwrap();
        inbox.post(&at("/b.md")).unwrap();
        assert_eq!(inbox.take().map(|location| location.file), Some(Path::new("/b.md").into()));
        assert_eq!(inbox.take(), None);
    }
}
