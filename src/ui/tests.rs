//! View-level tests: a `GitPanda` in a headless window, driven by
//! keystrokes and method calls against real throwaway repositories.

use std::time::Duration;

use gpui::TestAppContext;

use crate::git::conflict::Pick;
use crate::git::ops::RebaseAction;
use crate::git::repo::wip_oid;

use super::app::{Modal, ToastKind};
use super::test_support::*;

#[gpui::test]
fn opens_repository(cx: &mut TestAppContext) {
    let r = three_commits();
    let (view, cx) = open(&r, cx);
    view.read_with(cx, |v, _| {
        let s = v.snap.as_ref().expect("snapshot loaded");
        assert_eq!(s.commits.len(), 3);
        assert_eq!(v.workdir.as_ref(), Some(&s.workdir));
        // A clean tree has no WIP row, so HEAD is selected.
        assert_eq!(v.selected, s.head.oid);
        assert!(v.open_error.is_none());
        assert!(!v.loading);
    });
    assert_eq!(selected_summary(&view, cx), "three");
    let title = cx.window_title().unwrap();
    assert!(title.starts_with(r.0.file_name().unwrap().to_str().unwrap()), "{title}");
    assert!(title.ends_with(" — gitpanda"), "{title}");
}

#[gpui::test]
fn dirty_tree_selects_the_wip_row(cx: &mut TestAppContext) {
    let r = three_commits();
    r.write("one.txt", "changed\n");
    let (view, cx) = open(&r, cx);
    view.read_with(cx, |v, _| {
        assert_eq!(v.selected, Some(wip_oid()));
        assert_eq!(v.selected_index(), Some(0));
        // WIP is never part of the commits an operation acts on.
        assert!(v.selected_commits().is_empty());
    });
}

#[gpui::test]
fn opening_a_plain_folder_shows_an_error(cx: &mut TestAppContext) {
    let r = three_commits();
    let (view, cx) = open(&r, cx);
    let plain = std::env::temp_dir().join(format!("gitpanda-plain-{}", std::process::id()));
    std::fs::create_dir_all(&plain).unwrap();
    // With a repository open, a bad path is only a toast.
    view.update(cx, |v, cx| v.open_repo(&plain, cx));
    assert!(view.read_with(cx, |v, _| v.workdir.is_some() && v.open_error.is_none()));
    assert_eq!(view.read_with(cx, |v, _| v.toasts.last().map(|t| t.kind)), Some(ToastKind::Error));
    // Without one, it's the welcome screen's error.
    view.update(cx, |v, cx| {
        v.close_repo(cx);
        v.open_repo(&plain, cx);
    });
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert!(v.workdir.is_none() && v.snap.is_none());
        assert!(v.open_error.as_ref().is_some_and(|e| e.contains("not inside a git repository")), "{:?}", v.open_error);
    });
    let _ = std::fs::remove_dir_all(&plain);
}

#[gpui::test]
fn arrow_keys_move_the_selection(cx: &mut TestAppContext) {
    let r = three_commits();
    let (view, cx) = open(&r, cx);
    cx.simulate_keystrokes("down");
    assert_eq!(selected_summary(&view, cx), "two");
    cx.simulate_keystrokes("j");
    assert_eq!(selected_summary(&view, cx), "one");
    // Stops at the ends.
    cx.simulate_keystrokes("down");
    assert_eq!(selected_summary(&view, cx), "one");
    cx.simulate_keystrokes("k up up");
    assert_eq!(selected_summary(&view, cx), "three");
    cx.simulate_keystrokes("pagedown");
    assert_eq!(selected_summary(&view, cx), "one");
    cx.simulate_keystrokes("pageup");
    assert_eq!(selected_summary(&view, cx), "three");
}

