//! What changes on disk under a folder.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

pub struct Watch {
    /// Canonical, as the events name their paths.
    root: PathBuf,
    events: Receiver<notify::Result<Event>>,
    _watcher: RecommendedWatcher,
}

impl Watch {
    /// Watch everything under `root`, or nothing when the system refuses.
    pub fn start(root: &Path) -> Option<Self> {
        let root = root.canonicalize().ok()?;
        let (sender, events) = mpsc::channel();
        let mut watcher =
            notify::recommended_watcher(move |event| drop(sender.send(event))).ok()?;
        watcher.watch(&root, RecursiveMode::Recursive).ok()?;
        Some(Self { root, events, _watcher: watcher })
    }

    /// Whether anything changed since the last call at a path, relative to the root, that
    /// `matters`. Losing track of what changed counts as a change that matters.
    pub fn changed(&self, matters: impl Fn(&Path) -> bool) -> bool {
        let events: Vec<_> = self.events.try_iter().collect();
        events.iter().any(|event| match event {
            Ok(event) if event.need_rescan() => true,
            Ok(Event { kind: EventKind::Access(_), .. }) => false,
            Ok(event) => event
                .paths
                .iter()
                .filter_map(|path| path.strip_prefix(&self.root).ok())
                .any(&matters),
            Err(_) => true,
        })
    }
}
