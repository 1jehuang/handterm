use crate::grid::{Grid, Selection};
pub use crate::input::{
    KeyEventKind, apply_modifier_key_transition, effective_modifiers_for_key_event, key_to_bytes,
    modifiers_with_extra,
};
use crate::terminal::{CursorStyle, TerminalView};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};

const IME_KEY_DEDUPE_WINDOW: Duration = Duration::from_millis(50);
const SCROLLBACK_WHEEL_STEP_MULTIPLIER: usize = 2;
const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(500);

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
        "clear" => Key::Named(NamedKey::Clear),
        "menu" | "context_menu" | "contextmenu" => Key::Named(NamedKey::ContextMenu),
        "page_up" | "pageup" => Key::Named(NamedKey::PageUp),
        "page_down" | "pagedown" => Key::Named(NamedKey::PageDown),
        "alt" => Key::Named(NamedKey::Alt),
        "shift" => Key::Named(NamedKey::Shift),
        "control" | "ctrl" => Key::Named(NamedKey::Control),
        "super" | "meta" => Key::Named(NamedKey::Super),
        "caps_lock" | "capslock" => Key::Named(NamedKey::CapsLock),
        "num_lock" | "numlock" => Key::Named(NamedKey::NumLock),
        _ => Key::Character(spec.into()),
    }
}

fn parse_synthetic_named_physical_key(spec: &str) -> Option<KeyCode> {
    match spec {
        "escape" | "esc" => Some(KeyCode::Escape),
        "tab" => Some(KeyCode::Tab),
        "enter" | "return" => Some(KeyCode::Enter),
        "space" => Some(KeyCode::Space),
        "backspace" => Some(KeyCode::Backspace),
        "delete" => Some(KeyCode::Delete),
        "insert" => Some(KeyCode::Insert),
        "home" => Some(KeyCode::Home),
        "end" => Some(KeyCode::End),
        "page_up" | "pageup" => Some(KeyCode::PageUp),
        "page_down" | "pagedown" => Some(KeyCode::PageDown),
        "up" | "arrow_up" | "arrowup" => Some(KeyCode::ArrowUp),
        "down" | "arrow_down" | "arrowdown" => Some(KeyCode::ArrowDown),
        "left" | "arrow_left" | "arrowleft" => Some(KeyCode::ArrowLeft),
        "right" | "arrow_right" | "arrowright" => Some(KeyCode::ArrowRight),
        "caps_lock" | "capslock" => Some(KeyCode::CapsLock),
        "num_lock" | "numlock" => Some(KeyCode::NumLock),
        "scroll_lock" | "scrolllock" => Some(KeyCode::ScrollLock),
        "print_screen" | "printscreen" => Some(KeyCode::PrintScreen),
        "pause" => Some(KeyCode::Pause),
        "context_menu" | "contextmenu" | "menu" => Some(KeyCode::ContextMenu),
        "backquote" => Some(KeyCode::Backquote),
        "minus" => Some(KeyCode::Minus),
        "equal" => Some(KeyCode::Equal),
        "bracket_left" | "bracketleft" => Some(KeyCode::BracketLeft),
        "bracket_right" | "bracketright" => Some(KeyCode::BracketRight),
        "backslash" => Some(KeyCode::Backslash),
        "semicolon" => Some(KeyCode::Semicolon),
        "quote" => Some(KeyCode::Quote),
        "comma" => Some(KeyCode::Comma),
        "period" => Some(KeyCode::Period),
        "slash" => Some(KeyCode::Slash),
        "shift_left" | "shiftleft" => Some(KeyCode::ShiftLeft),
        "shift_right" | "shiftright" => Some(KeyCode::ShiftRight),
        "control_left" | "controlleft" | "ctrl_left" | "ctrlleft" => Some(KeyCode::ControlLeft),
        "control_right" | "controlright" | "ctrl_right" | "ctrlright" => {
            Some(KeyCode::ControlRight)
        }
        "alt_left" | "altleft" => Some(KeyCode::AltLeft),
        "alt_right" | "altright" => Some(KeyCode::AltRight),
        "super_left" | "superleft" => Some(KeyCode::SuperLeft),
        "super_right" | "superright" => Some(KeyCode::SuperRight),
        "meta" => Some(KeyCode::Meta),
        "hyper" => Some(KeyCode::Hyper),
        "numpad0" => Some(KeyCode::Numpad0),
        "numpad1" => Some(KeyCode::Numpad1),
        "numpad2" => Some(KeyCode::Numpad2),
        "numpad3" => Some(KeyCode::Numpad3),
        "numpad4" => Some(KeyCode::Numpad4),
        "numpad5" => Some(KeyCode::Numpad5),
        "numpad6" => Some(KeyCode::Numpad6),
        "numpad7" => Some(KeyCode::Numpad7),
        "numpad8" => Some(KeyCode::Numpad8),
        "numpad9" => Some(KeyCode::Numpad9),
        "numpad_decimal" | "numpaddecimal" => Some(KeyCode::NumpadDecimal),
        "numpad_divide" | "numpaddivide" => Some(KeyCode::NumpadDivide),
        "numpad_multiply" | "numpadmultiply" => Some(KeyCode::NumpadMultiply),
        "numpad_subtract" | "numpadsubtract" => Some(KeyCode::NumpadSubtract),
        "numpad_add" | "numpadadd" => Some(KeyCode::NumpadAdd),
        "numpad_enter" | "numpadenter" => Some(KeyCode::NumpadEnter),
        "numpad_equal" | "numpadequal" => Some(KeyCode::NumpadEqual),
        "numpad_comma" | "numpadcomma" => Some(KeyCode::NumpadComma),
        _ => None,
    }
}

