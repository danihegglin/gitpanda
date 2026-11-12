//! Right panel: staging area and commit box for the working tree, or the
//! details and changed files of the selected commit.

use gpui::{
    AnyElement, ClickEvent, ClipboardItem, Context, MouseButton, MouseDownEvent, ScrollStrategy,
    SharedString, Window, div, prelude::*, px, uniform_list,
};

use crate::git::diff::{DiffSource, FileStatus};
use crate::git::repo::wip_oid;

use super::app::{Menu, MenuItem, ToastKind, GitPanda, action};
use super::theme::*;
use super::widgets::*;

pub const WIDTH: f32 = 390.;
const ROW_H: f32 = 26.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Group {
    Conflicted,
    Unstaged,
    Staged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WipRow {
    Header(Group, usize),
    File(Group, usize),
    Gap,
}

fn split_path(path: &str) -> (SharedString, SharedString) {
    match path.rsplit_once('/') {
        Some((dir, name)) => (name.to_string().into(), format!("{dir}/").into()),
        None => (path.to_string().into(), "".into()),
    }
}

fn file_label(path: &str, old: Option<&str>) -> gpui::Div {
    let (name, dir) = split_path(path);
    div()
        .flex()
        .flex_1()
        .min_w_0()
        .items_center()
        .gap(px(6.))
        .overflow_hidden()
        .whitespace_nowrap()
        .child(div().flex_none().text_color(c(FG)).child(name))
        .children(old.map(|o| div().flex_none().text_color(c(MAGENTA)).text_size(px(11.)).child(format!("← {o}"))))
        .child(div().min_w_0().truncate().text_color(c(DARK3)).text_size(px(11.5)).child(dir))
}

impl GitPanda {
    pub fn render_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let content = if self.selected == Some(wip_oid()) {
            self.render_wip_panel(window, cx)
        } else {
            self.render_commit_panel(cx)
        };
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(WIDTH))
            .h_full()
            .bg(c(PANE_BG))
            .border_l_1()
            .border_color(c(PANE_EDGE))
            .child(content)
            .into_any_element()
    }

    fn wip_rows(&self) -> Vec<WipRow> {
        let Some(s) = &self.snap else { return Vec::new() };
        let st = &s.status;
        let mut rows = Vec::new();
        for (g, n) in [
            (Group::Conflicted, st.conflicted.len()),
            (Group::Unstaged, st.unstaged.len()),
            (Group::Staged, st.staged.len()),
        ] {
            if g == Group::Conflicted && n == 0 {
                continue;
            }
            if !rows.is_empty() {
                rows.push(WipRow::Gap);
            }
            rows.push(WipRow::Header(g, n));
            rows.extend((0..n).map(|i| WipRow::File(g, i)));
        }
        rows
    }

    /// The files the side panel lists, in display order, each with the diff
    /// it opens and its row in the panel's list. Conflicted files open the
    /// resolver instead of a diff, so they are skipped.
    fn panel_files(&self) -> Vec<(DiffSource, String, FileStatus, usize)> {
        if self.selected == Some(wip_oid()) {
            let Some(s) = &self.snap else { return Vec::new() };
            return self
                .wip_rows()
                .into_iter()
                .enumerate()
                .filter_map(|(row, r)| match r {
                    WipRow::File(Group::Unstaged, i) => {
                        let e = &s.status.unstaged[i];
                        Some((DiffSource::Unstaged, e.path.to_string(), e.status, row))
                    }
                    WipRow::File(Group::Staged, i) => {
                        let e = &s.status.staged[i];
                        Some((DiffSource::Staged, e.path.to_string(), e.status, row))
                    }
                    _ => None,
                })
                .collect();
        }
        match &self.detail {
            Some(d) if self.selected == Some(d.oid) => d
                .files
                .iter()
                .enumerate()
                .map(|(row, f)| (DiffSource::Commit(d.oid), f.path.to_string(), f.status, row))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Opens the file `delta` places from the open one, or the first file
    /// when none is open.
    pub fn step_file(&mut self, delta: isize, cx: &mut Context<Self>) {
        self.clear_file_selection();
        let files = self.panel_files();
        if files.is_empty() {
            return cx.notify();
        }
        let cur = self
            .file_view
            .as_ref()
            .and_then(|fv| files.iter().position(|(s, p, ..)| *s == fv.source && *p == fv.path));
        let next = match cur {
            Some(i) => (i as isize + delta).clamp(0, files.len() as isize - 1) as usize,
            None => 0,
        };
        if Some(next) == cur {
            return;
        }
        let (source, path, status, row) = files[next].clone();
        self.open_file(source, path, status, cx);
        self.files_scroll.scroll_to_item(row, ScrollStrategy::Center);
    }

    // ---- multi-selection of unstaged files ---------------------------------

    /// Unstaged files, in display order, and whether each is untracked.
    fn unstaged_files(&self) -> Vec<(String, bool)> {
        match &self.snap {
            Some(s) if self.selected == Some(wip_oid()) => {
                s.status.unstaged.iter().map(|e| (e.path.to_string(), e.status == FileStatus::Untracked)).collect()
            }
            _ => Vec::new(),
        }
    }

    /// The selected unstaged files, in display order.
    pub fn selected_files(&self) -> Vec<(String, bool)> {
        self.unstaged_files().into_iter().filter(|(p, _)| self.file_sel.contains(p)).collect()
    }

    pub fn clear_file_selection(&mut self) {
        self.file_sel.clear();
        self.file_anchor = None;
        self.file_head = None;
    }

    /// Drops selected files that are no longer unstaged.
    pub fn prune_file_selection(&mut self) {
        let paths: std::collections::HashSet<String> = self.unstaged_files().into_iter().map(|(p, _)| p).collect();
        self.file_sel.retain(|p| paths.contains(p));
        if self.file_sel.is_empty() {
            self.clear_file_selection();
        }
    }

    pub fn select_all_files(&mut self, cx: &mut Context<Self>) {
        let files = self.unstaged_files();
        self.file_anchor = files.first().map(|(p, _)| p.clone());
        self.file_head = files.last().map(|(p, _)| p.clone());
        self.file_sel = files.into_iter().map(|(p, _)| p).collect();
        cx.notify();
    }

    /// The unstaged file a shift-selection grows from: its moving end, or
    /// else the open file when that is unstaged.
    pub fn file_cursor(&self) -> Option<String> {
        let files = self.unstaged_files();
        let has = |p: &String| files.iter().any(|(f, _)| f == p);
        self.file_head.clone().filter(has).or_else(|| {
            self.file_view.as_ref().filter(|f| f.source == DiffSource::Unstaged).map(|f| f.path.clone()).filter(has)
        })
    }

    /// Selects the unstaged files from the anchor through `path`.
    fn select_file_range(&mut self, path: String) {
        let files = self.unstaged_files();
        let pos = |p: &str| files.iter().position(|(f, _)| f == p);
        let Some(head) = pos(&path) else { return };
        let anchor = self
            .file_anchor
            .as_deref()
            .and_then(pos)
            .or_else(|| self.file_cursor().as_deref().and_then(pos))
            .unwrap_or(head);
        let (lo, hi) = (anchor.min(head), anchor.max(head));
        self.file_sel = files[lo..=hi].iter().map(|(p, _)| p.clone()).collect();
        self.file_anchor = Some(files[anchor].0.clone());
        self.file_head = Some(path);
    }

    /// Moves the selection's moving end `delta` files, opening that file.
    pub fn extend_file_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(cur) = self.file_cursor() else { return };
        let files = self.unstaged_files();
        let i = files.iter().position(|(p, _)| *p == cur).unwrap_or(0);
        let next = (i as isize + delta).clamp(0, files.len() as isize - 1) as usize;
        if self.file_anchor.is_none() {
            self.file_anchor = Some(cur);
        }
        let path = files[next].0.clone();
        self.select_file_range(path.clone());
        let hit = self.panel_files().into_iter().find(|(s, p, ..)| *s == DiffSource::Unstaged && *p == path);
        if let Some((source, p, status, row)) = hit {
            self.open_file(source, p, status, cx);
            self.files_scroll.scroll_to_item(row, ScrollStrategy::Center);
        }
        cx.notify();
    }

    /// The files a row action applies to: the whole selection when the row
    /// is part of it, else just that row.
    fn action_files(&self, path: &str, untracked: bool) -> Vec<(String, bool)> {
        if self.file_sel.len() > 1 && self.file_sel.contains(path) {
            self.selected_files()
        } else {
            vec![(path.to_string(), untracked)]
        }
    }

    pub fn discard_files(&mut self, files: Vec<(String, bool)>, cx: &mut Context<Self>) {
        match files.len() {
            0 => {}
            1 => {
                let (path, untracked) = files.into_iter().next().unwrap();
                self.discard_path(path, untracked, cx);
            }
            n => {
                let deleted = files.iter().filter(|(_, u)| *u).count();
                let mut body = format!("All unstaged changes to {n} files will be lost.");
                if deleted > 0 {
                    body += &format!(" {deleted} untracked file{} will be deleted.", if deleted == 1 { "" } else { "s" });
                }
                self.confirm(
                    format!("Discard {n} files?"),
                    body,
                    "Discard",
                    true,
                    action(move |this, _, cx| {
                        let files = files.clone();
                        this.clear_file_selection();
                        this.run("Discarding", move |g| g.discard_many(&files).map(|_| format!("Discarded {n} files")), cx);
                    }),
                );
                cx.notify();
            }
        }
    }

    pub fn stage_files(&mut self, files: Vec<(String, bool)>, cx: &mut Context<Self>) {
        self.clear_file_selection();
        self.run(
            "Staging",
            move |g| g.stage(&files.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>()).map(|_| String::new()),
            cx,
        );
    }

    fn render_wip_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let rows = self.wip_rows();
        let n = rows.len();
        let branch = self.head_branch().unwrap_or_else(|| "detached HEAD".into());
        let staged = self.snap.as_ref().map_or(0, |s| s.status.staged.len());
        let amend = self.amend;
        let input_focused = self.commit_input.read(cx).focus.is_focused(window);
        let commit_label = match (amend, staged) {
            (true, _) => format!("Amend last commit on {branch}"),
            (false, 0) => "Stage changes to commit".to_string(),
            (false, n) => format!("Commit {n} file{} to {branch}", if n == 1 { "" } else { "s" }),
        };
        let can_commit = amend || staged > 0;

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(8.))
                    .h(px(44.))
                    .px(px(14.))
                    .border_b_1()
                    .border_color(c(BORDER))
                    .child(div().text_size(px(14.)).font_weight(gpui::FontWeight::SEMIBOLD).child("Working tree"))
                    .child(div().flex_1())
                    .child(pill(branch, GREEN, false)),
            )
            .child(
                uniform_list(
                    "wip-files",
                    n,
                    cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                        range.map(|i| this.render_wip_row(rows[i], i, cx)).collect::<Vec<_>>()
                    }),
                )
                .track_scroll(self.files_scroll.clone())
                .flex_1()
                .py(px(6.)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .gap(px(10.))
                    .p(px(14.))
                    .border_t_1()
                    .border_color(c(BORDER))
                    .bg(c(PANE_BG))
                    .child(section_label("COMMIT MESSAGE"))
                    .child(self.commit_input.clone())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .id("amend")
                                    .flex()
                                    .items_center()
                                    .gap(px(7.))
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| this.toggle_amend(cx)))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .size(px(15.))
                                            .rounded(px(4.))
                                            .border_1()
                                            .border_color(if amend { c(BLUE) } else { c(TERMINAL_BLACK) })
                                            .when(amend, |d| d.bg(c(BLUE)).text_color(c(BG_DARKER)).text_size(px(10.)).child("✓")),
                                    )
                                    .child(div().text_size(px(12.)).text_color(c(FG_DARK)).child("Amend")),
                            )
                            .child(div().flex_1())
                            .when(input_focused, |d| d.child(div().text_size(px(11.)).text_color(c(COMMENT)).child("⌘⏎ to commit"))),
                    )
                    .child(
                        button("commit", commit_label, if can_commit { Tone::Primary } else { Tone::Normal })
                            .w_full()
                            .h(px(34.))
                            .text_size(px(13.))
                            .when(!can_commit, |d| d.opacity(0.6))
                            .on_click(cx.listener(|this, _, window, cx| this.commit(window, cx))),
                    ),
            )
            .into_any_element()
    }

    fn render_wip_row(&mut self, row: WipRow, i: usize, cx: &mut Context<Self>) -> AnyElement {
        let snap = self.snap.as_ref().unwrap();
        let st = &snap.status;
        let base = div().id(("wip", i)).flex().items_center().gap(px(8.)).h(px(ROW_H)).w_full().px(px(14.));
        match row {
            WipRow::Gap => base.h(px(ROW_H)).into_any_element(),
            WipRow::Header(g, n) => {
                let (label, btn): (&str, Option<(&str, u32)>) = match g {
                    Group::Conflicted => ("CONFLICTS", None),
                    Group::Unstaged => ("UNSTAGED", (n > 0).then_some(("Stage all", GREEN))),
                    Group::Staged => ("STAGED", (n > 0).then_some(("Unstage all", YELLOW))),
                };
                let picked = if g == Group::Unstaged { self.file_sel.len() } else { 0 };
                base.child(section_label(label))
                    .child(pill(n.to_string(), if g == Group::Conflicted { RED } else { DARK5 }, false))
                    .when(picked > 0, |d| d.child(div().text_size(px(11.)).text_color(c(COMMENT)).child(format!("{picked} selected"))))
                    .child(div().flex_1())
                    .when(picked > 1, |d| {
                        d.child(mini_button("discard-selected", format!("Discard {picked}"), RED).on_click(cx.listener(|this, _, _, cx| {
                            this.discard_files(this.selected_files(), cx)
                        })))
                    })
                    .children(btn.map(|(t, col)| {
                        mini_button(("all", i), t, col).on_click(cx.listener(move |this, _, _, cx| match g {
                            Group::Unstaged => this.run("Staging", |g| g.stage_all().map(|_| String::new()), cx),
                            _ => this.run("Unstaging", |g| g.unstage_all().map(|_| String::new()), cx),
                        }))
                    }))
                    .into_any_element()
            }
            WipRow::File(g, fi) => {
                let e = match g {
                    Group::Conflicted => &st.conflicted[fi],
                    Group::Unstaged => &st.unstaged[fi],
                    Group::Staged => &st.staged[fi],
                };
                let path = e.path.to_string();
                let status = e.status;
                let open = match g {
                    Group::Conflicted => self.conflict.as_ref().is_some_and(|c| c.path == path),
                    Group::Unstaged => self.file_view.as_ref().is_some_and(|f| f.path == path && f.source == DiffSource::Unstaged),
                    Group::Staged => self.file_view.as_ref().is_some_and(|f| f.path == path && f.source == DiffSource::Staged),
                };
                let (p1, p2, p3) = (path.clone(), path.clone(), path.clone());
                let untracked = status == FileStatus::Untracked;
                let picked = g == Group::Unstaged && self.file_sel.contains(&path);
                let actions = div().flex().gap(px(4.)).opacity(0.).group_hover("wip-row", |s| s.opacity(1.));
                let actions = match g {
                    Group::Unstaged => actions
                        .child(mini_button(("discard", i), "Discard", RED).on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.discard_files(this.action_files(&p1, untracked), cx)
                        })))
                        .child(mini_button(("stage", i), "Stage", GREEN).on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.stage_files(this.action_files(&p2, untracked), cx)
                        }))),
                    Group::Staged => actions.child(mini_button(("unstage", i), "Unstage", YELLOW).on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.unstage_path(p1.clone(), cx)
                    }))),
                    Group::Conflicted => actions
                        .child(mini_button(("ours", i), "Ours", BLUE).on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            let p = p1.clone();
                            this.run("Resolving", move |g| g.take_side(&p, true).map(|_| format!("Took ours for {p}")), cx)
                        })))
                        .child(mini_button(("theirs", i), "Theirs", MAGENTA).on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            let p = p2.clone();
                            this.run("Resolving", move |g| g.take_side(&p, false).map(|_| format!("Took theirs for {p}")), cx)
                        }))),
                };
                base.group("wip-row")
                    .cursor_pointer()
                    .when(open || picked, |d| d.bg(c(BG_HIGHLIGHT)))
                    .hover(|s| s.bg(c(BG_HOVER)))
                    .child(status_badge(status.letter()))
                    .child(file_label(&path, None))
                    .child(actions)
                    .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                        window.focus(&this.focus);
                        let p = p3.clone();
                        let m = ev.modifiers();
                        if g == Group::Unstaged && (m.shift || m.platform || m.control) && ev.click_count() == 1 {
                            if m.shift {
                                this.select_file_range(p.clone());
                            } else if !this.file_sel.remove(&p) {
                                // ⌘-click toggles, starting from the open file.
                                if let Some(open) = this.file_cursor() {
                                    this.file_sel.insert(open);
                                }
                                this.file_sel.insert(p.clone());
                                this.file_anchor = Some(p.clone());
                                this.file_head = Some(p.clone());
                            }
                            this.open_file(DiffSource::Unstaged, p, status, cx);
                            return;
                        }
                        this.clear_file_selection();
                        match g {
                            Group::Conflicted => this.open_conflict(p, cx),
                            Group::Unstaged if ev.click_count() == 2 => this.stage_path(p, cx),
                            Group::Staged if ev.click_count() == 2 => this.unstage_path(p, cx),
                            Group::Unstaged => this.open_file(DiffSource::Unstaged, p, status, cx),
                            Group::Staged => this.open_file(DiffSource::Staged, p, status, cx),
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            let items = if g == Group::Unstaged && this.file_sel.len() > 1 && this.file_sel.contains(&path) {
                                this.selection_menu()
                            } else {
                                this.file_menu(path.clone(), g == Group::Staged, untracked)
                            };
                            this.menu = Some(Menu { pos: ev.position, items });
                            cx.notify();
                        }),
                    )
                    .into_any_element()
            }
        }
    }

    /// Right-click menu for a row that is part of a multi-selection.
    fn selection_menu(&self) -> Vec<MenuItem> {
        let n = self.file_sel.len();
        vec![
            MenuItem::Action {
                label: format!("Stage {n} files").into(),
                danger: false,
                action: action(|this, _, cx| this.stage_files(this.selected_files(), cx)),
            },
            MenuItem::Separator,
            MenuItem::Action {
                label: format!("Discard {n} files…").into(),
                danger: true,
                action: action(|this, _, cx| this.discard_files(this.selected_files(), cx)),
            },
        ]
    }

    fn file_menu(&self, path: String, staged: bool, untracked: bool) -> Vec<MenuItem> {
        let mut items = Vec::new();
        let p = path.clone();
        if staged {
            items.push(MenuItem::Action { label: "Unstage".into(), danger: false, action: action(move |this, _, cx| this.unstage_path(p.clone(), cx)) });
        } else {
            items.push(MenuItem::Action { label: "Stage".into(), danger: false, action: action(move |this, _, cx| this.stage_path(p.clone(), cx)) });
        }
        let p = path.clone();
        items.push(MenuItem::Action { label: "Rename / move…".into(), danger: false, action: action(move |this, w, cx| this.rename_file(p.clone(), w, cx)) });
        let p = path.clone();
        items.push(MenuItem::Action {
            label: "Open in default app".into(),
            danger: false,
            action: action(move |this, _, cx| {
                if let Some(d) = &this.workdir {
                    cx.open_with_system(&d.join(&p));
                }
            }),
        });
        let p = path.clone();
        items.push(MenuItem::Action {
            label: "Reveal in Finder".into(),
            danger: false,
            action: action(move |this, _, cx| {
                if let Some(d) = &this.workdir {
                    cx.reveal_path(&d.join(&p));
                }
            }),
        });
        let p = path.clone();
        items.push(MenuItem::Action {
            label: "Copy path".into(),
            danger: false,
            action: action(move |this, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(p.clone()));
                this.toast(ToastKind::Info, "Copied path", cx);
            }),
        });
        if !staged {
            items.push(MenuItem::Separator);
            items.push(MenuItem::Action {
                label: if untracked { "Delete file…".into() } else { "Discard changes…".into() },
                danger: true,
                action: action(move |this, _, cx| this.discard_path(path.clone(), untracked, cx)),
            });
        }
        items
    }

    fn render_commit_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(d) = self.detail.clone() else {
            return div()
                .flex()
                .size_full()
                .items_center()
                .justify_center()
                .text_color(c(COMMENT))
                .child(if self.selected.is_some() { "Loading…" } else { "Select a commit" })
                .into_any_element();
        };
        let (summary, body) = match d.message.split_once('\n') {
            Some((s, b)) => (s.to_string(), b.trim().to_string()),
            None => (d.message.to_string(), String::new()),
        };
        let sha = d.oid.to_string();
        let adds: usize = d.files.iter().map(|f| f.additions).sum();
        let dels: usize = d.files.iter().map(|f| f.deletions).sum();
        let n = d.files.len();
        let oid = d.oid;
        let stash = self.snap.as_ref().and_then(|s| s.stashes.iter().find(|st| st.oid == oid).map(|st| st.index));

        let parents = div().flex().flex_wrap().gap(px(6.)).items_center().child(section_label("PARENTS")).children(
            d.parents.iter().enumerate().map(|(pi, p)| {
                let p = *p;
                div()
                    .id(("parent", pi))
                    .font_family(MONO_FONT)
                    .text_size(px(11.5))
                    .text_color(c(BLUE))
                    .cursor_pointer()
                    .hover(|s| s.text_color(c(CYAN)))
                    .child(p.to_string()[..7].to_string())
                    .on_click(cx.listener(move |this, _, _, cx| this.reveal(p, cx)))
            }),
        );

        let header = div()
            .id("commit-header")
            .flex()
            .flex_col()
            .flex_none()
            .max_h(px(360.))
            .overflow_y_scroll()
            .gap(px(12.))
            .p(px(16.))
            .border_b_1()
            .border_color(c(BORDER))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .id("sha")
                            .font_family(MONO_FONT)
                            .text_size(px(11.5))
                            .text_color(c(DARK5))
                            .px(px(6.))
                            .py(px(2.))
                            .rounded(px(4.))
                            .bg(c(BG_HIGHLIGHT))
                            .cursor_pointer()
                            .hover(|s| s.text_color(c(FG)))
                            .child(sha[..10].to_string())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(sha.clone()));
                                this.toast(ToastKind::Info, "Copied commit SHA", cx);
                            })),
                    )
                    .when(stash.is_some(), |d| d.child(pill("stash", MAGENTA, false)))
                    .child(div().flex_1())
                    .children(stash.map(|index| {
                        button("pop", "Pop", Tone::Normal).on_click(cx.listener(move |this, _, _, cx| {
                            this.run("Popping", move |g| g.stash_pop(index).map(|_| "Popped stash".into()), cx)
                        }))
                    })),
            )
            .child(div().text_size(px(15.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(FG)).child(summary))
            .when(!body.is_empty(), |el| el.child(div().text_size(px(12.5)).text_color(c(FG_DARK)).child(body)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(avatar(&d.author, &d.email, 30.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .child(div().text_color(c(FG)).child(d.author.clone()))
                            .child(div().text_size(px(11.5)).text_color(c(COMMENT)).truncate().child(format!("{} · {}", d.email, date_time(d.time)))),
                    ),
            )
            .when(d.committer != d.author, |el| {
                el.child(div().text_size(px(11.5)).text_color(c(COMMENT)).child(format!("committed by {}", d.committer)))
            })
            .child(parents);

        let files = d.files.clone();
        let file_list = uniform_list(
            "commit-files",
            n,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range
                    .map(|i| {
                        let f = &files[i];
                        let path = f.path.clone();
                        let status = f.status;
                        let open = this.file_view.as_ref().is_some_and(|v| v.path == f.path && v.source == DiffSource::Commit(oid));
                        div()
                            .id(("cf", i))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .h(px(ROW_H))
                            .w_full()
                            .px(px(14.))
                            .cursor_pointer()
                            .when(open, |d| d.bg(c(BG_HIGHLIGHT)))
                            .hover(|s| s.bg(c(BG_HOVER)))
                            .child(status_badge(f.status.letter()))
                            .child(file_label(&f.path, f.old_path.as_deref()))
                            .child(
                                div()
                                    .flex()
                                    .flex_none()
                                    .gap(px(6.))
                                    .text_size(px(11.))
                                    .font_family(MONO_FONT)
                                    .when(f.additions > 0, |d| d.child(div().text_color(c(GREEN)).child(format!("+{}", f.additions))))
                                    .when(f.deletions > 0, |d| d.child(div().text_color(c(RED)).child(format!("−{}", f.deletions)))),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                window.focus(&this.focus);
                                this.open_file(DiffSource::Commit(oid), path.clone(), status, cx)
                            }))
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(self.files_scroll.clone())
        .flex_1()
        .py(px(4.));

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(header)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(8.))
                    .h(px(34.))
                    .px(px(14.))
                    .child(section_label(format!("{n} FILE{} CHANGED", if n == 1 { "" } else { "S" })))
                    .child(div().flex_1())
                    .child(div().text_size(px(11.)).font_family(MONO_FONT).text_color(c(GREEN)).child(format!("+{adds}")))
                    .child(div().text_size(px(11.)).font_family(MONO_FONT).text_color(c(RED)).child(format!("−{dels}"))),
            )
            .child(file_list)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::super::test_support::*;
    use super::*;

    #[test]
    fn paths_split_into_name_and_folder() {
        assert_eq!(split_path("src/ui/panel.rs"), ("panel.rs".into(), "src/ui/".into()));
        assert_eq!(split_path("README.md"), ("README.md".into(), "".into()));
    }

    /// Two unstaged files (the second untracked) and one staged.
    fn mixed_tree() -> TempRepo {
        let r = three_commits();
        r.write("one.txt", "changed\n");
        r.write("new.txt", "new\n");
        r.write("two.txt", "staged\n");
        r.git(&["add", "two.txt"]);
        r
    }

    #[gpui::test]
    fn wip_rows_group_the_working_tree(cx: &mut TestAppContext) {
        let r = mixed_tree();
        let (view, cx) = open(&r, cx);
        let rows = view.read_with(cx, |v, _| v.wip_rows());
        assert_eq!(
            rows,
            [
                WipRow::Header(Group::Unstaged, 2),
                WipRow::File(Group::Unstaged, 0),
                WipRow::File(Group::Unstaged, 1),
                WipRow::Gap,
                WipRow::Header(Group::Staged, 1),
                WipRow::File(Group::Staged, 0),
            ]
        );
    }

    #[gpui::test]
    fn conflicted_files_come_first(cx: &mut TestAppContext) {
        let r = three_commits();
        r.git(&["checkout", "-q", "-b", "topic"]);
        r.commit("one.txt", "theirs\n", "theirs");
        r.git(&["checkout", "-q", "main"]);
        r.commit("one.txt", "ours\n", "ours");
        let _ = crate::git::ops::Git::new(&r.0).run(&["merge", "-q", "topic"]);
        let (view, cx) = open(&r, cx);
        let rows = view.read_with(cx, |v, _| v.wip_rows());
        assert_eq!(rows[..3], [WipRow::Header(Group::Conflicted, 1), WipRow::File(Group::Conflicted, 0), WipRow::Gap]);
        // The resolver opens those, so stepping through files skips them.
        let files = view.read_with(cx, |v, _| v.panel_files());
        assert!(files.iter().all(|(_, p, ..)| p != "one.txt"), "{files:?}");
    }

    #[gpui::test]
    fn panel_files_in_display_order(cx: &mut TestAppContext) {
        let r = mixed_tree();
        let (view, cx) = open(&r, cx);
        let files = view.read_with(cx, |v, _| v.panel_files());
        assert_eq!(
            files,
            [
                (DiffSource::Unstaged, "one.txt".to_string(), FileStatus::Modified, 1),
                (DiffSource::Unstaged, "new.txt".to_string(), FileStatus::Untracked, 2),
                (DiffSource::Staged, "two.txt".to_string(), FileStatus::Modified, 5),
            ]
        );
    }

    #[gpui::test]
    fn arrows_walk_the_file_list(cx: &mut TestAppContext) {
        let r = mixed_tree();
        let (view, cx) = open(&r, cx);
        let open_file = |cx: &mut gpui::VisualTestContext| {
            view.read_with(cx, |v, _| v.file_view.as_ref().map(|f| (f.source.clone(), f.path.clone())))
        };
        cx.simulate_keystrokes("right");
        assert_eq!(open_file(cx), Some((DiffSource::Unstaged, "one.txt".into())));
        cx.simulate_keystrokes("down");
        assert_eq!(open_file(cx), Some((DiffSource::Unstaged, "new.txt".into())));
        cx.simulate_keystrokes("j");
        assert_eq!(open_file(cx), Some((DiffSource::Staged, "two.txt".into())));
        // Stops at the last file, and doesn't move the graph selection.
        cx.simulate_keystrokes("down");
        assert_eq!(open_file(cx), Some((DiffSource::Staged, "two.txt".into())));
        assert_eq!(view.read_with(cx, |v, _| v.selected), Some(crate::git::repo::wip_oid()));
        cx.simulate_keystrokes("up up up");
        assert_eq!(open_file(cx), Some((DiffSource::Unstaged, "one.txt".into())));
        cx.simulate_keystrokes("left");
        assert_eq!(open_file(cx), None);
    }

    #[gpui::test]
    fn arrows_walk_a_commits_files(cx: &mut TestAppContext) {
        let r = three_commits();
        r.write("a.txt", "a\n");
        r.write("b.txt", "b\n");
        r.git(&["add", "."]);
        r.git(&["commit", "-q", "-m", "two files"]);
        let (view, cx) = open(&r, cx);
        let head = view.read_with(cx, |v, _| v.selected.unwrap());
        assert_eq!(view.read_with(cx, |v, _| v.panel_files().len()), 2);
        cx.simulate_keystrokes("right");
        cx.run_until_parked();
        view.read_with(cx, |v, _| {
            let fv = v.file_view.as_ref().unwrap();
            assert_eq!((fv.source.clone(), fv.path.as_str()), (DiffSource::Commit(head), "a.txt"));
            assert!(fv.diff.is_some());
        });
        cx.simulate_keystrokes("down");
        assert_eq!(view.read_with(cx, |v, _| v.file_view.as_ref().unwrap().path.clone()), "b.txt");
    }

    #[gpui::test]
    fn file_menus(cx: &mut TestAppContext) {
        let r = three_commits();
        let (view, cx) = open(&r, cx);
        let menu = |staged, untracked, cx: &mut gpui::VisualTestContext| {
            view.read_with(cx, |v, _| labels(&v.file_menu("f.txt".into(), staged, untracked)))
        };
        let common = ["Rename / move…", "Open in default app", "Reveal in Finder", "Copy path"];
        assert_eq!(menu(true, false, cx), [&["Unstage"][..], &common].concat());
        assert_eq!(menu(false, false, cx), [&["Stage"][..], &common, &["—", "Discard changes…"]].concat());
        assert_eq!(menu(false, true, cx).last().unwrap(), "Delete file…");
    }

    #[gpui::test]
    fn stage_and_discard_from_the_file_menu(cx: &mut TestAppContext) {
        let r = mixed_tree();
        let (view, cx) = open(&r, cx);
        let items = view.read_with(cx, |v, _| v.file_menu("one.txt".into(), false, false));
        click(&view, items, "Stage", cx);
        assert_eq!(r.git(&["diff", "--cached", "--name-only"]).split_whitespace().collect::<Vec<_>>(), ["one.txt", "two.txt"]);

        // Deleting an untracked file asks first.
        let items = view.read_with(cx, |v, _| v.file_menu("new.txt".into(), false, true));
        click(&view, items, "Delete file…", cx);
        assert!(r.0.join("new.txt").exists());
        view.read_with(cx, |v, _| match &v.modal {
            Some(super::super::app::Modal::Confirm { body, .. }) => assert_eq!(body.as_ref(), "new.txt is untracked and will be deleted."),
            _ => panic!("expected a confirmation"),
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(!r.0.join("new.txt").exists());
    }

    fn picked(view: &gpui::Entity<GitPanda>, cx: &mut gpui::VisualTestContext) -> Vec<String> {
        view.read_with(cx, |v, _| v.selected_files().into_iter().map(|(p, _)| p).collect())
    }

    /// `mixed_tree` plus a third unstaged file.
    fn three_unstaged() -> TempRepo {
        let r = mixed_tree();
        r.write("three.txt", "changed too\n");
        r
    }

    #[gpui::test]
    fn cmd_a_selects_every_unstaged_file(cx: &mut TestAppContext) {
        let r = three_unstaged();
        let (view, cx) = open(&r, cx);
        let all: Vec<String> = view.read_with(cx, |v, _| v.unstaged_files().into_iter().map(|(p, _)| p).collect());
        assert_eq!(all.len(), 3);
        cx.simulate_keystrokes("cmd-a");
        assert_eq!(picked(&view, cx), all);
        // The staged file is not part of it, and escape lets go.
        assert!(!picked(&view, cx).contains(&"two.txt".to_string()));
        cx.simulate_keystrokes("escape");
        assert!(picked(&view, cx).is_empty());
        cx.simulate_keystrokes("ctrl-a");
        assert_eq!(picked(&view, cx).len(), 3);
    }

    #[gpui::test]
    fn cmd_a_does_nothing_on_a_commit(cx: &mut TestAppContext) {
        let r = three_unstaged();
        let (view, cx) = open(&r, cx);
        cx.simulate_keystrokes("down cmd-a");
        assert_ne!(view.read_with(cx, |v, _| v.selected), Some(wip_oid()));
        assert!(view.read_with(cx, |v, _| v.file_sel.is_empty()));
    }

    #[gpui::test]
    fn shift_arrows_extend_the_selection(cx: &mut TestAppContext) {
        let r = three_unstaged();
        let (view, cx) = open(&r, cx);
        let all: Vec<String> = view.read_with(cx, |v, _| v.unstaged_files().into_iter().map(|(p, _)| p).collect());
        let open_path = |cx: &mut gpui::VisualTestContext| view.read_with(cx, |v, _| v.file_view.as_ref().map(|f| f.path.clone()));
        cx.simulate_keystrokes("right");
        assert_eq!(open_path(cx).as_ref(), Some(&all[0]));
        cx.simulate_keystrokes("shift-down");
        assert_eq!(picked(&view, cx), all[..2]);
        assert_eq!(open_path(cx).as_ref(), Some(&all[1]));
        cx.simulate_keystrokes("shift-down shift-down");
        assert_eq!(picked(&view, cx), all, "stops at the last unstaged file");
        cx.simulate_keystrokes("shift-up");
        assert_eq!(picked(&view, cx), all[..2]);
        // The graph selection stays on the working tree.
        assert_eq!(view.read_with(cx, |v, _| v.selected), Some(wip_oid()));
        // A plain arrow goes back to one file.
        cx.simulate_keystrokes("down");
        assert!(picked(&view, cx).is_empty());
        assert_eq!(open_path(cx).as_ref(), Some(&all[2]));
    }

    #[gpui::test]
    fn discarding_the_selection(cx: &mut TestAppContext) {
        let r = three_unstaged();
        let (view, cx) = open(&r, cx);
        cx.simulate_keystrokes("cmd-a cmd-backspace");
        view.read_with(cx, |v, _| match &v.modal {
            Some(super::super::app::Modal::Confirm { title, body, .. }) => {
                assert_eq!(title.as_ref(), "Discard 3 files?");
                assert_eq!(body.as_ref(), "All unstaged changes to 3 files will be lost. 1 untracked file will be deleted.");
            }
            _ => panic!("expected a confirmation"),
        });
        assert!(r.0.join("new.txt").exists());
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(!r.0.join("new.txt").exists());
        assert_eq!(std::fs::read_to_string(r.0.join("one.txt")).unwrap(), "one\n");
        assert_eq!(std::fs::read_to_string(r.0.join("three.txt")).unwrap(), "three\n");
        // Staged changes are left alone.
        assert_eq!(r.git(&["diff", "--cached", "--name-only"]).trim(), "two.txt");
        view.read_with(cx, |v, _| {
            assert!(v.snap.as_ref().unwrap().status.unstaged.is_empty());
            assert!(v.file_sel.is_empty());
        });
    }

    #[gpui::test]
    fn row_actions_and_menu_cover_the_selection(cx: &mut TestAppContext) {
        let r = three_unstaged();
        let (view, cx) = open(&r, cx);
        cx.simulate_keystrokes("right shift-down");
        let two = picked(&view, cx);
        assert_eq!(view.read_with(cx, |v, _| v.action_files(&two[0], false)).len(), 2);
        let items = view.read_with(cx, |v, _| v.selection_menu());
        assert_eq!(labels(&items), ["Stage 2 files", "—", "Discard 2 files…"]);
        click(&view, items, "Stage 2 files", cx);
        let mut staged: Vec<String> = r.git(&["diff", "--cached", "--name-only"]).split_whitespace().map(String::from).collect();
        staged.sort();
        let mut want = two.clone();
        want.push("two.txt".into());
        want.sort();
        assert_eq!(staged, want);
    }
}
