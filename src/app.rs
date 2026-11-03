//! The pane: its state, its keys, and its loop.

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, KeyboardEnhancementFlags, MouseButton, MouseEvent, MouseEventKind,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::supports_keyboard_enhancement;
use ratatui::layout::{Position, Rect};
use ratatui_image::picker::Picker;

use crate::comment::{self, Anchor, Comment, Comments};
use crate::composer::{Composer, Target};
use crate::doc::{Doc, Spot};
use crate::field::Field;
use crate::herdr::{self, Agent};
use crate::inbox::Inbox;
use crate::jumps::{Jumps, Place};
use crate::layout::Lines;
use crate::location::Location;
use crate::search::{Outcome, Search, Searcher};
use crate::tree::Tree;
use crate::ui::{self, Panes};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Tree,
    File,
}

/// What the keys drive besides the tree and the file.
pub enum Mode {
    Browse,
    Composing(Composer),
    /// The list of every comment, and the selected one's place in it.
    List {
        selected: usize,
    },
    /// The agents to send the comments to, and the highlighted one.
    Picker {
        agents: Vec<Agent>,
        selected: usize,
    },
    Search(Box<Search>),
    /// Finding text in the file, and where the cursor was before, for `Esc` to go back to.
    Find {
        field: Field,
        from: Spot,
    },
}

pub struct App {
    pub tree: Tree,
    pub doc: Option<Doc>,
    pub focus: Focus,
    pub comments: Comments,
    pub mode: Mode,
    /// What the last key did, when that needs saying; the next key clears it.
    pub notice: Option<String>,
    /// A place pointed at while a comment was being written, which `⏎` goes to once it's done.
    pub pending: Option<Location>,
    /// What the last find kept, never empty: marked in every file, and what `n` `N` go to.
    pub found: Option<String>,
    jumps: Jumps,
    graphics: Graphics,
    /// Whether markdown shows rendered rather than as its source, in every file the pane opens.
    rendered: bool,
    /// Started the first time search opens.
    searcher: Option<Searcher>,
    /// The pane the comments last went to, which the picker opens on.
    last_sent: Option<String>,
    /// Where `herdr-navigator <file>` posts files for this pane; `None` outside herdr.
    inbox: Option<Inbox>,
    quit: bool,
}

/// What images are drawn with.
enum Graphics {
    /// Chosen without the terminal's cell size, which a pane can lack until it has been sized,
    /// so asked for again once an image shows.
    Unsized(Picker),
    Settled(Picker),
}

impl Graphics {
    /// Ask the terminal, so only while nothing else is reading its input.
    fn query() -> Self {
        match Picker::from_query_stdio() {
            // Without a cell size the answer is halfblocks, every capability dropped.
            Ok(picker) if picker.capabilities().is_empty() => Self::Unsized(picker),
            Ok(picker) => Self::Settled(picker),
            Err(_) => Self::Settled(Picker::halfblocks()),
        }
    }

    fn picker(&self) -> &Picker {
        match self {
            Self::Unsized(picker) | Self::Settled(picker) => picker,
        }
    }
}

const TICK: Duration = Duration::from_millis(100);
/// Quick enough that results show as soon as they're in.
const SEARCH_TICK: Duration = Duration::from_millis(16);
const WHEEL_LINES: isize = 3;

/// Run the pane, showing `location` first when given and the tree otherwise, and return the
/// process exit code.
pub fn run(location: Option<Location>) -> u8 {
    let cwd = std::env::current_dir().unwrap_or_default();
    let inbox = herdr::var("HERDR_PANE_ID").and_then(|pane| Inbox::for_pane(&pane).ok());
    // Left by an earlier pane with the same id.
    if let Some(inbox) = &inbox {
        inbox.take();
    }
    let mut terminal = ratatui::init();
    let graphics = Graphics::query();
    let _ = execute!(io::stdout(), EnableMouseCapture);
    // Where the terminal can, have it tell keys apart that it otherwise sends alike, like `Tab`
    // and `Ctrl+i`, or `Enter` and `Shift+Enter`.
    let enhanced = supports_keyboard_enhancement().unwrap_or(false);
    if enhanced {
        let flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES;
        let _ = execute!(io::stdout(), PushKeyboardEnhancementFlags(flags));
    }
    let outcome = App::new(Tree::new(&cwd), inbox, location, graphics).run(&mut terminal);
    if enhanced {
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = execute!(io::stdout(), DisableMouseCapture);
    ratatui::restore();
    match outcome {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("herdr-navigator: {error}");
            1
        }
    }
}

