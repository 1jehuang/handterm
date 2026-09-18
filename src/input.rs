//! Winit adapter for the shared terminal keyboard encoder.
pub use common::KeyEventKind;
use handterm_common::input as common;
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};

fn logical_key(key: &Key) -> common::Key<'_> {
    match key {
        Key::Character(text) => common::Key::Character(text.as_str().into()),
        Key::Named(named) => match named {
            NamedKey::Alt => common::Key::Named(common::NamedKey::Alt),
            NamedKey::AltGraph => common::Key::Named(common::NamedKey::AltGraph),
            NamedKey::ArrowDown => common::Key::Named(common::NamedKey::ArrowDown),
            NamedKey::ArrowLeft => common::Key::Named(common::NamedKey::ArrowLeft),
            NamedKey::ArrowRight => common::Key::Named(common::NamedKey::ArrowRight),
            NamedKey::ArrowUp => common::Key::Named(common::NamedKey::ArrowUp),
            NamedKey::AudioVolumeDown => common::Key::Named(common::NamedKey::AudioVolumeDown),
            NamedKey::AudioVolumeMute => common::Key::Named(common::NamedKey::AudioVolumeMute),
            NamedKey::AudioVolumeUp => common::Key::Named(common::NamedKey::AudioVolumeUp),
            NamedKey::Backspace => common::Key::Named(common::NamedKey::Backspace),
            NamedKey::CapsLock => common::Key::Named(common::NamedKey::CapsLock),
            NamedKey::Clear => common::Key::Named(common::NamedKey::Clear),
            NamedKey::ContextMenu => common::Key::Named(common::NamedKey::ContextMenu),
            NamedKey::Control => common::Key::Named(common::NamedKey::Control),
            NamedKey::Delete => common::Key::Named(common::NamedKey::Delete),
            NamedKey::End => common::Key::Named(common::NamedKey::End),
            NamedKey::Enter => common::Key::Named(common::NamedKey::Enter),
            NamedKey::Escape => common::Key::Named(common::NamedKey::Escape),
            NamedKey::F1 => common::Key::Named(common::NamedKey::F1),
            NamedKey::F10 => common::Key::Named(common::NamedKey::F10),
            NamedKey::F11 => common::Key::Named(common::NamedKey::F11),
            NamedKey::F12 => common::Key::Named(common::NamedKey::F12),
            NamedKey::F13 => common::Key::Named(common::NamedKey::F13),
            NamedKey::F14 => common::Key::Named(common::NamedKey::F14),
            NamedKey::F15 => common::Key::Named(common::NamedKey::F15),
            NamedKey::F16 => common::Key::Named(common::NamedKey::F16),
            NamedKey::F17 => common::Key::Named(common::NamedKey::F17),
            NamedKey::F18 => common::Key::Named(common::NamedKey::F18),
            NamedKey::F19 => common::Key::Named(common::NamedKey::F19),
            NamedKey::F2 => common::Key::Named(common::NamedKey::F2),
            NamedKey::F20 => common::Key::Named(common::NamedKey::F20),
            NamedKey::F21 => common::Key::Named(common::NamedKey::F21),
            NamedKey::F22 => common::Key::Named(common::NamedKey::F22),
            NamedKey::F23 => common::Key::Named(common::NamedKey::F23),
            NamedKey::F24 => common::Key::Named(common::NamedKey::F24),
            NamedKey::F25 => common::Key::Named(common::NamedKey::F25),
            NamedKey::F26 => common::Key::Named(common::NamedKey::F26),
            NamedKey::F27 => common::Key::Named(common::NamedKey::F27),
            NamedKey::F28 => common::Key::Named(common::NamedKey::F28),
            NamedKey::F29 => common::Key::Named(common::NamedKey::F29),
            NamedKey::F3 => common::Key::Named(common::NamedKey::F3),
            NamedKey::F30 => common::Key::Named(common::NamedKey::F30),
            NamedKey::F31 => common::Key::Named(common::NamedKey::F31),
            NamedKey::F32 => common::Key::Named(common::NamedKey::F32),
            NamedKey::F33 => common::Key::Named(common::NamedKey::F33),
            NamedKey::F34 => common::Key::Named(common::NamedKey::F34),
            NamedKey::F35 => common::Key::Named(common::NamedKey::F35),
            NamedKey::F4 => common::Key::Named(common::NamedKey::F4),
            NamedKey::F5 => common::Key::Named(common::NamedKey::F5),
            NamedKey::F6 => common::Key::Named(common::NamedKey::F6),
            NamedKey::F7 => common::Key::Named(common::NamedKey::F7),
            NamedKey::F8 => common::Key::Named(common::NamedKey::F8),
            NamedKey::F9 => common::Key::Named(common::NamedKey::F9),
            NamedKey::Home => common::Key::Named(common::NamedKey::Home),
            NamedKey::Hyper => common::Key::Named(common::NamedKey::Hyper),
            NamedKey::Insert => common::Key::Named(common::NamedKey::Insert),
            NamedKey::MediaFastForward => common::Key::Named(common::NamedKey::MediaFastForward),
            NamedKey::MediaPause => common::Key::Named(common::NamedKey::MediaPause),
            NamedKey::MediaPlay => common::Key::Named(common::NamedKey::MediaPlay),
            NamedKey::MediaPlayPause => common::Key::Named(common::NamedKey::MediaPlayPause),
            NamedKey::MediaRecord => common::Key::Named(common::NamedKey::MediaRecord),
            NamedKey::MediaRewind => common::Key::Named(common::NamedKey::MediaRewind),
            NamedKey::MediaStop => common::Key::Named(common::NamedKey::MediaStop),
            NamedKey::MediaTrackNext => common::Key::Named(common::NamedKey::MediaTrackNext),
            NamedKey::MediaTrackPrevious => {
                common::Key::Named(common::NamedKey::MediaTrackPrevious)
            }
            NamedKey::Meta => common::Key::Named(common::NamedKey::Meta),
            NamedKey::NumLock => common::Key::Named(common::NamedKey::NumLock),
            NamedKey::PageDown => common::Key::Named(common::NamedKey::PageDown),
            NamedKey::PageUp => common::Key::Named(common::NamedKey::PageUp),
            NamedKey::Pause => common::Key::Named(common::NamedKey::Pause),
            NamedKey::PrintScreen => common::Key::Named(common::NamedKey::PrintScreen),
            NamedKey::ScrollLock => common::Key::Named(common::NamedKey::ScrollLock),
            NamedKey::Shift => common::Key::Named(common::NamedKey::Shift),
            NamedKey::Space => common::Key::Named(common::NamedKey::Space),
            NamedKey::Super => common::Key::Named(common::NamedKey::Super),
            NamedKey::Tab => common::Key::Named(common::NamedKey::Tab),
            _ => common::Key::Unidentified,
        },
        _ => common::Key::Unidentified,
    }
}

