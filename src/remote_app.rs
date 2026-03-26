use crate::client::{ProtocolClient, TryRecvStatus};
use crate::config::AppConfig;
use crate::font::GlyphAtlas;
use crate::frontend::{
    FrameScheduler, KeyEventKind, RecentTextKeyEvent, RedrawWork, base64_decode,
    classify_redraw_work, copy_to_clipboard, key_to_bytes, open_url, paste_from_clipboard,
    remember_text_key_event, scroll_to_bytes, scrollback_wheel_delta,
    should_skip_duplicate_ime_input, should_skip_ime_commit_after_key_event, spawn_fd_watcher,
    visual_signature,
};
use crate::protocol::{
    CellMetrics, ClientMessage, KeyEvent as ProtocolKeyEvent, KeyEventKind as ProtocolKeyEventKind,
    MouseButton as ProtocolMouseButton, MouseEvent as ProtocolMouseEvent,
    MouseEventKind as ProtocolMouseEventKind, ServerMessage, WindowId,
};
use crate::remote::{
    RemoteTerminalState, modifier_bits, should_apply_message, terminal_size_for_pixels,
};
use crate::render::OffscreenRenderer;
use anyhow::{Context, Result};
use softbuffer::{Context as SoftContext, Surface};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, Size};
use winit::event::{ElementState, Ime, Modifiers, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::platform::wayland::WindowAttributesExtWayland;
use winit::window::{ImePurpose, Window, WindowAttributes, WindowId as WinitWindowId};

#[derive(Debug, Clone)]
enum AppEvent {
    ServerReadable,
}

const FRAME_INTERVAL: Duration = Duration::from_millis(8);

pub fn run(config: AppConfig, socket_path: PathBuf) -> Result<()> {
    let event_loop = EventLoop::<AppEvent>::with_user_event()
        .build()
        .context("failed to create event loop")?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let proxy = event_loop.create_proxy();
    eprintln!(
        "{}",
        crate::build_info::startup_banner(crate::backend::Backend::Cpu, Some(&socket_path))
    );
    eprintln!("handterm: connecting to {}", socket_path.display());

    let mut app = RemoteHandtermApp::new(config, socket_path, proxy);
    event_loop
        .run_app(&mut app)
        .context("failed while running remote app")
}

struct RemoteHandtermApp {
    config: AppConfig,
    socket_path: PathBuf,
    state: Option<RemoteState>,
    proxy: EventLoopProxy<AppEvent>,
    watcher_started: bool,
    watcher_stop: Option<Arc<AtomicBool>>,
    scheduler: FrameScheduler,
}

struct RemoteState {
    window: Arc<Window>,
    _context: SoftContext<Arc<Window>>,
    surface: Surface<Arc<Window>, Arc<Window>>,
    surface_size: (u32, u32),
    terminal: RemoteTerminalState,
    client: ProtocolClient,
    window_id: Option<WindowId>,
    pending_size: Option<(u16, u16)>,
    disconnected: bool,
    modifiers: Modifiers,
    hyper_modifier: bool,
    meta_modifier: bool,
    caps_lock_modifier: bool,
    num_lock_modifier: bool,
    pending_ime_commit: Option<String>,
    recent_text_key_event: Option<RecentTextKeyEvent>,
    mouse_col: usize,
    mouse_row: usize,
    selecting: bool,
    atlas: GlyphAtlas,
    renderer: OffscreenRenderer,
    last_presented_signature: Option<u64>,
}

impl RemoteHandtermApp {
    fn new(config: AppConfig, socket_path: PathBuf, proxy: EventLoopProxy<AppEvent>) -> Self {
        Self {
            config,
            socket_path,
            state: None,
            proxy,
            watcher_started: false,
            watcher_stop: None,
            scheduler: FrameScheduler::default(),
        }
    }

    fn start_server_watcher(&mut self) {
        if self.watcher_started {
            return;
        }
        let Some(state) = &self.state else { return };

        let stop = Arc::new(AtomicBool::new(false));
        self.watcher_stop = Some(stop.clone());
        self.watcher_started = true;
        spawn_fd_watcher(
            "protocol-watcher",
            state.client.raw_fd(),
            -1,
            self.proxy.clone(),
            AppEvent::ServerReadable,
            stop,
        );
    }

    fn create_window_attributes(&self, metrics: CellMetrics) -> WindowAttributes {
        let width = self.config.window.columns as f64 * f64::from(metrics.cell_width.max(1));
        let height = self.config.window.rows as f64 * f64::from(metrics.cell_height.max(1));

        Window::default_attributes()
            .with_title("handterm [cpu remote]")
            .with_name("handterm", "handterm")
            .with_transparent(false)
            .with_inner_size(Size::Logical(LogicalSize::new(width, height)))
    }
}

impl ApplicationHandler<AppEvent> for RemoteHandtermApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let probe_window = event_loop
            .create_window(Window::default_attributes().with_visible(false))
            .expect("probe window should succeed");
        let scale_factor = probe_window.scale_factor();
        drop(probe_window);
        let dpi = (96.0 * scale_factor) as u32;

        let cols = self.config.window.columns;
        let rows = self.config.window.rows;
        let mut client = ProtocolClient::connect(&self.socket_path)
            .expect("remote frontend should connect to daemon");
        client
            .send(&ClientMessage::NewWindow { cols, rows, dpi })
            .expect("remote frontend should request a new window");

        let created = client
            .recv()
            .expect("remote frontend should receive window creation response");
        let (window_id, metrics) = match created {
            ServerMessage::WindowCreated {
                window_id, metrics, ..
            } => (window_id, metrics),
            other => panic!("expected WindowCreated from server, got {other:?}"),
        };

        let atlas = GlyphAtlas::protocol_only(metrics);

        let window = Arc::new(
            event_loop
                .create_window(self.create_window_attributes(metrics))
                .expect("window creation should succeed"),
        );
        window.set_ime_allowed(true);
        window.set_ime_purpose(ImePurpose::Terminal);
        let context =
            SoftContext::new(window.clone()).expect("softbuffer context should be created");
        let surface =
            Surface::new(&context, window.clone()).expect("softbuffer surface should be created");

        let renderer = OffscreenRenderer::new(cols, rows, &atlas);

        let mut terminal = RemoteTerminalState::new(cols, rows);
        let _ = terminal.apply_server_message(&ServerMessage::WindowCreated {
            window_id,
            cols,
            rows,
            metrics,
            modes: Default::default(),
        });
        client
            .set_nonblocking(true)
            .expect("protocol client should become non-blocking");

        self.state = Some(RemoteState {
            window,
            _context: context,
            surface,
            surface_size: (0, 0),
            terminal,
            client,
            window_id: Some(window_id),
            pending_size: None,
            disconnected: false,
            modifiers: Modifiers::default(),
            hyper_modifier: false,
            meta_modifier: false,
            caps_lock_modifier: false,
            num_lock_modifier: false,
            pending_ime_commit: None,
            recent_text_key_event: None,
            mouse_col: 0,
            mouse_row: 0,
            selecting: false,
            atlas,
            renderer,
            last_presented_signature: None,
        });

        if let Some(state) = self.state.as_mut() {
            while let Ok(TryRecvStatus::Message(message)) = state.client.try_recv() {
                let _ = apply_server_message(state, &message, event_loop);
            }
            state.window.request_redraw();
        }
        self.start_server_watcher();
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::ServerReadable => {
                self.scheduler.mark_io_ready(Instant::now(), FRAME_INTERVAL);
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WinitWindowId,
        event: WindowEvent,
    ) {
        let Some(state) = self.state.as_mut() else {
            return;
        };

        match event {
            WindowEvent::CloseRequested => {
                if let Some(window_id) = state.window_id {
                    let _ = state.client.send(&ClientMessage::CloseWindow { window_id });
                }
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let (Some(width), Some(height)) =
                    (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
                {
                    state
                        .surface
                        .resize(width, height)
                        .expect("surface resize should succeed");
                    state.surface_size = (width.get(), height.get());
                    state
                        .renderer
                        .resize_pixels(width.get() as usize, height.get() as usize);
                    state.last_presented_signature = None;

                    let new_size =
                        terminal_size_for_pixels(width.get(), height.get(), &state.atlas);
                    state.pending_size = Some(new_size);
                    maybe_send_resize(state);
                    state.window.request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(new_modifiers) => {
                state.modifiers = new_modifiers;
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                if !text.is_empty() {
                    let ime_commit_text = crate::frontend::normalize_ime_dedupe_text(&text)
                        .unwrap_or_else(|| text.clone());
                    if should_skip_ime_commit_after_key_event(
                        &mut state.recent_text_key_event,
                        &ime_commit_text,
                        Instant::now(),
                    ) {
                        return;
                    }
                    state.pending_ime_commit = Some(ime_commit_text);
                    let _ = send_key_input(
                        state,
                        KeyEventKind::Press,
                        text.as_bytes().to_vec(),
                        Some(text),
                    );
                    if state.terminal.grid.scroll_offset > 0 {
                        state.terminal.grid.scroll_offset = 0;
                        state.terminal.grid.all_dirty = true;
                    }
                    state.terminal.grid.selection = None;
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    let ctrl = state.modifiers.state().control_key();
                    let shift = state.modifiers.state().shift_key();

                    if ctrl
                        && shift
                        && let Key::Character(s) = &event.logical_key
                    {
                        let ch = s.chars().next().unwrap_or('\0').to_ascii_lowercase();
                        if ch == 'v' {
                            if let Some(mut text) = paste_from_clipboard() {
                                if state.terminal.bracketed_paste_mode() {
                                    let mut wrapped = Vec::with_capacity(text.len() + 12);
                                    wrapped.extend_from_slice(b"\x1b[200~");
                                    wrapped.append(&mut text);
                                    wrapped.extend_from_slice(b"\x1b[201~");
                                    let _ = send_paste(state, wrapped);
                                } else {
                                    let _ = send_paste(state, text);
                                }
                            }
                            return;
                        } else if ch == 'c' {
                            let text = state.terminal.grid.get_selection_text();
                            if !text.is_empty() {
                                copy_to_clipboard(text.as_bytes());
                            }
                            return;
                        }
                    }

                    if shift {
                        if let Key::Named(NamedKey::PageUp) = &event.logical_key {
                            let max = state.terminal.grid.scrollback_len();
                            let half = state.terminal.rows as usize / 2;
                            state.terminal.grid.scroll_offset =
                                (state.terminal.grid.scroll_offset + half).min(max);
                            self.scheduler.mark_redraw_needed();
                            return;
                        }
                        if let Key::Named(NamedKey::PageDown) = &event.logical_key {
                            let half = state.terminal.rows as usize / 2;
                            state.terminal.grid.scroll_offset =
                                state.terminal.grid.scroll_offset.saturating_sub(half);
                            self.scheduler.mark_redraw_needed();
                            return;
                        }
                    }
                }

                let event_kind = match (event.state, event.repeat) {
                    (ElementState::Pressed, true) => KeyEventKind::Repeat,
                    (ElementState::Pressed, false) => KeyEventKind::Press,
                    (ElementState::Released, _) => KeyEventKind::Release,
                };

                let ime_dedupe_text =
                    crate::frontend::key_ime_dedupe_text(&event.logical_key, event.text.as_deref());

                let modifiers = crate::frontend::effective_modifiers_for_key_event(
                    state.modifiers.state(),
                    state.hyper_modifier,
                    state.meta_modifier,
                    state.caps_lock_modifier,
                    state.num_lock_modifier,
                    &event.logical_key,
                    event_kind,
                );

                if let Some(bytes) = key_to_bytes(
                    &event.logical_key,
                    event.text.as_deref(),
                    Some(&event.physical_key),
                    state.terminal.application_cursor_keys,
                    modifiers,
                    state.terminal.kitty_keyboard_flags(),
                    event_kind,
                ) {
                    if should_skip_duplicate_ime_input(
                        &mut state.pending_ime_commit,
                        event_kind,
                        ime_dedupe_text.as_deref(),
                        Some(&bytes),
                    ) {
                        return;
                    }
                    remember_text_key_event(
                        &mut state.recent_text_key_event,
                        event_kind,
                        ime_dedupe_text.as_deref(),
                        Some(&bytes),
                        Instant::now(),
                    );
                    let _ = send_key_input(
                        state,
                        event_kind,
                        bytes,
                        event.text.as_ref().map(ToString::to_string),
                    );
                    if state.terminal.grid.scroll_offset > 0 {
                        state.terminal.grid.scroll_offset = 0;
                        state.terminal.grid.all_dirty = true;
                    }
                    state.terminal.grid.selection = None;
                } else {
                    remember_text_key_event(
                        &mut state.recent_text_key_event,
                        event_kind,
                        ime_dedupe_text.as_deref(),
                        None,
                        Instant::now(),
                    );
                    let _ = should_skip_duplicate_ime_input(
                        &mut state.pending_ime_commit,
                        event_kind,
                        ime_dedupe_text.as_deref(),
                        None,
                    );
                }

                crate::frontend::apply_modifier_key_transition(
                    &mut state.hyper_modifier,
                    &mut state.meta_modifier,
                    &mut state.caps_lock_modifier,
                    &mut state.num_lock_modifier,
                    &event.logical_key,
                    event_kind,
                );
            }
            WindowEvent::CursorMoved { position, .. } => {
                let cw = state.atlas.cell_width.max(1);
                let ch = state.atlas.cell_height.max(1);
                state.mouse_col = position.x as usize / cw;
                state.mouse_row = position.y as usize / ch;

                if state.selecting {
                    if let Some(ref mut sel) = state.terminal.grid.selection {
                        sel.end_col = state.mouse_col;
                        sel.end_row = state.mouse_row;
                    }
                    self.scheduler.mark_redraw_needed();
                }
            }
            WindowEvent::MouseInput {
                state: btn_state,
                button,
                ..
            } => {
                let protocol_button = match button {
                    MouseButton::Left => ProtocolMouseButton::Left,
                    MouseButton::Middle => ProtocolMouseButton::Middle,
                    MouseButton::Right => ProtocolMouseButton::Right,
                    _ => return,
                };
                let pressed = btn_state == ElementState::Pressed;

                if button == MouseButton::Left {
                    if pressed {
                        let ctrl = state.modifiers.state().control_key();
                        if ctrl {
                            let cell = state
                                .terminal
                                .grid
                                .cell_at(state.mouse_row, state.mouse_col);
                            if cell.hyperlink_id != 0
                                && let Some(url) =
                                    state.terminal.grid.hyperlink_url(cell.hyperlink_id)
                            {
                                open_url(url);
                                return;
                            }
                        }
                        state.terminal.grid.selection = Some(crate::grid::Selection {
                            start_col: state.mouse_col,
                            start_row: state.mouse_row,
                            end_col: state.mouse_col,
                            end_row: state.mouse_row,
                        });
                        state.selecting = true;
                        self.scheduler.mark_redraw_needed();
                    } else {
                        state.selecting = false;
                        let text = state.terminal.grid.get_selection_text();
                        if !text.is_empty() {
                            copy_to_clipboard(text.as_bytes());
                        }
                    }
                }

                if state.terminal.mouse_mode != crate::terminal::MouseMode::Off {
                    let _ = send_mouse_input(
                        state,
                        ProtocolMouseEvent {
                            kind: if pressed {
                                ProtocolMouseEventKind::Press
                            } else {
                                ProtocolMouseEventKind::Release
                            },
                            button: protocol_button,
                            col: state.mouse_col as u16,
                            row: state.mouse_row as u16,
                            modifiers: modifier_bits(state.modifiers.state()),
                        },
                    );
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (up, lines) = match delta {
                    MouseScrollDelta::LineDelta(_, y) => (y > 0.0, y.abs().max(1.0) as usize),
                    MouseScrollDelta::PixelDelta(pos) => {
                        let ch = state.atlas.cell_height.max(1) as f64;
                        (pos.y > 0.0, (pos.y.abs() / ch).max(1.0) as usize)
                    }
                };

                if state.terminal.mouse_mode != crate::terminal::MouseMode::Off {
                    for _ in 0..lines {
                        let _ = send_mouse_input(
                            state,
                            ProtocolMouseEvent {
                                kind: if up {
                                    ProtocolMouseEventKind::ScrollUp
                                } else {
                                    ProtocolMouseEventKind::ScrollDown
                                },
                                button: ProtocolMouseButton::None,
                                col: state.mouse_col as u16,
                                row: state.mouse_row as u16,
                                modifiers: modifier_bits(state.modifiers.state()),
                            },
                        );
                    }
                } else if state.terminal.alternate_scroll_mode() && state.terminal.in_alt_screen() {
                    let bytes = scroll_to_bytes(up, state.terminal.application_cursor_keys);
                    for _ in 0..lines {
                        let _ = send_key_input(state, KeyEventKind::Press, bytes.clone(), None);
                    }
                } else {
                    let max = state.terminal.grid.scrollback_len();
                    let delta = scrollback_wheel_delta(lines);
                    if up {
                        state.terminal.grid.scroll_offset =
                            (state.terminal.grid.scroll_offset + delta).min(max);
                    } else {
                        state.terminal.grid.scroll_offset =
                            state.terminal.grid.scroll_offset.saturating_sub(delta);
                    }
                    self.scheduler.mark_redraw_needed();
                }
            }
            WindowEvent::Focused(focused) => {
                if state.terminal.focus_events_mode() {
                    let bytes = if focused {
                        b"\x1b[I".to_vec()
                    } else {
                        b"\x1b[O".to_vec()
                    };
                    let _ = send_key_input(state, KeyEventKind::Press, bytes, None);
                }
            }
            WindowEvent::RedrawRequested => {
                render_grid(state, &self.config).expect("frame render should succeed");
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(state) = &mut self.state {
            let decision = self
                .scheduler
                .prepare_redraw(Instant::now(), || process_pending_io(state, event_loop));
            if let Some(deadline) = decision.wait_until {
                event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
            } else {
                event_loop.set_control_flow(ControlFlow::Wait);
            }
            if decision.request_redraw {
                state.window.request_redraw();
            }
            if state.disconnected {
                if let Some(stop) = &self.watcher_stop {
                    stop.store(true, Ordering::Relaxed);
                }
                event_loop.exit();
            }
        }
    }
}

fn process_pending_io(state: &mut RemoteState, event_loop: &ActiveEventLoop) -> RedrawWork {
    let mut changed = false;

    loop {
        match state.client.try_recv() {
            Ok(TryRecvStatus::Message(message)) => {
                changed |= apply_server_message(state, &message, event_loop);
            }
            Ok(TryRecvStatus::Empty) => break,
            Ok(TryRecvStatus::Closed) => {
                state.disconnected = true;
                break;
            }
            Err(_) => {
                state.disconnected = true;
                break;
            }
        }
    }

    classify_redraw_work(&state.terminal, changed)
}

fn apply_server_message(
    state: &mut RemoteState,
    message: &ServerMessage,
    event_loop: &ActiveEventLoop,
) -> bool {
    if !should_apply_message(state.window_id, message) {
        return false;
    }

    if let ServerMessage::WindowCreated { window_id, .. } = message {
        state.window_id = Some(*window_id);
        maybe_send_resize(state);
    }

    let effects = state.terminal.apply_server_message(message);
    if let ServerMessage::AtlasUpdate { glyph } = message {
        state.atlas.insert_protocol_glyph(glyph);
        state.last_presented_signature = None;
    }
    if let Some(title) = effects.title {
        state.window.set_title(&title);
    }
    if let Some(text) = effects.clipboard {
        if let Ok(decoded) = base64_decode(&text) {
            copy_to_clipboard(&decoded);
        } else {
            copy_to_clipboard(&text);
        }
    }
    if effects.closed.is_some() {
        state.disconnected = true;
        event_loop.exit();
    }

    matches!(
        message,
        ServerMessage::WindowCreated { .. }
            | ServerMessage::WindowResized { .. }
            | ServerMessage::CellUpdate { .. }
            | ServerMessage::AtlasUpdate { .. }
    )
}

fn maybe_send_resize(state: &mut RemoteState) {
    let Some(window_id) = state.window_id else {
        return;
    };
    let Some((cols, rows)) = state.pending_size else {
        return;
    };
    if cols == state.terminal.cols && rows == state.terminal.rows {
        state.pending_size = None;
        return;
    }
    let _ = state.client.send(&ClientMessage::Resize {
        window_id,
        cols,
        rows,
    });
    state.pending_size = None;
}

fn send_key_input(
    state: &mut RemoteState,
    kind: KeyEventKind,
    bytes: Vec<u8>,
    text: Option<String>,
) -> Result<()> {
    let Some(window_id) = state.window_id else {
        return Ok(());
    };
    state.client.send(&ClientMessage::KeyInput {
        window_id,
        event: ProtocolKeyEvent {
            kind: match kind {
                KeyEventKind::Press => ProtocolKeyEventKind::Press,
                KeyEventKind::Repeat => ProtocolKeyEventKind::Repeat,
                KeyEventKind::Release => ProtocolKeyEventKind::Release,
            },
            bytes,
            text,
            modifiers: modifier_bits(state.modifiers.state()),
        },
    })
}

fn send_paste(state: &mut RemoteState, text: Vec<u8>) -> Result<()> {
    let Some(window_id) = state.window_id else {
        return Ok(());
    };
    state.client.send(&ClientMessage::Paste { window_id, text })
}

fn send_mouse_input(state: &mut RemoteState, event: ProtocolMouseEvent) -> Result<()> {
    let Some(window_id) = state.window_id else {
        return Ok(());
    };
    state
        .client
        .send(&ClientMessage::MouseInput { window_id, event })
}

fn render_grid(state: &mut RemoteState, config: &AppConfig) -> Result<()> {
    let size = state.window.inner_size();
    let (Some(width), Some(height)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
    else {
        return Ok(());
    };

    let size_tuple = (width.get(), height.get());
    if state.surface_size != size_tuple {
        state
            .surface
            .resize(width, height)
            .map_err(|e| anyhow::anyhow!("failed to resize backbuffer: {e}"))?;
        state.surface_size = size_tuple;
        state.last_presented_signature = None;
    }

    let signature = visual_signature(&state.terminal);
    if state.last_presented_signature == Some(signature) {
        state.terminal.grid.clear_dirty();
        return Ok(());
    }

    state
        .renderer
        .render(&mut state.terminal, &mut state.atlas, config);
    let mut buffer = state
        .surface
        .buffer_mut()
        .map_err(|e| anyhow::anyhow!("failed to acquire backbuffer: {e}"))?;
    buffer.copy_from_slice(&state.renderer.pixels);

    buffer
        .present()
        .map_err(|e| anyhow::anyhow!("failed presenting frame: {e}"))?;
    state.last_presented_signature = Some(signature);
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::remote::modifier_bits;

    #[test]
    fn modifier_bits_match_expected_protocol_mask() {
        let modifiers = winit::keyboard::ModifiersState::SHIFT
            | winit::keyboard::ModifiersState::CONTROL
            | winit::keyboard::ModifiersState::ALT;
        assert_eq!(modifier_bits(modifiers), 0b0111);
    }
}
