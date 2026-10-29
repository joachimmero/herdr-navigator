//! Markdown to styled terminal lines, wrapped to a width.

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const BULLETS: [&str; 3] = ["•", "◦", "▪"];
const TAB: &str = "    ";

fn heading_style(level: HeadingLevel) -> Style {
    let bold = Style::new().add_modifier(Modifier::BOLD);
    match level {
        HeadingLevel::H1 => bold.fg(Color::Magenta),
        HeadingLevel::H2 => bold.fg(Color::Cyan),
        HeadingLevel::H3 => bold.fg(Color::Blue),
        _ => bold,
    }
}

fn code_style() -> Style {
    Style::new().fg(Color::Yellow)
}

fn link_style() -> Style {
    Style::new().fg(Color::Blue).add_modifier(Modifier::UNDERLINED)
}

fn dim() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}

/// `source` laid out for a terminal `width` columns wide.
pub fn render(source: &str, width: u16) -> Vec<Line<'static>> {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS;
    let mut renderer = Renderer::new(usize::from(width).max(1));
    Parser::new_ext(source, options).for_each(|event| renderer.on(event));
    renderer.flush_implicit();
    renderer.lines
}

/// A block whose lines all start with a prefix: a quote's bar, or a list item's bullet on its
/// first line and as much blank space on the rest.
struct Container {
    first: Vec<Span<'static>>,
    rest: Vec<Span<'static>>,
    is_item: bool,
    first_pending: bool,
}

impl Container {
    fn quote() -> Self {
        let bar = vec![Span::styled("│ ", Style::new().fg(Color::Green))];
        Self { first: bar.clone(), rest: bar, is_item: false, first_pending: true }
    }

    fn item(marker: String, style: Style) -> Self {
        let indent = " ".repeat(marker.width());
        let first = vec![Span::styled(marker, style)];
        Self { first, rest: vec![Span::raw(indent)], is_item: true, first_pending: true }
    }

    fn width(&self) -> usize {
        self.rest.iter().map(Span::width).sum()
    }
}

struct List {
    /// The next item's number, for an ordered list.
    next: Option<u64>,
    loose: bool,
    items: usize,
}

struct Table {
    alignments: Vec<Alignment>,
    rows: Vec<Vec<Vec<Span<'static>>>>,
}

struct Code {
    language: Option<String>,
    text: String,
}

struct Renderer {
    width: usize,
    lines: Vec<Line<'static>>,
    containers: Vec<Container>,
    lists: Vec<List>,
    inline: Vec<Span<'static>>,
    styles: Vec<Style>,
    /// Each open link's destination, and where its text starts in `inline`.
    links: Vec<(String, usize)>,
    code: Option<Code>,
    html: Option<String>,
    table: Option<Table>,
    /// Whether the next block is set apart from the last by a blank line.
    gap: bool,
}

impl Renderer {
    fn new(width: usize) -> Self {
        Self {
            width,
            lines: Vec::new(),
            containers: Vec::new(),
            lists: Vec::new(),
            inline: Vec::new(),
            styles: Vec::new(),
            links: Vec::new(),
            code: None,
            html: None,
            table: None,
            gap: false,
        }
    }

