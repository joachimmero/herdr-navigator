//! Drawing the pane.

use std::path::Path;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};

use crate::app::{App, Focus, Mode};
use crate::comment::Comments;
use crate::composer::{Composer, Target};
use crate::doc::Doc;
use crate::layout::Lines;
use crate::tree::{Row, Tree};

/// The columns left blank on either side of the document.
const MARGIN: u16 = 1;
/// Below this width only the focused side shows.
const NARROW: u16 = 70;

/// Where each part of the pane goes: the title bar, and the tree and file where they show.
pub struct Panes {
    pub title: Rect,
    pub tree: Option<Rect>,
    pub file: Option<Rect>,
}

impl Panes {
    /// The lines a page of the file takes.
    pub fn page(&self) -> usize {
        self.file.map_or(1, |file| usize::from(file.height).max(1))
    }
}

pub fn panes(area: Rect, app: &App) -> Panes {
    let [title, body] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
    let (tree, file) = match (&app.doc, app.focus) {
        (None, _) => (Some(body), None),
        _ if body.width < NARROW => match app.focus {
            Focus::Tree => (Some(body), None),
            Focus::File => (None, Some(inset(body))),
        },
        _ => {
            let width = (body.width / 3).clamp(24, 40);
            let [tree, _rule, file] = Layout::horizontal([
                Constraint::Length(width),
                Constraint::Length(1),
                Constraint::Fill(1),
            ])
            .areas(body);
            (Some(tree), Some(inset(file)))
        }
    };
    Panes { title, tree, file }
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let panes = panes(frame.area(), app);
    if let (Some(tree), Some(_)) = (panes.tree, panes.file) {
        let rule = Rect { x: tree.right(), width: 1, ..tree };
        let bar = vec![Line::styled("│", dim()); usize::from(rule.height)];
        frame.render_widget(Paragraph::new(bar), rule);
    }
    if let Some(area) = panes.tree {
        draw_tree(frame, &app.tree, area, app.focus == Focus::Tree);
    }
    let mut status = Vec::new();
    match (&app.mode, panes.file.or(panes.tree)) {
        (Mode::List { selected }, Some(area)) => draw_list(frame, app, *selected, area),
        _ => {
            if let (Some(area), Some(doc)) = (panes.file, &mut app.doc) {
                let composer = match &app.mode {
                    Mode::Composing(composer) => Some(composer),
                    _ => None,
                };
                let focused = app.focus == Focus::File;
                status = draw_doc(frame, doc, &app.comments, composer, area, focused);
            }
        }
    }
    if app.comments.len() > 0 {
        let count = app.comments.len();
        let noun = if count == 1 { "comment" } else { "comments" };
        status.insert(0, Span::styled(format!("{count} {noun} "), Style::new().fg(Color::Yellow)));
    }
    if app.tree.is_scanning() {
        status.insert(0, Span::styled("scanning… ", dim()));
    }
    let mut left = vec![Span::styled(display_path(&app.tree.root), bold())];
    if let Some(doc) = &app.doc {
        let shown = doc.path.strip_prefix(&app.tree.root).unwrap_or(&doc.path);
        left.extend([Span::styled(" › ", dim()), Span::raw(shown.display().to_string())]);
    }
    draw_title(frame, panes.title, left, status);
}

fn draw_tree(frame: &mut Frame, tree: &Tree, area: Rect, focused: bool) {
    let rows = tree.rows();
    if rows.is_empty() && !tree.is_scanning() {
        frame.render_widget(Paragraph::new(Line::styled(" No files here.", dim())), area);
        return;
    }
    let height = usize::from(area.height).max(1);
    let selected = tree.selected_index();
    let offset = tree.offset.get().min(selected).max((selected + 1).saturating_sub(height));
    tree.offset.set(offset);
    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .skip(offset)
        .take(height)
        .map(|(index, row)| tree_row(tree, row, index == selected, focused))
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn tree_row(tree: &Tree, row: &Row, selected: bool, focused: bool) -> Line<'static> {
    let node = row.node;
    let marker = match (node.dir, tree.is_expanded(&node.path)) {
        (true, true) => "▾ ",
        (true, false) => "▸ ",
        (false, _) => "  ",
    };
    let mut style = if node.dir { Style::new().fg(Color::Blue) } else { Style::new() };
    if node.ignored {
        style = style.add_modifier(Modifier::DIM);
    }
    let line = Line::from(vec![
        Span::raw(format!(" {}", "  ".repeat(row.depth))),
        Span::styled(marker, dim()),
        Span::styled(node.name.clone(), style),
    ]);
    match (selected, focused) {
        (true, true) => line.style(Style::new().add_modifier(Modifier::REVERSED)),
        (true, false) => line.style(Style::new().add_modifier(Modifier::UNDERLINED)),
        (false, _) => line,
    }
}

