//! Finding text in the rows on screen.

use std::ops::Range;

use ratatui::style::{Color, Style};
use ratatui::text::Line;

use crate::layout;

const MATCH: Style = Style::new().fg(Color::Black).bg(Color::Yellow);

/// The byte ranges `query` is found at in `text`, apart from each other. Case matters only when
/// `query` holds a capital.
pub fn ranges(text: &str, query: &str) -> Vec<Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    let case = query.chars().any(char::is_uppercase);
    let same = |a: char, b: char| if case { a == b } else { a.to_lowercase().eq(b.to_lowercase()) };
    let mut found: Vec<Range<usize>> = Vec::new();
    for (start, _) in text.char_indices() {
        if found.last().is_some_and(|last| start < last.end) {
            continue;
        }
        let mut rest = text[start..].char_indices();
        let mut end = start;
        let matched = query.chars().all(|wanted| match rest.next() {
            Some((at, ch)) if same(ch, wanted) => {
                end = start + at + ch.len_utf8();
                true
            }
            _ => false,
        });
        if matched {
            found.push(start..end);
        }
    }
    found
}

/// `line` with `query` marked wherever it is found past the first `chrome` spans.
pub fn highlight(line: Line<'static>, chrome: usize, query: &str) -> Line<'static> {
    let found = ranges(&layout::text(&line, chrome), query);
    layout::mark(line, chrome, &found, MATCH)
}

#[cfg(test)]
mod tests {
    use ratatui::text::Span;

    use super::*;

    #[test]
    fn a_lowercase_query_ignores_case_and_a_capital_makes_it_matter() {
        assert_eq!(ranges("Foo foo fOO", "foo"), [0..3, 4..7, 8..11]);
        assert_eq!(ranges("Foo foo Foo", "Foo"), [0..3, 8..11]);
        assert_eq!(ranges("aaaa", "aa"), [0..2, 2..4]);
        assert_eq!(ranges("äb Äb", "äb"), [0..3, 4..7]);
        assert!(ranges("abc", "").is_empty());
    }

    #[test]
    fn highlighting_splits_spans_at_the_match_and_skips_the_chrome() {
        let line = Line::from(vec![Span::raw("12 "), Span::raw("a1"), Span::raw("2b")]);
        let lit = highlight(line, 1, "12");
        let pieces: Vec<(&str, bool)> =
            lit.spans.iter().map(|span| (&*span.content, span.style == MATCH)).collect();
        assert_eq!(pieces, [("12 ", false), ("a", false), ("1", true), ("2", true), ("b", false)]);
    }
}
