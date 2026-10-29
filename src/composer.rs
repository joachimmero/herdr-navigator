//! The box a comment is written in.

use crate::comment::Comment;
use crate::field::Field;

/// What saving the box writes to.
pub enum Target {
    /// A new comment, settled but for its text.
    New(Comment),
    /// The comment with this id.
    Edit(u64),
}

pub struct Composer {
    pub target: Target,
    pub field: Field,
}

impl Composer {
    pub fn new(target: Target, text: String) -> Self {
        Self { target, field: Field::new(text) }
    }
}
