//! The commit graph: a virtualized list where each visible row paints its
//! slice of the lanes on a canvas.

use git2::Oid;
use gpui::{
    AnyElement, Bounds, ClickEvent, Context, MouseButton, MouseDownEvent, PathBuilder, Pixels,
    SharedString, Window, canvas, div, fill, point, prelude::*, px, size, uniform_list,
};

use crate::git::graph::{GraphRow, Half};
use crate::git::ops::RebaseAction;
use crate::git::repo::{RefKind, RefLabel, wip_oid};

use super::app::{Menu, MenuItem, ToastKind, GitPanda, action};
use super::theme::*;
use super::widgets::*;

pub const ROW_H: f32 = 30.;
const LANE_W: f32 = 16.;
const GRAPH_PAD: f32 = 10.;
const MAX_VISIBLE_LANES: u16 = 14;

impl GitPanda {
    pub fn render_center(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.conflict.is_some() {
            return self.render_conflict(window, cx);
        }
        if self.file_view.is_some() {
            return self.render_diff(window, cx);
        }
        self.render_graph(cx)
    }

    fn graph_width(&self) -> f32 {
        let lanes = self.snap.as_ref().map_or(1, |s| s.graph_width.min(MAX_VISIBLE_LANES));
        // Never narrower than the "GRAPH" header.
        (GRAPH_PAD * 2. + lanes as f32 * LANE_W).max(60.)
    }

