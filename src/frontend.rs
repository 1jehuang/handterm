use crate::grid::{Grid, Selection};
pub use crate::input::{KeyEventKind, key_to_bytes};
use crate::ipc::{IpcAction, Request, Response};
use crate::terminal::{CursorStyle, Terminal, TerminalView};
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use winit::event_loop::EventLoopProxy;

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
    burst_started_at: Option<Instant>,
    frame_interval: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameDecision {
    pub request_redraw: bool,
    pub wait_until: Option<Instant>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RedrawWork {
    pub changed: bool,
    pub heavy: bool,
}

impl VisualState {
    pub fn capture<T: TerminalView + ?Sized>(terminal: &T) -> Self {
        let grid = terminal.grid();
        let cursor = if terminal.cursor_visible() && grid.scroll_offset == 0 {
            let (col, row) = grid.cursor_pos();
            Some((col, row, terminal.cursor_style()))
        } else {
            None
        };

        Self {
            cursor,
            selection: grid.selection,
            scroll_offset: grid.scroll_offset,
        }
    }
}

impl FrameScheduler {
    pub fn mark_io_ready(&mut self, now: Instant, frame_interval: Duration) {
        self.frame_interval = frame_interval;
        self.io_pending = true;
        let burst_started_at = *self.burst_started_at.get_or_insert(now);
        let quiet_deadline = now + frame_interval;
        let max_deadline = burst_started_at + frame_interval.saturating_mul(3);
        self.redraw_at = Some(quiet_deadline.min(max_deadline));
    }

    pub fn mark_redraw_needed(&mut self) {
        self.redraw_at = None;
        self.redraw_pending = true;
    }

    pub fn prepare_redraw<F>(&mut self, now: Instant, mut process_io: F) -> FrameDecision
    where
        F: FnMut() -> RedrawWork,
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
            let work = process_io();
            if work.changed {
                self.redraw_pending = true;
            }
            if work.changed && work.heavy && !self.frame_interval.is_zero() {
                let burst_started_at = self.burst_started_at.unwrap_or(now);
                let burst_age = now.saturating_duration_since(burst_started_at);
                if burst_age >= self.frame_interval.saturating_mul(3) {
                    let settle_deadline = now + self.frame_interval;
                    let max_deadline = burst_started_at + self.frame_interval.saturating_mul(6);
                    let deadline = settle_deadline.min(max_deadline);
                    if now < deadline {
                        self.io_pending = false;
                        self.redraw_at = Some(deadline);
                        return FrameDecision {
                            request_redraw: false,
                            wait_until: Some(deadline),
                        };
                    }
                }
            }
            self.io_pending = false;
        }

        self.redraw_at = None;
        self.burst_started_at = None;

        FrameDecision {
            request_redraw: std::mem::take(&mut self.redraw_pending),
            wait_until: None,
        }
    }
}