fn physical_key(key: &PhysicalKey) -> common::PhysicalKey {
    match key {
        PhysicalKey::Code(code) => match code {
            KeyCode::AltLeft => common::PhysicalKey::Code(common::KeyCode::AltLeft),
            KeyCode::AltRight => common::PhysicalKey::Code(common::KeyCode::AltRight),
            KeyCode::Backquote => common::PhysicalKey::Code(common::KeyCode::Backquote),
            KeyCode::Backslash => common::PhysicalKey::Code(common::KeyCode::Backslash),
            KeyCode::BracketLeft => common::PhysicalKey::Code(common::KeyCode::BracketLeft),
            KeyCode::BracketRight => common::PhysicalKey::Code(common::KeyCode::BracketRight),
            KeyCode::CapsLock => common::PhysicalKey::Code(common::KeyCode::CapsLock),
            KeyCode::Comma => common::PhysicalKey::Code(common::KeyCode::Comma),
            KeyCode::ContextMenu => common::PhysicalKey::Code(common::KeyCode::ContextMenu),
            KeyCode::ControlLeft => common::PhysicalKey::Code(common::KeyCode::ControlLeft),
            KeyCode::ControlRight => common::PhysicalKey::Code(common::KeyCode::ControlRight),
            KeyCode::Digit0 => common::PhysicalKey::Code(common::KeyCode::Digit0),
            KeyCode::Digit1 => common::PhysicalKey::Code(common::KeyCode::Digit1),
            KeyCode::Digit2 => common::PhysicalKey::Code(common::KeyCode::Digit2),
            KeyCode::Digit3 => common::PhysicalKey::Code(common::KeyCode::Digit3),
            KeyCode::Digit4 => common::PhysicalKey::Code(common::KeyCode::Digit4),
            KeyCode::Digit5 => common::PhysicalKey::Code(common::KeyCode::Digit5),
            KeyCode::Digit6 => common::PhysicalKey::Code(common::KeyCode::Digit6),
            KeyCode::Digit7 => common::PhysicalKey::Code(common::KeyCode::Digit7),
            KeyCode::Digit8 => common::PhysicalKey::Code(common::KeyCode::Digit8),
            KeyCode::Digit9 => common::PhysicalKey::Code(common::KeyCode::Digit9),
            KeyCode::Equal => common::PhysicalKey::Code(common::KeyCode::Equal),
            KeyCode::F13 => common::PhysicalKey::Code(common::KeyCode::F13),
            KeyCode::Hyper => common::PhysicalKey::Code(common::KeyCode::Hyper),
            KeyCode::KeyA => common::PhysicalKey::Code(common::KeyCode::KeyA),
            KeyCode::KeyB => common::PhysicalKey::Code(common::KeyCode::KeyB),
            KeyCode::KeyC => common::PhysicalKey::Code(common::KeyCode::KeyC),
            KeyCode::KeyD => common::PhysicalKey::Code(common::KeyCode::KeyD),
            KeyCode::KeyE => common::PhysicalKey::Code(common::KeyCode::KeyE),
            KeyCode::KeyF => common::PhysicalKey::Code(common::KeyCode::KeyF),
            KeyCode::KeyG => common::PhysicalKey::Code(common::KeyCode::KeyG),
            KeyCode::KeyH => common::PhysicalKey::Code(common::KeyCode::KeyH),
            KeyCode::KeyI => common::PhysicalKey::Code(common::KeyCode::KeyI),
            KeyCode::KeyJ => common::PhysicalKey::Code(common::KeyCode::KeyJ),
            KeyCode::KeyK => common::PhysicalKey::Code(common::KeyCode::KeyK),
            KeyCode::KeyL => common::PhysicalKey::Code(common::KeyCode::KeyL),
            KeyCode::KeyM => common::PhysicalKey::Code(common::KeyCode::KeyM),
            KeyCode::KeyN => common::PhysicalKey::Code(common::KeyCode::KeyN),
            KeyCode::KeyO => common::PhysicalKey::Code(common::KeyCode::KeyO),
            KeyCode::KeyP => common::PhysicalKey::Code(common::KeyCode::KeyP),
            KeyCode::KeyQ => common::PhysicalKey::Code(common::KeyCode::KeyQ),
            KeyCode::KeyR => common::PhysicalKey::Code(common::KeyCode::KeyR),
            KeyCode::KeyS => common::PhysicalKey::Code(common::KeyCode::KeyS),
            KeyCode::KeyT => common::PhysicalKey::Code(common::KeyCode::KeyT),
            KeyCode::KeyU => common::PhysicalKey::Code(common::KeyCode::KeyU),
            KeyCode::KeyV => common::PhysicalKey::Code(common::KeyCode::KeyV),
            KeyCode::KeyW => common::PhysicalKey::Code(common::KeyCode::KeyW),
            KeyCode::KeyX => common::PhysicalKey::Code(common::KeyCode::KeyX),
            KeyCode::KeyY => common::PhysicalKey::Code(common::KeyCode::KeyY),
            KeyCode::KeyZ => common::PhysicalKey::Code(common::KeyCode::KeyZ),
            KeyCode::MediaPlayPause => common::PhysicalKey::Code(common::KeyCode::MediaPlayPause),
            KeyCode::Meta => common::PhysicalKey::Code(common::KeyCode::Meta),
            KeyCode::Minus => common::PhysicalKey::Code(common::KeyCode::Minus),
            KeyCode::NumLock => common::PhysicalKey::Code(common::KeyCode::NumLock),
            KeyCode::Numpad0 => common::PhysicalKey::Code(common::KeyCode::Numpad0),
            KeyCode::Numpad1 => common::PhysicalKey::Code(common::KeyCode::Numpad1),
            KeyCode::Numpad2 => common::PhysicalKey::Code(common::KeyCode::Numpad2),
            KeyCode::Numpad3 => common::PhysicalKey::Code(common::KeyCode::Numpad3),
            KeyCode::Numpad4 => common::PhysicalKey::Code(common::KeyCode::Numpad4),
            KeyCode::Numpad5 => common::PhysicalKey::Code(common::KeyCode::Numpad5),
            KeyCode::Numpad6 => common::PhysicalKey::Code(common::KeyCode::Numpad6),
            KeyCode::Numpad7 => common::PhysicalKey::Code(common::KeyCode::Numpad7),
            KeyCode::Numpad8 => common::PhysicalKey::Code(common::KeyCode::Numpad8),
            KeyCode::Numpad9 => common::PhysicalKey::Code(common::KeyCode::Numpad9),
            KeyCode::NumpadAdd => common::PhysicalKey::Code(common::KeyCode::NumpadAdd),
            KeyCode::NumpadComma => common::PhysicalKey::Code(common::KeyCode::NumpadComma),
            KeyCode::NumpadDecimal => common::PhysicalKey::Code(common::KeyCode::NumpadDecimal),
            KeyCode::NumpadDivide => common::PhysicalKey::Code(common::KeyCode::NumpadDivide),
            KeyCode::NumpadEnter => common::PhysicalKey::Code(common::KeyCode::NumpadEnter),
            KeyCode::NumpadEqual => common::PhysicalKey::Code(common::KeyCode::NumpadEqual),
            KeyCode::NumpadMultiply => common::PhysicalKey::Code(common::KeyCode::NumpadMultiply),
            KeyCode::NumpadSubtract => common::PhysicalKey::Code(common::KeyCode::NumpadSubtract),
            KeyCode::Period => common::PhysicalKey::Code(common::KeyCode::Period),
            KeyCode::Quote => common::PhysicalKey::Code(common::KeyCode::Quote),
            KeyCode::Semicolon => common::PhysicalKey::Code(common::KeyCode::Semicolon),
            KeyCode::ShiftLeft => common::PhysicalKey::Code(common::KeyCode::ShiftLeft),
            KeyCode::ShiftRight => common::PhysicalKey::Code(common::KeyCode::ShiftRight),
            KeyCode::Slash => common::PhysicalKey::Code(common::KeyCode::Slash),
            KeyCode::Space => common::PhysicalKey::Code(common::KeyCode::Space),
            KeyCode::SuperLeft => common::PhysicalKey::Code(common::KeyCode::SuperLeft),
            KeyCode::SuperRight => common::PhysicalKey::Code(common::KeyCode::SuperRight),
            _ => common::PhysicalKey::Unidentified,
        },
        _ => common::PhysicalKey::Unidentified,
    }
}

