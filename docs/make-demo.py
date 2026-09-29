#!/usr/bin/env python3
"""Record docs/demo.mp4 and docs/demo.gif: a tour of the real app.

Builds a small demo project in a temporary directory (four repositories with
branches, merges, a stash, one behind its remote and one stuck mid-merge),
drives gitpanda through it with GITPANDA_SCRIPT, which screenshots the app's
own window, and joins the frames with ffmpeg. Needs macOS and ffmpeg:

    python3 docs/make-demo.py
    python3 docs/make-demo.py --keep       # keep the demo repos and frames
"""

import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
NOW = int(time.time())

PEOPLE = {
    "mei": ("Mei Lin", "mei@bamboo.dev"),
    "tomas": ("Tomás Rivera", "tomas@bamboo.dev"),
    "ada": ("Ada Okafor", "ada@bamboo.dev"),
    "kenji": ("Kenji Sato", "kenji@bamboo.dev"),
}

# Git as a clean room: no global config, hooks or signing from this machine.
GIT_ENV = {**os.environ, "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_CONFIG_NOSYSTEM": "1"}


class Repo:
    def __init__(self, path: Path, bare_remote: Path | None = None):
        self.path = path
        path.mkdir(parents=True, exist_ok=True)
        self.git("init", "-q", "-b", "main")
        self.git("config", "user.name", "Mei Lin")
        self.git("config", "user.email", "mei@bamboo.dev")
        self.git("config", "commit.gpgsign", "false")
        if bare_remote:
            subprocess.run(["git", "init", "-q", "--bare", "-b", "main", str(bare_remote)], check=True, env=GIT_ENV)
            self.git("remote", "add", "origin", str(bare_remote))

    def git(self, *args: str, env: dict | None = None, check: bool = True) -> str:
        r = subprocess.run(["git", *args], cwd=self.path, env=env or GIT_ENV, capture_output=True, text=True)
        if check and r.returncode:
            sys.exit(f"git {' '.join(args)} failed in {self.path}:\n{r.stderr}")
        return r.stdout

    def write(self, rel: str, text: str):
        f = self.path / rel
        f.parent.mkdir(parents=True, exist_ok=True)
        f.write_text(text)

    def commit(self, who: str, hours_ago: float, message: str, *files: str):
        name, email = PEOPLE[who]
        when = f"{NOW - int(hours_ago * 3600)} +0000"
        self.git("add", *(files or ["-A"]))
        env = {**GIT_ENV, "GIT_AUTHOR_NAME": name, "GIT_AUTHOR_EMAIL": email, "GIT_AUTHOR_DATE": when,
               "GIT_COMMITTER_NAME": name, "GIT_COMMITTER_EMAIL": email, "GIT_COMMITTER_DATE": when}
        self.git("commit", "-q", "-m", message, env=env)

    def merge(self, who: str, hours_ago: float, branch: str):
        name, email = PEOPLE[who]
        when = f"{NOW - int(hours_ago * 3600)} +0000"
        env = {**GIT_ENV, "GIT_AUTHOR_NAME": name, "GIT_AUTHOR_EMAIL": email, "GIT_AUTHOR_DATE": when,
               "GIT_COMMITTER_NAME": name, "GIT_COMMITTER_EMAIL": email, "GIT_COMMITTER_DATE": when}
        self.git("merge", "-q", "--no-ff", "-m", f"Merge branch '{branch}'", branch, env=env)


ROUTES = """use axum::{routing::get, Router};

use crate::inventory::Inventory;

pub fn router(inv: Inventory) -> Router {
    Router::new()
        .route("/stalks", get(list_stalks))
        .route("/stalks/:id", get(stalk))
        .with_state(inv)
}

async fn list_stalks(inv: Inventory) -> String {
    let stalks = inv.all().await;
    format!("{} stalks in stock", stalks.len())
}

async fn stalk(inv: Inventory, id: u32) -> String {
    match inv.get(id).await {
        Some(s) => s.describe(),
        None => "no such stalk".into(),
    }
}
"""


