//! The pane: its state, its keys, and its loop.

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Position, Rect};

use crate::comment::{self, Comment, Comments};
use crate::composer::{Composer, Target};
use crate::doc::Doc;
use crate::herdr::{self, Agent};
use crate::inbox::Inbox;
use crate::location::Location;
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
    /// The pane the comments last went to, which the picker opens on.
    last_sent: Option<String>,
    /// Where `herdr-navigator <file>` posts files for this pane; `None` outside herdr.
    inbox: Option<Inbox>,
    quit: bool,
}

const TICK: Duration = Duration::from_millis(100);
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
    let _ = execute!(io::stdout(), EnableMouseCapture);
    let outcome = App::new(Tree::new(&cwd), inbox, location).run(&mut terminal);
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

fn half_page_direction(key: &KeyEvent) -> Option<isize> {
    if !key.modifiers.contains(KeyModifiers::CONTROL) {
        return None;
    }
    match key.code {
        KeyCode::Char('d') => Some(1),
        KeyCode::Char('u') => Some(-1),
        _ => None,
    }
}

impl App {
    fn new(mut tree: Tree, inbox: Option<Inbox>, location: Option<Location>) -> Self {
        tree.rescan();
        let mut app = Self {
            tree,
            doc: None,
            focus: Focus::Tree,
            comments: Comments::default(),
            mode: Mode::Browse,
            notice: None,
            pending: None,
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
            if let Some(doc) = &mut self.doc
                && doc.refresh()
            {
                self.comments.reanchor(&doc.path, doc.text());
            }
            terminal.draw(|frame| ui::draw(frame, &mut self))?;
            if event::poll(TICK)? {
                let size = terminal.size()?;
                let panes = ui::panes(Rect::new(0, 0, size.width, size.height), &self);
                self.on_event(event::read()?, &panes);
            }
        }
        Ok(())
    }

    /// Show `location`'s file, with its lines marked when it names some.
    fn go(&mut self, location: Location) {
        self.mode = Mode::Browse;
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
            let doc = Doc::open(file);
            self.comments.reanchor(&doc.path, doc.text());
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
        if !matches!(self.mode, Mode::Browse) {
            return;
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
                    doc.click(usize::from(at.y - file.y), panes.page());
                    self.focus = Focus::File;
                }
                return;
            }
            MouseEventKind::Down(MouseButton::Left) if in_tree => {
                let tree = panes.tree.expect("the click landed in it");
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
        match &mut self.mode {
            Mode::Picker { .. } => return self.on_picker_key(key),
            Mode::Composing(composer) => {
                if on_composer_key(composer, key) {
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
            KeyCode::Enter if self.pending.is_some() => {
                let pending = self.pending.take().expect("checked just above");
                self.go(pending);
            }
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('n') => self.next_comment(true),
            KeyCode::Char('N') => self.next_comment(false),
            KeyCode::Char('s') => self.send(),
            KeyCode::Char('y') => self.copy(),
            _ => match (self.focus, &mut self.doc) {
                (Focus::File, Some(doc)) => match key.code {
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
                _ => self.on_tree_key(key, panes.page()),
            },
        }
    }

    /// Start a comment on the selected lines.
    fn compose(&mut self) {
        let Some(doc) = &self.doc else { return };
        let Some(lines) = doc.selection() else { return };
        let snippet: Vec<&str> =
            doc.text().lines().skip(lines.first - 1).take(lines.last - lines.first + 1).collect();
        let comment = Comment {
            file: doc.path.clone(),
            lines,
            snippet: snippet.join("\n"),
            text: String::new(),
            outdated: false,
        };
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
        let text = composer.text.trim_end().to_owned();
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

    /// Go to the next comment after the cursor in file and line order, or the one before it,
    /// wrapping around.
    fn next_comment(&mut self, forward: bool) {
        let here = self.doc.as_ref().map(|doc| (doc.path.clone(), doc.cursor_line().unwrap_or(0)));
        let sorted = self.comments.sorted();
        let places: Vec<(PathBuf, usize)> =
            sorted.iter().map(|(_, comment)| (comment.file.clone(), comment.lines.first)).collect();
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

    /// Show `file` with the cursor on `line`.
    fn jump(&mut self, file: PathBuf, line: usize) {
        self.show(file);
        if let Some(doc) = &mut self.doc {
            doc.go_to_line(line);
        }
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
            let source = fs::read_to_string(&file).unwrap_or_default();
            self.comments.reanchor(&file, &source);
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
        let chosen = sorted
            .get(selected)
            .map(|(id, comment)| (*id, comment.file.clone(), comment.lines.first));
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

    fn on_tree_key(&mut self, key: KeyEvent, page: usize) {
        let tree = &mut self.tree;
        if let Some(direction) = half_page_direction(&key) {
            tree.move_by(direction * (page / 2).max(1) as isize);
            self.follow_tree();
            return;
        }
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
fn on_composer_key(composer: &mut Composer, key: KeyEvent) -> bool {
    let newline = key.modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT);
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Enter if newline => composer.insert('\n'),
        KeyCode::Char('j') if control => composer.insert('\n'),
        KeyCode::Enter | KeyCode::Esc => return true,
        KeyCode::Backspace => composer.backspace(),
        KeyCode::Delete => composer.delete(),
        KeyCode::Left => composer.left(),
        KeyCode::Right => composer.right(),
        KeyCode::Home => composer.home(),
        KeyCode::End => composer.end(),
        KeyCode::Char(ch) if !control => composer.insert(ch),
        _ => {}
    }
    false
}

fn on_doc_key(doc: &mut Doc, key: KeyEvent, page: usize) {
    if let Some(direction) = half_page_direction(&key) {
        doc.page_by(direction * (page / 2).max(1) as isize, page);
        return;
    }
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => doc.move_cursor(1, page),
        KeyCode::Char('k') | KeyCode::Up => doc.move_cursor(-1, page),
        KeyCode::PageDown | KeyCode::Char(' ') => doc.page_by(page as isize, page),
        KeyCode::PageUp => doc.page_by(-(page as isize), page),
        KeyCode::Char('g') | KeyCode::Home => doc.move_cursor(isize::MIN, page),
        KeyCode::Char('G') | KeyCode::End => doc.move_cursor(isize::MAX, page),
        KeyCode::Char('v') => doc.toggle_selection(),
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