#[gpui::test]
fn shift_arrows_extend_a_range(cx: &mut TestAppContext) {
    let r = three_commits();
    let (view, cx) = open(&r, cx);
    let (one, two, three) = (oid_of(&view, "one", cx), oid_of(&view, "two", cx), oid_of(&view, "three", cx));
    cx.simulate_keystrokes("shift-down");
    view.read_with(cx, |v, _| {
        assert_eq!(v.range, Some((0, 1)));
        // The anchor stays selected.
        assert_eq!(v.selected, Some(three));
        assert_eq!(v.selected_commits(), [three, two]);
    });
    cx.simulate_keystrokes("shift-down");
    assert_eq!(view.read_with(cx, |v, _| v.selected_commits()), [three, two, one]);
    cx.simulate_keystrokes("shift-up");
    assert_eq!(view.read_with(cx, |v, _| v.range), Some((0, 1)));
    // Back onto the anchor: no range left.
    cx.simulate_keystrokes("shift-up");
    assert_eq!(view.read_with(cx, |v, _| v.range), None);

    cx.simulate_keystrokes("shift-down shift-down escape");
    view.read_with(cx, |v, _| {
        assert_eq!(v.range, None);
        assert_eq!(v.selected, Some(three));
    });
    // A plain move drops the range too.
    cx.simulate_keystrokes("shift-down down");
    view.read_with(cx, |v, _| {
        assert_eq!(v.range, None);
        assert_eq!(v.selected, Some(two));
    });
}

#[gpui::test]
fn range_from_the_wip_row_skips_it(cx: &mut TestAppContext) {
    let r = three_commits();
    r.write("one.txt", "changed\n");
    let (view, cx) = open(&r, cx);
    cx.simulate_keystrokes("shift-down shift-down");
    let (two, three) = (oid_of(&view, "two", cx), oid_of(&view, "three", cx));
    assert_eq!(view.read_with(cx, |v, _| v.selected_commits()), [three, two]);
}

#[gpui::test]
fn selection_survives_a_reload(cx: &mut TestAppContext) {
    let r = three_commits();
    let (view, cx) = open(&r, cx);
    cx.simulate_keystrokes("down");
    let two = oid_of(&view, "two", cx);
    r.commit("b.txt", "b\n", "four");
    view.update(cx, |v, cx| v.reload(cx));
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert_eq!(v.snap.as_ref().unwrap().commits.len(), 4);
        assert_eq!(v.selected, Some(two));
    });
}

#[gpui::test]
fn commit_from_the_message_box(cx: &mut TestAppContext) {
    let r = three_commits();
    r.write("four.txt", "four\n");
    let (view, cx) = open(&r, cx);

    // Nothing to commit yet.
    cx.simulate_keystrokes("cmd-enter");
    assert_eq!(toasts(&view, cx), ["Write a commit message first"]);
    view.update(cx, |v, cx| v.commit_input.update(cx, |i, cx| i.set_text("  four  ", cx)));
    cx.simulate_keystrokes("cmd-enter");
    assert_eq!(toasts(&view, cx).last().unwrap(), "Nothing staged. Stage files first, or amend.");

    view.update(cx, |v, cx| v.stage_path("four.txt".into(), cx));
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.snap.as_ref().unwrap().status.staged.len()), 1);
    cx.simulate_keystrokes("cmd-enter");
    cx.run_until_parked();
    assert_eq!(r.log(), ["four", "three", "two", "one"]);
    view.read_with(cx, |v, cx| {
        assert_eq!(v.commit_input.read(cx).text(), "");
        assert!(v.busy.is_none());
        assert_eq!(v.toasts.last().unwrap().kind, ToastKind::Success);
    });
}

#[gpui::test]
fn amend_prefills_the_last_message(cx: &mut TestAppContext) {
    let r = three_commits();
    let (view, cx) = open(&r, cx);
    view.update(cx, |v, cx| v.toggle_amend(cx));
    view.read_with(cx, |v, cx| {
        assert!(v.amend);
        assert_eq!(v.commit_input.read(cx).text().trim(), "three");
    });
    view.update(cx, |v, cx| v.commit_input.update(cx, |i, cx| i.set_text("three, amended", cx)));
    cx.simulate_keystrokes("cmd-enter");
    cx.run_until_parked();
    assert_eq!(r.log(), ["three, amended", "two", "one"]);
    assert!(!view.read_with(cx, |v, _| v.amend));
}

