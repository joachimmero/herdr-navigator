//! The project's files as a tree, found under the project the pane sits in.

use std::cell::Cell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::thread;

use ignore::WalkBuilder;

use crate::watch::Watch;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    /// Relative to the tree's root.
    pub path: PathBuf,
    pub name: String,
    pub dir: bool,
    pub ignored: bool,
}

/// One visible row: a node, and how deep it sits.
pub struct Row<'a> {
    pub node: &'a Node,
    pub depth: usize,
}

/// Each directory's children, keyed by its path relative to the root (`""` for the root itself),
/// directories first and then by name.
type Children = HashMap<PathBuf, Vec<Node>>;

pub struct Tree {
    pub root: PathBuf,
    children: Children,
    scan: Option<Receiver<Children>>,
    watch: Option<Watch>,
    /// Something it lists changed since the scan in progress began.
    stale: bool,
    expanded: BTreeSet<PathBuf>,
    pub show_ignored: bool,
    /// The selected node's path: a rescan keeps the selection on it while it exists.
    selected: Option<PathBuf>,
    /// The list's first row on screen at the last draw, so the view moves only when the
    /// selection would leave it.
    pub offset: Cell<usize>,
}

impl Tree {
    /// A tree for the project at or above `dir`.
    pub fn new(dir: &Path) -> Self {
        Self {
            root: find_root(dir),
            children: Children::new(),
            scan: None,
            watch: None,
            stale: false,
            expanded: BTreeSet::new(),
            show_ignored: false,
            selected: None,
            offset: Cell::new(0),
        }
    }

    /// Look for files again in the background.
    pub fn rescan(&mut self) {
        if self.scan.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let root = self.root.clone();
        thread::spawn(move || sender.send(discover(&root)));
        self.scan = Some(receiver);
    }

    /// Look for files again whenever something changes on disk in a folder the tree lists.
    pub fn watch(&mut self) {
        self.watch = Watch::start(&self.root);
    }

    /// Take a finished scan's files, keeping the selection where it still exists and the expanded
    /// ignored folders filled in, and start another when what the tree lists changed meanwhile.
    pub fn poll(&mut self) {
        let listed = |path: &Path| self.children.contains_key(path.parent().unwrap_or(path));
        self.stale |= self.watch.as_ref().is_some_and(|watch| watch.changed(listed));
        if let Some(Ok(children)) = self.scan.as_ref().map(Receiver::try_recv) {
            self.scan = None;
            self.children = children;
            let ignored: Vec<PathBuf> = self.expanded.iter().cloned().collect();
            ignored.iter().for_each(|dir| self.fill_ignored(dir));
            let rows = self.rows();
            let kept = self
                .selected
                .as_ref()
                .is_some_and(|path| rows.iter().any(|r| r.node.path == *path));
            if !kept {
                self.selected = rows.first().map(|row| row.node.path.clone());
            }
        }
        if self.stale && !self.is_scanning() {
            self.stale = false;
            self.rescan();
        }
    }

    pub fn is_scanning(&self) -> bool {
        self.scan.is_some()
    }

