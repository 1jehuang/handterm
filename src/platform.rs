use std::io::Write;

use winit::window::WindowAttributes;

/// Read the system clipboard as raw bytes.
///
/// Uses the Wayland `wl-paste` tool on Linux and `pbpaste` on macOS.
pub fn paste_from_clipboard() -> Option<Vec<u8>> {
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("pbpaste");

    #[cfg(not(target_os = "macos"))]
    let mut command = {
        let mut c = std::process::Command::new("wl-paste");
        c.arg("--no-newline");
        c
    };

    command
        .output()
        .ok()
        .map(|output| output.stdout)
        .filter(|stdout| !stdout.is_empty())
}

/// Write raw bytes to the system clipboard.
///
/// Uses the Wayland `wl-copy` tool on Linux and `pbcopy` on macOS.
pub fn copy_to_clipboard(text: &[u8]) {
    #[cfg(target_os = "macos")]
    let program = "pbcopy";
    #[cfg(not(target_os = "macos"))]
    let program = "wl-copy";

    let mut child = std::process::Command::new(program)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .ok();
    if let Some(ref mut child) = child
        && let Some(ref mut stdin) = child.stdin
    {
        let _ = stdin.write_all(text);
    }
    // Reap the child so it does not linger as a zombie.
    if let Some(mut child) = child {
        let _ = child.wait();
    }
}

/// Open a URL in the user's default browser/handler.
pub fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(not(target_os = "macos"))]
    let program = "xdg-open";

    let _ = std::process::Command::new(program).arg(url).spawn();
}

/// Apply the Wayland app-id to window attributes.
///
/// On Wayland this sets the `app_id` (used by compositors for grouping and
/// icons). On other platforms (macOS, X11) there is no equivalent attribute,
/// so this is a no-op that simply returns the attributes unchanged.
pub fn with_app_id(attrs: WindowAttributes, _app_id: &str) -> WindowAttributes {
    #[cfg(target_os = "linux")]
    {
        use winit::platform::wayland::WindowAttributesExtWayland;
        attrs.with_name(_app_id, _app_id)
    }
    #[cfg(not(target_os = "linux"))]
    {
        attrs
    }
}