pub fn classify_redraw_work<T: TerminalView + ?Sized>(terminal: &T, changed: bool) -> RedrawWork {
    if !changed {
        return RedrawWork::default();
    }

    let grid = terminal.grid();
    let total_cells = grid.rows * grid.cols;
    let dirty_cells = grid.dirty_cell_count();
    let heavy = grid.all_dirty || dirty_cells.saturating_mul(3) >= total_cells.max(1);

    RedrawWork { changed, heavy }
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

pub fn visual_signature<T: TerminalView + ?Sized>(terminal: &T) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    #[inline]
    fn mix(hash: &mut u64, value: u64) {
        *hash ^= value;
        *hash = hash.wrapping_mul(FNV_PRIME);
    }

    #[inline]
    fn mix_bytes(hash: &mut u64, bytes: &[u8]) {
        mix(hash, bytes.len() as u64);
        for &byte in bytes {
            mix(hash, byte as u64);
        }
    }

    let mut hash = FNV_OFFSET;
    let grid = terminal.grid();

    mix(&mut hash, terminal.cols() as u64);
    mix(&mut hash, terminal.rows() as u64);
    mix(&mut hash, terminal.cursor_visible() as u64);
    mix(&mut hash, terminal.cursor_style() as u64);
    mix(&mut hash, grid.scroll_offset as u64);

    if let Some(selection) = grid.selection {
        mix(&mut hash, 1);
        mix(&mut hash, selection.start_row as u64);
        mix(&mut hash, selection.start_col as u64);
        mix(&mut hash, selection.end_row as u64);
        mix(&mut hash, selection.end_col as u64);
    } else {
        mix(&mut hash, 0);
    }

    if terminal.cursor_visible() && grid.scroll_offset == 0 {
        let (cursor_col, cursor_row) = grid.cursor_pos();
        mix(&mut hash, cursor_row as u64);
        mix(&mut hash, cursor_col as u64);
    }

    for row in 0..grid.rows {
        for col in 0..grid.cols {
            let cell = grid.cell_at_scroll(row, col);
            mix(&mut hash, cell.ch as u64);
            if let Some(grapheme) = grid.cell_grapheme_at_scroll(row, col) {
                mix(&mut hash, 1);
                mix_bytes(&mut hash, grapheme.as_bytes());
            } else {
                mix(&mut hash, 0);
            }
            mix(&mut hash, cell.fg as u64);
            mix(&mut hash, cell.bg as u64);
            mix(&mut hash, cell.underline_color as u64);
            mix(&mut hash, cell.attrs as u64);
            mix(&mut hash, cell.flags as u64);
            mix(&mut hash, cell.underline_style as u64);
        }
    }

    mix(&mut hash, terminal.kitty_generation());
    mix(&mut hash, terminal.kitty_placements().len() as u64);
    for placement in terminal.kitty_placements() {
        mix(&mut hash, placement.image_id as u64);
        mix(&mut hash, placement.row as u64);
        mix(&mut hash, placement.col as u64);
        mix(&mut hash, placement.rows as u64);
        mix(&mut hash, placement.cols as u64);
    }

    hash
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
    if let Some(ref mut child) = child
        && let Some(ref mut stdin) = child.stdin
    {
        let _ = stdin.write_all(text);
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

pub fn spawn_fd_watcher<E: Clone + Send + 'static>(
    thread_name: &str,
    primary_fd: i32,
    secondary_fd: i32,
    proxy: EventLoopProxy<E>,
    event: E,
    stop: Arc<AtomicBool>,
) {
    let thread_name = thread_name.to_string();
    std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || fd_watcher_thread(primary_fd, secondary_fd, proxy, event, stop))
        .expect("failed to spawn fd watcher thread");
}

pub fn spawn_pty_watcher<E: Clone + Send + 'static>(
    thread_name: &str,
    pty_fd: i32,
    ipc_fd: i32,
    proxy: EventLoopProxy<E>,
    event: E,
    stop: Arc<AtomicBool>,
) {
    spawn_fd_watcher(thread_name, pty_fd, ipc_fd, proxy, event, stop);
}

