//! Comments on the project's files, kept for the session, and the text they're sent as.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

use crate::layout::{Lines, Row, chunk, wrap_spans};

/// What in its file a comment is on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Anchor {
    Lines {
        lines: Lines,
        /// The lines' text when the comment was written, which it follows when the file changes.
        snippet: String,
    },
    /// The whole file, as `hash` of its bytes when the comment was written.
    File { hash: u64 },
}

impl Anchor {
    pub fn lines(&self) -> Option<Lines> {
        match self {
            Self::Lines { lines, .. } => Some(*lines),
            Self::File { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comment {
    pub file: PathBuf,
    pub anchor: Anchor,
    pub text: String,
    /// Whether the file no longer holds what the anchor was written on: for lines, leaving them
    /// where they last were.
    pub outdated: bool,
}

impl Comment {
    /// `path:start-end`, `path:line` for one line, or `path` for the whole file, the path
    /// relative to `root` when inside it.
    pub fn location(&self, root: &Path) -> String {
        let path = self.file.strip_prefix(root).unwrap_or(&self.file).display();
        match self.anchor.lines() {
            Some(Lines { first, last }) if first == last => format!("{path}:{first}"),
            Some(Lines { first, last }) => format!("{path}:{first}-{last}"),
            None => path.to_string(),
        }
    }

    /// Where it sorts in its file: its first line, or before every line for the whole file.
    pub fn line(&self) -> usize {
        self.anchor.lines().map_or(0, |lines| lines.first)
    }

    /// Follow what the anchor was written on in the file now holding `source`, its bytes
    /// hashing to `hash`: move onto the snippet's lines, the nearest match when it appears more
    /// than once, and mark the comment outdated while they or the file's bytes are gone.
    fn reanchor(&mut self, source: &str, hash: u64) {
        match &mut self.anchor {
            Anchor::Lines { lines, snippet } => {
                let source: Vec<&str> = source.lines().collect();
                let snippet: Vec<&str> = snippet.split('\n').collect();
                let nearest = source
                    .windows(snippet.len())
                    .enumerate()
                    .filter(|(_, window)| *window == snippet.as_slice())
                    .map(|(index, _)| index + 1)
                    .min_by_key(|first| first.abs_diff(lines.first));
                if let Some(first) = nearest {
                    *lines = Lines { first, last: first + snippet.len() - 1 };
                }
                self.outdated = nearest.is_none();
            }
            Anchor::File { hash: written } => self.outdated = *written != hash,
        }
    }
}

/// What a whole-file comment remembers of `bytes`, to tell when the file has changed.
pub fn hash(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// The session's comments, each under an id never reused.
#[derive(Default)]
pub struct Comments {
    items: Vec<(u64, Comment)>,
    next_id: u64,
    /// Counts every change, so a view built from the comments knows when it is stale.
    revision: u64,
}

impl Comments {
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn add(&mut self, comment: Comment) {
        self.items.push((self.next_id, comment));
        self.next_id += 1;
        self.revision += 1;
    }

    pub fn get(&self, id: u64) -> Option<&Comment> {
        self.items.iter().find(|(item, _)| *item == id).map(|(_, comment)| comment)
    }

    pub fn set_text(&mut self, id: u64, text: String) {
        if let Some((_, comment)) = self.items.iter_mut().find(|(item, _)| *item == id) {
            comment.text = text;
            self.revision += 1;
        }
    }

    pub fn remove(&mut self, id: u64) {
        self.items.retain(|(item, _)| *item != id);
        self.revision += 1;
    }

    /// Every comment, by file and then by line, a whole-file comment first in its file.
    pub fn sorted(&self) -> Vec<(u64, &Comment)> {
        let mut sorted: Vec<(u64, &Comment)> =
            self.items.iter().map(|(id, comment)| (*id, comment)).collect();
        sorted.sort_by(|(_, a), (_, b)| (&a.file, a.line()).cmp(&(&b.file, b.line())));
        sorted
    }

    /// The comments on `file`, by line.
    pub fn in_file(&self, file: &Path) -> Vec<(u64, &Comment)> {
        self.sorted().into_iter().filter(|(_, comment)| comment.file == file).collect()
    }

    /// Every file a comment is on.
    pub fn files(&self) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> =
            self.items.iter().map(|(_, comment)| comment.file.clone()).collect();
        files.sort();
        files.dedup();
        files
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.revision += 1;
    }

    /// Follow each comment on `file` to what it was written on in `source`, the file's text,
    /// its bytes hashing to `hash`.
    pub fn reanchor(&mut self, file: &Path, source: &str, hash: u64) {
        let before: Vec<(Option<Lines>, bool)> = self.items.iter().map(Self::place).collect();
        self.items
            .iter_mut()
            .filter(|(_, comment)| comment.file == file)
            .for_each(|(_, comment)| comment.reanchor(source, hash));
        let after = self.items.iter().map(Self::place);
        if !after.eq(before) {
            self.revision += 1;
        }
    }

    /// What reanchoring can change about an item.
    fn place((_, comment): &(u64, Comment)) -> (Option<Lines>, bool) {
        (comment.anchor.lines(), comment.outdated)
    }
}

/// `count` comments, in words.
pub fn counted(count: usize) -> String {
    let noun = if count == 1 { "comment" } else { "comments" };
    format!("{count} {noun}")
}

/// `comments` as one message: each its location, the lines it quotes if it is on lines, then
/// what it says, blank-line separated.
pub fn format_all(comments: &[&Comment], root: &Path) -> String {
    comments.iter().map(|comment| format(comment, root)).collect::<Vec<_>>().join("\n\n")
}

/// An outdated comment still quotes the lines it was written on, marked so the reader knows
/// they're gone.
fn format(comment: &Comment, root: &Path) -> String {
    let location = comment.location(root);
    let location = if comment.outdated { format!("{location} (outdated)") } else { location };
    let text: Vec<&str> =
        comment.text.lines().map(str::trim_end).filter(|line| !line.trim().is_empty()).collect();
    match &comment.anchor {
        Anchor::Lines { snippet, .. } => format!("{location}\n{snippet}\n{}", text.join("\n")),
        Anchor::File { .. } => format!("{location}\n{}", text.join("\n")),
    }
}

/// How a comment box takes columns on the left of the source.
#[derive(Clone, Copy)]
pub enum Sides {
    /// The left wall replaces a line-number gutter of this many columns.
    Gutter(usize),
    /// The left wall is a pipe and a space, plus one pipe per extra nest.
    Inset,
}

/// A comment being written, outlined like a saved one until it is saved or cancelled.
#[derive(Clone, Copy)]
pub struct Draft<'a> {
    pub comment: &'a Comment,
    pub text: &'a str,
    pub edit: Option<u64>,
}

/// Id of a new comment that is not in the session yet.
pub const DRAFT: u64 = u64::MAX;

/// A line comment on `file`, with later overlapping comments nested inside earlier ones.
pub struct Nest<'a> {
    pub id: u64,
    pub comment: &'a Comment,
    pub color: Color,
    pub children: Vec<Nest<'a>>,
}

impl Nest<'_> {
    pub fn lines(&self) -> Lines {
        self.comment.anchor.lines().expect("nests are line comments")
    }

    /// Its own lines and every nested comment's, so the box can close after the innermost.
    pub fn drawn(&self) -> Lines {
        self.children.iter().map(Self::drawn).fold(self.lines(), Lines::union)
    }
}

/// Line comments on `file`, nested: a later comment that overlaps an earlier one sits inside it.
/// `draft` is outlined in cyan; a comment being edited is replaced by it.
pub fn nests<'a>(comments: &'a Comments, file: &Path, draft: Option<Draft<'a>>) -> Vec<Nest<'a>> {
    let editing = draft.and_then(|draft| draft.edit);
    let mut items: Vec<(u64, &'a Comment, Color)> = comments
        .in_file(file)
        .into_iter()
        .filter(|(id, _)| editing != Some(*id))
        .map(|(id, comment)| (id, comment, Color::Yellow))
        .collect();
    if let Some(draft) = draft {
        if draft.comment.file == file && draft.comment.anchor.lines().is_some() {
            items.push((draft.edit.unwrap_or(DRAFT), draft.comment, Color::Cyan));
            items.sort_by_key(|(id, comment, _)| (comment.line(), *id));
        }
    }
    let mut roots = Vec::new();
    for (id, comment, color) in items {
        let Some(lines) = comment.anchor.lines() else { continue };
        if !attach(&mut roots, id, comment, color, lines) {
            roots.push(Nest { id, comment, color, children: Vec::new() });
        }
    }
    roots
}

fn attach<'a>(
    nodes: &mut Vec<Nest<'a>>,
    id: u64,
    comment: &'a Comment,
    color: Color,
    lines: Lines,
) -> bool {
    for node in nodes.iter_mut().rev() {
        if attach(&mut node.children, id, comment, color, lines) {
            return true;
        }
        if node.lines().overlaps(lines) {
            node.children.push(Nest { id, comment, color, children: Vec::new() });
            return true;
        }
    }
    false
}

/// The wall colors enclosing each 1-based source line, outer first.
pub fn stacks(nests: &[Nest], lines: usize) -> Vec<Vec<Color>> {
    let mut stacks = vec![Vec::new(); lines];
    for nest in nests {
        paint_stack(nest, &[], &mut stacks);
    }
    stacks
}

fn paint_stack(nest: &Nest, above: &[Color], stacks: &mut [Vec<Color>]) {
    let mut stack = above.to_vec();
    stack.push(nest.color);
    let lines = nest.drawn();
    let len = stacks.len();
    let first = lines.first.min(len).saturating_sub(1);
    let end = lines.last.min(len);
    if let Some(slot) = stacks.get_mut(first..end) {
        for at in slot {
            if stack.len() > at.len() {
                *at = stack.clone();
            }
        }
    }
    for child in &nest.children {
        paint_stack(child, &stack, stacks);
    }
}

/// Left and right wall columns at `depth`; both 0 when the line is not in a box.
pub fn fills(sides: Sides, depth: usize) -> (usize, usize) {
    if depth == 0 {
        return (0, 0);
    }
    let left = match sides {
        Sides::Gutter(gutter) => gutter + depth - 1,
        Sides::Inset => depth + 1,
    };
    (left, depth + 1)
}

/// `content` padded to `inner` columns between walls of `colors`, `left_fill` columns on the left.
pub fn box_row(
    content: Vec<Span<'static>>,
    colors: &[Color],
    left_fill: usize,
    inner: usize,
    source: Option<Lines>,
) -> Row {
    box_row_with_lead(Vec::new(), content, colors, left_fill, inner, source)
}

fn box_row_with_lead(
    lead: Vec<Span<'static>>,
    content: Vec<Span<'static>>,
    colors: &[Color],
    left_fill: usize,
    inner: usize,
    source: Option<Lines>,
) -> Row {
    let lead_n = lead.len();
    let mut mid = lead;
    mid.extend(content);
    let used: usize = mid.iter().map(Span::width).sum();
    let pad = inner.saturating_sub(used);
    let mut spans = left_wall(colors, left_fill);
    let chrome = spans.len() + lead_n;
    spans.extend(mid);
    let right = right_wall(colors, pad);
    let trail = right.len();
    spans.extend(right);
    Row { line: Line::from(spans), source, chrome, trail }
}

fn border(color: Color) -> Style {
    Style::new().fg(color)
}

fn left_wall(colors: &[Color], fill: usize) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> =
        colors.iter().map(|&color| Span::styled("│", border(color))).collect();
    let pad = fill.saturating_sub(colors.len());
    if pad > 0 {
        let color = colors.last().copied().unwrap_or(Color::Yellow);
        spans.push(Span::styled(" ".repeat(pad), border(color)));
    }
    spans
}

