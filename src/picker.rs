//! The list of markdown files the pane can open, found under the project it sits in.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::thread;

use ignore::WalkBuilder;

use crate::doc::is_markdown;

enum Scan {
    Idle,
    Running(Receiver<Vec<PathBuf>>),
    Done(Vec<PathBuf>),
}

pub struct Picker {
    pub root: PathBuf,
    scan: Scan,
    pub filter: String,
    pub editing_filter: bool,
    /// An index into `matches()`.
    pub selected: usize,
    /// The list's first row on screen at the last draw, so the view moves only when the
    /// selection would leave it.
    pub offset: Cell<usize>,
}

impl Picker {
    /// A picker for the project at or above `dir`.
    pub fn new(dir: &Path) -> Self {
        Self {
            root: find_root(dir),
            scan: Scan::Idle,
            filter: String::new(),
            editing_filter: false,
            selected: 0,
            offset: Cell::new(0),
        }
    }

    /// Look for files again in the background, keeping the selection on the same file.
    pub fn rescan(&mut self) {
        if matches!(self.scan, Scan::Running(_)) {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let root = self.root.clone();
        thread::spawn(move || sender.send(discover(&root)));
        self.scan = Scan::Running(receiver);
    }

    /// Take a finished scan's files.
    pub fn poll(&mut self) {
        let Scan::Running(receiver) = &self.scan else { return };
        let Ok(files) = receiver.try_recv() else { return };
        let selected = self.selected_file().map(Path::to_path_buf);
        self.scan = Scan::Done(files);
        self.selected = selected
            .and_then(|selected| self.matches().iter().position(|file| *file == selected))
            .unwrap_or(0);
    }

    pub fn is_scanning(&self) -> bool {
        matches!(self.scan, Scan::Running(_))
    }

    /// The found files whose path holds the filter, relative to `root`.
    pub fn matches(&self) -> Vec<&Path> {
        let Scan::Done(files) = &self.scan else { return Vec::new() };
        let filter = self.filter.to_lowercase();
        files
            .iter()
            .map(PathBuf::as_path)
            .filter(|file| file.to_string_lossy().to_lowercase().contains(&filter))
            .collect()
    }

    pub fn selected_file(&self) -> Option<&Path> {
        self.matches().get(self.selected).copied()
    }

    pub fn move_by(&mut self, delta: isize) {
        let last = self.matches().len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(delta).min(last);
    }

    pub fn set_filter(&mut self, filter: String) {
        self.filter = filter;
        self.selected = 0;
    }
}

/// The git work tree holding `dir`, or `dir` itself outside one.
fn find_root(dir: &Path) -> PathBuf {
    dir.ancestors().find(|ancestor| ancestor.join(".git").exists()).unwrap_or(dir).to_path_buf()
}

/// The markdown files under `root` that git doesn't ignore, relative to it and sorted by path.
fn discover(root: &Path) -> Vec<PathBuf> {
    let walk = WalkBuilder::new(root)
        .hidden(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build();
    let mut files: Vec<PathBuf> = walk
        .filter_map(Result::ok)
        // Following links, so a file linked into the tree is found like any other.
        .filter(|entry| entry.path().is_file())
        .filter(|entry| is_markdown(entry.path()))
        .filter_map(|entry| entry.path().strip_prefix(root).ok().map(Path::to_path_buf))
        .collect();
    files.sort_by_key(|file| file.to_string_lossy().to_lowercase());
    files
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn tree(name: &str, files: &[&str]) -> PathBuf {
        let root = std::env::temp_dir()
            .join(format!("herdr-navigator-picker-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for file in files {
            let path = root.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "").unwrap();
        }
        root
    }

    #[test]
    fn finds_markdown_under_the_git_root_and_skips_what_git_ignores() {
        let root = tree(
            "git",
            &[
                ".gitignore",
                "README.md",
                "docs/Design.markdown",
                "src/main.rs",
                "target/notes.md",
                "docs/real.md",
                ".github/PULL_REQUEST_TEMPLATE.md",
            ],
        );
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".gitignore"), "/target\n").unwrap();
        std::os::unix::fs::symlink(root.join("docs/real.md"), root.join("LINKED.md")).unwrap();

        let picker = Picker::new(&root.join("src"));
        assert_eq!(picker.root, root);
        let found: Vec<String> =
            discover(&root).iter().map(|file| file.display().to_string()).collect();
        assert_eq!(
            found,
            [
                ".github/PULL_REQUEST_TEMPLATE.md",
                "docs/Design.markdown",
                "docs/real.md",
                "LINKED.md",
                "README.md"
            ]
        );
    }

    #[test]
    fn outside_git_the_directory_itself_is_searched() {
        let root = tree("plain", &["a.md", "sub/b.md"]);
        assert_eq!(find_root(&root.join("sub")), root.join("sub"));
    }

    #[test]
    fn the_filter_matches_any_part_of_the_path_ignoring_case() {
        let root = tree("filter", &["README.md", "docs/plan.md", "docs/Design.md"]);
        let mut picker = Picker::new(&root);
        picker.scan = Scan::Done(discover(&root));
        picker.set_filter("DOCS/d".into());
        assert_eq!(picker.matches(), [Path::new("docs/Design.md")]);
    }
}
