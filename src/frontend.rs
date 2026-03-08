use crate::grid::{Grid, Selection};
use crate::ipc::{IpcAction, Request, Response};
use crate::terminal::{CursorStyle, Terminal};
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use winit::event_loop::EventLoopProxy;
use winit::keyboard::{Key, NamedKey};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisualState {
    cursor: Option<(usize, usize, CursorStyle)>,
    selection: Option<Selection>,
    scroll_offset: usize,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FrameScheduler {
    io_pending: bool,
    redraw_pending: bool,
    redraw_at: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameDecision {
    pub request_redraw: bool,
    pub wait_until: Option<Instant>,
}

impl VisualState {
    pub fn capture(terminal: &Terminal) -> Self {
        let cursor = if terminal.cursor_visible && terminal.grid.scroll_offset == 0 {
            let (col, row) = terminal.grid.cursor_pos();
            Some((col, row, terminal.cursor_style))
        } else {
            None
        };

        Self {
            cursor,
            selection: terminal.grid.selection,
            scroll_offset: terminal.grid.scroll_offset,
        }
    }
}

impl FrameScheduler {
    pub fn mark_io_ready(&mut self, now: Instant, frame_interval: Duration) {
        self.io_pending = true;
        self.redraw_at.get_or_insert(now + frame_interval);
    }

    pub fn mark_redraw_needed(&mut self) {
        self.redraw_at = None;
        self.redraw_pending = true;
    }

    pub fn prepare_redraw<F>(&mut self, now: Instant, mut process_io: F) -> FrameDecision
    where
        F: FnMut() -> bool,
    {
        if let Some(deadline) = self.redraw_at
            && now < deadline
        {
            return FrameDecision {
                request_redraw: false,
                wait_until: Some(deadline),
            };
        }

        if self.io_pending {
            if process_io() {
                self.redraw_pending = true;
            }
            self.io_pending = false;
        }

        self.redraw_at = None;

        FrameDecision {
            request_redraw: std::mem::take(&mut self.redraw_pending),
            wait_until: None,
        }
    }
}

pub fn sync_visual_damage(grid: &mut Grid, previous: Option<VisualState>, current: VisualState) {
    let Some(previous) = previous else {
        grid.mark_all_dirty();
        return;
    };

    if previous.selection != current.selection || previous.scroll_offset != current.scroll_offset {
        grid.mark_all_dirty();
        return;
    }

    if previous.cursor != current.cursor {
        mark_cursor_dirty(grid, previous.cursor);
        mark_cursor_dirty(grid, current.cursor);
    }
}

fn mark_cursor_dirty(grid: &mut Grid, cursor: Option<(usize, usize, CursorStyle)>) {
    if let Some((col, row, _)) = cursor {
        grid.mark_cell_dirty(row, col);
    }
}

pub fn handle_ipc_request(terminal: &mut Terminal, req: &Request) -> (Response, IpcAction) {
    match req.cmd.as_str() {
        "get-text" => {
            let text = if let Some(obj) = req.args.as_object() {
                let start = obj
                    .get("start_row")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize;
                let end = obj
                    .get("end_row")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(terminal.grid.rows as u64) as usize;
                terminal.grid.get_text(start, end)
            } else {
                terminal.grid.get_all_text()
            };
            (
                Response::ok(serde_json::json!({ "text": text })),
                IpcAction::None,
            )
        }
        "send-text" => {
            let text = req
                .args
                .as_object()
                .and_then(|o| o.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if text.is_empty() {
                (Response::err("missing 'text' argument"), IpcAction::None)
            } else {
                (
                    Response::ok_empty(),
                    IpcAction::SendText(text.as_bytes().to_vec()),
                )
            }
        }
        "send-key" => {
            let key = req
                .args
                .as_object()
                .and_then(|o| o.get("key"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let bytes = match key {
                "enter" | "return" => Some(b"\r".to_vec()),
                "tab" => Some(b"\t".to_vec()),
                "escape" | "esc" => Some(b"\x1b".to_vec()),
                "backspace" => Some(b"\x7f".to_vec()),
                "up" => Some(b"\x1b[A".to_vec()),
                "down" => Some(b"\x1b[B".to_vec()),
                "right" => Some(b"\x1b[C".to_vec()),
                "left" => Some(b"\x1b[D".to_vec()),
                "home" => Some(b"\x1b[H".to_vec()),
                "end" => Some(b"\x1b[F".to_vec()),
                "delete" => Some(b"\x1b[3~".to_vec()),
                "page_up" => Some(b"\x1b[5~".to_vec()),
                "page_down" => Some(b"\x1b[6~".to_vec()),
                "space" => Some(b" ".to_vec()),
                k if k.starts_with("ctrl+") && k.len() == 6 => {
                    let ch = k.as_bytes()[5];
                    if ch.is_ascii_alphabetic() {
                        Some(vec![ch.to_ascii_lowercase() - b'a' + 1])
                    } else {
                        None
                    }
                }
                _ => None,
            };
            match bytes {
                Some(bytes) => (Response::ok_empty(), IpcAction::SendText(bytes)),
                None => (
                    Response::err(format!("unknown key: {key}")),
                    IpcAction::None,
                ),
            }
        }
        "get-cursor" => {
            let (col, row) = terminal.grid.cursor_pos();
            (
                Response::ok(serde_json::json!({ "row": row, "col": col })),
                IpcAction::None,
            )
        }
        "get-size" => (
            Response::ok(serde_json::json!({
                "cols": terminal.cols,
                "rows": terminal.rows,
            })),
            IpcAction::None,
        ),
        "set-title" => {
            let title = req
                .args
                .as_object()
                .and_then(|o| o.get("title"))
                .and_then(|v| v.as_str())
                .unwrap_or("handterm")
                .to_string();
            (Response::ok_empty(), IpcAction::SetTitle(title))
        }
        "close" => (Response::ok_empty(), IpcAction::Close),
        "ls" => (
            Response::ok(serde_json::json!({
                "commands": [
                    "get-text", "send-text", "send-key",
                    "get-cursor", "get-size", "set-title",
                    "close", "ls"
                ]
            })),
            IpcAction::None,
        ),
        _ => (
            Response::err(format!("unknown command: {}", req.cmd)),
            IpcAction::None,
        ),
    }
}

pub fn key_to_bytes(key: &Key, app_cursor: bool, ctrl: bool) -> Option<Vec<u8>> {
    if ctrl {
        if let Key::Character(s) = key {
            let ch = s.chars().next()?;
            if ch.is_ascii_alphabetic() {
                return Some(vec![ch.to_ascii_lowercase() as u8 - b'a' + 1]);
            }
            match ch {
                '@' | ' ' | '`' => return Some(vec![0]),
                '[' | '\x1b' => return Some(vec![0x1b]),
                '\\' => return Some(vec![0x1c]),
                ']' => return Some(vec![0x1d]),
                '^' | '~' => return Some(vec![0x1e]),
                '_' | '/' => return Some(vec![0x1f]),
                _ => {}
            }
        }
    }

    match key {
        Key::Character(s) => Some(s.as_bytes().to_vec()),
        Key::Named(named) => match named {
            NamedKey::Enter => Some(b"\r".to_vec()),
            NamedKey::Backspace => Some(b"\x7f".to_vec()),
            NamedKey::Tab => Some(b"\t".to_vec()),
            NamedKey::Escape => Some(b"\x1b".to_vec()),
            NamedKey::ArrowUp if app_cursor => Some(b"\x1bOA".to_vec()),
            NamedKey::ArrowDown if app_cursor => Some(b"\x1bOB".to_vec()),
            NamedKey::ArrowRight if app_cursor => Some(b"\x1bOC".to_vec()),
            NamedKey::ArrowLeft if app_cursor => Some(b"\x1bOD".to_vec()),
            NamedKey::Home if app_cursor => Some(b"\x1bOH".to_vec()),
            NamedKey::End if app_cursor => Some(b"\x1bOF".to_vec()),
            NamedKey::ArrowUp => Some(b"\x1b[A".to_vec()),
            NamedKey::ArrowDown => Some(b"\x1b[B".to_vec()),
            NamedKey::ArrowRight => Some(b"\x1b[C".to_vec()),
            NamedKey::ArrowLeft => Some(b"\x1b[D".to_vec()),
            NamedKey::Home => Some(b"\x1b[H".to_vec()),
            NamedKey::End => Some(b"\x1b[F".to_vec()),
            NamedKey::Delete => Some(b"\x1b[3~".to_vec()),
            NamedKey::PageUp => Some(b"\x1b[5~".to_vec()),
            NamedKey::PageDown => Some(b"\x1b[6~".to_vec()),
            NamedKey::Space => {
                if ctrl {
                    Some(vec![0])
                } else {
                    Some(b" ".to_vec())
                }
            }
            NamedKey::Insert => Some(b"\x1b[2~".to_vec()),
            NamedKey::F1 => Some(b"\x1bOP".to_vec()),
            NamedKey::F2 => Some(b"\x1bOQ".to_vec()),
            NamedKey::F3 => Some(b"\x1bOR".to_vec()),
            NamedKey::F4 => Some(b"\x1bOS".to_vec()),
            NamedKey::F5 => Some(b"\x1b[15~".to_vec()),
            NamedKey::F6 => Some(b"\x1b[17~".to_vec()),
            NamedKey::F7 => Some(b"\x1b[18~".to_vec()),
            NamedKey::F8 => Some(b"\x1b[19~".to_vec()),
            NamedKey::F9 => Some(b"\x1b[20~".to_vec()),
            NamedKey::F10 => Some(b"\x1b[21~".to_vec()),
            NamedKey::F11 => Some(b"\x1b[23~".to_vec()),
            NamedKey::F12 => Some(b"\x1b[24~".to_vec()),
            _ => None,
        },
        _ => None,
    }
}

pub fn scroll_to_bytes(up: bool, app_cursor: bool) -> Vec<u8> {
    match (up, app_cursor) {
        (true, true) => b"\x1bOA".to_vec(),
        (false, true) => b"\x1bOB".to_vec(),
        (true, false) => b"\x1b[A".to_vec(),
        (false, false) => b"\x1b[B".to_vec(),
    }
}

pub fn paste_from_clipboard() -> Option<Vec<u8>> {
    std::process::Command::new("wl-paste")
        .arg("--no-newline")
        .output()
        .ok()
        .map(|output| output.stdout)
        .filter(|stdout| !stdout.is_empty())
}

pub fn copy_to_clipboard(text: &[u8]) {
    let mut child = std::process::Command::new("wl-copy")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .ok();
    if let Some(ref mut child) = child {
        if let Some(ref mut stdin) = child.stdin {
            let _ = stdin.write_all(text);
        }
    }
}

pub fn open_url(url: &str) {
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

pub fn base64_decode(input: &[u8]) -> Result<Vec<u8>, ()> {
    const TABLE: [u8; 256] = {
        let mut table = [0xffu8; 256];
        let mut i = 0u8;
        while i < 26 {
            table[(b'A' + i) as usize] = i;
            table[(b'a' + i) as usize] = i + 26;
            i += 1;
        }
        let mut digit = 0u8;
        while digit < 10 {
            table[(b'0' + digit) as usize] = digit + 52;
            digit += 1;
        }
        table[b'+' as usize] = 62;
        table[b'/' as usize] = 63;
        table
    };

    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0u32;

    for &b in input {
        if b == b'=' || b == b'\n' || b == b'\r' {
            continue;
        }
        let val = TABLE[b as usize];
        if val == 0xff {
            return Err(());
        }
        buf = (buf << 6) | val as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }

    Ok(out)
}

pub fn spawn_pty_watcher<E: Clone + Send + 'static>(
    thread_name: &str,
    pty_fd: i32,
    ipc_fd: i32,
    proxy: EventLoopProxy<E>,
    event: E,
    stop: Arc<AtomicBool>,
) {
    let thread_name = thread_name.to_string();
    std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || pty_watcher_thread(pty_fd, ipc_fd, proxy, event, stop))
        .expect("failed to spawn pty watcher thread");
}

fn pty_watcher_thread<E: Clone + Send + 'static>(
    pty_fd: i32,
    ipc_fd: i32,
    proxy: EventLoopProxy<E>,
    event: E,
    stop: Arc<AtomicBool>,
) {
    use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
    use std::os::fd::BorrowedFd;

    let mut fds = Vec::with_capacity(2);
    fds.push(PollFd::new(
        unsafe { BorrowedFd::borrow_raw(pty_fd) },
        PollFlags::POLLIN | PollFlags::POLLHUP,
    ));
    if ipc_fd >= 0 {
        fds.push(PollFd::new(
            unsafe { BorrowedFd::borrow_raw(ipc_fd) },
            PollFlags::POLLIN,
        ));
    }

    while !stop.load(Ordering::Relaxed) {
        match poll(&mut fds, PollTimeout::from(100u16)) {
            Ok(0) => continue,
            Ok(_) => {
                let _ = proxy.send_event(event.clone());
                if fds[0]
                    .revents()
                    .map_or(false, |revents| revents.contains(PollFlags::POLLHUP))
                {
                    break;
                }
            }
            Err(nix::errno::Errno::EINTR) => continue,
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::Terminal;

    #[test]
    fn visual_damage_marks_previous_and_current_cursor_cells() {
        let mut terminal = Terminal::new(4, 2);
        let previous = VisualState::capture(&terminal);
        terminal.grid.clear_dirty();
        terminal.grid.set_cursor(0, 2);
        let current = VisualState::capture(&terminal);

        sync_visual_damage(&mut terminal.grid, Some(previous), current);

        assert!(terminal.grid.is_cell_dirty(0, 0));
        assert!(terminal.grid.is_cell_dirty(0, 2));
    }

    #[test]
    fn clearing_selection_forces_full_redraw() {
        let mut terminal = Terminal::new(4, 2);
        let previous = VisualState {
            cursor: Some((0, 0, CursorStyle::Block)),
            selection: Some(Selection {
                start_col: 0,
                start_row: 0,
                end_col: 1,
                end_row: 0,
            }),
            scroll_offset: 0,
        };

        terminal.grid.clear_dirty();
        let current = VisualState::capture(&terminal);
        sync_visual_damage(&mut terminal.grid, Some(previous), current);

        assert!(terminal.grid.all_dirty);
    }

    #[test]
    fn frame_scheduler_waits_until_deadline_before_processing_io() {
        let mut scheduler = FrameScheduler::default();
        let start = Instant::now();
        let frame_interval = Duration::from_millis(8);
        let mut process_calls = 0;

        scheduler.mark_io_ready(start, frame_interval);
        scheduler.mark_io_ready(start, frame_interval);

        let before_deadline = scheduler.prepare_redraw(start + Duration::from_millis(4), || {
            process_calls += 1;
            true
        });

        assert!(!before_deadline.request_redraw);
        assert_eq!(before_deadline.wait_until, Some(start + frame_interval));
        assert_eq!(process_calls, 0);

        let at_deadline = scheduler.prepare_redraw(start + frame_interval, || {
            process_calls += 1;
            true
        });

        assert!(at_deadline.request_redraw);
        assert_eq!(at_deadline.wait_until, None);
        assert_eq!(process_calls, 1);
    }

    #[test]
    fn frame_scheduler_skips_redraw_when_io_changes_nothing() {
        let mut scheduler = FrameScheduler::default();
        let start = Instant::now();
        let frame_interval = Duration::from_millis(8);
        let mut process_calls = 0;

        scheduler.mark_io_ready(start, frame_interval);

        let should_redraw = scheduler.prepare_redraw(start + frame_interval, || {
            process_calls += 1;
            false
        });

        assert!(!should_redraw.request_redraw);
        assert_eq!(process_calls, 1);
    }

    #[test]
    fn frame_scheduler_preserves_manual_redraw_requests() {
        let mut scheduler = FrameScheduler::default();

        scheduler.mark_redraw_needed();

        let first = scheduler.prepare_redraw(Instant::now(), || false);
        let second = scheduler.prepare_redraw(Instant::now(), || false);

        assert!(first.request_redraw);
        assert!(!second.request_redraw);
    }
}