fn right_wall(colors: &[Color], pad: usize) -> Vec<Span<'static>> {
    let inner = colors.last().copied().unwrap_or(Color::Yellow);
    let mut spans = vec![Span::styled(format!("{} ", " ".repeat(pad)), border(inner))];
    spans.extend(colors.iter().rev().map(|&color| Span::styled("│", border(color))));
    spans
}

/// Wrap each row that sits in a comment's band in walls, rewrapping its text to the inner width.
pub fn frame_rows(rows: Vec<Row>, width: u16, nests: &[Nest]) -> Vec<Row> {
    if nests.is_empty() {
        return rows;
    }
    let width = usize::from(width);
    let stacks = row_stacks(&rows, nests);
    rows.into_iter()
        .enumerate()
        .flat_map(|(index, row)| inset_row(row, &stacks[index], width))
        .collect()
}

fn row_stacks(rows: &[Row], nests: &[Nest]) -> Vec<Vec<Color>> {
    let mut stacks = vec![Vec::new(); rows.len()];
    fn paint(nest: &Nest, above: &[Color], rows: &[Row], stacks: &mut [Vec<Color>]) {
        let mut stack = above.to_vec();
        stack.push(nest.color);
        if let Some((first, last)) = band(rows, nest.drawn()) {
            for at in stacks.get_mut(first..=last).into_iter().flatten() {
                if stack.len() > at.len() {
                    *at = stack.clone();
                }
            }
        }
        for child in &nest.children {
            paint(child, &stack, rows, stacks);
        }
    }
    for nest in nests {
        paint(nest, &[], rows, &mut stacks);
    }
    stacks
}

