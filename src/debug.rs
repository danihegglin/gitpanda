//! Development hook. `GITPANDA_SCRIPT` holds `;`-separated commands that drive
//! the live UI, so its states can be screenshotted without screen-recording
//! permission (a process may always capture its own windows):
//!
//!   wait <ms> · key <keystroke> · call <name> [args…] · shot <file.png> · quit

use gpui::{App, Entity, Keystroke, Window};

use crate::ui::app::GitPanda;

pub fn start(window: &mut Window, view: Entity<GitPanda>, cx: &mut App) {
    let Ok(script) = std::env::var("GITPANDA_SCRIPT") else { return };
    let cmds: Vec<String> = script.split(';').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    window
        .spawn(cx, async move |cx| {
            for cmd in cmds {
                let (op, arg) = cmd.split_once(' ').unwrap_or((&cmd, ""));
                let arg = arg.trim().to_string();
                match op {
                    "wait" => {
                        let ms = arg.parse().unwrap_or(500);
                        cx.background_executor().timer(std::time::Duration::from_millis(ms)).await;
                    }
                    "key" => {
                        let _ = cx.update(|window, cx| {
                            if let Ok(k) = Keystroke::parse(&arg) {
                                window.dispatch_keystroke(k, cx);
                            }
                        });
                    }
                    "call" => {
                        let view = view.clone();
                        let _ = cx.update(|window, cx| {
                            view.update(cx, |this, cx| this.debug_call(&arg, window, cx));
                        });
                    }
                    "shot" => {
                        // Let the last change render first.
                        cx.background_executor().timer(std::time::Duration::from_millis(250)).await;
                        let _ = cx.update(|window, _| {
                            if let Err(e) = capture(window, &arg) {
                                eprintln!("gitpanda: screenshot failed: {e}");
                            }
                        });
                    }
                    "quit" => {
                        let _ = cx.update(|_, cx| cx.quit());
                    }
                    _ => eprintln!("gitpanda: unknown script command {op:?}"),
                }
            }
        })
        .detach();
}

#[cfg(target_os = "macos")]
fn capture(window: &Window, path: &str) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
    use core_graphics::window::{
        create_image, kCGWindowImageBestResolution, kCGWindowImageBoundsIgnoreFraming,
        kCGWindowListOptionIncludingWindow,
    };
    use objc::runtime::Object;
    use objc::{msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let RawWindowHandle::AppKit(h) = HasWindowHandle::window_handle(window).map_err(|_| anyhow::anyhow!("no window handle"))?.as_raw() else {
        anyhow::bail!("not an AppKit window");
    };
    let view = h.ns_view.as_ptr() as *mut Object;
    let number: isize = unsafe {
        let w: *mut Object = msg_send![view, window];
        msg_send![w, windowNumber]
    };
    let null = CGRect::new(&CGPoint::new(f64::INFINITY, f64::INFINITY), &CGSize::new(0., 0.));
    let img = create_image(
        null,
        kCGWindowListOptionIncludingWindow,
        number as u32,
        kCGWindowImageBoundsIgnoreFraming | kCGWindowImageBestResolution,
    )
    .context("CGWindowListCreateImage returned nothing")?;
    let (w, h, bpr) = (img.width(), img.height(), img.bytes_per_row());
    let data = img.data();
    let bytes = data.bytes();
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for px in bytes[y * bpr..y * bpr + w * 4].chunks_exact(4) {
            rgba.extend([px[2], px[1], px[0], 255]);
        }
    }
    image::save_buffer(path, &rgba, w as u32, h as u32, image::ExtendedColorType::Rgba8)?;
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn capture(_: &Window, _: &str) -> anyhow::Result<()> {
    anyhow::bail!("screenshots are only implemented on macOS")
}
