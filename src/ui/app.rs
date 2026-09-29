//! The root view: repository state, background loading, operations and
//! keyboard handling. Rendering of each region lives in sibling modules.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use git2::Oid;
use gpui::{
    AppContext, ClipboardItem, Context, Entity, FocusHandle, KeyDownEvent, PathPromptOptions,
    Pixels, Point, ScrollStrategy, SharedString, Task, UniformListScrollHandle, Window, div,
    prelude::*, px,
};
use notify::{RecursiveMode, Watcher};

use crate::git::conflict::{ConflictFile, Pick};
use crate::git::diff::{DiffSource, FileDiff, FileStatus, file_diff};
use crate::git::ops::{Git, RebaseAction, RebaseStep, rebase_range};
use crate::git::repo::{self, CommitDetail, Snapshot, Summary, wip_oid};
use crate::workspace::Workspace;

use super::text_input::TextInput;
use super::theme::*;

pub type Action = Rc<dyn Fn(&mut GitPanda, &mut Window, &mut Context<GitPanda>)>;

pub fn action(f: impl Fn(&mut GitPanda, &mut Window, &mut Context<GitPanda>) + 'static) -> Action {
    Rc::new(f)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Error,
}

pub struct Toast {
    pub id: u64,
    pub kind: ToastKind,
    pub text: SharedString,
}

pub enum MenuItem {
    Action { label: SharedString, danger: bool, action: Action },
    Separator,
}

pub struct Menu {
    pub pos: Point<Pixels>,
    pub items: Vec<MenuItem>,
}

pub enum Modal {
    Input {
        title: SharedString,
        hint: Option<SharedString>,
        input: Entity<TextInput>,
        confirm: SharedString,
        submit: Rc<dyn Fn(&mut GitPanda, String, &mut Window, &mut Context<GitPanda>)>,
    },
    Confirm {
        title: SharedString,
        body: SharedString,
        confirm: SharedString,
        danger: bool,
        on_confirm: Action,
    },
    Rebase(RebaseEditor),
}

pub struct RebaseEditor {
    pub base: Option<Oid>,
    pub steps: Vec<RebaseStep>,
    pub cursor: usize,
    /// Step whose message is being edited, and the editor.
    pub editing: Option<(usize, Entity<TextInput>)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DiffRow {
    Hunk(usize),
    Line(usize, usize),
}

pub struct FileView {
    pub source: DiffSource,
    pub path: String,
    pub status: FileStatus,
    pub diff: Option<FileDiff>,
    pub rows: Vec<DiffRow>,
    /// Selected (hunk, line) pairs for line-level staging.
    pub selected: HashSet<(usize, usize)>,
    pub anchor: Option<(usize, usize)>,
    pub error: Option<SharedString>,
}

impl FileView {
    fn set_diff(&mut self, diff: Option<FileDiff>) {
        self.rows.clear();
        if let Some(d) = &diff {
            for (h, hunk) in d.hunks.iter().enumerate() {
                self.rows.push(DiffRow::Hunk(h));
                self.rows.extend((0..hunk.lines.len()).map(|l| DiffRow::Line(h, l)));
            }
        }
        // Keep the selection only if those lines still exist.
        self.selected.retain(|&(h, l)| diff.as_ref().is_some_and(|d| d.hunks.get(h).is_some_and(|x| l < x.lines.len())));
        self.diff = diff;
    }

