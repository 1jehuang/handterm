use crate::terminal::{
    KITTY_KBD_DISAMBIGUATE, KITTY_KBD_REPORT_ALL, KITTY_KBD_REPORT_ALTERNATE,
    KITTY_KBD_REPORT_EVENTS, KITTY_KBD_REPORT_TEXT,
};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEventKind {
    Press,
    Repeat,
    Release,
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
    if kitty_flags != 0
        && let Some(bytes) = encode_kitty_key(
            key,
            text,
            physical_key,
            app_cursor,
            modifiers,
            kitty_flags,
            event_kind,
        )
    {
        return Some(bytes);
    }

    if matches!(event_kind, KeyEventKind::Release) {
        return None;
    }

    legacy_key_to_bytes(key, app_cursor, modifiers)
}

fn legacy_key_to_bytes(key: &Key, app_cursor: bool, modifiers: ModifiersState) -> Option<Vec<u8>> {
    let ctrl = modifiers.control_key();
    let alt = modifiers.alt_key();

    if ctrl && let Key::Character(s) = key {
        let ch = s.chars().next()?;
        let mut out = Vec::new();
        if alt {
            out.push(0x1b);
        }
        if ch.is_ascii_alphabetic() {
            out.push(ch.to_ascii_lowercase() as u8 - b'a' + 1);
            return Some(out);
        }
        match ch {
            '@' | ' ' | '`' => out.push(0),
            '2' => out.push(0),
            '3' => out.push(0x1b),
            '4' => out.push(0x1c),
            '5' => out.push(0x1d),
            '6' => out.push(0x1e),
            '7' => out.push(0x1f),
            '8' | '?' => out.push(0x7f),
            '9' => out.push(b'9'),
            '0' => out.push(b'0'),
            '[' | '\x1b' => out.push(0x1b),
            '\\' => out.push(0x1c),
            ']' => out.push(0x1d),
            '^' | '~' => out.push(0x1e),
            '_' | '/' => out.push(0x1f),
            _ => return None,
        }
        return Some(out);
    }

    let mut bytes = match key {
        Key::Character(s) => Some(s.as_bytes().to_vec()),
        Key::Named(named) => legacy_named_key_to_bytes(*named, app_cursor, modifiers),
        _ => None,
    }?;

    if alt && matches!(key, Key::Character(_)) {
        bytes.insert(0, 0x1b);
    }

    Some(bytes)
}

fn legacy_named_key_to_bytes(
    named: NamedKey,
    app_cursor: bool,
    modifiers: ModifiersState,
) -> Option<Vec<u8>> {
    let shift = modifiers.shift_key();
    let alt = modifiers.alt_key();
    let ctrl = modifiers.control_key();

    match named {
        NamedKey::Enter => {
            let mut out = Vec::new();
            if alt {
                out.push(0x1b);
            }
            out.push(b'\r');
            Some(out)
        }
        NamedKey::Escape => Some(if alt {
            b"\x1b\x1b".to_vec()
        } else {
            b"\x1b".to_vec()
        }),
        NamedKey::Backspace => {
            let mut out = Vec::new();
            if alt {
                out.push(0x1b);
            }
            out.push(if ctrl { 0x08 } else { 0x7f });
            Some(out)
        }
        NamedKey::Tab => {
            let mut out = Vec::new();
            if alt {
                out.push(0x1b);
            }
            if shift {
                out.extend_from_slice(b"\x1b[Z");
            } else {
                out.push(b'\t');
            }
            Some(out)
        }
        NamedKey::Space => {
            let mut out = Vec::new();
            if alt {
                out.push(0x1b);
            }
            out.push(if ctrl { 0x00 } else { b' ' });
            Some(out)
        }
        NamedKey::ArrowUp => legacy_named_sequence(app_cursor, modifiers, b'A', 0, 0),
        NamedKey::ArrowDown => legacy_named_sequence(app_cursor, modifiers, b'B', 0, 0),
        NamedKey::ArrowRight => legacy_named_sequence(app_cursor, modifiers, b'C', 0, 0),
        NamedKey::ArrowLeft => legacy_named_sequence(app_cursor, modifiers, b'D', 0, 0),
        NamedKey::Home => legacy_named_sequence(app_cursor, modifiers, b'H', 0, 0),
        NamedKey::End => legacy_named_sequence(app_cursor, modifiers, b'F', 0, 0),
        NamedKey::Insert => legacy_named_sequence(false, modifiers, 0, 2, b'~'),
        NamedKey::Delete => legacy_named_sequence(false, modifiers, 0, 3, b'~'),
        NamedKey::PageUp => legacy_named_sequence(false, modifiers, 0, 5, b'~'),
        NamedKey::PageDown => legacy_named_sequence(false, modifiers, 0, 6, b'~'),
        NamedKey::F1 => legacy_named_sequence(false, modifiers, b'P', 0, 0),
        NamedKey::F2 => legacy_named_sequence(false, modifiers, b'Q', 0, 0),
        NamedKey::F3 => legacy_named_sequence(false, modifiers, 0, 13, b'~'),
        NamedKey::F4 => legacy_named_sequence(false, modifiers, b'S', 0, 0),
        NamedKey::F5 => legacy_named_sequence(false, modifiers, 0, 15, b'~'),
        NamedKey::F6 => legacy_named_sequence(false, modifiers, 0, 17, b'~'),
        NamedKey::F7 => legacy_named_sequence(false, modifiers, 0, 18, b'~'),
        NamedKey::F8 => legacy_named_sequence(false, modifiers, 0, 19, b'~'),
        NamedKey::F9 => legacy_named_sequence(false, modifiers, 0, 20, b'~'),
        NamedKey::F10 => legacy_named_sequence(false, modifiers, 0, 21, b'~'),
        NamedKey::F11 => legacy_named_sequence(false, modifiers, 0, 23, b'~'),
        NamedKey::F12 => legacy_named_sequence(false, modifiers, 0, 24, b'~'),
        _ => None,
    }
}