    /// The rows on show: the root's children, and the children of every expanded directory.
    pub fn rows(&self) -> Vec<Row<'_>> {
        let mut rows = Vec::new();
        self.push_rows(Path::new(""), 0, &mut rows);
        rows
    }

    fn push_rows<'a>(&'a self, dir: &Path, depth: usize, rows: &mut Vec<Row<'a>>) {
        let shown = self.children.get(dir).into_iter().flatten();
        for node in shown.filter(|node| self.show_ignored || !node.ignored) {
            rows.push(Row { node, depth });
            if node.dir && self.expanded.contains(&node.path) {
                self.push_rows(&node.path, depth + 1, rows);
            }
        }
    }

    pub fn is_expanded(&self, dir: &Path) -> bool {
        self.expanded.contains(dir)
    }

    pub fn selected(&self) -> Option<&Node> {
        let path = self.selected.as_ref()?;
        self.rows().into_iter().find(|row| row.node.path == *path).map(|row| row.node)
    }

    pub fn selected_index(&self) -> usize {
        let rows = self.rows();
        let path = self.selected.as_ref();
        rows.iter().position(|row| Some(&row.node.path) == path).unwrap_or(0)
    }

    pub fn move_by(&mut self, delta: isize) {
        let rows = self.rows();
        let last = rows.len().saturating_sub(1);
        let index = self.selected_index().saturating_add_signed(delta).min(last);
        self.selected = rows.get(index).map(|row| row.node.path.clone());
    }

    pub fn select_index(&mut self, index: usize) {
        self.selected = self.rows().get(index).map(|row| row.node.path.clone());
    }

    /// Open the selected directory, or close it when it is open.
    pub fn toggle(&mut self) {
        let Some(node) = self.selected().filter(|node| node.dir) else { return };
        let path = node.path.clone();
        if !self.expanded.remove(&path) {
            self.fill_ignored(&path);
            self.expanded.insert(path);
        }
    }

    /// Close the selected directory, or step out to the one holding the selection.
    pub fn collapse(&mut self) {
        let Some(path) = self.selected.clone() else { return };
        if !self.expanded.remove(&path) {
            let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty());
            if let Some(parent) = parent {
                self.expanded.remove(parent);
                self.selected = Some(parent.to_path_buf());
            }
        }
    }

    /// Select `file`, given as an absolute path, opening the directories above it. A file
    /// outside the root leaves the selection where it was.
    pub fn reveal(&mut self, file: &Path) {
        let Ok(relative) = file.strip_prefix(&self.root) else { return };
        let ancestors: Vec<PathBuf> = relative
            .ancestors()
            .skip(1)
            .filter(|dir| !dir.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .collect();
        ancestors.iter().for_each(|dir| self.fill_ignored(dir));
        self.expanded.extend(ancestors);
        self.selected = Some(relative.to_path_buf());
    }

    pub fn toggle_ignored(&mut self) {
        self.show_ignored = !self.show_ignored;
        let rows = self.rows();
        if !self.selected.as_ref().is_some_and(|path| rows.iter().any(|r| r.node.path == *path)) {
            self.selected = rows.first().map(|row| row.node.path.clone());
        }
    }

    /// List ignored directory `dir`'s contents, which a scan never walks into.
    fn fill_ignored(&mut self, dir: &Path) {
        let is_ignored_dir =
            |nodes: &[Node]| nodes.iter().any(|node| node.path == dir && node.ignored);
        let parent = dir.parent().unwrap_or(Path::new(""));
        let ignored = self.children.get(parent).is_some_and(|nodes| is_ignored_dir(nodes));
        if !ignored || self.children.contains_key(dir) {
            return;
        }
        let nodes = list_dir(&self.root, dir, &HashSet::new());
        self.children.insert(dir.to_path_buf(), nodes);
    }
}

/// The git work tree holding `dir`, or `dir` itself outside one.
fn find_root(dir: &Path) -> PathBuf {
    dir.ancestors().find(|ancestor| ancestor.join(".git").exists()).unwrap_or(dir).to_path_buf()
}

