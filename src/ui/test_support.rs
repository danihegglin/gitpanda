//! Headless harness for the UI tests: a `GitPanda` view in a test window,
//! open on a throwaway repository.

use git2::Oid;
use gpui::{Entity, TestAppContext, VisualTestContext};

pub(crate) use crate::git::tests::TempRepo;
use crate::workspace::Workspace;

use super::app::{GitPanda, MenuItem};

/// Opens `repo` in a new window, with projects that are never saved, and
/// waits for the first snapshot.
pub(crate) fn open<'a>(repo: &TempRepo, cx: &'a mut TestAppContext) -> (Entity<GitPanda>, &'a mut VisualTestContext) {
    let path = repo.0.clone();
    let (view, cx) = cx.add_window_view(|window, cx| {
        let view = GitPanda::with_workspace(Some(path), Workspace::default(), window, cx);
        window.focus(&view.focus);
        view
    });
    cx.run_until_parked();
    (view, cx)
}

/// A repository with three commits on main, `one`, `two` and `three`, each
/// adding its own file (so they can be reordered or dropped without
/// conflicts), and a clean working tree.
pub(crate) fn three_commits() -> TempRepo {
    let r = TempRepo::new();
    for name in ["one", "two", "three"] {
        r.commit(&format!("{name}.txt"), &format!("{name}\n"), name);
    }
    r
}

/// Menu labels, with separators as "—".
pub(crate) fn labels(items: &[MenuItem]) -> Vec<String> {
    items
        .iter()
        .map(|i| match i {
            MenuItem::Action { label, .. } => label.to_string(),
            MenuItem::Separator => "—".into(),
        })
        .collect()
}

/// Runs the menu item labelled `label`, then waits for what it started.
pub(crate) fn click(view: &Entity<GitPanda>, items: Vec<MenuItem>, label: &str, cx: &mut VisualTestContext) {
    let action = items
        .into_iter()
        .find_map(|i| match i {
            MenuItem::Action { label: l, action, .. } if l.as_ref() == label => Some(action),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no menu item {label:?}"));
    view.update_in(cx, |v, window, cx| action(v, window, cx));
    cx.run_until_parked();
}

/// The oid of the commit whose summary is `summary`.
pub(crate) fn oid_of(view: &Entity<GitPanda>, summary: &str, cx: &mut VisualTestContext) -> Oid {
    view.read_with(cx, |v, _| {
        v.snap.as_ref().unwrap().commits.iter().find(|c| c.summary.as_ref() == summary).map(|c| c.oid)
    })
    .unwrap_or_else(|| panic!("no commit {summary:?}"))
}

/// The summary of the selected commit.
pub(crate) fn selected_summary(view: &Entity<GitPanda>, cx: &mut VisualTestContext) -> String {
    view.read_with(cx, |v, _| {
        let s = v.snap.as_ref().unwrap();
        s.commits[v.selected_index().unwrap()].summary.to_string()
    })
}

/// Texts of the toasts on screen, oldest first.
pub(crate) fn toasts(view: &Entity<GitPanda>, cx: &mut VisualTestContext) -> Vec<String> {
    view.read_with(cx, |v, _| v.toasts.iter().map(|t| t.text.to_string()).collect())
}
