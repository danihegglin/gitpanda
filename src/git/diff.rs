//! Diff model, loading via libgit2, and partial-patch construction for
//! staging, unstaging and discarding individual hunks or lines.

use std::collections::HashSet;

use anyhow::Result;
use git2::{Delta, Diff, DiffFindOptions, DiffOptions, Oid, Patch, Repository};
use gpui::SharedString;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Add,
    Del,
    /// "\ No newline at end of file", attached to the line before it.
    NoNewline,
}

#[derive(Clone, Debug)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
    pub text: SharedString,
}

#[derive(Clone, Debug)]
pub struct Hunk {
    pub header: SharedString,
    pub old_start: u32,
    pub new_start: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Deleted,
    Modified,
    Renamed,
    Copied,
    TypeChange,
    Untracked,
    Conflicted,
}

impl FileStatus {
    pub fn from_delta(d: Delta) -> Self {
        match d {
            Delta::Added => Self::Added,
            Delta::Deleted => Self::Deleted,
            Delta::Renamed => Self::Renamed,
            Delta::Copied => Self::Copied,
            Delta::Typechange => Self::TypeChange,
            Delta::Untracked => Self::Untracked,
            Delta::Conflicted => Self::Conflicted,
            _ => Self::Modified,
        }
    }

    pub fn letter(self) -> &'static str {
        match self {
            Self::Added => "A",
            Self::Deleted => "D",
            Self::Modified => "M",
            Self::Renamed => "R",
            Self::Copied => "C",
            Self::TypeChange => "T",
            Self::Untracked => "U",
            Self::Conflicted => "!",
        }
    }
}

#[derive(Clone, Debug)]
pub struct FileDiff {
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileStatus,
    pub binary: bool,
    pub hunks: Vec<Hunk>,
    pub additions: usize,
    pub deletions: usize,
}

/// Which diff is being looked at; decides what patch operations make sense.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffSource {
    /// Index → working tree.
    Unstaged,
    /// HEAD → index.
    Staged,
    /// First parent → commit.
    Commit(Oid),
}

fn opts(path: Option<&str>) -> DiffOptions {
    let mut o = DiffOptions::new();
    o.context_lines(3).ignore_submodules(true);
    if let Some(p) = path {
        o.pathspec(p).disable_pathspec_match(true);
    }
    o
}

fn find_renames(diff: &mut Diff) -> Result<()> {
    let mut f = DiffFindOptions::new();
    f.renames(true).copies(false);
    diff.find_similar(Some(&mut f))?;
    Ok(())
}

pub fn raw_diff<'r>(
    repo: &'r Repository,
    source: &DiffSource,
    path: Option<&str>,
) -> Result<Diff<'r>> {
    let mut o = opts(path);
    let mut diff = match source {
        DiffSource::Unstaged => {
            o.include_untracked(true)
                .recurse_untracked_dirs(true)
                .show_untracked_content(true);
            repo.diff_index_to_workdir(None, Some(&mut o))?
        }
        DiffSource::Staged => {
            let head = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
            repo.diff_tree_to_index(head.as_ref(), None, Some(&mut o))?
        }
        DiffSource::Commit(oid) => {
            let commit = repo.find_commit(*oid)?;
            let tree = commit.tree()?;
            let parent = commit.parent(0).ok().and_then(|p| p.tree().ok());
            repo.diff_tree_to_tree(parent.as_ref(), Some(&tree), Some(&mut o))?
        }
    };
    // A rename can only be found when both sides are in the diff, so it is
    // skipped for a single-path diff (the pathspec hides the other side).
    if path.is_none() {
        find_renames(&mut diff)?;
    }
    Ok(diff)
}

/// All files in a diff, without their hunks (cheap: no content is diffed
/// beyond what libgit2 needs for rename detection).
pub fn file_list(diff: &Diff) -> Vec<FileDiff> {
    diff.deltas()
        .map(|d| {
            let new = d.new_file().path().map(|p| p.to_string_lossy().into_owned());
            let old = d.old_file().path().map(|p| p.to_string_lossy().into_owned());
            let status = FileStatus::from_delta(d.status());
            let path = new.clone().or(old.clone()).unwrap_or_default();
            let old_path = match status {
                FileStatus::Renamed | FileStatus::Copied => old,
                _ => None,
            };
            FileDiff {
                path,
                old_path,
                status,
                binary: false,
                hunks: Vec::new(),
                additions: 0,
                deletions: 0,
            }
        })
        .collect()
}

