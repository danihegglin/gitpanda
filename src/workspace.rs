//! Projects: named groups of repositories, shown as tabs above the toolbar.
//! One repository is open at a time; the rest of its project is summarised.
//!
//! Saved as a small text file (`GITPANDA_PROJECTS`, else
//! `$XDG_CONFIG_HOME/gitpanda/projects` or `~/.config/gitpanda/projects`):
//!
//! ```text
//! active = foss
//! [foss]
//! * /Users/me/git/foss/yoru
//! /Users/me/git/foss/tsv
//! ```
//!
//! `active` names the open project; `*` marks a project's last open repository.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Project {
    pub name: String,
    pub repos: Vec<PathBuf>,
    pub last: Option<PathBuf>,
}

impl Project {
    /// The repository to open when switching to this project.
    pub fn start(&self) -> Option<&PathBuf> {
        self.last.as_ref().filter(|l| self.repos.contains(l)).or(self.repos.first())
    }
}

#[derive(Debug, Default)]
pub struct Workspace {
    pub projects: Vec<Project>,
    pub active: usize,
    file: Option<PathBuf>,
}

fn config_file() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("GITPANDA_PROJECTS") {
        return Some(p.into());
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("gitpanda").join("projects"))
}

impl Workspace {
    pub fn load() -> Self {
        let file = config_file();
        let text = file.as_ref().and_then(|f| std::fs::read_to_string(f).ok()).unwrap_or_default();
        Workspace { file, ..Self::parse(&text) }
    }

    pub fn parse(text: &str) -> Self {
        let mut ws = Workspace::default();
        let mut active = None;
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                ws.projects.push(Project { name: name.trim().to_string(), ..Default::default() });
            } else if let Some(name) = line.strip_prefix("active").and_then(|l| l.trim_start().strip_prefix('=')) {
                active = Some(name.trim().to_string());
            } else if let Some(p) = ws.projects.last_mut() {
                let (last, path) = match line.strip_prefix('*') {
                    Some(rest) => (true, PathBuf::from(rest.trim())),
                    None => (false, PathBuf::from(line)),
                };
                if !p.repos.contains(&path) {
                    p.repos.push(path.clone());
                }
                if last {
                    p.last = Some(path);
                }
            }
        }
        ws.active = active.and_then(|a| ws.projects.iter().position(|p| p.name == a)).unwrap_or(0);
        ws
    }

    pub fn serialize(&self) -> String {
        let mut out = String::new();
        if let Some(p) = self.project() {
            out.push_str(&format!("active = {}\n", p.name));
        }
        for p in &self.projects {
            out.push_str(&format!("[{}]\n", p.name));
            for r in &p.repos {
                let mark = if p.last.as_ref() == Some(r) { "* " } else { "" };
                out.push_str(&format!("{mark}{}\n", r.display()));
            }
        }
        out
    }

    pub fn save(&self) -> Result<()> {
        let Some(file) = &self.file else { return Ok(()) };
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(file, self.serialize()).with_context(|| format!("Couldn't save projects to {}", file.display()))
    }

    pub fn project(&self) -> Option<&Project> {
        self.projects.get(self.active)
    }

    pub fn project_mut(&mut self) -> Option<&mut Project> {
        self.projects.get_mut(self.active)
    }

    /// Records that `repo` was opened: switches to the project holding it,
    /// or adds it to the active project (creating one, named after the
    /// repository's parent folder, if there are none).
    pub fn opened(&mut self, repo: &Path) {
        let holds = |p: &Project| p.repos.iter().any(|r| r == repo);
        if !self.project().is_some_and(holds) {
            match self.projects.iter().position(holds) {
                Some(i) => self.active = i,
                None => {
                    if self.projects.is_empty() {
                        let name = repo
                            .parent()
                            .and_then(|p| p.file_name())
                            .map_or("My project".into(), |n| n.to_string_lossy().into_owned());
                        self.projects.push(Project { name, ..Default::default() });
                        self.active = 0;
                    }
                    self.project_mut().unwrap().repos.push(repo.to_path_buf());
                }
            }
        }
        self.project_mut().unwrap().last = Some(repo.to_path_buf());
    }

    /// Adds repositories to the active project; returns how many were new.
    pub fn add(&mut self, repos: &[PathBuf]) -> usize {
        let Some(p) = self.project_mut() else { return 0 };
        let before = p.repos.len();
        for r in repos {
            if !p.repos.contains(r) {
                p.repos.push(r.clone());
            }
        }
        p.repos.len() - before
    }

    pub fn remove(&mut self, repo: &Path) {
        if let Some(p) = self.project_mut() {
            p.repos.retain(|r| r != repo);
            if p.last.as_deref() == Some(repo) {
                p.last = None;
            }
        }
    }

    /// Moves `repo` one place left (`-1`) or right (`1`) in the active project.
    pub fn shift(&mut self, repo: &Path, delta: isize) {
        let Some(p) = self.project_mut() else { return };
        let Some(i) = p.repos.iter().position(|r| r == repo) else { return };
        let j = i as isize + delta;
        if (0..p.repos.len() as isize).contains(&j) {
            p.repos.swap(i, j as usize);
        }
    }

    /// A project name not yet taken, based on `name`.
    pub fn unique_name(&self, name: &str, except: Option<usize>) -> String {
        let taken = |n: &str| self.projects.iter().enumerate().any(|(i, p)| Some(i) != except && p.name == n);
        let name = name.replace(['[', ']'], "").trim().to_string();
        let name = if name.is_empty() { "Project".to_string() } else { name };
        if !taken(&name) {
            return name;
        }
        (2..).map(|k| format!("{name} {k}")).find(|n| !taken(n)).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let text = "active = b\n[a]\n/x/one\n[b]\n/y/two\n* /y/three\n[empty]\n";
        let ws = Workspace::parse(text);
        assert_eq!(ws.projects.len(), 3);
        assert_eq!(ws.active, 1);
        assert_eq!(ws.project().unwrap().start(), Some(&PathBuf::from("/y/three")));
        assert!(ws.projects[2].repos.is_empty());
        assert_eq!(ws.serialize(), text);
    }

    #[test]
    fn opening_adds_or_switches() {
        let mut ws = Workspace::default();
        ws.opened(Path::new("/git/foss/yoru"));
        assert_eq!(ws.projects[0].name, "foss");
        ws.projects.push(Project { name: "work".into(), repos: vec!["/w/api".into()], last: None });
        ws.opened(Path::new("/w/api"));
        assert_eq!(ws.active, 1);
        ws.opened(Path::new("/w/web"));
        assert_eq!(ws.projects[1].repos.len(), 2);
        assert_eq!(ws.project().unwrap().start(), Some(&PathBuf::from("/w/web")));
        assert_eq!(ws.unique_name("work", None), "work 2");
        assert_eq!(ws.unique_name("work", Some(1)), "work");
    }
}