fn legacy_named_sequence(
    app_cursor: bool,
    modifiers: ModifiersState,
    letter_suffix: u8,
    tilde_number: u16,
    trailing: u8,
) -> Option<Vec<u8>> {
    if modifiers.super_key() {
        return None;
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
            return Some(if app_cursor {
                vec![0x1b, b'O', letter_suffix]
            } else {
                vec![0x1b, b'[', letter_suffix]
            });
        }
        let mut seq = vec![0x1b, b'[', b'1', b';'];
        seq.extend_from_slice(value.to_string().as_bytes());
        seq.push(letter_suffix);
        return Some(seq);
    }

    let mut seq = vec![0x1b, b'['];
    seq.extend_from_slice(tilde_number.to_string().as_bytes());
    if value != 1 {
        seq.push(b';');
        seq.extend_from_slice(value.to_string().as_bytes());
    }
    seq.push(trailing);
    Some(seq)
}

fn encode_kitty_key(
    key: &Key,
    text: Option<&str>,
    physical_key: Option<&PhysicalKey>,
    app_cursor: bool,
    modifiers: ModifiersState,
    kitty_flags: u8,
    event_kind: KeyEventKind,
) -> Option<Vec<u8>> {
    let report_all = kitty_flags & KITTY_KBD_REPORT_ALL != 0;
    let report_events = kitty_flags & KITTY_KBD_REPORT_EVENTS != 0;
    let report_alternate = kitty_flags & KITTY_KBD_REPORT_ALTERNATE != 0;
    let report_text = report_all && (kitty_flags & KITTY_KBD_REPORT_TEXT != 0);
    let disambiguate = kitty_flags & KITTY_KBD_DISAMBIGUATE != 0;
    let produces_text = text.filter(|s| !s.is_empty()).is_some();

    if let Some(bytes) = encode_kitty_named_key(
        key,
        physical_key,
        app_cursor,
        modifiers,
        report_all,
        report_events,
        event_kind,
    ) {
        return Some(bytes);
    }

    if !produces_text {
        return None;
    }
    if !report_all && matches!(event_kind, KeyEventKind::Repeat | KeyEventKind::Release) {
        return None;
    }
    if !report_all && !disambiguate && matches!(event_kind, KeyEventKind::Press) {
        return None;
    }
    if !report_all && !text_key_is_ambiguous(modifiers) {
        return None;
    }

    let primary = text_key_primary_codepoint(key, physical_key)?;
    let first = kitty_first_param(primary, text, physical_key, modifiers, report_alternate);
    let modifier_field = kitty_modifier_field(modifiers, report_events, event_kind);
    let text_field = if report_text { kitty_text_field(text) } else { None };
    Some(format_kitty_csi_u(&first, modifier_field, text_field))
}