fn is_quit(key: &KeyEvent) -> bool {
    key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
}

impl App {
    fn new(
        mut tree: Tree,
        inbox: Option<Inbox>,
        location: Option<Location>,
        graphics: Graphics,
    ) -> Self {
        tree.rescan();
        tree.watch();
        let mut app = Self {
            tree,
            doc: None,
            focus: Focus::Tree,
            comments: Comments::default(),
            mode: Mode::Browse,
            notice: None,
            pending: None,
            found: None,
            jumps: Jumps::default(),
            graphics,
            rendered: true,
            searcher: None,
            last_sent: None,
            inbox,
            quit: false,
        };
        if let Some(location) = location {
            app.go(location);
        }
        app
    }

    fn run(mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        while !self.quit {
            if let Some(location) = self.inbox.as_ref().and_then(Inbox::take) {
                match self.mode {
                    Mode::Composing(_) => self.pending = Some(location),
                    _ => self.go(location),
                }
            }
            self.tree.poll();
            if let Some(doc) = &mut self.doc {
                if doc.refresh() {
                    self.comments.reanchor(&doc.path, doc.text(), doc.hash());
                }
                doc.poll();
            }
            self.pump_search();
            self.settle_graphics();
            terminal.draw(|frame| ui::draw(frame, &mut self))?;
            let tick = if matches!(self.mode, Mode::Search(_)) { SEARCH_TICK } else { TICK };
            if event::poll(tick)? {
                let size = terminal.size()?;
                let panes = ui::panes(Rect::new(0, 0, size.width, size.height), &self);
                self.on_event(event::read()?, &panes);
            }
        }
        Ok(())
    }

    /// Take in the search's latest results, ask for the query's when it changed, and preview the
    /// pick.
    fn pump_search(&mut self) {
        let (Mode::Search(search), Some(searcher)) = (&mut self.mode, &mut self.searcher) else {
            return;
        };
        if let Some(outcome) = searcher.outcome() {
            search.land(outcome);
        }
        if std::mem::take(&mut search.dirty) && !searcher.query(search.field.text.clone()) {
            search.land(Outcome::Failed("search worker unavailable".into()));
        }
        search.follow_pick(&self.tree.root, self.graphics.picker());
        if let Some(preview) = &mut search.preview {
            preview.poll();
        }
    }

    /// Ask the terminal once more what images are drawn with, when its first answer came without
    /// its cell size and an image is showing, and redraw what shows with the new answer.
    fn settle_graphics(&mut self) {
        let Graphics::Unsized(_) = self.graphics else { return };
        let preview = match &mut self.mode {
            Mode::Search(search) => search.preview.as_mut(),
            _ => None,
        };
        let mut shown: Vec<&mut Doc> = self.doc.iter_mut().chain(preview).collect();
        if !shown.iter().any(|doc| doc.is_image()) {
            return;
        }
        let picker = Graphics::query().picker().clone();
        shown.iter_mut().for_each(|doc| doc.use_picker(&picker));
        self.graphics = Graphics::Settled(picker);
    }

    /// Show `location`'s file, with its lines marked when it names some.
    fn go(&mut self, location: Location) {
        self.mode = Mode::Browse;
        self.remember();
        self.show(location.file);
        if let (Some(doc), Some(lines)) = (&mut self.doc, location.lines) {
            doc.point(lines);
        }
    }

    /// Show `file`, keeping the place in it when it is already showing, and select it in the tree.
    fn show(&mut self, file: PathBuf) {
        self.tree.reveal(&file);
        self.open(file);
        self.focus = Focus::File;
    }

    fn open(&mut self, file: PathBuf) {
        if !self.doc.as_ref().is_some_and(|doc| doc.path == file) {
            let mut doc = Doc::open(file, self.graphics.picker());
            doc.set_rendered(self.rendered);
            self.comments.reanchor(&doc.path, doc.text(), doc.hash());
            self.doc = Some(doc);
        }
    }