#[gpui::test]
fn switching_repositories_keeps_unsent_messages(cx: &mut TestAppContext) {
    let (a, b) = (three_commits(), three_commits());
    let (view, cx) = open(&a, cx);
    let a_dir = view.read_with(cx, |v, _| v.workdir.clone().unwrap());
    let text = |cx: &mut gpui::VisualTestContext| view.read_with(cx, |v, cx| v.commit_input.read(cx).text().to_string());
    view.update(cx, |v, cx| v.commit_input.update(cx, |i, cx| i.set_text("half-written", cx)));
    view.update(cx, |v, cx| v.open_repo(&b.0, cx));
    cx.run_until_parked();
    assert_eq!(text(cx), "");
    view.read_with(cx, |v, _| {
        assert_ne!(v.workdir.as_ref(), Some(&a_dir));
        // Both are now in the project, and its keyboard shortcuts.
        assert_eq!(v.workspace.project().unwrap().repos.len(), 2);
        assert!(v.selected.is_some() && v.file_view.is_none() && v.modal.is_none());
    });
    cx.simulate_keystrokes("cmd-1");
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.workdir.clone()), Some(a_dir));
    assert_eq!(text(cx), "half-written");
}

#[gpui::test]
fn toasts_are_capped_and_expire(cx: &mut TestAppContext) {
    let r = three_commits();
    let (view, cx) = open(&r, cx);
    view.update(cx, |v, cx| {
        for i in 0..5 {
            v.toast(ToastKind::Info, format!("info {i}"), cx);
        }
        v.toast(ToastKind::Error, "error", cx);
    });
    assert_eq!(toasts(&view, cx), ["info 2", "info 3", "info 4", "error"]);
    cx.executor().advance_clock(Duration::from_millis(4000));
    cx.run_until_parked();
    assert_eq!(toasts(&view, cx), ["error"]);
    cx.executor().advance_clock(Duration::from_millis(4000));
    cx.run_until_parked();
    assert!(toasts(&view, cx).is_empty());
}

#[gpui::test]
fn dropping_a_commit_asks_first(cx: &mut TestAppContext) {
    let r = three_commits();
    let (view, cx) = open(&r, cx);
    cx.simulate_keystrokes("down cmd-backspace");
    view.read_with(cx, |v, _| match &v.modal {
        Some(Modal::Confirm { title, body, danger, .. }) => {
            assert_eq!(title.as_ref(), "Drop commit?");
            assert!(body.contains("\"two\"") && body.contains("main"), "{body}");
            assert!(danger);
        }
        _ => panic!("expected a confirmation"),
    });
    // Escape cancels.
    cx.simulate_keystrokes("escape");
    assert!(view.read_with(cx, |v, _| v.modal.is_none()));
    assert_eq!(r.log(), ["three", "two", "one"]);

    cx.simulate_keystrokes("cmd-backspace enter");
    cx.run_until_parked();
    assert_eq!(r.log(), ["three", "one"]);
    assert_eq!(toasts(&view, cx).last().unwrap(), "Dropped 1 commit");
}

#[gpui::test]
fn dropping_a_range_names_the_count(cx: &mut TestAppContext) {
    let r = three_commits();
    let (view, cx) = open(&r, cx);
    cx.simulate_keystrokes("shift-down cmd-backspace");
    view.read_with(cx, |v, _| match &v.modal {
        Some(Modal::Confirm { title, .. }) => assert_eq!(title.as_ref(), "Drop 2 commits?"),
        _ => panic!("expected a confirmation"),
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(r.log(), ["one"]);
}

#[gpui::test]
fn rebase_editor_keys(cx: &mut TestAppContext) {
    let r = three_commits();
    r.commit("four.txt", "four\n", "four");
    let (view, cx) = open(&r, cx);
    let two = oid_of(&view, "two", cx);
    view.update(cx, |v, cx| v.start_rebase(two, |_| {}, cx));
    let editor = |cx: &mut gpui::VisualTestContext| {
        view.read_with(cx, |v, _| match &v.modal {
            Some(Modal::Rebase(ed)) => (ed.cursor, ed.steps.iter().map(|s| (s.summary.clone(), s.action)).collect::<Vec<_>>()),
            _ => panic!("expected the rebase editor"),
        })
    };
    // Oldest first, with the newest under the cursor.
    let (cursor, steps) = editor(cx);
    assert_eq!(cursor, 2);
    assert_eq!(steps.iter().map(|s| s.0.as_str()).collect::<Vec<_>>(), ["two", "three", "four"]);
    assert!(steps.iter().all(|s| s.1 == RebaseAction::Pick));

    // The list is drawn newest first: down goes back in history.
    cx.simulate_keystrokes("down");
    assert_eq!(editor(cx).0, 1);
    cx.simulate_keystrokes("j j");
    assert_eq!(editor(cx).0, 0);
    cx.simulate_keystrokes("k s");
    assert_eq!(editor(cx).1[1].1, RebaseAction::Squash);
    cx.simulate_keystrokes("f");
    assert_eq!(editor(cx).1[1].1, RebaseAction::Fixup);
    cx.simulate_keystrokes("d p");
    assert_eq!(editor(cx).1[1].1, RebaseAction::Pick);

    // Alt moves the step itself.
    cx.simulate_keystrokes("alt-up");
    let (cursor, steps) = editor(cx);
    assert_eq!(cursor, 2);
    assert_eq!(steps.iter().map(|s| s.0.as_str()).collect::<Vec<_>>(), ["two", "four", "three"]);
    cx.simulate_keystrokes("alt-up");
    assert_eq!(editor(cx).0, 2);

    cx.simulate_keystrokes("f enter");
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.modal.is_none()));
    assert_eq!(r.log(), ["four", "two", "one"]);
    // three, moved above four, was folded into it.
    assert_eq!(r.git(&["show", "--format=", "--name-only", "HEAD"]).split_whitespace().collect::<Vec<_>>(), ["four.txt", "three.txt"]);
}