    fn on(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(&text),
            Event::Code(text) => self.push(&text, self.style().patch(code_style())),
            Event::InlineHtml(html) => self.push(&html, dim()),
            Event::Html(html) => self.html.get_or_insert_default().push_str(&html),
            Event::SoftBreak => self.push(" ", self.style()),
            Event::HardBreak => self.push("\n", self.style()),
            Event::FootnoteReference(label) => self.push(&format!("[{label}]"), dim()),
            Event::TaskListMarker(done) => self.mark_task(done),
            Event::Rule => {
                self.flush_implicit();
                self.start_block();
                let rule = "─".repeat(self.content_width());
                self.emit(vec![Span::styled(rule, dim())]);
                self.gap = true;
            }
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => {
                self.flush_implicit();
                if self.containers.last().is_some_and(|container| container.is_item)
                    && let Some(list) = self.lists.last_mut()
                {
                    list.loose = true;
                }
            }
            Tag::Heading { .. } | Tag::HtmlBlock => self.flush_implicit(),
            Tag::BlockQuote(_) => {
                self.flush_implicit();
                self.containers.push(Container::quote());
            }
            Tag::CodeBlock(kind) => {
                self.flush_implicit();
                let language = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().map(str::to_owned)
                    }
                    CodeBlockKind::Indented => None,
                };
                self.code = Some(Code { language, text: String::new() });
            }
            Tag::List(start) => {
                self.flush_implicit();
                self.lists.push(List { next: start, loose: false, items: 0 });
            }
            Tag::Item => {
                self.flush_implicit();
                let depth = self.lists.len().saturating_sub(1);
                let Some(list) = self.lists.last_mut() else { return };
                if list.loose && list.items > 0 {
                    self.gap = true;
                }
                list.items += 1;
                let marker = match list.next.as_mut() {
                    Some(number) => {
                        *number += 1;
                        format!("{}. ", *number - 1)
                    }
                    None => format!("{} ", BULLETS[depth % BULLETS.len()]),
                };
                self.containers.push(Container::item(marker, Style::new().fg(Color::Cyan)));
            }
            Tag::FootnoteDefinition(label) => {
                self.flush_implicit();
                self.containers.push(Container::item(format!("[{label}] "), dim()));
            }
            Tag::Table(alignments) => {
                self.flush_implicit();
                self.table = Some(Table { alignments, rows: Vec::new() });
            }
            Tag::TableHead | Tag::TableRow => {
                if let Some(table) = self.table.as_mut() {
                    table.rows.push(Vec::new());
                }
            }
            Tag::Emphasis => self.styles.push(Style::new().add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.styles.push(Style::new().add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => {
                self.styles.push(Style::new().add_modifier(Modifier::CROSSED_OUT));
            }
            Tag::Link { dest_url, .. } => {
                self.links.push((dest_url.into_string(), self.inline.len()));
                self.styles.push(link_style());
            }
            Tag::Image { .. } => {
                self.push("[image: ", dim());
                self.styles.push(dim().add_modifier(Modifier::ITALIC));
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush_inline(Style::new());
                self.gap = true;
            }
            TagEnd::Heading(level) => self.flush_heading(level),
            TagEnd::BlockQuote(_) | TagEnd::FootnoteDefinition => {
                self.flush_implicit();
                self.containers.pop();
                self.gap = true;
            }
            TagEnd::CodeBlock => {
                if let Some(code) = self.code.take() {
                    self.flush_code(code);
                }
            }
            TagEnd::HtmlBlock => {
                if let Some(html) = self.html.take() {
                    self.flush_html(&html);
                }
            }
            TagEnd::List(_) => {
                self.flush_implicit();
                self.lists.pop();
                // A list nested in a tight item runs straight on into the next item.
                self.gap = !self.containers.last().is_some_and(|container| container.is_item);
            }
            TagEnd::Item => {
                self.flush_implicit();
                self.containers.pop();
            }
            TagEnd::TableCell => {
                let cell = std::mem::take(&mut self.inline);
                if let Some(row) = self.table.as_mut().and_then(|table| table.rows.last_mut()) {
                    row.push(cell);
                }
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    self.flush_table(table);
                }
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.styles.pop();
            }
            TagEnd::Link => {
                self.styles.pop();
                let Some((destination, start)) = self.links.pop() else { return };
                let text: String =
                    self.inline[start..].iter().map(|span| span.content.as_ref()).collect();
                if !destination.is_empty() && text != destination {
                    self.push(&format!(" ({destination})"), dim());
                }
            }
            TagEnd::Image => {
                self.styles.pop();
                self.push("]", dim());
            }
            _ => {}
        }
    }

    fn text(&mut self, text: &str) {
        match self.code.as_mut() {
            Some(code) => code.text.push_str(text),
            None => self.push(text, self.style()),
        }
    }

    fn push(&mut self, text: &str, style: Style) {
        self.inline.push(Span::styled(text.to_owned(), style));
    }

    fn style(&self) -> Style {
        self.styles.iter().fold(Style::new(), |style, layer| style.patch(*layer))
    }

    /// A task item shows its box where its bullet would be.
    fn mark_task(&mut self, done: bool) {
        let Some(item) = self.containers.iter_mut().rev().find(|container| container.is_item)
        else {
            return;
        };
        let (mark, style) =
            if done { ("☑", Style::new().fg(Color::Green)) } else { ("☐", Style::new()) };
        item.first = vec![Span::styled(format!("{mark:<width$}", width = item.width()), style)];
    }

    fn content_width(&self) -> usize {
        let prefix: usize = self.containers.iter().map(Container::width).sum();
        self.width.saturating_sub(prefix).max(1)
    }

    /// Set the next block apart from the last one when it should be.
    fn start_block(&mut self) {
        if self.gap && !self.lines.is_empty() {
            // A container opened since the last block holds none of the gap.
            let mut prefix: Vec<Span<'static>> = self
                .containers
                .iter()
                .filter(|container| !container.first_pending)
                .flat_map(|container| container.rest.iter().cloned())
                .collect();
            while prefix.last().is_some_and(|span| span.content.trim().is_empty()) {
                prefix.pop();
            }
            self.lines.push(Line::from(prefix));
        }
        self.gap = false;
    }

    /// Add `content` as a line under the open containers' prefixes, giving each item its bullet
    /// on the first line it holds.
    fn emit(&mut self, content: Vec<Span<'static>>) {
        let prefix: Vec<Span<'static>> = self
            .containers
            .iter()
            .flat_map(|container| {
                if container.first_pending { &container.first } else { &container.rest }
                    .iter()
                    .cloned()
            })
            .collect();
        self.containers.iter_mut().for_each(|container| container.first_pending = false);
        self.lines.push(Line::from([prefix, content].concat()));
    }

    /// Text a tight list item holds without a paragraph around it, which unlike a paragraph
    /// leaves no gap after itself.
    fn flush_implicit(&mut self) {
        self.flush_inline(Style::new());
    }

    fn flush_inline(&mut self, base: Style) {
        let spans: Vec<Span<'static>> = std::mem::take(&mut self.inline)
            .into_iter()
            .map(|span| {
                let style = base.patch(span.style);
                span.style(style)
            })
            .collect();
        if spans.iter().all(|span| span.content.trim().is_empty()) {
            return;
        }
        self.start_block();
        wrap(&spans, self.content_width()).into_iter().for_each(|line| self.emit(line));
    }

    fn flush_heading(&mut self, level: HeadingLevel) {
        let style = heading_style(level);
        let wrapped = wrap(&std::mem::take(&mut self.inline), self.content_width());
        let underline = match level {
            HeadingLevel::H1 => Some("━"),
            HeadingLevel::H2 => Some("─"),
            _ => None,
        };
        let widest = wrapped
            .iter()
            .map(|line| line.iter().map(Span::width).sum::<usize>())
            .max()
            .unwrap_or(0);

        self.start_block();
        for line in wrapped {
            self.emit(line.into_iter().map(|span| span.patch_style(style)).collect());
        }
        if let Some(underline) = underline {
            self.emit(vec![Span::styled(underline.repeat(widest), style)]);
        }
        self.gap = true;
    }

    fn flush_code(&mut self, code: Code) {
        let gutter = Span::styled("▎ ", dim());
        let width = self.content_width().saturating_sub(gutter.width()).max(1);
        let lines: Vec<String> = code
            .text
            .trim_end_matches('\n')
            .split('\n')
            .flat_map(|line| chunk(&line.replace('\t', TAB), width))
            .collect();

        self.start_block();
        if let Some(language) = code.language {
            self.emit(vec![
                gutter.clone(),
                Span::styled(language, dim().add_modifier(Modifier::ITALIC)),
            ]);
        }
        for line in lines {
            self.emit(vec![gutter.clone(), Span::styled(line, code_style())]);
        }
        self.gap = true;
    }

    /// Raw HTML shows as dim source, except comments, which are meant to be invisible.
    fn flush_html(&mut self, html: &str) {
        if html.trim_start().starts_with("<!--") {
            return;
        }
        let width = self.content_width();
        let lines: Vec<String> =
            html.trim_end_matches('\n').split('\n').flat_map(|line| chunk(line, width)).collect();
        self.start_block();
        lines.into_iter().for_each(|line| self.emit(vec![Span::styled(line, dim())]));
        self.gap = true;
    }

    fn flush_table(&mut self, table: Table) {
        let columns = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        if columns == 0 {
            return;
        }
        let natural: Vec<usize> = (0..columns)
            .map(|column| {
                table
                    .rows
                    .iter()
                    .filter_map(|row| row.get(column))
                    .map(|cell| cell.iter().map(Span::width).sum::<usize>())
                    .max()
                    .unwrap_or(0)
                    .max(1)
            })
            .collect();
        // "│ a │ b │" spends a bar before each column and a space either side of it.
        let budget = self.content_width().saturating_sub(1 + 3 * columns).max(columns);
        let widths = fit_columns(&natural, budget);
        let border = |left: &str, join: &str, right: &str| {
            let segments: Vec<String> = widths.iter().map(|width| "─".repeat(width + 2)).collect();
            vec![Span::styled(format!("{left}{}{right}", segments.join(join)), dim())]
        };
        let mut lines = vec![border("┌", "┬", "┐")];
        for (index, row) in table.rows.iter().enumerate() {
            let header = index == 0;
            lines.extend(table_row(row, &widths, &table.alignments, header));
            if header && table.rows.len() > 1 {
                lines.push(border("├", "┼", "┤"));
            }
        }
        lines.push(border("└", "┴", "┘"));

        self.start_block();
        lines.into_iter().for_each(|line| self.emit(line));
        self.gap = true;
    }
}

