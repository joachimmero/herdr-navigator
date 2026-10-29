//! The pane: its state, its keys, and its loop.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Position, Rect};

use crate::doc::Doc;
use crate::herdr;
use crate::inbox::Inbox;
use crate::tree::Tree;
use crate::ui::{self, Panes};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Tree,
    File,
}

pub struct App {
    pub tree: Tree,
    pub doc: Option<Doc>,
    pub focus: Focus,
    /// Where `herdr-navigator <file>` posts files for this pane; `None` outside herdr.
    inbox: Option<Inbox>,
    quit: bool,
}

const TICK: Duration = Duration::from_millis(100);
const WHEEL_LINES: isize = 3;

/// Run the pane, showing `file` first when given and the tree otherwise, and return the
/// process exit code.
pub fn run(file: Option<PathBuf>) -> u8 {
    let cwd = std::env::current_dir().unwrap_or_default();
    let inbox = herdr::var("HERDR_PANE_ID").and_then(|pane| Inbox::for_pane(&pane).ok());
    // Left by an earlier pane with the same id.
    if let Some(inbox) = &inbox {
        inbox.take();
    }
    let mut terminal = ratatui::init();
    let _ = execute!(io::stdout(), EnableMouseCapture);
    let outcome = App::new(Tree::new(&cwd), inbox, file).run(&mut terminal);
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
    fn new(mut tree: Tree, inbox: Option<Inbox>, file: Option<PathBuf>) -> Self {
        tree.rescan();
        let mut app = Self { tree, doc: None, focus: Focus::Tree, inbox, quit: false };
        if let Some(file) = file {
            app.show(file);
        }
        app
    }

    fn run(mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        while !self.quit {
            if let Some(file) = self.inbox.as_ref().and_then(Inbox::take) {
                self.show(file);
            }
            self.tree.poll();
            if let Some(doc) = &mut self.doc {
                doc.refresh();
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

    /// Show `file`, keeping the place in it when it is already showing, and select it in the tree.
    fn show(&mut self, file: PathBuf) {
        self.tree.reveal(&file);
        self.open(file);
        self.focus = Focus::File;
    }

    fn open(&mut self, file: PathBuf) {
        if !self.doc.as_ref().is_some_and(|doc| doc.path == file) {
            self.doc = Some(Doc::open(file));
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
        if is_quit(&key) || key.code == KeyCode::Char('q') {
            self.quit = true;
            return;
        }
        match (self.focus, &mut self.doc) {
            (Focus::File, Some(doc)) => match key.code {
                KeyCode::Esc if doc.clear_selection() => {}
                KeyCode::Tab | KeyCode::Esc => self.focus = Focus::Tree,
                _ => on_doc_key(doc, key, panes.page()),
            },
            _ => self.on_tree_key(key, panes.page()),
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