fn text_key_is_ambiguous(modifiers: ModifiersState) -> bool {
    modifiers.shift_key() || modifiers.alt_key() || modifiers.control_key() || modifiers.super_key()
}

fn text_key_primary_codepoint(key: &Key, physical_key: Option<&PhysicalKey>) -> Option<u32> {
    if let Some(base) = physical_key.and_then(base_layout_codepoint) {
        return Some(base);
    }

    match key {
        Key::Character(s) => {
            let ch = s.chars().next()?;
            Some(if ch.is_ascii_alphabetic() {
                ch.to_ascii_lowercase() as u32
            } else {
                ch as u32
            })
        }
        _ => None,
    }
}

fn kitty_first_param(
    primary: u32,
    text: Option<&str>,
    physical_key: Option<&PhysicalKey>,
    modifiers: ModifiersState,
    report_alternate: bool,
) -> String {
    if !report_alternate {
        return primary.to_string();
    }

    let shifted = if modifiers.shift_key() {
        text.and_then(|s| s.chars().next()).map(|ch| ch as u32)
    } else {
        None
    };
    let base_layout = physical_key.and_then(base_layout_codepoint);

    match (
        shifted.filter(|cp| *cp != primary),
        base_layout.filter(|cp| *cp != primary),
    ) {
        (Some(shifted), Some(base_layout)) => format!("{primary}:{shifted}:{base_layout}"),
        (Some(shifted), None) => format!("{primary}:{shifted}"),
        (None, Some(base_layout)) => format!("{primary}::{base_layout}"),
        (None, None) => primary.to_string(),
    }
}

fn kitty_modifier_field(
    modifiers: ModifiersState,
    report_events: bool,
    event_kind: KeyEventKind,
) -> Option<String> {
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

    if report_events {
        let event = match event_kind {
            KeyEventKind::Press => 1,
            KeyEventKind::Repeat => 2,
            KeyEventKind::Release => 3,
        };
        if value == 1 && event == 1 {
            None
        } else {
            Some(format!("{value}:{event}"))
        }
    } else if value == 1 {
        None
    } else {
        Some(value.to_string())
    }
}

fn kitty_text_field(text: Option<&str>) -> Option<String> {
    let text = text?;
    let codepoints = text
        .chars()
        .filter(|ch| !ch.is_control())
        .map(|ch| (ch as u32).to_string())
        .collect::<Vec<_>>();
    if codepoints.is_empty() {
        None
    } else {
        Some(codepoints.join(":"))
    }
}

fn format_kitty_csi_u(first: &str, modifier_field: Option<String>, text_field: Option<String>) -> Vec<u8> {
    let mut seq = String::from("\x1b[");
    seq.push_str(first);
    if modifier_field.is_some() || text_field.is_some() {
        seq.push(';');
        if let Some(mods) = modifier_field {
            seq.push_str(&mods);
        }
    }
    if let Some(text) = text_field {
        seq.push(';');
        seq.push_str(&text);
    }
    seq.push('u');
    seq.into_bytes()
}

