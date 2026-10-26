//! End-to-end tests against real throwaway repositories.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::conflict::{ConflictFile, Pick};
use super::diff::{DiffSource, file_diff};
use super::ops::{Git, RebaseAction, rebase_range};
use super::repo::{self, OpState};

pub(crate) struct TempRepo(pub(crate) PathBuf);

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl TempRepo {
    pub(crate) fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "gitpanda-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let r = TempRepo(dir);
        r.git(&["init", "-q", "-b", "main"]);
        r.git(&["config", "user.name", "Test"]);
        r.git(&["config", "user.email", "t@example.com"]);
        r.git(&["config", "commit.gpgsign", "false"]);
        r
    }

    pub(crate) fn git(&self, args: &[&str]) -> String {
        Git::new(&self.0).run(args).unwrap_or_else(|e| panic!("git {args:?}: {e}"))
    }

    pub(crate) fn write(&self, path: &str, s: &str) {
        std::fs::write(self.0.join(path), s).unwrap();
    }

    pub(crate) fn read(&self, path: &str) -> String {
        std::fs::read_to_string(self.0.join(path)).unwrap()
    }

    pub(crate) fn commit(&self, path: &str, content: &str, msg: &str) {
        self.write(path, content);
        self.git(&["add", path]);
        self.git(&["commit", "-q", "-m", msg]);
    }

    pub(crate) fn log(&self) -> Vec<String> {
        self.git(&["log", "--format=%s"]).lines().map(str::to_string).collect()
    }
}

#[test]
fn snapshot_has_wip_row_and_graph() {
    let r = TempRepo::new();
    r.commit("a.txt", "1\n", "first");
    r.commit("a.txt", "2\n", "second");
    r.write("a.txt", "3\n");
    let s = repo::load(&r.0, None).unwrap();
    assert_eq!(s.commits.len(), 3);
    assert_eq!(s.commits[0].oid, repo::wip_oid());
    assert_eq!(s.graph.len(), 3);
    assert_eq!(s.head.branch.as_ref().map(|b| b.as_ref()), Some("main"));
    assert_eq!(s.status.unstaged.len(), 1);
}

#[test]
fn stage_single_line_of_hunk() {
    let r = TempRepo::new();
    r.commit("f.txt", "a\nb\nc\n", "base");
    r.write("f.txt", "a\nB\nc\nd\n");
    let repo = git2::Repository::open(&r.0).unwrap();
    let f = file_diff(&repo, &DiffSource::Unstaged, "f.txt").unwrap().unwrap();
    assert_eq!(f.hunks.len(), 1);
    // Stage only the added "d" line.
    let d = f.hunks[0].lines.iter().position(|l| l.text.as_ref() == "d").unwrap();
    let sel: HashSet<usize> = [d].into();
    Git::new(&r.0).apply_hunk(&f, &DiffSource::Unstaged, 0, Some(&sel), false).unwrap();
    assert_eq!(r.git(&["show", ":f.txt"]), "a\nb\nc\nd\n");

    // Now unstage it again from the staged diff.
    let f = file_diff(&repo, &DiffSource::Staged, "f.txt").unwrap().unwrap();
    Git::new(&r.0).apply_hunk(&f, &DiffSource::Staged, 0, None, false).unwrap();
    assert_eq!(r.git(&["show", ":f.txt"]), "a\nb\nc\n");
    assert_eq!(r.read("f.txt"), "a\nB\nc\nd\n", "working tree untouched");
}

#[test]
fn discard_hunk() {
    let r = TempRepo::new();
    let base: String = (0..30).map(|i| format!("{i}\n")).collect();
    r.commit("f.txt", &base, "base");
    let edit = |lines: &[(usize, &str)]| -> String {
        (0..30)
            .map(|i| lines.iter().find(|l| l.0 == i).map_or(format!("{i}\n"), |l| format!("{}\n", l.1)))
            .collect()
    };
    let changed = edit(&[(2, "two"), (25, "twentyfive")]);
    r.write("f.txt", &changed);
    let repo = git2::Repository::open(&r.0).unwrap();
    let f = file_diff(&repo, &DiffSource::Unstaged, "f.txt").unwrap().unwrap();
    assert_eq!(f.hunks.len(), 2);
    Git::new(&r.0).apply_hunk(&f, &DiffSource::Unstaged, 1, None, true).unwrap();
    assert_eq!(r.read("f.txt"), edit(&[(2, "two")]));
}

