//! A file laid out for the pane: rows on screen, each knowing the source lines it shows.

use ratatui::text::Line;

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
}

/// One row on screen.
#[derive(Clone, Debug)]
pub struct Row {
    pub line: Line<'static>,
    /// `None` for a row no source line feeds, like the gap between two blocks.
    pub source: Option<Lines>,
}