def build_api(base: Path) -> Repo:
    r = Repo(base / "bamboo-api", base / "remotes" / "bamboo-api.git")
    r.write("Cargo.toml", '[package]\nname = "bamboo-api"\nversion = "0.1.0"\nedition = "2024"\n\n[dependencies]\naxum = "0.7"\ntokio = { version = "1", features = ["full"] }\n')
    r.write("README.md", "# bamboo-api\n\nServes the bamboo inventory to hungry pandas.\n")
    r.write("src/main.rs", "mod inventory;\nmod routes;\n\n#[tokio::main]\nasync fn main() {\n    let inv = inventory::Inventory::load().await;\n    routes::serve(inv).await;\n}\n")
    r.commit("mei", 120, "Initial commit")
    r.write("src/inventory.rs", "pub struct Stalk {\n    pub id: u32,\n    pub height_cm: u32,\n    pub fresh: bool,\n}\n\n#[derive(Clone)]\npub struct Inventory;\n")
    r.commit("ada", 110, "Add inventory model")
    r.write("src/routes.rs", ROUTES)
    r.commit("mei", 100, "Add stalk routes")
    r.git("checkout", "-q", "-b", "feature/search")
    r.write("src/search.rs", "pub fn fuzzy(query: &str, name: &str) -> bool {\n    let mut chars = name.chars();\n    query.chars().all(|q| chars.any(|c| c.eq_ignore_ascii_case(&q)))\n}\n")
    r.commit("tomas", 96, "Add fuzzy stalk search")
    r.write("src/search.rs", (r.path / "src/search.rs").read_text() + "\npub fn rank(fresh: bool, height: u32) -> u32 {\n    height + if fresh { 1000 } else { 0 }\n}\n")
    r.commit("tomas", 90, "Rank results by freshness")
    r.git("checkout", "-q", "main")
    r.write("src/inventory.rs", (r.path / "src/inventory.rs").read_text() + "\nimpl Inventory {\n    pub fn count(&self, stalks: &[Stalk]) -> usize {\n        stalks.len()\n    }\n}\n")
    r.commit("ada", 88, "Fix stock count off-by-one")
    r.merge("mei", 80, "feature/search")
    r.git("checkout", "-q", "-b", "feature/cache")
    r.write("src/cache.rs", "use std::time::{Duration, Instant};\n\npub struct Cache<T> {\n    value: Option<(Instant, T)>,\n    ttl: Duration,\n}\n")
    r.write("src/main.rs", (r.path / "src/main.rs").read_text().replace("mod inventory;", "mod cache;\nmod inventory;"))
    r.commit("kenji", 60, "Cache inventory reads")
    r.write("src/cache.rs", (r.path / "src/cache.rs").read_text() + "\nimpl<T: Clone> Cache<T> {\n    pub fn get(&self) -> Option<T> {\n        let (at, v) = self.value.as_ref()?;\n        (at.elapsed() < self.ttl).then(|| v.clone())\n    }\n}\n")
    r.commit("kenji", 54, "Expire cache after five minutes")
    r.git("push", "-q", "-u", "origin", "feature/cache")
    r.git("checkout", "-q", "main")
    r.write("Cargo.toml", (r.path / "Cargo.toml").read_text().replace('axum = "0.7"', 'axum = "0.8"'))
    r.commit("tomas", 48, "Bump axum to 0.8")
    r.git("tag", "v0.3.0")
    r.write("src/main.rs", (r.path / "src/main.rs").read_text().replace("    routes::serve", "    tracing_subscriber::fmt::init();\n    routes::serve"))
    r.commit("kenji", 30, "Log slow requests")
    r.git("push", "-q", "-u", "origin", "main", "--tags")
    r.git("checkout", "-q", "-b", "fix/panda-names")
    r.write("src/inventory.rs", (r.path / "src/inventory.rs").read_text().replace("pub fresh: bool,", "pub fresh: bool,\n    pub planted_by: String,"))
    r.commit("ada", 20, "Allow unicode panda names")
    r.git("checkout", "-q", "main")
    r.write("README.md", (r.path / "README.md").read_text() + "\n## Running\n\n    cargo run\n")
    r.commit("mei", 5, "Document how to run the API")
    # A stash, then the work in progress.
    r.write("src/main.rs", (r.path / "src/main.rs").read_text() + "\n// try a smaller connection pool\n")
    r.git("stash", "push", "-q", "-m", "try a smaller pool")
    r.write("src/routes.rs", ROUTES.replace(
        '        .route("/stalks/:id", get(stalk))\n',
        '        .route("/stalks/:id", get(stalk))\n        .route("/stalks/fresh", get(fresh_stalks))\n        .route("/health", get(|| async { "ok" }))\n',
    ).replace(
        '    format!("{} stalks in stock", stalks.len())\n}\n',
        '    format!("{} stalks in stock", stalks.len())\n}\n\nasync fn fresh_stalks(inv: Inventory) -> String {\n    let fresh = inv.all().await.into_iter().filter(|s| s.fresh).count();\n    format!("{fresh} fresh stalks, come and get them")\n}\n',
    ))
    r.write("README.md", (r.path / "README.md").read_text().replace("hungry pandas.", "hungry pandas, fast."))
    r.write("src/metrics.rs", "pub static REQUESTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);\n")
    return r


