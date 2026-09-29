//! Project bar: the active project's repositories as tabs above the
//! toolbar, each with its branch and whether it needs attention, plus the
//! project menu (switch, create, rename, delete).

use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, ClipboardItem, Context, MouseButton, MouseDownEvent, PathPromptOptions,
    SharedString, Window, div, point, prelude::*, px,
};

use crate::git::repo::{self, OpState, Summary};
use crate::workspace::Project;

use super::app::{GitPanda, Menu, MenuItem, ToastKind, action};
use super::chrome::drag_space;
use super::theme::*;
use super::widgets::*;

pub const HEIGHT: f32 = 40.;

fn repo_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Short labels for the tabs; the banner spells them out.
fn state_label(s: OpState) -> &'static str {
    match s {
        OpState::Clean => "",
        OpState::Merge => "merging",
        OpState::Rebase => "rebasing",
        OpState::CherryPick => "picking",
        OpState::Revert => "reverting",
        OpState::Bisect => "bisecting",
        OpState::Other => "busy",
    }
}

impl GitPanda {
    pub fn save_workspace(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = self.workspace.save() {
            self.toast(ToastKind::Error, e.to_string(), cx);
        }
    }

    /// Reads a summary of every repository in the active project except the
    /// open one, which is kept current from its snapshot.
    pub fn refresh_summaries(&mut self, cx: &mut Context<Self>) {
        let Some(p) = self.workspace.project() else { return };
        let paths: Vec<PathBuf> = p.repos.iter().filter(|r| Some(*r) != self.workdir.as_ref()).cloned().collect();
        if paths.is_empty() {
            return;
        }
        self.summary_gen += 1;
        let generation = self.summary_gen;
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    paths
                        .into_iter()
                        .map(|p| {
                            let s = match p.exists() {
                                true => repo::summary(&p).map_err(|_| "not a repository".into()),
                                false => Err("missing".into()),
                            };
                            (p, s)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if generation != this.summary_gen {
                    return;
                }
                for (p, s) in res {
                    if Some(&p) != this.workdir.as_ref() {
                        this.summaries.insert(p, s);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn switch_project(&mut self, i: usize, cx: &mut Context<Self>) {
        if i >= self.workspace.projects.len() {
            return;
        }
        self.workspace.active = i;
        self.save_workspace(cx);
        match self.workspace.project().and_then(Project::start).cloned() {
            Some(p) => self.open_repo(&p, cx),
            None => {
                self.close_repo(cx);
                self.open_error = None;
            }
        }
        self.refresh_summaries(cx);
    }

    pub fn prompt_add_repos(&mut self, cx: &mut Context<Self>) {
        let name = self.workspace.project().map_or("project".into(), |p| p.name.clone());
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: Some(format!("Add to {name}").into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                let _ = this.update(cx, |this, cx| this.add_repos(&paths, cx));
            }
        })
        .detach();
    }

    fn add_repos(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        let mut dirs = Vec::new();
        for p in paths {
            match repo::discover(p) {
                Ok(d) => dirs.push(d),
                Err(e) => self.toast(ToastKind::Error, e.to_string(), cx),
            }
        }
        if dirs.is_empty() {
            return;
        }
        if self.workspace.projects.is_empty() {
            // Opening creates the first project; the rest join it.
            self.open_repo(&dirs[0], cx);
        }
        let n = self.workspace.add(&dirs);
        self.save_workspace(cx);
        if n > 0 {
            let name = self.workspace.project().map(|p| p.name.clone()).unwrap_or_default();
            self.toast(ToastKind::Success, format!("Added {} to {name}", plural(n, "repository", "repositories")), cx);
        }
        if self.workdir.is_none() {
            self.open_repo(&dirs[0], cx);
        }
        self.refresh_summaries(cx);
    }

    pub fn remove_repo(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(p) = self.workspace.project() else { return };
        let i = p.repos.iter().position(|r| *r == path).unwrap_or(0);
        self.workspace.remove(&path);
        self.save_workspace(cx);
        if self.workdir.as_ref() == Some(&path) {
            let repos = &self.workspace.project().unwrap().repos;
            match repos.get(i).or(repos.last()).cloned() {
                Some(next) => self.open_repo(&next, cx),
                None => self.close_repo(cx),
            }
        }
        cx.notify();
    }

    pub fn new_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt("New project", Some("Group repositories you work on together.".into()), "", "Create", window, cx, |this, name, _, cx| {
            let name = this.workspace.unique_name(&name, None);
            this.workspace.projects.push(Project { name, ..Default::default() });
            this.switch_project(this.workspace.projects.len() - 1, cx);
            this.prompt_add_repos(cx);
        });
    }

    pub fn rename_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.workspace.project() else { return };
        let old = p.name.clone();
        self.prompt("Rename project", None, &old, "Rename", window, cx, |this, name, _, cx| {
            let active = this.workspace.active;
            let name = this.workspace.unique_name(&name, Some(active));
            if let Some(p) = this.workspace.project_mut() {
                p.name = name;
            }
            this.save_workspace(cx);
        });
    }

    pub fn delete_project(&mut self, cx: &mut Context<Self>) {
        let Some(p) = self.workspace.project() else { return };
        let body = format!(
            "{} leaves the project bar. The repositories themselves aren't touched.",
            plural(p.repos.len(), "repository", "repositories")
        );
        self.confirm(format!("Delete {}?", p.name), body, "Delete project", true, action(|this, _, cx| {
            let i = this.workspace.active;
            this.workspace.projects.remove(i);
            match this.workspace.projects.len() {
                0 => {
                    this.workspace.active = 0;
                    this.save_workspace(cx);
                    this.close_repo(cx);
                }
                n => this.switch_project(i.min(n - 1), cx),
            }
        }));
        cx.notify();
    }

    pub fn project_menu(&self) -> Vec<MenuItem> {
        let item = |label: String, danger: bool, f| MenuItem::Action { label: label.into(), danger, action: f };
        let mut items: Vec<MenuItem> = self
            .workspace
            .projects
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let mark = if i == self.workspace.active { "✓" } else { "    " };
                let label = format!("{mark}  {}  ·  {}", p.name, plural(p.repos.len(), "repo", "repos"));
                item(label, false, action(move |this, _, cx| this.switch_project(i, cx)))
            })
            .collect();
        if !items.is_empty() {
            items.push(MenuItem::Separator);
        }
        items.push(item("New project…".into(), false, action(|this, window, cx| this.new_project(window, cx))));
        if self.workspace.project().is_some() {
            items.push(item("Add repositories…".into(), false, action(|this, _, cx| this.prompt_add_repos(cx))));
            items.push(item("Rename project…".into(), false, action(|this, window, cx| this.rename_project(window, cx))));
            items.push(MenuItem::Separator);
            items.push(item("Delete project…".into(), true, action(|this, _, cx| this.delete_project(cx))));
        }
        items
    }

    fn repo_menu(&self, path: &Path, index: usize) -> Vec<MenuItem> {
        let item = |label: &str, danger: bool, f| MenuItem::Action { label: label.to_string().into(), danger, action: f };
        let n = self.workspace.project().map_or(0, |p| p.repos.len());
        let mut items = Vec::new();
        if self.workdir.as_deref() != Some(path) {
            let p = path.to_path_buf();
            items.push(item("Open", false, action(move |this, _, cx| this.open_repo(&p, cx))));
        }
        for (label, delta, ok) in [("Move left", -1, index > 0), ("Move right", 1, index + 1 < n)] {
            if ok {
                let p = path.to_path_buf();
                items.push(item(label, false, action(move |this, _, cx| {
                    this.workspace.shift(&p, delta);
                    this.save_workspace(cx);
                })));
            }
        }
        let p = path.to_path_buf();
        items.push(item("Copy path", false, action(move |this, _, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(p.display().to_string()));
            this.toast(ToastKind::Info, "Copied path", cx);
        })));
        let p = path.to_path_buf();
        let reveal = if cfg!(target_os = "macos") { "Show in Finder" } else { "Open folder" };
        items.push(item(reveal, false, action(move |this, _, cx| {
            let res = if cfg!(target_os = "macos") {
                std::process::Command::new("open").arg("-R").arg(&p).spawn()
            } else {
                std::process::Command::new("xdg-open").arg(&p).spawn()
            };
            if let Err(e) = res {
                this.toast(ToastKind::Error, e.to_string(), cx);
            }
        })));
        items.push(MenuItem::Separator);
        let p = path.to_path_buf();
        items.push(item("Remove from project", true, action(move |this, _, cx| this.remove_repo(p.clone(), cx))));
        items
    }

    // ---- rendering -------------------------------------------------------------

    pub fn render_project_bar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let project = self.workspace.project();
        let name: SharedString = project.map_or("No project".to_string(), |p| p.name.clone()).into();
        let repos = project.map(|p| p.repos.clone()).unwrap_or_default();
        let color = person(&name);
        let initial = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();

        let known: Vec<&Summary> = repos.iter().filter_map(|r| self.summaries.get(r)?.as_ref().ok()).collect();
        let mut overview = vec![plural(repos.len(), "repo", "repos")];
        let count = |f: &dyn Fn(&Summary) -> bool| known.iter().filter(|s| f(s)).count();
        for (n, what) in [
            (count(&|s| s.state != OpState::Clean), "mid-operation"),
            (count(&|s| s.changes > 0), "with changes"),
            (count(&|s| s.ahead > 0), "to push"),
            (count(&|s| s.behind > 0), "to pull"),
        ] {
            if n > 0 {
                overview.push(format!("{n} {what}"));
            }
        }

        let tabs: Vec<AnyElement> = repos.iter().enumerate().map(|(i, r)| self.render_repo_tab(i, r, cx)).collect();

        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(px(HEIGHT))
            .pl(px(78.)) // clear the macOS traffic lights
            .pr(px(14.))
            .bg(c(BG_DARKER))
            .border_b_1()
            .border_color(c(BORDER))
            .child(
                div()
                    .id("project")
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(7.))
                    .h(px(28.))
                    .px(px(8.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|s| s.bg(c(BG_HIGHLIGHT)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(18.))
                            .rounded(px(5.))
                            .bg(alpha(color, 0.22))
                            .text_color(c(color))
                            .text_size(px(10.5))
                            .font_weight(gpui::FontWeight::BOLD)
                            .child(initial),
                    )
                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(FG)).child(name))
                    .child(div().text_size(px(10.)).text_color(c(COMMENT)).child("▾"))
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        let items = this.project_menu();
                        this.menu = Some(Menu { pos: point(px(76.), px(HEIGHT - 2.)), items });
                        cx.notify();
                    })),
            )
            .child(div().flex_none().w(px(1.)).h(px(20.)).bg(c(BORDER_HI)).mx(px(2.)))
            .child(
                div()
                    .id("repo-tabs")
                    .flex()
                    .flex_shrink()
                    .min_w_0()
                    .items_center()
                    .gap(px(3.))
                    .overflow_x_scroll()
                    .children(tabs)
                    .when(project.is_some(), |d| {
                        d.child(
                            div()
                                .id("add-repo")
                                .flex()
                                .flex_none()
                                .items_center()
                                .justify_center()
                                .size(px(26.))
                                .rounded(px(6.))
                                .text_size(px(15.))
                                .text_color(c(DARK5))
                                .cursor_pointer()
                                .hover(|s| s.bg(c(BG_HIGHLIGHT)).text_color(c(CYAN)))
                                .child("+")
                                .on_click(cx.listener(|this, _, _, cx| this.prompt_add_repos(cx))),
                        )
                    }),
            )
            .child(drag_space())
            .when(project.is_some(), |d| {
                d.child(div().flex_none().text_size(px(11.)).text_color(c(DARK3)).child(overview.join(" · ")))
            })
            .into_any_element()
    }

    fn render_repo_tab(&self, i: usize, path: &Path, cx: &mut Context<Self>) -> AnyElement {
        let active = self.workdir.as_deref() == Some(path);
        let summary = self.summaries.get(path);
        let ok = summary.and_then(|s| s.as_ref().ok());
        let branch = ok.map(|s| match (&s.head.branch, s.head.oid) {
            (Some(b), _) => b.to_string(),
            (None, Some(o)) => format!("@{}", &o.to_string()[..7]),
            (None, None) => "empty".into(),
        });
        let track = ok.map(|s| match (s.ahead, s.behind) {
            (0, 0) => String::new(),
            (a, 0) => format!("↑{a}"),
            (0, b) => format!("↓{b}"),
            (a, b) => format!("↑{a} ↓{b}"),
        });
        let changes = ok.map_or(0, |s| s.changes);
        let state = ok.map_or(OpState::Clean, |s| s.state);
        let error = summary.and_then(|s| s.as_ref().err()).cloned();
        let p = path.to_path_buf();
        let p2 = path.to_path_buf();

        div()
            .id(("repo-tab", i))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(7.))
            .h(px(28.))
            .px(px(10.))
            .rounded(px(6.))
            .border_1()
            .cursor_pointer()
            .when(active, |d| d.bg(c(BG)).border_color(c(BORDER_HI)))
            .when(!active, |d| d.border_color(alpha(0, 0.)).hover(|s| s.bg(c(BG_HOVER))))
            .child(
                div()
                    .text_color(if active { c(FG) } else { c(FG_DARK) })
                    .when(active, |d| d.font_weight(gpui::FontWeight::SEMIBOLD))
                    .child(repo_name(path)),
            )
            .children(branch.map(|b| {
                div().max_w(px(140.)).truncate().text_size(px(11.)).text_color(c(if active { GREEN } else { DARK5 })).child(b)
            }))
            .when(state != OpState::Clean, |d| d.child(pill(state_label(state), ORANGE, false)))
            .when(changes > 0, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .text_size(px(11.))
                        .text_color(c(YELLOW))
                        .child(div().size(px(6.)).rounded_full().bg(c(YELLOW)))
                        .child(changes.to_string()),
                )
            })
            .children(track.filter(|t| !t.is_empty()).map(|t| div().text_size(px(11.)).text_color(c(CYAN)).child(t)))
            .children(error.map(|e| div().text_size(px(11.)).text_color(c(RED)).child(e)))
            .on_click(cx.listener(move |this, _, _, cx| this.open_repo(&p, cx)))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    let items = this.repo_menu(&p2, i);
                    this.menu = Some(Menu { pos: ev.position, items });
                    cx.notify();
                }),
            )
            .into_any_element()
    }
}
