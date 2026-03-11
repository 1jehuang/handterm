use crate::grid::{Grid, Selection};
pub use crate::input::{KeyEventKind, key_to_bytes};
use crate::terminal::{CursorStyle, TerminalView};
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
        terminal.process("❤️".as_bytes());
        let with_heart = visual_signature(&terminal);
        terminal.grid.set_cell_with_grapheme(0, 0, crate::grid::Cell::BLANK, Some("♠️".into()));
        let with_suit = visual_signature(&terminal);
        assert_ne!(with_heart, with_suit);
    }

    #[test]
    fn frame_scheduler_defers_redraw_until_quiet_period() {
        let start = Instant::now();
        let frame_interval = Duration::from_millis(8);
        let mut scheduler = FrameScheduler::default();
        scheduler.mark_io_ready(start, frame_interval);

        let decision = scheduler.prepare_redraw(start + Duration::from_millis(1), || RedrawWork {
            changed: true,
            heavy: false,
        });
        assert!(!decision.request_redraw);
        assert!(decision.wait_until.is_some());

        let decision = scheduler.prepare_redraw(start + Duration::from_millis(9), || RedrawWork {
            changed: true,
            heavy: false,
        });
        assert!(decision.request_redraw);
        assert!(decision.wait_until.is_none());
    }
}