/// Full diff (with hunks) of one file.
pub fn file_diff(repo: &Repository, source: &DiffSource, path: &str) -> Result<Option<FileDiff>> {
    let diff = raw_diff(repo, source, Some(path))?;
    for i in 0..diff.deltas().len() {
        let Some(patch) = Patch::from_diff(&diff, i)? else {
            // Binary files produce no patch.
            let mut f = file_list(&diff).swap_remove(i);
            f.binary = true;
            return Ok(Some(f));
        };
        let delta = patch.delta();
        let path_new = delta.new_file().path().map(|p| p.to_string_lossy().into_owned());
        let path_old = delta.old_file().path().map(|p| p.to_string_lossy().into_owned());
        let status = FileStatus::from_delta(delta.status());
        let binary = delta.flags().is_binary();
        let mut out = FileDiff {
            path: path_new.clone().or(path_old.clone()).unwrap_or_default(),
            old_path: if path_old != path_new { path_old } else { None },
            status,
            binary,
            hunks: Vec::with_capacity(patch.num_hunks()),
            additions: 0,
            deletions: 0,
        };
        for h in 0..patch.num_hunks() {
            let (hunk, n) = patch.hunk(h)?;
            let header = String::from_utf8_lossy(hunk.header()).trim_end().to_string();
            let mut lines = Vec::with_capacity(n);
            for l in 0..n {
                let line = patch.line_in_hunk(h, l)?;
                let kind = match line.origin() {
                    '+' => LineKind::Add,
                    '-' => LineKind::Del,
                    '>' | '<' | '=' => LineKind::NoNewline,
                    _ => LineKind::Context,
                };
                match kind {
                    LineKind::Add => out.additions += 1,
                    LineKind::Del => out.deletions += 1,
                    _ => {}
                }
                let mut text = String::from_utf8_lossy(line.content()).into_owned();
                if kind == LineKind::NoNewline {
                    text = "\\ No newline at end of file".into();
                } else if text.ends_with('\n') {
                    text.pop();
                    if text.ends_with('\r') {
                        text.pop();
                    }
                }
                lines.push(DiffLine {
                    kind,
                    old_no: line.old_lineno(),
                    new_no: line.new_lineno(),
                    text: text.into(),
                });
            }
            out.hunks.push(Hunk {
                header: header.into(),
                old_start: hunk.old_start(),
                new_start: hunk.new_start(),
                lines,
            });
        }
        return Ok(Some(out));
    }
    Ok(None)
}

/// Direction a partial patch will be applied in. It decides what happens to
/// the lines that were *not* selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Apply {
    /// `git apply` (staging an unstaged change).
    Forward,
    /// `git apply -R` (unstaging a staged change, discarding an unstaged one).
    Reverse,
}

