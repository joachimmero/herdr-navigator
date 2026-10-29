//! The box a comment is written in: its text, its caret, and how it wraps.

use unicode_width::UnicodeWidthChar;

use crate::comment::Comment;

/// What saving the box writes to.
pub enum Target {
    /// A new comment, settled but for its text.
    New(Comment),
    /// The comment with this id.
    Edit(u64),
}

pub struct Composer {
    pub target: Target,
    pub text: String,
    /// The caret's position in `text`, in chars.
    caret: usize,
}

impl Composer {
    pub fn new(target: Target, text: String) -> Self {
        let caret = text.chars().count();
        Self { target, text, caret }
    }

    pub fn insert(&mut self, ch: char) {
        let at = self.byte(self.caret);
        self.text.insert(at, ch);
        self.caret += 1;
    }

    pub fn backspace(&mut self) {
        if self.caret > 0 {
            self.caret -= 1;
            self.text.remove(self.byte(self.caret));
        }
    }

    pub fn delete(&mut self) {
        if self.caret < self.text.chars().count() {
            self.text.remove(self.byte(self.caret));
        }
    }

    pub fn left(&mut self) {
        self.caret = self.caret.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.caret = (self.caret + 1).min(self.text.chars().count());
    }

    /// Move to the start of the caret's line.
    pub fn home(&mut self) {
        let before: Vec<char> = self.text.chars().take(self.caret).collect();
        self.caret = before.iter().rposition(|&ch| ch == '\n').map_or(0, |at| at + 1);
    }

    /// Move to the end of the caret's line.
    pub fn end(&mut self) {
        let after = self.text.chars().skip(self.caret).position(|ch| ch == '\n');
        self.caret = after.map_or(self.text.chars().count(), |at| self.caret + at);
    }

    /// The text cut into rows `width` columns wide, and the caret's row and column among them.
    pub fn rows(&self, width: usize) -> (Vec<String>, (usize, usize)) {
        let width = width.max(1);
        let mut rows = vec![String::new()];
        let mut used = 0;
        let mut caret = (0, 0);
        for (index, ch) in self.text.chars().chain(std::iter::once('\n')).enumerate() {
            let ch_width = ch.width().unwrap_or(0);
            if ch != '\n' && used + ch_width > width {
                rows.push(String::new());
                used = 0;
            }
            if index == self.caret {
                caret = (rows.len() - 1, used);
            }
            if ch == '\n' {
                rows.push(String::new());
                used = 0;
            } else {
                rows.last_mut().expect("rows start non-empty").push(ch);
                used += ch_width;
            }
        }
        rows.pop();
        (rows, caret)
    }

    fn byte(&self, chars: usize) -> usize {
        self.text.char_indices().nth(chars).map_or(self.text.len(), |(at, _)| at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn composer(text: &str) -> Composer {
        Composer::new(Target::Edit(0), text.to_owned())
    }

    #[test]
    fn typing_lands_at_the_caret() {
        let mut box_ = composer("ac");
        box_.left();
        box_.insert('b');
        box_.home();
        box_.insert('>');
        box_.end();
        box_.backspace();
        box_.home();
        box_.delete();
        assert_eq!(box_.text, "ab");
    }

    #[test]
    fn home_and_end_stay_on_the_caret_s_line() {
        let mut box_ = composer("one\ntwo");
        box_.left();
        box_.home();
        box_.insert('-');
        box_.end();
        box_.insert('!');
        assert_eq!(box_.text, "one\n-two!");
    }

    #[test]
    fn rows_wrap_at_the_width_and_place_the_caret() {
        let box_ = composer("abcde\nf");
        assert_eq!(box_.rows(3), (vec!["abc".into(), "de".into(), "f".into()], (2, 1)));
        let mut full = composer("abc");
        assert_eq!(full.rows(3).1, (0, 3));
        full.insert('\n');
        assert_eq!(full.rows(3), (vec!["abc".into(), String::new()], (1, 0)));
    }
}