/// A table row's lines, each cell wrapped to its column and padded to its alignment.
fn table_row(
    row: &[Vec<Span<'static>>],
    widths: &[usize],
    alignments: &[Alignment],
    header: bool,
) -> Vec<Vec<Span<'static>>> {
    let bold = Style::new().add_modifier(Modifier::BOLD);
    let cells: Vec<Vec<Vec<Span<'static>>>> = widths
        .iter()
        .enumerate()
        .map(|(column, &width)| {
            let cell = row.get(column).map(Vec::as_slice).unwrap_or_default();
            let cell: Vec<Span<'static>> = if header {
                cell.iter().map(|span| span.clone().patch_style(bold)).collect()
            } else {
                cell.to_vec()
            };
            wrap(&cell, width)
        })
        .collect();
    let height = cells.iter().map(Vec::len).max().unwrap_or(0).max(1);
    let bar = || Span::styled("│", dim());

    (0..height)
        .map(|line| {
            let mut spans = vec![bar()];
            for (column, cell) in cells.iter().enumerate() {
                let content = cell.get(line).cloned().unwrap_or_default();
                let room = widths[column].saturating_sub(content.iter().map(Span::width).sum());
                let (before, after) = match alignments.get(column) {
                    Some(Alignment::Right) => (room, 0),
                    Some(Alignment::Center) => (room / 2, room - room / 2),
                    _ => (0, room),
                };
                spans.push(Span::raw(" ".repeat(before + 1)));
                spans.extend(content);
                spans.push(Span::raw(" ".repeat(after + 1)));
                spans.push(bar());
            }
            spans
        })
        .collect()
}

