//! macOS adapters behind cfg(target_os = "macos"); empty stubs elsewhere so the
//! workspace always compiles (SPEC phase 3: CI matrix stays green on Windows).

#[cfg(target_os = "macos")]
pub mod capture;
#[cfg(target_os = "macos")]
pub mod paste;
#[cfg(target_os = "macos")]
pub use capture::MacCapture;
#[cfg(target_os = "macos")]
pub use paste::MacPaster;

#[cfg(target_os = "macos")]
pub fn focused_application() -> Option<i32> {
    objc2_app_kit::NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .map(|app| app.processIdentifier())
        .filter(|pid| *pid > 0 && *pid != std::process::id() as i32)
}

#[cfg(target_os = "macos")]
pub fn focus_application(pid: i32) -> anyhow::Result<()> {
    use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
    let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
        .ok_or_else(|| anyhow::anyhow!("Target application has exited"))?;
    anyhow::ensure!(
        app.activateWithOptions(NSApplicationActivationOptions::empty()),
        "Could not activate target application"
    );
    for _ in 0..20 {
        if focused_application() == Some(pid) {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    anyhow::bail!("Could not focus target application; use Copy only and paste manually")
}