#[test]
fn drop_commits_via_interactive_rebase() {
    let r = TempRepo::new();
    r.commit("a.txt", "a\n", "one");
    r.commit("b.txt", "b\n", "two");
    r.commit("c.txt", "c\n", "three");
    r.commit("d.txt", "d\n", "four");
    let oid = git2::Oid::from_str(r.git(&["rev-parse", "HEAD~2"]).trim()).unwrap();
    let (base, mut steps) = rebase_range(&r.0, oid).unwrap();
    steps[0].action = RebaseAction::Drop;
    steps[2].action = RebaseAction::Drop;
    r.write("a.txt", "dirty\n");
    Git::new(&r.0).interactive_rebase(base, &steps).unwrap();
    assert_eq!(r.log(), ["three", "one"]);
    assert!(!r.0.join("b.txt").exists() && !r.0.join("d.txt").exists());
    // Uncommitted work survives the autostash.
    assert_eq!(r.read("a.txt"), "dirty\n");
}

#[test]
fn squash_and_reword_via_interactive_rebase() {
    let r = TempRepo::new();
    r.commit("f.txt", "1\n", "one");
    r.commit("f.txt", "2\n", "two");
    r.commit("f.txt", "3\n", "three");
    r.commit("f.txt", "4\n", "four");
    let oid = git2::Oid::from_str(r.git(&["rev-parse", "HEAD~2"]).trim()).unwrap();
    let (base, mut steps) = rebase_range(&r.0, oid).unwrap();
    assert!(base.is_some());
    assert_eq!(steps.iter().map(|s| s.summary.as_str()).collect::<Vec<_>>(), ["two", "three", "four"]);
    steps[0].message = Some("two+three".into());
    steps[1].action = RebaseAction::Squash;
    steps[2].action = RebaseAction::Reword;
    steps[2].message = Some("FOUR".into());
    Git::new(&r.0).interactive_rebase(base, &steps).unwrap();
    assert_eq!(r.log(), ["FOUR", "two+three", "one"]);
    assert_eq!(r.read("f.txt"), "4\n");
}

#[test]
fn squash_without_new_message_keeps_both_messages() {
    let r = TempRepo::new();
    r.commit("f.txt", "1\n", "one");
    r.commit("f.txt", "2\n", "two");
    let oid = git2::Oid::from_str(r.git(&["rev-parse", "HEAD~1"]).trim()).unwrap();
    let (base, mut steps) = rebase_range(&r.0, oid).unwrap();
    assert!(base.is_none(), "root commit has no base");
    steps[1].action = RebaseAction::Fixup;
    Git::new(&r.0).interactive_rebase(base, &steps).unwrap();
    assert_eq!(r.log(), ["one"]);
    assert_eq!(r.read("f.txt"), "2\n");
}

#[test]
fn merge_conflict_resolution() {
    let r = TempRepo::new();
    r.commit("f.txt", "top\nmid\nbottom\n", "base");
    r.git(&["checkout", "-q", "-b", "feature"]);
    r.commit("f.txt", "top\nfeature\nbottom\n", "feature change");
    r.git(&["checkout", "-q", "main"]);
    r.commit("f.txt", "top\nmain\nbottom\n", "main change");
    let g = Git::new(&r.0);
    assert!(g.merge("feature").is_err());
    let s = repo::load(&r.0, None).unwrap();
    assert_eq!(s.state, OpState::Merge);
    assert_eq!(s.status.conflicted.len(), 1);

    let mut cf = ConflictFile::parse(&r.read("f.txt"));
    assert_eq!(cf.unresolved(), 1);
    cf.pick_all(Pick::OursThenTheirs);
    g.write_resolved("f.txt", &cf.render()).unwrap();
    g.continue_op(OpState::Merge).unwrap();
    assert_eq!(r.read("f.txt"), "top\nmain\nfeature\nbottom\n");
    let s = repo::load(&r.0, None).unwrap();
    assert_eq!(s.state, OpState::Clean);
    assert_eq!(s.commits[0].parents.len(), 2);
}