/// Build a patch for one hunk of `file`, keeping only the change lines whose
/// index (within the hunk) is in `selected` (`None` keeps all of them).
///
/// Unselected changes must survive on the side the patch is applied to: going
/// forward the old side is the preimage, so an unselected `-` stays as context
/// and an unselected `+` vanishes; in reverse the new side is the preimage, so
/// it is the other way round. Hunk counts are left for `git apply --recount`.
pub fn hunk_patch(
    file: &FileDiff,
    hunk: usize,
    selected: Option<&HashSet<usize>>,
    dir: Apply,
) -> Option<String> {
    let h = file.hunks.get(hunk)?;
    let old_path = file.old_path.as_deref().unwrap_or(&file.path);
    let mut body = String::new();
    let mut any_change = false;
    let mut last_kept = true;
    let (mut old_n, mut new_n) = (0u32, 0u32);
    for (i, l) in h.lines.iter().enumerate() {
        let keep = selected.is_none_or(|s| s.contains(&i));
        let (prefix, kept) = match (l.kind, keep, dir) {
            (LineKind::Context, _, _) => (' ', true),
            (LineKind::NoNewline, _, _) => {
                if last_kept {
                    body.push_str("\\ No newline at end of file\n");
                }
                continue;
            }
            (LineKind::Add, true, _) => ('+', true),
            (LineKind::Del, true, _) => ('-', true),
            (LineKind::Add, false, Apply::Forward) => (' ', false),
            (LineKind::Del, false, Apply::Forward) => (' ', true),
            (LineKind::Add, false, Apply::Reverse) => (' ', true),
            (LineKind::Del, false, Apply::Reverse) => (' ', false),
        };
        // The (' ', false) arms mean "drop this line".
        last_kept = kept;
        if !kept {
            continue;
        }
        if keep && matches!(l.kind, LineKind::Add | LineKind::Del) {
            any_change = true;
        }
        match prefix {
            ' ' => {
                old_n += 1;
                new_n += 1;
            }
            '-' => old_n += 1,
            _ => new_n += 1,
        }
        body.push(prefix);
        body.push_str(&l.text);
        body.push('\n');
    }
    if !any_change {
        return None;
    }
    let (old_hdr, new_hdr) = match file.status {
        FileStatus::Added | FileStatus::Untracked => ("/dev/null".to_string(), format!("b/{}", file.path)),
        FileStatus::Deleted => (format!("a/{old_path}"), "/dev/null".to_string()),
        _ => (format!("a/{old_path}"), format!("b/{}", file.path)),
    };
    let mut out = format!("diff --git a/{old_path} b/{}\n", file.path);
    match file.status {
        FileStatus::Added | FileStatus::Untracked => out.push_str("new file mode 100644\n"),
        FileStatus::Deleted => out.push_str("deleted file mode 100644\n"),
        _ => {}
    }
    out.push_str(&format!("--- {old_hdr}\n+++ {new_hdr}\n"));
    out.push_str(&format!(
        "@@ -{},{} +{},{} @@\n",
        h.old_start, old_n, h.new_start, new_n
    ));
    out.push_str(&body);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(kind: LineKind, text: &str) -> DiffLine {
        DiffLine { kind, old_no: None, new_no: None, text: text.to_string().into() }
    }

    fn file() -> FileDiff {
        FileDiff {
            path: "f.txt".into(),
            old_path: None,
            status: FileStatus::Modified,
            binary: false,
            additions: 2,
            deletions: 1,
            hunks: vec![Hunk {
                header: "@@ -1,3 +1,4 @@".into(),
                old_start: 1,
                new_start: 1,
                lines: vec![
                    line(LineKind::Context, "a"),
                    line(LineKind::Del, "b"),
                    line(LineKind::Add, "B"),
                    line(LineKind::Add, "new"),
                    line(LineKind::Context, "c"),
                ],
            }],
        }
    }

    #[test]
    fn whole_hunk() {
        let p = hunk_patch(&file(), 0, None, Apply::Forward).unwrap();
        assert!(p.ends_with("@@ -1,3 +1,4 @@\n a\n-b\n+B\n+new\n c\n"), "{p}");
    }

    #[test]
    fn selected_lines_forward_keeps_unselected_deletions_as_context() {
        let sel: HashSet<usize> = [3].into();
        let p = hunk_patch(&file(), 0, Some(&sel), Apply::Forward).unwrap();
        assert!(p.ends_with("@@ -1,3 +1,4 @@\n a\n b\n+new\n c\n"), "{p}");
    }

    #[test]
    fn selected_lines_reverse_keeps_unselected_additions_as_context() {
        let sel: HashSet<usize> = [1].into();
        let p = hunk_patch(&file(), 0, Some(&sel), Apply::Reverse).unwrap();
        assert!(p.ends_with("@@ -1,5 +1,4 @@\n a\n-b\n B\n new\n c\n"), "{p}");
    }

    #[test]
    fn selecting_only_context_is_no_patch() {
        let sel: HashSet<usize> = [0].into();
        assert!(hunk_patch(&file(), 0, Some(&sel), Apply::Forward).is_none());
    }
}
