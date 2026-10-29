//! The pane: its state, its keys, and its loop.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseEventKind,
};
use ratatui::crossterm::execute;

use crate::doc::Doc;
use crate::herdr;
use crate::inbox::Inbox;
use crate::picker::Picker;
use crate::ui;

pub enum Screen {
    Picker,
    Doc(Doc),
}

pub struct App {
    pub screen: Screen,
    pub picker: Picker,
    /// Where `herdr-navigator <file>` posts files for this pane; `None` outside herdr.
    inbox: Option<Inbox>,
    quit: bool,
}

const TICK: Duration = Duration::from_millis(100);
const WHEEL_LINES: isize = 3;

/// Run the pane, showing `file` first when given and the picker otherwise, and return the
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
    let outcome = App::new(Picker::new(&cwd), inbox, file).run(&mut terminal);
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
    fn new(picker: Picker, inbox: Option<Inbox>, file: Option<PathBuf>) -> Self {
        let mut app = Self { screen: Screen::Picker, picker, inbox, quit: false };
        match file {
            Some(file) => app.screen = Screen::Doc(Doc::open(file)),
            None => app.show_picker(),
        }
        app
    }

    fn run(mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        while !self.quit {
            if let Some(file) = self.inbox.as_ref().and_then(Inbox::take) {
                self.show(file);
            }
            match &mut self.screen {
                Screen::Picker => self.picker.poll(),
                Screen::Doc(doc) => doc.refresh(),
            }
            let size = terminal.size()?;
            terminal.draw(|frame| ui::draw(frame, &mut self))?;
            if event::poll(TICK)? {
                self.on_event(event::read()?, ui::page_height(size.height));
            }
        }
        Ok(())
    }

    /// Show `file`, keeping the place in it when it is already showing.
    fn show(&mut self, file: PathBuf) {
        if !matches!(&self.screen, Screen::Doc(doc) if doc.path == file) {
            self.screen = Screen::Doc(Doc::open(file));
        }
    }

    fn show_picker(&mut self) {
        self.picker.rescan();
        self.screen = Screen::Picker;
    }

    fn on_event(&mut self, event: Event, page: usize) {
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => self.on_key(key, page),
            Event::Mouse(mouse) => {
                let delta = match mouse.kind {
                    MouseEventKind::ScrollDown => WHEEL_LINES,
                    MouseEventKind::ScrollUp => -WHEEL_LINES,
                    _ => return,
                };
                match &mut self.screen {
                    Screen::Picker => self.picker.move_by(delta),
                    Screen::Doc(doc) => doc.scroll_by(delta, page),
                }
            }
            _ => {}
        }
    }

    fn on_key(&mut self, key: KeyEvent, page: usize) {
        if is_quit(&key) {
            self.quit = true;
            return;
        }
        match &mut self.screen {
            Screen::Picker if self.picker.editing_filter => self.on_filter_key(key),
            Screen::Picker => self.on_picker_key(key, page),
            Screen::Doc(doc) => match key.code {
                KeyCode::Char('q') => self.quit = true,
                KeyCode::Esc | KeyCode::Char('o') => self.show_picker(),
                _ => on_doc_key(doc, key, page),
            },
        }
    }

    fn on_picker_key(&mut self, key: KeyEvent, page: usize) {
        let picker = &mut self.picker;
        if let Some(direction) = half_page_direction(&key) {
            picker.move_by(direction * (page / 2).max(1) as isize);
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('j') | KeyCode::Down => picker.move_by(1),
            KeyCode::Char('k') | KeyCode::Up => picker.move_by(-1),
            KeyCode::Char('g') | KeyCode::Home => picker.selected = 0,
            KeyCode::Char('G') | KeyCode::End => picker.move_by(isize::MAX),
            KeyCode::Char('/') => picker.editing_filter = true,
            KeyCode::Char('r') => picker.rescan(),
            KeyCode::Enter => {
                if let Some(file) = picker.selected_file() {
                    self.screen = Screen::Doc(Doc::open(picker.root.join(file)));
                }
            }
            _ => {}
        }
    }

    fn on_filter_key(&mut self, key: KeyEvent) {
        let picker = &mut self.picker;
        match key.code {
            KeyCode::Esc => {
                picker.set_filter(String::new());
                picker.editing_filter = false;
            }
            KeyCode::Enter => picker.editing_filter = false,
            KeyCode::Backspace if picker.filter.is_empty() => picker.editing_filter = false,
            KeyCode::Backspace => {
                let mut filter = picker.filter.clone();
                filter.pop();
                picker.set_filter(filter);
            }
            KeyCode::Down => picker.move_by(1),
            KeyCode::Up => picker.move_by(-1),
            KeyCode::Char(ch) => picker.set_filter(format!("{}{ch}", picker.filter)),
            _ => {}
        }
    }
}

fn on_doc_key(doc: &mut Doc, key: KeyEvent, page: usize) {
    if let Some(direction) = half_page_direction(&key) {
        doc.scroll_by(direction * (page / 2).max(1) as isize, page);
        return;
    }
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => doc.scroll_by(1, page),
        KeyCode::Char('k') | KeyCode::Up => doc.scroll_by(-1, page),
        KeyCode::PageDown | KeyCode::Char(' ') => doc.scroll_by(page as isize, page),
        KeyCode::PageUp => doc.scroll_by(-(page as isize), page),
        KeyCode::Char('g') | KeyCode::Home => doc.scroll = 0,
        KeyCode::Char('G') | KeyCode::End => doc.scroll_to_end(page),
        _ => {}
    }
}
