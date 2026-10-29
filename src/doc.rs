//! The file the pane shows, kept in step with the disk.

use std::fs;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::layout::{Lines, Row};
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
    /// The width `rows` were laid out for.
    width: u16,
    rows: Vec<Row>,
    pub scroll: usize,
    /// The row the cursor is on.
    cursor: usize,
    /// The row a selection started on, while there is one.
    anchor: Option<usize>,
}

impl Doc {
    pub fn open(path: PathBuf) -> Self {
        let mut doc = Self {
            path,
            content: Content::Code(Vec::new()),
            stamp: None,
            width: 0,
            rows: Vec::new(),
            scroll: 0,
            cursor: 0,
            anchor: None,
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

    /// The document laid out for `width` columns, the cursor and selection kept on the source
    /// lines they were on.
    pub fn rows(&mut self, width: u16) -> &[Row] {
        if width != self.width {
            let cursor_line = self.source_line(self.cursor);
            let anchor_line = self.anchor.and_then(|anchor| self.source_line(anchor));
            let cursor_on_screen = self.cursor.saturating_sub(self.scroll);
            self.rows = match &self.content {
                Content::Markdown(source) => markdown::render(source, width),
                Content::Code(lines) => code::layout(lines, width),
                Content::Binary => vec![Row {
                    line: Line::styled(
                        "Binary file, not shown.",
                        Style::new().add_modifier(Modifier::DIM),
                    ),
                    source: None,
                }],
            };
            self.width = width;
            if let Some(line) = cursor_line {
                self.cursor = row_for(&self.rows, line);
                self.scroll = self.cursor.saturating_sub(cursor_on_screen);
            }
            self.anchor = self
                .anchor
                .map(|anchor| anchor_line.map_or(anchor, |line| row_for(&self.rows, line)));
        }
        &self.rows
    }

    /// The rows on screen in a page `width` columns by `height` lines, scrolled to keep the
    /// cursor on it.
    pub fn page(&mut self, width: u16, height: usize) -> &[Row] {
        self.rows(width);
        self.move_cursor(0, height);
        let end = (self.scroll + height).min(self.rows.len());
        &self.rows[self.scroll..end]
    }

    /// How many rows the document took at its last layout.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Move the cursor by `delta` rows, scrolling a page of `height` rows to keep it on screen.
    pub fn move_cursor(&mut self, delta: isize, height: usize) {
        self.cursor = self.cursor.saturating_add_signed(delta).min(self.last_row());
        self.scroll = self.scroll.min(self.cursor).max((self.cursor + 1).saturating_sub(height));
        self.scroll = self.scroll.min(self.max_scroll(height));
    }

    /// Scroll by `delta` rows, keeping a page of `height` rows on screen where it can, and the
    /// cursor on that page.
    pub fn scroll_by(&mut self, delta: isize, height: usize) {
        self.scroll = self.scroll.saturating_add_signed(delta).min(self.max_scroll(height));
        let bottom = (self.scroll + height).saturating_sub(1);
        self.cursor = self.cursor.clamp(self.scroll, bottom.max(self.scroll)).min(self.last_row());
    }

    /// Move the view and the cursor together by `delta` rows, as paging does.
    pub fn page_by(&mut self, delta: isize, height: usize) {
        self.scroll = self.scroll.saturating_add_signed(delta).min(self.max_scroll(height));
        self.move_cursor(delta, height);
    }

    /// Put the cursor on the row `offset` rows down the page.
    pub fn click(&mut self, offset: usize, height: usize) {
        self.cursor = (self.scroll + offset).min(self.last_row());
        self.move_cursor(0, height);
    }

    /// Start a selection at the cursor, or drop the one there is.
    pub fn toggle_selection(&mut self) {
        self.anchor = match self.anchor {
            Some(_) => None,
            None => Some(self.cursor),
        };
    }

    /// Drop the selection, saying whether there was one.
    pub fn clear_selection(&mut self) -> bool {
        self.anchor.take().is_some()
    }

    pub fn is_selecting(&self) -> bool {
        self.anchor.is_some()
    }

    /// The source lines the selection covers, or the cursor's row without one; `None` when no
    /// source line feeds those rows.
    pub fn selection(&self) -> Option<Lines> {
        let rows = self.rows.get(self.selected_rows())?;
        rows.iter().filter_map(|row| row.source).reduce(Lines::union)
    }

    /// Whether row `index` shows part of the selection: one of the rows it spans, or another row
    /// of the same source lines, like a long line's continuation.
    pub fn is_selected(&self, index: usize) -> bool {
        let shared = || {
            let source = self.rows.get(index).and_then(|row| row.source);
            self.selection().zip(source).is_some_and(|(selection, row)| selection.contains(row))
        };
        self.selected_rows().contains(&index) || shared()
    }

    fn selected_rows(&self) -> RangeInclusive<usize> {
        let anchor = self.anchor.unwrap_or(self.cursor);
        anchor.min(self.cursor)..=anchor.max(self.cursor)
    }

    fn source_line(&self, row: usize) -> Option<usize> {
        self.rows.get(..=row)?.iter().rev().find_map(|row| row.source).map(|source| source.first)
    }

    fn last_row(&self) -> usize {
        self.rows.len().saturating_sub(1)
    }

    fn max_scroll(&self, height: usize) -> usize {
        self.rows.len().saturating_sub(height)
    }
}

/// The first row showing source line `line`, or the first after it when none does.
fn row_for(rows: &[Row], line: usize) -> usize {
    let shows = |row: &Row| row.source.is_some_and(|source| source.last >= line);
    rows.iter().position(shows).unwrap_or(rows.len().saturating_sub(1))
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
        assert_eq!(doc.rows(20)[0].line.to_string(), "one");

        fs::write(&path, "two words").unwrap();
        doc.refresh();
        assert_eq!(doc.rows(20)[0].line.to_string(), "two words");
    }

    #[test]
    fn a_deleted_file_keeps_its_last_content_until_it_returns() {
        let path = temp("deleted");
        fs::write(&path, "kept").unwrap();
        let mut doc = Doc::open(path.clone());

        fs::remove_file(&path).unwrap();
        doc.refresh();
        assert!(doc.is_missing());
        assert_eq!(doc.rows(20)[0].line.to_string(), "kept");

        fs::write(&path, "back").unwrap();
        doc.refresh();
        assert!(!doc.is_missing());
        assert_eq!(doc.rows(20)[0].line.to_string(), "back");
    }

    #[test]
    fn source_shows_numbered_and_a_binary_file_as_a_notice() {
        let dir = temp("kinds").with_file_name("");
        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
        fs::write(dir.join("blob.bin"), [0x89, 0, 1]).unwrap();
        assert_eq!(Doc::open(dir.join("main.rs")).rows(20)[0].line.to_string(), "1 fn main() {}");
        assert_eq!(
            Doc::open(dir.join("blob.bin")).rows(30)[0].line.to_string(),
            "Binary file, not shown."
        );
    }

    #[test]
    fn scrolling_stops_at_the_last_page_and_takes_the_cursor_along() {
        let path = temp("scroll");
        fs::write(&path, "a\n\nb\n\nc").unwrap();
        let mut doc = Doc::open(path);
        doc.rows(20);
        doc.scroll_by(10, 2);
        assert_eq!((doc.scroll, doc.cursor()), (3, 3));
        doc.scroll_by(-10, 2);
        assert_eq!((doc.scroll, doc.cursor()), (0, 1));
    }

    #[test]
    fn the_cursor_keeps_its_page_on_screen() {
        let path = temp("cursor").with_file_name("cursor.rs");
        fs::write(&path, "1\n2\n3\n4\n5\n").unwrap();
        let mut doc = Doc::open(path);
        doc.rows(20);
        doc.move_cursor(3, 2);
        assert_eq!((doc.cursor(), doc.scroll), (3, 2));
        doc.move_cursor(10, 2);
        assert_eq!((doc.cursor(), doc.scroll), (4, 3));
        doc.move_cursor(-4, 2);
        assert_eq!((doc.cursor(), doc.scroll), (0, 0));
    }

    #[test]
    fn a_selection_spans_the_source_lines_between_its_ends() {
        let path = temp("select").with_file_name("select.rs");
        fs::write(&path, "one\ntwo\nthree\n").unwrap();
        let mut doc = Doc::open(path);
        doc.rows(20);
        assert_eq!(doc.selection(), Some(Lines::one(1)));
        doc.move_cursor(2, 10);
        doc.toggle_selection();
        doc.move_cursor(-1, 10);
        assert_eq!(doc.selection(), Some(Lines { first: 2, last: 3 }));
        assert!(doc.clear_selection());
        assert_eq!(doc.selection(), Some(Lines::one(2)));
    }

    #[test]
    fn a_long_lines_continuation_is_selected_with_it_and_a_resize_keeps_the_cursor_on_it() {
        let path = temp("wrap").with_file_name("wrap.rs");
        fs::write(&path, "a\nbbbbbbbb\nc\n").unwrap();
        let mut doc = Doc::open(path);
        doc.rows(6);
        doc.move_cursor(3, 10);
        assert_eq!(doc.selection(), Some(Lines::one(3)));
        doc.move_cursor(-2, 10);
        assert_eq!(doc.selection(), Some(Lines::one(2)));
        assert!(doc.is_selected(1) && doc.is_selected(2) && !doc.is_selected(3));

        doc.rows(20);
        assert_eq!(doc.cursor(), 1);
        assert_eq!(doc.selection(), Some(Lines::one(2)));
    }
}