/// First and last row whose source overlaps `lines`, including the gaps between them.
pub fn band(rows: &[Row], lines: Lines) -> Option<(usize, usize)> {
    let hits = |row: &Row| row.source.is_some_and(|source| source.overlaps(lines));
    Some((rows.iter().position(hits)?, rows.iter().rposition(hits)?))
}

fn inset_row(row: Row, colors: &[Color], width: usize) -> Vec<Row> {
    if colors.is_empty() {
        return vec![row];
    }
    let (left_fill, right_fill) = fills(Sides::Inset, colors.len());
    let inner = width.saturating_sub(left_fill + right_fill).max(1);
    let chrome = row.chrome.min(row.line.spans.len());
    let lead = row.line.spans[..chrome].to_vec();
    let content = row.line.spans[chrome..].to_vec();
    let chrome_w: usize = lead.iter().map(Span::width).sum();
    let text_w = inner.saturating_sub(chrome_w).max(1);
    wrap_spans(&content, text_w)
        .into_iter()
        .enumerate()
        .map(|(index, spans)| {
            let lead =
                if index == 0 { lead.clone() } else { vec![Span::raw(" ".repeat(chrome_w))] };
            box_row_with_lead(lead, spans, colors, left_fill, inner, row.source)
        })
        .collect()
}