/// Everything under `root` that git doesn't ignore, plus the ignored entries beside it, which are
/// listed but not walked into.
fn discover(root: &Path) -> Children {
    let walk = WalkBuilder::new(root)
        .hidden(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build();
    let kept: Vec<PathBuf> = walk
        .filter_map(Result::ok)
        .filter_map(|entry| entry.path().strip_prefix(root).ok().map(Path::to_path_buf))
        .filter(|path| !path.as_os_str().is_empty())
        .collect();
    let kept_set: HashSet<PathBuf> = kept.iter().cloned().collect();
    let dirs = std::iter::once(PathBuf::new())
        .chain(kept.iter().filter(|path| root.join(path).is_dir()).cloned());
    dirs.map(|dir| {
        let nodes = list_dir(root, &dir, &kept_set);
        (dir, nodes)
    })
    .collect()
}

/// Directory `dir`'s entries, those not in `kept` marked ignored, sorted directories first.
fn list_dir(root: &Path, dir: &Path, kept: &HashSet<PathBuf>) -> Vec<Node> {
    let entries = fs::read_dir(root.join(dir)).into_iter().flatten().filter_map(Result::ok);
    let mut nodes: Vec<Node> = entries
        .filter(|entry| entry.file_name() != ".git")
        .map(|entry| {
            let path = dir.join(entry.file_name());
            Node {
                name: entry.file_name().to_string_lossy().into_owned(),
                // Following links, so a linked folder opens like any other.
                dir: entry.path().is_dir(),
                ignored: !kept.contains(&path),
                path,
            }
        })
        .collect();
    nodes.sort_by_key(|node| (!node.dir, node.name.to_lowercase()));
    nodes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(name: &str, files: &[&str]) -> PathBuf {
        let root = std::env::temp_dir()
            .join(format!("herdr-navigator-tree-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for file in files {
            let path = root.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "").unwrap();
        }
        fs::create_dir_all(root.join(".git")).unwrap();
        root
    }

    fn scanned(root: &Path) -> Tree {
        let mut tree = Tree::new(root);
        tree.children = discover(root);
        tree.selected = tree.rows().first().map(|row| row.node.path.clone());
        tree
    }

    fn shown(tree: &Tree) -> Vec<String> {
        tree.rows()
            .iter()
            .map(|row| format!("{}{}", "  ".repeat(row.depth), row.node.name))
            .collect()
    }

    #[test]
    fn lists_directories_first_and_hides_what_git_ignores_until_asked() {
        let root = project("ignored", &[".gitignore", "b.rs", "A.md", "src/main.rs", "target/out"]);
        fs::write(root.join(".gitignore"), "/target\n").unwrap();
        let mut tree = scanned(&root);
        assert_eq!(tree.root, root);
        assert_eq!(shown(&tree), ["src", ".gitignore", "A.md", "b.rs"]);

        tree.toggle_ignored();
        assert_eq!(shown(&tree), ["src", "target", ".gitignore", "A.md", "b.rs"]);
        assert!(tree.rows()[1].node.ignored);
    }

    #[test]
    fn an_ignored_folder_lists_its_contents_only_when_opened() {
        let root = project("lazy", &[".gitignore", "target/debug/out"]);
        fs::write(root.join(".gitignore"), "/target\n").unwrap();
        let mut tree = scanned(&root);
        assert!(!tree.children.contains_key(Path::new("target")));
        tree.toggle_ignored();
        tree.select_index(0);
        tree.toggle();
        assert_eq!(shown(&tree), ["target", "  debug", ".gitignore"]);
        assert!(tree.rows()[1].node.ignored);
    }

    #[test]
    fn revealing_a_file_opens_its_folders_and_selects_it() {
        let root = project("reveal", &["a/b/c.rs", "z.rs"]);
        let mut tree = scanned(&root);
        assert_eq!(shown(&tree), ["a", "z.rs"]);
        tree.reveal(&root.join("a/b/c.rs"));
        assert_eq!(shown(&tree), ["a", "  b", "    c.rs", "z.rs"]);
        assert_eq!(tree.selected().unwrap().path, Path::new("a/b/c.rs"));

        tree.collapse();
        assert_eq!(tree.selected().unwrap().path, Path::new("a/b"));
        assert_eq!(shown(&tree), ["a", "  b", "z.rs"]);
    }

    #[test]
    fn a_rescan_keeps_the_selection_on_the_same_path() {
        let root = project("rescan", &["b.rs", "c.rs"]);
        let mut tree = scanned(&root);
        tree.move_by(1);
        fs::write(root.join("a.rs"), "").unwrap();
        tree.rescan();
        while tree.is_scanning() {
            tree.poll();
        }
        assert_eq!(tree.selected().unwrap().path, Path::new("c.rs"));
        assert_eq!(tree.selected_index(), 2);
    }

    #[test]
    fn outside_git_the_directory_itself_is_the_root() {
        let root =
            std::env::temp_dir().join(format!("herdr-navigator-plain-{}", std::process::id()));
        fs::create_dir_all(root.join("sub")).unwrap();
        assert_eq!(find_root(&root.join("sub")), root.join("sub"));
    }
}
