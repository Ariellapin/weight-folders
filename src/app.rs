//! Application state machine and per-frame update loop.

use crate::model::{fmt_size, fmt_time, now_unix, NodeId, Tree};
use crate::scanner::{self, Progress, ScanError};
use crate::snapshot::{self, DiffSummary, RecentEntry, Snapshot};
use crate::ui;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub enum ScanMsg {
    Done(Tree),
    Failed(String),
    Cancelled,
}

/// A running scan on a worker thread.
pub struct ScanJob {
    pub root: PathBuf,
    pub progress: Arc<Progress>,
    pub cancel: Arc<AtomicBool>,
    pub rx: Receiver<ScanMsg>,
    pub started: Instant,
}

impl ScanJob {
    pub fn spawn(root: PathBuf) -> Self {
        let progress = Arc::new(Progress::default());
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        {
            let root = root.clone();
            let progress = progress.clone();
            let cancel = cancel.clone();
            std::thread::Builder::new()
                .name("scanner".into())
                .stack_size(64 << 20)
                .spawn(move || {
                    let msg = match scanner::scan(&root, progress, cancel) {
                        Ok(t) => ScanMsg::Done(t),
                        Err(ScanError::Cancelled) => ScanMsg::Cancelled,
                        Err(ScanError::Io(e)) => ScanMsg::Failed(e),
                    };
                    let _ = tx.send(msg);
                })
                .expect("spawn scanner thread");
        }
        Self {
            root,
            progress,
            cancel,
            rx,
            started: Instant::now(),
        }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn poll(&self) -> Option<ScanMsg> {
        self.rx.try_recv().ok()
    }
}

impl Drop for ScanJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub enum Validation {
    NotStarted,
    Running(ScanJob),
    Done { summary: DiffSummary, at: i64 },
    Failed(String),
}

/// Everything the "Ready" screen works on.
pub struct Session {
    pub root: PathBuf,
    pub tree: Tree,
    /// Bumped whenever `tree` is replaced or mutated; invalidates caches.
    pub generation: u64,
    /// Directory currently shown in the treemap.
    pub view: NodeId,
    pub selected: Option<NodeId>,
    pub search: ui::search::SearchState,
    pub validation: Validation,
    pub pending_delete: Option<NodeId>,
    pub deleting: Option<(NodeId, Receiver<Result<(), String>>)>,
    pub status: String,
    pub snapshot_created_at: i64,
    pub treemap: ui::treemap::TreemapCache,
    /// Tree panel should expand to (and scroll to) this node this frame.
    pub expand_to: Option<NodeId>,
    pub error: Option<(String, Instant)>,
}

impl Session {
    pub fn new(root: PathBuf, tree: Tree, created_at: i64) -> Self {
        let view = tree.root;
        Self {
            root,
            tree,
            generation: 1,
            view,
            selected: None,
            search: ui::search::SearchState::default(),
            validation: Validation::NotStarted,
            pending_delete: None,
            deleting: None,
            status: String::new(),
            snapshot_created_at: created_at,
            treemap: ui::treemap::TreemapCache::default(),
            expand_to: None,
            error: None,
        }
    }

    pub fn is_scanning(&self) -> bool {
        matches!(self.validation, Validation::Running(_))
    }

    pub fn set_error(&mut self, msg: String) {
        self.error = Some((msg, Instant::now()));
    }

    /// Replace the tree with a freshly scanned one, keeping the view and
    /// selection where they were (by path).
    fn swap_tree(&mut self, new_tree: Tree) -> DiffSummary {
        let view_path = self.tree.path(self.view);
        let sel_path = self.selected.map(|s| self.tree.path(s));
        let summary = snapshot::diff_summary(&self.tree, &new_tree);
        self.tree = new_tree;
        self.generation += 1;
        self.view = self.tree.resolve_nearest(&view_path);
        self.selected = sel_path.and_then(|p| self.tree.find_by_path(&p));
        self.search.dirty = true;
        summary
    }