    /// Show the tree's selection when it is a file.
    fn follow_tree(&mut self) {
        let file = self.tree.selected().filter(|node| !node.dir).map(|node| node.path.clone());
        if let Some(file) = file {
            self.open(self.tree.root.join(file));
        }
    }

    fn on_event(&mut self, event: Event, panes: &Panes) {
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => self.on_key(key, panes),
            Event::Mouse(mouse) => self.on_mouse(mouse, panes),
            _ => {}
        }
    }

    fn on_mouse(&mut self, mouse: MouseEvent, panes: &Panes) {
        match self.mode {
            Mode::Browse => {}
            Mode::Search(_) => return self.on_search_mouse(mouse, panes),
            _ => return,
        }
        let at = Position::new(mouse.column, mouse.row);
        let in_tree = panes.tree.is_some_and(|tree| tree.contains(at));
        let in_file = panes.file.is_some_and(|file| file.contains(at));
        let delta = match mouse.kind {
            MouseEventKind::ScrollDown => WHEEL_LINES,
            MouseEventKind::ScrollUp => -WHEEL_LINES,
            MouseEventKind::Down(MouseButton::Left) if in_file => {
                let file = panes.file.expect("the click landed in it");
                if let Some(doc) = &mut self.doc {
                    let column = usize::from(at.x - file.x);
                    doc.press(usize::from(at.y - file.y), column, panes.page());
                    self.focus = Focus::File;
                }
                return;
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let (Some(doc), Some(file)) = (&mut self.doc, panes.file) {
                    // Lossless: both are u16s.
                    let offset = (i32::from(at.y) - i32::from(file.y)) as isize;
                    let column = usize::from(at.x.saturating_sub(file.x));
                    doc.drag_to(offset, column, panes.page());
                }
                return;
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some(text) = self.doc.as_mut().and_then(Doc::release) {
                    self.notice = Some(match pbcopy(&text) {
                        Ok(()) => format!("copied {} characters", text.chars().count()),
                        Err(error) => format!("copy failed: {error}"),
                    });
                }
                return;
            }
            MouseEventKind::Down(MouseButton::Left) if in_tree => {
                let tree = panes.tree.expect("the click landed in it");
                if let Some(doc) = &mut self.doc {
                    doc.clear_drag();
                }
                let row = usize::from(at.y - tree.y) + self.tree.offset.get();
                let again = row == self.tree.selected_index();
                self.tree.select_index(row);
                self.focus = Focus::Tree;
                if again {
                    self.tree.toggle();
                }
                self.follow_tree();
                return;
            }
            _ => return,
        };
        match (in_tree, &mut self.doc) {
            (true, _) => {
                self.tree.move_by(delta);
                self.follow_tree();
            }
            (false, Some(doc)) => doc.scroll_by(delta, panes.page()),
            (false, None) => {}
        }
    }

    fn on_key(&mut self, key: KeyEvent, panes: &Panes) {
        if is_quit(&key) {
            self.quit = true;
            return;
        }
        self.notice = None;
        if let Some(doc) = &mut self.doc {
            doc.clear_drag();
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return self.on_chord(key, panes);
        }
        match &mut self.mode {
            Mode::Picker { .. } => return self.on_picker_key(key),
            Mode::Search(_) => return self.on_search_key(key, panes),
            Mode::Find { .. } => return self.on_find_key(key),
            Mode::Composing(composer) => {
                if on_composer_key(&mut composer.field, key) {
                    self.finish_composing(key.code == KeyCode::Enter);
                }
                return;
            }
            Mode::List { selected } => {
                let selected = *selected;
                return self.on_list_key(key, selected);
            }
            Mode::Browse => {}
        }
        match key.code {
            KeyCode::Char('m') => match &mut self.doc {
                Some(doc) if doc.is_markdown() => {
                    self.rendered = !self.rendered;
                    doc.set_rendered(self.rendered);
                }
                _ => self.notice = Some("not markdown".into()),
            },
            KeyCode::Enter if self.pending.is_some() => {
                let pending = self.pending.take().expect("checked just above");
                self.go(pending);
            }
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('n') => self.next(true),
            KeyCode::Char('N') => self.next(false),
            KeyCode::Char('s') => self.send(),
            KeyCode::Char('y') => self.copy(),
            KeyCode::Char('/') => self.open_find(),
            _ => match (self.focus, &mut self.doc) {
                (Focus::File, Some(doc)) => match key.code {
                    KeyCode::Esc if self.found.take().is_some() => {}
                    KeyCode::Esc if doc.clear_selection() => {}
                    KeyCode::Tab | KeyCode::Esc => self.focus = Focus::Tree,
                    KeyCode::Char('c') => self.compose(),
                    KeyCode::Char('e') => self.edit(),
                    KeyCode::Char('d') => {
                        if let Some(id) = doc.comment_at_cursor(&self.comments) {
                            self.comments.remove(id);
                        }
                    }
                    KeyCode::Char('l') => self.mode = Mode::List { selected: 0 },
                    _ => on_doc_key(doc, key, panes.page()),
                },
                _ => self.on_tree_key(key),
            },
        }
    }

    /// Act on a key pressed with `Ctrl`, which never does what the key alone does.
    fn on_chord(&mut self, key: KeyEvent, panes: &Panes) {
        match &mut self.mode {
            Mode::Composing(composer) => {
                if key.code == KeyCode::Char('j') {
                    composer.field.insert('\n');
                }
            }
            Mode::Search(search) => match key.code {
                KeyCode::Char('n') => search.move_pick(1),
                KeyCode::Char('p') => search.move_pick(-1),
                _ => {}
            },
            Mode::Browse => match key.code {
                KeyCode::Char('o') => {
                    let back = self.here().and_then(|here| self.jumps.back(here));
                    if let Some(place) = back {
                        self.visit(place);
                    }
                }
                KeyCode::Char('i') => {
                    if let Some(place) = self.jumps.forward() {
                        self.visit(place);
                    }
                }
                KeyCode::Char('f') => self.open_search(),
                KeyCode::Char('d') => self.move_half_page(1, panes.page()),
                KeyCode::Char('u') => self.move_half_page(-1, panes.page()),
                _ => {}
            },
            Mode::List { .. } | Mode::Picker { .. } | Mode::Find { .. } => {}
        }
    }

    /// Move the focused side half a page down for a positive `direction`, up for a negative one.
    fn move_half_page(&mut self, direction: isize, page: usize) {
        let delta = direction * (page / 2).max(1) as isize;
        match (self.focus, &mut self.doc) {
            (Focus::File, Some(doc)) => doc.page_by(delta, page),
            _ => {
                self.tree.move_by(delta);
                self.follow_tree();
            }
        }
    }

    /// Start a comment on the selected lines, or on the whole of an image, editing the one it
    /// already has.
    fn compose(&mut self) {
        let Some(doc) = &self.doc else { return };
        if doc.is_image() && doc.comment_at_cursor(&self.comments).is_some() {
            return self.edit();
        }
        let anchor = if doc.is_image() {
            Anchor::File { hash: doc.hash() }
        } else {
            let Some(lines) = doc.selection() else { return };
            let snippet: Vec<&str> = doc
                .text()
                .lines()
                .skip(lines.first - 1)
                .take(lines.last - lines.first + 1)
                .collect();
            Anchor::Lines { lines, snippet: snippet.join("\n") }
        };
        let comment =
            Comment { file: doc.path.clone(), anchor, text: String::new(), outdated: false };
        self.mode = Mode::Composing(Composer::new(Target::New(comment), String::new()));
    }

    /// Edit the comment at the cursor.
    fn edit(&mut self) {
        let Some(id) = self.doc.as_ref().and_then(|doc| doc.comment_at_cursor(&self.comments))
        else {
            return;
        };
        let text = self.comments.get(id).map(|comment| comment.text.clone()).unwrap_or_default();
        self.mode = Mode::Composing(Composer::new(Target::Edit(id), text));
    }

    /// Close the comment box, writing what it holds when `save` and it holds anything.
    fn finish_composing(&mut self, save: bool) {
        let Mode::Composing(composer) = std::mem::replace(&mut self.mode, Mode::Browse) else {
            return;
        };
        let text = composer.field.text.trim_end().to_owned();
        if !save || text.trim().is_empty() {
            return;
        }
        match composer.target {
            Target::New(comment) => {
                self.comments.add(Comment { text, ..comment });
                if let Some(doc) = &mut self.doc {
                    doc.clear_selection();
                }
            }
            Target::Edit(id) => self.comments.set_text(id, text),
        }
    }

    /// Go to the next place the kept find is found after the cursor, or the one before it, and
    /// to the next comment without one.
    fn next(&mut self, forward: bool) {
        let (Some(query), Some(doc)) = (&self.found, &mut self.doc) else {
            return self.next_comment(forward);
        };
        if doc.found(query).1 == 0 {
            self.notice = Some(format!("{query} not found"));
            return;
        }
        doc.find(query, if forward { 1 } else { -1 });
        self.focus = Focus::File;
    }

    /// Go to the next comment after the cursor in file and line order, or the one before it,
    /// wrapping around.
    fn next_comment(&mut self, forward: bool) {
        let here = self.doc.as_ref().map(|doc| (doc.path.clone(), doc.cursor_line().unwrap_or(0)));
        let sorted = self.comments.sorted();
        let places: Vec<(PathBuf, usize)> =
            sorted.iter().map(|(_, comment)| (comment.file.clone(), comment.line())).collect();
        let target = match (forward, &here) {
            (true, Some(here)) => places.iter().find(|place| *place > here),
            (false, Some(here)) => places.iter().rev().find(|place| *place < here),
            (_, None) => None,
        };
        let wrapped = if forward { places.first() } else { places.last() };
        if let Some((file, line)) = target.or(wrapped).cloned() {
            self.jump(file, line);
        }
    }

    /// Show `file` with the cursor on `line`, remembering where it was.
    fn jump(&mut self, file: PathBuf, line: usize) {
        self.remember();
        self.visit((file, line));
    }

    fn visit(&mut self, (file, line): Place) {
        self.show(file);
        if let Some(doc) = &mut self.doc {
            doc.go_to_line(line);
        }
    }

    /// Remember where the cursor is before jumping away, for `Ctrl+o` to come back to.
    fn remember(&mut self) {
        if let Some(here) = self.here() {
            self.jumps.record(here);
        }
    }

    fn here(&self) -> Option<Place> {
        let doc = self.doc.as_ref()?;
        Some((doc.path.clone(), doc.cursor_line()?))
    }

    /// Send every comment to the workspace's one agent, or let a picker choose among several.
    fn send(&mut self) {
        if self.comments.len() == 0 {
            self.notice = Some("no comments yet".into());
            return;
        }
        match herdr::agents() {
            Ok(agents) if agents.is_empty() => {
                self.notice = Some("no agent in this workspace, press y to copy".into());
            }
            Ok(agents) if agents.len() == 1 => self.send_to(&agents[0]),
            Ok(agents) => {
                let last = self.last_sent.as_deref();
                let selected = agents.iter().position(|agent| Some(&*agent.pane) == last);
                self.mode = Mode::Picker { agents, selected: selected.unwrap_or(0) };
            }
            Err(error) => self.notice = Some(format!("{error}, press y to copy")),
        }
    }

    /// Send every comment to `agent`, letting go of them once it has them.
    fn send_to(&mut self, agent: &Agent) {
        let count = self.comments.len();
        let text = self.export();
        match herdr::send(&agent.pane, &text) {
            Ok(()) => {
                self.comments.clear();
                self.last_sent = Some(agent.pane.clone());
                self.notice = Some(format!("sent {} to {}", comment::counted(count), agent.name));
            }
            Err(error) => self.notice = Some(format!("{error}, press y to copy")),
        }
    }

    /// Copy every comment to the clipboard, letting go of them once it has them.
    fn copy(&mut self) {
        if self.comments.len() == 0 {
            self.notice = Some("no comments yet".into());
            return;
        }
        let count = self.comments.len();
        let text = self.export();
        match pbcopy(&text) {
            Ok(()) => {
                self.comments.clear();
                self.notice = Some(format!("copied {}", comment::counted(count)));
            }
            Err(error) => self.notice = Some(format!("copy failed: {error}")),
        }
    }

    /// Every comment as the message it is sent as, each first followed to where its lines sit in
    /// the file now, open or not.
    fn export(&mut self) -> String {
        for file in self.comments.files() {
            let bytes = fs::read(&file).unwrap_or_default();
            self.comments.reanchor(&file, &String::from_utf8_lossy(&bytes), comment::hash(&bytes));
        }
        let sorted = self.comments.sorted();
        let comments: Vec<&Comment> = sorted.iter().map(|(_, comment)| *comment).collect();
        comment::format_all(&comments, &self.tree.root)
    }

    fn on_picker_key(&mut self, key: KeyEvent) {
        let Mode::Picker { agents, selected } = &mut self.mode else { return };
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                *selected = (*selected + 1).min(agents.len().saturating_sub(1));
            }
            KeyCode::Char('k') | KeyCode::Up => *selected = selected.saturating_sub(1),
            KeyCode::Char(digit @ '1'..='9') => {
                let row = usize::from(digit as u8 - b'1');
                if row < agents.len() {
                    *selected = row;
                }
            }
            KeyCode::Enter => {
                let agent = agents.get(*selected).cloned();
                self.mode = Mode::Browse;
                if let Some(agent) = agent {
                    self.send_to(&agent);
                }
            }
            KeyCode::Esc | KeyCode::Char('q') => self.mode = Mode::Browse,
            _ => {}
        }
    }

    fn on_list_key(&mut self, key: KeyEvent, selected: usize) {
        let sorted = self.comments.sorted();
        let last = sorted.len().saturating_sub(1);
        let chosen =
            sorted.get(selected).map(|(id, comment)| (*id, comment.file.clone(), comment.line()));
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.mode = Mode::List { selected: (selected + 1).min(last) };
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.mode = Mode::List { selected: selected.saturating_sub(1) };
            }
            KeyCode::Char('d') => {
                if let Some((id, _, _)) = chosen {
                    self.comments.remove(id);
                    self.mode = Mode::List { selected: selected.min(last.saturating_sub(1)) };
                }
            }
            KeyCode::Enter => {
                self.mode = Mode::Browse;
                if let Some((_, file, line)) = chosen {
                    self.jump(file, line);
                }
            }
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Esc | KeyCode::Char('l') => self.mode = Mode::Browse,
            _ => {}
        }
    }

    fn open_find(&mut self) {
        if let Some(doc) = self.doc.as_mut().filter(|doc| !doc.is_image()) {
            doc.clear_selection();
            self.focus = Focus::File;
            self.mode = Mode::Find { field: Field::new(String::new()), from: doc.spot() };
        }
    }

    fn on_find_key(&mut self, key: KeyEvent) {
        let (Mode::Find { field, from }, Some(doc)) = (&mut self.mode, &mut self.doc) else {
            return;
        };
        match key.code {
            KeyCode::Esc => {
                doc.return_to(*from);
                self.mode = Mode::Browse;
            }
            KeyCode::Enter => {
                let query = std::mem::take(&mut field.text);
                match doc.found(&query).1 {
                    _ if query.is_empty() => doc.return_to(*from),
                    0 => {
                        doc.return_to(*from);
                        self.notice = Some(format!("{query} not found"));
                    }
                    _ => self.found = Some(query),
                }
                self.mode = Mode::Browse;
            }
            KeyCode::Down => doc.find(&field.text, 1),
            KeyCode::Up => doc.find(&field.text, -1),
            _ => {
                let before = field.text.clone();
                on_field_key(field, key);
                if field.text != before {
                    doc.find(&field.text, 0);
                }
            }
        }
    }

    fn open_search(&mut self) {
        let root = self.tree.root.clone();
        self.searcher.get_or_insert_with(|| Searcher::start(root));
        self.mode = Mode::Search(Box::new(Search::new()));
    }

    fn on_search_key(&mut self, key: KeyEvent, panes: &Panes) {
        let Mode::Search(search) = &mut self.mode else { return };
        let page = panes.search.as_ref().map_or(1, |screen| usize::from(screen.preview.height));
        match key.code {
            KeyCode::Esc => self.mode = Mode::Browse,
            KeyCode::Enter => self.open_pick(),
            KeyCode::Tab => search.flip(),
            KeyCode::PageDown => search.scroll_preview(page as isize, page),
            KeyCode::PageUp => search.scroll_preview(-(page as isize), page),
            KeyCode::Down => search.move_pick(1),
            KeyCode::Up => search.move_pick(-1),
            _ => {
                let before = search.field.text.clone();
                on_field_key(&mut search.field, key);
                search.dirty |= search.field.text != before;
            }
        }
    }

    fn on_search_mouse(&mut self, mouse: MouseEvent, panes: &Panes) {
        let (Mode::Search(search), Some(screen)) = (&mut self.mode, &panes.search) else {
            return;
        };
        let at = Position::new(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) if screen.chips.contains(at) => search.flip(),
            MouseEventKind::Down(MouseButton::Left) if screen.list.contains(at) => {
                let row = search.scroll.get() + usize::from(at.y - screen.list.y);
                if search.pick_row(row) {
                    self.open_pick();
                }
            }
            MouseEventKind::ScrollDown if screen.list.contains(at) => search.move_pick(1),
            MouseEventKind::ScrollUp if screen.list.contains(at) => search.move_pick(-1),
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
                if screen.preview.contains(at) =>
            {
                let delta = match mouse.kind {
                    MouseEventKind::ScrollDown => WHEEL_LINES,
                    _ => -WHEEL_LINES,
                };
                search.scroll_preview(delta, usize::from(screen.preview.height));
            }
            _ => {}
        }
    }

    /// Show the picked result, at its line for a code hit.
    fn open_pick(&mut self) {
        let Mode::Search(search) = &self.mode else { return };
        let Some((path, line)) = search.target().map(|(path, line)| (path.to_owned(), line)) else {
            return;
        };
        let file = self.tree.root.join(&path);
        if !file.is_file() {
            return;
        }
        if let Some(searcher) = &self.searcher {
            searcher.track(path);
        }
        self.go(Location { file, lines: line.map(Lines::one) });
    }

    fn on_tree_key(&mut self, key: KeyEvent) {
        let tree = &mut self.tree;
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => tree.move_by(1),
            KeyCode::Char('k') | KeyCode::Up => tree.move_by(-1),
            KeyCode::Char('g') | KeyCode::Home => tree.select_index(0),
            KeyCode::Char('G') | KeyCode::End => tree.move_by(isize::MAX),
            KeyCode::Char('h') | KeyCode::Left => tree.collapse(),
            KeyCode::Char('i') => tree.toggle_ignored(),
            KeyCode::Char('r') => tree.rescan(),
            KeyCode::Tab if self.doc.is_some() => self.focus = Focus::File,
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => match tree.selected() {
                Some(node) if node.dir => tree.toggle(),
                Some(_) => self.focus = Focus::File,
                None => {}
            },
            _ => return,
        }
        self.follow_tree();
    }
}