#[gpui::test]
fn squash_selected_commits(cx: &mut TestAppContext) {
    let r = three_commits();
    let (view, cx) = open(&r, cx);
    view.update(cx, |v, cx| v.squash_selected(cx));
    assert_eq!(toasts(&view, cx).last().unwrap(), "Shift-click to select two or more commits to squash");

    cx.simulate_keystrokes("shift-down");
    view.update(cx, |v, cx| v.squash_selected(cx));
    view.read_with(cx, |v, _| match &v.modal {
        Some(Modal::Rebase(ed)) => {
            let actions: Vec<_> = ed.steps.iter().map(|s| s.action).collect();
            assert_eq!(actions, [RebaseAction::Pick, RebaseAction::Squash]);
            assert_eq!(ed.steps[0].message.as_deref().map(str::trim), Some("two\n\nthree"));
        }
        _ => panic!("expected the rebase editor"),
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(r.log(), ["two", "one"]);
    assert_eq!(r.git(&["log", "-1", "--format=%B"]).trim(), "two\n\nthree");
}

#[gpui::test]
fn reword_head_through_a_prompt(cx: &mut TestAppContext) {
    let r = three_commits();
    let (view, cx) = open(&r, cx);
    let three = oid_of(&view, "three", cx);
    view.update_in(cx, |v, window, cx| v.reword(three, window, cx));
    view.read_with(cx, |v, cx| match &v.modal {
        Some(Modal::Input { title, input, .. }) => {
            assert_eq!(title.as_ref(), "Reword commit");
            assert_eq!(input.read(cx).text().trim(), "three");
        }
        _ => panic!("expected a prompt"),
    });
    // The prefilled text is selected, so typing replaces it.
    cx.simulate_input("third");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(r.log(), ["third", "two", "one"]);
}

#[gpui::test]
fn reword_older_commit_through_the_rebase_editor(cx: &mut TestAppContext) {
    let r = three_commits();
    let (view, cx) = open(&r, cx);
    let two = oid_of(&view, "two", cx);
    view.update_in(cx, |v, window, cx| v.reword(two, window, cx));
    view.read_with(cx, |v, _| match &v.modal {
        Some(Modal::Rebase(ed)) => {
            assert_eq!(ed.cursor, 0);
            assert_eq!(ed.steps[0].action, RebaseAction::Reword);
            assert!(ed.editing.as_ref().is_some_and(|(i, _)| *i == 0));
        }
        _ => panic!("expected the rebase editor"),
    });
    // Enter is a newline in the message; ⌘⏎ finishes editing it.
    cx.simulate_keystrokes("cmd-a");
    cx.simulate_input("second");
    cx.simulate_keystrokes("enter enter");
    cx.simulate_input("body");
    cx.simulate_keystrokes("cmd-enter");
    view.read_with(cx, |v, _| match &v.modal {
        Some(Modal::Rebase(ed)) => {
            assert!(ed.editing.is_none());
            assert_eq!(ed.steps[0].summary, "second");
            assert_eq!(ed.steps[0].message.as_deref(), Some("second\n\nbody"));
        }
        _ => panic!("expected the rebase editor"),
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(r.log(), ["three", "second", "one"]);
    assert_eq!(r.git(&["log", "-1", "--format=%B", "HEAD~1"]).trim(), "second\n\nbody");
}

#[gpui::test]
fn escape_stops_editing_a_rebase_message_first(cx: &mut TestAppContext) {
    let r = three_commits();
    let (view, cx) = open(&r, cx);
    let one = oid_of(&view, "one", cx);
    view.update(cx, |v, cx| v.start_rebase(one, |_| {}, cx));
    cx.simulate_keystrokes("e");
    assert!(view.read_with(cx, |v, _| matches!(&v.modal, Some(Modal::Rebase(ed)) if ed.editing.is_some())));
    cx.simulate_keystrokes("escape");
    assert!(view.read_with(cx, |v, _| matches!(&v.modal, Some(Modal::Rebase(ed)) if ed.editing.is_none())));
    cx.simulate_keystrokes("escape");
    assert!(view.read_with(cx, |v, _| v.modal.is_none()));
    assert_eq!(r.log(), ["three", "two", "one"]);
}

#[gpui::test]
fn new_branch_from_the_keyboard(cx: &mut TestAppContext) {
    let r = three_commits();
    let (view, cx) = open(&r, cx);
    cx.simulate_keystrokes("down cmd-b");
    view.read_with(cx, |v, _| match &v.modal {
        Some(Modal::Input { title, hint, .. }) => {
            assert_eq!(title.as_ref(), "Create branch");
            assert!(hint.as_ref().unwrap().starts_with("Starts at "));
        }
        _ => panic!("expected a prompt"),
    });
    // Spaces become dashes.
    cx.simulate_input("my topic");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.head_branch()).as_deref(), Some("my-topic"));
    assert_eq!(r.git(&["log", "-1", "--format=%s"]).trim(), "two");
}

#[gpui::test]
fn empty_prompt_does_nothing(cx: &mut TestAppContext) {
    let r = three_commits();
    let (view, cx) = open(&r, cx);
    cx.simulate_keystrokes("cmd-b enter");
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.modal.is_none()));
    assert_eq!(view.read_with(cx, |v, _| v.head_branch()).as_deref(), Some("main"));
}

