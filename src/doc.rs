//! The file the pane shows, kept in step with the disk, with its comments' outlines among its rows.

use std::fs;
use std::ops::{Range, RangeInclusive};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui_image::picker::Picker;
use unicode_width::UnicodeWidthChar;

use crate::comment::{self, Comments};
use crate::layout::{self, Lines, Row};
use crate::picture::{self, Picture};
use crate::{code, find, highlight, markdown};

/// What a file looked like on disk the last time it was read.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    modified: SystemTime,
    len: u64,
}

/// What the file holds, as the pane shows it.
enum Content {
    /// Each line styled by its grammar, as for code.
    Markdown(Vec<Vec<Span<'static>>>),
    /// Each line styled by its grammar.
    Code(Vec<Vec<Span<'static>>>),
    Image(Picture),
    Binary,
}

/// A character of the file's text on screen: its row, and how many characters into the row's
/// text it is.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Point {
    row: usize,
    char: usize,
}

/// Text dragged over with the mouse, both ends included.
#[derive(Clone, Copy)]
struct Drag {
    anchor: Point,
    extent: Point,
    /// Whether the button is still down, so the extent still follows the pointer.
    held: bool,
}

impl Drag {
    /// Its ends in the order they read.
    fn ordered(self) -> (Point, Point) {
        (self.anchor.min(self.extent), self.anchor.max(self.extent))
    }
}

/// One row on screen: one of the file's rows, or a line of a comment's outline.
enum Shown {
    Source(usize),
    Card { comment: u64, line: Line<'static> },
}

/// Where the cursor and the view were, to put them back.
#[derive(Clone, Copy)]
pub struct Spot {
    cursor: usize,
    scroll: usize,
}

pub struct Doc {
    pub path: PathBuf,
    content: Content,
    /// The file's text, empty for an image or a binary file.
    text: String,
    /// The file's bytes as `comment::hash` gives them.
    hash: u64,
    /// What an image is drawn with.
    picker: Picker,
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
    /// Dropped whenever the rows are laid out again, since its points are rows on screen.
    drag: Option<Drag>,
    /// Whether markdown is laid out rendered rather than as its source.
    rendered: bool,
}

impl Doc {
    pub fn open(path: PathBuf, picker: &Picker) -> Self {
        let mut doc = Self {
            path,
            content: Content::Binary,
            text: String::new(),
            hash: 0,
            picker: picker.clone(),
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
            drag: None,
            rendered: true,
        };
        doc.refresh();
        doc
    }

    pub fn is_markdown(&self) -> bool {
        matches!(self.content, Content::Markdown(_))
    }

    pub fn is_image(&self) -> bool {
        matches!(self.content, Content::Image(_))
    }

    /// Take in what an image's decoding finished since the last call.
    pub fn poll(&mut self) {
        if let Some(picture) = self.picture() {
            picture.poll();
        }
    }

    pub fn picture(&mut self) -> Option<&mut Picture> {
        match &mut self.content {
            Content::Image(picture) => Some(picture),
            _ => None,
        }
    }

    /// Lay markdown out rendered or as its source from the next layout on, keeping the cursor
    /// on the source line it is on.
    pub fn set_rendered(&mut self, rendered: bool) {
        if rendered != self.rendered {
            self.rendered = rendered;
            self.width = 0;
        }
    }

