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
use crate::ui;

pub enum Screen {
    Empty,
    Doc(Doc),
}

pub struct App {
    pub screen: Screen,
    quit: bool,
}

const TICK: Duration = Duration::from_millis(100);
const WHEEL_LINES: isize = 3;

/// Run the pane, showing `file` first when given, and return the process exit code.
pub fn run(file: Option<PathBuf>) -> u8 {
    let mut terminal = ratatui::init();
    let _ = execute!(io::stdout(), EnableMouseCapture);
    let outcome = App::new(file).run(&mut terminal);
    let _ = execute!(io::stdout(), DisableMouseCapture);
    ratatui::restore();
    match outcome {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("herdr-md: {error}");
            1
        }
    }
}

fn is_quit(key: &KeyEvent) -> bool {
    key.code == KeyCode::Char('q')
        || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
}

impl App {
    fn new(file: Option<PathBuf>) -> Self {
        let screen = file.map_or(Screen::Empty, |file| Screen::Doc(Doc::open(file)));
        Self { screen, quit: false }
    }

    fn run(mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        while !self.quit {
            if let Screen::Doc(doc) = &mut self.screen {
                doc.refresh();
            }
            let size = terminal.size()?;
            terminal.draw(|frame| ui::draw(frame, &mut self.screen))?;
            if event::poll(TICK)? {
                self.on_event(event::read()?, ui::page_height(size.height));
            }
        }
        Ok(())
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
                if let Screen::Doc(doc) = &mut self.screen {
                    doc.scroll_by(delta, page);
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
        if let Screen::Doc(doc) = &mut self.screen {
            on_doc_key(doc, key, page);
        }
    }
}

fn on_doc_key(doc: &mut Doc, key: KeyEvent, page: usize) {
    let half = (page / 2).max(1) as isize;
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => doc.scroll_by(1, page),
        KeyCode::Char('k') | KeyCode::Up => doc.scroll_by(-1, page),
        KeyCode::Char('d') if control => doc.scroll_by(half, page),
        KeyCode::Char('u') if control => doc.scroll_by(-half, page),
        KeyCode::PageDown | KeyCode::Char(' ') => doc.scroll_by(page as isize, page),
        KeyCode::PageUp => doc.scroll_by(-(page as isize), page),
        KeyCode::Char('g') | KeyCode::Home => doc.scroll = 0,
        KeyCode::Char('G') | KeyCode::End => doc.scroll_to_end(page),
        _ => {}
    }
}
