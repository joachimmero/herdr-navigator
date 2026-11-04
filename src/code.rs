//! Source text laid out for the pane: a line-number gutter, and lines wrapped hard to the width.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::comment;
use crate::layout::{self, Lines, Row};

const TAB: &str = "    ";

/// `lines`, already styled, laid out for `width` columns. `depths[i]` is how many comment boxes
/// enclose source line `i + 1`; missing entries are 0.
pub fn layout(lines: &[Vec<Span<'static>>], width: u16, depths: &[usize]) -> Vec<Row> {
    let digits = lines.len().max(1).to_string().len();
    let gutter = digits + 1;
    let width = usize::from(width);
    lines
        .iter()
        .enumerate()
        .flat_map(|(index, spans)| {
            let depth = depths.get(index).copied().unwrap_or(0);
            let (left_fill, right_fill) = comment::fills(comment::Sides::Gutter(gutter), depth);
            let text_width = if depth == 0 {
                width.saturating_sub(gutter).max(1)
            } else {
                width.saturating_sub(left_fill + right_fill).max(1)
            };
            let expanded: Vec<Span<'static>> = spans
                .iter()
                .map(|span| Span::styled(span.content.replace('\t', TAB), span.style))
                .collect();
            layout::wrap_spans(&expanded, text_width).into_iter().enumerate().map(
                move |(row, spans)| {
                    let source = Some(Lines::one(index + 1));
                    if depth == 0 {
                        let number = if row == 0 {
                            format!("{:>digits$} ", index + 1)
                        } else {
                            " ".repeat(gutter)
                        };
                        let number = Span::styled(number, Style::new().add_modifier(Modifier::DIM));
                        return Row {
                            line: Line::from([vec![number], spans].concat()),
                            source,
                            chrome: 1,
                            trail: 0,
                        };
                    }
                    comment::box_row(spans, depth, left_fill, text_width, source)
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(rows: &[Row]) -> Vec<String> {
        rows.iter().map(|row| row.line.to_string()).collect()
    }

    #[test]
    fn long_lines_wrap_under_a_blank_gutter() {
        let lines = [vec![Span::raw("abcdef")], vec![], vec![Span::raw("x")]];
        assert_eq!(text(&layout(&lines, 5, &[])), ["1 abc", "  def", "2 ", "3 x"]);
    }

    #[test]
    fn the_gutter_is_as_wide_as_the_last_line_number() {
        let lines = vec![vec![Span::raw("a")]; 10];
        assert_eq!(text(&layout(&lines, 20, &[]))[0], " 1 a");
    }

    #[test]
    fn tabs_expand_and_a_split_keeps_each_part_styled() {
        let bold = Style::new().add_modifier(Modifier::BOLD);
        let lines = [vec![Span::raw("\ta"), Span::styled("bcd", bold)]];
        let laid = layout(&lines, 6, &[]);
        assert_eq!(text(&laid), ["1     ", "  abcd"]);
        assert_eq!(laid[1].line.spans.last().unwrap().style, bold);
        assert!(laid.iter().all(|row| row.source == Some(Lines::one(1))));
    }

    #[test]
    fn a_commented_line_replaces_the_gutter_with_a_wall_and_wraps_inside() {
        let lines = [vec![Span::raw("abcdef")], vec![Span::raw("x")]];
        let laid = layout(&lines, 8, &[1, 0]);
        assert_eq!(text(&laid), ["│ abcd │", "│ ef   │", "2 x"]);
    }
}
