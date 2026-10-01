// Release builds on Windows are GUI apps: no console window behind them.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod debug;
mod git;
mod ui;
mod workspace;

use std::path::PathBuf;

use gpui::{
    App, AppContext, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, point, px,
    size,
};

use ui::app::GitPanda;

fn main() {
    let mut args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    if args.first().is_some_and(|a| a == "--bench") {
        args.remove(0);
        return bench(args.first().cloned().unwrap_or_else(|| ".".into()));
    }
    let path = args.into_iter().next();
    Application::new().run(move |cx: &mut App| {
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let bounds = Bounds::centered(None, size(px(1480.), px(900.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("gitpanda".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(16.), px(13.))),
                }),
                window_min_size: Some(size(px(960.), px(560.))),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| GitPanda::new(path, window, cx));
                window.focus(&view.read(cx).focus);
                debug::start(window, view.clone(), cx);
                view
            },
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}

/// `gitpanda --bench [path]`: time loading a repository, without any UI.
fn bench(path: PathBuf) {
    let dir = match git::repo::discover(&path) {
        Ok(d) => d,
        Err(e) => return eprintln!("gitpanda: {e}"),
    };
    let mut times = Vec::new();
    let mut last = None;
    let mut prev = None;
    // First load walks history; the rest show the cached (typical) reload.
    for _ in 0..10 {
        let snap = git::repo::load(&dir, prev.take()).expect("load failed");
        if times.is_empty() {
            println!("cold load: {:.1} ms", snap.load_time.as_secs_f64() * 1000.);
        }
        prev = Some(snap.history.clone());
        times.push(snap.load_time.as_secs_f64() * 1000.);
        last = Some(snap);
    }
    let snap = last.unwrap();
    times.sort_by(|a, b| a.total_cmp(b));
    println!(
        "{}: {} commits, {} lanes, {} refs — warm reload median {:.1} ms (min {:.1}, max {:.1})",
        dir.display(),
        snap.commits.len(),
        snap.graph_width,
        snap.branches.len() + snap.remotes.len() + snap.tags.len(),
        times[times.len() / 2],
        times[0],
        times[times.len() - 1],
    );
}