fn parse_synthetic_letter_physical_key(spec: &str) -> Option<KeyCode> {
    let suffix = spec
        .strip_prefix("key_")
        .or_else(|| spec.strip_prefix("key"))?;
    if suffix.len() != 1 {
        return None;
    }
    match suffix.chars().next()? {
        'a' => Some(KeyCode::KeyA),
        'b' => Some(KeyCode::KeyB),
        'c' => Some(KeyCode::KeyC),
        'd' => Some(KeyCode::KeyD),
        'e' => Some(KeyCode::KeyE),
        'f' => Some(KeyCode::KeyF),
        'g' => Some(KeyCode::KeyG),
        'h' => Some(KeyCode::KeyH),
        'i' => Some(KeyCode::KeyI),
        'j' => Some(KeyCode::KeyJ),
        'k' => Some(KeyCode::KeyK),
        'l' => Some(KeyCode::KeyL),
        'm' => Some(KeyCode::KeyM),
        'n' => Some(KeyCode::KeyN),
        'o' => Some(KeyCode::KeyO),
        'p' => Some(KeyCode::KeyP),
        'q' => Some(KeyCode::KeyQ),
        'r' => Some(KeyCode::KeyR),
        's' => Some(KeyCode::KeyS),
        't' => Some(KeyCode::KeyT),
        'u' => Some(KeyCode::KeyU),
        'v' => Some(KeyCode::KeyV),
        'w' => Some(KeyCode::KeyW),
        'x' => Some(KeyCode::KeyX),
        'y' => Some(KeyCode::KeyY),
        'z' => Some(KeyCode::KeyZ),
        _ => None,
    }
}

fn parse_synthetic_digit_physical_key(spec: &str) -> Option<KeyCode> {
    let suffix = spec
        .strip_prefix("digit_")
        .or_else(|| spec.strip_prefix("digit"))?;
    if suffix.len() != 1 {
        return None;
    }
    match suffix.chars().next()? {
        '0' => Some(KeyCode::Digit0),
        '1' => Some(KeyCode::Digit1),
        '2' => Some(KeyCode::Digit2),
        '3' => Some(KeyCode::Digit3),
        '4' => Some(KeyCode::Digit4),
        '5' => Some(KeyCode::Digit5),
        '6' => Some(KeyCode::Digit6),
        '7' => Some(KeyCode::Digit7),
        '8' => Some(KeyCode::Digit8),
        '9' => Some(KeyCode::Digit9),
        _ => None,
    }
}

fn parse_synthetic_function_physical_key(spec: &str) -> Option<KeyCode> {
    let value = spec.strip_prefix('f')?.parse::<u8>().ok()?;
    match value {
        1 => Some(KeyCode::F1),
        2 => Some(KeyCode::F2),
        3 => Some(KeyCode::F3),
        4 => Some(KeyCode::F4),
        5 => Some(KeyCode::F5),
        6 => Some(KeyCode::F6),
        7 => Some(KeyCode::F7),
        8 => Some(KeyCode::F8),
        9 => Some(KeyCode::F9),
        10 => Some(KeyCode::F10),
        11 => Some(KeyCode::F11),
        12 => Some(KeyCode::F12),
        13 => Some(KeyCode::F13),
        14 => Some(KeyCode::F14),
        15 => Some(KeyCode::F15),
        16 => Some(KeyCode::F16),
        17 => Some(KeyCode::F17),
        18 => Some(KeyCode::F18),
        19 => Some(KeyCode::F19),
        20 => Some(KeyCode::F20),
        21 => Some(KeyCode::F21),
        22 => Some(KeyCode::F22),
        23 => Some(KeyCode::F23),
        24 => Some(KeyCode::F24),
        25 => Some(KeyCode::F25),
        26 => Some(KeyCode::F26),
        27 => Some(KeyCode::F27),
        28 => Some(KeyCode::F28),
        29 => Some(KeyCode::F29),
        30 => Some(KeyCode::F30),
        31 => Some(KeyCode::F31),
        32 => Some(KeyCode::F32),
        33 => Some(KeyCode::F33),
        34 => Some(KeyCode::F34),
        35 => Some(KeyCode::F35),
        _ => None,
    }
}

