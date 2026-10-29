//! The file the pane shows, kept in step with the disk, with its comments' cards among its rows.

use std::fs;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::comment::{self, Comments};
use crate::layout::{Lines, Row};
use crate::{code, find, highlight, markdown};

/// What a file looked like on disk the last time it was read.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    modified: SystemTime,
    len: u64,
}

/// What the file holds, as the pane shows it.
enum Content {
    Markdown,
    /// Each line styled by its grammar.
    Code(Vec<Vec<Span<'static>>>),
    Binary,
}

/// One row on screen: one of the file's rows, or a line of a comment's card under them.
enum Shown {
    Source(usize),
    Card { comment: u64, line: Line<'static> },
}

pub struct Doc {
    pub path: PathBuf,
    content: Content,
    /// The file's text, empty for a binary file.
    text: String,
    /// `None` while the file is missing, in which case the content is what it last held.
    stamp: Option<Stamp>,
    /// The width `rows` were laid out for, 0 when they need laying out again.
    width: u16,
    rows: Vec<Row>,
    /// The comments' revision `shown` was built for.
    revision: Option<u64>,
    shown: Vec<Shown>,
    pub scroll: usize,
    /// The row on screen the cursor is on.
    cursor: usize,
    /// The row on screen a selection started on, while there is one.
    anchor: Option<usize>,
    /// A source line to put the cursor on at the next layout.
    goto: Option<usize>,
    /// Lines pointed at from outside, marked until the cursor leaves them.
    pointed: Option<Lines>,
    /// Whether the next layout puts the cursor in the middle of the page.
    centering: bool,
}

impl Doc {
    pub fn open(path: PathBuf) -> Self {
        let mut doc = Self {
            path,
            content: Content::Binary,
            text: String::new(),
            stamp: None,
            width: 0,
            rows: Vec::new(),
            revision: None,
            shown: Vec::new(),
            scroll: 0,
            cursor: 0,
            anchor: None,
            goto: None,
            pointed: None,
            centering: false,
        };
        doc.refresh();
        doc
    }

    pub fn is_missing(&self) -> bool {
        self.stamp.is_none()
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Reread the file if it changed on disk since it was last read, saying whether it did.
    pub fn refresh(&mut self) -> bool {
        let stamp = fs::metadata(&self.path).ok().and_then(|metadata| {
            Some(Stamp { modified: metadata.modified().ok()?, len: metadata.len() })
        });
        if stamp == self.stamp {
            return false;
        }
        let source = stamp.and_then(|_| fs::read(&self.path).ok());
        self.stamp = source.as_ref().and(stamp);
        let Some(source) = source else { return false };
        (self.content, self.text) = read(&self.path, &source);
        self.width = 0;
        true
    }

    /// Lay the file out for `width` columns with `comments`' cards under their lines, keeping
    /// the cursor and selection on the source lines they were on.
    pub fn build(&mut self, width: u16, comments: &Comments) {
        if width == self.width && self.revision == Some(comments.revision()) {
            return;
        }
        let cursor_line = self.source_line(self.cursor);
        let anchor_line = self.anchor.and_then(|anchor| self.source_line(anchor));
        let cursor_on_screen = self.cursor.saturating_sub(self.scroll);
        if width != self.width {
            self.rows = match &self.content {
                Content::Markdown => markdown::render(&self.text, width),
                Content::Code(lines) => code::layout(lines, width),
                Content::Binary => vec![Row {
                    line: Line::styled(
                        "Binary file, not shown.",
                        Style::new().add_modifier(Modifier::DIM),
                    ),
                    source: None,
                    chrome: 1,
                }],
            };
            self.width = width;
        }
        self.shown = with_cards(&self.rows, &self.path, comments, width);
        self.revision = Some(comments.revision());
        if let Some(line) = cursor_line {
            self.cursor = row_for(&self.rows, &self.shown, line);
            self.scroll = self.cursor.saturating_sub(cursor_on_screen);
        }
        self.anchor = self.anchor.map(|anchor| {
            anchor_line.map_or(anchor, |line| row_for(&self.rows, &self.shown, line))
        });
    }

    /// The rows on screen in a page `width` columns by `height` rows, scrolled to keep the
    /// cursor on it and `room` rows free under the selection.
    pub fn page(
        &mut self,
        width: u16,
        height: usize,
        comments: &Comments,
        room: usize,
    ) -> Vec<Line<'static>> {
        self.build(width, comments);
        if let Some(line) = self.goto.take() {
            self.cursor = row_for(&self.rows, &self.shown, line);
            // Bring as much of the pointed lines on screen as fits under the cursor.
            let last = self.pointed.map(|lines| row_for(&self.rows, &self.shown, lines.last));
            let bottom =
                last.map_or(self.cursor, |last| last.min(self.cursor + height.saturating_sub(1)));
            self.scroll = self.scroll.max((bottom + 1).saturating_sub(height));
        }
        if std::mem::take(&mut self.centering) {
            self.scroll = self.cursor.saturating_sub(height / 2);
        }
        self.move_cursor(0, height);
        if !self.is_pointed(self.cursor) {
            self.pointed = None;
        }
        if room > 0 {
            let room = room.min(height.saturating_sub(1));
            self.scroll = self.scroll.max((self.selection_end() + 1 + room).saturating_sub(height));
        }
        let end = (self.scroll + height).min(self.shown.len());
        (self.scroll..end).map(|index| self.line(index).clone()).collect()
    }

