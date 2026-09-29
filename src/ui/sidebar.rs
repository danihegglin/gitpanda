//! Left sidebar: local and remote branches, tags and stashes, as one
//! virtualized list of equally tall rows.

use git2::Oid;
use gpui::{
    AnyElement, ClickEvent, Context, MouseButton, MouseDownEvent, SharedString, div, prelude::*,
    px, uniform_list,
};

use super::app::{Menu, MenuItem, ToastKind, GitPanda, action};
use super::theme::*;
use super::widgets::*;

const ROW_H: f32 = 26.;
pub const WIDTH: f32 = 250.;

#[derive(Clone, Copy)]
enum Row {
    Header(&'static str, usize),
    Local(usize),
    Remote(usize),
    Tag(usize),
    Stash(usize),
}

impl GitPanda {
    fn sidebar_rows(&self) -> Vec<Row> {
        let Some(s) = &self.snap else { return Vec::new() };
        let mut rows = Vec::new();
        let section = |rows: &mut Vec<Row>, name: &'static str, n: usize, f: &dyn Fn(usize) -> Row| {
            rows.push(Row::Header(name, n));
            if !self.collapsed.contains(name) {
                rows.extend((0..n).map(f));
            }
        };
        section(&mut rows, "LOCAL", s.branches.len(), &Row::Local);
        section(&mut rows, "REMOTE", s.remotes.len(), &Row::Remote);
        section(&mut rows, "TAGS", s.tags.len(), &Row::Tag);
        section(&mut rows, "STASHES", s.stashes.len(), &Row::Stash);
        rows
    }