    fn save_snapshot_async(&self, validated_at: Option<i64>) {
        let snap = Snapshot {
            version: snapshot::VERSION,
            root: self.root.clone(),
            created_at: self.snapshot_created_at,
            validated_at,
            tree: self.tree.clone(),
        };
        std::thread::spawn(move || {
            if let Err(e) = snapshot::save(&snap) {
                eprintln!("failed to save snapshot: {e}");
            }
            snapshot::touch_recent(RecentEntry::from(&snap));
        });
    }
}

pub struct StartState {
    pub path: String,
    pub drives: Vec<String>,
    pub recent: Vec<RecentEntry>,
    pub error: Option<String>,
}

impl StartState {
    pub fn new() -> Self {
        let drives = ('A'..='Z')
            .map(|c| format!("{c}:\\"))
            .filter(|d| Path::new(d).exists())
            .collect();
        Self {
            path: String::new(),
            drives,
            recent: snapshot::load_recent(),
            error: None,
        }
    }
}

pub enum AppState {
    Start(StartState),
    Scanning(ScanJob),
    Ready(Box<Session>),
}

/// Screen-level events produced by the UI, applied after drawing.
pub enum AppEvent {
    StartScan(PathBuf),
    CancelScan,
    BackToStart,
    Rescan,
    ConfirmDelete,
    CancelDelete,
    DismissError,
}

/// Per-item actions produced by the treemap, tree panel and search results.
#[derive(Debug, Clone, Copy)]
pub enum ItemAction {
    Select(NodeId),
    Zoom(NodeId),
    ZoomOut,
    Open(NodeId),
    Reveal(NodeId),
    CopyPath(NodeId),
    Delete(NodeId),
}

pub struct App {
    state: AppState,
    events: Vec<AppEvent>,
    actions: Vec<ItemAction>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_theme(egui::Theme::Dark);
        let mut app = Self {
            state: AppState::Start(StartState::new()),
            events: Vec::new(),
            actions: Vec::new(),
        };
        // `weight-folders.exe <folder>` starts scanning that folder right away.
        if let Some(arg) = std::env::args().nth(1) {
            let p = ui::start::normalize_input_path(&arg);
            if p.is_dir() {
                app.events.push(AppEvent::StartScan(p));
            } else if let AppState::Start(s) = &mut app.state {
                s.path = arg;
            }
        }
        app
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let mut next: Option<AppState> = None;
        match &mut self.state {
            AppState::Start(_) => {}
            AppState::Scanning(job) => {
                if let Some(msg) = job.poll() {
                    match msg {
                        ScanMsg::Done(tree) => {
                            let secs = job.started.elapsed().as_secs_f32();
                            let mut sess = Session::new(job.root.clone(), tree, now_unix());
                            sess.status = format!(
                                "Scan finished in {secs:.1}s: {} files, {} folders, {}. Snapshot saved.",
                                sess.tree.file_count,
                                sess.tree.dir_count,
                                fmt_size(sess.tree.total_size())
                            );
                            sess.save_snapshot_async(None);
                            next = Some(AppState::Ready(Box::new(sess)));
                        }
                        ScanMsg::Failed(e) => {
                            let mut s = StartState::new();
                            s.path = job.root.display().to_string();
                            s.error = Some(format!("Scan failed: {e}"));
                            next = Some(AppState::Start(s));
                        }
                        ScanMsg::Cancelled => {
                            let mut s = StartState::new();
                            s.path = job.root.display().to_string();
                            next = Some(AppState::Start(s));
                        }
                    }
                } else {
                    ctx.request_repaint_after(Duration::from_millis(100));
                }
            }
            AppState::Ready(sess) => {
                // Background validation / rescan.
                let mut finished: Option<ScanMsg> = None;
                if let Validation::Running(job) = &sess.validation {
                    finished = job.poll();
                    if finished.is_none() {
                        ctx.request_repaint_after(Duration::from_millis(100));
                    }
                }
                if let Some(msg) = finished {
                    match msg {
                        ScanMsg::Done(tree) => {
                            let summary = sess.swap_tree(tree);
                            let at = now_unix();
                            sess.status = if summary.is_empty() {
                                format!("Validated at {}: no changes since snapshot.", fmt_time(at))
                            } else {
                                format!(
                                    "Validated at {}: +{} added, -{} removed, ~{} changed, {}{} net.",
                                    fmt_time(at),
                                    summary.added,
                                    summary.removed,
                                    summary.changed,
                                    if summary.delta_bytes < 0 { "-" } else { "+" },
                                    fmt_size(summary.delta_bytes.unsigned_abs())
                                )
                            };
                            sess.validation = Validation::Done { summary, at };
                            sess.save_snapshot_async(Some(at));
                        }
                        ScanMsg::Failed(e) => {
                            sess.status = format!("Validation failed: {e}");
                            sess.validation = Validation::Failed(e);
                        }
                        ScanMsg::Cancelled => {
                            sess.status = "Validation cancelled.".into();
                            sess.validation = Validation::NotStarted;
                        }
                    }
                }

                // Pending delete on a worker thread.
                let mut delete_result: Option<(NodeId, Result<(), String>)> = None;
                if let Some((id, rx)) = &sess.deleting {
                    if let Ok(r) = rx.try_recv() {
                        delete_result = Some((*id, r));
                    } else {
                        ctx.request_repaint_after(Duration::from_millis(100));
                    }
                }
                if let Some((id, r)) = delete_result {
                    sess.deleting = None;
                    match r {
                        Ok(()) => {
                            let name = sess.tree.node(id).name.clone();
                            let size = sess.tree.node(id).size;
                            let view_path = if sess.tree.is_descendant_of(sess.view, id) {
                                let parent = sess.tree.node(id).parent.unwrap_or(sess.tree.root);
                                sess.tree.path(parent)
                            } else {
                                sess.tree.path(sess.view)
                            };
                            sess.tree.remove_subtree(id);
                            sess.generation += 1;
                            sess.view = sess.tree.resolve_nearest(&view_path);
                            sess.selected = None;
                            sess.search.dirty = true;
                            sess.status = format!("Moved {name} ({}) to the Recycle Bin.", fmt_size(size));
                            sess.save_snapshot_async(match &sess.validation {
                                Validation::Done { at, .. } => Some(*at),
                                _ => None,
                            });
                        }
                        Err(e) => {
                            sess.status = "Delete failed.".into();
                            sess.set_error(e);
                        }
                    }
                }
                if sess.error.is_some() {
                    ctx.request_repaint_after(Duration::from_millis(500));
                }
            }
        }
        if let Some(n) = next {
            self.state = n;
        }
    }

