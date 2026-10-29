//! A file laid out for the pane: rows on screen, each knowing the source lines it shows.

use ratatui::text::Line;
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
