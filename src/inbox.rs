//! How `herdr-md <file>` hands a file to a pane that is already open: a file per pane naming the
//! file to show, which the pane takes and removes.

use std::fs;
use std::path::{Path, PathBuf};

use crate::herdr;

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

    /// Ask the pane to show `file`, replacing any request it hasn't taken yet. Written through
    /// a temporary file, so the pane never reads half a path.
    pub fn post(&self, file: &Path) -> Result<(), String> {
        let dir = self.path.parent().ok_or("the inbox has no directory")?;
        fs::create_dir_all(dir)
            .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
        let partial = self.path.with_extension("partial");
        fs::write(&partial, file.as_os_str().as_encoded_bytes())
            .and_then(|()| fs::rename(&partial, &self.path))
            .map_err(|error| format!("cannot write {}: {error}", self.path.display()))
    }

    /// The file last posted and not yet taken.
    pub fn take(&self) -> Option<PathBuf> {
        let path = fs::read_to_string(&self.path).ok()?;
        fs::remove_file(&self.path).ok()?;
        Some(PathBuf::from(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_posted_file_is_taken_once_and_the_latest_post_wins() {
        let dir = std::env::temp_dir().join(format!("herdr-md-inbox-{}", std::process::id()));
        let inbox = Inbox::at(dir.join("w1_p1"));
        assert_eq!(inbox.take(), None);
        inbox.post(Path::new("/a.md")).unwrap();
        inbox.post(Path::new("/b.md")).unwrap();
        assert_eq!(inbox.take(), Some(PathBuf::from("/b.md")));
        assert_eq!(inbox.take(), None);
    }
}
