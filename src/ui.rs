//! Drawing the pane.

use std::path::Path;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::Screen;
use crate::doc::Doc;

/// The columns left blank on either side of the document.
const MARGIN: u16 = 1;

/// The lines a page of content gets in a pane `height` rows tall, under its title bar.
pub fn page_height(height: u16) -> usize {
    usize::from(height.saturating_sub(1))
}

pub fn draw(frame: &mut Frame, screen: &mut Screen) {
    let [title, body] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(frame.area());
    match screen {
        Screen::Empty => {
            draw_title(frame, title, vec![Span::raw("herdr-md")], vec![]);
            let hint = Paragraph::new(Line::styled("No file open.", dim()));
            frame.render_widget(hint, inset(body));
        }
        Screen::Doc(doc) => draw_doc(frame, doc, title, body),
    }
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
