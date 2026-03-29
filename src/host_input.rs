use crate::frontend::{
    RecentTextKeyEvent, key_to_bytes, remember_text_key_event, should_skip_duplicate_ime_input,
    should_skip_ime_commit_after_key_event,
};
use crate::ipc::SyntheticKeyEvent;
use crate::pty::PtyChild;
use crate::terminal::Terminal;
use std::time::Instant;
use winit::keyboard::Key;

pub trait SyntheticInputTarget {
    fn label(&self) -> &'static str;
    fn terminal(&mut self) -> &mut Terminal;
    fn pty(&mut self) -> &mut PtyChild;
    fn pending_ime_commit(&mut self) -> &mut Option<String>;
    fn recent_text_key_event(&mut self) -> &mut Option<RecentTextKeyEvent>;
    fn hyper_modifier_mut(&mut self) -> &mut bool;
    fn meta_modifier_mut(&mut self) -> &mut bool;
    fn caps_lock_modifier_mut(&mut self) -> &mut bool;
    fn num_lock_modifier_mut(&mut self) -> &mut bool;
    fn caps_lock_modifier(&self) -> bool;
    fn num_lock_modifier(&self) -> bool;
    fn apply_modifier_transition(
        &mut self,
        logical_key: &Key,
        event_kind: crate::frontend::KeyEventKind,
    );
    fn before_pty_write(&mut self) {}
    fn reset_scrollback(&mut self);
    fn drain_pty(&mut self) -> bool;
}

pub fn apply_synthetic_ime_commit<T: SyntheticInputTarget>(state: &mut T, text: &str) -> bool {
    if text.is_empty() {
        return false;
    }

    let ime_commit_text =
        crate::frontend::normalize_ime_dedupe_text(text).unwrap_or_else(|| text.to_string());
    crate::frontend::trace_input(format!(
        "{} synthetic ime-commit raw={:?} normalized={:?}",
        state.label(),
        text,
        ime_commit_text
    ));
    if should_skip_ime_commit_after_key_event(
        state.recent_text_key_event(),
        &ime_commit_text,
        Instant::now(),
    ) {
        crate::frontend::trace_input(format!(
            "{} synthetic ime-commit skipped after key-event dedupe",
            state.label()
        ));
        return false;
    }

    state.before_pty_write();
    *state.pending_ime_commit() = Some(ime_commit_text);
    let _ = state.pty().write_all(text.as_bytes());
    if state.terminal().grid.scroll_offset > 0 {
        state.reset_scrollback();
    }
    state.terminal().grid.selection = None;
    state.drain_pty()
}

pub fn apply_synthetic_key_event<T: SyntheticInputTarget>(
    state: &mut T,
    event: &SyntheticKeyEvent,
) -> bool {
    let logical_key = crate::frontend::parse_synthetic_key(&event.key);
    let base_modifiers =
        crate::frontend::synthetic_modifiers_state(crate::frontend::SyntheticModifierState {
            ctrl: event.ctrl,
            alt: event.alt,
            shift: event.shift,
            super_key: event.super_key,
            hyper: event.hyper,
            meta: event.meta,
            caps_lock: state.caps_lock_modifier(),
            num_lock: state.num_lock_modifier(),
        });
    let modifiers = crate::frontend::effective_modifiers_for_key_event(
        base_modifiers,
        event.hyper,
        event.meta,
        state.caps_lock_modifier(),
        state.num_lock_modifier(),
        &logical_key,
        event.kind,
    );
    let ime_dedupe_text = crate::frontend::key_ime_dedupe_text(&logical_key, event.text.as_deref());
    let (application_cursor_keys, kitty_keyboard_flags) = {
        let terminal = state.terminal();
        (
            terminal.application_cursor_keys,
            terminal.kitty_keyboard_flags(),
        )
    };

    let changed = if let Some(bytes) = key_to_bytes(
        &logical_key,
        event.text.as_deref(),
        None,
        application_cursor_keys,
        modifiers,
        kitty_keyboard_flags,
        event.kind,
    ) {
        crate::frontend::trace_input(format!(
            "{} synthetic key-event kind={:?} key={:?} text={:?} dedupe_text={:?} bytes={:?}",
            state.label(),
            event.kind,
            logical_key,
            event.text,
            ime_dedupe_text,
            bytes
        ));
        if should_skip_duplicate_ime_input(
            state.pending_ime_commit(),
            event.kind,
            ime_dedupe_text.as_deref(),
            Some(&bytes),
        ) {
            crate::frontend::trace_input(format!(
                "{} synthetic key-event skipped by ime dedupe",
                state.label()
            ));
            return false;
        }
        remember_text_key_event(
            state.recent_text_key_event(),
            event.kind,
            ime_dedupe_text.as_deref(),
            Some(&bytes),
            Instant::now(),
        );
        state.before_pty_write();
        let _ = state.pty().write_all(&bytes);
        if state.terminal().grid.scroll_offset > 0 {
            state.reset_scrollback();
        }
        state.terminal().grid.selection = None;
        state.drain_pty()
    } else {
        remember_text_key_event(
            state.recent_text_key_event(),
            event.kind,
            ime_dedupe_text.as_deref(),
            None,
            Instant::now(),
        );
        let _ = should_skip_duplicate_ime_input(
            state.pending_ime_commit(),
            event.kind,
            ime_dedupe_text.as_deref(),
            None,
        );
        false
    };

    state.apply_modifier_transition(&logical_key, event.kind);

    changed
}
