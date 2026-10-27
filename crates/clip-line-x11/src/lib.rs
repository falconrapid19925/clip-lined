//! X11 adapters behind cfg (linux + BSDs); empty stubs elsewhere so the
//! workspace always compiles.

#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
pub mod capture;
#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
pub mod paste;
#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
pub use capture::X11Capture;
#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
pub use paste::X11Paster;

/// Save the window that owns keyboard focus before showing the picker.
#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
pub fn focused_window() -> anyhow::Result<u32> {
    use x11rb::protocol::xproto::ConnectionExt;
    let (connection, _) = x11rb::connect(None)?;
    let focus = connection.get_input_focus()?.reply()?.focus;
    anyhow::ensure!(focus > 1, "no focused X11 window");
    Ok(focus)
}

/// Return keyboard focus to the saved window before sending paste keys.
#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
pub fn focus_window(window: u32) -> anyhow::Result<()> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ConnectionExt, InputFocus};
    let (connection, _) = x11rb::connect(None)?;
    connection
        .set_input_focus(InputFocus::PARENT, window, x11rb::CURRENT_TIME)?
        .check()?;
    connection.flush()?;
    anyhow::ensure!(
        connection.get_input_focus()?.reply()?.focus == window,
        "could not restore X11 focus"
    );
    Ok(())
}