def build_web(base: Path) -> Repo:
    remote = base / "remotes" / "bamboo-web.git"
    r = Repo(base / "bamboo-web", remote)
    r.write("index.html", "<h1>Bamboo</h1>\n")
    r.commit("ada", 72, "Landing page")
    r.write("style.css", "h1 { color: #9ece6a; }\n")
    r.commit("ada", 70, "Make it green")
    r.git("push", "-q", "-u", "origin", "main")
    # Teammates push three commits we haven't pulled yet.
    other = base / "tmp-web"
    subprocess.run(["git", "clone", "-q", str(remote), str(other)], check=True, env=GIT_ENV)
    o = Repo.__new__(Repo)
    o.path = other
    o.git("config", "commit.gpgsign", "false")
    for who, h, msg in [("tomas", 8, "Add stalk gallery"), ("kenji", 6, "Lazy-load panda photos"), ("tomas", 3, "Fix gallery on phones")]:
        o.write(f"{msg.split()[1].lower()}.js", f"// {msg}\n")
        o.commit(who, h, msg)
    o.git("push", "-q")
    shutil.rmtree(other)
    r.git("fetch", "-q")
    return r


def build_infra(base: Path) -> Repo:
    r = Repo(base / "bamboo-infra")
    r.write("k8s/deployment.yaml", "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: bamboo-api\nspec:\n  replicas: 2\n  template:\n    spec:\n      containers:\n        - name: api\n          image: bamboo-api:0.2\n")
    r.commit("kenji", 50, "Deploy the API")
    r.git("checkout", "-q", "-b", "scale-up")
    r.write("k8s/deployment.yaml", (r.path / "k8s/deployment.yaml").read_text().replace("replicas: 2", "replicas: 6").replace("0.2", "0.3"))
    r.commit("tomas", 10, "Scale up for feeding time")
    r.git("checkout", "-q", "main")
    r.write("k8s/deployment.yaml", (r.path / "k8s/deployment.yaml").read_text().replace("replicas: 2", "replicas: 3").replace("0.2", "0.3.0"))
    r.commit("kenji", 9, "Roll out v0.3.0")
    r.git("merge", "scale-up", check=False)  # conflicts, on purpose
    return r


def build_docs(base: Path) -> Repo:
    r = Repo(base / "panda-docs")
    r.write("guide.md", "# Feeding guide\n\nTwice a day.\n")
    r.commit("mei", 200, "Start the feeding guide")
    r.write("faq.md", "# FAQ\n\n**Do pandas eat anything else?** Rarely.\n")
    r.commit("ada", 150, "Add FAQ")
    r.write("guide.md", "# Feeding guide\n\nThree times a day, more in winter.\n")
    r.write("glossary.md", "# Glossary\n\n- culm: a bamboo stalk\n")
    return r