#[gpui::test]
fn stash_from_the_keyboard(cx: &mut TestAppContext) {
    let r = three_commits();
    r.write("one.txt", "dirty\n");
    let (view, cx) = open(&r, cx);
    cx.simulate_keystrokes("cmd-shift-s");
    cx.simulate_input("parked");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        let s = v.snap.as_ref().unwrap();
        assert_eq!(s.stashes.len(), 1);
        assert!(s.status.unstaged.is_empty());
    });
    assert_eq!(r.read("one.txt"), "one\n");
}

#[gpui::test]
fn resolve_a_merge_conflict(cx: &mut TestAppContext) {
    let r = three_commits();
    r.git(&["checkout", "-q", "-b", "topic"]);
    r.commit("a.txt", "theirs\n", "theirs");
    r.git(&["checkout", "-q", "main"]);
    r.commit("a.txt", "ours\n", "ours");
    let _ = crate::git::ops::Git::new(&r.0).run(&["merge", "-q", "topic"]);
    let (view, cx) = open(&r, cx);
    assert_eq!(view.read_with(cx, |v, _| v.snap.as_ref().unwrap().status.conflicted.len()), 1);

    view.update(cx, |v, cx| v.open_conflict("a.txt".into(), cx));
    view.read_with(cx, |v, _| {
        let cv = v.conflict.as_ref().expect("conflict view");
        assert_eq!(cv.file.unresolved(), 1);
        assert!(v.file_view.is_none());
    });
    view.update(cx, |v, cx| v.pick_conflict(0, Some(Pick::Theirs), cx));
    assert_eq!(view.read_with(cx, |v, _| v.conflict.as_ref().unwrap().resolved_line(0, 0).map(str::to_string)).as_deref(), Some("theirs"));
    view.update(cx, |v, cx| v.pick_all_conflicts(Pick::OursThenTheirs, cx));
    view.update(cx, |v, cx| v.save_conflict(cx));
    cx.run_until_parked();
    assert_eq!(r.read("a.txt"), "ours\ntheirs\n");
    view.read_with(cx, |v, _| {
        assert!(v.conflict.is_none());
        assert!(v.snap.as_ref().unwrap().status.conflicted.is_empty());
    });
    assert_eq!(toasts(&view, cx).last().unwrap(), "Resolved a.txt");
}