fn modifiers(value: ModifiersState) -> common::ModifiersState {
    let mut result = common::ModifiersState::from_bits_retain(value.bits() & 0x0f00_0000);
    result.set(common::ModifiersState::SHIFT, value.shift_key());
    result.set(common::ModifiersState::CONTROL, value.control_key());
    result.set(common::ModifiersState::ALT, value.alt_key());
    result.set(common::ModifiersState::SUPER, value.super_key());
    result
}

fn winit_modifiers(value: common::ModifiersState) -> ModifiersState {
    let mut result = ModifiersState::from_bits_retain(value.bits() & 0x0f00_0000);
    result.set(ModifiersState::SHIFT, value.shift_key());
    result.set(ModifiersState::CONTROL, value.control_key());
    result.set(ModifiersState::ALT, value.alt_key());
    result.set(ModifiersState::SUPER, value.super_key());
    result
}

pub fn effective_modifiers_for_key_event(
    state: ModifiersState,
    hyper: bool,
    meta: bool,
    caps_lock: bool,
    num_lock: bool,
    key: &Key,
    event_kind: KeyEventKind,
) -> ModifiersState {
    winit_modifiers(common::effective_modifiers_for_key_event(
        modifiers(state),
        hyper,
        meta,
        caps_lock,
        num_lock,
        &logical_key(key),
        event_kind,
    ))
}