#[test]
fn rename_branch_and_detect_file_rename() {
    let r = TempRepo::new();
    r.commit("old.txt", "hello world\nthis is a file\nwith some lines\n", "add");
    let g = Git::new(&r.0);
    g.create_branch("topic", "HEAD", true).unwrap();
    g.rename_branch("topic", "topic2").unwrap();
    g.rename_file("old.txt", "new.txt").unwrap();
    g.commit("rename", false).unwrap();
    let s = repo::load(&r.0, None).unwrap();
    assert_eq!(s.head.branch.as_ref().map(|b| b.as_ref()), Some("topic2"));
    let d = repo::commit_detail(&r.0, s.head.oid.unwrap()).unwrap();
    assert_eq!(d.files.len(), 1);
    assert_eq!(d.files[0].path, "new.txt");
    assert_eq!(d.files[0].old_path.as_deref(), Some("old.txt"));
}

#[test]
fn status_lists_staged_unstaged_untracked_and_renames() {
    let r = TempRepo::new();
    r.commit("a.txt", "1\n", "first");
    r.commit("old name.txt", "x\n", "second");
    r.write("a.txt", "2\n");
    r.git(&["add", "a.txt"]);
    r.write("a.txt", "3\n");
    r.git(&["mv", "old name.txt", "new name.txt"]);
    std::fs::create_dir(r.0.join("dir")).unwrap();
    r.write("dir/u.txt", "u\n");
    let s = repo::status(&r.0).unwrap();
    let names = |v: &[repo::StatusEntry]| v.iter().map(|e| e.path.to_string()).collect::<Vec<_>>();
    assert_eq!(names(&s.staged), ["a.txt", "new name.txt"]);
    assert_eq!(s.staged[1].status, super::diff::FileStatus::Renamed);
    assert_eq!(names(&s.unstaged), ["a.txt", "dir/u.txt"]);
    assert_eq!(s.unstaged[1].status, super::diff::FileStatus::Untracked);
    // Reading status must not rewrite the index (that would wake the watcher).
    let index = std::fs::metadata(r.0.join(".git/index")).unwrap().modified().unwrap();
    repo::status(&r.0).unwrap();
    assert_eq!(std::fs::metadata(r.0.join(".git/index")).unwrap().modified().unwrap(), index);
}

#[test]
fn history_keeps_children_above_parents_despite_clock_skew() {
    let r = TempRepo::new();
    r.commit("a.txt", "1\n", "base");
    // A parent committed "in the future", then a child with a sane clock.
    let env = |t: &str| [("GIT_COMMITTER_DATE", t.to_string()), ("GIT_AUTHOR_DATE", t.to_string())];
    r.write("a.txt", "2\n");
    r.git(&["add", "a.txt"]);
    Git::new(&r.0).run_env(&["commit", "-q", "-m", "skewed parent"], &env("2099-01-01T00:00:00").each_ref().map(|(k, v)| (*k, v.as_str()))).unwrap();
    // A branch at the parent makes the walk start from it too.
    r.git(&["branch", "keep"]);
    r.write("a.txt", "3\n");
    r.git(&["add", "a.txt"]);
    Git::new(&r.0).run_env(&["commit", "-q", "-m", "child"], &env("2020-01-01T00:00:00").each_ref().map(|(k, v)| (*k, v.as_str()))).unwrap();
    let s = repo::load(&r.0, None).unwrap();
    let order: Vec<_> = s.commits.iter().map(|c| c.summary.to_string()).collect();
    assert_eq!(order, ["child", "skewed parent", "base"]);
}