    fn render_graph(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let count = self.snap.as_ref().map_or(0, |s| s.commits.len());
        let gw = self.graph_width();
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .h(px(28.))
            .px(px(0.))
            .border_b_1()
            .border_color(c(BORDER))
            .bg(c(BG_DARK))
            .text_size(px(10.5))
            .font_weight(gpui::FontWeight::BOLD)
            .text_color(c(DARK5))
            .child(div().w(px(gw)).flex_none().pl(px(GRAPH_PAD)).child("GRAPH"))
            .child(div().flex_1().child("COMMIT MESSAGE"))
            .child(div().w(px(170.)).flex_none().child("AUTHOR"))
            .child(div().w(px(96.)).flex_none().child("DATE"))
            .child(div().w(px(76.)).flex_none().child("SHA"));

        let list = if count == 0 {
            div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .text_color(c(COMMENT))
                .child(if self.loading { "Loading history…" } else { "No commits yet" })
                .into_any_element()
        } else {
            uniform_list(
                "graph",
                count,
                cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                    range.map(|i| this.render_commit_row(i, gw, cx)).collect::<Vec<_>>()
                }),
            )
            .track_scroll(self.graph_scroll.clone())
            .flex_1()
            .into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(c(BG))
            .child(header)
            .child(list)
            .into_any_element()
    }

    fn render_commit_row(&mut self, i: usize, gw: f32, cx: &mut Context<Self>) -> AnyElement {
        let snap = self.snap.as_ref().unwrap();
        let commit = &snap.commits[i];
        let row = snap.graph[i].clone();
        let oid = commit.oid;
        let is_wip = oid == wip_oid();
        let is_head = snap.head.oid == Some(oid);
        let is_merge = commit.parents.len() > 1;
        let selected = self.selected == Some(oid);
        let in_range = self.range.is_some_and(|(a, b)| i >= a && i <= b);
        let lane_color = lane(row.color);

        let bg = if selected {
            tint(lane_color, 0.16)
        } else if in_range {
            tint(BLUE, 0.1)
        } else {
            alpha(0, 0.0)
        };

        let graph = canvas(|_, _, _| {}, move |bounds, _, window, _| {
            paint_graph_row(&row, bounds, is_wip, is_head, is_merge, window)
        })
        .w(px(gw))
        .h_full()
        .flex_none();

        // Ref pills: a local branch and its remote at the same commit share one.
        let mut pills: Vec<AnyElement> = Vec::new();
        if let Some(labels) = snap.refs.get(&oid) {
            let locals: Vec<&str> = labels
                .iter()
                .filter(|l| matches!(l.kind, RefKind::Local | RefKind::Head))
                .map(|l| l.name.as_ref())
                .collect();
            for (n, l) in labels.iter().enumerate() {
                if n >= 4 {
                    pills.push(pill(format!("+{}", labels.len() - n), DARK5, false).into_any_element());
                    break;
                }
                pills.push(ref_pill(l, &locals, lane_color).into_any_element());
            }
        }

        let summary = if is_wip {
            let st = &snap.status;
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(div().italic().text_color(c(FG_DARK)).child(commit.summary.clone()))
                .when(!st.staged.is_empty(), |d| d.child(pill(format!("{} staged", st.staged.len()), GREEN, false)))
                .when(!st.unstaged.is_empty(), |d| d.child(pill(format!("{} unstaged", st.unstaged.len()), YELLOW, false)))
                .when(!st.conflicted.is_empty(), |d| d.child(pill(format!("{} conflicted", st.conflicted.len()), RED, false)))
        } else {
            div()
                .flex()
                .items_center()
                .gap(px(5.))
                .min_w_0()
                .children(pills)
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(if is_merge { c(DARK5) } else { c(FG) })
                        .child(commit.summary.clone()),
                )
        };

        let author = if is_wip {
            div().w(px(170.)).flex_none()
        } else {
            div()
                .w(px(170.))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(7.))
                .overflow_hidden()
                .child(avatar(&commit.author, &commit.email, 18.))
                .child(div().truncate().text_color(c(FG_DARK)).child(commit.author.clone()))
        };

        let date: SharedString = if is_wip { "".into() } else { relative_time(commit.time).into() };

        div()
            .id(("commit", i))
            .flex()
            .items_center()
            .h(px(ROW_H))
            .w_full()
            .bg(bg)
            .when(!selected, |d| d.hover(|s| s.bg(c(BG_HOVER))))
            .when(selected, |d| d.border_l_2().border_color(c(lane_color)))
            .cursor_pointer()
            .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                window.focus(&this.focus);
                if ev.click_count() == 2 {
                    this.double_click_commit(oid, cx);
                } else {
                    this.select_index(i, ev.modifiers().shift, cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    let in_range = this.range.is_some_and(|(a, b)| i >= a && i <= b);
                    if !in_range {
                        this.select_index(i, false, cx);
                    }
                    let items = this.commit_menu(oid);
                    this.menu = Some(Menu { pos: ev.position, items });
                    cx.notify();
                }),
            )
            .child(graph)
            .child(div().flex_1().min_w_0().overflow_hidden().pr(px(12.)).child(summary))
            .child(author)
            .child(div().w(px(96.)).flex_none().text_color(c(COMMENT)).text_size(px(12.)).child(date))
            .child(
                div()
                    .w(px(76.))
                    .flex_none()
                    .font_family(MONO_FONT)
                    .text_size(px(11.5))
                    .text_color(c(DARK3))
                    .child(commit.short.clone()),
            )
            .into_any_element()
    }

    fn double_click_commit(&mut self, oid: Oid, cx: &mut Context<Self>) {
        let Some(snap) = &self.snap else { return };
        if oid == wip_oid() {
            return;
        }
        let local = snap
            .refs
            .get(&oid)
            .and_then(|ls| ls.iter().find(|l| l.kind == RefKind::Local))
            .map(|l| l.name.to_string());
        if let Some(name) = local {
            self.run("Checking out", move |g| g.checkout(&name).map(|_| format!("Switched to {name}")), cx);
        }
    }

    pub fn commit_menu(&self, oid: Oid) -> Vec<MenuItem> {
        let mut items = Vec::new();
        let add = |items: &mut Vec<MenuItem>, label: &str, danger: bool, f: super::app::Action| {
            items.push(MenuItem::Action { label: label.to_string().into(), danger, action: f });
        };
        if oid == wip_oid() {
            add(&mut items, "Stage all changes", false, action(|this, _, cx| this.run("Staging", |g| g.stage_all().map(|_| String::new()), cx)));
            add(&mut items, "Unstage all changes", false, action(|this, _, cx| this.run("Unstaging", |g| g.unstage_all().map(|_| String::new()), cx)));
            add(&mut items, "Stash changes…", false, action(|this, w, cx| this.stash(w, cx)));
            items.push(MenuItem::Separator);
            add(&mut items, "Discard all changes…", true, action(|this, _, cx| {
                this.confirm("Discard all changes?", "Every uncommitted change, staged or not, will be lost. Untracked files are kept.", "Discard all", true,
                    action(|this, _, cx| this.run("Discarding", |g| g.run(&["reset", "--hard", "-q"]).map(|_| "Discarded all changes".into()), cx)));
                cx.notify();
            }));
            return items;
        }
        let sha = oid.to_string();
        let short = sha[..7].to_string();
        let multi = self.selected_commits();
        if multi.len() > 1 {
            add(&mut items, &format!("Squash {} commits…", multi.len()), false, action(|this, _, cx| this.squash_selected(cx)));
            add(&mut items, &format!("Drop {} commits…", multi.len()), true, action(|this, _, cx| this.confirm_drop(this.selected_commits(), cx)));
            items.push(MenuItem::Separator);
        }
        let branch = self.head_branch().unwrap_or_else(|| "HEAD".into());
        {
            let s = sha.clone();
            add(&mut items, "Create branch here…", false, action(move |this, w, cx| this.new_branch_at(s.clone(), w, cx)));
        }
        {
            let s = sha.clone();
            add(&mut items, "Create tag here…", false, action(move |this, w, cx| this.new_tag_at(s.clone(), w, cx)));
        }
        {
            let s = sha.clone();
            add(&mut items, "Checkout this commit (detached)", false, action(move |this, _, cx| {
                let s = s.clone();
                this.run("Checking out", move |g| g.checkout(&s).map(|_| format!("HEAD detached at {}", &s[..7])), cx)
            }));
        }
        items.push(MenuItem::Separator);
        add(&mut items, "Cherry-pick onto current", false, action(move |this, _, cx| {
            this.run("Cherry-picking", move |g| g.cherry_pick(oid).map(|_| "Cherry-picked".into()), cx)
        }));
        add(&mut items, "Revert commit", false, action(move |this, _, cx| {
            this.run("Reverting", move |g| g.revert(oid).map(|_| "Reverted".into()), cx)
        }));
        add(&mut items, "Reword…", false, action(move |this, w, cx| this.reword(oid, w, cx)));
        add(&mut items, &format!("Interactive rebase {branch} from here…"), false, action(move |this, _, cx| this.start_rebase(oid, |_| {}, cx)));
        add(&mut items, "Squash into parent", false, action(move |this, _, cx| {
            let Some(dir) = this.workdir.clone() else { return };
            let parent = git2::Repository::open(&dir)
                .ok()
                .and_then(|r| r.find_commit(oid).ok().and_then(|c| c.parent_id(0).ok()));
            match parent {
                Some(p) => this.start_rebase(p, move |steps| {
                    if let Some(s) = steps.iter_mut().find(|s| s.oid == oid) {
                        s.action = RebaseAction::Squash;
                    }
                }, cx),
                None => this.toast(ToastKind::Info, "The root commit has no parent to squash into", cx),
            }
        }));
        add(&mut items, "Drop commit…", true, action(move |this, _, cx| this.confirm_drop(vec![oid], cx)));
        items.push(MenuItem::Separator);
        for mode in ["soft", "mixed", "hard"] {
            add(&mut items, &format!("Reset {branch} here — {mode}"), mode == "hard", action(move |this, _, cx| this.reset_to(oid, mode, cx)));
        }
        items.push(MenuItem::Separator);
        add(&mut items, &format!("Copy SHA ({short})"), false, action(move |this, _, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(sha.clone()));
            this.toast(ToastKind::Info, "Copied commit SHA", cx);
        }));
        items
    }
}

