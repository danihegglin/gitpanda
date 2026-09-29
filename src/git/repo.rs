//! A read-only snapshot of a repository, built on a background thread:
//! refs and history with its graph layout from libgit2, working tree status
//! from `git status` (see `status`).

use std::collections::{BinaryHeap, HashMap, HashSet};
use std::cmp::Reverse;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use git2::{BranchType, Oid, Repository, RepositoryState};
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
    /// Tag ref target → the commit it peels to. Keys are content hashes, so
    /// entries never go stale; peeling ~1000 tags each reload costs ~0.5 s.
    peeled: HashMap<Oid, Oid>,
    /// Graph layouts without and with the `// WIP` row, made on first use.
    layouts: [OnceLock<(Arc<Vec<GraphRow>>, u16)>; 2],
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
    pub graph: Arc<Vec<GraphRow>>,
    pub graph_width: u16,
    pub refs: HashMap<Oid, Vec<RefLabel>>,
    pub index_of: HashMap<Oid, usize>,
    pub status: WorkStatus,
    pub truncated: bool,
    pub load_time: Duration,
    pub history: Arc<History>,
}

/// Oid used for the "uncommitted changes" pseudo-commit at the top.
pub fn wip_oid() -> Oid {
    Oid::zero()
}

pub fn discover(path: &Path) -> Result<PathBuf> {
    let repo = Repository::discover(path)
        .with_context(|| format!("{} is not inside a git repository", path.display()))?;
    // Without the trailing slash libgit2 adds, so paths compare and print cleanly.
    repo.workdir()
        .map(|p| p.components().collect())
        .context("bare repositories are not supported")
}