/// `line` of a box, with the outer walls of `ancestors` around it.
pub fn framed(line: Line<'static>, ancestors: &[Color], sides: Sides) -> Line<'static> {
    if ancestors.is_empty() {
        return line;
    }
    let (left_fill, _) = fills(sides, ancestors.len());
    let mut spans = left_wall(ancestors, left_fill);
    spans.extend(line.spans);
    spans.extend(right_wall(ancestors, 0));
    Line::from(spans)
}

/// Inner width of a box at `depth` in a pane `width` columns wide.
pub fn box_width(sides: Sides, depth: usize, width: usize) -> usize {
    let ancestor = depth.saturating_sub(1);
    let (left, right) = fills(sides, ancestor);
    width.saturating_sub(left + right).max(5)
}

fn title(comment: &Comment) -> String {
    let mut title = match comment.anchor.lines() {
        Some(Lines { first, last }) if first == last => format!("L{first}"),
        Some(Lines { first, last }) => format!("L{first}–{last}"),
        None => "image".to_owned(),
    };
    if comment.outdated {
        title.push_str(" · outdated");
    }
    title
}

fn boxed_top(title: &str, width: usize, color: Color) -> Line<'static> {
    let width = width.max(5);
    let title_width = title.chars().count().min(width - 4);
    let title: String = title.chars().take(title_width).collect();
    Line::from(vec![
        Span::styled("╭ ", border(color)),
        Span::styled(title, border(color).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {}╮", "─".repeat(width - 4 - title_width)), border(color)),
    ])
}

fn boxed_bottom(width: usize, color: Color) -> Line<'static> {
    let width = width.max(5);
    Line::styled(format!("╰{}╯", "─".repeat(width - 2)), border(color))
}

