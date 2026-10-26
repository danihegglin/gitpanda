//! The diff view (with hunk and line staging) and the conflict resolver.
//! Both are flattened into equally tall rows for a virtualized list.

use gpui::{
    AnyElement, ClickEvent, Context, SharedString, Window, div, prelude::*, px, uniform_list,
};

use crate::git::conflict::{Pick, Segment};
use crate::git::diff::{DiffSource, LineKind};

use super::app::{ConflictRow, DiffRow, ToastKind, GitPanda, action};
use super::theme::*;
use super::widgets::*;

const LINE_H: f32 = 22.;

fn expand_tabs(s: &str) -> SharedString {
    if s.contains('\t') { s.replace('\t', "    ").into() } else { s.to_string().into() }
}

fn top_bar() -> gpui::Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(10.))
        .h(px(44.))
        .px(px(12.))
        .bg(c(BG_DARK))
        .border_b_1()
        .border_color(c(BORDER))
}

impl GitPanda {
    pub fn render_diff(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let fv = self.file_view.as_ref().unwrap();
        let source = fv.source.clone();
        let path = fv.path.clone();
        let status = fv.status;
        let (adds, dels) = fv.diff.as_ref().map_or((0, 0), |d| (d.additions, d.deletions));
        let old_path = fv.diff.as_ref().and_then(|d| d.old_path.clone());
        let untracked = status == crate::git::diff::FileStatus::Untracked;
        let side = match source {
            DiffSource::Unstaged => pill("unstaged", YELLOW, false),
            DiffSource::Staged => pill("staged", GREEN, false),
            DiffSource::Commit(o) => pill(o.to_string()[..7].to_string(), BLUE, false),
        };

        let (p1, p2, p3) = (path.clone(), path.clone(), path.clone());
        let bar = top_bar()
            .child(
                button("back", "← Graph", Tone::Ghost).on_click(cx.listener(|this, _, window, cx| {
                    this.file_view = None;
                    window.focus(&this.focus);
                    cx.notify();
                })),
            )
            .child(status_badge(status.letter()))
            .child(
                div()
                    .flex()
                    .min_w_0()
                    .gap(px(6.))
                    .items_center()
                    .children(old_path.map(|o| div().text_color(c(COMMENT)).child(format!("{o} →"))))
                    .child(div().truncate().font_weight(gpui::FontWeight::SEMIBOLD).child(path.clone())),
            )
            .child(side)
            .child(div().text_size(px(11.)).font_family(MONO_FONT).text_color(c(GREEN)).child(format!("+{adds}")))
            .child(div().text_size(px(11.)).font_family(MONO_FONT).text_color(c(RED)).child(format!("−{dels}")))
            .child(div().flex_1())
            .when(source == DiffSource::Unstaged, |d| {
                d.child(button("discard-file", "Discard file", Tone::Danger).on_click(cx.listener(move |this, _, _, cx| {
                    this.discard_path(p1.clone(), untracked, cx)
                })))
                .child(button("stage-file", "Stage file", Tone::Primary).on_click(cx.listener(move |this, _, _, cx| {
                    this.stage_path(p2.clone(), cx)
                })))
            })
            .when(source == DiffSource::Staged, |d| {
                d.child(button("unstage-file", "Unstage file", Tone::Normal).on_click(cx.listener(move |this, _, _, cx| {
                    this.unstage_path(p3.clone(), cx)
                })))
            });

        let body = if let Some(err) = &fv.error {
            div().flex().flex_1().items_center().justify_center().text_color(c(COMMENT)).child(err.clone()).into_any_element()
        } else if fv.diff.as_ref().is_some_and(|d| d.binary) {
            div().flex().flex_1().items_center().justify_center().text_color(c(COMMENT)).child("Binary file").into_any_element()
        } else if fv.diff.is_none() {
            div().flex().flex_1().items_center().justify_center().text_color(c(COMMENT)).child("Loading…").into_any_element()
        } else {
            let n = fv.rows.len();
            uniform_list(
                "diff",
                n,
                cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                    range.map(|i| this.render_diff_row(i, cx)).collect::<Vec<_>>()
                }),
            )
            .track_scroll(self.diff_scroll.clone())
            .flex_1()
            .into_any_element()
        };

        let hint = match source {
            DiffSource::Commit(_) => None,
            _ => Some("Click lines to select them (shift-click for a range), then stage just those lines from the hunk header."),
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(c(BG))
            .child(bar)
            .child(body)
            .children(hint.filter(|_| !untracked).map(|h| {
                div().flex_none().px(px(12.)).py(px(6.)).text_size(px(11.)).text_color(c(COMMENT)).border_t_1().border_color(c(BORDER)).child(h)
            }))
            .into_any_element()
    }

    fn render_diff_row(&mut self, i: usize, cx: &mut Context<Self>) -> AnyElement {
        let fv = self.file_view.as_ref().unwrap();
        let diff = fv.diff.as_ref().unwrap();
        let editable = !matches!(fv.source, DiffSource::Commit(_));
        // Hunks of untracked files can't be applied until the file is known
        // to the index; they're staged whole.
        let hunk_ops = editable && fv.status != crate::git::diff::FileStatus::Untracked;
        match fv.rows[i] {
            DiffRow::Hunk(h) => {
                let hunk = &diff.hunks[h];
                let sel = fv.selected_in(h).len();
                let what = if sel > 0 { format!("{sel} line{}", if sel == 1 { "" } else { "s" }) } else { "hunk".into() };
                let lines_only = sel > 0;
                let staged = fv.source == DiffSource::Staged;
                div()
                    .id(("hunk", i))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(px(LINE_H))
                    .w_full()
                    .px(px(12.))
                    .bg(c(BG_HIGHLIGHT))
                    .border_t_1()
                    .border_color(c(BORDER_HI))
                    .child(div().flex_1().truncate().font_family(MONO_FONT).text_size(px(11.5)).text_color(c(BLUE1)).child(hunk.header.clone()))
                    .when(hunk_ops && !staged, |d| {
                        d.child(mini_button(("dh", i), format!("Discard {what}"), RED).on_click(cx.listener(move |this, _, _, cx| this.hunk_op(h, lines_only, true, cx))))
                            .child(mini_button(("sh", i), format!("Stage {what}"), GREEN).on_click(cx.listener(move |this, _, _, cx| this.hunk_op(h, lines_only, false, cx))))
                    })
                    .when(hunk_ops && staged, |d| {
                        d.child(mini_button(("uh", i), format!("Unstage {what}"), YELLOW).on_click(cx.listener(move |this, _, _, cx| this.hunk_op(h, lines_only, false, cx))))
                    })
                    .into_any_element()
            }
            DiffRow::Line(h, l) => {
                let line = &diff.hunks[h].lines[l];
                let selected = fv.selected.contains(&(h, l));
                let (bg, sign, sign_color) = match line.kind {
                    LineKind::Add => (if selected { DIFF_ADD_HI } else { DIFF_ADD }, "+", GREEN),
                    LineKind::Del => (if selected { DIFF_DEL_HI } else { DIFF_DEL }, "−", RED),
                    LineKind::Context => (BG, " ", COMMENT),
                    LineKind::NoNewline => (BG, " ", COMMENT),
                };
                let changeable = hunk_ops && matches!(line.kind, LineKind::Add | LineKind::Del);
                let num = |n: Option<u32>| {
                    div()
                        .w(px(44.))
                        .flex_none()
                        .pr(px(8.))
                        .flex()
                        .justify_end()
                        .text_color(c(FG_GUTTER))
                        .child(n.map(|n| n.to_string()).unwrap_or_default())
                };
                div()
                    .id(("line", i))
                    .flex()
                    .items_center()
                    .h(px(LINE_H))
                    .w_full()
                    .bg(c(bg))
                    .font_family(MONO_FONT)
                    .text_size(px(12.))
                    .when(selected, |d| d.border_l_2().border_color(c(BLUE)))
                    .when(changeable, |d| d.cursor_pointer().hover(|s| s.opacity(0.85)))
                    .child(num(line.old_no))
                    .child(num(line.new_no))
                    .child(div().w(px(18.)).flex_none().text_color(c(sign_color)).child(sign))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_color(match line.kind {
                                LineKind::NoNewline => c(COMMENT),
                                LineKind::Context => c(FG_DARK),
                                _ => c(FG),
                            })
                            .child(expand_tabs(&line.text)),
                    )
                    .when(changeable, |d| {
                        d.on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| this.toggle_line(h, l, ev.modifiers().shift, cx)))
                    })
                    .into_any_element()
            }
        }
    }

    fn toggle_line(&mut self, h: usize, l: usize, extend: bool, cx: &mut Context<Self>) {
        let Some(fv) = &mut self.file_view else { return };
        let Some(diff) = &fv.diff else { return };
        // Selections are per hunk: picking a line elsewhere starts over.
        if fv.selected.iter().any(|(sh, _)| *sh != h) {
            fv.selected.clear();
        }
        match fv.anchor.filter(|(ah, _)| extend && *ah == h) {
            Some((_, al)) => {
                let lines = &diff.hunks[h].lines;
                for x in al.min(l)..=al.max(l) {
                    if matches!(lines[x].kind, LineKind::Add | LineKind::Del) {
                        fv.selected.insert((h, x));
                    }
                }
            }
            None => {
                if !fv.selected.remove(&(h, l)) {
                    fv.selected.insert((h, l));
                }
                fv.anchor = Some((h, l));
            }
        }
        cx.notify();
    }

    // ---- conflicts -----------------------------------------------------------

    pub fn render_conflict(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let cv = self.conflict.as_ref().unwrap();
        let total = cv.file.conflicts().count();
        let left = cv.file.unresolved();
        let (ours_l, theirs_l) = cv
            .file
            .conflicts()
            .next()
            .map(|c| (c.ours_label.clone(), c.theirs_label.clone()))
            .unwrap_or_default();
        let path = cv.path.clone();
        let n = cv.rows.len();

        let bar = top_bar()
            .child(button("back", "← Graph", Tone::Ghost).on_click(cx.listener(|this, _, window, cx| {
                this.conflict = None;
                window.focus(&this.focus);
                cx.notify();
            })))
            .child(status_badge("!"))
            .child(div().truncate().font_weight(gpui::FontWeight::SEMIBOLD).child(path.clone()))
            .child(pill(
                if total == 0 { "no markers".to_string() } else { format!("{} of {total} resolved", total - left) },
                if left == 0 { GREEN } else { ORANGE },
                false,
            ))
            .child(div().flex_1())
            .child(button("all-ours", format!("All ours · {}", short_label(&ours_l)), Tone::Normal).on_click(cx.listener(|this, _, _, cx| this.pick_all_conflicts(Pick::Ours, cx))))
            .child(button("all-theirs", format!("All theirs · {}", short_label(&theirs_l)), Tone::Normal).on_click(cx.listener(|this, _, _, cx| this.pick_all_conflicts(Pick::Theirs, cx))))
            .child(
                button("save-resolved", if left == 0 { "Save & mark resolved" } else { "Save" }, if left == 0 { Tone::Primary } else { Tone::Normal })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let left = this.conflict.as_ref().map_or(0, |c| c.file.unresolved());
                        if left > 0 {
                            this.confirm(
                                "Conflicts remain",
                                format!("{left} conflict{} still unresolved. Their markers will be kept and the file marked resolved anyway.", if left == 1 { " is" } else { "s are" }),
                                "Save anyway",
                                false,
                                action(|this, _, cx| this.save_conflict(cx)),
                            );
                            cx.notify();
                        } else {
                            this.save_conflict(cx);
                        }
                    })),
            );

        let body = if n == 0 {
            div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .text_color(c(COMMENT))
                .child("This file has no conflict markers (binary or deleted on one side). Use Ours / Theirs in the panel.")
                .into_any_element()
        } else {
            uniform_list(
                "conflict",
                n,
                cx.processor(|this, range: std::ops::Range<usize>, _, cx| range.map(|i| this.render_conflict_row(i, cx)).collect::<Vec<_>>()),
            )
            .flex_1()
            .into_any_element()
        };

        let p = path.clone();
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(c(BG))
            .child(bar)
            .child(body)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(10.))
                    .px(px(12.))
                    .py(px(6.))
                    .border_t_1()
                    .border_color(c(BORDER))
                    .text_size(px(11.))
                    .text_color(c(COMMENT))
                    .child(div().size(px(8.)).rounded(px(2.)).bg(c(BLUE)))
                    .child(format!("ours · {ours_l}"))
                    .child(div().size(px(8.)).rounded(px(2.)).bg(c(MAGENTA)))
                    .child(format!("theirs · {theirs_l}"))
                    .child(div().flex_1())
                    .child(mini_button("open-editor", "Open in editor", BLUE).on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(d) = &this.workdir {
                            cx.open_with_system(&d.join(&p));
                            this.toast(ToastKind::Info, "Opened. Come back and reload the file (⌘R) after editing.", cx);
                        }
                    }))),
            )
            .into_any_element()
    }

    fn render_conflict_row(&mut self, i: usize, cx: &mut Context<Self>) -> AnyElement {
        let cv = self.conflict.as_ref().unwrap();
        let row = cv.rows[i];
        let line = |bg: u32, bar: u32, text: SharedString, color: u32| {
            div()
                .flex()
                .items_center()
                .h(px(LINE_H))
                    .w_full()
                .bg(c(bg))
                .border_l_2()
                .border_color(c(bar))
                .pl(px(14.))
                .font_family(MONO_FONT)
                .text_size(px(12.))
                .text_color(c(color))
                .whitespace_nowrap()
                .overflow_hidden()
                .child(text)
        };
        let conflict = |n: usize| cv.file.conflicts().nth(n).unwrap();
        match row {
            ConflictRow::Text(si, l) => {
                let Segment::Text(t) = &cv.file.segments[si] else { unreachable!() };
                line(BG, BG, expand_tabs(&t[l]), COMMENT).into_any_element()
            }
            ConflictRow::Ours(n, l) => line(OURS_BG, BLUE, expand_tabs(&conflict(n).ours[l]), FG).into_any_element(),
            ConflictRow::Theirs(n, l) => line(THEIRS_BG, MAGENTA, expand_tabs(&conflict(n).theirs[l]), FG).into_any_element(),
            ConflictRow::Resolved(n, l) => line(RESOLVED_BG, GREEN, expand_tabs(cv.resolved_line(n, l).unwrap_or("")), FG).into_any_element(),
            ConflictRow::Empty(_) => line(RESOLVED_BG, GREEN, "(nothing — both sides dropped)".into(), COMMENT).into_any_element(),
            ConflictRow::Sep(n) => div()
                .flex()
                .items_center()
                .gap(px(8.))
                .h(px(LINE_H))
                    .w_full()
                .px(px(14.))
                .bg(c(BG_DARK))
                .text_size(px(10.5))
                .text_color(c(DARK5))
                .child(div().text_color(c(BLUE)).child(format!("▲ ours · {}", short_label(&conflict(n).ours_label))))
                .child(div().flex_1().h(px(1.)).bg(c(BORDER_HI)))
                .child(div().text_color(c(MAGENTA)).child(format!("▼ theirs · {}", short_label(&conflict(n).theirs_label))))
                .into_any_element(),
            ConflictRow::Header(n) => {
                let c0 = conflict(n);
                let picked = c0.pick;
                let base = div()
                    .id(("ch", i))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(px(LINE_H))
                    .w_full()
                    .px(px(12.))
                    .bg(c(BG_HIGHLIGHT))
                    .border_t_1()
                    .border_color(c(BORDER_HI))
                    .child(div().text_size(px(11.)).font_weight(gpui::FontWeight::BOLD).text_color(if picked.is_some() { c(GREEN) } else { c(ORANGE) }).child(
                        match picked {
                            None => format!("CONFLICT {}", n + 1),
                            Some(p) => format!("✓ RESOLVED {} · {}", n + 1, pick_label(p)),
                        },
                    ))
                    .child(div().flex_1());
                match picked {
                    Some(_) => base
                        .child(mini_button(("undo", i), "Undo", DARK5).on_click(cx.listener(move |this, _, _, cx| this.pick_conflict(n, None, cx))))
                        .into_any_element(),
                    None => base
                        .child(mini_button(("o", i), "Take ours", BLUE).on_click(cx.listener(move |this, _, _, cx| this.pick_conflict(n, Some(Pick::Ours), cx))))
                        .child(mini_button(("t", i), "Take theirs", MAGENTA).on_click(cx.listener(move |this, _, _, cx| this.pick_conflict(n, Some(Pick::Theirs), cx))))
                        .child(mini_button(("ot", i), "Both (ours first)", GREEN1).on_click(cx.listener(move |this, _, _, cx| this.pick_conflict(n, Some(Pick::OursThenTheirs), cx))))
                        .child(mini_button(("to", i), "Both (theirs first)", GREEN1).on_click(cx.listener(move |this, _, _, cx| this.pick_conflict(n, Some(Pick::TheirsThenOurs), cx))))
                        .into_any_element(),
                }
            }
        }
    }
}

