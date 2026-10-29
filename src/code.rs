//! Source text laid out for the pane: a line-number gutter, and lines wrapped hard to the width.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

const TAB: &str = "    ";

/// `lines`, already styled, laid out for `width` columns.
pub fn layout(lines: &[Vec<Span<'static>>], width: u16) -> Vec<Line<'static>> {
    let digits = lines.len().max(1).to_string().len();
    let gutter = digits + 1;
    let text_width = usize::from(width).saturating_sub(gutter).max(1);
    lines
        .iter()
        .enumerate()
        .flat_map(|(index, spans)| {
            wrap(spans, text_width).into_iter().enumerate().map(move |(row, spans)| {
                let number =
                    if row == 0 { format!("{:>digits$} ", index + 1) } else { " ".repeat(gutter) };
                let number = Span::styled(number, Style::new().add_modifier(Modifier::DIM));
                Line::from([vec![number], spans].concat())
            })
        })
        .collect()
}

/// `spans` cut into rows of at most `width` columns, tabs expanded; an empty line is one row.
fn wrap(spans: &[Span<'static>], width: usize) -> Vec<Vec<Span<'static>>> {
    let mut rows = vec![Vec::new()];
    let mut used = 0;
    for span in spans {
        let mut text = String::new();
        for ch in span.content.replace('\t', TAB).chars() {
            let ch_width = ch.width().unwrap_or(0);
            if used + ch_width > width && used > 0 {
                rows.last_mut().expect("rows start non-empty").push(Span::styled(text, span.style));
                rows.push(Vec::new());
                text = String::new();
                used = 0;
            }
            text.push(ch);
            used += ch_width;
        }
        if !text.is_empty() {
            rows.last_mut().expect("rows start non-empty").push(Span::styled(text, span.style));
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lines: &[Line]) -> Vec<String> {
        lines.iter().map(Line::to_string).collect()
    }

    #[test]
    fn long_lines_wrap_under_a_blank_gutter() {
        let lines = [vec![Span::raw("abcdef")], vec![], vec![Span::raw("x")]];
        assert_eq!(text(&layout(&lines, 5)), ["1 abc", "  def", "2 ", "3 x"]);
    }

    #[test]
    fn the_gutter_is_as_wide_as_the_last_line_number() {
        let lines = vec![vec![Span::raw("a")]; 10];
        assert_eq!(text(&layout(&lines, 20))[0], " 1 a");
    }

    #[test]
    fn tabs_expand_and_a_split_keeps_each_part_styled() {
        let bold = Style::new().add_modifier(Modifier::BOLD);
        let lines = [vec![Span::raw("\ta"), Span::styled("bcd", bold)]];
        let laid = layout(&lines, 6);
        assert_eq!(text(&laid), ["1     ", "  abcd"]);
        assert_eq!(laid[1].spans.last().unwrap().style, bold);
    }
}