/// Draw the file, its cursor and selection marked in the margin and the comment box under the
/// selection while there is one, and return its part of the title bar's status.
fn draw_doc(
    frame: &mut Frame,
    doc: &mut Doc,
    comments: &Comments,
    composer: Option<&Composer>,
    area: Rect,
    focused: bool,
) -> Vec<Span<'static>> {
    let height = usize::from(area.height);
    let box_height = composer.map_or(0, |composer| composer_height(composer, area));
    let lines = doc.page(area.width, height, comments, usize::from(box_height));
    let shown = lines.len();
    frame.render_widget(Paragraph::new(lines), area);
    if let Some(composer) = composer {
        let below = u16::try_from(doc.selection_end() + 1 - doc.scroll).unwrap_or(u16::MAX);
        let y = area.y.saturating_add(below).min(area.bottom().saturating_sub(box_height));
        let rect = Rect { y, height: box_height.min(area.bottom() - y), ..area };
        draw_composer(frame, composer, comments, rect);
    }

    let mark = if focused { Style::new().fg(Color::Cyan) } else { dim() };
    let marks: Vec<Line> = (doc.scroll..doc.scroll + shown)
        .map(|index| match (index == doc.cursor(), doc.is_selected(index)) {
            (true, _) => Line::styled("▌", mark.add_modifier(Modifier::BOLD)),
            (false, true) => Line::styled("▎", mark),
            (false, false) => Line::raw(""),
        })
        .collect();
    let margin = Rect { x: area.x.saturating_sub(MARGIN), width: 1, ..area };
    frame.render_widget(Paragraph::new(marks), margin);

    let total = doc.len();
    let mut status = Vec::new();
    if doc.is_missing() {
        status.push(Span::styled("file missing ", Style::new().fg(Color::Red)));
    }
    if let Some(lines) = doc.selection() {
        let style = if doc.is_selecting() { Style::new().fg(Color::Cyan) } else { dim() };
        status.push(Span::styled(format!("{} ", lines_label(lines)), style));
    }
    if total > height {
        let percent = (doc.scroll + height).min(total) * 100 / total;
        status.push(Span::styled(format!("{percent}%"), dim()));
    }
    status
}

/// The rows the comment box takes: its text's, between two borders, at most half the file.
fn composer_height(composer: &Composer, area: Rect) -> u16 {
    let rows = composer.rows(usize::from(area.width.saturating_sub(2))).0.len();
    u16::try_from(rows + 2).unwrap_or(u16::MAX).min((area.height / 2).max(3))
}

fn draw_composer(frame: &mut Frame, composer: &Composer, comments: &Comments, area: Rect) {
    let (verb, lines) = match &composer.target {
        Target::New(comment) => ("comment", Some(comment.lines)),
        Target::Edit(id) => ("edit", comments.get(*id).map(|comment| comment.lines)),
    };
    let title = match lines {
        Some(lines) => format!(" {verb} · {} ", lines_label(lines)),
        None => format!(" {verb} "),
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(Color::Cyan))
        .title(title)
        .title_bottom(Line::styled(" ⏎ save · ⇧⏎ newline · esc cancel ", dim()).right_aligned());
    let inner = block.inner(area);
    let (rows, (row, column)) = composer.rows(usize::from(inner.width));
    // A box too short for its text scrolls to keep the caret's row in it.
    let scroll = row.saturating_sub(usize::from(inner.height).saturating_sub(1));
    let text: Vec<Line> = if composer.text.is_empty() {
        vec![Line::styled("Leave a comment…", dim())]
    } else {
        rows.into_iter().skip(scroll).map(Line::raw).collect()
    };
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(text).block(block), area);
    let x = inner.x + u16::try_from(column).unwrap_or(u16::MAX).min(inner.width.saturating_sub(1));
    let y = inner.y + u16::try_from(row - scroll).unwrap_or(0);
    frame.set_cursor_position((x, y));
}

/// Every comment, one line each: its place and the start of what it says.
fn draw_list(frame: &mut Frame, app: &App, selected: usize, area: Rect) {
    let sorted = app.comments.sorted();
    if sorted.is_empty() {
        let empty = Line::styled("No comments yet. Select lines and press c.", dim());
        frame.render_widget(Paragraph::new(empty), area);
        return;
    }
    let height = usize::from(area.height).max(1);
    let offset = (selected + 1).saturating_sub(height);
    let lines: Vec<Line> = sorted
        .iter()
        .enumerate()
        .skip(offset)
        .take(height)
        .map(|(index, (_, comment))| {
            let mut location = Style::new().fg(Color::Yellow);
            if comment.outdated {
                location = location.add_modifier(Modifier::CROSSED_OUT);
            }
            let first = comment.text.lines().next().unwrap_or_default().to_owned();
            let line = Line::from(vec![
                Span::styled(comment.location(&app.tree.root), location),
                Span::raw("  "),
                Span::raw(first),
            ]);
            if index == selected {
                line.style(Style::new().add_modifier(Modifier::REVERSED))
            } else {
                line
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_title(frame: &mut Frame, area: Rect, left: Vec<Span>, right: Vec<Span>) {
    let bar = Style::new().add_modifier(Modifier::REVERSED);
    let left = Line::from([vec![Span::raw(" ")], left].concat());
    let right = Line::from([right, vec![Span::raw(" ")]].concat()).right_aligned();
    frame.render_widget(Paragraph::new(left).style(bar), area);
    frame.render_widget(Paragraph::new(right).style(bar), area);
}

fn lines_label(Lines { first, last }: Lines) -> String {
    if first == last { format!("L{first}") } else { format!("L{first}–{last}") }
}

fn inset(area: Rect) -> Rect {
    Rect { x: area.x + MARGIN, width: area.width.saturating_sub(2 * MARGIN), ..area }
}

/// `path` with the home directory written as `~`.
fn display_path(path: &Path) -> String {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    match home.as_deref().and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

fn dim() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}

fn bold() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}
