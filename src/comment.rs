//! Comments on the project's files, kept for the session, and the text they're sent as.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

use crate::layout::{Lines, chunk};

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

/// A comment as a card `width` columns wide: its lines, or `image` for a whole-file comment, in
/// the top border, and its text inside.
pub fn card(comment: &Comment, width: u16) -> Vec<Line<'static>> {
    let width = usize::from(width).max(5);
    let border = Style::new().fg(Color::Yellow);
    let mut title = match comment.anchor.lines() {
        Some(Lines { first, last }) if first == last => format!("L{first}"),
        Some(Lines { first, last }) => format!("L{first}–{last}"),
        None => "image".to_owned(),
    };
    if comment.outdated {
        title.push_str(" · outdated");
    }
    let title_width = title.chars().count().min(width - 4);
    let title: String = title.chars().take(title_width).collect();
    let top = vec![
        Span::styled("╭ ", border),
        Span::styled(title, border.add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {}╮", "─".repeat(width - 4 - title_width)), border),
    ];
    let inner = width - 4;
    let body = comment.text.split('\n').flat_map(|line| chunk(line, inner)).map(|line| {
        let pad = inner.saturating_sub(line.chars().filter_map(UnicodeWidthChar::width).sum());
        Line::from(vec![
            Span::styled("│ ", border),
            Span::raw(format!("{line}{}", " ".repeat(pad))),
            Span::styled(" │", border),
        ])
    });
    let bottom = Line::styled(format!("╰{}╯", "─".repeat(width - 2)), border);
    std::iter::once(Line::from(top)).chain(body).chain([bottom]).collect()
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
}
