use crate::terminal::{
    KITTY_KBD_DISAMBIGUATE, KITTY_KBD_REPORT_ALL, KITTY_KBD_REPORT_ALTERNATE,
    KITTY_KBD_REPORT_EVENTS, KITTY_KBD_REPORT_TEXT,
};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};

const HYPER_MODIFIER_BIT: u32 = 1 << 24;
const META_MODIFIER_BIT: u32 = 1 << 25;
const CAPS_LOCK_MODIFIER_BIT: u32 = 1 << 26;
const NUM_LOCK_MODIFIER_BIT: u32 = 1 << 27;

const KITTY_LEGACY_LETTER_KEYS: &[(NamedKey, char, bool)] = &[
    (NamedKey::ArrowUp, 'A', true),
    (NamedKey::ArrowDown, 'B', true),
    (NamedKey::ArrowRight, 'C', true),
    (NamedKey::ArrowLeft, 'D', true),
    (NamedKey::Home, 'H', true),
    (NamedKey::End, 'F', true),
    (NamedKey::F1, 'P', false),
    (NamedKey::F2, 'Q', false),
    (NamedKey::F4, 'S', false),
];

const KITTY_LEGACY_TILDE_KEYS: &[(NamedKey, u16)] = &[
    (NamedKey::Insert, 2),
    (NamedKey::Delete, 3),
    (NamedKey::PageUp, 5),
    (NamedKey::PageDown, 6),
    (NamedKey::F3, 13),
    (NamedKey::F5, 15),
    (NamedKey::F6, 17),
    (NamedKey::F7, 18),
    (NamedKey::F8, 19),
    (NamedKey::F9, 20),
    (NamedKey::F10, 21),
    (NamedKey::F11, 23),
    (NamedKey::F12, 24),
    (NamedKey::ContextMenu, 29),
];

const KITTY_DIRECT_NAMED_KEY_CODES: &[(NamedKey, u32)] = &[
    (NamedKey::Escape, 27),
    (NamedKey::Enter, 13),
    (NamedKey::Tab, 9),
    (NamedKey::Backspace, 127),
    (NamedKey::Space, 32),
    (NamedKey::CapsLock, 57358),
    (NamedKey::ScrollLock, 57359),
    (NamedKey::NumLock, 57360),
    (NamedKey::PrintScreen, 57361),
    (NamedKey::Pause, 57362),
    (NamedKey::ContextMenu, 57363),
    (NamedKey::F13, 57376),
    (NamedKey::F14, 57377),
    (NamedKey::F15, 57378),
    (NamedKey::F16, 57379),
    (NamedKey::F17, 57380),
    (NamedKey::F18, 57381),
    (NamedKey::F19, 57382),
    (NamedKey::F20, 57383),
    (NamedKey::F21, 57384),
    (NamedKey::F22, 57385),
    (NamedKey::F23, 57386),
    (NamedKey::F24, 57387),
    (NamedKey::F25, 57388),
    (NamedKey::F26, 57389),
    (NamedKey::F27, 57390),
    (NamedKey::F28, 57391),
    (NamedKey::F29, 57392),
    (NamedKey::F30, 57393),
    (NamedKey::F31, 57394),
    (NamedKey::F32, 57395),
    (NamedKey::F33, 57396),
    (NamedKey::F34, 57397),
    (NamedKey::F35, 57398),
    (NamedKey::MediaPlay, 57428),
    (NamedKey::MediaPause, 57429),
    (NamedKey::MediaPlayPause, 57430),
    (NamedKey::MediaStop, 57432),
    (NamedKey::MediaFastForward, 57433),
    (NamedKey::MediaRewind, 57434),
    (NamedKey::MediaTrackNext, 57435),
    (NamedKey::MediaTrackPrevious, 57436),
    (NamedKey::MediaRecord, 57437),
    (NamedKey::AudioVolumeDown, 57438),
    (NamedKey::AudioVolumeUp, 57439),
    (NamedKey::AudioVolumeMute, 57440),
    (NamedKey::AltGraph, 57453),
];

