use crate::grid::{Grid, Selection};
pub use crate::input::{KeyEventKind, key_to_bytes};
use crate::terminal::{CursorStyle, TerminalView};
use std::io::Write;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use winit::event_loop::EventLoopProxy;
use winit::keyboard::{Key, ModifiersState, NamedKey};

const IME_KEY_DEDUPE_WINDOW: Duration = Duration::from_millis(50);

fn input_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("HANDTERM_TRACE_INPUT").is_some())
}

pub fn trace_input(message: impl AsRef<str>) {
    if input_trace_enabled() {
        eprintln!("handterm input-trace: {}", message.as_ref());
    }
}

pub fn parse_synthetic_key(spec: &str) -> Key {
    match spec.to_ascii_lowercase().as_str() {
        "enter" | "return" => Key::Named(NamedKey::Enter),
        "tab" => Key::Named(NamedKey::Tab),
        "escape" | "esc" => Key::Named(NamedKey::Escape),
        "backspace" => Key::Named(NamedKey::Backspace),
        "space" => Key::Named(NamedKey::Space),
        "up" => Key::Named(NamedKey::ArrowUp),
        "down" => Key::Named(NamedKey::ArrowDown),
        "left" => Key::Named(NamedKey::ArrowLeft),
        "right" => Key::Named(NamedKey::ArrowRight),
        "home" => Key::Named(NamedKey::Home),
        "end" => Key::Named(NamedKey::End),
        "delete" => Key::Named(NamedKey::Delete),
        "page_up" | "pageup" => Key::Named(NamedKey::PageUp),
        "page_down" | "pagedown" => Key::Named(NamedKey::PageDown),
        "alt" => Key::Named(NamedKey::Alt),
        "shift" => Key::Named(NamedKey::Shift),
        "control" | "ctrl" => Key::Named(NamedKey::Control),
        "super" | "meta" => Key::Named(NamedKey::Super),
        _ => Key::Character(spec.into()),
    }
}

pub fn synthetic_modifiers_state(
    ctrl: bool,
    alt: bool,
    shift: bool,
    super_key: bool,
) -> ModifiersState {
    let mut modifiers = ModifiersState::empty();
    modifiers.set(ModifiersState::CONTROL, ctrl);
    modifiers.set(ModifiersState::ALT, alt);
    modifiers.set(ModifiersState::SHIFT, shift);
    modifiers.set(ModifiersState::SUPER, super_key);
    modifiers
}

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