fn boxed_divider(width: usize, color: Color) -> Line<'static> {
    let width = width.max(5);
    Line::styled(format!("├{}┤", "─".repeat(width - 2)), border(color))
}

fn boxed_body(text: &str, width: usize, color: Color, dim_text: bool) -> Vec<Line<'static>> {
    let width = width.max(5);
    let inner = width - 4;
    let style = if dim_text { Style::new().add_modifier(Modifier::DIM) } else { Style::new() };
    text.split('\n')
        .flat_map(|line| chunk(line, inner))
        .map(|line| {
            let pad = inner.saturating_sub(line.chars().filter_map(UnicodeWidthChar::width).sum());
            Line::from(vec![
                Span::styled("│ ", border(color)),
                Span::styled(format!("{line}{}", " ".repeat(pad)), style),
                Span::styled(" │", border(color)),
            ])
        })
        .collect()
}

/// A comment as a card `width` columns wide: its lines, or `image` for a whole-file comment, in
/// the top border, and its text inside.
pub fn card(comment: &Comment, width: u16) -> Vec<Line<'static>> {
    let width = usize::from(width);
    std::iter::once(boxed_top(&title(comment), width, Color::Yellow))
        .chain(boxed_body(&comment.text, width, Color::Yellow, false))
        .chain([boxed_bottom(width, Color::Yellow)])
        .collect()
}

pub fn outline_top(comment: &Comment, width: usize, color: Color) -> Line<'static> {
    boxed_top(&title(comment), width, color)
}