    pub fn is_missing(&self) -> bool {
        self.stamp.is_none()
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn hash(&self) -> u64 {
        self.hash
    }

    /// Draw images with `picker` from now on, starting over the one it shows.
    pub fn use_picker(&mut self, picker: &Picker) {
        self.picker = picker.clone();
        if self.is_image() {
            // Forgetting when it was read makes the next refresh read it again.
            self.stamp = None;
            self.refresh();
        }
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
        self.hash = comment::hash(&source);
        (self.content, self.text) = read(&self.path, source, &self.picker);
        self.width = 0;
        true
    }

    /// Lay the file out for `width` columns with `comments`' outlines around their lines, keeping
    /// the cursor and selection on the source lines they were on.
    pub fn build(&mut self, width: u16, comments: &Comments) {
        if width == self.width && self.revision == Some(comments.revision()) {
            return;
        }
        let cursor_line = self.source_line(self.cursor);
        let anchor_line = self.anchor.and_then(|anchor| self.source_line(anchor));
        let cursor_on_screen = self.cursor.saturating_sub(self.scroll);
        self.drag = None;
        let nests = comment::nests(comments, &self.path);
        let sides = match &self.content {
            Content::Markdown(_) if self.rendered => comment::Sides::Inset,
            Content::Markdown(lines) | Content::Code(lines) => {
                comment::Sides::Gutter(lines.len().max(1).to_string().len() + 1)
            }
            _ => comment::Sides::Inset,
        };
        self.rows = match &self.content {
            Content::Markdown(_) if self.rendered => {
                comment::frame_rows(markdown::render(&self.text, width), width, &nests)
            }
            Content::Markdown(lines) | Content::Code(lines) => {
                code::layout(lines, width, &comment::depths(&nests, lines.len()))
            }
            Content::Image(_) => Vec::new(),
            Content::Binary => vec![Row {
                line: Line::styled(
                    "Binary file, not shown.",
                    Style::new().add_modifier(Modifier::DIM),
                ),
                source: None,
                chrome: 1,
                trail: 0,
            }],
        };
        self.width = width;
        self.shown = with_outlines(&self.rows, &self.path, comments, width, &nests, sides);
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

    pub fn spot(&self) -> Spot {
        Spot { cursor: self.cursor, scroll: self.scroll }
    }

    pub fn return_to(&mut self, spot: Spot) {
        self.cursor = spot.cursor.min(self.last_row());
        self.scroll = spot.scroll;
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

    /// The comment whose card the cursor is on, or else the first on the cursor's lines, or on
    /// the whole of an image.
    pub fn comment_at_cursor(&self, comments: &Comments) -> Option<u64> {
        if let Some(Shown::Card { comment, .. }) = self.shown.get(self.cursor) {
            return Some(*comment);
        }
        let lines = self.source(self.cursor);
        let here = |(_, comment): &(u64, &comment::Comment)| match comment.anchor.lines() {
            Some(anchor) => lines.is_some_and(|lines| anchor.overlaps(lines)),
            None => self.is_image(),
        };
        comments.in_file(&self.path).into_iter().find(here).map(|(id, _)| id)
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
            let text = self.chrome(*index).map(|chrome| {
                layout::text(self.line(*index), chrome, self.trail(*index).unwrap_or(0))
            });
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

    /// How many spans trail row `index` without being the file's text; `None` for a card's row.
    pub fn trail(&self, index: usize) -> Option<usize> {
        match self.shown.get(index)? {
            Shown::Source(row) => Some(self.rows[*row].trail),
            Shown::Card { .. } => None,
        }
    }

    /// Put the cursor on the row `offset` rows down the page and start dragging from the
    /// character `column` columns across it.
    pub fn press(&mut self, offset: usize, column: usize, height: usize) {
        self.click(offset, height);
        let point = self.point_at(self.scroll + offset, column);
        self.drag = point.map(|point| Drag { anchor: point, extent: point, held: true });
    }

    /// Carry a held drag to the character under the pointer, `offset` rows down the page and
    /// `column` columns across, first scrolling a row when the pointer is above or below it.
    pub fn drag_to(&mut self, offset: isize, column: usize, height: usize) {
        if !self.drag.is_some_and(|drag| drag.held) {
            return;
        }
        let past = isize::try_from(height.max(1)).unwrap_or(isize::MAX);
        if offset < 0 || offset >= past {
            self.scroll_by(offset.signum(), height);
        }
        let row = self.scroll.saturating_add_signed(offset.clamp(0, past - 1)).min(self.last_row());
        if let (Some(point), Some(drag)) = (self.point_at(row, column), &mut self.drag) {
            drag.extent = point;
        }
    }

    /// Let go of a held drag: the text it covers, which stays marked, or `None` when it never
    /// left the character it started on.
    pub fn release(&mut self) -> Option<String> {
        let drag = self.drag.as_mut().filter(|drag| drag.held)?;
        if drag.anchor == drag.extent {
            self.drag = None;
            return None;
        }
        drag.held = false;
        let (from, to) = drag.ordered();
        let texts = (from.row..=to.row).filter_map(|row| Some((row, self.text_of(row)?)));
        let pieces = texts.map(|(row, text)| {
            let first = if row == from.row { from.char } else { 0 };
            let last = if row == to.row { to.char } else { usize::MAX };
            text.chars().skip(first).take(last.saturating_add(1) - first).collect::<String>()
        });
        Some(pieces.collect::<Vec<_>>().join("\n"))
    }

    pub fn clear_drag(&mut self) {
        self.drag = None;
    }

    /// The bytes of row `index`'s text the drag covers.
    pub fn dragged(&self, index: usize) -> Option<Range<usize>> {
        let (from, to) = self.drag?.ordered();
        if !(from.row..=to.row).contains(&index) {
            return None;
        }
        let text = self.text_of(index)?;
        let byte = |char: usize| text.char_indices().nth(char).map_or(text.len(), |(at, _)| at);
        let first = if index == from.row { byte(from.char) } else { 0 };
        let end = if index == to.row { byte(to.char + 1) } else { text.len() };
        Some(first..end)
    }

    /// The character of row `index`'s text `column` columns across the page, the nearest one
    /// when the column is in the margin or past the end; `None` for a card's row.
    fn point_at(&self, index: usize, column: usize) -> Option<Point> {
        let chrome = self.chrome(index)?;
        let line = self.line(index);
        let margin: usize = line.spans.iter().take(chrome).map(Span::width).sum();
        let trail = self.trail(index).unwrap_or(0);
        let widths: Vec<usize> =
            layout::text(line, chrome, trail).chars().map(|ch| ch.width().unwrap_or(0)).collect();
        let mut left = column.saturating_sub(margin);
        let under = widths.iter().position(|&width| {
            let here = left < width;
            left = left.saturating_sub(width);
            here
        });
        Some(Point { row: index, char: under.unwrap_or(widths.len().saturating_sub(1)) })
    }

    /// Row `index`'s text past its line number or bullet; `None` for a card's row.
    fn text_of(&self, index: usize) -> Option<String> {
        Some(layout::text(self.line(index), self.chrome(index)?, self.trail(index).unwrap_or(0)))
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

/// `rows` with each line comment outlined around its lines, nested when they overlap, and a
/// whole-file comment after them all.
fn with_outlines(
    rows: &[Row],
    file: &Path,
    comments: &Comments,
    width: u16,
    nests: &[comment::Nest],
    sides: comment::Sides,
) -> Vec<Shown> {
    let width_cols = usize::from(width);
    let mut shown = Vec::new();
    if !rows.is_empty() {
        let mut flat = Vec::new();
        flatten(nests, 1, &mut flat);
        let mut opens: Vec<Vec<(usize, &comment::Nest)>> = vec![Vec::new(); rows.len()];
        let mut closes: Vec<Vec<(usize, &comment::Nest)>> = vec![Vec::new(); rows.len()];
        for &(depth, nest) in &flat {
            let (first, last) =
                comment::band(rows, nest.drawn()).unwrap_or((rows.len() - 1, rows.len() - 1));
            opens[first].push((depth, nest));
            closes[last].push((depth, nest));
        }
        for list in &mut opens {
            list.sort_by_key(|(depth, _)| *depth);
        }
        for list in &mut closes {
            list.sort_by_key(|(depth, _)| std::cmp::Reverse(*depth));
        }
        let emit = |depth: usize, nest: &comment::Nest, line: Line<'static>| Shown::Card {
            comment: nest.id,
            line: comment::framed(line, depth, sides),
        };
        for (index, _) in rows.iter().enumerate() {
            for &(depth, nest) in &opens[index] {
                let inner = comment::box_width(sides, depth, width_cols);
                shown.push(emit(depth, nest, comment::outline_top(nest.comment, inner)));
            }
            shown.push(Shown::Source(index));
            for &(depth, nest) in &closes[index] {
                let inner = comment::box_width(sides, depth, width_cols);
                shown.extend(
                    comment::outline_tail(nest.comment, inner)
                        .into_iter()
                        .map(|line| emit(depth, nest, line)),
                );
            }
        }
    }
    comments
        .in_file(file)
        .into_iter()
        .filter(|(_, comment)| comment.anchor.lines().is_none())
        .flat_map(|(id, file_comment)| {
            comment::card(file_comment, width)
                .into_iter()
                .map(move |line| Shown::Card { comment: id, line })
        })
        .for_each(|card| shown.push(card));
    shown
}

fn flatten<'a>(
    nests: &'a [comment::Nest<'a>],
    depth: usize,
    out: &mut Vec<(usize, &'a comment::Nest<'a>)>,
) {
    for nest in nests {
        out.push((depth, nest));
        flatten(&nest.children, depth + 1, out);
    }
}

/// The first row on screen showing source line `line`, or the first after it when none does.
fn row_for(rows: &[Row], shown: &[Shown], line: usize) -> usize {
    let shows = |item: &Shown| match item {
        Shown::Source(row) => rows[*row].source.is_some_and(|source| source.last >= line),
        Shown::Card { .. } => false,
    };
    shown.iter().position(shows).unwrap_or(shown.len().saturating_sub(1))
}

fn read(path: &Path, bytes: Vec<u8>, picker: &Picker) -> (Content, String) {
    if let Some(format) = picture::format(&bytes) {
        return (Content::Image(Picture::load(bytes, format, picker.clone())), String::new());
    }
    // Git's test: a NUL among the first 8000 bytes.
    if bytes.iter().take(8000).any(|byte| *byte == 0) {
        return (Content::Binary, String::new());
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let lines: Vec<&str> = text.lines().collect();
    let styled = highlight::highlight(path, &lines);
    match is_markdown(path) {
        true => (Content::Markdown(styled), text),
        false => (Content::Code(styled), text),
    }
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
    fn a_drag_copies_from_the_character_it_started_on_to_the_one_it_ended_on() {
        let path = temp("drag");
        fs::write(&path, "alpha beta\n\ngamma delta").unwrap();
        let mut doc = Doc::open(path, &Picker::halfblocks());
        lines(&mut doc, 20);
        doc.press(2, 8, 100);
        doc.drag_to(0, 6, 100);
        assert_eq!(doc.release().as_deref(), Some("beta\n\ngamma del"));
        assert_eq!(doc.dragged(0), Some(6..10));
        assert_eq!(doc.dragged(2), Some(0..9));

        doc.press(0, 30, 100);
        assert_eq!(doc.release(), None, "a click past the end copies nothing");
        assert_eq!(doc.dragged(0), None);
    }

    #[test]
    fn markdown_flips_to_its_source_and_back_keeping_the_cursor_on_its_line() {
        let path = temp("flip");
        fs::write(&path, "# Title\n\ntext").unwrap();
        let mut doc = Doc::open(path, &Picker::halfblocks());
        assert_eq!(lines(&mut doc, 20)[0].to_string(), "Title");
        doc.go_to_line(3);
        lines(&mut doc, 20);

        doc.set_rendered(false);
        let source: Vec<String> = lines(&mut doc, 20).iter().map(Line::to_string).collect();
        assert_eq!(source, ["1 # Title", "2 ", "3 text"]);
        assert_eq!(doc.cursor(), 2);

        doc.set_rendered(true);
        assert_eq!(lines(&mut doc, 20)[doc.cursor()].to_string(), "text");
    }

    #[test]
    fn a_change_on_disk_is_picked_up() {
        let path = temp("change");
        fs::write(&path, "one").unwrap();
        let mut doc = Doc::open(path.clone(), &Picker::halfblocks());
        assert_eq!(lines(&mut doc, 20)[0].to_string(), "one");

        fs::write(&path, "two words").unwrap();
        doc.refresh();
        assert_eq!(lines(&mut doc, 20)[0].to_string(), "two words");
    }

    #[test]
    fn a_deleted_file_keeps_its_last_content_until_it_returns() {
        let path = temp("deleted");
        fs::write(&path, "kept").unwrap();
        let mut doc = Doc::open(path.clone(), &Picker::halfblocks());

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
        assert_eq!(
            lines(&mut Doc::open(dir.join("main.rs"), &Picker::halfblocks()), 20)[0].to_string(),
            "1 fn main() {}"
        );
        assert_eq!(
            lines(&mut Doc::open(dir.join("blob.bin"), &Picker::halfblocks()), 30)[0].to_string(),
            "Binary file, not shown."
        );
    }

    #[test]
    fn an_image_opens_as_a_picture_whatever_its_name() {
        let path = temp("image").with_file_name("misnamed.txt");
        fs::write(&path, picture::png(4, 4)).unwrap();
        let mut doc = Doc::open(path, &Picker::halfblocks());
        assert!(doc.is_image());
        assert!(lines(&mut doc, 20).is_empty());
    }

    #[test]
    fn a_comment_on_an_image_cards_on_its_own_and_is_at_the_cursor() {
        let path = temp("image comment").with_file_name("logo.png");
        let bytes = picture::png(4, 4);
        fs::write(&path, &bytes).unwrap();
        let mut doc = Doc::open(path.clone(), &Picker::halfblocks());
        let mut comments = Comments::default();
        comments.add(comment::Comment {
            file: path,
            anchor: comment::Anchor::File { hash: comment::hash(&bytes) },
            text: "bigger".to_owned(),
            outdated: false,
        });
        let page = doc.page(20, 10, &comments, 0);
        assert_eq!(page.first().map(ToString::to_string).as_deref(), Some("╭ image ───────────╮"));
        assert!(doc.comment_at_cursor(&comments).is_some());
    }

    #[test]
    fn scrolling_stops_at_the_last_page_and_takes_the_cursor_along() {
        let path = temp("scroll");
        fs::write(&path, "a\n\nb\n\nc").unwrap();
        let mut doc = Doc::open(path, &Picker::halfblocks());
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
        let mut doc = Doc::open(path, &Picker::halfblocks());
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
        let mut doc = Doc::open(path, &Picker::halfblocks());
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
        let mut doc = Doc::open(path, &Picker::halfblocks());
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
        let mut doc = Doc::open(path, &Picker::halfblocks());
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
    fn a_comment_outlines_its_lines_and_the_cursor_can_land_on_the_box() {
        let path = temp("cards").with_file_name("cards.rs");
        fs::write(&path, "a\nb\nc\n").unwrap();
        let mut doc = Doc::open(path.clone(), &Picker::halfblocks());
        let mut comments = Comments::default();
        comments.add(comment::Comment {
            file: path,
            anchor: comment::Anchor::Lines {
                lines: Lines { first: 1, last: 2 },
                snippet: "a\nb".to_owned(),
            },
            text: "hm".to_owned(),
            outdated: false,
        });
        let shown: Vec<String> =
            doc.page(12, 10, &comments, 0).iter().map(Line::to_string).collect();
        assert_eq!(
            shown,
            [
                "╭ L1–2 ────╮",
                "│ a        │",
                "│ b        │",
                "├──────────┤",
                "│ hm       │",
                "╰──────────╯",
                "3 c",
            ]
        );
        assert_eq!(doc.comment_at_cursor(&comments), Some(0));
        doc.move_cursor(1, 10);
        assert_eq!(doc.comment_at_cursor(&comments), Some(0));
        doc.move_cursor(5, 10);
        assert_eq!(
            (doc.selection(), doc.comment_at_cursor(&comments)),
            (Some(Lines::one(3)), None)
        );
    }

    #[test]
    fn a_comment_on_rendered_markdown_insets_the_lines() {
        let path = temp("md comment").with_file_name("note.md");
        fs::write(&path, "hello\n\nworld\n").unwrap();
        let mut doc = Doc::open(path.clone(), &Picker::halfblocks());
        let mut comments = Comments::default();
        comments.add(comment::Comment {
            file: path,
            anchor: comment::Anchor::Lines {
                lines: Lines { first: 1, last: 1 },
                snippet: "hello".to_owned(),
            },
            text: "hi".to_owned(),
            outdated: false,
        });
        let shown: Vec<String> =
            doc.page(12, 10, &comments, 0).iter().map(Line::to_string).collect();
        assert_eq!(shown[0], "╭ L1 ──────╮");
        assert!(shown.iter().any(|row| row.starts_with("│ hello")));
        assert!(shown.iter().any(|row| row.contains("hi")));
        assert!(shown.iter().any(|row| row == "world"));
    }
}