    pub fn render_sidebar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let rows = self.sidebar_rows();
        let count = rows.len();
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(WIDTH))
            .h_full()
            .bg(c(BG_DARK))
            .border_r_1()
            .border_color(c(BORDER))
            .child(
                uniform_list(
                    "sidebar",
                    count,
                    cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                        range.map(|i| this.render_sidebar_row(rows[i], i, cx)).collect::<Vec<_>>()
                    }),
                )
                .flex_1()
                .pt(px(6.)),
            )
            .into_any_element()
    }

    fn render_sidebar_row(&mut self, row: Row, i: usize, cx: &mut Context<Self>) -> AnyElement {
        let snap = self.snap.as_ref().unwrap();
        let base = div()
            .id(("side", i))
            .flex()
            .items_center()
            .gap(px(8.))
            .h(px(ROW_H))
            .w_full()
            .px(px(14.))
            .rounded(px(5.))
            .cursor_pointer();

        match row {
            Row::Header(name, n) => {
                let collapsed = self.collapsed.contains(name);
                base.child(
                    div()
                        .text_color(c(DARK3))
                        .text_size(px(9.))
                        .w(px(10.))
                        .child(if collapsed { "▶" } else { "▼" }),
                )
                .child(section_label(name))
                .child(div().flex_1())
                .child(div().text_size(px(11.)).text_color(c(DARK3)).child(n.to_string()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.collapsed.remove(name) {
                        this.collapsed.insert(name);
                    }
                    cx.notify();
                }))
                .into_any_element()
            }
            Row::Local(bi) => {
                let b = &snap.branches[bi];
                let oid = b.oid;
                let name = b.name.to_string();
                let selected = self.selected == Some(oid);
                let track: Option<SharedString> = match (b.ahead, b.behind) {
                    (0, 0) => None,
                    (a, 0) => Some(format!("↑{a}").into()),
                    (0, d) => Some(format!("↓{d}").into()),
                    (a, d) => Some(format!("↑{a} ↓{d}").into()),
                };
                let n2 = name.clone();
                base.when(selected, |d| d.bg(tint(GREEN, 0.14)))
                    .hover(|s| s.bg(c(BG_HOVER)))
                    .child(
                        div()
                            .size(px(8.))
                            .rounded_full()
                            .when(b.is_head, |d| d.bg(c(GREEN)))
                            .when(!b.is_head, |d| d.border_1().border_color(c(DARK3))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .text_color(if b.is_head { c(FG) } else { c(FG_DARK) })
                            .when(b.is_head, |d| d.font_weight(gpui::FontWeight::SEMIBOLD))
                            .child(b.name.clone()),
                    )
                    .children(track.map(|t| div().text_size(px(11.)).text_color(c(CYAN)).child(t)))
                    .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                        if ev.click_count() == 2 {
                            let n = n2.clone();
                            this.run("Checking out", move |g| g.checkout(&n).map(|_| format!("Switched to {n}")), cx);
                        } else {
                            this.reveal(oid, cx);
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            let items = this.branch_menu(&name, false);
                            this.menu = Some(Menu { pos: ev.position, items });
                            cx.notify();
                        }),
                    )
                    .into_any_element()
            }
            Row::Remote(ri) => {
                let b = &snap.remotes[ri];
                let oid = b.oid;
                let name = b.name.to_string();
                let n2 = name.clone();
                base.when(self.selected == Some(oid), |d| d.bg(tint(CYAN, 0.14)))
                    .hover(|s| s.bg(c(BG_HOVER)))
                    .child(div().text_color(c(CYAN)).text_size(px(11.)).child("☁"))
                    .child(div().flex_1().truncate().text_color(c(FG_DARK)).child(b.name.clone()))
                    .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                        if ev.click_count() == 2 {
                            let n = n2.clone();
                            this.run("Checking out", move |g| g.checkout_remote(&n).map(|_| format!("Checked out {n}")), cx);
                        } else {
                            this.reveal(oid, cx);
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            let items = this.branch_menu(&name, true);
                            this.menu = Some(Menu { pos: ev.position, items });
                            cx.notify();
                        }),
                    )
                    .into_any_element()
            }
            Row::Tag(ti) => {
                let (name, oid) = snap.tags[ti].clone();
                let n = name.to_string();
                base.when(self.selected == Some(oid), |d| d.bg(tint(YELLOW, 0.14)))
                    .hover(|s| s.bg(c(BG_HOVER)))
                    .child(div().text_color(c(YELLOW)).text_size(px(11.)).child("⌁"))
                    .child(div().flex_1().truncate().text_color(c(FG_DARK)).child(name))
                    .on_click(cx.listener(move |this, _, _, cx| this.reveal(oid, cx)))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            let items = this.tag_menu(&n, oid);
                            this.menu = Some(Menu { pos: ev.position, items });
                            cx.notify();
                        }),
                    )
                    .into_any_element()
            }
            Row::Stash(si) => {
                let st = &snap.stashes[si];
                let (oid, index) = (st.oid, st.index);
                base.when(self.selected == Some(oid), |d| d.bg(tint(MAGENTA, 0.14)))
                    .hover(|s| s.bg(c(BG_HOVER)))
                    .child(div().text_color(c(MAGENTA)).text_size(px(11.)).child("≡"))
                    .child(div().flex_1().truncate().text_color(c(FG_DARK)).child(st.message.clone()))
                    .on_click(cx.listener(move |this, _, _, cx| this.select(oid, cx)))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            let items = stash_menu(index);
                            this.menu = Some(Menu { pos: ev.position, items });
                            cx.notify();
                        }),
                    )
                    .into_any_element()
            }
        }
    }

    fn branch_menu(&self, name: &str, remote: bool) -> Vec<MenuItem> {
        let mut items = Vec::new();
        let head = self.head_branch();
        let current = head.clone().unwrap_or_else(|| "HEAD".into());
        let is_head = !remote && head.as_deref() == Some(name);
        let mut add = |label: String, danger: bool, f: super::app::Action| {
            items.push(MenuItem::Action { label: label.into(), danger, action: f });
        };
        let n = name.to_string();
        if !is_head {
            let n1 = n.clone();
            add(format!("Checkout {n}"), false, action(move |this, _, cx| {
                let n = n1.clone();
                if remote {
                    this.run("Checking out", move |g| g.checkout_remote(&n).map(|_| format!("Checked out {n}")), cx);
                } else {
                    this.run("Checking out", move |g| g.checkout(&n).map(|_| format!("Switched to {n}")), cx);
                }
            }));
            let n2 = n.clone();
            let cur = current.clone();
            add(format!("Merge {n} into {current}"), false, action(move |this, _, cx| {
                let (n, cur) = (n2.clone(), cur.clone());
                this.run("Merging", move |g| g.merge(&n).map(|_| format!("Merged {n} into {cur}")), cx);
            }));
            let n3 = n.clone();
            let cur = current.clone();
            add(format!("Rebase {current} onto {n}"), false, action(move |this, _, cx| {
                let (n, cur) = (n3.clone(), cur.clone());
                this.run("Rebasing", move |g| g.rebase_onto(&n).map(|_| format!("Rebased {cur} onto {n}")), cx);
            }));
        } else {
            add("Push".into(), false, action(|this, _, cx| this.push(false, cx)));
            add("Force push (with lease)".into(), true, action(|this, _, cx| {
                this.confirm("Force push?", "The remote branch will be overwritten if nobody else has pushed to it since your last fetch.", "Force push", true,
                    action(|this, _, cx| this.push(true, cx)));
                cx.notify();
            }));
            add("Pull".into(), false, action(|this, _, cx| this.run("Pulling", |g| g.pull().map(|_| "Pulled".into()), cx)));
        }
        items.push(MenuItem::Separator);
        let mut add = |label: String, danger: bool, f: super::app::Action| {
            items.push(MenuItem::Action { label: label.into(), danger, action: f });
        };
        {
            let n = n.clone();
            add("Create branch from here…".into(), false, action(move |this, w, cx| this.new_branch_at(n.clone(), w, cx)));
        }
        if !remote {
            let n2 = n.clone();
            add("Rename…".into(), false, action(move |this, w, cx| this.rename_branch(n2.clone(), w, cx)));
        }
        {
            let n = n.clone();
            add("Copy name".into(), false, action(move |this, _, cx| {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(n.clone()));
                this.toast(ToastKind::Info, "Copied branch name", cx);
            }));
        }
        if !is_head {
            items.push(MenuItem::Separator);
            let label = if remote { format!("Delete {n} on remote…") } else { format!("Delete {n}…") };
            items.push(MenuItem::Action {
                label: label.into(),
                danger: true,
                action: action(move |this, _, cx| {
                    let n = n.clone();
                    let body = if remote {
                        format!("{n} will be deleted on the remote for everyone.")
                    } else {
                        format!("{n} will be force-deleted, even if it isn't merged.")
                    };
                    this.confirm("Delete branch?", body, "Delete", true, action(move |this, _, cx| {
                        let n = n.clone();
                        if remote {
                            this.run("Deleting", move |g| g.delete_remote_branch(&n).map(|_| format!("Deleted {n}")), cx);
                        } else {
                            this.run("Deleting", move |g| g.delete_branch(&n).map(|_| format!("Deleted {n}")), cx);
                        }
                    }));
                    cx.notify();
                }),
            });
        }
        items
    }

    fn tag_menu(&self, name: &str, oid: Oid) -> Vec<MenuItem> {
        let n = name.to_string();
        let n2 = n.clone();
        vec![
            MenuItem::Action {
                label: format!("Checkout {n} (detached)").into(),
                danger: false,
                action: action(move |this, _, cx| {
                    let n = n2.clone();
                    this.run("Checking out", move |g| g.checkout(&n).map(|_| format!("HEAD detached at {n}")), cx)
                }),
            },
            MenuItem::Action {
                label: "Create branch here…".into(),
                danger: false,
                action: action(move |this, w, cx| this.new_branch_at(oid.to_string(), w, cx)),
            },
            MenuItem::Separator,
            MenuItem::Action {
                label: format!("Delete tag {n}").into(),
                danger: true,
                action: action(move |this, _, cx| {
                    let n = n.clone();
                    this.run("Deleting", move |g| g.delete_tag(&n).map(|_| format!("Deleted tag {n}")), cx)
                }),
            },
        ]
    }
}

fn stash_menu(index: usize) -> Vec<MenuItem> {
    vec![
        MenuItem::Action {
            label: "Pop stash".into(),
            danger: false,
            action: action(move |this, _, cx| this.run("Popping", move |g| g.stash_pop(index).map(|_| "Popped stash".into()), cx)),
        },
        MenuItem::Action {
            label: "Apply stash".into(),
            danger: false,
            action: action(move |this, _, cx| this.run("Applying", move |g| g.stash_apply(index).map(|_| "Applied stash".into()), cx)),
        },
        MenuItem::Separator,
        MenuItem::Action {
            label: "Drop stash…".into(),
            danger: true,
            action: action(move |this, _, cx| {
                this.confirm("Drop stash?", "The stashed changes will be deleted.", "Drop", true, action(move |this, _, cx| {
                    this.run("Dropping", move |g| g.stash_drop(index).map(|_| "Dropped stash".into()), cx)
                }));
                cx.notify();
            }),
        },
    ]
}
