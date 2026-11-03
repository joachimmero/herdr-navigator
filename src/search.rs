//! Searching the project's files by path and their lines by content: the `fff-search` engine on a
//! worker thread, and the search screen's state.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use fff_search::{
    FFFQuery, FilePicker, FilePickerOptions, FileSearchConfig, FrecencyTracker, FuzzySearchOptions,
    GrepSearchOptions, PaginationArgs, SharedFilePicker, SharedFrecency,
};
use ratatui_image::picker::Picker;

use crate::doc::Doc;
use crate::field::Field;
use crate::herdr;
use crate::layout::Lines;

/// The most results one query fetches per kind; the rest show as `… more`.
const FILE_LIMIT: usize = 50;
const CODE_LIMIT: usize = 200;
/// One grep's time budget; past it, partial results.
const GREP_BUDGET_MS: u64 = 80;
/// How often a cold worker checks again whether its first scan finished.
const WARMUP_POLL: Duration = Duration::from_millis(50);

enum Job {
    /// Run `query`; the completion echoes the generation back.
    Query { generation: u64, query: String },
    /// Record an opened result, so ranking improves with use.
    Track { path: String },
}

/// A path match, in engine order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileHit {
    pub path: String,
    /// Byte spans into `path` the engine matched.
    pub spans: Vec<(u32, u32)>,
}

/// A content match, in engine order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeHit {
    pub path: String,
    /// 1-based.
    pub line: usize,
    /// The line, its indentation dropped.
    pub text: String,
    /// Byte spans into `text` the engine matched.
    pub spans: Vec<(u32, u32)>,
}

/// One query's results; `code_more` marks a grep the cap or the time budget cut short.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Results {
    pub files: Vec<FileHit>,
    pub code: Vec<CodeHit>,
    pub file_total: usize,
    pub code_more: bool,
}

pub enum Outcome {
    Ready(Results),
    /// The first scan is still running; the query runs once it's done.
    Indexing,
    Failed(String),
}

struct Completion {
    generation: u64,
    outcome: Outcome,
}

/// The engine, started once on the worker thread.
struct Engine {
    shared: SharedFilePicker,
    frecency: SharedFrecency,
    root: PathBuf,
}

impl Engine {
    /// Start scanning, indexing and watching `root`, keeping the frecency store under `cache`.
    fn start(root: PathBuf, cache: &Path) -> Result<Self, String> {
        let shared = SharedFilePicker::default();
        let frecency = SharedFrecency::default();
        // Search works without frecency; ranking just stops improving with use.
        if std::fs::create_dir_all(cache).is_ok()
            && let Ok(tracker) = FrecencyTracker::open(cache.join("frecency"))
        {
            let _ = frecency.init(tracker);
        }
        FilePicker::new_with_shared_state(
            shared.clone(),
            frecency.clone(),
            FilePickerOptions {
                base_path: root.to_string_lossy().into_owned(),
                enable_content_indexing: true,
                watch: true,
                ..Default::default()
            },
        )
        .map_err(|error| error.to_string())?;
        Ok(Self { shared, frecency, root })
    }

    fn warm(&self) -> bool {
        self.shared.wait_for_scan(Duration::from_millis(1))
    }