    fn handle_actions(&mut self, ctx: &egui::Context) {
        let AppState::Ready(sess) = &mut self.state else {
            self.actions.clear();
            return;
        };
        for a in self.actions.drain(..) {
            match a {
                ItemAction::Select(id) => {
                    sess.selected = Some(id);
                    sess.expand_to = Some(id);
                }
                ItemAction::Zoom(id) => {
                    let n = sess.tree.node(id);
                    sess.view = if n.is_dir { id } else { n.parent.unwrap_or(sess.tree.root) };
                    sess.selected = Some(id);
                    sess.expand_to = Some(id);
                }
                ItemAction::ZoomOut => {
                    if let Some(p) = sess.tree.node(sess.view).parent {
                        sess.selected = Some(sess.view);
                        sess.expand_to = Some(sess.view);
                        sess.view = p;
                    }
                }
                ItemAction::Open(id) => {
                    let p = sess.tree.path(id);
                    if let Err(e) = crate::actions::open_path(&p) {
                        sess.set_error(e);
                    }
                }
                ItemAction::Reveal(id) => {
                    let p = sess.tree.path(id);
                    if let Err(e) = crate::actions::reveal_in_explorer(&p) {
                        sess.set_error(e);
                    }
                }
                ItemAction::CopyPath(id) => {
                    let p = sess.tree.path(id);
                    ctx.copy_text(p.display().to_string());
                    sess.status = format!("Copied {}", p.display());
                }
                ItemAction::Delete(id) => {
                    if id == sess.tree.root {
                        sess.set_error("The scan root itself cannot be deleted from here.".into());
                    } else if sess.deleting.is_some() {
                        sess.set_error("A delete is already in progress.".into());
                    } else {
                        sess.pending_delete = Some(id);
                    }
                }
            }
        }
    }

    fn handle_events(&mut self) {
        let events: Vec<AppEvent> = self.events.drain(..).collect();
        for ev in events {
            match ev {
                AppEvent::StartScan(path) => self.start_scan(path),
                AppEvent::CancelScan => {
                    if let AppState::Scanning(job) = &self.state {
                        job.cancel();
                    }
                }
                AppEvent::BackToStart => {
                    let mut s = StartState::new();
                    if let AppState::Ready(sess) = &self.state {
                        s.path = sess.root.display().to_string();
                    }
                    self.state = AppState::Start(s);
                }
                AppEvent::Rescan => {
                    if let AppState::Ready(sess) = &mut self.state {
                        if let Validation::Running(job) = &sess.validation {
                            job.cancel();
                        }
                        sess.validation = Validation::Running(ScanJob::spawn(sess.root.clone()));
                        sess.status = "Rescanning in the background…".into();
                    }
                }
                AppEvent::ConfirmDelete => {
                    if let AppState::Ready(sess) = &mut self.state {
                        if let Some(id) = sess.pending_delete.take() {
                            let path = sess.tree.path(id);
                            let (tx, rx) = mpsc::channel();
                            std::thread::spawn(move || {
                                let _ = tx.send(crate::actions::delete_to_trash(&path));
                            });
                            sess.deleting = Some((id, rx));
                            sess.status = "Moving to the Recycle Bin…".into();
                        }
                    }
                }
                AppEvent::CancelDelete => {
                    if let AppState::Ready(sess) = &mut self.state {
                        sess.pending_delete = None;
                    }
                }
                AppEvent::DismissError => {
                    if let AppState::Ready(sess) = &mut self.state {
                        sess.error = None;
                    }
                }
            }
        }
    }

    fn start_scan(&mut self, path: PathBuf) {
        if !path.is_dir() {
            if let AppState::Start(s) = &mut self.state {
                s.error = Some(format!("{} is not a folder.", path.display()));
            }
            return;
        }
        if let Some(snap) = snapshot::load(&path) {
            let mut sess = Session::new(path.clone(), snap.tree, snap.created_at);
            sess.status = format!(
                "Loaded snapshot from {} — validating in the background…",
                fmt_time(snap.validated_at.unwrap_or(snap.created_at))
            );
            sess.validation = Validation::Running(ScanJob::spawn(path));
            self.state = AppState::Ready(Box::new(sess));
        } else {
            self.state = AppState::Scanning(ScanJob::spawn(path));
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);
        match &mut self.state {
            AppState::Start(s) => ui::start::show_start(ui, s, &mut self.events),
            AppState::Scanning(job) => ui::start::show_scanning(ui, job, &mut self.events),
            AppState::Ready(sess) => ui::show_ready(ui, sess, &mut self.events, &mut self.actions),
        }
        self.handle_actions(&ctx);
        self.handle_events();
    }
}