#[derive(Debug, Clone)]
pub struct StartupTiming {
    started_at: Instant,
    pty_spawned_at: Option<Instant>,
    first_pty_event_at: Option<Instant>,
    first_pty_read_at: Option<Instant>,
    first_visible_output_at: Option<Instant>,
    first_present_at: Option<Instant>,
    first_present_after_visible_at: Option<Instant>,
    bytes_read: usize,
    bytes_before_visible: Option<usize>,
    logged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentTextKeyEvent {
    text: String,
    at: Instant,
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

impl StartupTiming {
    pub fn new(started_at: Instant) -> Self {
        Self {
            started_at,
            pty_spawned_at: None,
            first_pty_event_at: None,
            first_pty_read_at: None,
            first_visible_output_at: None,
            first_present_at: None,
            first_present_after_visible_at: None,
            bytes_read: 0,
            bytes_before_visible: None,
            logged: false,
        }
    }

    pub fn mark_pty_spawned(&mut self, at: Instant) {
        let _ = self.pty_spawned_at.get_or_insert(at);
    }

    pub fn mark_pty_event(&mut self, at: Instant) {
        let _ = self.first_pty_event_at.get_or_insert(at);
    }

    pub fn mark_pty_read(&mut self, at: Instant, bytes: usize) {
        if bytes == 0 {
            return;
        }
        self.bytes_read += bytes;
        let _ = self.first_pty_read_at.get_or_insert(at);
    }

    pub fn maybe_mark_visible<T: TerminalView + ?Sized>(&mut self, at: Instant, terminal: &T) {
        if self.first_visible_output_at.is_some() || !terminal_has_visible_content(terminal) {
            return;
        }
        self.first_visible_output_at = Some(at);
        self.bytes_before_visible = Some(self.bytes_read);
    }

    pub fn mark_present(&mut self, at: Instant) {
        let _ = self.first_present_at.get_or_insert(at);
        if self.first_visible_output_at.is_some() {
            let _ = self.first_present_after_visible_at.get_or_insert(at);
        }
    }

    pub fn emit_if_ready(&mut self, label: &str, id: u64) {
        if self.logged
            || self.first_present_at.is_none()
            || self.first_visible_output_at.is_none()
            || self.first_present_after_visible_at.is_none()
        {
            return;
        }

        fn fmt_ms(started_at: Instant, at: Option<Instant>) -> String {
            at.map(|instant| {
                format!(
                    "{:.2}ms",
                    instant.duration_since(started_at).as_secs_f64() * 1000.0
                )
            })
            .unwrap_or_else(|| "n/a".to_string())
        }

        let read_to_present = match (self.first_pty_read_at, self.first_present_after_visible_at) {
            (Some(read), Some(present)) => {
                Some(present.duration_since(read).as_secs_f64() * 1000.0)
            }
            _ => None,
        };
        let visible_to_present = match (
            self.first_visible_output_at,
            self.first_present_after_visible_at,
        ) {
            (Some(visible), Some(present)) => {
                Some(present.duration_since(visible).as_secs_f64() * 1000.0)
            }
            _ => None,
        };

        eprintln!(
            "handterm {label}: startup id={id}\n\
             \x20 open_to_pty_spawn={} open_to_first_pty_event={} open_to_first_pty_read={}\n\
             \x20 open_to_first_visible_output={} bytes_before_visible={} open_to_first_present={}\n\
             \x20 open_to_first_visible_present={} first_read_to_visible_present={} first_visible_to_present={}",
            fmt_ms(self.started_at, self.pty_spawned_at),
            fmt_ms(self.started_at, self.first_pty_event_at),
            fmt_ms(self.started_at, self.first_pty_read_at),
            fmt_ms(self.started_at, self.first_visible_output_at),
            self.bytes_before_visible
                .map(|bytes| bytes.to_string())
                .unwrap_or_else(|| "n/a".to_string()),
            fmt_ms(self.started_at, self.first_present_at),
            fmt_ms(self.started_at, self.first_present_after_visible_at),
            read_to_present
                .map(|ms| format!("{ms:.2}ms"))
                .unwrap_or_else(|| "n/a".to_string()),
            visible_to_present
                .map(|ms| format!("{ms:.2}ms"))
                .unwrap_or_else(|| "n/a".to_string()),
        );
        self.logged = true;
    }
}

impl FrameScheduler {
    pub fn mark_io_ready(&mut self, now: Instant, frame_interval: Duration) {
        self.frame_interval = frame_interval;
        self.io_pending = true;
        if self.burst_started_at.is_none() {
            self.burst_started_at = Some(now);
        }
    }

    pub fn mark_io_processed(
        &mut self,
        now: Instant,
        frame_interval: Duration,
        work: RedrawWork,
    ) -> bool {
        self.frame_interval = frame_interval;
        self.io_pending = false;

        if !work.changed {
            return false;
        }

        if work.heavy {
            let burst_started_at = *self.burst_started_at.get_or_insert(now);
            let burst_age = now.saturating_duration_since(burst_started_at);
            if burst_age < self.frame_interval.saturating_mul(2) {
                self.redraw_pending = true;
                self.redraw_at = Some(now + self.frame_interval);
                return false;
            }
        }

        self.redraw_at = None;
        self.redraw_pending = true;
        self.burst_started_at = None;
        true
    }

    pub fn mark_io_ready_light(&mut self) -> bool {
        self.mark_io_processed(
            Instant::now(),
            self.frame_interval,
            RedrawWork {
                changed: true,
                heavy: false,
            },
        )
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
            if work.changed && work.heavy {
                let burst_started_at = self.burst_started_at.unwrap_or(now);
                let burst_age = now.saturating_duration_since(burst_started_at);
                if burst_age < self.frame_interval.saturating_mul(2) {
                    let deadline = now + self.frame_interval;
                    self.io_pending = false;
                    self.redraw_at = Some(deadline);
                    return FrameDecision {
                        request_redraw: false,
                        wait_until: Some(deadline),
                    };
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

pub fn terminal_has_visible_content<T: TerminalView + ?Sized>(terminal: &T) -> bool {
    let grid = terminal.grid();
    for row in 0..grid.rows {
        for col in 0..grid.cols {
            if let Some(grapheme) = grid.cell_grapheme_at_scroll(row, col)
                && grapheme.chars().any(|ch| !ch.is_whitespace())
            {
                return true;
            }
            let ch = grid.cell_at_scroll(row, col).char_display();
            if !ch.is_whitespace() {
                return true;
            }
        }
    }
    false
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

    mix(&mut hash, terminal.content_generation());

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

pub fn normalize_ime_dedupe_text(text: &str) -> Option<String> {
    if text.is_empty() {
        return None;
    }

    Some(match text {
        "\u{7f}" => "\u{8}".to_string(),
        "\n" | "\r\n" => "\r".to_string(),
        _ => text.to_string(),
    })
}

pub fn named_key_ime_dedupe_text(key: &Key) -> Option<&'static str> {
    match key {
        Key::Named(NamedKey::Space) => Some(" "),
        Key::Named(NamedKey::Tab) => Some("\t"),
        Key::Named(NamedKey::Enter) => Some("\r"),
        Key::Named(NamedKey::Backspace) => Some("\u{8}"),
        _ => None,
    }
}

pub fn key_ime_dedupe_text(key: &Key, event_text: Option<&str>) -> Option<String> {
    if let Some(text) = event_text.and_then(normalize_ime_dedupe_text) {
        return Some(text);
    }

    match key {
        Key::Character(text) => normalize_ime_dedupe_text(text),
        Key::Named(_) => named_key_ime_dedupe_text(key).map(ToString::to_string),
        _ => None,
    }
}

pub fn should_skip_duplicate_ime_key_event(
    pending_ime_commit: &mut Option<String>,
    event_kind: KeyEventKind,
    event_text: Option<&str>,
) -> bool {
    if !matches!(event_kind, KeyEventKind::Press) {
        return false;
    }

    let should_skip = pending_ime_commit
        .as_deref()
        .filter(|text| !text.is_empty())
        == event_text.and_then(normalize_ime_dedupe_text).as_deref();
    *pending_ime_commit = None;
    should_skip
}

pub fn should_skip_duplicate_ime_input(
    pending_ime_commit: &mut Option<String>,
    event_kind: KeyEventKind,
    event_text: Option<&str>,
    encoded_bytes: Option<&[u8]>,
) -> bool {
    if !matches!(event_kind, KeyEventKind::Press) {
        return false;
    }

    let normalized_event_text = event_text.and_then(normalize_ime_dedupe_text);
    let should_skip = pending_ime_commit
        .as_deref()
        .filter(|text| !text.is_empty())
        .is_some_and(|text| {
            normalized_event_text.as_deref() == Some(text)
                || encoded_bytes.is_some_and(|bytes| text.as_bytes() == bytes)
        });
    *pending_ime_commit = None;
    should_skip
}

fn text_for_ime_key_dedupe(
    event_text: Option<&str>,
    encoded_bytes: Option<&[u8]>,
) -> Option<String> {
    if let Some(text) = event_text.and_then(normalize_ime_dedupe_text) {
        return Some(text);
    }

    let bytes = encoded_bytes?;
    let text = std::str::from_utf8(bytes).ok()?.trim_matches('\0');
    let text = normalize_ime_dedupe_text(text)?;
    if text
        .chars()
        .any(|ch| ch.is_control() && !matches!(ch, ' ' | '\t' | '\n' | '\r' | '\u{8}'))
    {
        return None;
    }
    Some(text)
}

pub fn remember_text_key_event(
    recent_text_key_event: &mut Option<RecentTextKeyEvent>,
    event_kind: KeyEventKind,
    event_text: Option<&str>,
    encoded_bytes: Option<&[u8]>,
    now: Instant,
) {
    if !matches!(event_kind, KeyEventKind::Press) {
        return;
    }

    *recent_text_key_event = text_for_ime_key_dedupe(event_text, encoded_bytes)
        .map(|text| RecentTextKeyEvent { text, at: now });
}

pub fn should_skip_ime_commit_after_key_event(
    recent_text_key_event: &mut Option<RecentTextKeyEvent>,
    text: &str,
    now: Instant,
) -> bool {
    let Some(recent) = recent_text_key_event.take() else {
        return false;
    };

    normalize_ime_dedupe_text(text).as_deref() == Some(recent.text.as_str())
        && now
            .checked_duration_since(recent.at)
            .is_some_and(|elapsed| elapsed <= IME_KEY_DEDUPE_WINDOW)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Base64DecodeError {
    InvalidByte,
}

pub fn base64_decode(input: &[u8]) -> Result<Vec<u8>, Base64DecodeError> {
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
            return Err(Base64DecodeError::InvalidByte);
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
        match poll(&mut fds, PollTimeout::from(500u16)) {
            Ok(0) => continue,
            Ok(_) => {
                let has_data = fds[0]
                    .revents()
                    .is_some_and(|r| r.intersects(PollFlags::POLLIN));
                let secondary_data = fds.len() > 1
                    && fds[1]
                        .revents()
                        .is_some_and(|r| r.intersects(PollFlags::POLLIN));
                if has_data || secondary_data {
                    let _ = proxy.send_event(event.clone());
                }
                if fds[0]
                    .revents()
                    .is_some_and(|revents| revents.contains(PollFlags::POLLHUP))
                {
                    let _ = proxy.send_event(event.clone());
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
        terminal
            .grid
            .set_cell_with_grapheme(0, 0, crate::grid::Cell::BLANK, Some("♠️".into()));
        let with_suit = visual_signature(&terminal);
        assert_ne!(with_heart, with_suit);
    }

    #[test]
    fn frame_scheduler_immediate_for_light_changes() {
        let start = Instant::now();
        let frame_interval = Duration::from_millis(8);
        let mut scheduler = FrameScheduler::default();
        scheduler.mark_io_ready(start, frame_interval);

        let decision = scheduler.prepare_redraw(start + Duration::from_millis(1), || RedrawWork {
            changed: true,
            heavy: false,
        });
        assert!(decision.request_redraw);
        assert!(decision.wait_until.is_none());
    }

    #[test]
    fn frame_scheduler_defers_heavy_burst() {
        let start = Instant::now();
        let frame_interval = Duration::from_millis(8);
        let mut scheduler = FrameScheduler::default();
        scheduler.mark_io_ready(start, frame_interval);

        let decision = scheduler.prepare_redraw(start + Duration::from_millis(1), || RedrawWork {
            changed: true,
            heavy: true,
        });
        assert!(!decision.request_redraw);
        assert!(decision.wait_until.is_some());

        let decision = scheduler.prepare_redraw(start + Duration::from_millis(9), || RedrawWork {
            changed: true,
            heavy: false,
        });
        assert!(decision.request_redraw);
    }

    #[test]
    fn frame_scheduler_light_io_ready_redraws_immediately() {
        let mut scheduler = FrameScheduler::default();
        scheduler.mark_io_ready_light();

        let decision = scheduler.prepare_redraw(Instant::now(), || RedrawWork {
            changed: true,
            heavy: false,
        });
        assert!(decision.request_redraw);
    }

    #[test]
    fn ime_commit_dedupes_matching_followup_key_press() {
        let mut pending = Some(" ".to_string());
        assert!(should_skip_duplicate_ime_key_event(
            &mut pending,
            KeyEventKind::Press,
            Some(" "),
        ));
        assert_eq!(pending, None);
    }

    #[test]
    fn ime_commit_does_not_dedupe_different_key_press() {
        let mut pending = Some(" ".to_string());
        assert!(!should_skip_duplicate_ime_key_event(
            &mut pending,
            KeyEventKind::Press,
            Some("a"),
        ));
        assert_eq!(pending, None);
    }

    #[test]
    fn ime_commit_dedupes_matching_named_key_bytes() {
        let mut pending = Some(" ".to_string());
        assert!(should_skip_duplicate_ime_input(
            &mut pending,
            KeyEventKind::Press,
            None,
            Some(b" "),
        ));
        assert_eq!(pending, None);
    }

    #[test]
    fn ime_commit_does_not_dedupe_different_named_key_bytes() {
        let mut pending = Some(" ".to_string());
        assert!(!should_skip_duplicate_ime_input(
            &mut pending,
            KeyEventKind::Press,
            None,
            Some(b"a"),
        ));
        assert_eq!(pending, None);
    }

    #[test]
    fn ime_commit_dedupes_when_text_or_bytes_match() {
        let mut pending = Some(" ".to_string());
        assert!(should_skip_duplicate_ime_input(
            &mut pending,
            KeyEventKind::Press,
            Some(" "),
            Some(b"x"),
        ));
        assert_eq!(pending, None);

        let mut pending = Some(" ".to_string());
        assert!(should_skip_duplicate_ime_input(
            &mut pending,
            KeyEventKind::Press,
            key_ime_dedupe_text(&Key::Character(" ".into()), None).as_deref(),
            Some(b"\x1b[32u"),
        ));
        assert_eq!(pending, None);
    }

    #[test]
    fn key_event_dedupes_matching_followup_ime_commit_text() {
        let now = Instant::now();
        let mut recent = None;
        remember_text_key_event(&mut recent, KeyEventKind::Press, Some(" "), Some(b" "), now);

        assert!(should_skip_ime_commit_after_key_event(
            &mut recent,
            " ",
            now + Duration::from_millis(5),
        ));
        assert_eq!(recent, None);
    }

    #[test]
    fn key_event_dedupes_matching_followup_ime_commit_bytes_only_space() {
        let now = Instant::now();
        let mut recent = None;
        remember_text_key_event(&mut recent, KeyEventKind::Press, None, Some(b" "), now);

        assert!(should_skip_ime_commit_after_key_event(
            &mut recent,
            " ",
            now + Duration::from_millis(5),
        ));
        assert_eq!(recent, None);
    }

    #[test]
    fn stale_key_event_does_not_dedupe_later_ime_commit() {
        let now = Instant::now();
        let mut recent = None;
        remember_text_key_event(&mut recent, KeyEventKind::Press, Some(" "), Some(b" "), now);

        assert!(!should_skip_ime_commit_after_key_event(
            &mut recent,
            " ",
            now + Duration::from_millis(200),
        ));
        assert_eq!(recent, None);
    }

    #[test]
    fn named_backspace_reports_canonical_ime_dedupe_text() {
        assert_eq!(
            named_key_ime_dedupe_text(&Key::Named(NamedKey::Backspace)),
            Some("\u{8}")
        );
        assert_eq!(
            normalize_ime_dedupe_text("\u{7f}"),
            Some("\u{8}".to_string())
        );
        assert_eq!(
            key_ime_dedupe_text(&Key::Character(" ".into()), None),
            Some(" ".to_string())
        );
    }

    #[test]
    fn ime_commit_dedupes_matching_named_backspace_key_event() {
        let mut pending = Some("\u{8}".to_string());
        assert!(should_skip_duplicate_ime_input(
            &mut pending,
            KeyEventKind::Press,
            named_key_ime_dedupe_text(&Key::Named(NamedKey::Backspace)),
            Some(&[0x7f]),
        ));
        assert_eq!(pending, None);

        let mut pending = Some("\u{8}".to_string());
        assert!(should_skip_duplicate_ime_input(
            &mut pending,
            KeyEventKind::Press,
            Some("\u{7f}"),
            Some(&[0x7f]),
        ));
        assert_eq!(pending, None);
    }

    #[test]
    fn key_event_dedupes_matching_followup_backspace_ime_commit() {
        let now = Instant::now();
        let mut recent = None;
        remember_text_key_event(
            &mut recent,
            KeyEventKind::Press,
            named_key_ime_dedupe_text(&Key::Named(NamedKey::Backspace)),
            Some(&[0x7f]),
            now,
        );

        assert!(should_skip_ime_commit_after_key_event(
            &mut recent,
            "\u{8}",
            now + Duration::from_millis(5),
        ));
        assert_eq!(recent, None);

        remember_text_key_event(
            &mut recent,
            KeyEventKind::Press,
            named_key_ime_dedupe_text(&Key::Named(NamedKey::Backspace)),
            Some(&[0x7f]),
            now,
        );
        assert!(should_skip_ime_commit_after_key_event(
            &mut recent,
            "\u{7f}",
            now + Duration::from_millis(5),
        ));
        assert_eq!(recent, None);
    }

    #[test]
    fn parse_synthetic_key_supports_named_and_character_keys() {
        assert_eq!(parse_synthetic_key("space"), Key::Named(NamedKey::Space));
        assert_eq!(
            parse_synthetic_key("Backspace"),
            Key::Named(NamedKey::Backspace)
        );
        assert_eq!(parse_synthetic_key("x"), Key::Character("x".into()));
        assert_eq!(parse_synthetic_key("👨‍💻"), Key::Character("👨‍💻".into()));
    }

    #[test]
    fn synthetic_modifiers_state_sets_requested_bits() {
        let modifiers = synthetic_modifiers_state(true, false, true, true);
        assert!(modifiers.control_key());
        assert!(!modifiers.alt_key());
        assert!(modifiers.shift_key());
        assert!(modifiers.super_key());
    }
}