/// Working tree status from `git status`, which checks files on several
/// threads (and uses fsmonitor or the untracked cache when configured):
/// ~4× faster than libgit2 on the Linux kernel's 96k files.
pub fn status(workdir: &Path) -> Result<WorkStatus> {
    let out = std::process::Command::new("git")
        .current_dir(workdir)
        // Never refresh the index as a side effect: that would take its
        // lock under the user's feet and wake our own file watcher.
        .args(["--no-optional-locks", "status", "--porcelain=v2", "-z", "--untracked-files=all", "--ignore-submodules=all"])
        .stdin(std::process::Stdio::null())
        .output()
        .context("could not run git")?;
    if !out.status.success() {
        anyhow::bail!("git status failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(parse_status(&String::from_utf8_lossy(&out.stdout)))
}

/// Parses `git status --porcelain=v2 -z`.
pub fn parse_status(out: &str) -> WorkStatus {
    let mut ws = WorkStatus::default();
    let mut records = out.split('\0');
    while let Some(rec) = records.next() {
        let entry = |path: &str, status| StatusEntry { path: path.to_string().into(), status };
        let (xy, path) = match rec.as_bytes().first() {
            Some(b'?') => {
                ws.unstaged.push(entry(&rec[2..], FileStatus::Untracked));
                continue;
            }
            Some(b'u') => {
                if let Some(path) = rec.splitn(11, ' ').nth(10) {
                    ws.conflicted.push(entry(path, FileStatus::Conflicted));
                }
                continue;
            }
            Some(b'1') => match rec.splitn(9, ' ').collect::<Vec<_>>()[..] {
                [_, xy, .., path] => (xy, path),
                _ => continue,
            },
            Some(b'2') => {
                records.next(); // the original path of a rename
                match rec.splitn(10, ' ').collect::<Vec<_>>()[..] {
                    [_, xy, .., path] => (xy, path),
                    _ => continue,
                }
            }
            _ => continue,
        };
        let [x, y] = xy.as_bytes()[..] else { continue };
        let staged = match x {
            b'A' | b'C' => Some(FileStatus::Added),
            b'D' => Some(FileStatus::Deleted),
            b'R' => Some(FileStatus::Renamed),
            b'T' => Some(FileStatus::TypeChange),
            b'M' => Some(FileStatus::Modified),
            _ => None,
        };
        let unstaged = match y {
            b'D' => Some(FileStatus::Deleted),
            b'T' => Some(FileStatus::TypeChange),
            b'M' => Some(FileStatus::Modified),
            _ => None,
        };
        // A staged rename is listed under its new path.
        if let Some(st) = staged {
            ws.staged.push(entry(path, st));
        }
        if let Some(st) = unstaged {
            ws.unstaged.push(entry(path, st));
        }
    }
    ws
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

fn read_head(repo: &Repository) -> Head {
    let Ok(head_ref) = repo.head() else {
        // An unborn branch still has a name worth showing.
        let unborn = repo
            .find_reference("HEAD")
            .ok()
            .and_then(|r| r.symbolic_target().map(|t| t.trim_start_matches("refs/heads/").to_string()));
        return Head { branch: unborn.map(Into::into), oid: None };
    };
    Head {
        branch: head_ref.is_branch().then(|| head_ref.shorthand().map(|s| s.to_string().into())).flatten(),
        oid: head_ref.target(),
    }
}

/// A glance at a repository for the project bar, without its history.
#[derive(Clone, Debug)]
pub struct Summary {
    pub head: Head,
    pub ahead: usize,
    pub behind: usize,
    /// Changed files, counted like the `// WIP` row.
    pub changes: usize,
    pub state: OpState,
}

impl WorkStatus {
    pub fn changes(&self) -> usize {
        self.staged.len() + self.unstaged.len() + self.conflicted.len()
    }
}

impl Summary {
    pub fn of(snap: &Snapshot) -> Self {
        let (ahead, behind) = snap.branches.iter().find(|b| b.is_head).map_or((0, 0), |b| (b.ahead, b.behind));
        Summary { head: snap.head.clone(), ahead, behind, changes: snap.status.changes(), state: snap.state }
    }
}

pub fn summary(workdir: &Path) -> Result<Summary> {
    let repo = Repository::open(workdir)?;
    let head = read_head(&repo);
    let (ahead, behind) = head
        .branch
        .as_ref()
        .and_then(|b| repo.find_branch(b, BranchType::Local).ok())
        .and_then(|b| {
            let local = b.get().target()?;
            let up = b.upstream().ok()?.get().target()?;
            repo.graph_ahead_behind(local, up).ok()
        })
        .unwrap_or((0, 0));
    Ok(Summary { changes: status(workdir)?.changes(), state: op_state(&repo), head, ahead, behind })
}

pub fn load(workdir: &Path, prev: Option<Arc<History>>) -> Result<Snapshot> {
    let start = Instant::now();
    // Status runs in git while we read refs and walk history.
    let status = std::thread::spawn({
        let dir = workdir.to_path_buf();
        move || status(&dir)
    });
    let repo = Repository::open(workdir)?;
    let name: SharedString = workdir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
        .into();

    let head = read_head(&repo);

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
    let mut peeled = HashMap::new();
    repo.tag_foreach(|oid, name| {
        let name = String::from_utf8_lossy(name).trim_start_matches("refs/tags/").to_string();
        let target = prev.as_ref().and_then(|h| h.peeled.get(&oid).copied()).unwrap_or_else(|| {
            repo.find_object(oid, None)
                .and_then(|o| o.peel_to_commit())
                .map(|c| c.id())
                .unwrap_or(oid)
        });
        peeled.insert(oid, target);
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
        None => Arc::new(walk_history(&repo, tips, peeled)?),
    };
    let status = status.join().map_err(|_| anyhow::anyhow!("git status panicked"))??;

    let mut commits = Vec::with_capacity(history.commits.len() + 1);
    let dirty = !status.is_clean();
    if dirty {
        let n = status.changes();
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
    // The WIP row's only parent is HEAD, which is one of the tips, so the
    // layout depends on nothing but the history and whether the row is there.
    let (graph, graph_width) = history.layouts[dirty as usize]
        .get_or_init(|| {
            let l = graph::layout(commits.iter().map(|c| (c.oid, c.parents.as_slice())));
            (Arc::new(l.rows), l.width)
        })
        .clone();
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
        graph,
        graph_width,
        refs,
        index_of,
        status,
        truncated,
        load_time: start.elapsed(),
        history,
    })
}

/// The newest `MAX_COMMITS` commits reachable from `tips`, children before
/// parents, otherwise newest first (like `git log --date-order`).
///
/// libgit2's sorted walks visit every commit before returning the first
/// (20 s on the Linux kernel without a commit-graph), so this walks newest
/// first from a queue and stops at the limit, as git does, then fixes up
/// any child that clock skew put below its parent.
fn walk_history(repo: &Repository, tips: Vec<Oid>, peeled: HashMap<Oid, Oid>) -> Result<History> {
    let mut queue = BinaryHeap::new();
    let mut found: HashMap<Oid, Commit> = HashMap::new();
    let mut seen = HashSet::new();
    let mut seq = 0u64;
    let mut enqueue = |oid: Oid, queue: &mut BinaryHeap<(i64, Reverse<u64>, Oid)>, found: &mut HashMap<Oid, Commit>| {
        if !seen.insert(oid) {
            return;
        }
        let Ok(c) = repo.find_commit(oid) else { return };
        let author = c.author();
        let time = c.time().seconds();
        found.insert(oid, Commit {
            oid,
            short: oid.to_string()[..7].to_string().into(),
            summary: c.summary_bytes().map(|s| String::from_utf8_lossy(s).into_owned()).unwrap_or_default().into(),
            author: author.name().unwrap_or("").to_string().into(),
            email: author.email().unwrap_or("").to_string().into(),
            time,
            parents: c.parent_ids().collect(),
        });
        // Ties keep the order commits were found in.
        queue.push((time, Reverse(seq), oid));
        seq += 1;
    };
    for t in &tips {
        enqueue(*t, &mut queue, &mut found);
    }
    let mut commits = Vec::with_capacity(1024);
    while let Some((_, _, oid)) = queue.pop() {
        if commits.len() >= MAX_COMMITS {
            break;
        }
        let c = found.remove(&oid).unwrap();
        for p in &c.parents {
            enqueue(*p, &mut queue, &mut found);
        }
        commits.push(c);
    }
    let truncated = !queue.is_empty();
    Ok(History { tips, commits: children_first(commits), truncated, peeled, layouts: Default::default() })
}

/// Reorders `commits` (newest first) so every commit comes after all of its
/// children, keeping the order otherwise.
fn children_first(commits: Vec<Commit>) -> Vec<Commit> {
    let index: HashMap<Oid, usize> = commits.iter().enumerate().map(|(i, c)| (c.oid, i)).collect();
    let mut children = vec![0u32; commits.len()];
    for c in &commits {
        for p in &c.parents {
            if let Some(&j) = index.get(p) {
                children[j] += 1;
            }
        }
    }
    let mut ready: BinaryHeap<Reverse<usize>> = (0..commits.len()).filter(|&i| children[i] == 0).map(Reverse).collect();
    let mut order = Vec::with_capacity(commits.len());
    while let Some(Reverse(i)) = ready.pop() {
        order.push(i);
        for p in &commits[i].parents {
            if let Some(&j) = index.get(p) {
                children[j] -= 1;
                if children[j] == 0 {
                    ready.push(Reverse(j));
                }
            }
        }
    }
    let mut slots: Vec<Option<Commit>> = commits.into_iter().map(Some).collect();
    order.into_iter().map(|i| slots[i].take().unwrap()).collect()
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