    /// Run one query against the warm index: the paths, then the contents.
    fn run(&self, raw: &str) -> Result<Results, String> {
        let guard = self.shared.read().map_err(|error| error.to_string())?;
        let picker = guard.as_ref().ok_or("search engine not ready")?;
        let query = FFFQuery::parse(raw, FileSearchConfig);

        let found = picker.fuzzy_search(
            &query,
            None,
            FuzzySearchOptions {
                pagination: PaginationArgs { offset: 0, limit: FILE_LIMIT },
                ..Default::default()
            },
        );
        let files: Vec<FileHit> = found
            .items
            .iter()
            .zip(&found.match_byte_offsets)
            .map(|(item, spans)| FileHit {
                path: item.relative_path(picker),
                spans: spans.iter().copied().collect(),
            })
            .collect();
        let file_total = found.total_matched.max(files.len());

        // An empty query lists the files by frecency alone; an empty grep would match everything.
        if raw.trim().is_empty() {
            return Ok(Results { files, code: Vec::new(), file_total, code_more: false });
        }

        let mut grep = picker.grep(
            &query,
            &GrepSearchOptions {
                page_limit: CODE_LIMIT,
                time_budget_ms: GREP_BUDGET_MS,
                ..Default::default()
            },
        );
        for found in &mut grep.matches {
            found.trim_leading_whitespace();
        }
        let code: Vec<CodeHit> = grep
            .matches
            .iter()
            .map(|found| CodeHit {
                path: grep.files[found.file_index].relative_path(picker),
                line: usize::try_from(found.line_number).unwrap_or(usize::MAX),
                text: found.line_content.clone(),
                spans: found.match_byte_offsets.iter().copied().collect(),
            })
            .collect();
        let code_more = grep.next_file_offset != 0;

        Ok(Results { files, code, file_total, code_more })
    }

    /// Record an opened result in the frecency store, if there is one.
    fn track(&self, path: &str) {
        if let Ok(guard) = self.frecency.read()
            && let Some(tracker) = guard.as_ref()
        {
            let _ = tracker.track_access(&self.root.join(path));
        }
    }
}

/// Answer `jobs` until the channel closes, a burst of queries answered by its last.
fn work(root: PathBuf, cache: &Path, jobs: Receiver<Job>, done: Sender<Completion>) {
    let engine = match Engine::start(root, cache) {
        Ok(engine) => engine,
        Err(error) => {
            // Every later query would fail alike, so say so once and stop.
            if let Ok(Job::Query { generation, .. }) = jobs.recv() {
                let _ = done.send(Completion { generation, outcome: Outcome::Failed(error) });
            }
            return;
        }
    };
    // The query waiting for the first scan to finish.
    let mut parked: Option<(u64, String)> = None;
    loop {
        let job = if parked.is_some() {
            match jobs.recv_timeout(WARMUP_POLL) {
                Ok(job) => Some(job),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match jobs.recv() {
                Ok(job) => Some(job),
                Err(_) => break,
            }
        };
        let mut latest = None;
        for job in job.into_iter().chain(std::iter::from_fn(|| jobs.try_recv().ok())) {
            match job {
                Job::Query { generation, query } => latest = Some((generation, query)),
                Job::Track { path } => engine.track(&path),
            }
        }
        let fresh = latest.is_some();
        let Some((generation, query)) = latest.or_else(|| parked.take()) else { continue };
        if !engine.warm() {
            parked = Some((generation, query));
            // Said once per query: a parked one checks again quietly.
            if fresh {
                let _ = done.send(Completion { generation, outcome: Outcome::Indexing });
            }
            continue;
        }
        parked = None;
        let outcome = match engine.run(&query) {
            Ok(results) => Outcome::Ready(results),
            Err(error) => Outcome::Failed(error),
        };
        if done.send(Completion { generation, outcome }).is_err() {
            break;
        }
    }
}

/// The handle on the search worker.
pub struct Searcher {
    jobs: Sender<Job>,
    completions: Receiver<Completion>,
    /// The latest query's tag; completions for older ones are dropped.
    generation: u64,
}

impl Searcher {
    /// Start a worker searching `root`.
    pub fn start(root: PathBuf) -> Self {
        let cache = herdr::state_dir()
            .map(|dir| dir.join("search"))
            .unwrap_or_else(|_| std::env::temp_dir().join("herdr-navigator"));
        let (jobs, inbox) = mpsc::channel();
        let (done, completions) = mpsc::channel();
        // A worker that didn't start drops its end, and the first query reports it.
        let _ = std::thread::Builder::new()
            .name("search".into())
            .spawn(move || work(root, &cache, inbox, done));
        Self { jobs, completions, generation: 0 }
    }

    /// Ask for `query`'s results, saying whether the worker is there to take it.
    pub fn query(&mut self, query: String) -> bool {
        self.generation += 1;
        self.jobs.send(Job::Query { generation: self.generation, query }).is_ok()
    }

    /// The outcome for the latest query, if one came in since last asked.
    pub fn outcome(&self) -> Option<Outcome> {
        let latest = self.completions.try_iter().filter(|done| done.generation == self.generation);
        latest.last().map(|done| done.outcome)
    }

    /// Record that `path`, relative to the root, was opened from the results.
    pub fn track(&self, path: String) {
        let _ = self.jobs.send(Job::Track { path });
    }
}

/// Which results the list shows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Files,
    Code,
}

