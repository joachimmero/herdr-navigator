//! The places jumped away from, to walk back and forth through like vim's jumplist.

use std::path::PathBuf;

/// A file and a source line in it.
pub type Place = (PathBuf, usize);

const KEPT: usize = 100;

#[derive(Default)]
pub struct Jumps {
    places: Vec<Place>,
    /// The place being visited in `places`, or `places.len()` when at none of them.
    index: usize,
}

impl Jumps {
    /// Remember `here` before jumping away from it, forgetting the places ahead of the one being
    /// visited.
    pub fn record(&mut self, here: Place) {
        self.places.truncate(self.index);
        if self.places.last() != Some(&here) {
            self.places.push(here);
        }
        if self.places.len() > KEPT {
            self.places.remove(0);
        }
        self.index = self.places.len();
    }

    /// The place before the one being visited, remembering `here` first when visiting none, so
    /// `forward` can come back to it.
    pub fn back(&mut self, here: Place) -> Option<Place> {
        if self.index == self.places.len() {
            self.record(here);
            self.index = self.places.len() - 1;
        }
        self.index = self.index.checked_sub(1)?;
        Some(self.places[self.index].clone())
    }

    /// The place after the one being visited.
    pub fn forward(&mut self) -> Option<Place> {
        let next = self.places.get(self.index + 1)?.clone();
        self.index += 1;
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(line: usize) -> Place {
        ("a.rs".into(), line)
    }

    #[test]
    fn back_and_forward_walk_the_places_jumped_from_and_return_to_the_latest() {
        let mut jumps = Jumps::default();
        jumps.record(at(1));
        jumps.record(at(2));
        assert_eq!(jumps.back(at(3)), Some(at(2)));
        assert_eq!(jumps.back(at(2)), Some(at(1)));
        assert_eq!(jumps.back(at(1)), None);
        assert_eq!(jumps.forward(), Some(at(2)));
        assert_eq!(jumps.forward(), Some(at(3)));
        assert_eq!(jumps.forward(), None);
    }

    #[test]
    fn a_jump_from_partway_back_forgets_the_places_ahead() {
        let mut jumps = Jumps::default();
        jumps.record(at(1));
        jumps.record(at(2));
        assert_eq!(jumps.back(at(3)), Some(at(2)));
        assert_eq!(jumps.back(at(2)), Some(at(1)));
        jumps.record(at(1));
        assert_eq!(jumps.forward(), None);
        assert_eq!(jumps.back(at(9)), Some(at(1)));
    }
}
