use std::io::Write;

use crate::config::{WindowPosition, WindowPositionPreset};
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::window::WindowAttributes;

/// Horizontal/vertical offset between successive centered windows so extra
/// windows cascade instead of stacking exactly on top of each other.
const CASCADE_STEP_PX: i32 = 32;
/// After this many cascaded windows, wrap back to the centered origin.
const CASCADE_WRAP: usize = 8;

/// Compute the initial top-left position for a new window, in physical
/// pixels, or `None` to let the OS place it.
///
/// `monitor` is the target monitor's origin and size in physical pixels
/// (usually the primary monitor). `cascade_index` is the number of windows
/// already open, used to offset centered windows so they do not stack.
///
/// This is a pure function so placement policy is unit-testable without a
/// window system. On Wayland compositors ignore client-requested positions,
/// so the result is a no-op hint there; macOS and X11 honor it.
pub fn initial_window_position(
    position: WindowPosition,
    window: PhysicalSize<u32>,
    monitor: Option<(PhysicalPosition<i32>, PhysicalSize<u32>)>,
    cascade_index: usize,
) -> Option<PhysicalPosition<i32>> {
    match position {
        WindowPosition::Preset(WindowPositionPreset::Auto) => None,
        WindowPosition::Fixed(x, y) => {
            let origin = monitor
                .map(|(pos, _)| pos)
                .unwrap_or_else(|| PhysicalPosition::new(0, 0));
            Some(PhysicalPosition::new(origin.x + x, origin.y + y))
        }
        WindowPosition::Preset(WindowPositionPreset::Center) => {
            let (mon_pos, mon_size) = monitor?;
            let step = ((cascade_index % CASCADE_WRAP) as i32) * CASCADE_STEP_PX;
            let x = mon_pos.x + (mon_size.width as i32 - window.width as i32) / 2 + step;
            let y = mon_pos.y + (mon_size.height as i32 - window.height as i32) / 2 + step;
            // Keep the window fully on the monitor even when cascaded or
            // larger than the display.
            let max_x = mon_pos.x + (mon_size.width as i32 - window.width as i32).max(0);
            let max_y = mon_pos.y + (mon_size.height as i32 - window.height as i32).max(0);
            Some(PhysicalPosition::new(
                x.clamp(mon_pos.x, max_x.max(mon_pos.x)),
                y.clamp(mon_pos.y, max_y.max(mon_pos.y)),
            ))
        }
    }
}

/// Geometry (origin, size) of the monitor new windows should spawn on:
/// the primary monitor when reported, otherwise the first available one.
pub fn spawn_monitor_geometry(
    event_loop: &winit::event_loop::ActiveEventLoop,
) -> Option<(PhysicalPosition<i32>, PhysicalSize<u32>)> {
    event_loop
        .primary_monitor()
        .or_else(|| event_loop.available_monitors().next())
        .map(|monitor| (monitor.position(), monitor.size()))
        .filter(|(_, size)| size.width > 0 && size.height > 0)
}

/// Apply an optional spawn position to window attributes.
pub fn with_initial_position(
    attrs: WindowAttributes,
    position: Option<PhysicalPosition<i32>>,
) -> WindowAttributes {
    match position {
        Some(pos) => attrs.with_position(winit::dpi::Position::Physical(pos)),
        None => attrs,
    }
}

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