#[test]
fn bare_repository_has_history_refs_and_diffs_but_no_working_tree() {
    let src = TempRepo::new();
    src.commit("a.txt", "1\n", "first");
    src.commit("a.txt", "2\n", "second");
    src.git(&["tag", "v1"]);
    let bare = TempRepo(src.0.with_extension("git"));
    let _ = std::fs::remove_dir_all(&bare.0);
    src.git(&["clone", "-q", "--bare", ".", bare.0.to_str().unwrap()]);

    // Found from the repository or anywhere inside it.
    let dir = repo::discover(&bare.0).unwrap();
    assert_eq!(repo::discover(&bare.0.join("refs")).unwrap(), dir);
    assert_eq!(dir.file_name(), bare.0.file_name());

    let s = repo::load(&dir, None).unwrap();
    assert!(s.bare);
    assert!(s.status.is_clean());
    let log: Vec<_> = s.commits.iter().map(|c| c.summary.to_string()).collect();
    assert_eq!(log, ["second", "first"]);
    assert_eq!(s.head.branch.as_ref().map(|b| b.as_ref()), Some("main"));
    assert!(s.branches.iter().any(|b| b.name.as_ref() == "main" && b.is_head));
    assert_eq!(s.tags.len(), 1);
    assert_eq!(repo::summary(&dir).unwrap().changes, 0);

    let head = s.head.oid.unwrap();
    let d = repo::commit_detail(&dir, head).unwrap();
    assert_eq!((d.files.len(), d.files[0].additions, d.files[0].deletions), (1, 1, 1));
    let r = git2::Repository::open(&dir).unwrap();
    assert!(file_diff(&r, &DiffSource::Commit(head), "a.txt").unwrap().is_some());

    // Ref operations need no working tree.
    let g = Git::new(&dir);
    g.create_branch("topic", "HEAD~1", false).unwrap();
    g.create_tag("v0", "topic").unwrap();
    let s = repo::load(&dir, Some(s.history.clone())).unwrap();
    assert!(s.branches.iter().any(|b| b.name.as_ref() == "topic"));
    assert_eq!(s.tags.len(), 2);
}

#[test]
fn working_tree_is_preferred_over_its_git_dir() {
    let r = TempRepo::new();
    r.commit("a.txt", "1\n", "first");
    let dir = repo::discover(&r.0.join(".git")).unwrap();
    assert!(!repo::load(&dir, None).unwrap().bare);
    assert_eq!(dir, repo::discover(&r.0).unwrap());
}

#[test]
fn hostile_ref_names_are_never_read_as_options() {
    let r = TempRepo::new();
    r.commit("f.txt", "a\n", "base");
    let marker = r.0.join("PWNED");
    // Rebase runs `exec` lines at the top of the work tree. A relative path
    // keeps the name valid on Windows, where a ref can't hold `:` or `\`.
    let evil = "--exec=touch${IFS}PWNED".to_string();
    r.git(&["update-ref", &format!("refs/heads/{evil}"), "HEAD"]);
    r.git(&["update-ref", "refs/tags/--force", "HEAD"]);
    r.commit("f.txt", "b\n", "local");
    let g = Git::new(&r.0);

    for res in [g.rebase_onto(&evil), g.merge(&evil), g.checkout(&evil), g.create_branch("x", &evil, true)] {
        assert!(res.unwrap_err().to_string().contains("would be read as an option"));
    }
    assert!(!marker.exists(), "a ref name ran a command");

    // Checking out the `--force` tag detaches HEAD and keeps local edits.
    r.write("f.txt", "edited\n");
    let tag = repo::load(&r.0, None).unwrap().tags.iter().find(|t| t.0.as_ref() == "--force").unwrap().1;
    assert!(g.checkout_detached(tag).is_err(), "would overwrite the edit");
    assert_eq!(r.read("f.txt"), "edited\n");

    // Hostile refs can still be cleaned up.
    g.delete_tag("--force").unwrap();
    g.delete_branch(&evil).unwrap();
}

#[test]
fn repo_local_fsmonitor_command_does_not_run() {
    let r = TempRepo::new();
    r.commit("f.txt", "a\n", "base");
    let marker = r.0.join("FSMONITOR_RAN");
    r.git(&["config", "core.fsmonitor", &format!("touch '{}'; false", marker.display())]);
    repo::status(&r.0).unwrap();
    Git::new(&r.0).stage_all().unwrap();
    assert!(!marker.exists());
}

#[test]
fn stage_untracked_file_with_newline_in_name() {
    if cfg!(windows) {
        return;
    }
    let r = TempRepo::new();
    r.commit("other.txt", "keep\n", "base");
    r.write("x\n+++ y.txt", "new\n");
    let f = file_diff(&git2::Repository::open(&r.0).unwrap(), &DiffSource::Unstaged, "x\n+++ y.txt").unwrap().unwrap();
    Git::new(&r.0).apply_hunk(&f, &DiffSource::Unstaged, 0, None, false).unwrap();
    let s = repo::status(&r.0).unwrap();
    assert_eq!(s.staged.len(), 1);
    assert_eq!(s.staged[0].path.as_ref(), "x\n+++ y.txt");
}
