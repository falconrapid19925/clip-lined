//! Login startup registration for desktop Unix systems.
//!
//! Linux uses the XDG autostart directory; macOS uses a per-user LaunchAgent.
//! Registration takes effect at the next login and does not launch a second
//! instance immediately.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[cfg(all(unix, not(target_os = "macos")))]
fn registration_path() -> Result<PathBuf> {
    let dir = match std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        Some(p) if Path::new(&p).is_absolute() => PathBuf::from(p),
        _ => home()?.join(".config"),
    };
    Ok(dir.join("autostart").join("clip-line.desktop"))
}

#[cfg(target_os = "macos")]
fn registration_path() -> Result<PathBuf> {
    Ok(home()?.join("Library/LaunchAgents/org.clipline.clip-line.plist"))
}

#[cfg(not(unix))]
fn registration_path() -> Result<PathBuf> {
    anyhow::bail!("autostart is unavailable on this platform")
}

#[cfg(unix)]
fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .context("HOME is not set; cannot configure login startup")
}

#[cfg(all(unix, not(target_os = "macos")))]
fn contents(exe: &Path) -> Result<String> {
    let exe = exe.to_str().context("executable path is not UTF-8")?;
    // Desktop Exec field codes use %, and these four characters must be
    // escaped inside a quoted argument according to the desktop-entry spec.
    let mut quoted = String::new();
    for c in exe.chars() {
        match c {
            '%' => quoted.push_str("%%"),
            '\\' => quoted.push_str("\\\\\\\\"),
            '"' | '$' | '`' => {
                quoted.push_str("\\\\");
                quoted.push(c);
            }
            '\n' | '\r' => anyhow::bail!("executable path contains a newline"),
            _ => quoted.push(c),
        }
    }
    Ok(format!(
        "[Desktop Entry]\nType=Application\nName=clipLine\nExec=\"{quoted}\"\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
    ))
}

#[cfg(target_os = "macos")]
fn contents(exe: &Path) -> Result<String> {
    let exe = exe.to_str().context("executable path is not UTF-8")?;
    let exe = exe
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;");
    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key><string>org.clipline.clip-line</string><key>ProgramArguments</key><array><string>{exe}</string></array><key>RunAtLoad</key><true/></dict></plist>\n"
    ))
}

#[cfg(not(unix))]
fn contents(_: &Path) -> Result<String> {
    anyhow::bail!("autostart is unavailable on this platform")
}

pub fn set_autostart(enabled: bool) -> Result<()> {
    let path = registration_path()?;
    if !enabled {
        match std::fs::remove_file(&path) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e).with_context(|| format!("remove {}", path.display())),
        }
    }
    let exe = std::env::current_exe().context("find clipLine executable")?;
    let text = contents(&exe)?;
    let parent = path.parent().context("invalid autostart path")?;
    std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    std::fs::write(&path, text).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

#[cfg(all(test, unix, not(target_os = "macos")))]
mod tests {
    use super::*;
    #[test]
    fn desktop_entry_quotes_the_executable_and_field_codes() {
        let text = contents(Path::new("/tmp/my app/$test%file")).unwrap();
        assert!(text.contains("Exec=\"/tmp/my app/\\\\$test%%file\"\n"));
        assert!(!text.contains("Exec=/tmp/my"));
    }
    #[test]
    fn desktop_entry_rejects_newlines() {
        assert!(contents(Path::new("/tmp/app\nOther=true")).is_err());
    }
}