/// Edit the comment box, saying whether the key closes it: `Enter` to save, `Esc` to cancel.
fn on_composer_key(field: &mut Field, key: KeyEvent) -> bool {
    let newline = key.modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT);
    match key.code {
        KeyCode::Enter if newline => field.insert('\n'),
        KeyCode::Enter | KeyCode::Esc => return true,
        _ => on_field_key(field, key),
    }
    false
}

/// Edit `field` the way any typed text is edited.
fn on_field_key(field: &mut Field, key: KeyEvent) {
    match key.code {
        KeyCode::Backspace => field.backspace(),
        KeyCode::Delete => field.delete(),
        KeyCode::Left => field.left(),
        KeyCode::Right => field.right(),
        KeyCode::Home => field.home(),
        KeyCode::End => field.end(),
        KeyCode::Char(ch) => field.insert(ch),
        _ => {}
    }
}

fn on_doc_key(doc: &mut Doc, key: KeyEvent, page: usize) {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => doc.move_cursor(1, page),
        KeyCode::Char('k') | KeyCode::Up => doc.move_cursor(-1, page),
        KeyCode::PageDown | KeyCode::Char(' ') => doc.page_by(page as isize, page),
        KeyCode::PageUp => doc.page_by(-(page as isize), page),
        KeyCode::Char('g') | KeyCode::Home => doc.move_cursor(isize::MIN, page),
        KeyCode::Char('G') | KeyCode::End => doc.move_cursor(isize::MAX, page),
        KeyCode::Char('v') if !doc.is_image() => doc.toggle_selection(),
        _ => {}
    }
}

fn pbcopy(text: &str) -> io::Result<()> {
    let mut child = Command::new("pbcopy").stdin(Stdio::piped()).spawn()?;
    child.stdin.take().expect("stdin is piped").write_all(text.as_bytes())?;
    match child.wait()? {
        status if status.success() => Ok(()),
        status => Err(io::Error::other(format!("pbcopy {status}"))),
    }
}