    /// How many rows the document took at its last layout.
    pub fn len(&self) -> usize {
        self.shown.len()
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

    /// Put the cursor on source line `line` when the page is next drawn, dropping any
    /// selection.
    pub fn go_to_line(&mut self, line: usize) {
        self.anchor = None;
        self.goto = Some(line);
    }

    /// Put the cursor on `lines` and mark them, until the cursor moves off them.
    pub fn point(&mut self, lines: Lines) {
        self.go_to_line(lines.first);
        self.pointed = Some(lines);
    }

    /// Put the cursor's row in the middle of the page when the page is next drawn.
    pub fn center(&mut self) {
        self.centering = true;
    }

    /// Whether row `index` shows part of the lines pointed at.
    pub fn is_pointed(&self, index: usize) -> bool {
        self.pointed.zip(self.source(index)).is_some_and(|(pointed, row)| pointed.overlaps(row))
    }

    /// The source line the cursor is on, or the nearest above it on a row with none.
    pub fn cursor_line(&self) -> Option<usize> {
        self.source_line(self.cursor)
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
        self.selected_rows().filter_map(|index| self.source(index)).reduce(Lines::union)
    }

    /// Whether row `index` shows part of the selection: one of the rows it spans, or another row
    /// of the same source lines, like a long line's continuation.
    pub fn is_selected(&self, index: usize) -> bool {
        let shared = || {
            let source = self.source(index);
            self.selection().zip(source).is_some_and(|(selection, row)| selection.contains(row))
        };
        self.selected_rows().contains(&index) || shared()
    }

    /// The last row showing part of the selection.
    pub fn selection_end(&self) -> usize {
        let end = *self.selected_rows().end();
        (end..self.shown.len()).take_while(|&index| self.is_selected(index)).last().unwrap_or(end)
    }

    /// The comment whose card the cursor is on, or else the first on the cursor's lines.
    pub fn comment_at_cursor(&self, comments: &Comments) -> Option<u64> {
        if let Some(Shown::Card { comment, .. }) = self.shown.get(self.cursor) {
            return Some(*comment);
        }
        let lines = self.source(self.cursor)?;
        let on_lines = |(_, comment): &(u64, &comment::Comment)| comment.lines.overlaps(lines);
        comments.in_file(&self.path).into_iter().find(on_lines).map(|(id, _)| id)
    }

    /// Move the cursor to a row `query` is found on, wrapping around: for `step` 1 the next after
    /// it, for -1 the one before, and for 0 its own row when found there.
    pub fn find(&mut self, query: &str, step: isize) {
        let found = self.found_rows(query);
        let after = found.partition_point(|&row| row < self.cursor);
        let on = found.get(after) == Some(&self.cursor);
        let target = match step {
            0 => after,
            1.. => after + usize::from(on),
            _ => after + found.len().saturating_sub(1),
        };
        if let Some(&row) = found.get(target.checked_rem(found.len()).unwrap_or(0)) {
            self.cursor = row;
        }
    }

    /// Which of the rows `query` is found on the cursor is on, counting from 1, and how many there
    /// are.
    pub fn found(&self, query: &str) -> (Option<usize>, usize) {
        let found = self.found_rows(query);
        let here = found.iter().position(|&row| row == self.cursor);
        (here.map(|index| index + 1), found.len())
    }

    /// The rows of the file, cards aside, whose text holds `query`.
    fn found_rows(&self, query: &str) -> Vec<usize> {
        let holds = |index: &usize| {
            let text = self.chrome(*index).map(|chrome| find::text(self.line(*index), chrome));
            text.is_some_and(|text| !find::ranges(&text, query).is_empty())
        };
        (0..self.shown.len()).filter(holds).collect()
    }

    /// How many spans lead row `index` without being the file's text; `None` for a card's row.
    pub fn chrome(&self, index: usize) -> Option<usize> {
        match self.shown.get(index)? {
            Shown::Source(row) => Some(self.rows[*row].chrome),
            Shown::Card { .. } => None,
        }
    }

    fn selected_rows(&self) -> RangeInclusive<usize> {
        let anchor = self.anchor.unwrap_or(self.cursor);
        anchor.min(self.cursor)..=anchor.max(self.cursor)
    }

    fn line(&self, index: usize) -> &Line<'static> {
        match &self.shown[index] {
            Shown::Source(row) => &self.rows[*row].line,
            Shown::Card { line, .. } => line,
        }
    }