const KITTY_KEYPAD_CODES: &[(KeyCode, u32, u32)] = &[
    (KeyCode::Numpad0, 57399, 57425),
    (KeyCode::Numpad1, 57400, 57424),
    (KeyCode::Numpad2, 57401, 57420),
    (KeyCode::Numpad3, 57402, 57422),
    (KeyCode::Numpad4, 57403, 57417),
    (KeyCode::Numpad5, 57404, 57427),
    (KeyCode::Numpad6, 57405, 57418),
    (KeyCode::Numpad7, 57406, 57423),
    (KeyCode::Numpad8, 57407, 57419),
    (KeyCode::Numpad9, 57408, 57421),
    (KeyCode::NumpadDecimal, 57409, 57426),
    (KeyCode::NumpadDivide, 57410, 57410),
    (KeyCode::NumpadMultiply, 57411, 57411),
    (KeyCode::NumpadSubtract, 57412, 57412),
    (KeyCode::NumpadAdd, 57413, 57413),
    (KeyCode::NumpadEnter, 57414, 57414),
    (KeyCode::NumpadEqual, 57415, 57415),
    (KeyCode::NumpadComma, 57416, 57416),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEventKind {
    Press,
    Repeat,
    Release,
}

pub fn modifiers_with_extra(
    modifiers: ModifiersState,
    hyper: bool,
    meta: bool,
    caps_lock: bool,
    num_lock: bool,
) -> ModifiersState {
    let mut bits = modifiers.bits();
    if hyper {
        bits |= HYPER_MODIFIER_BIT;
    }
    if meta {
        bits |= META_MODIFIER_BIT;
    }
    if caps_lock {
        bits |= CAPS_LOCK_MODIFIER_BIT;
    }
    if num_lock {
        bits |= NUM_LOCK_MODIFIER_BIT;
    }
    ModifiersState::from_bits_retain(bits)
}

pub fn effective_modifiers_for_key_event(
    modifiers: ModifiersState,
    hyper: bool,
    meta: bool,
    caps_lock: bool,
    num_lock: bool,
    key: &Key,
    event_kind: KeyEventKind,
) -> ModifiersState {
    let mut effective = modifiers_with_extra(modifiers, hyper, meta, caps_lock, num_lock);
    match (key, event_kind) {
        (Key::Named(NamedKey::Shift), KeyEventKind::Press | KeyEventKind::Repeat) => {
            effective.set(ModifiersState::SHIFT, true);
        }
        (Key::Named(NamedKey::Shift), KeyEventKind::Release) => {
            effective.set(ModifiersState::SHIFT, false);
        }
        (Key::Named(NamedKey::Control), KeyEventKind::Press | KeyEventKind::Repeat) => {
            effective.set(ModifiersState::CONTROL, true);
        }
        (Key::Named(NamedKey::Control), KeyEventKind::Release) => {
            effective.set(ModifiersState::CONTROL, false);
        }
        (Key::Named(NamedKey::Alt), KeyEventKind::Press | KeyEventKind::Repeat) => {
            effective.set(ModifiersState::ALT, true);
        }
        (Key::Named(NamedKey::Alt), KeyEventKind::Release) => {
            effective.set(ModifiersState::ALT, false);
        }
        (Key::Named(NamedKey::Super), KeyEventKind::Press | KeyEventKind::Repeat) => {
            effective.set(ModifiersState::SUPER, true);
        }
        (Key::Named(NamedKey::Super), KeyEventKind::Release) => {
            effective.set(ModifiersState::SUPER, false);
        }
        (Key::Named(NamedKey::Hyper), KeyEventKind::Press | KeyEventKind::Release) => {
            effective = modifiers_with_extra(modifiers, false, meta, caps_lock, num_lock);
        }
        (Key::Named(NamedKey::Meta), KeyEventKind::Press | KeyEventKind::Release) => {
            effective = modifiers_with_extra(modifiers, hyper, false, caps_lock, num_lock);
        }
        (Key::Named(NamedKey::CapsLock), KeyEventKind::Press) => {
            effective = modifiers_with_extra(modifiers, hyper, meta, !caps_lock, num_lock);
        }
        (Key::Named(NamedKey::CapsLock), KeyEventKind::Repeat | KeyEventKind::Release) => {
            effective = modifiers_with_extra(modifiers, hyper, meta, caps_lock, num_lock);
        }
        (Key::Named(NamedKey::NumLock), KeyEventKind::Press) => {
            effective = modifiers_with_extra(modifiers, hyper, meta, caps_lock, !num_lock);
        }
        (Key::Named(NamedKey::NumLock), KeyEventKind::Repeat | KeyEventKind::Release) => {
            effective = modifiers_with_extra(modifiers, hyper, meta, caps_lock, num_lock);
        }
        _ => {}
    }
    effective
}

pub fn apply_modifier_key_transition(
    hyper: &mut bool,
    meta: &mut bool,
    caps_lock: &mut bool,
    num_lock: &mut bool,
    key: &Key,
    event_kind: KeyEventKind,
) {
    let pressed = matches!(event_kind, KeyEventKind::Press | KeyEventKind::Repeat);
    match key {
        Key::Named(NamedKey::Hyper) => *hyper = pressed,
        Key::Named(NamedKey::Meta) => *meta = pressed,
        Key::Named(NamedKey::CapsLock) if matches!(event_kind, KeyEventKind::Press) => {
            *caps_lock = !*caps_lock;
        }
        Key::Named(NamedKey::NumLock) if matches!(event_kind, KeyEventKind::Press) => {
            *num_lock = !*num_lock;
        }
        _ => {}
    }
}

/// Append the decimal digits of `value` to `out` without allocating an
/// intermediate `String` (unlike `value.to_string()`).
fn write_u32_decimal(out: &mut Vec<u8>, value: u32) {
    // u32::MAX is 10 digits, which fits comfortably in this scratch buffer.
    let mut buf = [0u8; 10];
    let mut idx = buf.len();
    let mut v = value;
    loop {
        idx -= 1;
        buf[idx] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    out.extend_from_slice(&buf[idx..]);
}

fn write_u16_decimal(out: &mut Vec<u8>, value: u16) {
    write_u32_decimal(out, value as u32);
}

/// The kitty CSI-u modifier field, computed once and written without an
/// intermediate `String`.
#[derive(Clone, Copy)]
enum ModField {
    Absent,
    Value(u16),
    ValueEvent(u16, u8),
}

impl ModField {
    fn is_present(self) -> bool {
        !matches!(self, ModField::Absent)
    }

    fn write(self, out: &mut Vec<u8>) {
        match self {
            ModField::Absent => {}
            ModField::Value(value) => write_u16_decimal(out, value),
            ModField::ValueEvent(value, event) => {
                write_u16_decimal(out, value);
                out.push(b':');
                out.push(b'0' + event);
            }
        }
    }
}

pub fn key_to_bytes(
    key: &Key,
    text: Option<&str>,
    physical_key: Option<&PhysicalKey>,
    app_cursor: bool,
    modifiers: ModifiersState,
    kitty_flags: u8,
    event_kind: KeyEventKind,
) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    if key_to_bytes_into(
        &mut out,
        key,
        text,
        physical_key,
        app_cursor,
        modifiers,
        kitty_flags,
        event_kind,
    ) {
        Some(out)
    } else {
        None
    }
}

/// Encode a key event into `out`, reusing its existing capacity.
///
/// `out` is cleared first, then any encoded bytes are appended. Returns `true`
/// when bytes were produced and `false` otherwise (leaving `out` empty). A
/// caller that encodes a stream of key events can reuse a single buffer so that
/// steady-state key encoding performs no heap allocation at all, whereas
/// [`key_to_bytes`] allocates a fresh `Vec` per call for callers that want an
/// owned result.
#[allow(clippy::too_many_arguments)]
pub fn key_to_bytes_into(
    out: &mut Vec<u8>,
    key: &Key,
    text: Option<&str>,
    physical_key: Option<&PhysicalKey>,
    app_cursor: bool,
    modifiers: ModifiersState,
    kitty_flags: u8,
    event_kind: KeyEventKind,
) -> bool {
    out.clear();

    // Match the familiar macOS text-editing shortcut while preserving the
    // terminal convention used by shells and line editors: Ctrl-W deletes the
    // previous word. Treat this as a local shortcut even when the application
    // has enabled the kitty keyboard protocol.
    if modifiers.super_key() && matches!(key, Key::Named(NamedKey::Backspace)) {
        if matches!(event_kind, KeyEventKind::Release) {
            return false;
        }
        out.push(0x17);
        return true;
    }

    if kitty_flags != 0
        && encode_kitty_key_into(
            out,
            key,
            text,
            physical_key,
            app_cursor,
            modifiers,
            kitty_flags,
            event_kind,
        )
    {
        return true;
    }

    // The kitty path may bail out after writing nothing meaningful; make sure
    // the legacy path starts from an empty buffer.
    out.clear();

    if matches!(event_kind, KeyEventKind::Release) {
        return false;
    }

    legacy_key_to_bytes_into(out, key, app_cursor, modifiers)
}

fn legacy_key_to_bytes_into(
    out: &mut Vec<u8>,
    key: &Key,
    app_cursor: bool,
    modifiers: ModifiersState,
) -> bool {
    let ctrl = modifiers.control_key();
    let alt = modifiers.alt_key();

    if ctrl && let Key::Character(s) = key {
        let Some(ch) = s.chars().next() else {
            return false;
        };
        if alt {
            out.push(0x1b);
        }
        if ch.is_ascii_alphabetic() {
            out.push(ch.to_ascii_lowercase() as u8 - b'a' + 1);
            return true;
        }
        let byte = match ch {
            '@' | ' ' | '`' => 0,
            '2' => 0,
            '3' => 0x1b,
            '4' => 0x1c,
            '5' => 0x1d,
            '6' => 0x1e,
            '7' => 0x1f,
            '8' | '?' => 0x7f,
            '9' => b'9',
            '0' => b'0',
            '[' | '\x1b' => 0x1b,
            '\\' => 0x1c,
            ']' => 0x1d,
            '^' | '~' => 0x1e,
            '_' | '/' => 0x1f,
            _ => {
                out.clear();
                return false;
            }
        };
        out.push(byte);
        return true;
    }

    match key {
        Key::Character(s) => {
            if alt {
                out.push(0x1b);
            }
            out.extend_from_slice(s.as_bytes());
            true
        }
        Key::Named(named) => legacy_named_key_to_bytes_into(out, *named, app_cursor, modifiers),
        _ => false,
    }
}

fn lookup_named_mapping<T: Copy>(named: NamedKey, mappings: &[(NamedKey, T)]) -> Option<T> {
    mappings
        .iter()
        .find_map(|(candidate, value)| (*candidate == named).then_some(*value))
}

fn kitty_legacy_letter_key(named: NamedKey) -> Option<(char, bool)> {
    KITTY_LEGACY_LETTER_KEYS
        .iter()
        .find_map(|(candidate, suffix, use_app_cursor)| {
            (*candidate == named).then_some((*suffix, *use_app_cursor))
        })
}

fn write_kitty_legacy_letter_sequence(
    out: &mut Vec<u8>,
    prefix_o: bool,
    suffix: u8,
    mods: ModField,
) {
    if mods.is_present() {
        out.extend_from_slice(b"\x1b[1;");
        mods.write(out);
        out.push(suffix);
    } else if prefix_o {
        out.extend_from_slice(b"\x1bO");
        out.push(suffix);
    } else {
        out.extend_from_slice(b"\x1b[");
        out.push(suffix);
    }
}

fn write_kitty_legacy_tilde_sequence(out: &mut Vec<u8>, number: u16, mods: ModField) {
    out.extend_from_slice(b"\x1b[");
    write_u16_decimal(out, number);
    if mods.is_present() {
        out.push(b';');
        mods.write(out);
    }
    out.push(b'~');
}

fn legacy_named_key_to_bytes_into(
    out: &mut Vec<u8>,
    named: NamedKey,
    app_cursor: bool,
    modifiers: ModifiersState,
) -> bool {
    let shift = modifiers.shift_key();
    let alt = modifiers.alt_key();
    let ctrl = modifiers.control_key();

    match named {
        NamedKey::Enter => {
            if alt {
                out.push(0x1b);
            }
            out.push(b'\r');
            true
        }
        NamedKey::Escape => {
            out.push(0x1b);
            if alt {
                out.push(0x1b);
            }
            true
        }
        NamedKey::Backspace => {
            if alt {
                out.push(0x1b);
            }
            out.push(if ctrl { 0x08 } else { 0x7f });
            true
        }
        NamedKey::Tab => {
            if alt {
                out.push(0x1b);
            }
            if shift {
                out.extend_from_slice(b"\x1b[Z");
            } else {
                out.push(b'\t');
            }
            true
        }
        NamedKey::Space => {
            if alt {
                out.push(0x1b);
            }
            out.push(if ctrl { 0x00 } else { b' ' });
            true
        }
        NamedKey::ArrowUp => legacy_named_sequence_into(out, app_cursor, modifiers, b'A', 0, 0),
        NamedKey::ArrowDown => legacy_named_sequence_into(out, app_cursor, modifiers, b'B', 0, 0),
        NamedKey::ArrowRight => legacy_named_sequence_into(out, app_cursor, modifiers, b'C', 0, 0),
        NamedKey::ArrowLeft => legacy_named_sequence_into(out, app_cursor, modifiers, b'D', 0, 0),
        NamedKey::Home => legacy_named_sequence_into(out, app_cursor, modifiers, b'H', 0, 0),
        NamedKey::End => legacy_named_sequence_into(out, app_cursor, modifiers, b'F', 0, 0),
        NamedKey::Insert => legacy_named_sequence_into(out, false, modifiers, 0, 2, b'~'),
        NamedKey::Delete => legacy_named_sequence_into(out, false, modifiers, 0, 3, b'~'),
        NamedKey::PageUp => legacy_named_sequence_into(out, false, modifiers, 0, 5, b'~'),
        NamedKey::PageDown => legacy_named_sequence_into(out, false, modifiers, 0, 6, b'~'),
        NamedKey::F1 => legacy_named_sequence_into(out, false, modifiers, b'P', 0, 0),
        NamedKey::F2 => legacy_named_sequence_into(out, false, modifiers, b'Q', 0, 0),
        NamedKey::F3 => legacy_named_sequence_into(out, false, modifiers, 0, 13, b'~'),
        NamedKey::F4 => legacy_named_sequence_into(out, false, modifiers, b'S', 0, 0),
        NamedKey::F5 => legacy_named_sequence_into(out, false, modifiers, 0, 15, b'~'),
        NamedKey::F6 => legacy_named_sequence_into(out, false, modifiers, 0, 17, b'~'),
        NamedKey::F7 => legacy_named_sequence_into(out, false, modifiers, 0, 18, b'~'),
        NamedKey::F8 => legacy_named_sequence_into(out, false, modifiers, 0, 19, b'~'),
        NamedKey::F9 => legacy_named_sequence_into(out, false, modifiers, 0, 20, b'~'),
        NamedKey::F10 => legacy_named_sequence_into(out, false, modifiers, 0, 21, b'~'),
        NamedKey::F11 => legacy_named_sequence_into(out, false, modifiers, 0, 23, b'~'),
        NamedKey::F12 => legacy_named_sequence_into(out, false, modifiers, 0, 24, b'~'),
        _ => false,
    }
}

fn legacy_named_sequence_into(
    out: &mut Vec<u8>,
    app_cursor: bool,
    modifiers: ModifiersState,
    letter_suffix: u8,
    tilde_number: u16,
    trailing: u8,
) -> bool {
    if modifiers.super_key() {
        return false;
    }

    let mut value = 1u16;
    if modifiers.shift_key() {
        value += 1;
    }
    if modifiers.alt_key() {
        value += 2;
    }
    if modifiers.control_key() {
        value += 4;
    }

    if letter_suffix != 0 {
        if value == 1 {
            if app_cursor {
                out.extend_from_slice(&[0x1b, b'O', letter_suffix]);
            } else {
                out.extend_from_slice(&[0x1b, b'[', letter_suffix]);
            }
            return true;
        }
        out.extend_from_slice(b"\x1b[1;");
        write_u16_decimal(out, value);
        out.push(letter_suffix);
        return true;
    }

    out.extend_from_slice(b"\x1b[");
    write_u16_decimal(out, tilde_number);
    if value != 1 {
        out.push(b';');
        write_u16_decimal(out, value);
    }
    out.push(trailing);
    true
}

#[allow(clippy::too_many_arguments)]
fn encode_kitty_key_into(
    out: &mut Vec<u8>,
    key: &Key,
    text: Option<&str>,
    physical_key: Option<&PhysicalKey>,
    app_cursor: bool,
    modifiers: ModifiersState,
    kitty_flags: u8,
    event_kind: KeyEventKind,
) -> bool {
    let report_all = kitty_flags & KITTY_KBD_REPORT_ALL != 0;
    let report_events = kitty_flags & KITTY_KBD_REPORT_EVENTS != 0;
    let report_alternate = kitty_flags & KITTY_KBD_REPORT_ALTERNATE != 0;
    let report_text = report_all && (kitty_flags & KITTY_KBD_REPORT_TEXT != 0);
    let disambiguate = kitty_flags & KITTY_KBD_DISAMBIGUATE != 0;
    let produces_text = text.filter(|s| !s.is_empty()).is_some();

    if !report_events && matches!(event_kind, KeyEventKind::Release) {
        return false;
    }

    if let Some(code) = kitty_keypad_code(physical_key, produces_text)
        && (report_all || (disambiguate && !produces_text))
    {
        let mods = kitty_modifier_modfield(modifiers, report_events, event_kind);
        let text_field = if report_text { text } else { None };
        if !produces_text && matches!(physical_key, Some(PhysicalKey::Code(KeyCode::Numpad5))) {
            write_kitty_legacy_letter_sequence(out, false, b'E', mods);
            return true;
        }
        out.extend_from_slice(b"\x1b[");
        write_u32_decimal(out, code);
        finish_kitty_csi_u(out, mods, text_field);
        return true;
    }

    if encode_kitty_named_key_into(
        out,
        key,
        physical_key,
        app_cursor,
        modifiers,
        report_all,
        report_events,
        event_kind,
    ) {
        return true;
    }

    if !produces_text {
        return false;
    }
    if !report_all
        && matches!(event_kind, KeyEventKind::Repeat | KeyEventKind::Release)
        && !(report_events && (disambiguate || text_key_is_ambiguous(modifiers)))
    {
        return false;
    }
    if !report_all && !disambiguate && matches!(event_kind, KeyEventKind::Press) {
        return false;
    }
    if !report_all && !text_key_is_ambiguous(modifiers) {
        return false;
    }

    let Some(primary) = text_key_primary_codepoint(key, physical_key, modifiers) else {
        return false;
    };
    let mods = kitty_modifier_modfield(modifiers, report_events, event_kind);
    let text_field = if report_text { text } else { None };
    out.extend_from_slice(b"\x1b[");
    write_kitty_first_param(
        out,
        primary,
        text,
        physical_key,
        modifiers,
        report_alternate,
    );
    finish_kitty_csi_u(out, mods, text_field);
    true
}

fn text_key_is_ambiguous(modifiers: ModifiersState) -> bool {
    // Shift alone is NOT ambiguous: a text key pressed with only shift
    // produces plain shifted text (e.g. shift+/ types `?`), and kitty's
    // disambiguate flag only rewrites combinations whose legacy encodings
    // collide with control codes (esc, alt+key, ctrl+key, etc.). Encoding
    // shift-only keys as CSI u made applications see the unshifted base key
    // (`/` instead of `?`).
    modifiers.alt_key()
        || modifiers.control_key()
        || modifiers.super_key()
        || hyper_modifier_active(modifiers)
        || meta_modifier_active(modifiers)
}

fn text_key_primary_codepoint(
    key: &Key,
    physical_key: Option<&PhysicalKey>,
    modifiers: ModifiersState,
) -> Option<u32> {
    match key {
        Key::Character(s) => {
            let ch = s.chars().next()?;
            Some(if ch.is_alphabetic() {
                ch.to_lowercase().next().unwrap_or(ch) as u32
            } else if modifiers.shift_key() {
                physical_key
                    .and_then(base_layout_codepoint)
                    .unwrap_or(ch as u32)
            } else {
                ch as u32
            })
        }
        _ => physical_key.and_then(base_layout_codepoint),
    }
}

fn write_kitty_first_param(
    out: &mut Vec<u8>,
    primary: u32,
    text: Option<&str>,
    physical_key: Option<&PhysicalKey>,
    modifiers: ModifiersState,
    report_alternate: bool,
) {
    if !report_alternate {
        write_u32_decimal(out, primary);
        return;
    }

    let shifted = if modifiers.shift_key() {
        text.and_then(|s| s.chars().next()).map(|ch| ch as u32)
    } else {
        None
    }
    .filter(|cp| *cp != primary);
    let base_layout = physical_key
        .and_then(base_layout_codepoint)
        .filter(|cp| *cp != primary);

    write_u32_decimal(out, primary);
    match (shifted, base_layout) {
        (Some(shifted), Some(base_layout)) => {
            out.push(b':');
            write_u32_decimal(out, shifted);
            out.push(b':');
            write_u32_decimal(out, base_layout);
        }
        (Some(shifted), None) => {
            out.push(b':');
            write_u32_decimal(out, shifted);
        }
        (None, Some(base_layout)) => {
            out.extend_from_slice(b"::");
            write_u32_decimal(out, base_layout);
        }
        (None, None) => {}
    }
}

fn kitty_modifier_modfield(
    modifiers: ModifiersState,
    report_events: bool,
    event_kind: KeyEventKind,
) -> ModField {
    let mut value = 1u16;
    if modifiers.shift_key() {
        value += 1;
    }
    if modifiers.alt_key() {
        value += 2;
    }
    if modifiers.control_key() {
        value += 4;
    }
    if modifiers.super_key() {
        value += 8;
    }
    if hyper_modifier_active(modifiers) {
        value += 16;
    }
    if meta_modifier_active(modifiers) {
        value += 32;
    }
    if caps_lock_modifier_active(modifiers) {
        value += 64;
    }
    if num_lock_modifier_active(modifiers) {
        value += 128;
    }

    if report_events {
        let event = match event_kind {
            KeyEventKind::Press => 1,
            KeyEventKind::Repeat => 2,
            KeyEventKind::Release => 3,
        };
        if value == 1 && event == 1 {
            ModField::Absent
        } else {
            ModField::ValueEvent(value, event)
        }
    } else if value == 1 {
        ModField::Absent
    } else {
        ModField::Value(value)
    }
}

fn hyper_modifier_active(modifiers: ModifiersState) -> bool {
    modifiers.bits() & HYPER_MODIFIER_BIT != 0
}

fn meta_modifier_active(modifiers: ModifiersState) -> bool {
    modifiers.bits() & META_MODIFIER_BIT != 0
}

fn caps_lock_modifier_active(modifiers: ModifiersState) -> bool {
    modifiers.bits() & CAPS_LOCK_MODIFIER_BIT != 0
}

fn num_lock_modifier_active(modifiers: ModifiersState) -> bool {
    modifiers.bits() & NUM_LOCK_MODIFIER_BIT != 0
}

/// Does the optional kitty text field carry at least one reportable
/// (non-control) codepoint?
fn kitty_text_present(text: Option<&str>) -> bool {
    text.is_some_and(|text| text.chars().any(|ch| !ch.is_control()))
}

fn write_kitty_text_codepoints(out: &mut Vec<u8>, text: &str) {
    let mut first = true;
    for ch in text.chars().filter(|ch| !ch.is_control()) {
        if !first {
            out.push(b':');
        }
        first = false;
        write_u32_decimal(out, ch as u32);
    }
}

/// Append the modifier/text fields and the trailing `u` to a CSI-u sequence
/// whose leading parameter has already been written.
fn finish_kitty_csi_u(out: &mut Vec<u8>, mods: ModField, text_field: Option<&str>) {
    let text_present = kitty_text_present(text_field);
    if mods.is_present() || text_present {
        out.push(b';');
        mods.write(out);
    }
    if text_present {
        out.push(b';');
        if let Some(text) = text_field {
            write_kitty_text_codepoints(out, text);
        }
    }
    out.push(b'u');
}

#[allow(clippy::too_many_arguments)]
fn encode_kitty_named_key_into(
    out: &mut Vec<u8>,
    key: &Key,
    physical_key: Option<&PhysicalKey>,
    app_cursor: bool,
    modifiers: ModifiersState,
    report_all: bool,
    report_events: bool,
    event_kind: KeyEventKind,
) -> bool {
    if matches!(key, Key::Named(NamedKey::Backspace)) && modifiers.control_key() {
        return false;
    }

    let Key::Named(named) = key else {
        return false;
    };
    let named = *named;

    match named {
        NamedKey::Enter | NamedKey::Tab | NamedKey::Backspace if !report_all => return false,
        NamedKey::Space
            if !report_all
                && !modifiers.shift_key()
                && !modifiers.alt_key()
                && !modifiers.control_key()
                && !modifiers.super_key() =>
        {
            return false;
        }
        _ => {}
    }

    let mods = kitty_modifier_modfield(modifiers, report_events, event_kind);

    if let Some((suffix, use_app_cursor)) = kitty_legacy_letter_key(named) {
        write_kitty_legacy_letter_sequence(out, app_cursor && use_app_cursor, suffix as u8, mods);
        return true;
    }
    if let Some(number) = lookup_named_mapping(named, KITTY_LEGACY_TILDE_KEYS) {
        write_kitty_legacy_tilde_sequence(out, number, mods);
        return true;
    }
    let Some(code) = kitty_named_key_code(physical_key, named) else {
        return false;
    };
    out.extend_from_slice(b"\x1b[");
    write_u32_decimal(out, code);
    finish_kitty_csi_u(out, mods, None);
    true
}

fn kitty_named_key_code(physical_key: Option<&PhysicalKey>, named: NamedKey) -> Option<u32> {
    if let Some(code) = lookup_named_mapping(named, KITTY_DIRECT_NAMED_KEY_CODES) {
        return Some(code);
    }

    match named {
        NamedKey::Shift
        | NamedKey::Control
        | NamedKey::Alt
        | NamedKey::Super
        | NamedKey::Meta
        | NamedKey::Hyper => modifier_named_key_code(physical_key, &named),
        _ => None,
    }
}

fn modifier_named_key_code(physical_key: Option<&PhysicalKey>, named: &NamedKey) -> Option<u32> {
    match physical_key {
        Some(PhysicalKey::Code(KeyCode::ShiftLeft)) => Some(57441),
        Some(PhysicalKey::Code(KeyCode::ControlLeft)) => Some(57442),
        Some(PhysicalKey::Code(KeyCode::AltLeft)) => Some(57443),
        Some(PhysicalKey::Code(KeyCode::SuperLeft)) => Some(57444),
        Some(PhysicalKey::Code(KeyCode::Hyper)) => Some(57445),
        Some(PhysicalKey::Code(KeyCode::Meta)) => Some(57446),
        Some(PhysicalKey::Code(KeyCode::ShiftRight)) => Some(57447),
        Some(PhysicalKey::Code(KeyCode::ControlRight)) => Some(57448),
        Some(PhysicalKey::Code(KeyCode::AltRight)) => Some(57449),
        Some(PhysicalKey::Code(KeyCode::SuperRight)) => Some(57450),
        _ => match named {
            NamedKey::Shift => Some(57441),
            NamedKey::Control => Some(57442),
            NamedKey::Alt => Some(57443),
            NamedKey::Super => Some(57444),
            NamedKey::Hyper => Some(57445),
            NamedKey::Meta => Some(57446),
            _ => None,
        },
    }
}

fn kitty_keypad_code(physical_key: Option<&PhysicalKey>, produces_text: bool) -> Option<u32> {
    match physical_key {
        Some(PhysicalKey::Code(code)) => {
            KITTY_KEYPAD_CODES
                .iter()
                .find_map(|(candidate, text_code, navigation_code)| {
                    (*candidate == *code).then_some(if produces_text {
                        *text_code
                    } else {
                        *navigation_code
                    })
                })
        }
        _ => None,
    }
}

fn base_layout_codepoint(physical_key: &PhysicalKey) -> Option<u32> {
    match physical_key {
        PhysicalKey::Code(code) => match code {
            KeyCode::KeyA => Some('a' as u32),
            KeyCode::KeyB => Some('b' as u32),
            KeyCode::KeyC => Some('c' as u32),
            KeyCode::KeyD => Some('d' as u32),
            KeyCode::KeyE => Some('e' as u32),
            KeyCode::KeyF => Some('f' as u32),
            KeyCode::KeyG => Some('g' as u32),
            KeyCode::KeyH => Some('h' as u32),
            KeyCode::KeyI => Some('i' as u32),
            KeyCode::KeyJ => Some('j' as u32),
            KeyCode::KeyK => Some('k' as u32),
            KeyCode::KeyL => Some('l' as u32),
            KeyCode::KeyM => Some('m' as u32),
            KeyCode::KeyN => Some('n' as u32),
            KeyCode::KeyO => Some('o' as u32),
            KeyCode::KeyP => Some('p' as u32),
            KeyCode::KeyQ => Some('q' as u32),
            KeyCode::KeyR => Some('r' as u32),
            KeyCode::KeyS => Some('s' as u32),
            KeyCode::KeyT => Some('t' as u32),
            KeyCode::KeyU => Some('u' as u32),
            KeyCode::KeyV => Some('v' as u32),
            KeyCode::KeyW => Some('w' as u32),
            KeyCode::KeyX => Some('x' as u32),
            KeyCode::KeyY => Some('y' as u32),
            KeyCode::KeyZ => Some('z' as u32),
            KeyCode::Digit0 => Some('0' as u32),
            KeyCode::Digit1 => Some('1' as u32),
            KeyCode::Digit2 => Some('2' as u32),
            KeyCode::Digit3 => Some('3' as u32),
            KeyCode::Digit4 => Some('4' as u32),
            KeyCode::Digit5 => Some('5' as u32),
            KeyCode::Digit6 => Some('6' as u32),
            KeyCode::Digit7 => Some('7' as u32),
            KeyCode::Digit8 => Some('8' as u32),
            KeyCode::Digit9 => Some('9' as u32),
            KeyCode::Backquote => Some('`' as u32),
            KeyCode::Minus => Some('-' as u32),
            KeyCode::Equal => Some('=' as u32),
            KeyCode::BracketLeft => Some('[' as u32),
            KeyCode::BracketRight => Some(']' as u32),
            KeyCode::Backslash => Some('\\' as u32),
            KeyCode::Semicolon => Some(';' as u32),
            KeyCode::Quote => Some('\'' as u32),
            KeyCode::Comma => Some(',' as u32),
            KeyCode::Period => Some('.' as u32),
            KeyCode::Slash => Some('/' as u32),
            KeyCode::Space => Some(' ' as u32),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// A representative spread of key events covering the hot ASCII path, the
    /// legacy named-key path, and several kitty-protocol encoding paths. Used by
    /// the buffer-reuse parity, allocation, and timing tests below.
    // Test-only helper: the tuple mirrors `key_event_bytes_into` parameters
    // positionally, which reads better at the call sites than a named struct.
    #[allow(clippy::type_complexity)]
    fn sample_events() -> Vec<(
        Key,
        Option<&'static str>,
        Option<PhysicalKey>,
        bool,
        ModifiersState,
        u8,
        KeyEventKind,
    )> {
        vec![
            // Plain ASCII keypress (the dominant case).
            (
                Key::Character("a".into()),
                Some("a"),
                Some(PhysicalKey::Code(KeyCode::KeyA)),
                false,
                ModifiersState::default(),
                0,
                KeyEventKind::Press,
            ),
            // Ctrl-modified ASCII.
            (
                Key::Character("c".into()),
                Some("c"),
                Some(PhysicalKey::Code(KeyCode::KeyC)),
                false,
                ModifiersState::CONTROL,
                0,
                KeyEventKind::Press,
            ),
            // Alt-modified ASCII.
            (
                Key::Character("x".into()),
                Some("x"),
                Some(PhysicalKey::Code(KeyCode::KeyX)),
                false,
                ModifiersState::ALT,
                0,
                KeyEventKind::Press,
            ),
            // Legacy arrow key with modifiers.
            (
                Key::Named(NamedKey::ArrowUp),
                None,
                None,
                false,
                ModifiersState::SHIFT | ModifiersState::ALT,
                0,
                KeyEventKind::Press,
            ),
            // Legacy tilde key.
            (
                Key::Named(NamedKey::PageDown),
                None,
                None,
                false,
                ModifiersState::CONTROL,
                0,
                KeyEventKind::Press,
            ),
            // Kitty disambiguated modified text key.
            (
                Key::Character("Z".into()),
                Some("Z"),
                Some(PhysicalKey::Code(KeyCode::KeyZ)),
                false,
                ModifiersState::SHIFT | ModifiersState::CONTROL,
                KITTY_KBD_DISAMBIGUATE,
                KeyEventKind::Press,
            ),
            // Kitty report-all text key with text codepoints.
            (
                Key::Character("A".into()),
                Some("A"),
                Some(PhysicalKey::Code(KeyCode::KeyA)),
                false,
                ModifiersState::SHIFT,
                KITTY_KBD_REPORT_ALL | KITTY_KBD_REPORT_TEXT,
                KeyEventKind::Press,
            ),
            // Kitty named key (legacy letter form).
            (
                Key::Named(NamedKey::ArrowUp),
                None,
                None,
                true,
                ModifiersState::default(),
                KITTY_KBD_REPORT_ALL,
                KeyEventKind::Press,
            ),
            // Kitty release event.
            (
                Key::Named(NamedKey::ArrowUp),
                None,
                None,
                false,
                ModifiersState::default(),
                KITTY_KBD_REPORT_EVENTS,
                KeyEventKind::Release,
            ),
        ]
    }

    /// The buffer-reuse `key_to_bytes_into` must produce byte-for-byte the same
    /// output as the allocating `key_to_bytes`, including the `None`/`false`
    /// distinction for events that emit nothing.
    #[test]
    fn key_to_bytes_into_matches_allocating_variant() {
        let mut scratch = Vec::new();
        for (key, text, physical, app_cursor, modifiers, flags, kind) in sample_events() {
            let expected = key_to_bytes(
                &key,
                text,
                physical.as_ref(),
                app_cursor,
                modifiers,
                flags,
                kind,
            );
            let produced = key_to_bytes_into(
                &mut scratch,
                &key,
                text,
                physical.as_ref(),
                app_cursor,
                modifiers,
                flags,
                kind,
            );
            match expected {
                Some(bytes) => {
                    assert!(produced, "into-variant should emit for {key:?}");
                    assert_eq!(scratch, bytes, "byte mismatch for {key:?}");
                }
                None => {
                    assert!(!produced, "into-variant should not emit for {key:?}");
                    assert!(scratch.is_empty(), "scratch must be empty for {key:?}");
                }
            }
        }
    }

    /// Encoding a long stream of key events through one reused buffer must reach
    /// a steady state where no further heap allocation occurs. We prove this by
    /// asserting the buffer capacity stops growing after a short warmup: a
    /// `Vec` only reallocates when it must grow, so a stable capacity over
    /// thousands of encodes means zero per-event allocation.
    #[test]
    fn key_to_bytes_into_is_allocation_free_in_steady_state() {
        let events = sample_events();
        let mut scratch = Vec::new();

        // Warm up so the buffer reaches the capacity needed by the widest
        // sequence in the sample set.
        for _ in 0..64 {
            for (key, text, physical, app_cursor, modifiers, flags, kind) in &events {
                key_to_bytes_into(
                    &mut scratch,
                    key,
                    *text,
                    physical.as_ref(),
                    *app_cursor,
                    *modifiers,
                    *flags,
                    *kind,
                );
            }
        }

        let stable_capacity = scratch.capacity();
        for _ in 0..10_000 {
            for (key, text, physical, app_cursor, modifiers, flags, kind) in &events {
                key_to_bytes_into(
                    &mut scratch,
                    key,
                    *text,
                    physical.as_ref(),
                    *app_cursor,
                    *modifiers,
                    *flags,
                    *kind,
                );
                assert_eq!(
                    scratch.capacity(),
                    stable_capacity,
                    "buffer reallocated mid-stream, encoding is not allocation-free"
                );
            }
        }
    }

    /// Microbenchmark guard: encoding via the reused buffer must be fast. This
    /// is not a hard wall-clock SLA (CI machines vary), but it documents the
    /// measured per-event cost and fails if a future change makes the hot path
    /// pathologically slow. The reused-buffer path also avoids the per-call
    /// `Vec` allocation that `key_to_bytes` pays.
    #[test]
    fn key_to_bytes_into_hot_path_is_fast() {
        let events = sample_events();
        let iters = 200_000usize;
        let mut scratch = Vec::new();
        let mut sink = 0u64;

        // Warm up.
        for _ in 0..1000 {
            for (key, text, physical, app_cursor, modifiers, flags, kind) in &events {
                if key_to_bytes_into(
                    &mut scratch,
                    key,
                    *text,
                    physical.as_ref(),
                    *app_cursor,
                    *modifiers,
                    *flags,
                    *kind,
                ) {
                    sink = sink.wrapping_add(scratch.len() as u64);
                }
            }
        }

        let start = Instant::now();
        for _ in 0..iters {
            for (key, text, physical, app_cursor, modifiers, flags, kind) in &events {
                if key_to_bytes_into(
                    &mut scratch,
                    key,
                    *text,
                    physical.as_ref(),
                    *app_cursor,
                    *modifiers,
                    *flags,
                    *kind,
                ) {
                    sink = sink.wrapping_add(scratch.len() as u64);
                }
            }
        }
        let elapsed = start.elapsed();
        let total_events = iters * events.len();
        let per_event_ns = elapsed.as_nanos() as f64 / total_events as f64;
        // Keep the optimizer honest about `sink`.
        assert!(sink > 0);
        // Generous ceiling: the hot path encodes well under ~500ns/event even
        // on a slow CI box; in practice it is a few tens of ns.
        assert!(
            per_event_ns < 500.0,
            "input encode hot path too slow: {per_event_ns:.1} ns/event"
        );
    }

    #[test]
    fn legacy_alt_prefix_is_preserved() {
        let modifiers = ModifiersState::ALT;
        let bytes = key_to_bytes(
            &Key::Character("x".into()),
            Some("x"),
            None,
            false,
            modifiers,
            0,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(bytes, b"\x1bx");
    }

    #[test]
    fn cmd_backspace_sends_delete_previous_word() {
        let bytes = key_to_bytes(
            &Key::Named(NamedKey::Backspace),
            None,
            None,
            false,
            ModifiersState::SUPER,
            0,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(bytes, b"\x17");
    }

    #[test]
    fn cmd_backspace_overrides_kitty_keyboard_reporting() {
        let bytes = key_to_bytes(
            &Key::Named(NamedKey::Backspace),
            None,
            None,
            false,
            ModifiersState::SUPER,
            KITTY_KBD_REPORT_ALL | KITTY_KBD_REPORT_EVENTS,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(bytes, b"\x17");

        let release = key_to_bytes(
            &Key::Named(NamedKey::Backspace),
            None,
            None,
            false,
            ModifiersState::SUPER,
            KITTY_KBD_REPORT_ALL | KITTY_KBD_REPORT_EVENTS,
            KeyEventKind::Release,
        );
        assert_eq!(release, None);
    }

    #[test]
    fn legacy_function_keys_encode_modifier_parameters() {
        let bytes = key_to_bytes(
            &Key::Named(NamedKey::ArrowUp),
            None,
            None,
            false,
            ModifiersState::SHIFT | ModifiersState::ALT,
            0,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(bytes, b"\x1b[1;4A");
    }

    #[test]
    fn kitty_report_events_does_not_emit_backspace_release_without_report_all() {
        let bytes = key_to_bytes(
            &Key::Named(NamedKey::Backspace),
            None,
            None,
            false,
            ModifiersState::default(),
            KITTY_KBD_REPORT_EVENTS,
            KeyEventKind::Release,
        );
        assert_eq!(bytes, None);
    }

    #[test]
    fn kitty_report_events_does_not_emit_plain_space_release_without_report_all() {
        let bytes = key_to_bytes(
            &Key::Named(NamedKey::Space),
            Some(" "),
            Some(&PhysicalKey::Code(KeyCode::Space)),
            false,
            ModifiersState::default(),
            KITTY_KBD_REPORT_EVENTS,
            KeyEventKind::Release,
        );
        assert_eq!(bytes, None);
    }

    #[test]
    fn kitty_report_all_can_still_emit_space_release() {
        let bytes = key_to_bytes(
            &Key::Named(NamedKey::Space),
            Some(" "),
            Some(&PhysicalKey::Code(KeyCode::Space)),
            false,
            ModifiersState::default(),
            KITTY_KBD_REPORT_ALL | KITTY_KBD_REPORT_EVENTS,
            KeyEventKind::Release,
        )
        .unwrap();
        assert_eq!(bytes, b"\x1b[32;1:3u");
    }

    #[test]
    fn kitty_disambiguate_keeps_enter_on_legacy_press_path() {
        let bytes = key_to_bytes(
            &Key::Named(NamedKey::Enter),
            Some("\r"),
            None,
            false,
            ModifiersState::default(),
            KITTY_KBD_DISAMBIGUATE,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(bytes, b"\r");
    }

    #[test]
    fn kitty_ctrl_backspace_falls_back_to_legacy_ctrl_h() {
        let bytes = key_to_bytes(
            &Key::Named(NamedKey::Backspace),
            None,
            None,
            false,
            ModifiersState::CONTROL,
            KITTY_KBD_DISAMBIGUATE | KITTY_KBD_REPORT_ALL,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(bytes, b"\x08");
    }

    #[test]
    fn kitty_alt_ctrl_backspace_keeps_escape_prefix() {
        let bytes = key_to_bytes(
            &Key::Named(NamedKey::Backspace),
            None,
            None,
            false,
            ModifiersState::CONTROL | ModifiersState::ALT,
            KITTY_KBD_DISAMBIGUATE | KITTY_KBD_REPORT_ALL,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(bytes, b"\x1b\x08");
    }

    #[test]
    fn kitty_disambiguates_modified_text_keys() {
        let modifiers = ModifiersState::SHIFT | ModifiersState::CONTROL;
        let bytes = key_to_bytes(
            &Key::Character("Z".into()),
            Some("Z"),
            Some(&PhysicalKey::Code(KeyCode::KeyZ)),
            false,
            modifiers,
            KITTY_KBD_DISAMBIGUATE,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(bytes, b"\x1b[122;6u");
    }

    #[test]
    fn kitty_report_all_can_attach_text_codepoints() {
        let modifiers = ModifiersState::SHIFT;
        let bytes = key_to_bytes(
            &Key::Character("A".into()),
            Some("A"),
            Some(&PhysicalKey::Code(KeyCode::KeyA)),
            false,
            modifiers,
            KITTY_KBD_REPORT_ALL | KITTY_KBD_REPORT_TEXT,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(bytes, b"\x1b[97;2;65u");
    }

    #[test]
    fn kitty_report_events_encodes_key_release() {
        let bytes = key_to_bytes(
            &Key::Named(NamedKey::ArrowUp),
            None,
            None,
            false,
            ModifiersState::default(),
            KITTY_KBD_REPORT_EVENTS,
            KeyEventKind::Release,
        )
        .unwrap();
        assert_eq!(bytes, b"\x1b[1;1:3A");
    }

    #[test]
    fn kitty_report_events_encodes_modified_text_repeat_without_report_all() {
        let bytes = key_to_bytes(
            &Key::Character("f".into()),
            Some("f"),
            Some(&PhysicalKey::Code(KeyCode::KeyF)),
            false,
            ModifiersState::ALT,
            KITTY_KBD_DISAMBIGUATE | KITTY_KBD_REPORT_EVENTS,
            KeyEventKind::Repeat,
        );
        assert_eq!(bytes, Some(b"\x1b[102;3:2u".to_vec()));
    }

    #[test]
    fn kitty_report_events_encodes_modified_text_release_without_report_all() {
        let bytes = key_to_bytes(
            &Key::Character("f".into()),
            Some("f"),
            Some(&PhysicalKey::Code(KeyCode::KeyF)),
            false,
            ModifiersState::ALT,
            KITTY_KBD_DISAMBIGUATE | KITTY_KBD_REPORT_EVENTS,
            KeyEventKind::Release,
        );
        assert_eq!(bytes, Some(b"\x1b[102;3:3u".to_vec()));
    }

    #[test]
    fn kitty_modifier_keys_have_distinct_codes() {
        let bytes = key_to_bytes(
            &Key::Named(NamedKey::Control),
            None,
            Some(&PhysicalKey::Code(KeyCode::ControlLeft)),
            false,
            effective_modifiers_for_key_event(
                ModifiersState::default(),
                false,
                false,
                false,
                false,
                &Key::Named(NamedKey::Control),
                KeyEventKind::Press,
            ),
            KITTY_KBD_REPORT_ALL,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(bytes, b"\x1b[57442;5u");

        let shift = key_to_bytes(
            &Key::Named(NamedKey::Shift),
            None,
            Some(&PhysicalKey::Code(KeyCode::ShiftLeft)),
            false,
            effective_modifiers_for_key_event(
                ModifiersState::default(),
                false,
                false,
                false,
                false,
                &Key::Named(NamedKey::Shift),
                KeyEventKind::Press,
            ),
            KITTY_KBD_REPORT_ALL,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(shift, b"\x1b[57441;2u");

        let hyper = key_to_bytes(
            &Key::Named(NamedKey::Hyper),
            None,
            Some(&PhysicalKey::Code(KeyCode::Hyper)),
            false,
            ModifiersState::default(),
            KITTY_KBD_REPORT_ALL,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(hyper, b"\x1b[57445u");

        let meta = key_to_bytes(
            &Key::Named(NamedKey::Meta),
            None,
            Some(&PhysicalKey::Code(KeyCode::Meta)),
            false,
            ModifiersState::default(),
            KITTY_KBD_REPORT_ALL,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(meta, b"\x1b[57446u");
    }

    #[test]
    fn kitty_modifier_field_includes_extra_meta_and_hyper_bits() {
        let hyper_modifiers = effective_modifiers_for_key_event(
            ModifiersState::CONTROL,
            true,
            false,
            false,
            false,
            &Key::Character("x".into()),
            KeyEventKind::Press,
        );
        let hyper = key_to_bytes(
            &Key::Character("x".into()),
            Some("x"),
            Some(&PhysicalKey::Code(KeyCode::KeyX)),
            false,
            hyper_modifiers,
            KITTY_KBD_DISAMBIGUATE,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(hyper, b"\x1b[120;21u");

        let meta_modifiers = effective_modifiers_for_key_event(
            ModifiersState::CONTROL,
            false,
            true,
            false,
            false,
            &Key::Character("x".into()),
            KeyEventKind::Press,
        );
        let meta = key_to_bytes(
            &Key::Character("x".into()),
            Some("x"),
            Some(&PhysicalKey::Code(KeyCode::KeyX)),
            false,
            meta_modifiers,
            KITTY_KBD_DISAMBIGUATE,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(meta, b"\x1b[120;37u");
    }

    #[test]
    fn modifier_key_events_do_not_self_include_meta_or_hyper_bits() {
        let hyper = key_to_bytes(
            &Key::Named(NamedKey::Hyper),
            None,
            Some(&PhysicalKey::Code(KeyCode::Hyper)),
            false,
            effective_modifiers_for_key_event(
                ModifiersState::default(),
                true,
                false,
                false,
                false,
                &Key::Named(NamedKey::Hyper),
                KeyEventKind::Press,
            ),
            KITTY_KBD_REPORT_ALL,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(hyper, b"\x1b[57445u");

        let meta = key_to_bytes(
            &Key::Named(NamedKey::Meta),
            None,
            Some(&PhysicalKey::Code(KeyCode::Meta)),
            false,
            effective_modifiers_for_key_event(
                ModifiersState::default(),
                false,
                true,
                false,
                false,
                &Key::Named(NamedKey::Meta),
                KeyEventKind::Press,
            ),
            KITTY_KBD_REPORT_ALL,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(meta, b"\x1b[57446u");
    }

    #[test]
    fn kitty_report_all_without_event_reporting_suppresses_release_only() {
        let release = key_to_bytes(
            &Key::Named(NamedKey::ArrowUp),
            None,
            None,
            false,
            ModifiersState::default(),
            KITTY_KBD_REPORT_ALL,
            KeyEventKind::Release,
        );
        assert_eq!(release, None);

        let repeat = key_to_bytes(
            &Key::Character("x".into()),
            Some("x"),
            Some(&PhysicalKey::Code(KeyCode::KeyX)),
            false,
            ModifiersState::default(),
            KITTY_KBD_REPORT_ALL,
            KeyEventKind::Repeat,
        )
        .unwrap();
        assert_eq!(repeat, b"\x1b[120u");
    }

    #[test]
    fn kitty_report_events_modifier_release_clears_own_modifier_bit() {
        let release = key_to_bytes(
            &Key::Named(NamedKey::Control),
            None,
            Some(&PhysicalKey::Code(KeyCode::ControlLeft)),
            false,
            effective_modifiers_for_key_event(
                ModifiersState::CONTROL,
                false,
                false,
                false,
                false,
                &Key::Named(NamedKey::Control),
                KeyEventKind::Release,
            ),
            KITTY_KBD_REPORT_ALL | KITTY_KBD_REPORT_EVENTS,
            KeyEventKind::Release,
        )
        .unwrap();
        assert_eq!(release, b"\x1b[57442;1:3u");
    }

    #[test]
    fn kitty_supports_extended_named_functional_keys() {
        let caps = key_to_bytes(
            &Key::Named(NamedKey::CapsLock),
            None,
            Some(&PhysicalKey::Code(KeyCode::CapsLock)),
            false,
            effective_modifiers_for_key_event(
                ModifiersState::default(),
                false,
                false,
                false,
                false,
                &Key::Named(NamedKey::CapsLock),
                KeyEventKind::Press,
            ),
            KITTY_KBD_REPORT_ALL,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(caps, b"\x1b[57358;65u");

        let num = key_to_bytes(
            &Key::Named(NamedKey::NumLock),
            None,
            Some(&PhysicalKey::Code(KeyCode::NumLock)),
            false,
            effective_modifiers_for_key_event(
                ModifiersState::default(),
                false,
                false,
                false,
                false,
                &Key::Named(NamedKey::NumLock),
                KeyEventKind::Press,
            ),
            KITTY_KBD_REPORT_ALL,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(num, b"\x1b[57360;129u");

        let menu = key_to_bytes(
            &Key::Named(NamedKey::ContextMenu),
            None,
            Some(&PhysicalKey::Code(KeyCode::ContextMenu)),
            false,
            ModifiersState::default(),
            KITTY_KBD_REPORT_ALL,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(menu, b"\x1b[29~");

        let f13 = key_to_bytes(
            &Key::Named(NamedKey::F13),
            None,
            Some(&PhysicalKey::Code(KeyCode::F13)),
            false,
            ModifiersState::default(),
            KITTY_KBD_REPORT_ALL,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(f13, b"\x1b[57376u");
    }

    #[test]
    fn kitty_supports_media_and_legacy_meta_keys() {
        let media = key_to_bytes(
            &Key::Named(NamedKey::MediaPlayPause),
            None,
            Some(&PhysicalKey::Code(KeyCode::MediaPlayPause)),
            false,
            ModifiersState::default(),
            KITTY_KBD_REPORT_ALL,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(media, b"\x1b[57430u");

        let meta = key_to_bytes(
            &Key::Named(NamedKey::Meta),
            None,
            Some(&PhysicalKey::Code(KeyCode::Meta)),
            false,
            ModifiersState::default(),
            KITTY_KBD_REPORT_ALL,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(meta, b"\x1b[57446u");

        let hyper = key_to_bytes(
            &Key::Named(NamedKey::Hyper),
            None,
            Some(&PhysicalKey::Code(KeyCode::Hyper)),
            false,
            ModifiersState::default(),
            KITTY_KBD_REPORT_ALL,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(hyper, b"\x1b[57445u");

        let alt_graph = key_to_bytes(
            &Key::Named(NamedKey::AltGraph),
            None,
            Some(&PhysicalKey::Code(KeyCode::AltRight)),
            false,
            ModifiersState::default(),
            KITTY_KBD_REPORT_ALL,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(alt_graph, b"\x1b[57453u");

        let record = key_to_bytes(
            &Key::Named(NamedKey::MediaRecord),
            None,
            None,
            false,
            ModifiersState::default(),
            KITTY_KBD_REPORT_ALL,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(record, b"\x1b[57437u");
    }

    #[test]
    fn kitty_modifier_field_includes_caps_and_num_lock_bits() {
        let caps_modifiers = effective_modifiers_for_key_event(
            ModifiersState::default(),
            false,
            false,
            true,
            false,
            &Key::Named(NamedKey::ArrowUp),
            KeyEventKind::Press,
        );
        let caps = key_to_bytes(
            &Key::Named(NamedKey::ArrowUp),
            None,
            None,
            false,
            caps_modifiers,
            KITTY_KBD_REPORT_ALL,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(caps, b"\x1b[1;65A");

        let num_modifiers = effective_modifiers_for_key_event(
            ModifiersState::CONTROL,
            false,
            false,
            false,
            true,
            &Key::Character("x".into()),
            KeyEventKind::Press,
        );
        let num = key_to_bytes(
            &Key::Character("x".into()),
            Some("x"),
            Some(&PhysicalKey::Code(KeyCode::KeyX)),
            false,
            num_modifiers,
            KITTY_KBD_DISAMBIGUATE,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(num, b"\x1b[120;133u");
    }

    #[test]
    fn kitty_lock_key_release_preserves_toggled_state() {
        let release = key_to_bytes(
            &Key::Named(NamedKey::CapsLock),
            None,
            Some(&PhysicalKey::Code(KeyCode::CapsLock)),
            false,
            effective_modifiers_for_key_event(
                ModifiersState::default(),
                false,
                false,
                true,
                false,
                &Key::Named(NamedKey::CapsLock),
                KeyEventKind::Release,
            ),
            KITTY_KBD_REPORT_ALL | KITTY_KBD_REPORT_EVENTS,
            KeyEventKind::Release,
        )
        .unwrap();
        assert_eq!(release, b"\x1b[57358;65:3u");
    }

    #[test]
    fn kitty_report_all_uses_distinct_keypad_codes_for_text_keys() {
        let bytes = key_to_bytes(
            &Key::Character("1".into()),
            Some("1"),
            Some(&PhysicalKey::Code(KeyCode::Numpad1)),
            false,
            ModifiersState::default(),
            KITTY_KBD_REPORT_ALL | KITTY_KBD_REPORT_TEXT,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(bytes, b"\x1b[57400;;49u");
    }

    #[test]
    fn kitty_disambiguate_reports_non_text_keypad_navigation_distinctly() {
        let bytes = key_to_bytes(
            &Key::Named(NamedKey::ArrowLeft),
            None,
            Some(&PhysicalKey::Code(KeyCode::Numpad4)),
            false,
            ModifiersState::default(),
            KITTY_KBD_DISAMBIGUATE,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(bytes, b"\x1b[57417u");
    }

    #[test]
    fn kitty_report_all_reports_keypad_enter_distinct_from_enter() {
        let bytes = key_to_bytes(
            &Key::Named(NamedKey::Enter),
            Some("\r"),
            Some(&PhysicalKey::Code(KeyCode::NumpadEnter)),
            false,
            ModifiersState::default(),
            KITTY_KBD_REPORT_ALL,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(bytes, b"\x1b[57414u");
    }

    #[test]
    fn kitty_keypad_begin_uses_legacy_sequence() {
        let bytes = key_to_bytes(
            &Key::Named(NamedKey::Clear),
            None,
            Some(&PhysicalKey::Code(KeyCode::Numpad5)),
            false,
            ModifiersState::default(),
            KITTY_KBD_DISAMBIGUATE,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(bytes, b"\x1b[E");

        let modified = key_to_bytes(
            &Key::Named(NamedKey::Clear),
            None,
            Some(&PhysicalKey::Code(KeyCode::Numpad5)),
            false,
            ModifiersState::SHIFT | ModifiersState::CONTROL,
            KITTY_KBD_REPORT_ALL | KITTY_KBD_REPORT_EVENTS,
            KeyEventKind::Repeat,
        )
        .unwrap();
        assert_eq!(modified, b"\x1b[1;6:2E");
    }

    #[test]
    fn kitty_context_menu_uses_legacy_functional_sequence() {
        let bytes = key_to_bytes(
            &Key::Named(NamedKey::ContextMenu),
            None,
            Some(&PhysicalKey::Code(KeyCode::ContextMenu)),
            false,
            ModifiersState::default(),
            KITTY_KBD_DISAMBIGUATE,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(bytes, b"\x1b[29~");

        let release = key_to_bytes(
            &Key::Named(NamedKey::ContextMenu),
            None,
            Some(&PhysicalKey::Code(KeyCode::ContextMenu)),
            false,
            ModifiersState::SHIFT,
            KITTY_KBD_REPORT_ALL | KITTY_KBD_REPORT_EVENTS,
            KeyEventKind::Release,
        )
        .unwrap();
        assert_eq!(release, b"\x1b[29;2:3~");
    }

    #[test]
    fn kitty_report_alternate_includes_shifted_symbol_variant() {
        let bytes = key_to_bytes(
            &Key::Character("+".into()),
            Some("+"),
            Some(&PhysicalKey::Code(KeyCode::Equal)),
            false,
            ModifiersState::SHIFT | ModifiersState::CONTROL,
            KITTY_KBD_DISAMBIGUATE | KITTY_KBD_REPORT_ALTERNATE,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(bytes, b"\x1b[61:43;6u");
    }

    #[test]
    fn kitty_report_alternate_preserves_current_layout_and_base_layout_key() {
        let bytes = key_to_bytes(
            &Key::Character("с".into()),
            Some("с"),
            Some(&PhysicalKey::Code(KeyCode::KeyC)),
            false,
            ModifiersState::CONTROL,
            KITTY_KBD_DISAMBIGUATE | KITTY_KBD_REPORT_ALTERNATE,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(bytes, "\x1b[1089::99;5u".as_bytes());

        let shifted = key_to_bytes(
            &Key::Character("С".into()),
            Some("С"),
            Some(&PhysicalKey::Code(KeyCode::KeyC)),
            false,
            ModifiersState::SHIFT | ModifiersState::CONTROL,
            KITTY_KBD_DISAMBIGUATE | KITTY_KBD_REPORT_ALTERNATE,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(shifted, "\x1b[1089:1057:99;6u".as_bytes());
    }

    #[test]
    fn legacy_release_events_do_not_emit_bytes() {
        let bytes = key_to_bytes(
            &Key::Character("x".into()),
            Some("x"),
            None,
            false,
            ModifiersState::default(),
            0,
            KeyEventKind::Release,
        );
        assert!(bytes.is_none());
    }

    #[test]
    fn alt_modified_enter_keeps_escape_prefix() {
        let bytes = key_to_bytes(
            &Key::Named(NamedKey::Enter),
            Some("\r"),
            None,
            false,
            ModifiersState::ALT,
            0,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(bytes, b"\x1b\r");
    }

    #[test]
    fn kitty_plain_text_key_falls_back_to_legacy_when_not_ambiguous() {
        let bytes = key_to_bytes(
            &Key::Character("a".into()),
            Some("a"),
            Some(&PhysicalKey::Code(KeyCode::KeyA)),
            false,
            ModifiersState::default(),
            KITTY_KBD_DISAMBIGUATE,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(bytes, b"a");
    }

    #[test]
    fn kitty_shift_only_symbol_key_types_shifted_text() {
        // shift+/ must type `?`, not a CSI u report of the unshifted base
        // key. Regression test: with only the disambiguate flag active,
        // shift-only text keys take the legacy path and emit their text.
        let bytes = key_to_bytes(
            &Key::Character("?".into()),
            Some("?"),
            Some(&PhysicalKey::Code(KeyCode::Slash)),
            false,
            ModifiersState::SHIFT,
            KITTY_KBD_DISAMBIGUATE,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(bytes, b"?");

        // Same for shifted letters: shift+a types `A`.
        let bytes = key_to_bytes(
            &Key::Character("A".into()),
            Some("A"),
            Some(&PhysicalKey::Code(KeyCode::KeyA)),
            false,
            ModifiersState::SHIFT,
            KITTY_KBD_DISAMBIGUATE,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(bytes, b"A");
    }

    #[test]
    fn kitty_app_cursor_uses_ss3_sequence_without_modifiers() {
        let bytes = key_to_bytes(
            &Key::Named(NamedKey::ArrowUp),
            None,
            None,
            true,
            ModifiersState::default(),
            KITTY_KBD_REPORT_ALL,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(bytes, b"\x1bOA");
    }
}
