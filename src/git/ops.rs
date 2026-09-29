//! Mutating operations. These go through the `git` CLI so that hooks,
//! config, credentials and every edge case behave exactly like the user's
//! own git; reads stay on libgit2 where speed matters.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result, anyhow, bail};
use git2::{Oid, Repository};

use super::diff::{Apply, DiffSource, FileDiff, hunk_patch};
use std::collections::HashSet;

pub struct Git<'a> {
    pub dir: &'a Path,
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

impl<'a> Git<'a> {
    pub fn new(dir: &'a Path) -> Self {
        Self { dir }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new("git");
        c.current_dir(self.dir)
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            // Never block on an editor: messages come from us.
            .env("GIT_EDITOR", "true")
            .env("GIT_MERGE_AUTOEDIT", "no")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        c
    }

    fn finish(&self, args: &[&str], out: std::process::Output) -> Result<String> {
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        if out.status.success() {
            return Ok(stdout);
        }
        let stderr = String::from_utf8_lossy(&out.stderr);
        let msg = [stderr.trim(), stdout.trim()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        let msg = msg
            .lines()
            .filter(|l| !l.starts_with("hint:"))
            .collect::<Vec<_>>()
            .join("\n");
        Err(anyhow!(if msg.is_empty() {
            format!("git {} failed", args.first().unwrap_or(&""))
        } else {
            msg
        }))
    }

    pub fn run(&self, args: &[&str]) -> Result<String> {
        let out = self.command(args).output().context("could not run git")?;
        self.finish(args, out)
    }

    pub fn run_env(&self, args: &[&str], env: &[(&str, &str)]) -> Result<String> {
        let mut c = self.command(args);
        for (k, v) in env {
            c.env(k, v);
        }
        let out = c.output().context("could not run git")?;
        self.finish(args, out)
    }

    pub fn run_stdin(&self, args: &[&str], input: &str) -> Result<String> {
        let mut c = self.command(args);
        c.stdin(Stdio::piped());
        let mut child = c.spawn().context("could not run git")?;
        child.stdin.take().unwrap().write_all(input.as_bytes())?;
        let out = child.wait_with_output()?;
        self.finish(args, out)
    }

    fn git_dir(&self) -> Result<PathBuf> {
        Ok(Repository::open(self.dir)?.path().to_path_buf())
    }

    // ---- staging ---------------------------------------------------------

    pub fn stage(&self, paths: &[&str]) -> Result<()> {
        let mut args = vec!["add", "-A", "--"];
        args.extend_from_slice(paths);
        self.run(&args).map(drop)
    }

    pub fn stage_all(&self) -> Result<()> {
        self.run(&["add", "-A"]).map(drop)
    }

    pub fn unstage(&self, paths: &[&str]) -> Result<()> {
        let has_head = Repository::open(self.dir)?.head().is_ok();
        let mut args = if has_head {
            vec!["reset", "-q", "HEAD", "--"]
        } else {
            vec!["rm", "--cached", "-r", "-q", "--"]
        };
        args.extend_from_slice(paths);
        self.run(&args).map(drop)
    }

    pub fn unstage_all(&self) -> Result<()> {
        let has_head = Repository::open(self.dir)?.head().is_ok();
        if has_head {
            self.run(&["reset", "-q"]).map(drop)
        } else {
            self.run(&["rm", "--cached", "-r", "-q", "."]).map(drop)
        }
    }

    /// Throw away unstaged changes to `path` (deleting it if untracked).
    pub fn discard(&self, path: &str, untracked: bool) -> Result<()> {
        if untracked {
            let full = self.dir.join(path);
            if full.is_dir() {
                std::fs::remove_dir_all(full)?;
            } else {
                std::fs::remove_file(full)?;
            }
            Ok(())
        } else {
            self.run(&["checkout", "--", path]).map(drop)
        }
    }

    /// Stage, unstage or discard one hunk, or selected lines of it.
    pub fn apply_hunk(
        &self,
        file: &FileDiff,
        source: &DiffSource,
        hunk: usize,
        lines: Option<&HashSet<usize>>,
        discard: bool,
    ) -> Result<()> {
        let (dir, args): (Apply, &[&str]) = match (source, discard) {
            (DiffSource::Unstaged, false) => (Apply::Forward, &["apply", "--cached", "--recount", "--whitespace=nowarn", "-"]),
            (DiffSource::Unstaged, true) => (Apply::Reverse, &["apply", "-R", "--recount", "--whitespace=nowarn", "-"]),
            (DiffSource::Staged, false) => (Apply::Reverse, &["apply", "--cached", "-R", "--recount", "--whitespace=nowarn", "-"]),
            _ => bail!("nothing to apply for this diff"),
        };
        let patch = hunk_patch(file, hunk, lines, dir).context("no changed lines selected")?;
        self.run_stdin(args, &patch).map(drop)
    }

    pub fn rename_file(&self, from: &str, to: &str) -> Result<()> {
        self.run(&["mv", "--", from, to]).map(drop)
    }

    // ---- commits ---------------------------------------------------------

    pub fn commit(&self, message: &str, amend: bool) -> Result<String> {
        let mut args = vec!["commit", "-F", "-", "--cleanup=strip"];
        if amend {
            args.push("--amend");
        }
        self.run_stdin(&args, message)
    }

    pub fn head_message(&self) -> Result<String> {
        self.run(&["log", "-1", "--format=%B"]).map(|s| s.trim_end().to_string())
    }

    pub fn cherry_pick(&self, oid: Oid) -> Result<()> {
        self.run(&["cherry-pick", &oid.to_string()]).map(drop)
    }

    pub fn revert(&self, oid: Oid) -> Result<()> {
        self.run(&["revert", "--no-edit", &oid.to_string()]).map(drop)
    }

    pub fn reset(&self, oid: Oid, mode: &str) -> Result<()> {
        self.run(&["reset", &format!("--{mode}"), &oid.to_string()]).map(drop)
    }

    // ---- branches & tags -------------------------------------------------

    pub fn checkout(&self, target: &str) -> Result<()> {
        self.run(&["checkout", target]).map(drop)
    }

    /// Check out a remote branch: switch to the local branch tracking it,
    /// creating one if needed.
    pub fn checkout_remote(&self, remote_branch: &str) -> Result<()> {
        let local = remote_branch.split_once('/').map_or(remote_branch, |(_, b)| b);
        let exists = Repository::open(self.dir)?
            .find_branch(local, git2::BranchType::Local)
            .is_ok();
        if exists {
            self.checkout(local)
        } else {
            self.run(&["checkout", "--track", remote_branch]).map(drop)
        }
    }

    pub fn create_branch(&self, name: &str, at: &str, checkout: bool) -> Result<()> {
        if checkout {
            self.run(&["checkout", "-b", name, at]).map(drop)
        } else {
            self.run(&["branch", name, at]).map(drop)
        }
    }

    pub fn rename_branch(&self, old: &str, new: &str) -> Result<()> {
        self.run(&["branch", "-m", old, new]).map(drop)
    }

    pub fn delete_branch(&self, name: &str) -> Result<()> {
        self.run(&["branch", "-D", name]).map(drop)
    }

    pub fn delete_remote_branch(&self, remote_branch: &str) -> Result<()> {
        let (remote, branch) = remote_branch
            .split_once('/')
            .context("not a remote branch")?;
        self.run(&["push", remote, "--delete", branch]).map(drop)
    }

    pub fn create_tag(&self, name: &str, at: &str) -> Result<()> {
        self.run(&["tag", name, at]).map(drop)
    }

    pub fn delete_tag(&self, name: &str) -> Result<()> {
        self.run(&["tag", "-d", name]).map(drop)
    }

    pub fn merge(&self, what: &str) -> Result<()> {
        self.run(&["merge", "--no-edit", what]).map(drop)
    }

    pub fn rebase_onto(&self, onto: &str) -> Result<()> {
        self.run(&["rebase", "--autostash", onto]).map(drop)
    }

    // ---- remotes & stash -------------------------------------------------

    pub fn fetch(&self) -> Result<()> {
        self.run(&["fetch", "--all", "--prune"]).map(drop)
    }

    pub fn pull(&self) -> Result<()> {
        self.run(&["pull", "--autostash"]).map(drop)
    }

    pub fn push(&self, branch: &str, has_upstream: bool, force: bool) -> Result<()> {
        let mut args = vec!["push"];
        if force {
            args.push("--force-with-lease");
        }
        if !has_upstream {
            let remote = self.run(&["remote"])?;
            let remote = remote.lines().next().context("no remote configured")?.to_string();
            args.extend(["-u", &remote, branch]);
            return self.run(&args).map(drop);
        }
        self.run(&args).map(drop)
    }

    pub fn stash(&self, message: &str) -> Result<()> {
        let mut args = vec!["stash", "push", "--include-untracked"];
        if !message.is_empty() {
            args.extend(["-m", message]);
        }
        self.run(&args).map(drop)
    }

    pub fn stash_pop(&self, index: usize) -> Result<()> {
        self.run(&["stash", "pop", &format!("stash@{{{index}}}")]).map(drop)
    }

    pub fn stash_apply(&self, index: usize) -> Result<()> {
        self.run(&["stash", "apply", &format!("stash@{{{index}}}")]).map(drop)
    }

    pub fn stash_drop(&self, index: usize) -> Result<()> {
        self.run(&["stash", "drop", &format!("stash@{{{index}}}")]).map(drop)
    }

    // ---- in-progress operations -------------------------------------------

    pub fn continue_op(&self, state: super::repo::OpState) -> Result<()> {
        use super::repo::OpState::*;
        match state {
            Rebase => self.run(&["rebase", "--continue"]).map(drop),
            Merge => self.run(&["commit", "--no-edit"]).map(drop),
            CherryPick => self.run(&["cherry-pick", "--continue"]).map(drop),
            Revert => self.run(&["revert", "--continue"]).map(drop),
            _ => bail!("nothing to continue"),
        }
    }

    pub fn abort_op(&self, state: super::repo::OpState) -> Result<()> {
        use super::repo::OpState::*;
        match state {
            Rebase => self.run(&["rebase", "--abort"]).map(drop),
            Merge => self.run(&["merge", "--abort"]).map(drop),
            CherryPick => self.run(&["cherry-pick", "--abort"]).map(drop),
            Revert => self.run(&["revert", "--abort"]).map(drop),
            Bisect => self.run(&["bisect", "reset"]).map(drop),
            _ => bail!("nothing to abort"),
        }
    }

    pub fn skip_op(&self, state: super::repo::OpState) -> Result<()> {
        use super::repo::OpState::*;
        match state {
            Rebase => self.run(&["rebase", "--skip"]).map(drop),
            CherryPick => self.run(&["cherry-pick", "--skip"]).map(drop),
            Revert => self.run(&["revert", "--skip"]).map(drop),
            _ => bail!("nothing to skip"),
        }
    }

    /// Resolve a conflicted file wholesale to one side.
    pub fn take_side(&self, path: &str, ours: bool) -> Result<()> {
        self.run(&["checkout", if ours { "--ours" } else { "--theirs" }, "--", path])?;
        self.stage(&[path])
    }

    pub fn write_resolved(&self, path: &str, content: &str) -> Result<()> {
        std::fs::write(self.dir.join(path), content)?;
        self.stage(&[path])
    }

    // ---- interactive rebase ----------------------------------------------

    /// Run an interactive rebase of `steps` (oldest first) onto `base`
    /// (`None` rebases from the root). The todo list is handed to git by
    /// making the sequence editor copy our file over git's.
    pub fn interactive_rebase(&self, base: Option<Oid>, steps: &[RebaseStep]) -> Result<()> {
        let git_dir = self.git_dir()?;
        let todo_path = git_dir.join("gitpanda-rebase-todo");
        let mut todo = String::new();
        let mut i = 0;
        while i < steps.len() {
            let s = &steps[i];
            todo.push_str(&format!("{} {}\n", s.action.word(), s.oid));
            // A new message applies once the step's squash/fixup group ends.
            let mut end = i + 1;
            while end < steps.len()
                && matches!(steps[end].action, RebaseAction::Squash | RebaseAction::Fixup)
                && s.action != RebaseAction::Drop
            {
                todo.push_str(&format!("{} {}\n", steps[end].action.word(), steps[end].oid));
                end += 1;
            }
            if let Some(msg) = steps[i..end].iter().rev().find_map(|s| s.message.as_ref()) {
                if s.action != RebaseAction::Drop {
                    let msg_path = git_dir.join(format!("gitpanda-msg-{i}"));
                    std::fs::write(&msg_path, msg)?;
                    todo.push_str(&format!(
                        "exec git commit --amend --allow-empty --only --cleanup=strip -F {}\n",
                        quote(&msg_path.to_string_lossy())
                    ));
                }
            }
            i = end;
        }
        if steps.first().is_some_and(|s| matches!(s.action, RebaseAction::Squash | RebaseAction::Fixup)) {
            bail!("the oldest commit can't be squashed: there is nothing below it to squash into");
        }
        std::fs::write(&todo_path, todo)?;
        let editor = format!("cp {}", quote(&todo_path.to_string_lossy()));
        let base = base.map(|b| b.to_string());
        let mut args = vec!["rebase", "-i", "--autostash"];
        match &base {
            Some(b) => args.push(b),
            None => args.push("--root"),
        }
        let r = self.run_env(&args, &[("GIT_SEQUENCE_EDITOR", &editor)]);
        let _ = std::fs::remove_file(&todo_path);
        r.map(drop)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RebaseAction {
    Pick,
    Reword,
    Squash,
    Fixup,
    Drop,
}

impl RebaseAction {
    pub const ALL: [RebaseAction; 5] = [Self::Pick, Self::Reword, Self::Squash, Self::Fixup, Self::Drop];

    /// The todo verb. Rewording is a pick followed by an amend with our
    /// message, so git itself never needs an editor.
    fn word(self) -> &'static str {
        match self {
            Self::Pick | Self::Reword => "pick",
            Self::Squash => "squash",
            Self::Fixup => "fixup",
            Self::Drop => "drop",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Pick => "pick",
            Self::Reword => "reword",
            Self::Squash => "squash",
            Self::Fixup => "fixup",
            Self::Drop => "drop",
        }
    }
}

#[derive(Clone, Debug)]
pub struct RebaseStep {
    pub oid: Oid,
    pub short: String,
    pub summary: String,
    pub message: Option<String>,
    pub action: RebaseAction,
}

/// Commits from `oldest` up to HEAD along first parents, oldest first, plus
/// the base to rebase onto (`None` when `oldest` is a root commit). Fails if
/// `oldest` isn't a first-parent ancestor of HEAD or a merge is in the way.
pub fn rebase_range(dir: &Path, oldest: Oid) -> Result<(Option<Oid>, Vec<RebaseStep>)> {
    let repo = Repository::open(dir)?;
    let mut c = repo.head()?.peel_to_commit()?;
    let mut out = Vec::new();
    loop {
        if c.parent_count() > 1 {
            bail!("a merge commit ({}) is in the way; rebasing it would flatten history", &c.id().to_string()[..7]);
        }
        out.push(RebaseStep {
            oid: c.id(),
            short: c.id().to_string()[..7].to_string(),
            summary: c.summary().unwrap_or("").to_string(),
            message: None,
            action: RebaseAction::Pick,
        });
        if c.id() == oldest {
            break;
        }
        c = match c.parent(0) {
            Ok(p) => p,
            Err(_) => bail!("that commit isn't on the current branch"),
        };
        if out.len() > 5000 {
            bail!("that commit is too far back");
        }
    }
    let base = repo.find_commit(oldest)?.parent_id(0).ok();
    out.reverse();
    Ok((base, out))
}

/// Full message of a commit, for editing.
pub fn commit_message(dir: &Path, oid: Oid) -> Result<String> {
    let repo = Repository::open(dir)?;
    let c = repo.find_commit(oid)?;
    Ok(c.message().unwrap_or("").trim_end().to_string())
}
