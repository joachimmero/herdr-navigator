//! The file the pane shows, kept in step with the disk.

use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

use ratatui::text::Line;

use crate::markdown;

/// What a file looked like on disk the last time it was read.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    modified: SystemTime,
    len: u64,
}

pub struct Doc {
    pub path: PathBuf,
    source: String,
    /// `None` while the file is missing, in which case `source` is what it last held.
    stamp: Option<Stamp>,
    /// The width `lines` were laid out for.
    width: u16,
    lines: Vec<Line<'static>>,
    pub scroll: usize,
}

impl Doc {
    pub fn open(path: PathBuf) -> Self {
        let mut doc = Self {
            path,
            source: String::new(),
            stamp: None,
            width: 0,
            lines: Vec::new(),
            scroll: 0,
        };
        doc.refresh();
        doc
    }

    pub fn is_missing(&self) -> bool {
        self.stamp.is_none()
    }

    /// Reread the file if it changed on disk since it was last read.
    pub fn refresh(&mut self) {
        let stamp = fs::metadata(&self.path).ok().and_then(|metadata| {
            Some(Stamp { modified: metadata.modified().ok()?, len: metadata.len() })
        });
        if stamp == self.stamp {
            return;
        }
        let source = stamp.and_then(|_| fs::read(&self.path).ok());
        self.stamp = source.as_ref().and(stamp);
        if let Some(source) = source {
            self.source = String::from_utf8_lossy(&source).into_owned();
            self.width = 0;
        }
    }

    /// The document laid out for `width` columns.
    pub fn lines(&mut self, width: u16) -> &[Line<'static>] {
        if width != self.width {
            self.lines = markdown::render(&self.source, width);
            self.width = width;
        }
        &self.lines
    }

    /// The lines on screen in a page `width` columns by `height` lines, moving the scroll back
    /// when a reload or resize left it past the end.
    pub fn page(&mut self, width: u16, height: usize) -> &[Line<'static>] {
        self.lines(width);
        self.scroll_by(0, height);
        let end = (self.scroll + height).min(self.lines.len());
        &self.lines[self.scroll..end]
    }

    /// How many lines the document took at its last layout.
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// Scroll by `delta` lines, keeping a page of `height` lines on screen where it can.
    pub fn scroll_by(&mut self, delta: isize, height: usize) {
        self.scroll = self.scroll.saturating_add_signed(delta).min(self.max_scroll(height));
    }

    pub fn scroll_to_end(&mut self, height: usize) {
        self.scroll = self.max_scroll(height);
    }

    fn max_scroll(&self, height: usize) -> usize {
        self.lines.len().saturating_sub(height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("herdr-md-doc-{}-{name}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir.join("doc.md")
    }

    #[test]
    fn a_change_on_disk_is_picked_up() {
        let path = temp("change");
        fs::write(&path, "one").unwrap();
        let mut doc = Doc::open(path.clone());
        assert_eq!(doc.lines(20)[0].to_string(), "one");

        fs::write(&path, "two words").unwrap();
        doc.refresh();
        assert_eq!(doc.lines(20)[0].to_string(), "two words");
    }

    #[test]
    fn a_deleted_file_keeps_its_last_content_until_it_returns() {
        let path = temp("deleted");
        fs::write(&path, "kept").unwrap();
        let mut doc = Doc::open(path.clone());

        fs::remove_file(&path).unwrap();
        doc.refresh();
        assert!(doc.is_missing());
        assert_eq!(doc.lines(20)[0].to_string(), "kept");

        fs::write(&path, "back").unwrap();
        doc.refresh();
        assert!(!doc.is_missing());
        assert_eq!(doc.lines(20)[0].to_string(), "back");
    }

    #[test]
    fn scrolling_stops_at_the_last_page() {
        let path = temp("scroll");
        fs::write(&path, "a\n\nb\n\nc").unwrap();
        let mut doc = Doc::open(path);
        doc.lines(20);
        doc.scroll_by(10, 2);
        assert_eq!(doc.scroll, 3);
        doc.scroll_by(-10, 2);
        assert_eq!(doc.scroll, 0);
    }
}
