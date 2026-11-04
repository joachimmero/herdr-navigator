//! Drawing the pane.

use std::path::Path;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect, Size};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Wrap};
use ratatui_image::StatefulImage;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Focus, Mode};
use crate::comment::{self, Comments};
use crate::composer::{Composer, Target};
use crate::doc::Doc;
use crate::field::Field;
use crate::find;
use crate::herdr::Agent;
use crate::layout::{self, Lines};
use crate::picture::{self, Stage};
use crate::search::{CodeHit, Kind, Phase, ResultRow, Search};
use crate::tree::{Row, Tree};

/// The columns left blank on either side of the document.
const MARGIN: u16 = 1;
/// Below this width only the focused side shows.
const NARROW: u16 = 70;

/// Where each part of the pane goes: the title bar, and the tree and file or the search screen
/// where they show.
pub struct Panes {
    pub title: Rect,
    pub tree: Option<Rect>,
    pub file: Option<Rect>,
    /// The find band under the file while finding.
    pub find: Option<Rect>,
    pub search: Option<SearchPanes>,
}

/// Where each part of the search screen goes.
pub struct SearchPanes {
    /// The query, and the chips naming each kind of result on its right.
    pub band: Rect,
    pub chips: Rect,
    pub results_rule: Rect,
    pub list: Rect,
    pub preview_rule: Rect,
    pub preview: Rect,
}

impl Panes {
    /// The lines a page of the file takes.
    pub fn page(&self) -> usize {
        self.file.map_or(1, |file| usize::from(file.height).max(1))
    }
}

pub fn panes(area: Rect, app: &App) -> Panes {
    let [title, body] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
    if let Mode::Search(search) = &app.mode {
        let search = Some(search_panes(body, search));
        return Panes { title, tree: None, file: None, find: None, search };
    }
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
    let (file, find) = match (file, &app.mode) {
        (Some(file), Mode::Find { .. }) => {
            let [file, find] =
                Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(file);
            (Some(file), Some(find))
        }
        _ => (file, None),
    };
    Panes { title, tree, file, find, search: None }
}

fn search_panes(body: Rect, search: &Search) -> SearchPanes {
    let [band, results_rule, list, preview_rule, preview] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Percentage(40),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(body);
    let [files, code] = chip_labels(search);
    let width = u16::try_from(files.width() + CHIP_GAP.width() + code.width()).unwrap_or(u16::MAX);
    let width = width.min(band.width);
    let chips = Rect { x: band.right() - width, width, ..band };
    SearchPanes { band, chips, results_rule, list, preview_rule, preview: inset(preview) }
}

const CHIP_GAP: &str = " │ ";