fn pick_label(p: Pick) -> &'static str {
    match p {
        Pick::Ours => "ours",
        Pick::Theirs => "theirs",
        Pick::OursThenTheirs => "both, ours first",
        Pick::TheirsThenOurs => "both, theirs first",
    }
}

/// Conflict labels can be long ("a1b2c3d (Some commit message)").
fn short_label(l: &str) -> String {
    let l = if l.is_empty() { "?" } else { l };
    if l.chars().count() > 28 { format!("{}…", l.chars().take(27).collect::<String>()) } else { l.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_become_four_spaces() {
        assert_eq!(expand_tabs("a\tb"), SharedString::from("a    b"));
        assert_eq!(expand_tabs("\t\tx"), SharedString::from("        x"));
        assert_eq!(expand_tabs("plain"), SharedString::from("plain"));
    }

    #[test]
    fn long_conflict_labels_are_shortened() {
        assert_eq!(short_label(""), "?");
        assert_eq!(short_label("HEAD"), "HEAD");
        let exact = "x".repeat(28);
        assert_eq!(short_label(&exact), exact);
        let long = "a1b2c3d (A rather long commit message)";
        let short = short_label(long);
        assert_eq!(short.chars().count(), 28);
        assert!(short.ends_with('…'));
        assert!(long.starts_with(short.trim_end_matches('…')));
        // Counts characters, not bytes.
        assert_eq!(short_label(&"é".repeat(28)), "é".repeat(28));
        assert_eq!(short_label(&"é".repeat(30)).chars().count(), 28);
    }

    #[test]
    fn pick_labels() {
        assert_eq!(pick_label(Pick::Ours), "ours");
        assert_eq!(pick_label(Pick::Theirs), "theirs");
        assert_eq!(pick_label(Pick::OursThenTheirs), "both, ours first");
        assert_eq!(pick_label(Pick::TheirsThenOurs), "both, theirs first");
    }
}

#[cfg(test)]
mod view_tests {
    use gpui::TestAppContext;

    use crate::git::diff::FileStatus;

    use super::super::test_support::*;
    use super::*;

    /// Opens the unstaged diff of `f.txt`: one hunk of context, deletions
    /// and additions.
    fn diff_open(cx: &mut TestAppContext) -> (TempRepo, gpui::Entity<GitPanda>, &mut gpui::VisualTestContext) {
        let r = TempRepo::new();
        r.commit("f.txt", "a\nb\nc\nd\n", "base");
        r.write("f.txt", "a\nB\nc\nD\ne\n");
        let (view, cx) = open(&r, cx);
        view.update(cx, |v, cx| v.open_file(DiffSource::Unstaged, "f.txt".into(), FileStatus::Modified, cx));
        cx.run_until_parked();
        (r, view, cx)
    }

    fn kinds(view: &gpui::Entity<GitPanda>, cx: &mut gpui::VisualTestContext) -> Vec<LineKind> {
        view.read_with(cx, |v, _| v.file_view.as_ref().unwrap().diff.as_ref().unwrap().hunks[0].lines.iter().map(|l| l.kind).collect())
    }

    fn selected(view: &gpui::Entity<GitPanda>, cx: &mut gpui::VisualTestContext) -> Vec<usize> {
        let mut s: Vec<usize> = view.read_with(cx, |v, _| v.file_view.as_ref().unwrap().selected_in(0).into_iter().collect());
        s.sort();
        s
    }

    #[gpui::test]
    fn clicking_lines_toggles_them(cx: &mut TestAppContext) {
        let (_r, view, cx) = diff_open(cx);
        use LineKind::*;
        assert_eq!(kinds(&view, cx), [Context, Del, Add, Context, Del, Add, Add]);
        view.update(cx, |v, cx| v.toggle_line(0, 1, false, cx));
        view.update(cx, |v, cx| v.toggle_line(0, 2, false, cx));
        assert_eq!(selected(&view, cx), [1, 2]);
        view.update(cx, |v, cx| v.toggle_line(0, 1, false, cx));
        assert_eq!(selected(&view, cx), [2]);
    }

    #[gpui::test]
    fn shift_click_selects_changed_lines_in_between(cx: &mut TestAppContext) {
        let (_r, view, cx) = diff_open(cx);
        view.update(cx, |v, cx| v.toggle_line(0, 1, false, cx));
        view.update(cx, |v, cx| v.toggle_line(0, 5, true, cx));
        // Line 3 is context, so it's left out.
        assert_eq!(selected(&view, cx), [1, 2, 4, 5]);
    }

    #[gpui::test]
    fn stage_selected_lines(cx: &mut TestAppContext) {
        let (r, view, cx) = diff_open(cx);
        // b → B only.
        view.update(cx, |v, cx| v.toggle_line(0, 1, false, cx));
        view.update(cx, |v, cx| v.toggle_line(0, 2, true, cx));
        view.update(cx, |v, cx| v.hunk_op(0, true, false, cx));
        cx.run_until_parked();
        assert_eq!(r.git(&["show", ":f.txt"]), "a\nB\nc\nd\n");
        assert_eq!(r.read("f.txt"), "a\nB\nc\nD\ne\n");
        view.read_with(cx, |v, _| {
            let fv = v.file_view.as_ref().unwrap();
            assert!(fv.selected.is_empty());
            // The diff reloads with what's left.
            assert_eq!(fv.diff.as_ref().unwrap().hunks[0].lines.iter().filter(|l| l.kind != LineKind::Context).count(), 3);
        });
        assert_eq!(toasts(&view, cx).last().unwrap(), "Staged lines");
    }

    #[gpui::test]
    fn discarding_a_hunk_asks_first(cx: &mut TestAppContext) {
        let (r, view, cx) = diff_open(cx);
        view.update(cx, |v, cx| v.hunk_op(0, false, true, cx));
        assert_eq!(r.read("f.txt"), "a\nB\nc\nD\ne\n");
        view.read_with(cx, |v, _| match &v.modal {
            Some(super::super::app::Modal::Confirm { title, .. }) => assert_eq!(title.as_ref(), "Discard this hunk?"),
            _ => panic!("expected a confirmation"),
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(r.read("f.txt"), "a\nb\nc\nd\n");
        // Nothing left to show.
        assert!(view.read_with(cx, |v, _| v.file_view.is_none()));
    }

    #[gpui::test]
    fn a_file_without_changes_says_so(cx: &mut TestAppContext) {
        let r = three_commits();
        let (view, cx) = open(&r, cx);
        view.update(cx, |v, cx| v.open_file(DiffSource::Unstaged, "one.txt".into(), FileStatus::Modified, cx));
        cx.run_until_parked();
        view.read_with(cx, |v, _| {
            let fv = v.file_view.as_ref().unwrap();
            assert!(fv.diff.is_none() && fv.rows.is_empty());
            assert_eq!(fv.error.as_ref().map(|e| e.as_ref()), Some("No changes"));
        });
    }
}