    fn source(&self, index: usize) -> Option<Lines> {
        match self.shown.get(index)? {
            Shown::Source(row) => self.rows[*row].source,
            Shown::Card { .. } => None,
        }
    }

    /// The first source line of row `index`, or of the nearest row above it with one.
    fn source_line(&self, index: usize) -> Option<usize> {
        let above = (0..=index.min(self.last_row())).rev();
        above.filter_map(|index| self.source(index)).map(|source| source.first).next()
    }

    fn last_row(&self) -> usize {
        self.shown.len().saturating_sub(1)
    }

    fn max_scroll(&self, height: usize) -> usize {
        self.shown.len().saturating_sub(height)
    }
}

/// `rows` with each comment on `file` carded under the last row of its lines.
fn with_cards(rows: &[Row], file: &Path, comments: &Comments, width: u16) -> Vec<Shown> {
    let cards: Vec<(usize, u64, Vec<Line<'static>>)> = comments
        .in_file(file)
        .into_iter()
        .map(|(id, comment)| {
            let last = comment.lines.last;
            let shows = |row: &Row| row.source.is_some_and(|source| source.first <= last);
            let under = rows.iter().rposition(shows).unwrap_or(rows.len().saturating_sub(1));
            (under, id, comment::card(comment, width))
        })
        .collect();
    rows.iter()
        .enumerate()
        .flat_map(|(index, _)| {
            let under = cards.iter().filter(move |(row, _, _)| *row == index);
            let card_lines = under.flat_map(|(_, id, lines)| {
                lines.iter().map(|line| Shown::Card { comment: *id, line: line.clone() })
            });
            std::iter::once(Shown::Source(index)).chain(card_lines)
        })
        .collect()
}

/// The first row on screen showing source line `line`, or the first after it when none does.
fn row_for(rows: &[Row], shown: &[Shown], line: usize) -> usize {
    let shows = |item: &Shown| match item {
        Shown::Source(row) => rows[*row].source.is_some_and(|source| source.last >= line),
        Shown::Card { .. } => false,
    };
    shown.iter().position(shows).unwrap_or(shown.len().saturating_sub(1))
}

fn read(path: &Path, bytes: &[u8]) -> (Content, String) {
    // Git's test: a NUL among the first 8000 bytes.
    if bytes.iter().take(8000).any(|byte| *byte == 0) {
        return (Content::Binary, String::new());
    }
    let text = String::from_utf8_lossy(bytes).into_owned();
    if is_markdown(path) {
        return (Content::Markdown, text);
    }
    let lines: Vec<&str> = text.lines().collect();
    (Content::Code(highlight::highlight(path, &lines)), text)
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

    fn lines(doc: &mut Doc, width: u16) -> Vec<Line<'static>> {
        doc.page(width, 100, &Comments::default(), 0)
    }

    #[test]
    fn a_change_on_disk_is_picked_up() {
        let path = temp("change");
        fs::write(&path, "one").unwrap();
        let mut doc = Doc::open(path.clone());
        assert_eq!(lines(&mut doc, 20)[0].to_string(), "one");

        fs::write(&path, "two words").unwrap();
        doc.refresh();
        assert_eq!(lines(&mut doc, 20)[0].to_string(), "two words");
    }

    #[test]
    fn a_deleted_file_keeps_its_last_content_until_it_returns() {
        let path = temp("deleted");
        fs::write(&path, "kept").unwrap();
        let mut doc = Doc::open(path.clone());

        fs::remove_file(&path).unwrap();
        doc.refresh();
        assert!(doc.is_missing());
        assert_eq!(lines(&mut doc, 20)[0].to_string(), "kept");

        fs::write(&path, "back").unwrap();
        doc.refresh();
        assert!(!doc.is_missing());
        assert_eq!(lines(&mut doc, 20)[0].to_string(), "back");
    }

    #[test]
    fn source_shows_numbered_and_a_binary_file_as_a_notice() {
        let dir = temp("kinds").with_file_name("");
        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
        fs::write(dir.join("blob.bin"), [0x89, 0, 1]).unwrap();
        assert_eq!(lines(&mut Doc::open(dir.join("main.rs")), 20)[0].to_string(), "1 fn main() {}");
        assert_eq!(
            lines(&mut Doc::open(dir.join("blob.bin")), 30)[0].to_string(),
            "Binary file, not shown."
        );
    }

    #[test]
    fn scrolling_stops_at_the_last_page_and_takes_the_cursor_along() {
        let path = temp("scroll");
        fs::write(&path, "a\n\nb\n\nc").unwrap();
        let mut doc = Doc::open(path);
        doc.build(20, &Comments::default());
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
        doc.build(20, &Comments::default());
        doc.move_cursor(3, 2);
        assert_eq!((doc.cursor(), doc.scroll), (3, 2));
        doc.move_cursor(10, 2);
        assert_eq!((doc.cursor(), doc.scroll), (4, 3));
        doc.move_cursor(-4, 2);
        assert_eq!((doc.cursor(), doc.scroll), (0, 0));
    }

    #[test]
    fn pointed_lines_come_on_screen_marked_until_the_cursor_leaves_them() {
        let path = temp("point").with_file_name("point.rs");
        fs::write(&path, "1\n2\n3\n4\n5\n6\n7\n8\n").unwrap();
        let mut doc = Doc::open(path);
        doc.point(Lines { first: 4, last: 6 });
        doc.page(20, 3, &Comments::default(), 0);
        assert_eq!((doc.cursor(), doc.scroll), (3, 3));
        assert_eq!(
            (2..7).map(|index| doc.is_pointed(index)).collect::<Vec<_>>(),
            [false, true, true, true, false]
        );
        doc.move_cursor(2, 3);
        doc.page(20, 3, &Comments::default(), 0);
        assert!(doc.is_pointed(4));
        doc.move_cursor(1, 3);
        doc.page(20, 3, &Comments::default(), 0);
        assert!(!doc.is_pointed(4));
    }

    #[test]
    fn a_selection_spans_the_source_lines_between_its_ends() {
        let path = temp("select").with_file_name("select.rs");
        fs::write(&path, "one\ntwo\nthree\n").unwrap();
        let mut doc = Doc::open(path);
        doc.build(20, &Comments::default());
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
        doc.build(6, &Comments::default());
        doc.move_cursor(3, 10);
        assert_eq!(doc.selection(), Some(Lines::one(3)));
        doc.move_cursor(-2, 10);
        assert_eq!(doc.selection(), Some(Lines::one(2)));
        assert!(doc.is_selected(1) && doc.is_selected(2) && !doc.is_selected(3));

        doc.build(20, &Comments::default());
        assert_eq!(doc.cursor(), 1);
        assert_eq!(doc.selection(), Some(Lines::one(2)));
    }

    #[test]
    fn a_comment_s_card_sits_under_its_last_line_and_the_cursor_can_land_on_it() {
        let path = temp("cards").with_file_name("cards.rs");
        fs::write(&path, "a\nb\nc\n").unwrap();
        let mut doc = Doc::open(path.clone());
        let mut comments = Comments::default();
        comments.add(comment::Comment {
            file: path,
            lines: Lines { first: 1, last: 2 },
            snippet: "a\nb".to_owned(),
            text: "hm".to_owned(),
            outdated: false,
        });
        let shown: Vec<String> =
            doc.page(12, 10, &comments, 0).iter().map(Line::to_string).collect();
        assert_eq!(shown, ["1 a", "2 b", "╭ L1–2 ────╮", "│ hm       │", "╰──────────╯", "3 c"]);
        assert_eq!(doc.comment_at_cursor(&comments), Some(0));
        doc.move_cursor(3, 10);
        assert_eq!((doc.selection(), doc.comment_at_cursor(&comments)), (None, Some(0)));
        doc.move_cursor(2, 10);
        assert_eq!(
            (doc.selection(), doc.comment_at_cursor(&comments)),
            (Some(Lines::one(3)), None)
        );
    }
}