pub fn apply_modifier_key_transition(
    hyper: &mut bool,
    meta: &mut bool,
    caps_lock: &mut bool,
    num_lock: &mut bool,
    key: &Key,
    event_kind: KeyEventKind,
) {
    common::apply_modifier_key_transition(
        hyper,
        meta,
        caps_lock,
        num_lock,
        &logical_key(key),
        event_kind,
    );
}

pub fn key_to_bytes(
    key: &Key,
    text: Option<&str>,
    physical: Option<&PhysicalKey>,
    app_cursor: bool,
    state: ModifiersState,
    kitty_flags: u8,
    event_kind: KeyEventKind,
) -> Option<Vec<u8>> {
    common::key_to_bytes(
        &logical_key(key),
        text,
        physical.map(physical_key).as_ref(),
        app_cursor,
        modifiers(state),
        kitty_flags,
        event_kind,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn key_to_bytes_into(
    out: &mut Vec<u8>,
    key: &Key,
    text: Option<&str>,
    physical: Option<&PhysicalKey>,
    app_cursor: bool,
    state: ModifiersState,
    kitty_flags: u8,
    event_kind: KeyEventKind,
) -> bool {
    common::key_to_bytes_into(
        out,
        &logical_key(key),
        text,
        physical.map(physical_key).as_ref(),
        app_cursor,
        modifiers(state),
        kitty_flags,
        event_kind,
    )
}

pub fn modifiers_with_extra(
    state: ModifiersState,
    hyper: bool,
    meta: bool,
    caps_lock: bool,
    num_lock: bool,
) -> ModifiersState {
    winit_modifiers(common::modifiers_with_extra(
        modifiers(state),
        hyper,
        meta,
        caps_lock,
        num_lock,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_preserves_all_modifiers() {
        let state = ModifiersState::SHIFT
            | ModifiersState::CONTROL
            | ModifiersState::ALT
            | ModifiersState::SUPER;
        let state = modifiers_with_extra(state, true, true, true, true);
        assert_eq!(winit_modifiers(modifiers(state)), state);
        assert_eq!(
            key_to_bytes(
                &Key::Character("x".into()),
                Some("x"),
                None,
                false,
                state,
                crate::terminal::KITTY_KBD_REPORT_ALL,
                KeyEventKind::Press
            ),
            Some(b"\x1b[120;256u".to_vec())
        );
    }

    #[test]
    fn adapter_borrows_text_and_maps_physical_keys() {
        let key = Key::Character("a".into());
        assert!(matches!(
            logical_key(&key),
            common::Key::Character(std::borrow::Cow::Borrowed("a"))
        ));
        assert_eq!(
            physical_key(&PhysicalKey::Code(KeyCode::NumpadEnter)),
            common::PhysicalKey::Code(common::KeyCode::NumpadEnter)
        );
        assert_eq!(
            key_to_bytes(
                &Key::Named(NamedKey::Enter),
                Some("\r"),
                Some(&PhysicalKey::Code(KeyCode::NumpadEnter)),
                false,
                ModifiersState::default(),
                crate::terminal::KITTY_KBD_REPORT_ALL,
                KeyEventKind::Press
            ),
            Some(b"\x1b[57414u".to_vec())
        );
    }

    #[test]
    fn adapter_updates_modifier_transitions_and_reuses_output() {
        let state = effective_modifiers_for_key_event(
            ModifiersState::SHIFT,
            false,
            false,
            false,
            false,
            &Key::Named(NamedKey::Shift),
            KeyEventKind::Release,
        );
        assert!(!state.shift_key());
        let mut out = vec![1, 2, 3];
        assert!(key_to_bytes_into(
            &mut out,
            &Key::Named(NamedKey::ArrowUp),
            None,
            None,
            true,
            state,
            0,
            KeyEventKind::Press
        ));
        assert_eq!(out, b"\x1bOA");
        assert!(!key_to_bytes_into(
            &mut out,
            &Key::Named(NamedKey::ArrowUp),
            None,
            None,
            true,
            state,
            0,
            KeyEventKind::Release
        ));
        assert!(out.is_empty());
    }
}
