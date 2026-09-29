//! A read-only snapshot of a repository, built on a background thread with
//! libgit2: refs, history with its graph layout, and working tree status.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use git2::{BranchType, Oid, Repository, RepositoryState, Sort, Status, StatusOptions};
use gpui::SharedString;
use smallvec::SmallVec;

use super::diff::FileStatus;
use super::graph::{self, GraphRow};

/// Most commits loaded into the graph.
pub const MAX_COMMITS: usize = 50_000;

#[derive(Clone, Debug)]
pub struct Branch {
    pub name: SharedString,
    pub oid: Oid,
    pub is_head: bool,
    pub upstream: Option<SharedString>,
    pub ahead: usize,
    pub behind: usize,
}

#[derive(Clone, Debug)]
pub struct Stash {
    pub index: usize,
    pub message: SharedString,
    pub oid: Oid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefKind {
    Head,
    Local,
    Remote,
    Tag,
}

#[derive(Clone, Debug)]
pub struct RefLabel {
    pub name: SharedString,
    pub kind: RefKind,
}

#[derive(Clone, Debug)]
pub struct Commit {
    pub oid: Oid,
    pub short: SharedString,
    pub summary: SharedString,
    pub author: SharedString,
    pub email: SharedString,
    pub time: i64,
    pub parents: SmallVec<[Oid; 2]>,
}

#[derive(Clone, Debug)]
pub struct StatusEntry {
    pub path: SharedString,
    pub status: FileStatus,
}

#[derive(Clone, Debug, Default)]
pub struct WorkStatus {
    pub staged: Vec<StatusEntry>,
    pub unstaged: Vec<StatusEntry>,
    pub conflicted: Vec<StatusEntry>,
}

impl WorkStatus {
    pub fn is_clean(&self) -> bool {
        self.staged.is_empty() && self.unstaged.is_empty() && self.conflicted.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpState {
    Clean,
    Merge,
    Rebase,
    CherryPick,
    Revert,
    Bisect,
    Other,
}

impl OpState {
    pub fn label(self) -> &'static str {
        match self {
            OpState::Clean => "",
            OpState::Merge => "MERGING",
            OpState::Rebase => "REBASING",
            OpState::CherryPick => "CHERRY-PICKING",
            OpState::Revert => "REVERTING",
            OpState::Bisect => "BISECTING",
            OpState::Other => "IN PROGRESS",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Head {
    pub branch: Option<SharedString>,
    pub oid: Option<Oid>,
}

/// Walked history, reused while no ref moves (edits to the working tree
/// don't change it, so most reloads skip the walk entirely).
pub struct History {
    tips: Vec<Oid>,
    pub commits: Vec<Commit>,
    pub truncated: bool,
}

pub struct Snapshot {
    pub workdir: PathBuf,
    pub name: SharedString,
    pub head: Head,
    pub state: OpState,
    /// (step, total) for a rebase in progress.
    pub rebase_progress: Option<(usize, usize)>,
    pub branches: Vec<Branch>,
    pub remotes: Vec<Branch>,
    pub tags: Vec<(SharedString, Oid)>,
    pub stashes: Vec<Stash>,
    pub commits: Vec<Commit>,
    pub graph: Vec<GraphRow>,
    pub graph_width: u16,
    pub refs: HashMap<Oid, Vec<RefLabel>>,
    pub index_of: HashMap<Oid, usize>,
    pub status: WorkStatus,
    pub truncated: bool,
    pub load_time: Duration,
    pub history: std::sync::Arc<History>,
}

/// Oid used for the "uncommitted changes" pseudo-commit at the top.
pub fn wip_oid() -> Oid {
    Oid::zero()
}

pub fn discover(path: &Path) -> Result<PathBuf> {
    let repo = Repository::discover(path)
        .with_context(|| format!("{} is not inside a git repository", path.display()))?;
    repo.workdir()
        .map(Path::to_path_buf)
        .context("bare repositories are not supported")
}

pub fn status(repo: &Repository) -> Result<WorkStatus> {
    let mut o = StatusOptions::new();
    o.include_untracked(true)
        .recurse_untracked_dirs(true)
        .renames_head_to_index(true)
        .exclude_submodules(true);
    let statuses = repo.statuses(Some(&mut o))?;
    let mut ws = WorkStatus::default();
    for e in statuses.iter() {
        let s = e.status();
        let path: SharedString = e.path().unwrap_or("").to_string().into();
        if s.is_conflicted() {
            ws.conflicted.push(StatusEntry { path, status: FileStatus::Conflicted });
            continue;
        }
        let staged = if s.contains(Status::INDEX_NEW) {
            Some(FileStatus::Added)
        } else if s.contains(Status::INDEX_DELETED) {
            Some(FileStatus::Deleted)
        } else if s.contains(Status::INDEX_RENAMED) {
            Some(FileStatus::Renamed)
        } else if s.contains(Status::INDEX_TYPECHANGE) {
            Some(FileStatus::TypeChange)
        } else if s.contains(Status::INDEX_MODIFIED) {
            Some(FileStatus::Modified)
        } else {
            None
        };
        let unstaged = if s.contains(Status::WT_NEW) {
            Some(FileStatus::Untracked)
        } else if s.contains(Status::WT_DELETED) {
            Some(FileStatus::Deleted)
        } else if s.contains(Status::WT_RENAMED) {
            Some(FileStatus::Renamed)
        } else if s.contains(Status::WT_TYPECHANGE) {
            Some(FileStatus::TypeChange)
        } else if s.contains(Status::WT_MODIFIED) {
            Some(FileStatus::Modified)
        } else {
            None
        };
        if let Some(st) = staged {
            // A staged rename is shown under its new path.
            let p = e
                .head_to_index()
                .and_then(|d| d.new_file().path().map(|p| p.to_string_lossy().into_owned()))
                .map(SharedString::from)
                .unwrap_or_else(|| path.clone());
            ws.staged.push(StatusEntry { path: p, status: st });
        }
        if let Some(st) = unstaged {
            ws.unstaged.push(StatusEntry { path, status: st });
        }
    }
    Ok(ws)
}

fn op_state(repo: &Repository) -> OpState {
    match repo.state() {
        RepositoryState::Clean => OpState::Clean,
        RepositoryState::Merge => OpState::Merge,
        RepositoryState::Rebase
        | RepositoryState::RebaseInteractive
        | RepositoryState::RebaseMerge
        | RepositoryState::ApplyMailboxOrRebase => OpState::Rebase,
        RepositoryState::CherryPick | RepositoryState::CherryPickSequence => OpState::CherryPick,
        RepositoryState::Revert | RepositoryState::RevertSequence => OpState::Revert,
        RepositoryState::Bisect => OpState::Bisect,
        _ => OpState::Other,
    }
}

fn rebase_progress(repo: &Repository) -> Option<(usize, usize)> {
    let dir = repo.path();
    for sub in ["rebase-merge", "rebase-apply"] {
        let d = dir.join(sub);
        let read = |f: &str| {
            std::fs::read_to_string(d.join(f))
                .ok()
                .and_then(|s| s.trim().parse::<usize>().ok())
        };
        if let (Some(a), Some(b)) = (read("msgnum").or(read("next")), read("end").or(read("last"))) {
            return Some((a, b));
        }
    }
    None
}

pub fn load(workdir: &Path, prev: Option<std::sync::Arc<History>>) -> Result<Snapshot> {
    let start = Instant::now();
    let repo = Repository::open(workdir)?;
    let name: SharedString = workdir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
        .into();

    let head_ref = repo.head().ok();
    let head = Head {
        branch: head_ref
            .as_ref()
            .filter(|h| h.is_branch())
            .and_then(|h| h.shorthand().map(|s| s.to_string().into())),
        oid: head_ref.as_ref().and_then(|h| h.target()),
    };
    // An unborn branch still has a name worth showing.
    let head = if head_ref.is_none() {
        let unborn = repo
            .find_reference("HEAD")
            .ok()
            .and_then(|r| r.symbolic_target().map(|t| t.trim_start_matches("refs/heads/").to_string()));
        Head { branch: unborn.map(Into::into), oid: None }
    } else {
        head
    };

    let mut refs: HashMap<Oid, Vec<RefLabel>> = HashMap::new();
    let mut branches = Vec::new();
    let mut remotes = Vec::new();
    for b in repo.branches(None)? {
        let (b, kind) = b?;
        let Some(name) = b.name()?.map(str::to_string) else { continue };
        let Some(oid) = b.get().target() else { continue };
        match kind {
            BranchType::Local => {
                let upstream = b.upstream().ok();
                let up_name = upstream
                    .as_ref()
                    .and_then(|u| u.name().ok().flatten().map(|s| s.to_string().into()));
                let (ahead, behind) = upstream
                    .as_ref()
                    .and_then(|u| u.get().target())
                    .and_then(|u| repo.graph_ahead_behind(oid, u).ok())
                    .unwrap_or((0, 0));
                let is_head = b.is_head();
                refs.entry(oid).or_default().push(RefLabel {
                    name: name.clone().into(),
                    kind: if is_head { RefKind::Head } else { RefKind::Local },
                });
                branches.push(Branch { name: name.into(), oid, is_head, upstream: up_name, ahead, behind });
            }
            BranchType::Remote => {
                if name.ends_with("/HEAD") {
                    continue;
                }
                refs.entry(oid)
                    .or_default()
                    .push(RefLabel { name: name.clone().into(), kind: RefKind::Remote });
                remotes.push(Branch {
                    name: name.into(),
                    oid,
                    is_head: false,
                    upstream: None,
                    ahead: 0,
                    behind: 0,
                });
            }
        }
    }
    branches.sort_by(|a, b| a.name.cmp(&b.name));
    remotes.sort_by(|a, b| a.name.cmp(&b.name));

    let mut tags = Vec::new();
    repo.tag_foreach(|oid, name| {
        let name = String::from_utf8_lossy(name).trim_start_matches("refs/tags/").to_string();
        let target = repo
            .find_object(oid, None)
            .and_then(|o| o.peel_to_commit())
            .map(|c| c.id())
            .unwrap_or(oid);
        tags.push((SharedString::from(name), target));
        true
    })?;
    tags.sort_by(|a, b| b.0.cmp(&a.0));
    for (name, oid) in &tags {
        refs.entry(*oid)
            .or_default()
            .push(RefLabel { name: name.clone(), kind: RefKind::Tag });
    }
    if head.branch.is_none() {
        if let Some(oid) = head.oid {
            refs.entry(oid)
                .or_default()
                .insert(0, RefLabel { name: "HEAD".into(), kind: RefKind::Head });
        }
    }
    for labels in refs.values_mut() {
        labels.sort_by_key(|l| match l.kind {
            RefKind::Head => 0,
            RefKind::Local => 1,
            RefKind::Remote => 2,
            RefKind::Tag => 3,
        });
    }

    // `stash_foreach` needs `&mut`, so read stashes from a second handle.
    let mut stashes = Vec::new();
    if let Ok(mut r2) = Repository::open(workdir) {
        let _ = r2.stash_foreach(|index, message, oid| {
            stashes.push(Stash { index, message: message.to_string().into(), oid: *oid });
            true
        });
    }

    let status = status(&repo)?;
    let state = op_state(&repo);
    let rebase_progress = (state == OpState::Rebase).then(|| rebase_progress(&repo)).flatten();

    // History, unless every tip is where it was last time.
    let mut tips: Vec<Oid> = head.oid.into_iter().collect();
    tips.extend(branches.iter().chain(remotes.iter()).map(|b| b.oid));
    tips.extend(tags.iter().map(|t| t.1));
    tips.sort_unstable();
    tips.dedup();
    let history = match prev.filter(|h| h.tips == tips) {
        Some(h) => h,
        None => std::sync::Arc::new(walk_history(&repo, head.oid, tips)?),
    };

    let mut commits = Vec::with_capacity(history.commits.len() + 1);
    let dirty = !status.is_clean();
    if dirty {
        let n = status.staged.len() + status.unstaged.len() + status.conflicted.len();
        commits.push(Commit {
            oid: wip_oid(),
            short: "".into(),
            summary: format!("// WIP · {n} changed file{}", if n == 1 { "" } else { "s" }).into(),
            author: "".into(),
            email: "".into(),
            time: 0,
            parents: head.oid.into_iter().collect(),
        });
    }
    commits.extend(history.commits.iter().cloned());
    let truncated = history.truncated;
    let layout = graph::layout(commits.iter().map(|c| (c.oid, c.parents.as_slice())));
    let index_of = commits.iter().enumerate().map(|(i, c)| (c.oid, i)).collect();

    Ok(Snapshot {
        workdir: workdir.to_path_buf(),
        name,
        head,
        state,
        rebase_progress,
        branches,
        remotes,
        tags,
        stashes,
        commits,
        graph: layout.rows,
        graph_width: layout.width,
        refs,
        index_of,
        status,
        truncated,
        load_time: start.elapsed(),
        history,
    })
}

fn walk_history(repo: &Repository, head: Option<Oid>, tips: Vec<Oid>) -> Result<History> {
    let mut walk = repo.revwalk()?;
    walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)?;
    // HEAD first so its line gets the leftmost lane.
    if let Some(oid) = head {
        walk.push(oid)?;
    }
    for t in &tips {
        let _ = walk.push(*t);
    }
    let mut commits = Vec::with_capacity(1024);
    let mut truncated = false;
    for oid in walk {
        let oid = oid?;
        if commits.len() >= MAX_COMMITS {
            truncated = true;
            break;
        }
        let c = repo.find_commit(oid)?;
        let author = c.author();
        let short = oid.to_string()[..7].to_string();
        commits.push(Commit {
            oid,
            short: short.into(),
            summary: c.summary_bytes().map(|s| String::from_utf8_lossy(s).into_owned()).unwrap_or_default().into(),
            author: author.name().unwrap_or("").to_string().into(),
            email: author.email().unwrap_or("").to_string().into(),
            time: c.time().seconds(),
            parents: c.parent_ids().collect(),
        });
    }
    Ok(History { tips, commits, truncated })
}

/// Details for the commit panel.
#[derive(Clone, Debug)]
pub struct CommitDetail {
    pub oid: Oid,
    pub message: SharedString,
    pub author: SharedString,
    pub email: SharedString,
    pub time: i64,
    pub committer: SharedString,
    pub parents: Vec<Oid>,
    pub files: Vec<super::diff::FileDiff>,
}

pub fn commit_detail(workdir: &Path, oid: Oid) -> Result<CommitDetail> {
    let repo = Repository::open(workdir)?;
    let c = repo.find_commit(oid)?;
    let diff = super::diff::raw_diff(&repo, &super::diff::DiffSource::Commit(oid), None)?;
    let mut files = super::diff::file_list(&diff);
    for (i, f) in files.iter_mut().enumerate() {
        if let Ok(Some(p)) = git2::Patch::from_diff(&diff, i) {
            if let Ok((_, add, del)) = p.line_stats() {
                f.additions = add;
                f.deletions = del;
            }
        }
    }
    let author = c.author();
    Ok(CommitDetail {
        oid,
        message: c.message().unwrap_or("").trim_end().to_string().into(),
        author: author.name().unwrap_or("").to_string().into(),
        email: author.email().unwrap_or("").to_string().into(),
        time: author.when().seconds(),
        committer: c.committer().name().unwrap_or("").to_string().into(),
        parents: c.parent_ids().collect(),
        files,
    })
}