#[gpui::test]
fn opening_a_missing_conflict_file_is_an_error(cx: &mut TestAppContext) {
    let r = three_commits();
    let (view, cx) = open(&r, cx);
    view.update(cx, |v, cx| v.open_conflict("nope.txt".into(), cx));
    view.read_with(cx, |v, _| {
        assert!(v.conflict.is_none());
        assert_eq!(v.toasts.last().unwrap().kind, ToastKind::Error);
    });
}

#[gpui::test]
fn hard_reset_asks_first(cx: &mut TestAppContext) {
    let r = three_commits();
    let (view, cx) = open(&r, cx);
    let one = oid_of(&view, "one", cx);
    view.update(cx, |v, cx| v.reset_to(one, "soft", cx));
    cx.run_until_parked();
    assert_eq!(r.log(), ["one"]);
    assert_eq!(r.git(&["diff", "--cached", "--name-only"]).split_whitespace().collect::<Vec<_>>(), ["three.txt", "two.txt"]);

    r.git(&["commit", "-q", "-m", "back"]);
    view.update(cx, |v, cx| v.reset_to(one, "hard", cx));
    assert!(view.read_with(cx, |v, _| matches!(v.modal, Some(Modal::Confirm { danger: true, .. }))));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(r.log(), ["one"]);
    assert!(!r.0.join("two.txt").exists());
}

#[gpui::test]
fn push_needs_a_branch(cx: &mut TestAppContext) {
    let r = three_commits();
    r.git(&["checkout", "-q", "--detach"]);
    let (view, cx) = open(&r, cx);
    view.update(cx, |v, cx| v.push(false, cx));
    assert_eq!(toasts(&view, cx), ["HEAD is detached: check out a branch to push"]);
    assert!(view.read_with(cx, |v, _| v.busy.is_none()));
}

#[gpui::test]
fn escape_backs_out_one_level_at_a_time(cx: &mut TestAppContext) {
    let r = three_commits();
    r.write("one.txt", "dirty\n");
    let (view, cx) = open(&r, cx);
    cx.simulate_keystrokes("shift-down");
    view.update(cx, |v, cx| v.open_file(crate::git::diff::DiffSource::Unstaged, "one.txt".into(), crate::git::diff::FileStatus::Modified, cx));
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        let fv = v.file_view.as_ref().unwrap();
        assert!(fv.diff.is_some() && fv.error.is_none());
        assert_eq!(fv.rows.len(), 3);
    });
    cx.simulate_keystrokes("escape");
    view.read_with(cx, |v, _| assert!(v.file_view.is_none() && v.range.is_some()));
    cx.simulate_keystrokes("escape");
    assert!(view.read_with(cx, |v, _| v.range.is_none()));
}

#[gpui::test]
fn copy_sha_from_the_keyboard(cx: &mut TestAppContext) {
    let r = three_commits();
    let (view, cx) = open(&r, cx);
    cx.simulate_keystrokes("cmd-c");
    let three = oid_of(&view, "three", cx);
    assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()), Some(three.to_string()));
    assert_eq!(toasts(&view, cx), ["Copied commit SHA"]);
}

#[gpui::test]
fn commit_detail_follows_the_selection(cx: &mut TestAppContext) {
    let r = three_commits();
    r.commit("b.txt", "b\n", "four");
    let (view, cx) = open(&r, cx);
    let four = oid_of(&view, "four", cx);
    view.read_with(cx, |v, _| {
        let d = v.detail.as_ref().expect("detail loaded");
        assert_eq!(d.oid, four);
        assert_eq!(d.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["b.txt"]);
    });
    cx.simulate_keystrokes("down");
    cx.run_until_parked();
    let three = oid_of(&view, "three", cx);
    assert_eq!(view.read_with(cx, |v, _| v.detail.as_ref().map(|d| d.oid)), Some(three));
}


#[gpui::test]
fn commit_shortcut_keeps_the_keyboard_without_a_message_box(cx: &mut TestAppContext) {
    let r = three_commits();
    let (view, cx) = open(&r, cx);
    cx.simulate_keystrokes("cmd-enter");
    assert_eq!(toasts(&view, cx), ["Write a commit message first"]);
    cx.simulate_keystrokes("down");
    assert_eq!(selected_summary(&view, cx), "two");
}
