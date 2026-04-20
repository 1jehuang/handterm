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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::{KeyEventKind, apply_modifier_key_transition};
    use crate::terminal::{KITTY_KBD_REPORT_ALL, KITTY_KBD_REPORT_EVENTS};
    use std::time::{Duration, Instant};
    use winit::keyboard::Key;

    const READY_MARKER: &[u8] = b"__handterm-host-input-ready__";

    struct TestSyntheticTarget {
        terminal: Terminal,
        pty: PtyChild,
        pending_ime_commit: Option<String>,
        recent_text_key_event: Option<RecentTextKeyEvent>,
        hyper: bool,
        meta: bool,
        caps_lock: bool,
        num_lock: bool,
        captured_output: Vec<u8>,
        scrollback_reset_count: usize,
    }

    impl TestSyntheticTarget {
        fn new() -> Self {
            let pty = PtyChild::spawn_shell_command(
                "/bin/sh",
                "stty raw -echo; printf '__handterm-host-input-ready__'; cat",
                80,
                24,
            )
            .expect("pty should spawn raw cat shell");

            let mut ready = Vec::new();
            let mut buffer = [0_u8; 1024];
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                let n = pty
                    .try_read(&mut buffer)
                    .expect("ready read should succeed");
                if n == 0 {
                    continue;
                }
                ready.extend_from_slice(&buffer[..n]);
                if ready
                    .windows(READY_MARKER.len())
                    .any(|window| window == READY_MARKER)
                {
                    break;
                }
            }
            assert!(
                ready
                    .windows(READY_MARKER.len())
                    .any(|window| window == READY_MARKER),
                "timed out waiting for raw cat readiness: {ready:?}"
            );

            Self {
                terminal: Terminal::new(80, 24),
                pty,
                pending_ime_commit: None,
                recent_text_key_event: None,
                hyper: false,
                meta: false,
                caps_lock: false,
                num_lock: false,
                captured_output: Vec::new(),
                scrollback_reset_count: 0,
            }
        }

        fn take_captured_output(&mut self) -> Vec<u8> {
            std::mem::take(&mut self.captured_output)
        }
    }

    impl SyntheticInputTarget for TestSyntheticTarget {
        fn label(&self) -> &'static str {
            "test"
        }

        fn terminal(&mut self) -> &mut Terminal {
            &mut self.terminal
        }

        fn pty(&mut self) -> &mut PtyChild {
            &mut self.pty
        }

        fn pending_ime_commit(&mut self) -> &mut Option<String> {
            &mut self.pending_ime_commit
        }

        fn recent_text_key_event(&mut self) -> &mut Option<RecentTextKeyEvent> {
            &mut self.recent_text_key_event
        }

        fn hyper_modifier_mut(&mut self) -> &mut bool {
            &mut self.hyper
        }

        fn meta_modifier_mut(&mut self) -> &mut bool {
            &mut self.meta
        }

        fn caps_lock_modifier_mut(&mut self) -> &mut bool {
            &mut self.caps_lock
        }

        fn num_lock_modifier_mut(&mut self) -> &mut bool {
            &mut self.num_lock
        }

        fn caps_lock_modifier(&self) -> bool {
            self.caps_lock
        }

        fn num_lock_modifier(&self) -> bool {
            self.num_lock
        }

        fn apply_modifier_transition(&mut self, logical_key: &Key, event_kind: KeyEventKind) {
            apply_modifier_key_transition(
                &mut self.hyper,
                &mut self.meta,
                &mut self.caps_lock,
                &mut self.num_lock,
                logical_key,
                event_kind,
            );
        }

        fn reset_scrollback(&mut self) {
            self.scrollback_reset_count += 1;
        }

        fn drain_pty(&mut self) -> bool {
            let mut changed = false;
            let mut buffer = [0_u8; 1024];
            let deadline = Instant::now() + Duration::from_millis(250);
            while Instant::now() < deadline {
                let n = self
                    .pty
                    .try_read(&mut buffer)
                    .expect("pty drain should succeed");
                if n == 0 {
                    if changed {
                        break;
                    }
                    continue;
                }
                self.captured_output.extend_from_slice(&buffer[..n]);
                changed = true;
            }
            changed
        }
    }

    #[test]
    fn synthetic_key_event_uses_negotiated_kitty_keyboard_mode() {
        let mut state = TestSyntheticTarget::new();
        state.terminal.process(
            &format!("\x1b[={}u", KITTY_KBD_REPORT_ALL | KITTY_KBD_REPORT_EVENTS).into_bytes(),
        );

        let press = SyntheticKeyEvent {
            kind: KeyEventKind::Press,
            key: "ctrl".to_string(),
            text: None,
            ctrl: true,
            alt: false,
            shift: false,
            super_key: false,
            hyper: false,
            meta: false,
        };
        assert!(apply_synthetic_key_event(&mut state, &press));
        assert_eq!(state.take_captured_output(), b"\x1b[57442;5:1u");

        let release = SyntheticKeyEvent {
            kind: KeyEventKind::Release,
            key: "ctrl".to_string(),
            text: None,
            ctrl: true,
            alt: false,
            shift: false,
            super_key: false,
            hyper: false,
            meta: false,
        };
        assert!(apply_synthetic_key_event(&mut state, &release));
        assert_eq!(state.take_captured_output(), b"\x1b[57442;1:3u");
        assert_eq!(
            state.terminal.kitty_keyboard_flags(),
            KITTY_KBD_REPORT_ALL | KITTY_KBD_REPORT_EVENTS
        );
        assert_eq!(state.scrollback_reset_count, 0);
    }
}