fn ref_pill(l: &RefLabel, locals: &[&str], lane_color: u32) -> gpui::Div {
    match l.kind {
        RefKind::Head => pill(format!("● {}", l.name), lane_color, true)
            .font_weight(gpui::FontWeight::BOLD),
        RefKind::Local => pill(l.name.clone(), lane_color, false),
        RefKind::Remote => {
            let short = l.name.split_once('/').map_or(l.name.as_ref(), |(_, b)| b);
            let dim = locals.contains(&short);
            pill(l.name.clone(), if dim { DARK5 } else { CYAN }, false)
        }
        RefKind::Tag => pill(format!("⌁ {}", l.name), YELLOW, false),
    }
}

fn lane_x(col: u16) -> f32 {
    GRAPH_PAD + col as f32 * LANE_W + LANE_W / 2.
}

fn paint_graph_row(
    row: &GraphRow,
    bounds: Bounds<Pixels>,
    is_wip: bool,
    is_head: bool,
    is_merge: bool,
    window: &mut Window,
) {
    let o = bounds.origin;
    let h = bounds.size.height / px(1.);
    let max_x = bounds.size.width / px(1.) - 2.;
    let cy = h / 2.;
    let p = |x: f32, y: f32| point(o.x + px(x.min(max_x)), o.y + px(y));
    let stroke = 2.0;

    for line in &row.lines {
        let color = c(lane(line.color));
        let (y0, y1) = match line.half {
            Half::Full => (0., h),
            Half::Top => (0., cy),
            Half::Bottom => (cy, h),
        };
        let (x0, x1) = (lane_x(line.from), lane_x(line.to));
        if line.from == line.to {
            if lane_x(line.from) > max_x {
                continue;
            }
            window.paint_quad(fill(
                Bounds::new(p(x0 - stroke / 2., y0), size(px(stroke), px(y1 - y0))),
                color,
            ));
        } else {
            let mut b = PathBuilder::stroke(px(stroke));
            let mid = (y0 + y1) / 2.;
            b.move_to(p(x0, y0));
            b.cubic_bezier_to(p(x1, y1), p(x0, mid), p(x1, mid));
            if let Ok(path) = b.build() {
                window.paint_path(path, color);
            }
        }
    }

    let x = lane_x(row.col);
    if x > max_x {
        return;
    }
    let color = lane(row.color);
    let dot = |r: f32| Bounds::new(p(x - r, cy - r), size(px(r * 2.), px(r * 2.)));
    if is_wip {
        window.paint_quad(
            gpui::quad(dot(6.), px(6.), c(BG), px(2.), c(color), gpui::BorderStyle::Dashed),
        );
    } else if is_head {
        window.paint_quad(gpui::quad(dot(6.5), px(6.5), c(BG), px(2.5), c(color), gpui::BorderStyle::Solid));
        window.paint_quad(fill(dot(2.5), c(color)).corner_radii(px(2.5)));
    } else if is_merge {
        window.paint_quad(fill(dot(4.), c(color)).corner_radii(px(4.)));
    } else {
        window.paint_quad(fill(dot(5.5), c(color)).corner_radii(px(5.5)));
        window.paint_quad(fill(dot(2.), c(BG)).corner_radii(px(2.)));
    }
}