fn fd_watcher_thread<E: Clone + Send + 'static>(
    primary_fd: i32,
    secondary_fd: i32,
    proxy: EventLoopProxy<E>,
    event: E,
    stop: Arc<AtomicBool>,
) {
    use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
    use std::os::fd::BorrowedFd;

    let mut fds = Vec::with_capacity(2);
    fds.push(PollFd::new(
        unsafe { BorrowedFd::borrow_raw(primary_fd) },
        PollFlags::POLLIN | PollFlags::POLLHUP,
    ));
    if secondary_fd >= 0 {
        fds.push(PollFd::new(
            unsafe { BorrowedFd::borrow_raw(secondary_fd) },
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
                    .is_some_and(|revents| revents.contains(PollFlags::POLLHUP))
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
    use crate::config::AppConfig;
    use crate::font::GlyphAtlas;
    use crate::render::OffscreenRenderer;
    use crate::terminal::Terminal;

    fn simulate_redraw_times(
        io_events_ms: &[u64],
        sample_times_ms: &[u64],
        frame_interval_ms: u64,
    ) -> Vec<u64> {
        let start = Instant::now();
        let frame_interval = Duration::from_millis(frame_interval_ms);
        let mut scheduler = FrameScheduler::default();
        let mut redraws = Vec::new();
        let mut io_index = 0;

        for &sample_ms in sample_times_ms {
            while io_index < io_events_ms.len() && io_events_ms[io_index] <= sample_ms {
                scheduler.mark_io_ready(start + Duration::from_millis(io_events_ms[io_index]), frame_interval);
                io_index += 1;
            }

            let decision = scheduler.prepare_redraw(start + Duration::from_millis(sample_ms), || {
                RedrawWork {
                    changed: true,
                    heavy: false,
                }
            });
            if decision.request_redraw {
                redraws.push(sample_ms);
            }
        }

        redraws
    }

    fn new_atlas(config: &AppConfig) -> GlyphAtlas {
        GlyphAtlas::new(config.style.font_size).expect("should load a monospace font for rendering")
    }

    fn simulate_presented_frames(
        cols: u16,
        rows: u16,
        chunks: &[(&[u8], u64)],
        sample_times_ms: &[u64],
        frame_interval_ms: u64,
    ) -> Vec<Vec<u32>> {
        let config = AppConfig::default();
        let mut atlas = new_atlas(&config);
        let mut terminal = Terminal::new(cols, rows);
        let mut renderer = OffscreenRenderer::new(cols, rows, &atlas);
        let start = Instant::now();
        let frame_interval = Duration::from_millis(frame_interval_ms);
        let mut scheduler = FrameScheduler::default();
        let mut next_chunk = 0usize;
        let mut pending_chunks: Vec<&[u8]> = Vec::new();
        let mut frames = Vec::new();

        for &sample_ms in sample_times_ms {
            while next_chunk < chunks.len() && chunks[next_chunk].1 <= sample_ms {
                scheduler.mark_io_ready(start + Duration::from_millis(chunks[next_chunk].1), frame_interval);
                pending_chunks.push(chunks[next_chunk].0);
                next_chunk += 1;
            }

            let decision = scheduler.prepare_redraw(start + Duration::from_millis(sample_ms), || {
                if pending_chunks.is_empty() {
                    return RedrawWork::default();
                }
                for chunk in pending_chunks.drain(..) {
                    terminal.process(chunk);
                }
                classify_redraw_work(&terminal, true)
            });

            if decision.request_redraw {
                renderer.render(&mut terminal, &mut atlas, &config);
                frames.push(renderer.pixels.clone());
            }
        }

        frames
    }

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
    fn visual_signature_changes_with_visual_state() {
        let mut terminal = Terminal::new(4, 2);
        let base = visual_signature(&terminal);

        terminal.process(b"ab");
        let after_text = visual_signature(&terminal);
        assert_ne!(base, after_text);

        terminal.grid.selection = Some(Selection {
            start_col: 0,
            start_row: 0,
            end_col: 1,
            end_row: 0,
        });
        let after_selection = visual_signature(&terminal);
        assert_ne!(after_text, after_selection);

        terminal.process(b"\x1b_Ga=T,i=5,f=32,s=1,v=1,c=1,r=1;/wAA/w==\x1b\\");
        let after_image = visual_signature(&terminal);
        assert_ne!(after_selection, after_image);
    }

    #[test]
    fn visual_signature_changes_when_only_grapheme_changes() {
        let mut terminal = Terminal::new(2, 1);
        let mut cell = *terminal.grid.cell_at(0, 0);
        cell.ch = '❤' as u32;
        terminal
            .grid
            .set_cell_with_grapheme(0, 0, cell, Some("❤️".into()));
        let with_heart = visual_signature(&terminal);

        terminal
            .grid
            .set_cell_with_grapheme(0, 0, cell, Some("♥️".into()));
        let with_suit = visual_signature(&terminal);

        assert_ne!(with_heart, with_suit);
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
            RedrawWork {
                changed: true,
                heavy: false,
            }
        });

        assert!(!before_deadline.request_redraw);
        assert_eq!(before_deadline.wait_until, Some(start + frame_interval));
        assert_eq!(process_calls, 0);

        let at_deadline = scheduler.prepare_redraw(start + frame_interval, || {
            process_calls += 1;
            RedrawWork {
                changed: true,
                heavy: false,
            }
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
            RedrawWork::default()
        });

        assert!(!should_redraw.request_redraw);
        assert_eq!(process_calls, 1);
    }

    #[test]
    fn frame_scheduler_preserves_manual_redraw_requests() {
        let mut scheduler = FrameScheduler::default();

        scheduler.mark_redraw_needed();

        let first = scheduler.prepare_redraw(Instant::now(), RedrawWork::default);
        let second = scheduler.prepare_redraw(Instant::now(), RedrawWork::default);

        assert!(first.request_redraw);
        assert!(!second.request_redraw);
    }

    #[test]
    fn frame_scheduler_extends_deadline_during_active_io_burst() {
        let mut scheduler = FrameScheduler::default();
        let start = Instant::now();
        let frame_interval = Duration::from_millis(8);

        scheduler.mark_io_ready(start, frame_interval);
        scheduler.mark_io_ready(start + Duration::from_millis(4), frame_interval);

        let decision = scheduler.prepare_redraw(start + Duration::from_millis(9), || {
            RedrawWork {
                changed: true,
                heavy: false,
            }
        });

        assert!(!decision.request_redraw);
        assert_eq!(decision.wait_until, Some(start + Duration::from_millis(12)));
    }

    #[test]
    fn frame_scheduler_caps_io_burst_deferral() {
        let mut scheduler = FrameScheduler::default();
        let start = Instant::now();
        let frame_interval = Duration::from_millis(8);

        scheduler.mark_io_ready(start, frame_interval);
        scheduler.mark_io_ready(start + Duration::from_millis(18), frame_interval);

        let decision = scheduler.prepare_redraw(start + Duration::from_millis(23), || {
            RedrawWork {
                changed: true,
                heavy: false,
            }
        });

        assert!(!decision.request_redraw);
        assert_eq!(decision.wait_until, Some(start + Duration::from_millis(24)));

        let at_cap = scheduler.prepare_redraw(start + Duration::from_millis(24), || {
            RedrawWork {
                changed: true,
                heavy: false,
            }
        });
        assert!(at_cap.request_redraw);
    }

    #[test]
    fn heavy_redraws_get_one_extra_settle_interval_after_long_burst() {
        let mut scheduler = FrameScheduler::default();
        let start = Instant::now();
        let frame_interval = Duration::from_millis(8);

        scheduler.mark_io_ready(start, frame_interval);
        scheduler.mark_io_ready(start + Duration::from_millis(8), frame_interval);
        scheduler.mark_io_ready(start + Duration::from_millis(16), frame_interval);

        let at_deadline = scheduler.prepare_redraw(start + Duration::from_millis(24), || RedrawWork {
            changed: true,
            heavy: true,
        });

        assert!(!at_deadline.request_redraw);
        assert_eq!(
            at_deadline.wait_until,
            Some(start + Duration::from_millis(32))
        );

        let settled = scheduler.prepare_redraw(start + Duration::from_millis(32), RedrawWork::default);
        assert!(settled.request_redraw);
    }

    #[test]
    fn flicker_burst_test_collapses_many_io_events_into_one_redraw() {
        let redraws = simulate_redraw_times(
            &[0, 3, 6, 9, 12, 15, 18],
            &[4, 8, 12, 16, 20, 24, 28, 32],
            8,
        );

        assert_eq!(redraws, vec![24]);
    }

    #[test]
    fn flicker_burst_test_allows_next_frame_after_burst_finishes() {
        let redraws = simulate_redraw_times(
            &[0, 3, 6, 20],
            &[4, 8, 12, 16, 20, 24, 28, 32],
            8,
        );

        assert_eq!(redraws, vec![16, 28]);
    }

    #[test]
    fn presented_frames_skip_partial_full_screen_repaint_bursts() {
        let chunks: [(&[u8], u64); 5] = [
            (b"\x1b[?1049h\x1b[2J\x1b[H", 0),
            (b"\x1b[38;5;39mstatus\x1b[0m\r\n", 3),
            (b"alpha beta gamma\r\n", 6),
            (b"delta epsilon\r\n", 9),
            (b"zeta eta theta\r\niota kappa\r\n", 12),
        ];
        let samples = [4, 8, 12, 16, 20, 24, 28];
        let frames = simulate_presented_frames(32, 6, &chunks, &samples, 8);

        assert_eq!(frames.len(), 1, "repaint burst should collapse to one presented frame");

        let final_frame = simulate_presented_frames(32, 6, &chunks, &[28], 0)
            .pop()
            .expect("final frame should render");
        assert_eq!(frames[0], final_frame);
    }

    #[test]
    fn presented_frames_allow_next_stable_frame_after_burst() {
        let chunks: [(&[u8], u64); 6] = [
            (b"\x1b[?1049h\x1b[2J\x1b[H", 0),
            (b"one\r\n", 3),
            (b"two\r\n", 6),
            (b"three\r\n", 9),
            (b"\x1b[H\x1b[2Kdone\r\n", 26),
            (b"\x1b[2;1Hsteady\r\n", 27),
        ];
        let samples = [4, 8, 12, 16, 20, 24, 28, 32, 36];
        let frames = simulate_presented_frames(16, 4, &chunks, &samples, 8);

        assert_eq!(frames.len(), 2, "separate repaint bursts should present two stable frames");
        assert_ne!(frames[0], frames[1], "distinct stable states should present distinct frames");
    }

    #[test]
    fn presented_frames_skip_partial_tui_startup_and_help_overlay_repaints() {
        let chunks: [(&[u8], u64); 13] = [
            (b"\x1b[?1049h\x1b[2J\x1b[H", 0),
            (b"\x1b[48;5;236m\x1b[38;5;255m jcode \x1b[0m", 2),
            (b"\x1b[2;1Hprojects", 4),
            (b"\x1b[3;1Hmain.rs", 6),
            (b"\x1b[2;20Hfn main() {", 8),
            (b"\x1b[3;20H    println!(\"hi\");", 10),
            (b"\x1b[4;20H}", 12),
            (b"\x1b[20;1H/help", 28),
            (b"\x1b[2J\x1b[H", 30),
            (b"\x1b[48;5;24m\x1b[38;5;255m Help \x1b[0m", 32),
            (b"\x1b[3;3HESC  close", 34),
            (b"\x1b[4;3H/    commands", 36),
            (b"\x1b[5;3H?    search", 38),
        ];
        let samples = [4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 48];
        let frames = simulate_presented_frames(48, 20, &chunks, &samples, 8);

        assert_eq!(
            frames.len(),
            2,
            "startup burst and help-overlay burst should each present one stable frame"
        );
        assert_ne!(
            frames[0], frames[1],
            "help overlay should produce a distinct second stable frame"
        );

        let startup_final = simulate_presented_frames(48, 20, &chunks[..7], &[24], 0)
            .pop()
            .expect("startup frame should render");
        let help_final = simulate_presented_frames(48, 20, &chunks, &[48], 0)
            .pop()
            .expect("help frame should render");

        assert_eq!(frames[0], startup_final, "startup should skip partial app-launch frames");
        assert_eq!(frames[1], help_final, "help should skip partial overlay frames");
    }

    #[test]
    fn presented_frames_skip_long_heavy_repaint_bursts() {
        let chunks: [(&[u8], u64); 7] = [
            (b"\x1b[?1049h\x1b[2J\x1b[H", 0),
            (b"\x1b[48;5;236m\x1b[38;5;255m jcode \x1b[0m", 6),
            (b"\x1b[2;1Hprojects", 12),
            (b"\x1b[3;1Hmain.rs", 18),
            (b"\x1b[2;20Hfn main() {", 24),
            (b"\x1b[3;20H    println!(\"hi\");", 30),
            (b"\x1b[4;20H}", 36),
        ];
        let samples = [8, 16, 24, 32, 40, 48, 56];
        let frames = simulate_presented_frames(48, 20, &chunks, &samples, 8);

        assert_eq!(
            frames.len(),
            1,
            "long heavy repaint burst should settle to one presented frame"
        );

        let final_frame = simulate_presented_frames(48, 20, &chunks, &[56], 0)
            .pop()
            .expect("final settled frame should render");
        assert_eq!(frames[0], final_frame);
    }
}