/// The chips' labels, with counts once results are in.
fn chip_labels(search: &Search) -> [String; 2] {
    if search.phase != Phase::Ready {
        return ["files".into(), "code".into()];
    }
    let more = if search.results.code_more { "+" } else { "" };
    [
        format!("files {}", search.results.file_total),
        format!("code {}{more}", search.results.code.len()),
    ]
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
    if let (Mode::Search(search), Some(screen)) = (&mut app.mode, &panes.search) {
        draw_search(frame, search, &app.comments, &app.tree.root, screen);
    }
    let mut status = Vec::new();
    match (&app.mode, panes.file.or(panes.tree)) {
        (Mode::List { selected }, Some(area)) => draw_list(frame, app, *selected, area),
        _ => {
            if let (Some(area), Some(doc)) = (panes.file, &mut app.doc) {
                let (composer, find) = match &app.mode {
                    Mode::Composing(composer) => (Some(composer), None),
                    Mode::Find { field, .. } => (None, Some(field)),
                    _ => (None, None),
                };
                let focused = app.focus == Focus::File;
                let query = find.map(|field| &*field.text).or(app.found.as_deref());
                status = draw_doc(frame, doc, &app.comments, composer, query, area, focused);
                match (find, panes.find, &app.found) {
                    (Some(field), Some(band), _) => draw_find(frame, field, doc, band),
                    (None, _, Some(found)) => {
                        let label = format!("/{found} {} ", found_label(doc, found));
                        status.insert(0, Span::raw(label));
                    }
                    _ => {}
                }
            }
        }
    }
    if app.comments.len() > 0 {
        let count = comment::counted(app.comments.len());
        status.insert(0, Span::styled(format!("{count} "), Style::new().fg(Color::Yellow)));
    }
    if let Some(pending) = &app.pending {
        let label = format!("→ {} ⏎ ", pending.label(&app.tree.root));
        status.insert(0, Span::styled(label, Style::new().fg(Color::Magenta)));
    }
    if let Some(notice) = &app.notice {
        status.insert(0, Span::styled(format!("{notice} "), Style::new().fg(Color::Cyan)));
    }
    if let Mode::Picker { agents, selected } = &app.mode {
        draw_picker(frame, agents, *selected, frame.area());
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

/// Draw the file, its selection filling each source row, its cursor and selection marked in the
/// margin and the comment box under the selection while there is one, and return its part of the
/// title bar's status.
fn draw_doc(
    frame: &mut Frame,
    doc: &mut Doc,
    comments: &Comments,
    composer: Option<&Composer>,
    find: Option<&str>,
    area: Rect,
    focused: bool,
) -> Vec<Span<'static>> {
    if doc.is_image() {
        return draw_image(frame, doc, comments, composer, area);
    }
    let height = usize::from(area.height);
    let box_height = composer.map_or(0, |composer| composer_height(composer, area));
    let lines = doc.page(area.width, height, comments, usize::from(box_height));
    let lines: Vec<Line> = (doc.scroll..)
        .zip(lines)
        .map(|(index, line)| {
            let Some(chrome) = doc.chrome(index) else { return line };
            let line = match find {
                Some(query) => find::highlight(line, chrome, query),
                None => line,
            };
            let dragged = doc.dragged(index);
            layout::mark(
                line,
                chrome,
                dragged.as_slice(),
                Style::new().add_modifier(Modifier::REVERSED),
            )
        })
        .collect();
    let shown = lines.len();
    let selected_source: Vec<bool> = (doc.scroll..doc.scroll + shown)
        .map(|index| doc.is_selecting() && doc.is_selected(index) && doc.chrome(index).is_some())
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
    let fill = Style::new().add_modifier(Modifier::REVERSED);
    for (row, &selected) in selected_source.iter().enumerate() {
        if !selected {
            continue;
        }
        let y = area.y.saturating_add(u16::try_from(row).unwrap_or(u16::MAX));
        if y < area.bottom() {
            frame.buffer_mut().set_style(Rect { y, height: 1, ..area }, fill);
        }
    }
    if let Some(composer) = composer {
        let below = u16::try_from(doc.selection_end() + 1 - doc.scroll).unwrap_or(u16::MAX);
        let y = area.y.saturating_add(below).min(area.bottom().saturating_sub(box_height));
        let rect = Rect { y, height: box_height.min(area.bottom() - y), ..area };
        draw_composer(frame, composer, comments, rect);
    }

    let cursor_color = if doc.comment_at_cursor(comments).is_some() {
        Color::Yellow
    } else {
        Color::Cyan
    };
    let cursor_bar = if focused {
        Style::new().fg(cursor_color).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(cursor_color).add_modifier(Modifier::DIM)
    };
    let selection_bar = if focused { Style::new() } else { dim() };
    let pointed = Style::new().fg(Color::Magenta);
    let marks: Vec<Line> = (doc.scroll..doc.scroll + shown)
        .zip(&selected_source)
        .map(|(index, selected)| match (index == doc.cursor(), *selected, doc.is_pointed(index)) {
            (true, _, _) => Line::styled("▌", cursor_bar),
            (false, true, _) => Line::styled("▌", selection_bar),
            (false, false, true) => Line::styled("▎", pointed),
            (false, false, false) => Line::raw(""),
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

/// Draw an image fitted to `area`, or what stands in for it until it's decoded, with its comment
/// card and the comment box under it, and return its part of the title bar's status.
fn draw_image(
    frame: &mut Frame,
    doc: &mut Doc,
    comments: &Comments,
    composer: Option<&Composer>,
    area: Rect,
) -> Vec<Span<'static>> {
    let missing = doc.is_missing();
    let cards = doc.page(area.width, usize::from(area.height), comments, 0);
    let box_height = composer.map_or(0, |composer| composer_height(composer, area));
    let below = u16::try_from(cards.len()).unwrap_or(u16::MAX).saturating_add(box_height);
    let room = Size::new(area.width, area.height.saturating_sub(below));
    let Some(picture) = doc.picture() else { return Vec::new() };
    let notice = match &picture.stage {
        Stage::Loading(_) => Some("Loading image…".to_owned()),
        Stage::Ready { .. } => None,
        Stage::Failed(reason) => Some(format!("Couldn't read this image: {reason}")),
    };
    let height = match &notice {
        Some(notice) => notice.chars().count().div_ceil(usize::from(area.width.max(1))),
        None => usize::from(picture.fitted(room).height),
    };
    let height = u16::try_from(height).unwrap_or(u16::MAX).min(room.height);
    let shown = Rect { height, ..area };
    match (&notice, &mut picture.stage) {
        (Some(notice), _) => {
            let notice = Line::styled(notice.clone(), dim());
            frame.render_widget(Paragraph::new(notice).wrap(Wrap { trim: false }), shown);
        }
        (None, Stage::Ready { protocol, .. }) => {
            frame.render_stateful_widget(StatefulImage::default(), shown, &mut **protocol);
        }
        (None, _) => {}
    }
    let cards_rect = Rect { y: shown.bottom(), height: area.bottom() - shown.bottom(), ..area };
    let card_rows = u16::try_from(cards.len()).unwrap_or(u16::MAX).min(cards_rect.height);
    frame.render_widget(Paragraph::new(cards), cards_rect);
    if let Some(composer) = composer {
        let y = cards_rect.y + card_rows;
        let rect = Rect { y, height: box_height.min(area.bottom() - y), ..area };
        draw_composer(frame, composer, comments, rect);
    }

    let name = picture::format_name(picture.format);
    let label = match picture.size() {
        Some((width, height)) => format!("{name} · {width}×{height}"),
        None => name.to_owned(),
    };
    let missing = missing.then(|| Span::styled("file missing ", Style::new().fg(Color::Red)));
    missing.into_iter().chain([Span::styled(label, dim())]).collect()
}

/// The rows the comment box takes: its text's, between two borders, at most half the file.
fn composer_height(composer: &Composer, area: Rect) -> u16 {
    let rows = composer.field.rows(usize::from(area.width.saturating_sub(2))).0.len();
    u16::try_from(rows + 2).unwrap_or(u16::MAX).min((area.height / 2).max(3))
}

fn draw_composer(frame: &mut Frame, composer: &Composer, comments: &Comments, area: Rect) {
    let (verb, lines) = match &composer.target {
        Target::New(comment) => ("comment", comment.anchor.lines()),
        Target::Edit(id) => ("edit", comments.get(*id).and_then(|comment| comment.anchor.lines())),
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
    let (rows, (row, column)) = composer.field.rows(usize::from(inner.width));
    // A box too short for its text scrolls to keep the caret's row in it.
    let scroll = row.saturating_sub(usize::from(inner.height).saturating_sub(1));
    let text: Vec<Line> = if composer.field.text.is_empty() {
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

/// The agents to send to, numbered, in a box over the middle of the pane.
fn draw_picker(frame: &mut Frame, agents: &[Agent], selected: usize, area: Rect) {
    let name_width = agents.iter().map(|agent| agent.name.width()).max().unwrap_or(0);
    let lines: Vec<Line> = agents
        .iter()
        .enumerate()
        .map(|(index, agent)| {
            let number = if index < 9 { format!("{} ", index + 1) } else { "  ".into() };
            let place = if agent.tab.is_empty() {
                agent.state.clone()
            } else {
                format!("{} · {}", agent.state, agent.tab)
            };
            let line = Line::from(vec![
                Span::styled(number, dim()),
                Span::raw(format!("{:name_width$}  ", agent.name)),
                Span::styled(place, dim()),
            ]);
            if index == selected {
                line.style(Style::new().add_modifier(Modifier::REVERSED))
            } else {
                line
            }
        })
        .collect();
    let widest = lines.iter().map(Line::width).max().unwrap_or(0);
    let width = u16::try_from(widest + 4).unwrap_or(u16::MAX).max(30).min(area.width);
    let height = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX).min(area.height);
    let popup = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(Color::Cyan))
        .title(" send to ")
        .title_bottom(Line::styled(" ⏎ send · esc cancel ", dim()).right_aligned())
        .padding(ratatui::widgets::Padding::horizontal(1));
    frame.render_widget(Clear, popup);
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

/// The find band: what is being found, and which of the rows it is on the cursor is on.
fn draw_find(frame: &mut Frame, field: &Field, doc: &Doc, area: Rect) {
    let label = Span::styled("find ", dim());
    let query = match field.text.is_empty() {
        true => Span::styled("Find in file…", dim()),
        false => Span::raw(field.text.clone()),
    };
    let count = match field.text.is_empty() {
        true => String::new(),
        false => found_label(doc, &field.text),
    };
    let column = field.rows(usize::MAX).1.1;
    let caret = u16::try_from(label.width() + column).unwrap_or(u16::MAX);
    frame.render_widget(Paragraph::new(Line::from(vec![label, query])), area);
    frame.render_widget(Paragraph::new(Line::styled(count, dim()).right_aligned()), area);
    frame.set_cursor_position((area.x + caret.min(area.width.saturating_sub(1)), area.y));
}

/// Which of the rows `query` is found on the cursor is on, out of how many.
fn found_label(doc: &Doc, query: &str) -> String {
    match doc.found(query) {
        (_, 0) => "no matches".into(),
        (Some(here), total) => format!("{here}/{total}"),
        (None, total) => total.to_string(),
    }
}

fn draw_search(
    frame: &mut Frame,
    search: &mut Search,
    comments: &Comments,
    root: &Path,
    screen: &SearchPanes,
) {
    let prompt = Span::styled("> ", Style::new().fg(Color::Cyan));
    let (rows, (_, column)) = search.field.rows(usize::MAX);
    let query = match rows.into_iter().next().filter(|query| !query.is_empty()) {
        Some(query) => Span::raw(query),
        None => Span::styled("Search files and code…", dim()),
    };
    let band =
        Rect { width: screen.band.width.saturating_sub(screen.chips.width + 1), ..screen.band };
    let caret = u16::try_from(prompt.width() + column).unwrap_or(u16::MAX);
    frame.render_widget(Paragraph::new(Line::from(vec![prompt, query])), band);
    frame.set_cursor_position((band.x + caret.min(band.width.saturating_sub(1)), band.y));

    let chip = |kind: Kind| {
        if search.kind == kind {
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            dim()
        }
    };
    let [files, code] = chip_labels(search);
    let chips = Line::from(vec![
        Span::styled(files, chip(Kind::Files)),
        Span::styled(CHIP_GAP, dim()),
        Span::styled(code, chip(Kind::Code)),
    ]);
    frame.render_widget(Paragraph::new(chips), screen.chips);

    frame.render_widget(
        Paragraph::new(rule("results", screen.results_rule.width)),
        screen.results_rule,
    );
    draw_results(frame, search, screen.list);

    let label = match &search.preview {
        Some(doc) => {
            format!("preview · {}", doc.path.strip_prefix(root).unwrap_or(&doc.path).display())
        }
        None => "preview".into(),
    };
    frame.render_widget(
        Paragraph::new(rule(&label, screen.preview_rule.width)),
        screen.preview_rule,
    );
    match &mut search.preview {
        Some(doc) => {
            draw_doc(frame, doc, comments, None, None, screen.preview, false);
        }
        None => {
            frame.render_widget(Paragraph::new(Line::styled("no preview", dim())), screen.preview)
        }
    }
}

/// A rule across `width` columns, titled `label`.
fn rule(label: &str, width: u16) -> Line<'static> {
    let label = format!("─ {label} ");
    let rest = usize::from(width).saturating_sub(label.width());
    Line::styled(format!("{label}{}", "─".repeat(rest)), dim())
}

fn draw_results(frame: &mut Frame, search: &Search, area: Rect) {
    let rows = search.rows();
    let message = match &search.phase {
        Phase::Indexing => Some(Line::styled("indexing…", dim())),
        Phase::Failed(error) => Some(Line::styled(error.clone(), Style::new().fg(Color::Red))),
        // An empty query asks for no code, so it found none to report.
        Phase::Ready
            if rows.is_empty()
                && search.kind == Kind::Code
                && search.field.text.trim().is_empty() =>
        {
            Some(Line::raw(""))
        }
        Phase::Ready if rows.is_empty() => Some(Line::styled("no matches", dim())),
        Phase::Ready => None,
    };
    if let Some(message) = message {
        frame.render_widget(Paragraph::new(message), area);
        return;
    }
    let height = usize::from(area.height).max(1);
    let picked = rows.iter().position(|row| row.pick() == Some(search.pick)).unwrap_or(0);
    let scroll = search.scroll.get().min(picked).max((picked + 1).saturating_sub(height));
    search.scroll.set(scroll);
    let width = usize::from(area.width);
    let lines: Vec<Line> = rows
        .iter()
        .skip(scroll)
        .take(height)
        .map(|row| {
            let line = match row {
                ResultRow::Header(index) => {
                    let path = search.results.code[*index].path.clone();
                    Line::styled(path, Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD))
                }
                ResultRow::File(index) => {
                    let hit = &search.results.files[*index];
                    Line::from(emphasized(&hit.path, &hit.spans))
                }
                ResultRow::Code(index) => code_row(&search.results.code[*index], width),
                ResultRow::More => Line::styled("… more", dim()),
            };
            if row.pick() == Some(search.pick) {
                line.style(Style::new().add_modifier(Modifier::REVERSED))
            } else {
                line
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

/// A code hit's line number and text, cut at the front when that's what it takes to show the
/// first match.
fn code_row(hit: &CodeHit, width: usize) -> Line<'static> {
    let number = format!("{:>5}: ", hit.line);
    let room = width.saturating_sub(number.width());
    let (text, spans) = expand_tabs(hit.text.trim_end(), &hit.spans);
    let first = spans.first().map_or(0, |&(start, _)| start as usize).min(text.len());
    let first = (0..=first).rev().find(|&at| text.is_char_boundary(at)).unwrap_or(0);
    let head = text[..first].width();
    // Keep at most a third of the row's room of what comes before the match.
    let cut = if head + 8 > room && room > 8 {
        let keep = room / 3;
        let mut remaining = text[..first].char_indices().scan(head, |left, (at, ch)| {
            let here = (at, *left);
            *left -= unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            Some(here)
        });
        remaining.find(|&(_, left)| left <= keep).map_or(first, |(at, _)| at)
    } else {
        0
    };
    let shifted: Vec<(u32, u32)> = spans
        .iter()
        .filter(|&&(_, end)| end as usize > cut)
        .map(|&(start, end)| ((start as usize).saturating_sub(cut) as u32, end - cut as u32))
        .collect();
    let mut line = vec![Span::styled(number, dim())];
    if cut > 0 {
        line.push(Span::styled("…", dim()));
    }
    line.extend(emphasized(&text[cut..], &shifted));
    Line::from(line)
}

/// `text` with each tab as four spaces, and `spans` into it moved to match.
fn expand_tabs(text: &str, spans: &[(u32, u32)]) -> (String, Vec<(u32, u32)>) {
    let tabs: Vec<usize> = text.match_indices('\t').map(|(at, _)| at).collect();
    // Each tab before an offset adds three bytes.
    let shift = |at: u32| at + (tabs.partition_point(|&tab| tab < at as usize) * 3) as u32;
    let spans = spans.iter().map(|&(start, end)| (shift(start), shift(end))).collect();
    (text.replace('\t', "    "), spans)
}

/// `text` cut into spans, the bytes in `spans` marked as matches.
fn emphasized(text: &str, spans: &[(u32, u32)]) -> Vec<Span<'static>> {
    let matched =
        |at: usize| spans.iter().any(|&(start, end)| (start as usize..end as usize).contains(&at));
    let mut runs: Vec<(bool, String)> = Vec::new();
    for (at, ch) in text.char_indices() {
        let hit = matched(at);
        match runs.last_mut() {
            Some((last, run)) if *last == hit => run.push(ch),
            _ => runs.push((hit, ch.to_string())),
        }
    }
    let style = |hit: bool| {
        if hit { Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::new() }
    };
    runs.into_iter().map(|(hit, run)| Span::styled(run, style(hit))).collect()
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
    let home = crate::herdr::home();
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