#[derive(PartialEq, Eq)]
pub enum Phase {
    Indexing,
    /// Results are showing; they stay up while a newer query runs.
    Ready,
    Failed(String),
}

/// One row of the results list; the pick indexes only `File` and `Code` rows.
pub enum ResultRow {
    /// The file a run of code hits is in, by the first hit's index.
    Header(usize),
    File(usize),
    Code(usize),
    /// Results cut off by the engine's cap.
    More,
}

impl ResultRow {
    pub fn pick(&self) -> Option<usize> {
        match self {
            Self::File(index) | Self::Code(index) => Some(*index),
            Self::Header(_) | Self::More => None,
        }
    }
}

/// The search screen.
pub struct Search {
    pub field: Field,
    pub kind: Kind,
    /// The picked result, an index into the shown kind's results.
    pub pick: usize,
    /// The first row on screen, kept by drawing so the pick stays in view.
    pub scroll: Cell<usize>,
    pub results: Results,
    pub phase: Phase,
    /// Whether the query changed since it was last asked for.
    pub dirty: bool,
    /// The picked result's file.
    pub preview: Option<Doc>,
    /// The pick the preview shows, as `target` gives it.
    previewed: Option<(String, Option<usize>)>,
}

impl Search {
    pub fn new() -> Self {
        Self {
            field: Field::new(String::new()),
            kind: Kind::Files,
            pick: 0,
            scroll: Cell::new(0),
            results: Results::default(),
            phase: Phase::Indexing,
            dirty: true,
            preview: None,
            previewed: None,
        }
    }

    pub fn rows(&self) -> Vec<ResultRow> {
        match self.kind {
            Kind::Files => {
                let more = self.results.file_total > self.results.files.len();
                let files = (0..self.results.files.len()).map(ResultRow::File);
                files.chain(more.then_some(ResultRow::More)).collect()
            }
            Kind::Code => {
                let code = &self.results.code;
                let hits = code.iter().enumerate().flat_map(|(index, hit)| {
                    let first_in_file = index == 0 || code[index - 1].path != hit.path;
                    let header = first_in_file.then_some(ResultRow::Header(index));
                    header.into_iter().chain([ResultRow::Code(index)])
                });
                hits.chain(self.results.code_more.then_some(ResultRow::More)).collect()
            }
        }
    }

    fn picks(&self) -> usize {
        match self.kind {
            Kind::Files => self.results.files.len(),
            Kind::Code => self.results.code.len(),
        }
    }

    /// Show the other kind of results, picking the first.
    pub fn flip(&mut self) {
        self.kind = match self.kind {
            Kind::Files => Kind::Code,
            Kind::Code => Kind::Files,
        };
        self.pick = 0;
        self.scroll.set(0);
    }

    pub fn move_pick(&mut self, delta: isize) {
        if self.phase == Phase::Ready {
            let last = self.picks().saturating_sub(1);
            self.pick = self.pick.saturating_add_signed(delta).min(last);
        }
    }

