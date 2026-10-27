//! Per-user login startup configuration for desktop systems.

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
fn desktop_exec(path: &Path) -> anyhow::Result<String> {
    // Desktop Entry Exec uses its own quoting rules, not shell quoting. Percent
    // signs are field codes even inside quotes and therefore must be doubled.
    let path = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("executable path is not UTF-8"))?;
    anyhow::ensure!(
        !path.contains(['\n', '\r']),
        "executable path contains a newline"
    );
    let mut out = String::from("\"");
    for ch in path.chars() {
        match ch {
            '%' => out.push_str("%%"),
            '\\' | '"' | '`' | '$' => {
                // The desktop-file parser consumes one level of escaping
                // before the Exec parser handles the remaining backslash.
                out.push_str("\\\\");
                if ch == '\\' {
                    out.push_str("\\\\");
                    continue;
                }
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out.push('"');
    Ok(out)
}

#[cfg(target_os = "linux")]
pub fn set_autostart(enabled: bool) -> anyhow::Result<()> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|p| Path::new(p).is_absolute())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .ok_or_else(|| anyhow::anyhow!("no XDG configuration directory"))?;
    let dir = config.join("autostart");
    let file = dir.join("clip-line.desktop");
    if enabled {
        std::fs::create_dir_all(&dir)?;
        let exe = std::env::current_exe()?;
        let content = format!(
            "[Desktop Entry]\nType=Application\nName=clipLine\nExec={}\nX-GNOME-Autostart-enabled=true\n",
            desktop_exec(&exe)?
        );
        std::fs::write(file, content)?;
    } else if let Err(e) = std::fs::remove_file(file)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        return Err(e.into());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(target_os = "macos")]
pub fn set_autostart(enabled: bool) -> anyhow::Result<()> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("no home directory"))?;
    let dir = PathBuf::from(home).join("Library/LaunchAgents");
    let file = dir.join("dev.clip-line.plist");
    if enabled {
        std::fs::create_dir_all(&dir)?;
        let exe = std::env::current_exe()?;
        let content = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key><string>dev.clip-line</string><key>ProgramArguments</key><array><string>{}</string></array><key>RunAtLoad</key><true/></dict></plist>\n",
            xml_escape(&exe.to_string_lossy())
        );
        std::fs::write(file, content)?;
    } else if let Err(e) = std::fs::remove_file(file)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        return Err(e.into());
    }
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    #[test]
    fn desktop_command_quotes_field_codes_and_reserved_characters() {
        assert_eq!(
            desktop_exec(Path::new("/tmp/a b%\"$`\\c")).unwrap(),
            "\"/tmp/a b%%\\\\\"\\\\$\\\\`\\\\\\\\c\""
        );
    }
}