    pub fn selected_in(&self, hunk: usize) -> HashSet<usize> {
        self.selected.iter().filter(|(h, _)| *h == hunk).map(|(_, l)| *l).collect()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ConflictRow {
    Text(usize, usize),
    Header(usize),
    Ours(usize, usize),
    Sep(usize),
    Theirs(usize, usize),
    Resolved(usize, usize),
    Empty(usize),
}

pub struct ConflictView {
    pub path: String,
    pub file: ConflictFile,
    pub rows: Vec<ConflictRow>,
    /// Maps conflict number to segment index.
    pub seg_of: Vec<usize>,
}

impl ConflictView {
    pub fn new(path: String, content: &str) -> Self {
        let mut v = ConflictView { path, file: ConflictFile::parse(content), rows: Vec::new(), seg_of: Vec::new() };
        v.rebuild();
        v
    }

    pub fn rebuild(&mut self) {
        use crate::git::conflict::Segment;
        self.rows.clear();
        self.seg_of.clear();
        let mut n = 0;
        for (si, s) in self.file.segments.iter().enumerate() {
            match s {
                Segment::Text(lines) => self.rows.extend((0..lines.len()).map(|l| ConflictRow::Text(si, l))),
                Segment::Conflict(c) => {
                    self.seg_of.push(si);
                    self.rows.push(ConflictRow::Header(n));
                    match c.pick {
                        None => {
                            self.rows.extend((0..c.ours.len()).map(|l| ConflictRow::Ours(n, l)));
                            self.rows.push(ConflictRow::Sep(n));
                            self.rows.extend((0..c.theirs.len()).map(|l| ConflictRow::Theirs(n, l)));
                        }
                        Some(p) => {
                            let len = match p {
                                Pick::Ours => c.ours.len(),
                                Pick::Theirs => c.theirs.len(),
                                _ => c.ours.len() + c.theirs.len(),
                            };
                            if len == 0 {
                                self.rows.push(ConflictRow::Empty(n));
                            }
                            self.rows.extend((0..len).map(|l| ConflictRow::Resolved(n, l)));
                        }
                    }
                    n += 1;
                }
            }
        }
    }

    pub fn resolved_line(&self, n: usize, l: usize) -> Option<&str> {
        let c = self.file.conflicts().nth(n)?;
        let (a, b) = match c.pick? {
            Pick::Ours => (&c.ours, None),
            Pick::Theirs => (&c.theirs, None),
            Pick::OursThenTheirs => (&c.ours, Some(&c.theirs)),
            Pick::TheirsThenOurs => (&c.theirs, Some(&c.ours)),
        };
        a.get(l).or_else(|| b.and_then(|b| b.get(l - a.len()))).map(String::as_str)
    }
}

pub struct GitPanda {
    pub focus: FocusHandle,
    pub workdir: Option<PathBuf>,
    pub snap: Option<Snapshot>,
    pub open_error: Option<SharedString>,
    pub loading: bool,
    load_gen: u64,
    reload_queued: bool,

    pub selected: Option<Oid>,
    /// Inclusive graph row range for multi-selection (shift-click).
    pub range: Option<(usize, usize)>,
    pub detail: Option<CommitDetail>,
    detail_gen: u64,
    pub file_view: Option<FileView>,
    file_gen: u64,
    pub conflict: Option<ConflictView>,

    pub commit_input: Entity<TextInput>,
    pub amend: bool,

    pub modal: Option<Modal>,
    pub menu: Option<Menu>,
    pub toasts: Vec<Toast>,
    next_toast: u64,
    pub busy: Option<SharedString>,
    pub collapsed: HashSet<&'static str>,

    pub graph_scroll: UniformListScrollHandle,
    pub diff_scroll: UniformListScrollHandle,
    pub files_scroll: UniformListScrollHandle,

    pub workspace: Workspace,
    /// Project bar summaries, by repository; `Err` when it can't be read.
    pub summaries: HashMap<PathBuf, Result<Summary, SharedString>>,
    pub(super) summary_gen: u64,
    /// Unsent commit messages of repositories switched away from.
    drafts: HashMap<PathBuf, String>,

    watcher: Option<notify::RecommendedWatcher>,
    fs_dirty: Arc<AtomicBool>,
    _poll: Task<()>,
}

impl GitPanda {
    pub fn new(path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let commit_input = cx.new(|cx| TextInput::new(cx, "Commit message (⌘⏎ to commit)", true));
        let fs_dirty = Arc::new(AtomicBool::new(false));
        // File system events only raise a flag; this loop coalesces them.
        let flag = fs_dirty.clone();
        let poll = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(250)).await;
            if flag.swap(false, Ordering::AcqRel) {
                if this.update(cx, |this, cx| this.reload(cx)).is_err() {
                    break;
                }
            }
        });
        cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.reload(cx);
                this.refresh_summaries(cx);
            }
        })
        .detach();
        let mut this = Self {
            focus: cx.focus_handle(),
            workdir: None,
            snap: None,
            open_error: None,
            loading: false,
            load_gen: 0,
            reload_queued: false,
            selected: None,
            range: None,
            detail: None,
            detail_gen: 0,
            file_view: None,
            file_gen: 0,
            conflict: None,
            commit_input,
            amend: false,
            modal: None,
            menu: None,
            toasts: Vec::new(),
            next_toast: 0,
            busy: None,
            collapsed: HashSet::new(),
            graph_scroll: UniformListScrollHandle::new(),
            diff_scroll: UniformListScrollHandle::new(),
            files_scroll: UniformListScrollHandle::new(),
            workspace: Workspace::load(),
            summaries: HashMap::new(),
            summary_gen: 0,
            drafts: HashMap::new(),
            watcher: None,
            fs_dirty,
            _poll: poll,
        };
        // An explicit path, else the repository we're in, else where the
        // active project was left.
        let cwd = std::env::current_dir().unwrap_or_default();
        match path {
            Some(p) => this.open_repo(&p, cx),
            None if repo::discover(&cwd).is_ok() => this.open_repo(&cwd, cx),
            None => match this.workspace.project().and_then(|p| p.start()).cloned() {
                Some(p) => this.open_repo(&p, cx),
                None => this.refresh_summaries(cx),
            },
        }
        this
    }

    // ---- repository loading -----------------------------------------------

    pub fn open_repo(&mut self, path: &Path, cx: &mut Context<Self>) {
        match repo::discover(path) {
            Ok(dir) => {
                let project = self.workspace.active;
                self.workspace.opened(&dir);
                self.save_workspace(cx);
                if self.workdir.as_ref() == Some(&dir) {
                    return self.reload(cx);
                }
                self.close_repo(cx);
                if let Some(draft) = self.drafts.remove(&dir) {
                    self.commit_input.update(cx, |i, cx| i.set_text(draft, cx));
                }
                self.workdir = Some(dir.clone());
                self.open_error = None;
                self.watch(&dir);
                self.reload(cx);
                if project != self.workspace.active || !self.summaries.contains_key(&dir) {
                    self.refresh_summaries(cx);
                }
            }
            Err(e) => {
                if self.workdir.is_some() {
                    self.toast(ToastKind::Error, e.to_string(), cx);
                } else {
                    self.open_error = Some(e.to_string().into());
                }
            }
        }
        cx.notify();
    }

    /// Closes the open repository, keeping its unsent commit message.
    pub fn close_repo(&mut self, cx: &mut Context<Self>) {
        if let Some(old) = self.workdir.take() {
            let draft = self.commit_input.read(cx).text().to_string();
            if !draft.trim().is_empty() {
                self.drafts.insert(old, draft);
            }
            self.commit_input.update(cx, |i, cx| i.set_text("", cx));
        }
        // Results still in flight belong to the old repository.
        self.load_gen += 1;
        self.detail_gen += 1;
        self.file_gen += 1;
        self.loading = false;
        self.reload_queued = false;
        self.watcher = None;
        self.snap = None;
        self.selected = None;
        self.range = None;
        self.detail = None;
        self.file_view = None;
        self.conflict = None;
        self.amend = false;
        self.menu = None;
        self.modal = None;
        cx.notify();
    }

    pub fn prompt_open(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open Repository".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                if let Some(p) = paths.into_iter().next() {
                    let _ = this.update(cx, |this, cx| this.open_repo(&p, cx));
                }
            }
        })
        .detach();
    }

    fn watch(&mut self, dir: &Path) {
        self.watcher = None;
        let flag = self.fs_dirty.clone();
        let root = dir.to_path_buf();
        let Ok(repo) = git2::Repository::open(dir) else { return };
        let git_dir = repo.path().to_path_buf();
        let handler = move |res: notify::Result<notify::Event>| {
            let Ok(ev) = res else { return };
            if matches!(ev.kind, notify::EventKind::Access(_)) {
                return;
            }
            let relevant = ev.paths.iter().any(|p| {
                if let Ok(rel) = p.strip_prefix(&git_dir) {
                    let s = rel.to_string_lossy();
                    return !(s.starts_with("objects") || s.ends_with(".lock") || s.starts_with("logs") || s.starts_with("gitpanda-"));
                }
                match p.strip_prefix(&root) {
                    Ok(rel) => !repo.is_path_ignored(rel).unwrap_or(false),
                    Err(_) => false,
                }
            });
            if relevant {
                flag.store(true, Ordering::Release);
            }
        };
        if let Ok(mut w) = notify::recommended_watcher(handler) {
            if w.watch(dir, RecursiveMode::Recursive).is_ok() {
                self.watcher = Some(w);
            }
        }
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.workdir.clone() else { return };
        if self.loading {
            self.reload_queued = true;
            return;
        }
        self.loading = true;
        self.load_gen += 1;
        let generation = self.load_gen;
        let prev = self.snap.as_ref().filter(|s| s.workdir == dir).map(|s| s.history.clone());
        cx.spawn(async move |this, cx| {
            let res = cx.background_executor().spawn(async move { repo::load(&dir, prev) }).await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                if generation == this.load_gen {
                    match res {
                        Ok(snap) => this.apply_snapshot(snap, cx),
                        Err(e) => this.toast(ToastKind::Error, format!("Couldn't read repository: {e}"), cx),
                    }
                }
                if std::mem::take(&mut this.reload_queued) {
                    this.reload(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_snapshot(&mut self, snap: Snapshot, cx: &mut Context<Self>) {
        let first = self.snap.is_none();
        let keep = self.selected.filter(|o| snap.index_of.contains_key(o) || self.is_stash(&snap, *o));
        let selected = keep.or_else(|| {
            if snap.commits.first().is_some_and(|c| c.oid == wip_oid()) {
                Some(wip_oid())
            } else {
                snap.head.oid
            }
        });
        if self.range.is_some_and(|(a, b)| b >= snap.commits.len() || a > b) {
            self.range = None;
        }
        let changed = selected != self.selected;
        self.selected = selected;
        self.summaries.insert(snap.workdir.clone(), Ok(Summary::of(&snap)));
        self.snap = Some(snap);
        if first {
            if let Some(i) = self.selected_index() {
                self.graph_scroll.scroll_to_item(i, ScrollStrategy::Center);
            }
        }
        // A conflict view whose file is no longer conflicted is done.
        if let (Some(cv), Some(s)) = (&self.conflict, &self.snap) {
            if !s.status.conflicted.iter().any(|e| e.path.as_ref() == cv.path) {
                self.conflict = None;
            }
        }
        self.load_detail(changed, cx);
        self.reload_file_view(cx);
        cx.notify();
    }

    fn is_stash(&self, snap: &Snapshot, oid: Oid) -> bool {
        snap.stashes.iter().any(|s| s.oid == oid)
    }

    pub fn selected_index(&self) -> Option<usize> {
        let s = self.snap.as_ref()?;
        s.index_of.get(&self.selected?).copied()
    }

    fn load_detail(&mut self, changed: bool, cx: &mut Context<Self>) {
        let (Some(dir), Some(oid)) = (self.workdir.clone(), self.selected) else {
            self.detail = None;
            return;
        };
        if oid == wip_oid() {
            self.detail = None;
            return;
        }
        if !changed && self.detail.as_ref().is_some_and(|d| d.oid == oid) {
            return;
        }
        self.detail_gen += 1;
        let generation = self.detail_gen;
        cx.spawn(async move |this, cx| {
            let res = cx.background_executor().spawn(async move { repo::commit_detail(&dir, oid) }).await;
            let _ = this.update(cx, |this, cx| {
                if generation != this.detail_gen {
                    return;
                }
                match res {
                    Ok(d) => this.detail = Some(d),
                    Err(e) => {
                        this.detail = None;
                        this.toast(ToastKind::Error, e.to_string(), cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ---- selection -----------------------------------------------------------

    pub fn select(&mut self, oid: Oid, cx: &mut Context<Self>) {
        self.range = None;
        if self.selected != Some(oid) {
            self.selected = Some(oid);
            self.file_view = None;
            self.conflict = None;
            self.load_detail(true, cx);
        }
        cx.notify();
    }

    pub fn select_index(&mut self, i: usize, extend: bool, cx: &mut Context<Self>) {
        let Some(snap) = &self.snap else { return };
        let Some(c) = snap.commits.get(i) else { return };
        let oid = c.oid;
        // The selected commit is the anchor; the range runs from it to `i`.
        if let Some(a) = self.selected_index().filter(|_| extend) {
            let (lo, hi) = (a.min(i), a.max(i));
            self.range = (lo != hi).then_some((lo, hi));
            return cx.notify();
        }
        self.select(oid, cx);
        self.graph_scroll.scroll_to_item(i, ScrollStrategy::Top);
    }

    pub fn reveal(&mut self, oid: Oid, cx: &mut Context<Self>) {
        self.select(oid, cx);
        if let Some(i) = self.selected_index() {
            self.graph_scroll.scroll_to_item(i, ScrollStrategy::Center);
        }
    }

    /// Selected commits (multi-selection), newest first, excluding WIP.
    pub fn selected_commits(&self) -> Vec<Oid> {
        let Some(s) = &self.snap else { return Vec::new() };
        match self.range {
            Some((a, b)) => s.commits[a..=b].iter().map(|c| c.oid).filter(|o| *o != wip_oid()).collect(),
            None => self.selected.into_iter().filter(|o| *o != wip_oid()).collect(),
        }
    }

    // ---- file views --------------------------------------------------------

    pub fn open_file(&mut self, source: DiffSource, path: String, status: FileStatus, cx: &mut Context<Self>) {
        self.conflict = None;
        self.file_view = Some(FileView {
            source,
            path,
            status,
            diff: None,
            rows: Vec::new(),
            selected: HashSet::new(),
            anchor: None,
            error: None,
        });
        self.diff_scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.reload_file_view(cx);
        cx.notify();
    }

    fn reload_file_view(&mut self, cx: &mut Context<Self>) {
        let (Some(dir), Some(fv)) = (self.workdir.clone(), &self.file_view) else { return };
        let source = fv.source.clone();
        let path = fv.path.clone();
        self.file_gen += 1;
        let generation = self.file_gen;
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    let repo = git2::Repository::open(&dir)?;
                    file_diff(&repo, &source, &path)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if generation != this.file_gen {
                    return;
                }
                let Some(fv) = this.file_view.as_mut() else { return };
                match res {
                    Ok(Some(d)) => {
                        fv.status = d.status;
                        fv.error = None;
                        fv.set_diff(Some(d));
                    }
                    // The change is gone from this side (fully staged, say).
                    Ok(None) if fv.diff.is_some() => this.file_view = None,
                    Ok(None) => {
                        fv.set_diff(None);
                        fv.error = Some("No changes".into());
                    }
                    Err(e) => fv.error = Some(e.to_string().into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn open_conflict(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(dir) = &self.workdir else { return };
        match std::fs::read_to_string(dir.join(&path)) {
            Ok(content) => {
                self.file_view = None;
                self.conflict = Some(ConflictView::new(path, &content));
            }
            Err(e) => self.toast(ToastKind::Error, format!("{path}: {e}"), cx),
        }
        cx.notify();
    }

    pub fn pick_conflict(&mut self, n: usize, pick: Option<Pick>, cx: &mut Context<Self>) {
        if let Some(cv) = &mut self.conflict {
            if let Some(c) = cv.file.conflict_mut(n) {
                c.pick = pick;
            }
            cv.rebuild();
        }
        cx.notify();
    }

    pub fn pick_all_conflicts(&mut self, pick: Pick, cx: &mut Context<Self>) {
        if let Some(cv) = &mut self.conflict {
            cv.file.pick_all(pick);
            cv.rebuild();
        }
        cx.notify();
    }

    pub fn save_conflict(&mut self, cx: &mut Context<Self>) {
        let Some(cv) = &self.conflict else { return };
        let path = cv.path.clone();
        let content = cv.file.render();
        let left = cv.file.unresolved();
        let msg = if left == 0 {
            format!("Resolved {path}")
        } else {
            format!("Saved {path} ({left} conflict{} still marked)", if left == 1 { "" } else { "s" })
        };
        self.run("Saving", move |g| g.write_resolved(&path, &content).map(|_| msg), cx);
        self.conflict = None;
    }

    // ---- operations ----------------------------------------------------------

    /// Run a git operation off the main thread, then toast and reload.
    pub fn run(
        &mut self,
        label: &str,
        op: impl FnOnce(&Git) -> anyhow::Result<String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(dir) = self.workdir.clone() else { return };
        self.busy = Some(format!("{label}…").into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = cx.background_executor().spawn(async move { op(&Git::new(&dir)) }).await;
            let _ = this.update(cx, |this, cx| {
                this.busy = None;
                match res {
                    Ok(msg) if !msg.is_empty() => this.toast(ToastKind::Success, msg, cx),
                    Ok(_) => {}
                    Err(e) => this.toast(ToastKind::Error, e.to_string(), cx),
                }
                this.reload(cx);
            });
        })
        .detach();
    }

    pub fn toast(&mut self, kind: ToastKind, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        let id = self.next_toast;
        self.next_toast += 1;
        self.toasts.push(Toast { id, kind, text: text.into() });
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
        let ttl = if kind == ToastKind::Error { 7000 } else { 3500 };
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(ttl)).await;
            let _ = this.update(cx, |this, cx| {
                this.toasts.retain(|t| t.id != id);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub fn head_branch(&self) -> Option<String> {
        self.snap.as_ref()?.head.branch.as_ref().map(|s| s.to_string())
    }

    pub fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let msg = self.commit_input.read(cx).text().trim().to_string();
        if msg.is_empty() {
            self.toast(ToastKind::Info, "Write a commit message first", cx);
            window.focus(&self.commit_input.read(cx).focus);
            return;
        }
        let staged = self.snap.as_ref().is_some_and(|s| !s.status.staged.is_empty());
        if !staged && !self.amend {
            self.toast(ToastKind::Info, "Nothing staged. Stage files first, or amend.", cx);
            return;
        }
        let amend = self.amend;
        self.commit_input.update(cx, |i, cx| i.set_text("", cx));
        self.amend = false;
        window.focus(&self.focus);
        self.run(if amend { "Amending" } else { "Committing" }, move |g| {
            let out = g.commit(&msg, amend)?;
            let first = out.lines().next().unwrap_or("").to_string();
            Ok(if first.is_empty() { "Committed".into() } else { first })
        }, cx);
    }

    pub fn toggle_amend(&mut self, cx: &mut Context<Self>) {
        self.amend = !self.amend;
        if self.amend && self.commit_input.read(cx).text().trim().is_empty() {
            if let Some(dir) = &self.workdir {
                if let Ok(m) = Git::new(dir).head_message() {
                    self.commit_input.update(cx, |i, cx| i.set_text(m, cx));
                }
            }
        }
        cx.notify();
    }

    pub fn hunk_op(&mut self, hunk: usize, lines_only: bool, discard: bool, cx: &mut Context<Self>) {
        let Some(fv) = &self.file_view else { return };
        let Some(diff) = fv.diff.clone() else { return };
        let source = fv.source.clone();
        let lines = lines_only.then(|| fv.selected_in(hunk));
        let what = if lines_only { "lines" } else { "hunk" };
        let verb = match (&source, discard) {
            (_, true) => "Discarded",
            (DiffSource::Staged, _) => "Unstaged",
            _ => "Staged",
        };
        let msg = format!("{verb} {what}");
        let op = move |g: &Git| g.apply_hunk(&diff, &source, hunk, lines.as_ref(), discard).map(|_| msg);
        if discard {
            self.confirm(
                format!("Discard this {what}?"),
                "The change will be removed from your working tree. This can't be undone.",
                "Discard",
                true,
                action(move |this, _, cx| {
                    let op = op.clone();
                    this.run("Discarding", op, cx);
                }),
            );
            cx.notify();
        } else {
            self.run(verb, op, cx);
        }
        if let Some(fv) = &mut self.file_view {
            fv.selected.clear();
        }
    }

    pub fn stage_path(&mut self, path: String, cx: &mut Context<Self>) {
        self.run("Staging", move |g| g.stage(&[&path]).map(|_| String::new()), cx);
    }

    pub fn unstage_path(&mut self, path: String, cx: &mut Context<Self>) {
        self.run("Unstaging", move |g| g.unstage(&[&path]).map(|_| String::new()), cx);
    }

    pub fn discard_path(&mut self, path: String, untracked: bool, cx: &mut Context<Self>) {
        let body = if untracked {
            format!("{path} is untracked and will be deleted.")
        } else {
            format!("All unstaged changes to {path} will be lost.")
        };
        self.confirm(
            "Discard changes?",
            body,
            "Discard",
            true,
            action(move |this, _, cx| {
                let path = path.clone();
                this.run("Discarding", move |g| g.discard(&path, untracked).map(|_| format!("Discarded {path}")), cx);
            }),
        );
        cx.notify();
    }

    // ---- dialogs -------------------------------------------------------------

    pub fn confirm(
        &mut self,
        title: impl Into<SharedString>,
        body: impl Into<SharedString>,
        confirm: impl Into<SharedString>,
        danger: bool,
        on_confirm: Action,
    ) {
        self.menu = None;
        self.modal = Some(Modal::Confirm {
            title: title.into(),
            body: body.into(),
            confirm: confirm.into(),
            danger,
            on_confirm,
        });
    }

    pub fn prompt(
        &mut self,
        title: impl Into<SharedString>,
        hint: Option<SharedString>,
        initial: &str,
        confirm: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
        submit: impl Fn(&mut GitPanda, String, &mut Window, &mut Context<GitPanda>) + 'static,
    ) {
        self.menu = None;
        let initial = initial.to_string();
        let input = cx.new(|cx| {
            let mut i = TextInput::new(cx, "", false);
            i.set_text(initial, cx);
            i.select_all(cx);
            i
        });
        window.focus(&input.read(cx).focus);
        self.modal = Some(Modal::Input {
            title: title.into(),
            hint,
            input,
            confirm: confirm.into(),
            submit: Rc::new(submit),
        });
        cx.notify();
    }

    pub fn close_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.modal = None;
        window.focus(&self.focus);
        cx.notify();
    }

    pub fn submit_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.modal.take() {
            Some(Modal::Input { input, submit, .. }) => {
                let text = input.read(cx).text().trim().to_string();
                window.focus(&self.focus);
                if !text.is_empty() {
                    submit(self, text, window, cx);
                }
            }
            Some(Modal::Confirm { on_confirm, .. }) => {
                window.focus(&self.focus);
                on_confirm(self, window, cx);
            }
            Some(Modal::Rebase(ed)) => {
                if ed.editing.is_some() {
                    self.modal = Some(Modal::Rebase(ed));
                    self.finish_rebase_message(window, cx);
                    return;
                }
                window.focus(&self.focus);
                self.run_rebase(ed, cx);
            }
            None => {}
        }
        cx.notify();
    }

    // ---- branch / commit actions -------------------------------------------

    pub fn new_branch_at(&mut self, at: String, window: &mut Window, cx: &mut Context<Self>) {
        let short = at.get(..7).unwrap_or(&at).to_string();
        self.prompt(
            "Create branch",
            Some(format!("Starts at {short} and is checked out.").into()),
            "",
            "Create & checkout",
            window,
            cx,
            move |this, name, _, cx| {
                let at = at.clone();
                let n = name.replace(' ', "-");
                this.run("Creating branch", move |g| g.create_branch(&n, &at, true).map(|_| format!("Switched to new branch {n}")), cx);
            },
        );
    }

    pub fn new_tag_at(&mut self, at: String, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt("Create tag", None, "", "Create tag", window, cx, move |this, name, _, cx| {
            let at = at.clone();
            this.run("Tagging", move |g| g.create_tag(&name, &at).map(|_| format!("Tagged {name}")), cx);
        });
    }

    pub fn rename_branch(&mut self, old: String, window: &mut Window, cx: &mut Context<Self>) {
        let initial = old.clone();
        self.prompt("Rename branch", Some(format!("Renaming {old}").into()), &initial, "Rename", window, cx, move |this, new, _, cx| {
            let old = old.clone();
            let new = new.replace(' ', "-");
            this.run("Renaming", move |g| g.rename_branch(&old, &new).map(|_| format!("Renamed {old} → {new}")), cx);
        });
    }

    pub fn rename_file(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let initial = path.clone();
        self.prompt("Rename file", Some("Moves the file with git mv.".into()), &initial, "Rename", window, cx, move |this, new, _, cx| {
            let from = path.clone();
            this.run("Renaming", move |g| g.rename_file(&from, &new).map(|_| format!("Renamed {from} → {new}")), cx);
        });
    }

    pub fn stash(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt(
            "Stash changes",
            Some("Includes untracked files. Leave the default or describe the stash.".into()),
            "WIP",
            "Stash",
            window,
            cx,
            |this, msg, _, cx| this.run("Stashing", move |g| g.stash(&msg).map(|_| "Stashed changes".into()), cx),
        );
    }

    pub fn push(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(snap) = &self.snap else { return };
        let Some(branch) = snap.head.branch.clone() else {
            return self.toast(ToastKind::Info, "HEAD is detached: check out a branch to push", cx);
        };
        let has_upstream = snap.branches.iter().any(|b| b.name == branch && b.upstream.is_some());
        let branch = branch.to_string();
        self.run(if force { "Force pushing" } else { "Pushing" }, move |g| {
            g.push(&branch, has_upstream, force).map(|_| format!("Pushed {branch}"))
        }, cx);
    }

    pub fn reset_to(&mut self, oid: Oid, mode: &'static str, cx: &mut Context<Self>) {
        let branch = self.head_branch().unwrap_or_else(|| "HEAD".into());
        let short = oid.to_string()[..7].to_string();
        let op = move |g: &Git| g.reset(oid, mode).map(|_| format!("Reset {branch} to {short} ({mode})"));
        if mode == "hard" {
            self.confirm(
                "Hard reset?",
                "Uncommitted changes and commits after this one will be discarded from the branch.",
                "Reset hard",
                true,
                action(move |this, _, cx| this.run("Resetting", op.clone(), cx)),
            );
            cx.notify();
        } else {
            self.run("Resetting", op, cx);
        }
    }

    /// Start the interactive rebase editor from `oldest` up to HEAD.
    pub fn start_rebase(&mut self, oldest: Oid, preset: impl Fn(&mut [RebaseStep]), cx: &mut Context<Self>) {
        let Some(dir) = &self.workdir else { return };
        match rebase_range(dir, oldest) {
            Ok((base, mut steps)) => {
                preset(&mut steps);
                self.menu = None;
                self.modal = Some(Modal::Rebase(RebaseEditor { base, cursor: steps.len().saturating_sub(1), steps, editing: None }));
            }
            Err(e) => self.toast(ToastKind::Error, e.to_string(), cx),
        }
        cx.notify();
    }

    /// Squash the multi-selected commits into the oldest of them.
    pub fn squash_selected(&mut self, cx: &mut Context<Self>) {
        let sel = self.selected_commits();
        if sel.len() < 2 {
            return self.toast(ToastKind::Info, "Shift-click to select two or more commits to squash", cx);
        }
        let oldest = *sel.last().unwrap();
        let chosen: HashSet<Oid> = sel.iter().copied().collect();
        let dir = self.workdir.clone().unwrap();
        // Combined message: all summaries, oldest first.
        let messages: Vec<String> = sel
            .iter()
            .rev()
            .filter_map(|o| crate::git::ops::commit_message(&dir, *o).ok())
            .collect();
        let combined = messages.join("\n\n");
        self.start_rebase(
            oldest,
            move |steps| {
                for s in steps.iter_mut() {
                    if chosen.contains(&s.oid) && s.oid != oldest {
                        s.action = RebaseAction::Squash;
                    }
                    if s.oid == oldest {
                        s.message = Some(combined.clone());
                    }
                }
            },
            cx,
        );
    }

    /// Asks before dropping `oids` (newest first) from the current branch.
    pub fn confirm_drop(&mut self, oids: Vec<Oid>, cx: &mut Context<Self>) {
        let Some(snap) = &self.snap else { return };
        let branch = self.head_branch().unwrap_or_else(|| "HEAD".into());
        let (title, body) = match oids.as_slice() {
            [] => return,
            [oid] => {
                let summary = snap.commits.iter().find(|c| c.oid == *oid).map_or(String::new(), |c| c.summary.to_string());
                (
                    "Drop commit?".to_string(),
                    format!("\"{summary}\" ({}) is removed from {branch}, and the commits after it are rebased onto its parent.", &oid.to_string()[..7]),
                )
            }
            _ => (
                format!("Drop {} commits?", oids.len()),
                format!("The selected commits are removed from {branch}, and the commits after them are rebased."),
            ),
        };
        let body = format!("{body} Uncommitted changes are stashed and restored.");
        self.confirm(title, body, "Drop", true, action(move |this, _, cx| this.drop_commits(&oids, cx)));
        cx.notify();
    }

    /// Removes `oids` (newest first) from the current branch by rebasing
    /// from the oldest of them with those steps set to drop.
    pub fn drop_commits(&mut self, oids: &[Oid], cx: &mut Context<Self>) {
        let (Some(dir), Some(&oldest)) = (self.workdir.clone(), oids.last()) else { return };
        let (base, mut steps) = match rebase_range(&dir, oldest) {
            Ok(r) => r,
            Err(e) => return self.toast(ToastKind::Error, e.to_string(), cx),
        };
        let chosen: HashSet<Oid> = oids.iter().copied().collect();
        for s in steps.iter_mut().filter(|s| chosen.contains(&s.oid)) {
            s.action = RebaseAction::Drop;
        }
        let n = steps.iter().filter(|s| s.action == RebaseAction::Drop).count();
        if n != chosen.len() {
            return self.toast(ToastKind::Error, "Only commits on the current branch can be dropped", cx);
        }
        self.run("Dropping", move |g| {
            g.interactive_rebase(base, &steps).map(|_| format!("Dropped {n} commit{}", if n == 1 { "" } else { "s" }))
        }, cx);
    }

    pub fn reword(&mut self, oid: Oid, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = self.workdir.clone() else { return };
        let is_head = self.snap.as_ref().is_some_and(|s| s.head.oid == Some(oid));
        let current = crate::git::ops::commit_message(&dir, oid).unwrap_or_default();
        if is_head {
            // Just amend the message.
            self.prompt("Reword commit", Some("Amends the latest commit's message.".into()), &current, "Reword", window, cx, |this, msg, _, cx| {
                this.run("Rewording", move |g| g.run_stdin(&["commit", "--amend", "--only", "-F", "-"], &msg).map(|_| "Reworded commit".into()), cx);
            });
            return;
        }
        self.start_rebase(oid, move |steps| {
            if let Some(s) = steps.iter_mut().find(|s| s.oid == oid) {
                s.action = RebaseAction::Reword;
            }
        }, cx);
        if let Some(Modal::Rebase(ed)) = &mut self.modal {
            ed.cursor = ed.steps.iter().position(|s| s.oid == oid).unwrap_or(0);
        }
        self.edit_rebase_message(window, cx);
    }

    pub fn edit_rebase_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = self.workdir.clone() else { return };
        let Some(Modal::Rebase(ed)) = &mut self.modal else { return };
        let i = ed.cursor;
        let Some(step) = ed.steps.get_mut(i) else { return };
        let text = step.message.clone().unwrap_or_else(|| crate::git::ops::commit_message(&dir, step.oid).unwrap_or_default());
        if step.action == RebaseAction::Pick {
            step.action = RebaseAction::Reword;
        }
        let input = cx.new(|cx| {
            let mut t = TextInput::new(cx, "Commit message", true);
            t.set_text(text, cx);
            t
        });
        window.focus(&input.read(cx).focus);
        ed.editing = Some((i, input));
        cx.notify();
    }

    fn finish_rebase_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Modal::Rebase(ed)) = &mut self.modal else { return };
        if let Some((i, input)) = ed.editing.take() {
            let text = input.read(cx).text().trim().to_string();
            if let Some(s) = ed.steps.get_mut(i) {
                if !text.is_empty() {
                    s.summary = text.lines().next().unwrap_or("").to_string();
                    s.message = Some(text);
                }
            }
        }
        window.focus(&self.focus);
        cx.notify();
    }

    fn run_rebase(&mut self, ed: RebaseEditor, cx: &mut Context<Self>) {
        let n = ed.steps.len();
        self.run("Rebasing", move |g| {
            g.interactive_rebase(ed.base, &ed.steps).map(|_| format!("Rebased {n} commit{}", if n == 1 { "" } else { "s" }))
        }, cx);
    }

    pub fn rebase_set(&mut self, action: RebaseAction, cx: &mut Context<Self>) {
        if let Some(Modal::Rebase(ed)) = &mut self.modal {
            if let Some(s) = ed.steps.get_mut(ed.cursor) {
                s.action = action;
            }
        }
        cx.notify();
    }

    pub fn rebase_move(&mut self, up: bool, cx: &mut Context<Self>) {
        // The list is shown newest first, so "up" means later in history.
        if let Some(Modal::Rebase(ed)) = &mut self.modal {
            let i = ed.cursor;
            let j = if up { i + 1 } else { i.wrapping_sub(1) };
            if j < ed.steps.len() {
                ed.steps.swap(i, j);
                ed.cursor = j;
            }
        }
        cx.notify();
    }

    // ---- keyboard -------------------------------------------------------------

    pub fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let m = ks.modifiers;
        let cmd = m.platform || m.control;
        let key = ks.key.as_str();

        if cmd && key == "q" {
            return cx.quit();
        }
        if self.menu.is_some() && key == "escape" {
            self.menu = None;
            return cx.notify();
        }
        if let Some(modal) = &self.modal {
            let editing_msg = matches!(modal, Modal::Rebase(RebaseEditor { editing: Some(_), .. }));
            match key {
                "escape" if editing_msg => {
                    if let Some(Modal::Rebase(ed)) = &mut self.modal {
                        ed.editing = None;
                    }
                    window.focus(&self.focus);
                    cx.notify();
                }
                "escape" => self.close_modal(window, cx),
                "enter" if editing_msg && !cmd => {}
                "enter" => self.submit_modal(window, cx),
                _ if matches!(modal, Modal::Rebase(_)) && !editing_msg => self.rebase_key(key, m.alt, window, cx),
                _ => {}
            }
            return;
        }
        match key {
            "escape" => {
                if self.conflict.take().is_some() || self.file_view.take().is_some() {
                    window.focus(&self.focus);
                } else if self.range.take().is_some() {
                } else {
                    window.focus(&self.focus);
                }
                cx.notify();
            }
            "enter" if cmd => self.commit(window, cx),
            "r" if cmd => {
                self.reload(cx);
                self.toast(ToastKind::Info, "Refreshed", cx);
            }
            "o" if cmd => self.prompt_open(cx),
            "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" if cmd => {
                let i = key.parse::<usize>().unwrap() - 1;
                if let Some(p) = self.workspace.project().and_then(|p| p.repos.get(i)).cloned() {
                    self.open_repo(&p, cx);
                }
            }
            "b" if cmd => {
                let at = self.selected.filter(|o| *o != wip_oid()).map_or("HEAD".to_string(), |o| o.to_string());
                self.new_branch_at(at, window, cx);
            }
            "s" if cmd && m.shift => self.stash(window, cx),
            "backspace" if cmd && self.file_view.is_none() && self.conflict.is_none() => {
                self.confirm_drop(self.selected_commits(), cx)
            }
            // With a file open, the arrows walk the panel's file list.
            "up" | "k" if !cmd && self.file_view.is_some() => self.step_file(-1, cx),
            "down" | "j" if !cmd && self.file_view.is_some() => self.step_file(1, cx),
            "right" | "l" if !cmd && self.file_view.is_none() && self.conflict.is_none() => self.step_file(0, cx),
            "left" | "h" if !cmd && self.file_view.is_some() => {
                self.file_view = None;
                window.focus(&self.focus);
                cx.notify();
            }
            "up" | "k" if !cmd => self.move_selection(-1, m.shift, cx),
            "down" | "j" if !cmd => self.move_selection(1, m.shift, cx),
            "pageup" => self.move_selection(-20, false, cx),
            "pagedown" => self.move_selection(20, false, cx),
            "c" if cmd => {
                if let Some(o) = self.selected.filter(|o| *o != wip_oid()) {
                    cx.write_to_clipboard(ClipboardItem::new_string(o.to_string()));
                    self.toast(ToastKind::Info, "Copied commit SHA", cx);
                }
            }
            _ => {}
        }
    }

    fn move_selection(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        if self.file_view.is_some() || self.conflict.is_some() {
            return;
        }
        let Some(snap) = &self.snap else { return };
        let n = snap.commits.len();
        if n == 0 {
            return;
        }
        let cur = if extend {
            match self.range {
                Some((a, b)) => if Some(a) == self.selected_index() { b } else { a },
                None => self.selected_index().unwrap_or(0),
            }
        } else {
            self.selected_index().unwrap_or(0)
        };
        let next = (cur as isize + delta).clamp(0, n as isize - 1) as usize;
        self.select_index(next, extend, cx);
        self.graph_scroll.scroll_to_item(next, ScrollStrategy::Top);
    }

    fn rebase_key(&mut self, key: &str, alt: bool, window: &mut Window, cx: &mut Context<Self>) {
        let len = match &self.modal {
            Some(Modal::Rebase(ed)) => ed.steps.len(),
            _ => return,
        };
        match key {
            "up" | "k" if alt => self.rebase_move(true, cx),
            "down" | "j" if alt => self.rebase_move(false, cx),
            "up" | "k" => {
                if let Some(Modal::Rebase(ed)) = &mut self.modal {
                    ed.cursor = (ed.cursor + 1).min(len - 1);
                }
            }
            "down" | "j" => {
                if let Some(Modal::Rebase(ed)) = &mut self.modal {
                    ed.cursor = ed.cursor.saturating_sub(1);
                }
            }
            "p" => self.rebase_set(RebaseAction::Pick, cx),
            "s" => self.rebase_set(RebaseAction::Squash, cx),
            "f" => self.rebase_set(RebaseAction::Fixup, cx),
            "d" => self.rebase_set(RebaseAction::Drop, cx),
            "r" | "e" => self.edit_rebase_message(window, cx),
            _ => {}
        }
        cx.notify();
    }
}

impl Render for GitPanda {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let project = self.workspace.project().map(|p| format!(" · {}", p.name)).unwrap_or_default();
        let title = match &self.snap {
            Some(s) => format!("{}{project} — gitpanda", s.name),
            None => format!("gitpanda{project}"),
        };
        window.set_window_title(&title);

        let body = if self.workdir.is_none() {
            div()
                .flex()
                .flex_col()
                .size_full()
                .when(!self.workspace.projects.is_empty(), |d| d.child(self.render_project_bar(cx)))
                .child(div().flex_1().min_h_0().child(self.render_welcome(cx)))
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .size_full()
                .child(self.render_project_bar(cx))
                .child(self.render_toolbar(cx))
                .children(self.render_banner(cx))
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .min_h_0()
                        .child(self.render_sidebar(cx))
                        .child(self.render_center(window, cx))
                        .child(self.render_panel(window, cx)),
                )
                .child(self.render_statusbar())
                .into_any_element()
        };

        div()
            .id("root")
            .relative()
            .size_full()
            .bg(c(BG))
            .text_color(c(FG))
            .font_family(UI_FONT)
            .text_size(px(13.))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .child(body)
            .children(self.render_modal(window, cx))
            .children(self.render_menu(cx))
            .child(self.render_toasts(cx))
    }
}

impl GitPanda {
    /// Entry points for `GITPANDA_SCRIPT` (see `debug.rs`).
    pub fn debug_call(&mut self, cmd: &str, window: &mut Window, cx: &mut Context<Self>) {
        let mut it = cmd.split_whitespace();
        let name = it.next().unwrap_or("");
        let args: Vec<&str> = it.collect();
        let num = |i: usize| args.get(i).and_then(|a| a.parse::<usize>().ok()).unwrap_or(0);
        match name {
            "select" => self.select_index(num(0), false, cx),
            "range" => {
                self.select_index(num(0), false, cx);
                self.select_index(num(1), true, cx);
            }
            "unstaged" => self.open_file(DiffSource::Unstaged, args.join(" "), FileStatus::Modified, cx),
            "staged" => self.open_file(DiffSource::Staged, args.join(" "), FileStatus::Modified, cx),
            "commit-file" => {
                if let Some(d) = &self.detail {
                    if let Some(f) = d.files.get(num(0)) {
                        let (p, s, o) = (f.path.clone(), f.status, d.oid);
                        self.open_file(DiffSource::Commit(o), p, s, cx);
                    }
                }
            }
            "line" => {
                if let Some(fv) = &mut self.file_view {
                    fv.selected.insert((num(0), num(1)));
                }
            }
            "hunk" => self.hunk_op(num(0), num(1) == 1, false, cx),
            "save-conflict" => self.save_conflict(cx),
            "continue" => {
                if let Some(state) = self.snap.as_ref().map(|s| s.state) {
                    self.run("Continuing", move |g| g.continue_op(state).map(|_| String::new()), cx);
                }
            }
            "conflict" => self.open_conflict(args.join(" "), cx),
            "pick" => self.pick_conflict(num(0), Some(Pick::Ours), cx),
            "rebase" => {
                if let Some(o) = self.snap.as_ref().and_then(|s| s.commits.get(num(0))).map(|c| c.oid) {
                    self.start_rebase(o, |_| {}, cx);
                }
            }
            "squash" => self.squash_selected(cx),
            "menu" => {
                if let Some(o) = self.snap.as_ref().and_then(|s| s.commits.get(num(0))).map(|c| c.oid) {
                    let items = self.commit_menu(o);
                    self.menu = Some(Menu { pos: gpui::point(px(num(1) as f32), px(num(2) as f32)), items });
                }
            }
            "branch" => self.new_branch_at("HEAD".into(), window, cx),
            "type" => {
                let text = args.join(" ");
                self.commit_input.update(cx, |i, cx| i.set_text(text, cx));
                window.focus(&self.commit_input.read(cx).focus);
            }
            "toast" => self.toast(ToastKind::Success, args.join(" "), cx),
            "focus" => window.focus(&self.focus),
            "repo" => {
                if let Some(p) = self.workspace.project().and_then(|p| p.repos.get(num(0))).cloned() {
                    self.open_repo(&p, cx);
                }
            }
            "project" => self.switch_project(num(0), cx),
            "project-menu" => {
                let items = self.project_menu();
                self.menu = Some(Menu { pos: gpui::point(px(80.), px(38.)), items });
            }
            _ => eprintln!("gitpanda: unknown call {name:?}"),
        }
        cx.notify();
    }
}