def build_side(base: Path) -> Repo:
    r = Repo(base / "sock-drawer")
    r.write("README.md", "Matching socks, eventually.\n")
    r.commit("mei", 400, "Initial commit")
    return r


# The tour: (GITPANDA_SCRIPT commands before the shot, seconds on screen).
TOUR = [
    ("wait 1500", 3.0),                                 # project bar + working tree
    ("key down", 1.6),                                  # latest commit
    ("key down; key down; key down; key down; key down", 1.6),  # "Cache inventory reads"
    ("key right", 2.2),                                 # its first file
    ("key down", 1.8),                                  # next file
    ("key escape; call select 0; call unstaged src/routes.rs", 2.2),
    ("call line 1 3; call line 1 4; call line 1 5", 1.6),
    ("call hunk 1 1; wait 900", 2.2),                   # staged just those lines
    ("key escape; call type Add a health check route", 2.2),
    ("call range 1 4", 1.4),
    ("call focus; call squash", 2.6),                               # interactive rebase editor
    ("key escape; call menu 3 720 250", 2.2),           # commit menu
    ("key escape; call project-menu", 2.4),
    ("key escape; call repo 1; wait 900", 2.2),         # bamboo-web, behind
    ("call repo 2; wait 900", 2.2),                     # bamboo-infra, merging
    ("call conflict k8s/deployment.yaml", 2.2),
    ("call pick 0", 2.2),
    ("key escape; call repo 0; wait 900", 3.0),         # back, message kept
]


def main():
    keep = "--keep" in sys.argv
    if not shutil.which("ffmpeg"):
        sys.exit("ffmpeg is needed: brew install ffmpeg")
    base = Path(tempfile.mkdtemp(prefix="gitpanda-demo-"))
    frames = base / "frames"
    frames.mkdir()
    repos = [build_api(base), build_web(base), build_infra(base), build_docs(base)]
    side = build_side(base)
    projects = base / "projects"
    projects.write_text(
        "active = bamboo\n[bamboo]\n"
        + "".join(f"{'* ' if i == 0 else ''}{r.path}\n" for i, r in enumerate(repos))
        + f"[side quests]\n{side.path}\n"
    )

    subprocess.run(["cargo", "build", "--release", "-q"], cwd=ROOT, check=True)
    script = []
    for i, (cmds, _) in enumerate(TOUR):
        script += [cmds, f"shot {frames / f'{i:02}.png'}"]
    script.append("quit")
    env = {**GIT_ENV, "GITPANDA_PROJECTS": str(projects), "GITPANDA_SCRIPT": "; ".join(script)}
    subprocess.run([str(ROOT / "target/release/gitpanda"), str(repos[0].path)], env=env, check=True)

    missing = [i for i in range(len(TOUR)) if not (frames / f"{i:02}.png").exists()]
    if missing:
        sys.exit(f"frames {missing} weren't captured; see {frames}")
    concat = frames / "list.txt"
    lines = [f"file '{i:02}.png'\nduration {secs}\n" for i, (_, secs) in enumerate(TOUR)]
    concat.write_text("".join(lines) + f"file '{len(TOUR) - 1:02}.png'\n")
    docs = ROOT / "docs"
    ff = ["ffmpeg", "-y", "-loglevel", "error", "-f", "concat", "-safe", "0", "-i", str(concat)]
    subprocess.run(ff + ["-vf", "scale=1920:-2:flags=lanczos,format=yuv420p", "-r", "30",
                         "-c:v", "libx264", "-crf", "18", "-tune", "stillimage", "-movflags", "+faststart",
                         str(docs / "demo.mp4")], check=True)
    subprocess.run(ff + ["-vf", "scale=1200:-1:flags=lanczos,split[a][b];[a]palettegen=stats_mode=full:max_colors=200[p];[b][p]paletteuse=dither=none",
                         str(docs / "demo.gif")], check=True)
    for f in ("demo.mp4", "demo.gif"):
        print(f"docs/{f}: {(docs / f).stat().st_size / 1e6:.1f} MB")
    if keep:
        print(f"kept {base}")
    else:
        shutil.rmtree(base)


if __name__ == "__main__":
    main()