/// Divider, body of `text`, and bottom. Body rows are the ones the caret can sit on.
pub fn outline_tail(
    text: &str,
    width: usize,
    color: Color,
    placeholder: bool,
) -> Vec<(Line<'static>, bool)> {
    let display = if placeholder { "Leave a comment…" } else { text };
    let mut lines = vec![(boxed_divider(width, color), false)];
    lines.extend(
        boxed_body(display, width, color, placeholder).into_iter().map(|line| (line, true)),
    );
    lines.push((boxed_bottom(width, color), false));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comment(first: usize, last: usize, snippet: &str) -> Comment {
        Comment {
            file: PathBuf::from("/p/src/a.rs"),
            anchor: Anchor::Lines { lines: Lines { first, last }, snippet: snippet.to_owned() },
            text: "why?".to_owned(),
            outdated: false,
        }
    }

    #[test]
    fn a_comment_follows_its_lines_to_the_nearest_place_they_now_sit() {
        let mut moved = comment(2, 3, "b\nc");
        moved.reanchor("x\nb\nc\ny\nz\nb\nc", 0);
        assert_eq!(moved.anchor.lines(), Some(Lines { first: 2, last: 3 }));
        moved.reanchor("new\nx\nb\nc\ny\nz\nb\nc", 0);
        assert_eq!(moved.anchor.lines(), Some(Lines { first: 3, last: 4 }));
        assert!(!moved.outdated);
    }

    #[test]
    fn a_comment_whose_lines_are_gone_is_outdated_until_they_return() {
        let mut gone = comment(2, 2, "b");
        gone.reanchor("a\nB\n", 0);
        assert!(gone.outdated);
        assert_eq!(gone.anchor.lines(), Some(Lines::one(2)));
        gone.reanchor("b\n", 0);
        assert!(!gone.outdated);
        assert_eq!(gone.anchor.lines(), Some(Lines::one(1)));
    }

    fn on_image(written: &[u8]) -> Comment {
        Comment {
            file: PathBuf::from("/p/logo.png"),
            anchor: Anchor::File { hash: hash(written) },
            text: "bigger".to_owned(),
            outdated: false,
        }
    }

    #[test]
    fn a_whole_file_comment_is_outdated_while_the_file_differs_from_what_it_was_written_on() {
        let mut image = on_image(b"v1");
        image.reanchor("", hash(b"v2"));
        assert!(image.outdated);
        image.reanchor("", hash(b"v1"));
        assert!(!image.outdated);
    }

    #[test]
    fn a_whole_file_comment_sends_as_its_path_and_text_and_cards_as_image() {
        let mut image = on_image(b"v1");
        assert_eq!(format_all(&[&image], Path::new("/p")), "logo.png\nbigger");
        image.outdated = true;
        assert_eq!(format_all(&[&image], Path::new("/p")), "logo.png (outdated)\nbigger");
        let lines: Vec<String> = card(&image, 22).iter().map(|line| line.to_string()).collect();
        assert_eq!(lines[0], "╭ image · outdated ──╮");
    }

    #[test]
    fn comments_send_as_location_snippet_and_text() {
        let mut first = comment(4, 4, "let x = 1;");
        first.text = "rename\n\nthis  ".to_owned();
        let mut second = comment(7, 9, "a\nb\nc");
        second.outdated = true;
        assert_eq!(
            format_all(&[&first, &second], Path::new("/p")),
            "src/a.rs:4\nlet x = 1;\nrename\nthis\n\nsrc/a.rs:7-9 (outdated)\na\nb\nc\nwhy?"
        );
    }

    #[test]
    fn a_card_boxes_its_text_under_its_lines() {
        let lines: Vec<String> =
            card(&comment(3, 5, ""), 14).iter().map(|line| line.to_string()).collect();
        assert_eq!(lines, ["╭ L3–5 ──────╮", "│ why?       │", "╰────────────╯"]);
    }

    #[test]
    fn an_outline_puts_a_divider_between_the_lines_and_the_text() {
        let tail: Vec<String> = outline_tail("why?", 12, Color::Yellow, false)
            .iter()
            .map(|(line, _)| line.to_string())
            .collect();
        assert_eq!(tail, ["├──────────┤", "│ why?     │", "╰──────────╯"]);
        assert_eq!(outline_top(&comment(1, 2, ""), 12, Color::Yellow).to_string(), "╭ L1–2 ────╮");
    }

    #[test]
    fn a_later_comment_nests_inside_an_earlier_one_it_overlaps() {
        let mut comments = Comments::default();
        comments.add(comment(1, 4, "a"));
        comments.add(comment(2, 3, "b"));
        comments.add(comment(3, 5, "c"));
        let file = Path::new("/p/src/a.rs");
        let roots = nests(&comments, file, None);
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].lines(), Lines { first: 1, last: 4 });
        assert_eq!(roots[0].children.len(), 1);
        assert_eq!(roots[0].children[0].lines(), Lines { first: 2, last: 3 });
        assert_eq!(roots[0].children[0].children[0].lines(), Lines { first: 3, last: 5 });
        assert_eq!(roots[0].drawn(), Lines { first: 1, last: 5 });
    }
}
