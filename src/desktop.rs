//! Desktop integration for the Linux AppImage.
//!
//! Wayland docks and taskbars (GNOME, KDE) find a window's name and icon by
//! looking up a `<app_id>.desktop` file. The .deb, .rpm and Arch packages
//! install one, but an AppImage isn't installed anywhere, so without this the
//! window shows up as its title with a generic icon. On each start from an
//! AppImage, write a user-level desktop entry and icon that point at it.

use std::path::{Path, PathBuf};

const DESKTOP: &str = include_str!("../packaging/linux/gitpanda.desktop");
const PNG: &[u8] = include_bytes!("../packaging/gitpanda.png");
const SVG: &[u8] = include_bytes!("../packaging/gitpanda.svg");

pub fn integrate_appimage() {
    let Some(appimage) = std::env::var_os("APPIMAGE").map(PathBuf::from) else { return };
    let Some(data) = data_home() else { return };
    if let Err(e) = install(&data, &appimage) {
        eprintln!("gitpanda: couldn't add the desktop entry: {e}");
    }
}

fn data_home() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
}

fn install(data: &Path, appimage: &Path) -> std::io::Result<()> {
    write_if_changed(&data.join("icons/hicolor/512x512/apps/gitpanda.png"), PNG)?;
    write_if_changed(&data.join("icons/hicolor/scalable/apps/gitpanda.svg"), SVG)?;
    write_if_changed(&data.join("applications/gitpanda.desktop"), desktop_entry(appimage).as_bytes())
}

/// The packaged entry, launching this AppImage instead of `gitpanda` on PATH.
fn desktop_entry(appimage: &Path) -> String {
    // Desktop Entry spec: quote the path, escaping `"`, `` ` ``, `$` and `\`
    // (and `%`, a field code, by doubling it).
    let mut exec = String::from("\"");
    for ch in appimage.to_string_lossy().chars() {
        match ch {
            '"' | '`' | '$' | '\\' => exec.extend(['\\', ch]),
            '%' => exec.push_str("%%"),
            _ => exec.push(ch),
        }
    }
    exec.push('"');
    // The entry's own value level escapes backslashes once more.
    let exec = exec.replace('\\', "\\\\");
    DESKTOP
        .lines()
        .map(|l| match l.strip_prefix("Exec=gitpanda") {
            Some(rest) => format!("Exec={exec}{rest}\nTryExec={}", appimage.display()),
            None => l.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn write_if_changed(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    if std::fs::read(path).is_ok_and(|c| c == contents) {
        return Ok(());
    }
    std::fs::create_dir_all(path.parent().unwrap())?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents)?;
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_launches_the_appimage() {
        let e = desktop_entry(Path::new("/home/me/Apps/git panda$1.AppImage"));
        assert!(e.contains("\nExec=\"/home/me/Apps/git panda\\\\$1.AppImage\" %F\n"), "{e}");
        assert!(e.contains("\nTryExec=/home/me/Apps/git panda$1.AppImage\n"), "{e}");
        assert!(e.contains("\nStartupWMClass=gitpanda\n"), "{e}");
        assert!(e.contains("\nIcon=gitpanda\n"), "{e}");
    }

    #[test]
    fn install_writes_entry_and_icons() {
        let dir = std::env::temp_dir().join(format!("gitpanda-desktop-{}", std::process::id()));
        install(&dir, Path::new("/opt/gitpanda.AppImage")).unwrap();
        let entry = std::fs::read_to_string(dir.join("applications/gitpanda.desktop")).unwrap();
        assert!(entry.contains("Exec=\"/opt/gitpanda.AppImage\" %F"));
        assert_eq!(std::fs::read(dir.join("icons/hicolor/512x512/apps/gitpanda.png")).unwrap(), PNG);
        install(&dir, Path::new("/opt/gitpanda.AppImage")).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
