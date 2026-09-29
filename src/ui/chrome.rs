//! Toolbar, operation banner, status bar, welcome screen, and overlays
//! (dialogs, context menu, toasts).

use gpui::{
    AnyElement, Context, Window, anchored, deferred, div, prelude::*, px,
};

use crate::git::ops::RebaseAction;
use crate::git::repo::OpState;

use super::app::{MenuItem, Modal, ToastKind, GitPanda};
use super::theme::*;
use super::widgets::*;

fn tool(id: &'static str, glyph: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(1.))
        .w(px(56.))
        .h(px(42.))
        .rounded(px(7.))
        .cursor_pointer()
        .text_color(c(FG_DARK))
        .hover(|s| s.bg(alpha(BLUE, 0.1)).text_color(c(CYAN)))
        .child(div().text_size(px(15.)).child(glyph))
        .child(div().text_size(px(10.5)).child(label))
}

/// Empty toolbar space that drags the window (the titlebar is transparent).
pub(super) fn drag_space() -> gpui::Div {
    div().flex_1().h_full().on_mouse_down(gpui::MouseButton::Left, |ev, window, _| {
        if ev.click_count == 2 {
            window.zoom_window();
        } else {
            window.start_window_move();
        }
    })
}

fn action_color(a: RebaseAction) -> u32 {
    match a {
        RebaseAction::Pick => BLUE,
        RebaseAction::Reword => YELLOW,
        RebaseAction::Squash => MAGENTA,
        RebaseAction::Fixup => PURPLE,
        RebaseAction::Drop => RED,
    }
}

