//! A file laid out for the pane: rows on screen, each knowing the source lines it shows.

use std::ops::Range;

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

/// A run of 1-based source lines, both ends inclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lines {
    pub first: usize,
    pub last: usize,
}

impl Lines {
    pub fn one(line: usize) -> Self {
        Self { first: line, last: line }
    }

    /// The smallest run covering both.
    pub fn union(self, other: Self) -> Self {
        Self { first: self.first.min(other.first), last: self.last.max(other.last) }
    }

    pub fn contains(self, other: Self) -> bool {
        self.first <= other.first && other.last <= self.last
    }

    pub fn overlaps(self, other: Self) -> bool {
        self.first <= other.last && other.first <= self.last
    }
}

/// One row on screen.
#[derive(Clone, Debug)]
pub struct Row {
    pub line: Line<'static>,
    /// `None` for a row no source line feeds, like the gap between two blocks.
    pub source: Option<Lines>,
    /// How many of the line's spans lead it without being the file's text, like a line number
    /// or a bullet.
    pub chrome: usize,
}

/// The text of `line` past its first `chrome` spans.
pub fn text(line: &Line, chrome: usize) -> String {
    line.spans.iter().skip(chrome).map(|span| &*span.content).collect()
}

/// `line` with `style` patched onto the byte ranges `marked` of its text past the first `chrome`
/// spans, splitting spans where a range starts or ends inside one.
pub fn mark(
    line: Line<'static>,
    chrome: usize,
    marked: &[Range<usize>],
    style: Style,
) -> Line<'static> {
    if marked.is_empty() {
        return line;
    }
    let mut end = 0;
    let spans = line
        .spans
        .into_iter()
        .enumerate()
        .flat_map(|(index, span)| {
            if index < chrome {
                return vec![span];
            }
            let start = end;
            end += span.content.len();
            let inside = marked.iter().flat_map(|range| [range.start, range.end]);
            let mut cuts: Vec<usize> = inside.filter(|&cut| start < cut && cut < end).collect();
            cuts.extend([start, end]);
            cuts.sort_unstable();
            cuts.windows(2)
                .map(|piece| {
                    let text = span.content[piece[0] - start..piece[1] - start].to_owned();
                    let hit = marked.iter().any(|range| range.contains(&piece[0]));
                    Span::styled(text, if hit { span.style.patch(style) } else { span.style })
                })
                .collect()
        })
        .collect();
    Line { spans, ..line }
}

/// `text` cut into pieces at most `width` columns wide, keeping an empty line as one piece.
pub fn chunk(text: &str, width: usize) -> Vec<String> {
    let mut pieces = vec![String::new()];
    let mut used = 0;
    for ch in text.chars() {
        let ch_width = ch.width().unwrap_or(0);
        if used + ch_width > width && used > 0 {
            pieces.push(String::new());
            used = 0;
        }
        pieces.last_mut().expect("pieces starts non-empty").push(ch);
        used += ch_width;
    }
    pieces
}
