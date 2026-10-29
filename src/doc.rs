//! The file the pane shows, kept in step with the disk.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::{code, highlight, markdown};

/// What a file looked like on disk the last time it was read.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    modified: SystemTime,
    len: u64,
}

/// What the file holds, as the pane shows it.
enum Content {
    Markdown(String),
    /// Each line styled by its grammar.
    Code(Vec<Vec<Span<'static>>>),
    Binary,
}

pub struct Doc {
    pub path: PathBuf,
    content: Content,
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
            content: Content::Code(Vec::new()),
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
            self.content = read(&self.path, &source);
            self.width = 0;
        }
    }

    /// The document laid out for `width` columns.
    pub fn lines(&mut self, width: u16) -> &[Line<'static>] {
        if width != self.width {
            self.lines = match &self.content {
                Content::Markdown(source) => markdown::render(source, width),
                Content::Code(lines) => code::layout(lines, width),
                Content::Binary => vec![Line::styled(
                    "Binary file, not shown.",
                    Style::new().add_modifier(Modifier::DIM),
                )],
            };
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

fn read(path: &Path, bytes: &[u8]) -> Content {
    // Git's test: a NUL among the first 8000 bytes.
    if bytes.iter().take(8000).any(|byte| *byte == 0) {
        return Content::Binary;
    }
    let text = String::from_utf8_lossy(bytes);
    if is_markdown(path) {
        return Content::Markdown(text.into_owned());
    }
    let lines: Vec<&str> = text.lines().collect();
    Content::Code(highlight::highlight(path, &lines))
}

pub fn is_markdown(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("herdr-navigator-doc-{}-{name}", std::process::id()));
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
    fn source_shows_numbered_and_a_binary_file_as_a_notice() {
        let dir = temp("kinds").with_file_name("");
        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
        fs::write(dir.join("blob.bin"), [0x89, 0, 1]).unwrap();
        assert_eq!(Doc::open(dir.join("main.rs")).lines(20)[0].to_string(), "1 fn main() {}");
        assert_eq!(
            Doc::open(dir.join("blob.bin")).lines(30)[0].to_string(),
            "Binary file, not shown."
        );
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