impl GitPanda {
    pub fn render_toolbar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let snap = self.snap.as_ref();
        let name = snap.map(|s| s.name.clone()).unwrap_or_else(|| "…".into());
        let branch = snap.and_then(|s| s.head.branch.clone());
        let head_short = snap.and_then(|s| s.head.oid).map(|o| o.to_string()[..7].to_string());
        let upstream = snap.and_then(|s| s.branches.iter().find(|b| b.is_head)).map(|b| (b.ahead, b.behind, b.upstream.is_some()));

        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.))
            .h(px(54.))
            .px(px(14.))
            .bg(c(BG_DARK))
            .border_b_1()
            .border_color(c(BORDER))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .mr(px(10.))
                    .child(logo(26.))
                    .child(
                        div()
                            .id("repo")
                            .flex()
                            .flex_col()
                            .px(px(6.))
                            .py(px(2.))
                            .rounded(px(6.))
                            .cursor_pointer()
                            .hover(|s| s.bg(c(BG_HIGHLIGHT)))
                            .on_click(cx.listener(|this, _, _, cx| this.prompt_open(cx)))
                            .child(div().text_size(px(10.)).text_color(c(COMMENT)).child("REPOSITORY"))
                            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(format!("{name} ▾"))),
                    )
                    .child(div().w(px(1.)).h(px(26.)).bg(c(BORDER_HI)).mx(px(4.)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .px(px(6.))
                            .child(div().text_size(px(10.)).text_color(c(COMMENT)).child("BRANCH"))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(c(GREEN))
                                            .child(branch.map(|b| b.to_string()).unwrap_or_else(|| format!("detached @ {}", head_short.unwrap_or_default()))),
                                    )
                                    .children(upstream.filter(|u| u.2 && (u.0 > 0 || u.1 > 0)).map(|(a, b, _)| {
                                        div().text_size(px(11.)).text_color(c(CYAN)).child(format!("↑{a} ↓{b}"))
                                    })),
                            ),
                    ),
            )
            .child(drag_space())
            .child(tool("fetch", "⟳", "Fetch").on_click(cx.listener(|this, _, _, cx| this.run("Fetching", |g| g.fetch().map(|_| "Fetched all remotes".into()), cx))))
            .child(tool("pull", "↓", "Pull").on_click(cx.listener(|this, _, _, cx| this.run("Pulling", |g| g.pull().map(|_| "Pulled".into()), cx))))
            .child(tool("push", "↑", "Push").on_click(cx.listener(|this, _, _, cx| this.push(false, cx))))
            .child(div().w(px(1.)).h(px(28.)).bg(c(BORDER_HI)).mx(px(6.)))
            .child(tool("branch", "⑂", "Branch").on_click(cx.listener(|this, _, window, cx| {
                let at = this.selected.filter(|o| *o != crate::git::repo::wip_oid()).map_or("HEAD".to_string(), |o| o.to_string());
                this.new_branch_at(at, window, cx)
            })))
            .child(tool("stash", "⊟", "Stash").on_click(cx.listener(|this, _, window, cx| this.stash(window, cx))))
            .child(tool("pop", "⊞", "Pop").on_click(cx.listener(|this, _, _, cx| this.run("Popping", |g| g.stash_pop(0).map(|_| "Popped stash".into()), cx))))
            .child(drag_space())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(8.))
                    .w(px(220.))
                    .children(self.busy.clone().map(|b| {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .px(px(10.))
                            .h(px(26.))
                            .rounded_full()
                            .bg(alpha(BLUE, 0.12))
                            .text_size(px(12.))
                            .text_color(c(BLUE))
                            .child(div().size(px(7.)).rounded_full().bg(c(BLUE)))
                            .child(b)
                    })),
            )
            .into_any_element()
    }

    pub fn render_banner(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let snap = self.snap.as_ref()?;
        if snap.state == OpState::Clean {
            return None;
        }
        let state = snap.state;
        let conflicts = snap.status.conflicted.len();
        let progress = snap.rebase_progress.map(|(a, b)| format!(" {a}/{b}")).unwrap_or_default();
        let msg = if conflicts > 0 {
            format!("{conflicts} conflicted file{} — resolve them, then continue", if conflicts == 1 { "" } else { "s" })
        } else {
            "No conflicts left — continue when ready".to_string()
        };
        let color = if conflicts > 0 { ORANGE } else { GREEN1 };
        Some(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(10.))
                .h(px(40.))
                .px(px(14.))
                .bg(alpha(color, 0.12))
                .border_b_1()
                .border_color(alpha(color, 0.4))
                .child(pill(format!("{}{progress}", state.label()), color, true).font_weight(gpui::FontWeight::BOLD))
                .child(div().text_color(c(FG)).child(msg))
                .child(div().flex_1())
                .when(matches!(state, OpState::Rebase | OpState::CherryPick | OpState::Revert), |d| {
                    d.child(button("op-skip", "Skip", Tone::Normal).on_click(cx.listener(move |this, _, _, cx| {
                        this.run("Skipping", move |g| g.skip_op(state).map(|_| String::new()), cx)
                    })))
                })
                .child(button("op-abort", "Abort", Tone::Danger).on_click(cx.listener(move |this, _, _, cx| {
                    this.run("Aborting", move |g| g.abort_op(state).map(|_| "Aborted".into()), cx)
                })))
                .when(state != OpState::Bisect, |d| {
                    d.child(
                        button("op-continue", "Continue", if conflicts == 0 { Tone::Primary } else { Tone::Normal })
                            .when(conflicts > 0, |b| b.opacity(0.5))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if conflicts > 0 {
                                    return this.toast(ToastKind::Info, "Resolve every conflicted file first", cx);
                                }
                                this.run("Continuing", move |g| g.continue_op(state).map(|_| "Continued".into()), cx)
                            })),
                    )
                })
                .into_any_element(),
        )
    }

    pub fn render_statusbar(&self) -> AnyElement {
        let mut left = Vec::new();
        if let Some(s) = &self.snap {
            let n = s.commits.iter().filter(|c| c.oid != crate::git::repo::wip_oid()).count();
            left.push(format!("{n}{} commits", if s.truncated { "+" } else { "" }));
            left.push(format!("{} branches", s.branches.len()));
            left.push(format!("loaded in {:.1} ms", s.load_time.as_secs_f64() * 1000.));
        }
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(14.))
            .h(px(24.))
            .px(px(12.))
            .bg(c(BG_DARKER))
            .border_t_1()
            .border_color(c(BORDER))
            .text_size(px(11.))
            .text_color(c(DARK3))
            .child({
                let dot = if self.loading { YELLOW } else { GREEN1 };
                div().size(px(7.)).rounded_full().bg(c(dot))
            })
            .children(left.into_iter().map(|t| div().child(t)))
            .child(div().flex_1())
            .children(self.workdir.as_ref().map(|p| div().child(p.display().to_string())))
            .into_any_element()
    }

    pub fn render_welcome(&mut self, cx: &mut Context<Self>) -> AnyElement {
        // An empty project asks for repositories rather than a single one.
        let empty = self.workspace.project().filter(|p| p.repos.is_empty()).map(|p| p.name.clone());
        let subtitle = match &empty {
            Some(name) => format!("{name} has no repositories yet"),
            None => "A fast, friendly git client".into(),
        };
        div()
            .flex()
            .size_full()
            .items_center()
            .justify_center()
            .bg(c(BG))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(14.))
                    .child(logo(72.))
                    .child(div().text_size(px(28.)).font_weight(gpui::FontWeight::BOLD).child("gitpanda"))
                    .child(div().text_color(c(COMMENT)).child(subtitle))
                    .children(self.open_error.clone().map(|e| {
                        div().max_w(px(460.)).px(px(12.)).py(px(8.)).rounded(px(6.)).bg(alpha(RED, 0.1)).text_color(c(RED)).text_size(px(12.)).child(e)
                    }))
                    .child(
                        button("open", if empty.is_some() { "Add repositories…" } else { "Open repository…  ⌘O" }, Tone::Primary)
                            .h(px(36.))
                            .px(px(18.))
                            .text_size(px(13.))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if empty.is_some() {
                                    this.prompt_add_repos(cx)
                                } else {
                                    this.prompt_open(cx)
                                }
                            })),
                    ),
            )
            .into_any_element()
    }

    // ---- overlays ------------------------------------------------------------

    pub fn render_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let modal = self.modal.as_ref()?;
        let card = |w: f32| {
            div()
                .flex()
                .flex_col()
                .gap(px(14.))
                .w(px(w))
                .p(px(20.))
                .rounded(px(12.))
                .bg(c(BG))
                .border_1()
                .border_color(c(BORDER_HI))
                .shadow_lg()
        };
        let footer = |confirm: gpui::SharedString, tone: Tone| {
            div()
                .flex()
                .gap(px(8.))
                .justify_end()
                .child(button("cancel", "Cancel", Tone::Ghost).on_click(cx.listener(|this, _, window, cx| this.close_modal(window, cx))))
                .child(button("ok", confirm, tone).on_click(cx.listener(|this, _, window, cx| this.submit_modal(window, cx))))
        };
        let content = match modal {
            Modal::Input { title, hint, input, confirm, .. } => card(460.)
                .child(div().text_size(px(16.)).font_weight(gpui::FontWeight::SEMIBOLD).child(title.clone()))
                .children(hint.clone().map(|h| div().text_size(px(12.)).text_color(c(COMMENT)).child(h)))
                .child(input.clone())
                .child(footer(confirm.clone(), Tone::Primary)),
            Modal::Confirm { title, body, confirm, danger, .. } => card(440.)
                .child(div().text_size(px(16.)).font_weight(gpui::FontWeight::SEMIBOLD).child(title.clone()))
                .child(div().text_size(px(13.)).text_color(c(FG_DARK)).child(body.clone()))
                .child(footer(confirm.clone(), if *danger { Tone::Danger } else { Tone::Primary })),
            Modal::Rebase(ed) => {
                let n = ed.steps.len();
                let base = ed.base.map_or("root".to_string(), |b| b.to_string()[..7].to_string());
                let cursor = ed.cursor;
                let editing = ed.editing.as_ref().map(|(i, input)| (*i, input.clone()));
                let squash_first = ed.steps.first().is_some_and(|s| matches!(s.action, RebaseAction::Squash | RebaseAction::Fixup));
                let rows = ed.steps.iter().enumerate().rev().map(|(i, s)| {
                    let col = action_color(s.action);
                    let active = i == cursor;
                    let dropped = s.action == RebaseAction::Drop;
                    let folded = matches!(s.action, RebaseAction::Squash | RebaseAction::Fixup);
                    div()
                        .id(("step", i))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .h(px(34.))
                        .px(px(10.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .when(active, |d| d.bg(c(BG_HIGHLIGHT)).border_1().border_color(c(BLUE0)))
                        .when(!active, |d| d.hover(|s| s.bg(c(BG_HOVER))))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(Modal::Rebase(ed)) = &mut this.modal {
                                ed.cursor = i;
                            }
                            cx.notify();
                        }))
                        .child(
                            div()
                                .id(("act", i))
                                .w(px(64.))
                                .flex_none()
                                .child(pill(s.action.label(), col, true).justify_center().w_full())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(Modal::Rebase(ed)) = &mut this.modal {
                                        ed.cursor = i;
                                        let a = ed.steps[i].action;
                                        let k = RebaseAction::ALL.iter().position(|x| *x == a).unwrap_or(0);
                                        ed.steps[i].action = RebaseAction::ALL[(k + 1) % RebaseAction::ALL.len()];
                                    }
                                    cx.notify();
                                })),
                        )
                        .when(folded, |d| d.child(div().text_color(c(col)).child("↳")))
                        .child(div().font_family(MONO_FONT).text_size(px(11.5)).text_color(c(DARK5)).child(s.short.clone()))
                        .child(
                            div()
                                .flex_1()
                                .truncate()
                                .text_color(if dropped { c(COMMENT) } else { c(FG) })
                                .when(dropped, |d| d.line_through())
                                .child(s.summary.clone()),
                        )
                        .when(s.message.is_some(), |d| d.child(pill("edited", YELLOW, false)))
                })
                .collect::<Vec<_>>();

                card(680.)
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(px(10.))
                            .child(div().text_size(px(16.)).font_weight(gpui::FontWeight::SEMIBOLD).child("Interactive rebase"))
                            .child(div().text_size(px(12.)).text_color(c(COMMENT)).child(format!("{n} commit{} onto {base} · newest first", if n == 1 { "" } else { "s" }))),
                    )
                    .child(
                        div()
                            .id("steps")
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .max_h(px(360.))
                            .overflow_y_scroll()
                            .children(rows),
                    )
                    .when(squash_first, |d| {
                        d.child(div().text_size(px(12.)).text_color(c(RED)).child("The oldest commit can't be squashed or fixed up: there's nothing below it."))
                    })
                    .children(editing.clone().map(|(i, input)| {
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .child(section_label(format!("MESSAGE FOR {} · ⌘⏎ done · esc cancel", self.rebase_short(i))))
                            .child(input)
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(12.))
                            .text_size(px(11.))
                            .text_color(c(COMMENT))
                            .children(
                                [("p", "pick"), ("r", "reword"), ("s", "squash"), ("f", "fixup"), ("d", "drop"), ("⌥↑↓", "reorder"), ("⏎", "start")]
                                    .into_iter()
                                    .map(|(k, v)| {
                                        div()
                                            .flex()
                                            .gap(px(4.))
                                            .child(div().px(px(5.)).rounded(px(3.)).bg(c(BG_HIGHLIGHT)).text_color(c(FG_DARK)).child(k))
                                            .child(v)
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .items_center()
                            .child(button("mv-up", "▲", Tone::Normal).on_click(cx.listener(|this, _, _, cx| this.rebase_move(true, cx))))
                            .child(button("mv-down", "▼", Tone::Normal).on_click(cx.listener(|this, _, _, cx| this.rebase_move(false, cx))))
                            .child(button("edit-msg", "Edit message", Tone::Normal).on_click(cx.listener(|this, _, window, cx| this.edit_rebase_message(window, cx))))
                            .child(div().flex_1())
                            .child(button("cancel", "Cancel", Tone::Ghost).on_click(cx.listener(|this, _, window, cx| this.close_modal(window, cx))))
                            .child(
                                button("ok", if editing.is_some() { "Done editing" } else { "Start rebase" }, Tone::Primary)
                                    .on_click(cx.listener(|this, _, window, cx| this.submit_modal(window, cx))),
                            ),
                    )
            }
        };
        let _ = window;
        Some(
            div()
                .id("modal-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(alpha(0x05060a, 0.62))
                .occlude()
                .child(content)
                .into_any_element(),
        )
    }

    fn rebase_short(&self, i: usize) -> String {
        match &self.modal {
            Some(Modal::Rebase(ed)) => ed.steps.get(i).map(|s| s.short.clone()).unwrap_or_default(),
            _ => String::new(),
        }
    }

    pub fn render_menu(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        let items = menu.items.iter().enumerate().map(|(i, item)| match item {
            MenuItem::Separator => div().h(px(1.)).my(px(4.)).mx(px(6.)).bg(c(BORDER_HI)).into_any_element(),
            MenuItem::Action { label, danger, action } => {
                let action = action.clone();
                let danger = *danger;
                div()
                    .id(("menu", i))
                    .flex()
                    .items_center()
                    .h(px(28.))
                    .px(px(10.))
                    .rounded(px(5.))
                    .text_size(px(12.5))
                    .whitespace_nowrap()
                    .text_color(if danger { c(RED) } else { c(FG) })
                    .cursor_pointer()
                    .hover(move |s| s.bg(if danger { alpha(RED, 0.14) } else { alpha(BLUE0, 0.6) }))
                    .child(label.clone())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.menu = None;
                        action(this, window, cx);
                        cx.notify();
                    }))
                    .into_any_element()
            }
        });
        Some(
            deferred(
                anchored().position(menu.pos).snap_to_window_with_margin(px(8.)).child(
                    div()
                        .id("menu")
                        .occlude()
                        .flex()
                        .flex_col()
                        .min_w(px(220.))
                        .p(px(5.))
                        .rounded(px(8.))
                        .bg(c(BG_DARK))
                        .border_1()
                        .border_color(c(BORDER_HI))
                        .shadow_lg()
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.menu = None;
                            cx.notify();
                        }))
                        .children(items),
                ),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }

    pub fn render_toasts(&mut self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .absolute()
            .bottom(px(36.))
            .right(px(16.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .children(self.toasts.iter().map(|t| {
                let col = match t.kind {
                    ToastKind::Info => BLUE,
                    ToastKind::Success => GREEN,
                    ToastKind::Error => RED,
                };
                let id = t.id;
                div()
                    .id(("toast", id as usize))
                    .flex()
                    .items_start()
                    .gap(px(10.))
                    .max_w(px(420.))
                    .px(px(14.))
                    .py(px(10.))
                    .rounded(px(8.))
                    .bg(c(BG_DARK))
                    .border_1()
                    .border_color(alpha(col, 0.45))
                    .shadow_lg()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toasts.retain(|t| t.id != id);
                        cx.notify();
                    }))
                    .child(div().mt(px(5.)).size(px(8.)).flex_none().rounded_full().bg(c(col)))
                    .child(div().text_size(px(12.5)).text_color(c(FG)).child(t.text.clone()))
            }))
            .into_any_element()
    }
}