/// Column widths summing to at most `budget`: every column that fits a fair share keeps its
/// natural width, and the rest split what is left evenly.
fn fit_columns(natural: &[usize], budget: usize) -> Vec<usize> {
    if natural.iter().sum::<usize>() <= budget {
        return natural.to_vec();
    }
    let mut widths = vec![0; natural.len()];
    let mut open: Vec<usize> = (0..natural.len()).collect();
    let mut left = budget;
    while !open.is_empty() {
        let share = left / open.len();
        let (fits, wide): (Vec<usize>, Vec<usize>) =
            open.iter().partition(|&&column| natural[column] <= share);
        if fits.is_empty() {
            let spare = left - share * wide.len();
            for (index, &column) in wide.iter().enumerate() {
                widths[column] = share + usize::from(index < spare);
            }
            break;
        }
        for column in fits {
            widths[column] = natural[column];
            left -= natural[column];
        }
        open = wide;
    }
    widths.into_iter().map(|width| width.max(1)).collect()
}

/// `text` cut into pieces at most `width` columns wide, keeping an empty line as one piece.
fn chunk(text: &str, width: usize) -> Vec<String> {
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

/// `spans` word-wrapped into lines at most `width` columns wide, breaking at `\n` and cutting
/// any word wider than a line.
fn wrap(spans: &[Span<'static>], width: usize) -> Vec<Vec<Span<'static>>> {
    let width = width.max(1);
    let chars: Vec<(char, Style)> = spans
        .iter()
        .flat_map(|span| span.content.chars().map(move |ch| (ch, span.style)))
        .collect();
    chars
        .split(|&(ch, _)| ch == '\n')
        .flat_map(|segment| wrap_segment(segment, width))
        .map(|line| group(&line))
        .collect()
}

fn wrap_segment(segment: &[(char, Style)], width: usize) -> Vec<Vec<(char, Style)>> {
    let mut lines = Vec::new();
    let mut line: Vec<(char, Style)> = Vec::new();
    let mut used = 0;
    let mut space: Vec<(char, Style)> = Vec::new();
    for word in segment.chunk_by(|a, b| a.0.is_whitespace() == b.0.is_whitespace()) {
        if word[0].0.is_whitespace() {
            if !line.is_empty() {
                space = word.to_vec();
            }
            continue;
        }
        let word_width: usize = word.iter().map(|(ch, _)| ch.width().unwrap_or(0)).sum();
        let space_width: usize = space.iter().map(|(ch, _)| ch.width().unwrap_or(0)).sum();
        if used + space_width + word_width <= width {
            line.append(&mut space);
            line.extend_from_slice(word);
            used += space_width + word_width;
            continue;
        }
        space.clear();
        if !line.is_empty() {
            lines.push(std::mem::take(&mut line));
            used = 0;
        }
        for &(ch, style) in word {
            let ch_width = ch.width().unwrap_or(0);
            if used + ch_width > width && used > 0 {
                lines.push(std::mem::take(&mut line));
                used = 0;
            }
            line.push((ch, style));
            used += ch_width;
        }
    }
    lines.push(line);
    lines
}

/// Runs of same-styled characters as spans.
fn group(line: &[(char, Style)]) -> Vec<Span<'static>> {
    line.chunk_by(|a, b| a.1 == b.1)
        .map(|run| Span::styled(run.iter().map(|(ch, _)| ch).collect::<String>(), run[0].1))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(source: &str, width: u16) -> Vec<String> {
        render(source, width)
            .iter()
            .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn paragraphs_wrap_at_word_boundaries_and_are_set_apart() {
        assert_eq!(text("one two three four\n\nfive", 9), ["one two", "three", "four", "", "five"]);
    }

    #[test]
    fn a_word_wider_than_the_line_is_cut() {
        assert_eq!(text("abcdefgh", 3), ["abc", "def", "gh"]);
    }

    #[test]
    fn a_hard_break_starts_a_new_line() {
        assert_eq!(text("one  \ntwo", 20), ["one", "two"]);
    }

    #[test]
    fn top_headings_are_underlined_to_their_width() {
        assert_eq!(
            text("# Title\n\n## Sub\n\n### Low", 20),
            ["Title", "━━━━━", "", "Sub", "───", "", "Low"]
        );
    }

    #[test]
    fn tight_lists_nest_without_blank_lines() {
        assert_eq!(text("- a\n  - b\n- c", 20), ["• a", "  ◦ b", "• c"]);
    }

    #[test]
    fn a_list_after_a_heading_stays_tight() {
        assert_eq!(
            text("# H\n\n- a\n  - b\n- c\n\n1. d\n2. e", 20),
            ["H", "━", "", "• a", "  ◦ b", "• c", "", "1. d", "2. e"]
        );
    }

    #[test]
    fn a_quote_starts_without_a_bar_above_it() {
        assert_eq!(text("a\n\n> b", 20), ["a", "", "│ b"]);
    }

    #[test]
    fn loose_list_items_are_set_apart() {
        assert_eq!(text("- a\n\n- b", 20), ["• a", "", "• b"]);
    }

    #[test]
    fn ordered_lists_count_from_their_start() {
        assert_eq!(text("3. a\n4. b", 20), ["3. a", "4. b"]);
    }

    #[test]
    fn a_wrapped_item_hangs_under_its_text() {
        assert_eq!(text("- one two three", 9), ["• one two", "  three"]);
    }

    #[test]
    fn task_items_show_their_box() {
        assert_eq!(text("- [ ] todo\n- [x] done", 20), ["☐ todo", "☑ done"]);
    }

    #[test]
    fn a_quote_bars_every_line_it_holds() {
        assert_eq!(text("> one two\n>\n> three", 7), ["│ one", "│ two", "│ ", "│ three"]);
    }

    #[test]
    fn code_blocks_keep_their_lines_and_show_their_language() {
        assert_eq!(
            text("```rust\nfn main() {\n\tx\n}\n```", 30),
            ["▎ rust", "▎ fn main() {", "▎     x", "▎ }"]
        );
    }

    #[test]
    fn long_code_lines_are_cut_rather_than_hidden() {
        assert_eq!(text("```\nabcdef\n```", 5), ["▎ abc", "▎ def"]);
    }

    #[test]
    fn a_rule_spans_the_width() {
        assert_eq!(text("a\n\n---\n\nb", 4), ["a", "", "────", "", "b"]);
    }

    #[test]
    fn links_show_their_target_unless_it_is_the_text() {
        assert_eq!(text("[docs](a.md) <https://x.io>", 40), ["docs (a.md) https://x.io"]);
    }

    #[test]
    fn html_comments_are_hidden() {
        assert_eq!(text("<!-- note -->\n\ntext", 20), ["text"]);
    }

    #[test]
    fn footnotes_show_their_label() {
        assert_eq!(text("a[^1]\n\n[^1]: note", 20), ["a[1]", "", "[1] note"]);
    }

    #[test]
    fn a_table_that_fits_keeps_its_natural_widths() {
        assert_eq!(
            text("| a | bb |\n|---|---:|\n| ccc | d |", 40),
            ["┌─────┬────┐", "│ a   │ bb │", "├─────┼────┤", "│ ccc │  d │", "└─────┴────┘"]
        );
    }

    #[test]
    fn a_wide_table_wraps_its_cells_to_fit() {
        let lines = text("| k | value |\n|---|---|\n| x | one two three |", 16);
        assert_eq!(
            lines,
            [
                "┌───┬──────────┐",
                "│ k │ value    │",
                "├───┼──────────┤",
                "│ x │ one two  │",
                "│   │ three    │",
                "└───┴──────────┘"
            ]
        );
    }

    #[test]
    fn narrow_columns_keep_their_width_while_wide_ones_share_the_rest() {
        assert_eq!(fit_columns(&[2, 20, 30], 22), [2, 10, 10]);
        assert_eq!(fit_columns(&[2, 3], 10), [2, 3]);
    }
}