/// Apply the "no window chrome" look for `window.decorations = false`.
///
/// On macOS a fully undecorated window loses the standard rounded-corner
/// window shape and shadow, so instead of dropping decorations we keep the
/// system frame and hide the titlebar: transparent titlebar, hidden
/// traffic-light buttons, and a fullsize content view. This preserves the
/// native rounded corners while showing no chrome.
///
/// On other platforms this simply disables decorations.
pub fn with_decorations(attrs: WindowAttributes, decorations: bool) -> WindowAttributes {
    if decorations {
        return attrs;
    }
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::WindowAttributesExtMacOS;
        attrs
            .with_titlebar_transparent(true)
            .with_title_hidden(true)
            .with_titlebar_buttons_hidden(true)
            .with_fullsize_content_view(true)
    }
    #[cfg(not(target_os = "macos"))]
    {
        attrs.with_decorations(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor_1080p() -> Option<(PhysicalPosition<i32>, PhysicalSize<u32>)> {
        Some((PhysicalPosition::new(0, 0), PhysicalSize::new(1920, 1080)))
    }

    #[test]
    fn auto_defers_to_os_placement() {
        let pos = initial_window_position(
            WindowPosition::Preset(WindowPositionPreset::Auto),
            PhysicalSize::new(800, 600),
            monitor_1080p(),
            0,
        );
        assert_eq!(pos, None);
    }

    #[test]
    fn center_centers_first_window_on_monitor() {
        let pos = initial_window_position(
            WindowPosition::default(),
            PhysicalSize::new(800, 600),
            monitor_1080p(),
            0,
        )
        .expect("centered position should resolve");
        assert_eq!(pos, PhysicalPosition::new((1920 - 800) / 2, (1080 - 600) / 2));
    }

    #[test]
    fn center_respects_monitor_origin_on_secondary_display() {
        let monitor = Some((PhysicalPosition::new(1920, 200), PhysicalSize::new(1920, 1080)));
        let pos = initial_window_position(
            WindowPosition::default(),
            PhysicalSize::new(800, 600),
            monitor,
            0,
        )
        .expect("centered position should resolve");
        assert_eq!(
            pos,
            PhysicalPosition::new(1920 + (1920 - 800) / 2, 200 + (1080 - 600) / 2)
        );
    }

    #[test]
    fn center_cascades_additional_windows() {
        let first = initial_window_position(
            WindowPosition::default(),
            PhysicalSize::new(800, 600),
            monitor_1080p(),
            0,
        )
        .unwrap();
        let second = initial_window_position(
            WindowPosition::default(),
            PhysicalSize::new(800, 600),
            monitor_1080p(),
            1,
        )
        .unwrap();
        assert_eq!(second.x - first.x, CASCADE_STEP_PX);
        assert_eq!(second.y - first.y, CASCADE_STEP_PX);
    }

    #[test]
    fn cascade_wraps_back_to_center() {
        let first = initial_window_position(
            WindowPosition::default(),
            PhysicalSize::new(800, 600),
            monitor_1080p(),
            0,
        );
        let wrapped = initial_window_position(
            WindowPosition::default(),
            PhysicalSize::new(800, 600),
            monitor_1080p(),
            CASCADE_WRAP,
        );
        assert_eq!(first, wrapped);
    }

    #[test]
    fn center_clamps_oversized_window_to_monitor_origin() {
        let pos = initial_window_position(
            WindowPosition::default(),
            PhysicalSize::new(4000, 3000),
            monitor_1080p(),
            0,
        )
        .expect("clamped position should resolve");
        assert_eq!(pos, PhysicalPosition::new(0, 0));
    }

    #[test]
    fn center_clamps_cascade_to_stay_on_monitor() {
        // A window nearly as large as the monitor: the cascade step would
        // push it off-screen, so it clamps to the bottom-right edge.
        let pos = initial_window_position(
            WindowPosition::default(),
            PhysicalSize::new(1900, 1060),
            monitor_1080p(),
            3,
        )
        .expect("clamped position should resolve");
        assert_eq!(pos, PhysicalPosition::new(20, 20));
    }

    #[test]
    fn center_without_monitor_falls_back_to_os_placement() {
        let pos = initial_window_position(
            WindowPosition::default(),
            PhysicalSize::new(800, 600),
            None,
            0,
        );
        assert_eq!(pos, None);
    }

    #[test]
    fn fixed_position_is_relative_to_monitor_origin() {
        let monitor = Some((PhysicalPosition::new(1920, 0), PhysicalSize::new(1920, 1080)));
        let pos = initial_window_position(
            WindowPosition::Fixed(100, 50),
            PhysicalSize::new(800, 600),
            monitor,
            0,
        );
        assert_eq!(pos, Some(PhysicalPosition::new(2020, 50)));
    }

    #[test]
    fn fixed_position_without_monitor_uses_global_origin() {
        let pos = initial_window_position(
            WindowPosition::Fixed(100, 50),
            PhysicalSize::new(800, 600),
            None,
            0,
        );
        assert_eq!(pos, Some(PhysicalPosition::new(100, 50)));
    }
}
