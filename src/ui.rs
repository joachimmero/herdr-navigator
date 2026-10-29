//! Drawing the pane.

use std::path::Path;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{App, Focus};
use crate::doc::Doc;
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
    if let (Some(area), Some(doc)) = (panes.file, &mut app.doc) {
        status = draw_doc(frame, doc, area);
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

/// Draw the file and return its part of the title bar's status.
fn draw_doc(frame: &mut Frame, doc: &mut Doc, area: Rect) -> Vec<Span<'static>> {
    let height = usize::from(area.height);
    frame.render_widget(Paragraph::new(doc.page(area.width, height).to_vec()), area);

    let total = doc.len();
    let mut status = Vec::new();
    if doc.is_missing() {
        status.push(Span::styled("file missing ", Style::new().fg(Color::Red)));
    }
    if total > height {
        let percent = (doc.scroll + height).min(total) * 100 / total;
        status.push(Span::styled(format!("{percent}%"), dim()));
    }
    status
}

fn draw_title(frame: &mut Frame, area: Rect, left: Vec<Span>, right: Vec<Span>) {
    let bar = Style::new().add_modifier(Modifier::REVERSED);
    let left = Line::from([vec![Span::raw(" ")], left].concat());
    let right = Line::from([right, vec![Span::raw(" ")]].concat()).right_aligned();
    frame.render_widget(Paragraph::new(left).style(bar), area);
    frame.render_widget(Paragraph::new(right).style(bar), area);
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
