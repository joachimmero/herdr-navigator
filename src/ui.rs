//! Drawing the pane.

use std::path::Path;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{App, Screen};
use crate::doc::Doc;
use crate::picker::Picker;

/// The columns left blank on either side of the document.
const MARGIN: u16 = 1;

/// The lines a page of content gets in a pane `height` rows tall, under its title bar.
pub fn page_height(height: u16) -> usize {
    usize::from(height.saturating_sub(1))
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [title, body] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(frame.area());
    match &mut app.screen {
        Screen::Picker => draw_picker(frame, &app.picker, title, body),
        Screen::Doc(doc) => draw_doc(frame, doc, title, body),
    }
}

fn draw_picker(frame: &mut Frame, picker: &Picker, title: Rect, body: Rect) {
    let matches = picker.matches();
    let status = if picker.is_scanning() {
        "scanning…".to_owned()
    } else {
        match matches.len() {
            1 => "1 file".to_owned(),
            count => format!("{count} files"),
        }
    };
    let root = Span::styled(display_path(&picker.root), Style::new().add_modifier(Modifier::BOLD));
    draw_title(frame, title, vec![root], vec![Span::raw(status)]);

    let body = inset(body);
    let show_filter = picker.editing_filter || !picker.filter.is_empty();
    let [filter, list] =
        Layout::vertical([Constraint::Length(u16::from(show_filter)), Constraint::Fill(1)])
            .areas(body);
    if show_filter {
        let cursor = if picker.editing_filter { "▏" } else { "" };
        let line = Line::from(vec![
            Span::styled("/", dim()),
            Span::raw(picker.filter.clone()),
            Span::raw(cursor),
        ]);
        frame.render_widget(Paragraph::new(line), filter);
    }

    if matches.is_empty() && !picker.is_scanning() {
        let hint = if picker.filter.is_empty() { "No markdown files here." } else { "No matches." };
        frame.render_widget(Paragraph::new(Line::styled(hint, dim())), list);
        return;
    }
    let height = usize::from(list.height).max(1);
    let offset = picker.offset.get();
    let offset = offset.min(picker.selected).max((picker.selected + 1).saturating_sub(height));
    picker.offset.set(offset);
    let rows: Vec<Line> = matches
        .iter()
        .enumerate()
        .skip(offset)
        .take(height)
        .map(|(index, file)| file_row(file, index == picker.selected))
        .collect();
    frame.render_widget(Paragraph::new(rows), list);
}

/// A file's row, its directory dim beside its name.
fn file_row(file: &Path, selected: bool) -> Line<'static> {
    let name = file.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let dir = file
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| format!("{}/", dir.display()))
        .unwrap_or_default();
    let line = Line::from(vec![Span::styled(dir, dim()), Span::raw(name)]);
    if selected { line.style(Style::new().add_modifier(Modifier::REVERSED)) } else { line }
}

fn draw_doc(frame: &mut Frame, doc: &mut Doc, title: Rect, body: Rect) {
    let body = inset(body);
    let height = usize::from(body.height);
    frame.render_widget(Paragraph::new(doc.page(body.width, height).to_vec()), body);

    let total = doc.len();
    let mut status = Vec::new();
    if doc.is_missing() {
        status.push(Span::styled("file missing ", Style::new().fg(Color::Red)));
    }
    if total > height {
        let percent = (doc.scroll + height).min(total) * 100 / total;
        status.push(Span::styled(format!("{percent}%"), dim()));
    }
    let name = Span::styled(display_path(&doc.path), Style::new().add_modifier(Modifier::BOLD));
    draw_title(frame, title, vec![name], status);
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