    /// Pick the result on list row `row`, saying whether it was picked already.
    pub fn pick_row(&mut self, row: usize) -> bool {
        let rows = (self.phase == Phase::Ready).then(|| self.rows());
        let Some(pick) = rows.and_then(|rows| rows.get(row).and_then(ResultRow::pick)) else {
            return false;
        };
        let again = pick == self.pick;
        self.pick = pick;
        again
    }

    pub fn land(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Ready(results) => {
                self.results = results;
                self.phase = Phase::Ready;
                self.pick = 0;
                self.scroll.set(0);
            }
            Outcome::Indexing => self.phase = Phase::Indexing,
            Outcome::Failed(error) => self.phase = Phase::Failed(error),
        }
    }

    /// The picked result's path, relative to the root, and its line for a code hit.
    pub fn target(&self) -> Option<(&str, Option<usize>)> {
        if self.phase != Phase::Ready {
            return None;
        }
        match self.kind {
            Kind::Files => self.results.files.get(self.pick).map(|hit| (&*hit.path, None)),
            Kind::Code => self.results.code.get(self.pick).map(|hit| (&*hit.path, Some(hit.line))),
        }
    }

    /// Show the picked result in the preview, its line in the middle for a code hit.
    pub fn follow_pick(&mut self, root: &Path, picker: &Picker) {
        let target = self.target().map(|(path, line)| (path.to_owned(), line));
        if target == self.previewed {
            return;
        }
        self.preview = target.as_ref().map(|(path, line)| {
            let file = root.join(path);
            let Some(line) = *line else { return Doc::open(file, picker) };
            // Another hit in the same file keeps the file as it was read.
            let mut doc = match self.preview.take() {
                Some(doc) if doc.path == file => doc,
                _ => Doc::open(file, picker),
            };
            doc.point(Lines::one(line));
            doc.center();
            doc
        });
        self.previewed = target;
    }

    /// Scroll the preview by `delta` rows of a page `height` rows high.
    pub fn scroll_preview(&mut self, delta: isize, height: usize) {
        if let Some(doc) = &mut self.preview {
            doc.scroll_by(delta, height);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(path: &str, line: usize) -> CodeHit {
        CodeHit { path: path.into(), line, text: String::new(), spans: Vec::new() }
    }

    fn ready(results: Results) -> Search {
        let mut search = Search::new();
        search.land(Outcome::Ready(results));
        search
    }

    #[test]
    fn code_hits_sit_under_a_header_for_each_file() {
        let code = vec![hit("a.rs", 1), hit("a.rs", 9), hit("b.rs", 2)];
        let mut search = ready(Results { code, code_more: true, ..Results::default() });
        search.flip();
        let rows: Vec<String> = search
            .rows()
            .iter()
            .map(|row| match row {
                ResultRow::Header(index) => format!("h{index}"),
                ResultRow::File(index) => format!("f{index}"),
                ResultRow::Code(index) => format!("c{index}"),
                ResultRow::More => "more".into(),
            })
            .collect();
        assert_eq!(rows, ["h0", "c0", "c1", "h2", "c2", "more"]);
    }

    #[test]
    fn clicking_a_header_picks_nothing_and_a_hit_twice_opens_it() {
        let code = vec![hit("a.rs", 1), hit("a.rs", 9)];
        let mut search = ready(Results { code, ..Results::default() });
        search.flip();
        assert!(!search.pick_row(0));
        assert!(!search.pick_row(2));
        assert_eq!(search.target(), Some(("a.rs", Some(9))));
        assert!(search.pick_row(2));
    }

    #[test]
    fn nothing_is_picked_until_results_are_in() {
        let mut search = Search::new();
        search.results.files = vec![FileHit { path: "a.rs".into(), spans: Vec::new() }];
        search.move_pick(1);
        assert_eq!(search.target(), None);
        search.land(Outcome::Indexing);
        assert!(!search.pick_row(0));
    }
}