pub fn parse_synthetic_physical_key(spec: Option<&str>) -> Option<PhysicalKey> {
    let spec = spec?;
    let lower = spec.to_ascii_lowercase();
    let code = parse_synthetic_named_physical_key(&lower)
        .or_else(|| parse_synthetic_letter_physical_key(&lower))
        .or_else(|| parse_synthetic_digit_physical_key(&lower))
        .or_else(|| parse_synthetic_function_physical_key(&lower))?;
    Some(PhysicalKey::Code(code))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyntheticModifierState {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub super_key: bool,
    pub hyper: bool,
    pub meta: bool,
    pub caps_lock: bool,
    pub num_lock: bool,
}

pub fn synthetic_modifiers_state(synthetic: SyntheticModifierState) -> ModifiersState {
    let mut modifiers = ModifiersState::empty();
    modifiers.set(ModifiersState::CONTROL, synthetic.ctrl);
    modifiers.set(ModifiersState::ALT, synthetic.alt);
    modifiers.set(ModifiersState::SHIFT, synthetic.shift);
    modifiers.set(ModifiersState::SUPER, synthetic.super_key);
    modifiers_with_extra(
        modifiers,
        synthetic.hyper,
        synthetic.meta,
        synthetic.caps_lock,
        synthetic.num_lock,
    )
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
pub struct CursorBlinkState {
    focused: bool,
    visible: bool,
    next_toggle_at: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameDecision {
    pub request_redraw: bool,
    pub wait_until: Option<Instant>,
}

impl FrameDecision {
    pub fn blocks_periodic_redraw(self) -> bool {
        self.wait_until.is_some()
    }
}

impl CursorBlinkState {
    pub fn new(now: Instant, focused: bool) -> Self {
        Self {
            focused,
            visible: true,
            next_toggle_at: focused.then_some(now + CURSOR_BLINK_INTERVAL),
        }
    }

    pub fn visible(self) -> bool {
        self.visible
    }

    pub fn reset(&mut self, now: Instant, protocol_visible: bool) -> bool {
        let changed = !self.visible;
        self.visible = true;
        self.next_toggle_at =
            (self.focused && protocol_visible).then_some(now + CURSOR_BLINK_INTERVAL);
        changed
    }

    pub fn set_focused(&mut self, focused: bool, now: Instant, protocol_visible: bool) -> bool {
        self.focused = focused;
        self.reset(now, protocol_visible)
    }

    pub fn update(&mut self, now: Instant, protocol_visible: bool) -> bool {
        if !self.focused || !protocol_visible {
            return self.reset(now, protocol_visible);
        }

        let Some(next_toggle_at) = self.next_toggle_at else {
            return self.reset(now, protocol_visible);
        };
        if now < next_toggle_at {
            return false;
        }

        let elapsed_intervals =
            now.duration_since(next_toggle_at).as_millis() / CURSOR_BLINK_INTERVAL.as_millis();
        let toggles = elapsed_intervals.saturating_add(1).min(u32::MAX as u128) as u32;
        let previous = self.visible;
        if toggles % 2 == 1 {
            self.visible = !self.visible;
        }
        self.next_toggle_at = Some(next_toggle_at + CURSOR_BLINK_INTERVAL * toggles);
        previous != self.visible
    }

    pub fn next_deadline(self, protocol_visible: bool) -> Option<Instant> {
        (self.focused && protocol_visible)
            .then_some(self.next_toggle_at)
            .flatten()
    }
}

/// Clear the one-cell selection created by a simple click while preserving a
/// real drag selection. Returns `true` when a collapsed selection was removed.
pub fn clear_collapsed_selection(selection: &mut Option<Selection>) -> bool {
    let collapsed = selection.is_some_and(|selection| {
        selection.start_col == selection.end_col && selection.start_row == selection.end_row
    });
    if collapsed {
        *selection = None;
    }
    collapsed
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StartupTimingSnapshot {
    pub open_to_pty_spawn_ms: Option<f64>,
    pub open_to_first_pty_event_ms: Option<f64>,
    pub open_to_first_pty_read_ms: Option<f64>,
    pub open_to_first_visible_output_ms: Option<f64>,
    pub bytes_before_visible: Option<usize>,
    pub open_to_first_present_ms: Option<f64>,
    pub open_to_first_visible_present_ms: Option<f64>,
    pub first_read_to_visible_present_ms: Option<f64>,
    pub first_visible_to_present_ms: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentTextKeyEvent {
    text: String,
    at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewportScroll {
    pub sample_offset: usize,
    pub fractional_rows: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollbarGeometry {
    pub thumb_y_px: f32,
    pub thumb_h_px: f32,
}

impl ViewportScroll {
    pub const ZERO: Self = Self {
        sample_offset: 0,
        fractional_rows: 0.0,
    };

    pub fn from_scroll_rows(scroll_rows: f32) -> Self {
        const EPSILON: f32 = 0.001;

        let clamped = scroll_rows.max(0.0);
        let sample_offset = clamped.ceil() as usize;
        let fractional_rows = if sample_offset > 0 {
            sample_offset as f32 - clamped
        } else {
            0.0
        };

        Self {
            sample_offset,
            fractional_rows: if fractional_rows.abs() < EPSILON {
                0.0
            } else {
                fractional_rows
            },
        }
    }

    pub fn extra_visible_rows(self) -> usize {
        usize::from(self.fractional_rows > 0.0)
    }

    pub fn viewport_offset_y(self, cell_h: f32) -> f32 {
        -(self.fractional_rows * cell_h)
    }

    pub fn visible_rows(self, base_rows: usize) -> usize {
        base_rows + self.extra_visible_rows()
    }

    pub fn mouse_row_for_pixel_y(self, y_px: f32, cell_h: f32, base_rows: usize) -> usize {
        let cell_h = cell_h.max(1.0);
        let row = ((y_px - self.viewport_offset_y(cell_h)) / cell_h).floor();
        row.max(0.0)
            .min(self.visible_rows(base_rows).saturating_sub(1) as f32) as usize
    }
}

pub fn compute_scrollbar_geometry(
    scrollback_rows: usize,
    visible_rows: usize,
    scroll_rows: f32,
    viewport_h_px: f32,
    min_thumb_px: f32,
) -> Option<ScrollbarGeometry> {
    if scrollback_rows == 0 || visible_rows == 0 || viewport_h_px <= 0.0 {
        return None;
    }

    let total_rows = (scrollback_rows + visible_rows) as f32;
    let visible_rows = visible_rows as f32;
    let track_h_px = viewport_h_px.max(0.0);
    let thumb_h_px = (track_h_px * (visible_rows / total_rows))
        .max(min_thumb_px)
        .min(track_h_px);
    let max_scroll = scrollback_rows as f32;
    let progress = if max_scroll > 0.0 {
        (scroll_rows.clamp(0.0, max_scroll)) / max_scroll
    } else {
        0.0
    };
    let travel_px = (track_h_px - thumb_h_px).max(0.0);

    Some(ScrollbarGeometry {
        thumb_y_px: travel_px * progress,
        thumb_h_px,
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmoothScrollState {
    pub target_rows: f32,
    pub display_rows: f32,
    velocity_rows_per_sec: f32,
    last_tick_at: Option<Instant>,
}

impl Default for SmoothScrollState {
    fn default() -> Self {
        Self {
            target_rows: 0.0,
            display_rows: 0.0,
            velocity_rows_per_sec: 0.0,
            last_tick_at: None,
        }
    }
}

impl SmoothScrollState {
    const SETTLE_EPSILON: f32 = 0.01;
    const VELOCITY_EPSILON: f32 = 0.05;
    const SPRING_RATE: f32 = 20.0;
    const MOMENTUM_DECAY_RATE: f32 = 8.0;
    const MOMENTUM_IMPULSE_PER_ROW: f32 = 45.0;
    const BOTTOM_SNAP_THRESHOLD: f32 = 0.3;

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn clamp(&mut self, max_rows: f32) {
        self.target_rows = self.target_rows.clamp(0.0, max_rows.max(0.0));
        self.display_rows = self.display_rows.clamp(0.0, max_rows.max(0.0));
        if self.target_rows <= Self::SETTLE_EPSILON {
            self.target_rows = 0.0;
            if self.velocity_rows_per_sec < 0.0 {
                self.velocity_rows_per_sec = 0.0;
            }
        }
        if self.display_rows <= Self::SETTLE_EPSILON {
            self.display_rows = 0.0;
        }
        if self.target_rows == 0.0 && self.display_rows < Self::BOTTOM_SNAP_THRESHOLD {
            self.display_rows = 0.0;
        }
        if self.target_rows >= max_rows.max(0.0) - Self::SETTLE_EPSILON {
            self.target_rows = max_rows.max(0.0);
            if self.velocity_rows_per_sec > 0.0 {
                self.velocity_rows_per_sec = 0.0;
            }
        }
        if self.velocity_rows_per_sec.abs() <= Self::VELOCITY_EPSILON {
            self.velocity_rows_per_sec = 0.0;
        }
    }

    pub fn apply_delta(&mut self, delta_rows: f32, up: bool, max_rows: f32) {
        let delta_rows = delta_rows.max(0.0);
        if up {
            self.target_rows += delta_rows;
            self.velocity_rows_per_sec += delta_rows * Self::MOMENTUM_IMPULSE_PER_ROW;
        } else {
            self.target_rows = (self.target_rows - delta_rows).max(0.0);
            self.velocity_rows_per_sec -= delta_rows * Self::MOMENTUM_IMPULSE_PER_ROW;
            if self.target_rows < Self::BOTTOM_SNAP_THRESHOLD {
                self.target_rows = 0.0;
                self.velocity_rows_per_sec = 0.0;
            }
        }
        self.clamp(max_rows);
    }

    pub fn jump_to(&mut self, rows: f32, max_rows: f32) {
        self.target_rows = rows;
        self.velocity_rows_per_sec = 0.0;
        self.clamp(max_rows);
    }

    pub fn snap_to_target(&mut self) {
        self.display_rows = self.target_rows;
        self.velocity_rows_per_sec = 0.0;
        self.last_tick_at = None;
    }

    pub fn displayed_scroll_offset(&self) -> usize {
        self.display_rows.ceil() as usize
    }

    pub fn is_animating(&self) -> bool {
        (self.target_rows - self.display_rows).abs() > Self::SETTLE_EPSILON
            || self.velocity_rows_per_sec.abs() > Self::VELOCITY_EPSILON
    }

    pub fn advance(&mut self, now: Instant, max_rows: f32) -> bool {
        self.clamp(max_rows);
        if !self.is_animating() {
            if (self.display_rows - self.target_rows).abs() > 0.0 {
                self.display_rows = self.target_rows;
                self.last_tick_at = None;
                return true;
            }
            self.last_tick_at = None;
            return false;
        }

        let previous = self.display_rows;
        let dt = self
            .last_tick_at
            .map(|last| (now - last).as_secs_f32())
            .unwrap_or(1.0 / 120.0)
            .clamp(1.0 / 240.0, 0.05);
        self.last_tick_at = Some(now);

        if self.velocity_rows_per_sec != 0.0 {
            self.target_rows += self.velocity_rows_per_sec * dt;
            let decay = (-Self::MOMENTUM_DECAY_RATE * dt).exp();
            self.velocity_rows_per_sec *= decay;
        }

        let alpha = 1.0 - (-Self::SPRING_RATE * dt).exp();
        self.display_rows += (self.target_rows - self.display_rows) * alpha;
        if (self.target_rows - self.display_rows).abs() <= Self::SETTLE_EPSILON
            && self.velocity_rows_per_sec.abs() <= Self::VELOCITY_EPSILON
        {
            self.display_rows = self.target_rows;
            self.velocity_rows_per_sec = 0.0;
            self.last_tick_at = None;
        }
        self.clamp(max_rows);
        (self.display_rows - previous).abs() > 0.0001
    }
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

    pub fn snapshot_if_ready(&self) -> Option<StartupTimingSnapshot> {
        if self.first_present_at.is_none()
            || self.first_visible_output_at.is_none()
            || self.first_present_after_visible_at.is_none()
        {
            return None;
        }

        let since_start = |at: Option<Instant>| {
            at.map(|instant| instant.duration_since(self.started_at).as_secs_f64() * 1000.0)
        };
        let first_read_to_visible_present_ms =
            match (self.first_pty_read_at, self.first_present_after_visible_at) {
                (Some(read), Some(present)) => {
                    Some(present.duration_since(read).as_secs_f64() * 1000.0)
                }
                _ => None,
            };
        let first_visible_to_present_ms = match (
            self.first_visible_output_at,
            self.first_present_after_visible_at,
        ) {
            (Some(visible), Some(present)) => {
                Some(present.duration_since(visible).as_secs_f64() * 1000.0)
            }
            _ => None,
        };

        Some(StartupTimingSnapshot {
            open_to_pty_spawn_ms: since_start(self.pty_spawned_at),
            open_to_first_pty_event_ms: since_start(self.first_pty_event_at),
            open_to_first_pty_read_ms: since_start(self.first_pty_read_at),
            open_to_first_visible_output_ms: since_start(self.first_visible_output_at),
            bytes_before_visible: self.bytes_before_visible,
            open_to_first_present_ms: since_start(self.first_present_at),
            open_to_first_visible_present_ms: since_start(self.first_present_after_visible_at),
            first_read_to_visible_present_ms,
            first_visible_to_present_ms,
        })
    }

    pub fn emit_if_ready(&mut self, label: &str, id: u64) -> bool {
        if self.logged {
            return false;
        }

        let Some(snapshot) = self.snapshot_if_ready() else {
            return false;
        };

        fn fmt_ms(value: Option<f64>) -> String {
            value
                .map(|ms| format!("{ms:.2}ms"))
                .unwrap_or_else(|| "n/a".to_string())
        }

        eprintln!(
            "handterm {label}: startup id={id}\n\
             \x20 open_to_pty_spawn={} open_to_first_pty_event={} open_to_first_pty_read={}\n\
             \x20 open_to_first_visible_output={} bytes_before_visible={} open_to_first_present={}\n\
             \x20 open_to_first_visible_present={} first_read_to_visible_present={} first_visible_to_present={}",
            fmt_ms(snapshot.open_to_pty_spawn_ms),
            fmt_ms(snapshot.open_to_first_pty_event_ms),
            fmt_ms(snapshot.open_to_first_pty_read_ms),
            fmt_ms(snapshot.open_to_first_visible_output_ms),
            snapshot
                .bytes_before_visible
                .map(|bytes| bytes.to_string())
                .unwrap_or_else(|| "n/a".to_string()),
            fmt_ms(snapshot.open_to_first_present_ms),
            fmt_ms(snapshot.open_to_first_visible_present_ms),
            fmt_ms(snapshot.first_read_to_visible_present_ms),
            fmt_ms(snapshot.first_visible_to_present_ms),
        );
        self.logged = true;
        true
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

pub fn scrollback_wheel_delta(lines: usize) -> usize {
    lines.saturating_mul(SCROLLBACK_WHEEL_STEP_MULTIPLIER)
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
    fn simple_click_selection_is_cleared() {
        let mut selection = Some(Selection {
            start_col: 2,
            start_row: 1,
            end_col: 2,
            end_row: 1,
        });

        assert!(clear_collapsed_selection(&mut selection));
        assert!(selection.is_none());
    }

    #[test]
    fn forward_and_reverse_drag_selections_are_preserved() {
        for mut selection in [
            Some(Selection {
                start_col: 1,
                start_row: 0,
                end_col: 3,
                end_row: 0,
            }),
            Some(Selection {
                start_col: 3,
                start_row: 2,
                end_col: 1,
                end_row: 0,
            }),
        ] {
            let expected = selection;
            assert!(!clear_collapsed_selection(&mut selection));
            assert_eq!(selection, expected);
        }
    }

    #[test]
    fn cursor_blink_toggles_on_fixed_deadlines_and_resets_after_input() {
        let start = Instant::now();
        let mut blink = CursorBlinkState::new(start, true);

        assert!(blink.visible());
        assert_eq!(
            blink.next_deadline(true),
            Some(start + CURSOR_BLINK_INTERVAL)
        );
        assert!(!blink.update(
            start + CURSOR_BLINK_INTERVAL - Duration::from_millis(1),
            true
        ));
        assert!(blink.update(start + CURSOR_BLINK_INTERVAL, true));
        assert!(!blink.visible());

        let input_at = start + CURSOR_BLINK_INTERVAL + Duration::from_millis(20);
        assert!(blink.reset(input_at, true));
        assert!(blink.visible());
        assert_eq!(
            blink.next_deadline(true),
            Some(input_at + CURSOR_BLINK_INTERVAL)
        );
    }

    #[test]
    fn cursor_blink_catches_up_without_drifting() {
        let start = Instant::now();
        let mut blink = CursorBlinkState::new(start, true);
        let after_three_toggles = start + CURSOR_BLINK_INTERVAL * 3;

        assert!(blink.update(after_three_toggles, true));
        assert!(!blink.visible());
        assert_eq!(
            blink.next_deadline(true),
            Some(start + CURSOR_BLINK_INTERVAL * 4)
        );
    }

    #[test]
    fn cursor_blink_stops_when_unfocused_or_protocol_hidden() {
        let start = Instant::now();
        let mut blink = CursorBlinkState::new(start, true);
        assert!(blink.update(start + CURSOR_BLINK_INTERVAL, true));
        assert!(!blink.visible());

        assert!(blink.set_focused(false, start + Duration::from_secs(1), true));
        assert!(blink.visible());
        assert_eq!(blink.next_deadline(true), None);

        assert!(!blink.set_focused(true, start + Duration::from_secs(2), false));
        assert_eq!(blink.next_deadline(false), None);
        assert!(!blink.update(start + Duration::from_secs(3), false));
    }

    #[test]
    fn terminal_view_combines_protocol_and_blink_visibility() {
        let mut terminal = Terminal::new(2, 1);
        assert!(terminal.cursor_visible());

        assert!(terminal.set_cursor_blink_visible(false));
        assert!(!terminal.cursor_visible());
        assert!(terminal.set_cursor_blink_visible(true));
        terminal.cursor_visible = false;
        assert!(!terminal.cursor_visible());
        assert!(!terminal.set_cursor_blink_visible(false));
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
    fn frame_scheduler_coalesces_already_processed_heavy_pty_bursts() {
        let start = Instant::now();
        let frame_interval = Duration::from_millis(8);
        let mut scheduler = FrameScheduler::default();
        let heavy = RedrawWork {
            changed: true,
            heavy: true,
        };

        assert!(!scheduler.mark_io_processed(start, frame_interval, heavy));
        assert!(!scheduler.mark_io_processed(
            start + Duration::from_millis(2),
            frame_interval,
            heavy,
        ));

        let decision = scheduler.prepare_redraw(
            start + frame_interval + Duration::from_millis(2),
            RedrawWork::default,
        );
        assert!(decision.request_redraw);
        assert!(decision.wait_until.is_none());
        assert!(!decision.blocks_periodic_redraw());
    }

    #[test]
    fn deferred_heavy_frame_blocks_periodic_redraw_sources() {
        let start = Instant::now();
        let frame_interval = Duration::from_millis(8);
        let mut scheduler = FrameScheduler::default();

        assert!(!scheduler.mark_io_processed(
            start,
            frame_interval,
            RedrawWork {
                changed: true,
                heavy: true,
            },
        ));
        let decision = scheduler.prepare_redraw(start, RedrawWork::default);

        assert!(!decision.request_redraw);
        assert!(decision.blocks_periodic_redraw());
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
    fn scrollback_wheel_delta_uses_reduced_multiplier() {
        assert_eq!(scrollback_wheel_delta(1), 2);
        assert_eq!(scrollback_wheel_delta(3), 6);
    }

    #[test]
    fn viewport_scroll_maps_mouse_rows_with_fractional_offset() {
        let viewport = ViewportScroll::from_scroll_rows(0.25);
        assert_eq!(viewport.mouse_row_for_pixel_y(0.0, 16.0, 24), 0);
        assert_eq!(viewport.mouse_row_for_pixel_y(5.0, 16.0, 24), 1);
        assert_eq!(viewport.mouse_row_for_pixel_y(20.0, 16.0, 24), 2);
    }

    #[test]
    fn scrollbar_geometry_reflects_visible_fraction_and_progress() {
        let geometry = compute_scrollbar_geometry(100, 20, 50.0, 200.0, 24.0).unwrap();
        assert_eq!(geometry.thumb_h_px, 33.333336);
        assert_eq!(geometry.thumb_y_px, 83.33333);
    }

    #[test]
    fn scrollbar_geometry_respects_min_thumb_size() {
        let geometry = compute_scrollbar_geometry(500, 10, 250.0, 120.0, 24.0).unwrap();
        assert_eq!(geometry.thumb_h_px, 24.0);
    }

    #[test]
    fn smooth_scroll_state_eases_toward_target() {
        let start = Instant::now();
        let mut scroll = SmoothScrollState::default();
        scroll.apply_delta(4.0, true, 20.0);
        assert_eq!(scroll.target_rows, 4.0);
        assert_eq!(scroll.display_rows, 0.0);

        assert!(scroll.advance(start + Duration::from_millis(16), 20.0));
        assert!(scroll.display_rows > 0.0);
        assert!(scroll.display_rows < scroll.target_rows);
        assert!(scroll.is_animating());
        assert!(scroll.target_rows > 4.0);

        for step in 1..30 {
            scroll.advance(start + Duration::from_millis(16 * (step + 1)), 20.0);
        }

        assert!(scroll.display_rows > 4.0);
        assert!((scroll.display_rows - scroll.target_rows).abs() < 0.2);
        scroll.snap_to_target();
        assert_eq!(
            scroll.displayed_scroll_offset(),
            scroll.target_rows.ceil() as usize
        );
        assert!(!scroll.is_animating());
    }

    #[test]
    fn smooth_scroll_state_snaps_to_full_bottom_when_close() {
        let mut scroll = SmoothScrollState::default();
        scroll.apply_delta(1.0, true, 20.0);
        assert_eq!(scroll.target_rows, 1.0);

        scroll.apply_delta(0.8, false, 20.0);
        assert_eq!(scroll.target_rows, 0.0);
    }

    #[test]
    fn smooth_scroll_state_has_momentum_after_input() {
        let start = Instant::now();
        let mut scroll = SmoothScrollState::default();
        scroll.apply_delta(2.0, true, 50.0);
        let initial_target = scroll.target_rows;

        scroll.advance(start + Duration::from_millis(16), 50.0);
        let after_first_target = scroll.target_rows;
        assert!(after_first_target > initial_target);

        scroll.advance(start + Duration::from_millis(32), 50.0);
        assert!(scroll.target_rows > after_first_target);
    }

    #[test]
    fn smooth_scroll_state_clamps_and_stops_momentum_at_bottom() {
        let start = Instant::now();
        let mut scroll = SmoothScrollState::default();
        scroll.apply_delta(3.0, true, 50.0);
        scroll.advance(start + Duration::from_millis(16), 50.0);
        scroll.apply_delta(100.0, false, 50.0);

        for step in 1..10 {
            scroll.advance(start + Duration::from_millis(16 * (step + 1)), 50.0);
        }

        assert_eq!(scroll.target_rows, 0.0);
        assert_eq!(scroll.display_rows, 0.0);
        assert!(!scroll.is_animating());
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
        assert_eq!(
            parse_synthetic_key("capslock"),
            Key::Named(NamedKey::CapsLock)
        );
        assert_eq!(
            parse_synthetic_key("num_lock"),
            Key::Named(NamedKey::NumLock)
        );
        assert_eq!(parse_synthetic_key("clear"), Key::Named(NamedKey::Clear));
        assert_eq!(
            parse_synthetic_key("context_menu"),
            Key::Named(NamedKey::ContextMenu)
        );
        assert_eq!(parse_synthetic_key("x"), Key::Character("x".into()));
        assert_eq!(parse_synthetic_key("👨‍💻"), Key::Character("👨‍💻".into()));
    }

    #[test]
    fn parse_synthetic_physical_key_supports_keypad_menu_and_side_specific_modifiers() {
        assert_eq!(
            parse_synthetic_physical_key(Some("context_menu")),
            Some(PhysicalKey::Code(KeyCode::ContextMenu))
        );
        assert_eq!(
            parse_synthetic_physical_key(Some("numpad5")),
            Some(PhysicalKey::Code(KeyCode::Numpad5))
        );
        assert_eq!(
            parse_synthetic_physical_key(Some("shift_right")),
            Some(PhysicalKey::Code(KeyCode::ShiftRight))
        );
        assert_eq!(
            parse_synthetic_physical_key(Some("key_c")),
            Some(PhysicalKey::Code(KeyCode::KeyC))
        );
        assert_eq!(
            parse_synthetic_physical_key(Some("digit7")),
            Some(PhysicalKey::Code(KeyCode::Digit7))
        );
        assert_eq!(
            parse_synthetic_physical_key(Some("f12")),
            Some(PhysicalKey::Code(KeyCode::F12))
        );
        assert_eq!(
            parse_synthetic_physical_key(Some("page_down")),
            Some(PhysicalKey::Code(KeyCode::PageDown))
        );
        assert_eq!(
            parse_synthetic_physical_key(Some("backquote")),
            Some(PhysicalKey::Code(KeyCode::Backquote))
        );
        assert_eq!(parse_synthetic_physical_key(Some("unknown")), None);
        assert_eq!(parse_synthetic_physical_key(None), None);
    }

    #[test]
    fn synthetic_modifiers_state_sets_requested_bits() {
        let modifiers = synthetic_modifiers_state(SyntheticModifierState {
            ctrl: true,
            alt: false,
            shift: true,
            super_key: true,
            hyper: true,
            meta: true,
            caps_lock: true,
            num_lock: true,
        });
        assert!(modifiers.control_key());
        assert!(!modifiers.alt_key());
        assert!(modifiers.shift_key());
        assert!(modifiers.super_key());
        assert_ne!(modifiers.bits(), 0);
    }

    #[test]
    fn startup_timing_snapshot_reports_ready_milestones() {
        let started = Instant::now();
        let mut timing = StartupTiming::new(started);
        timing.mark_pty_spawned(started + Duration::from_millis(2));
        timing.mark_pty_event(started + Duration::from_millis(4));
        timing.mark_pty_read(started + Duration::from_millis(6), 12);
        timing.first_visible_output_at = Some(started + Duration::from_millis(9));
        timing.bytes_before_visible = Some(12);
        timing.mark_present(started + Duration::from_millis(11));
        timing.mark_present(started + Duration::from_millis(13));

        let snapshot = timing
            .snapshot_if_ready()
            .expect("startup snapshot should be available once ready");
        assert_eq!(snapshot.bytes_before_visible, Some(12));
        assert_eq!(snapshot.open_to_pty_spawn_ms, Some(2.0));
        assert_eq!(snapshot.open_to_first_pty_event_ms, Some(4.0));
        assert_eq!(snapshot.open_to_first_pty_read_ms, Some(6.0));
        assert_eq!(snapshot.open_to_first_visible_output_ms, Some(9.0));
        assert_eq!(snapshot.open_to_first_present_ms, Some(11.0));
        assert_eq!(snapshot.open_to_first_visible_present_ms, Some(11.0));
        assert_eq!(snapshot.first_read_to_visible_present_ms, Some(5.0));
        assert_eq!(snapshot.first_visible_to_present_ms, Some(2.0));
    }
}