fn encode_kitty_named_key(
    key: &Key,
    physical_key: Option<&PhysicalKey>,
    app_cursor: bool,
    modifiers: ModifiersState,
    report_all: bool,
    report_events: bool,
    event_kind: KeyEventKind,
) -> Option<Vec<u8>> {
    let mods = kitty_modifier_field(modifiers, report_events, event_kind);
    let letter_form = |prefix_o: bool, suffix: char| {
        let mut seq = String::from("\x1b[");
        if let Some(mods) = mods.clone() {
            seq.push('1');
            seq.push(';');
            seq.push_str(&mods);
        } else if prefix_o {
            seq.push('O');
            seq.push(suffix);
            return seq.into_bytes();
        }
        seq.push(suffix);
        seq.into_bytes()
    };
    let tilde_form = |number: u16| {
        let mut seq = String::from("\x1b[");
        seq.push_str(&number.to_string());
        if let Some(mods) = mods.clone() {
            seq.push(';');
            seq.push_str(&mods);
        }
        seq.push('~');
        seq.into_bytes()
    };

    match key {
        Key::Named(named) => Some(match named {
            NamedKey::Enter | NamedKey::Tab | NamedKey::Backspace
                if !report_all && matches!(event_kind, KeyEventKind::Press) =>
            {
                return None;
            }
            NamedKey::Space
                if !report_all
                    && matches!(event_kind, KeyEventKind::Press)
                    && !modifiers.shift_key()
                    && !modifiers.alt_key()
                    && !modifiers.control_key()
                    && !modifiers.super_key() =>
            {
                return None;
            }
            NamedKey::ArrowUp => letter_form(app_cursor, 'A'),
            NamedKey::ArrowDown => letter_form(app_cursor, 'B'),
            NamedKey::ArrowRight => letter_form(app_cursor, 'C'),
            NamedKey::ArrowLeft => letter_form(app_cursor, 'D'),
            NamedKey::Home => letter_form(app_cursor, 'H'),
            NamedKey::End => letter_form(app_cursor, 'F'),
            NamedKey::F1 => letter_form(false, 'P'),
            NamedKey::F2 => letter_form(false, 'Q'),
            NamedKey::F4 => letter_form(false, 'S'),
            NamedKey::Insert => tilde_form(2),
            NamedKey::Delete => tilde_form(3),
            NamedKey::PageUp => tilde_form(5),
            NamedKey::PageDown => tilde_form(6),
            NamedKey::F3 => tilde_form(13),
            NamedKey::F5 => tilde_form(15),
            NamedKey::F6 => tilde_form(17),
            NamedKey::F7 => tilde_form(18),
            NamedKey::F8 => tilde_form(19),
            NamedKey::F9 => tilde_form(20),
            NamedKey::F10 => tilde_form(21),
            NamedKey::F11 => tilde_form(23),
            NamedKey::F12 => tilde_form(24),
            NamedKey::Escape => format_kitty_csi_u("27", mods, None),
            NamedKey::Enter => format_kitty_csi_u("13", mods, None),
            NamedKey::Tab => format_kitty_csi_u("9", mods, None),
            NamedKey::Backspace => format_kitty_csi_u("127", mods, None),
            NamedKey::Space => format_kitty_csi_u("32", mods, None),
            NamedKey::Shift | NamedKey::Control | NamedKey::Alt | NamedKey::Super => {
                let code = modifier_named_key_code(physical_key, named)?;
                format_kitty_csi_u(&code.to_string(), mods, None)
            }
            _ => return None,
        }),
        _ => None,
    }
}

fn modifier_named_key_code(physical_key: Option<&PhysicalKey>, named: &NamedKey) -> Option<u32> {
    match physical_key {
        Some(PhysicalKey::Code(KeyCode::ShiftLeft)) => Some(57441),
        Some(PhysicalKey::Code(KeyCode::ControlLeft)) => Some(57442),
        Some(PhysicalKey::Code(KeyCode::AltLeft)) => Some(57443),
        Some(PhysicalKey::Code(KeyCode::SuperLeft)) => Some(57444),
        Some(PhysicalKey::Code(KeyCode::ShiftRight)) => Some(57447),
        Some(PhysicalKey::Code(KeyCode::ControlRight)) => Some(57448),
        Some(PhysicalKey::Code(KeyCode::AltRight)) => Some(57449),
        Some(PhysicalKey::Code(KeyCode::SuperRight)) => Some(57450),
        _ => match named {
            NamedKey::Shift => Some(57441),
            NamedKey::Control => Some(57442),
            NamedKey::Alt => Some(57443),
            NamedKey::Super => Some(57444),
            _ => None,
        },
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
    fn kitty_modifier_keys_have_distinct_codes() {
        let bytes = key_to_bytes(
            &Key::Named(NamedKey::Control),
            None,
            Some(&PhysicalKey::Code(KeyCode::ControlLeft)),
            false,
            ModifiersState::default(),
            KITTY_KBD_REPORT_ALL,
            KeyEventKind::Press,
        )
        .unwrap();
        assert_eq!(bytes, b"\x1b[57442u");
    }
}
