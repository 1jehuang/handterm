use crate::config::AppConfig;
use crate::font::GlyphAtlas;
use crate::frontend::{
    FrameScheduler, VisualState, base64_decode, copy_to_clipboard, handle_ipc_request,
    key_to_bytes, open_url, paste_from_clipboard, scroll_to_bytes, spawn_pty_watcher,
};
use crate::ipc::{IpcAction, IpcServer};
use crate::pty::PtyChild;
use crate::render::render_terminal_to_buffer;
use crate::terminal::Terminal;
use anyhow::{Context, Result};
use softbuffer::{Context as SoftContext, Surface};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, Size};
use winit::event::{ElementState, Modifiers, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::platform::wayland::WindowAttributesExtWayland;
use winit::window::{Window, WindowAttributes, WindowId};

#[derive(Debug, Clone)]
enum AppEvent {
    PtyReadable,
}

const FRAME_INTERVAL: Duration = Duration::from_millis(8);

pub fn run(config: AppConfig) -> Result<()> {
    let event_loop = EventLoop::<AppEvent>::with_user_event()
        .build()
        .context("failed to create event loop")?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let proxy = event_loop.create_proxy();

    let socket_path = crate::ipc::default_socket_path();
    let ipc = IpcServer::bind(&socket_path).ok();
    if let Some(ref ipc) = ipc {
        eprintln!("handterm: listening on {}", ipc.path().display());
    }

    let mut app = HandtermApp::new(config, ipc, proxy);
    event_loop
        .run_app(&mut app)
        .context("failed while running app")
}

struct HandtermApp {
    config: AppConfig,
    state: Option<AppState>,
    ipc: Option<IpcServer>,
    proxy: EventLoopProxy<AppEvent>,
    watcher_started: bool,
    watcher_stop: Option<Arc<AtomicBool>>,
    scheduler: FrameScheduler,
}

struct AppState {
    window: Arc<Window>,
    _context: SoftContext<Arc<Window>>,
    surface: Surface<Arc<Window>, Arc<Window>>,
    terminal: Terminal,
    pty: PtyChild,
    pty_buf: Vec<u8>,
    atlas: GlyphAtlas,
    pty_closed: bool,
    modifiers: Modifiers,
    mouse_col: usize,
    mouse_row: usize,
    selecting: bool,
    last_visual_state: Option<VisualState>,
}

impl HandtermApp {
    fn new(config: AppConfig, ipc: Option<IpcServer>, proxy: EventLoopProxy<AppEvent>) -> Self {
        Self {
            config,
            state: None,
            ipc,
            proxy,
            watcher_started: false,
            watcher_stop: None,
            scheduler: FrameScheduler::default(),
        }
    }

    fn start_pty_watcher(&mut self) {
        if self.watcher_started {
            return;
        }
        let Some(state) = &self.state else { return };

        let pty_fd = state.pty.raw_fd();
        let ipc_fd = self.ipc.as_ref().map(|s| s.listener_raw_fd()).unwrap_or(-1);
        let proxy = self.proxy.clone();
        let stop = Arc::new(AtomicBool::new(false));
        self.watcher_stop = Some(stop.clone());
        self.watcher_started = true;

        spawn_pty_watcher("pty-watcher", pty_fd, ipc_fd, proxy, AppEvent::PtyReadable, stop);
    }

    fn create_window_attributes(&self, atlas: &GlyphAtlas) -> WindowAttributes {
        let width = self.config.window.columns as f64 * atlas.cell_width as f64;
        let height = self.config.window.rows as f64 * atlas.cell_height as f64;

        Window::default_attributes()
            .with_title("handterm")
            .with_name("handterm", "handterm")
            .with_transparent(false)
            .with_inner_size(Size::Logical(LogicalSize::new(width, height)))
    }
}

impl ApplicationHandler<AppEvent> for HandtermApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let cols = self.config.window.columns;
        let rows = self.config.window.rows;

        let probe_window = event_loop
            .create_window(Window::default_attributes().with_visible(false))
            .expect("probe window should succeed");
        let scale_factor = probe_window.scale_factor();
        drop(probe_window);

        let dpi = (96.0 * scale_factor) as u32;

        let atlas = GlyphAtlas::with_family_dpi(
            &self.config.style.font_family,
            self.config.style.font_size,
            dpi,
        )
        .or_else(|_| GlyphAtlas::new_with_dpi(self.config.style.font_size, dpi))
        .expect("failed to load font atlas");

        let window = Arc::new(
            event_loop
                .create_window(self.create_window_attributes(&atlas))
                .expect("window creation should succeed"),
        );

        let context =
            SoftContext::new(window.clone()).expect("softbuffer context should be created");
        let surface =
            Surface::new(&context, window.clone()).expect("softbuffer surface should be created");

        let terminal = Terminal::new(cols, rows);
        let pty = PtyChild::spawn_default_shell(cols, rows).expect("pty should spawn");

        self.state = Some(AppState {
            window,
            _context: context,
            surface,
            terminal,
            pty,
            pty_buf: vec![0u8; 64 * 1024],
            atlas,
            pty_closed: false,
            modifiers: Modifiers::default(),
            mouse_col: 0,
            mouse_row: 0,
            selecting: false,
            last_visual_state: None,
        });

        if let Some(s) = &self.state {
            s.window.request_redraw();
        }

        self.start_pty_watcher();
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::PtyReadable => {
                self.scheduler.mark_io_ready(Instant::now(), FRAME_INTERVAL);
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(state) = self.state.as_mut() else {
            return;
        };

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let (Some(width), Some(height)) =
                    (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
                {
                    state
                        .surface
                        .resize(width, height)
                        .expect("surface resize should succeed");

                    let new_cols = (width.get() as usize / state.atlas.cell_width.max(1)) as u16;
                    let new_rows = (height.get() as usize / state.atlas.cell_height.max(1)) as u16;
                    let new_cols = new_cols.max(1);
                    let new_rows = new_rows.max(1);

                    if new_cols != state.terminal.cols || new_rows != state.terminal.rows {
                        state.terminal.resize(new_cols, new_rows);
                        let _ = state.pty.resize(new_cols, new_rows);
                    }

                    state.window.request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(new_modifiers) => {
                state.modifiers = new_modifiers;
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    let ctrl = state.modifiers.state().control_key();
                    let shift = state.modifiers.state().shift_key();

                    if ctrl && shift {
                        if let Key::Character(s) = &event.logical_key {
                            let ch = s.chars().next().unwrap_or('\0').to_ascii_lowercase();
                            if ch == 'v' {
                                if let Some(text) = paste_from_clipboard() {
                                    if state.terminal.bracketed_paste_mode() {
                                        let _ = state.pty.write_all(b"\x1b[200~");
                                        let _ = state.pty.write_all(&text);
                                        let _ = state.pty.write_all(b"\x1b[201~");
                                    } else {
                                        let _ = state.pty.write_all(&text);
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
                    }

                    if shift {
                        if let Key::Named(NamedKey::PageUp) = &event.logical_key {
                            let max = state.terminal.grid.scrollback_len();
                            let half = state.terminal.rows as usize / 2;
                            state.terminal.grid.scroll_offset = (state.terminal.grid.scroll_offset + half).min(max);
                            self.scheduler.mark_redraw_needed();
                            return;
                        }
                        if let Key::Named(NamedKey::PageDown) = &event.logical_key {
                            let half = state.terminal.rows as usize / 2;
                            state.terminal.grid.scroll_offset = state.terminal.grid.scroll_offset.saturating_sub(half);
                            self.scheduler.mark_redraw_needed();
                            return;
                        }
                    }

                    if let Some(bytes) =
                        key_to_bytes(&event.logical_key, state.terminal.application_cursor_keys, ctrl)
                    {
                        let _ = state.pty.write_all(&bytes);
                        if state.terminal.grid.scroll_offset > 0 {
                            state.terminal.grid.scroll_offset = 0;
                            state.terminal.grid.all_dirty = true;
                        }
                        state.terminal.grid.selection = None;
                    }
                }
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
            WindowEvent::MouseInput { state: btn_state, button, .. } => {
                let btn = match button {
                    MouseButton::Left => 0u8,
                    MouseButton::Middle => 1,
                    MouseButton::Right => 2,
                    _ => return,
                };
                let pressed = btn_state == ElementState::Pressed;

                if btn == 0 {
                    if pressed {
                        let ctrl = state.modifiers.state().control_key();
                        if ctrl {
                            let cell = state.terminal.grid.cell_at(state.mouse_row, state.mouse_col);
                            if cell.hyperlink_id != 0 {
                                if let Some(url) = state.terminal.grid.hyperlink_url(cell.hyperlink_id) {
                                    open_url(url);
                                    return;
                                }
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
                    if let Some(bytes) = state.terminal.encode_mouse(btn, state.mouse_col, state.mouse_row, pressed) {
                        let _ = state.pty.write_all(&bytes);
                    }
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
                        if let Some(bytes) = state.terminal.encode_mouse_scroll(up, state.mouse_col, state.mouse_row) {
                            let _ = state.pty.write_all(&bytes);
                        }
                    }
                } else if state.terminal.alternate_scroll_mode() && state.terminal.in_alt_screen() {
                    let bytes = scroll_to_bytes(up, state.terminal.application_cursor_keys);
                    for _ in 0..lines {
                        let _ = state.pty.write_all(&bytes);
                    }
                } else {
                    let max = state.terminal.grid.scrollback_len();
                    if up {
                        state.terminal.grid.scroll_offset = (state.terminal.grid.scroll_offset + lines * 3).min(max);
                    } else {
                        state.terminal.grid.scroll_offset = state.terminal.grid.scroll_offset.saturating_sub(lines * 3);
                    }
                    self.scheduler.mark_redraw_needed();
                }
            }
            WindowEvent::Focused(focused) => {
                if state.terminal.focus_events_mode() {
                    if focused {
                        let _ = state.pty.write_all(b"\x1b[I");
                    } else {
                        let _ = state.pty.write_all(b"\x1b[O");
                    }
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
            let decision = self.scheduler.prepare_redraw(Instant::now(), || {
                process_pending_io(state, self.ipc.as_mut(), event_loop)
            });
            if let Some(deadline) = decision.wait_until {
                event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
            } else {
                event_loop.set_control_flow(ControlFlow::Wait);
            }
            if decision.request_redraw {
                state.window.request_redraw();
            }
            if state.pty_closed {
                if let Some(stop) = &self.watcher_stop {
                    stop.store(true, Ordering::Relaxed);
                }
                event_loop.exit();
            }
        }
    }
}

fn process_pending_io(
    state: &mut AppState,
    ipc: Option<&mut IpcServer>,
    event_loop: &ActiveEventLoop,
) -> bool {
    let needs_redraw = drain_pty(state) > 0;

    if let Some(ipc) = ipc {
        let actions = ipc.poll(&mut |req| handle_ipc_request(&mut state.terminal, req));
        for action in actions {
            match action {
                IpcAction::SendText(bytes) => {
                    let _ = state.pty.write_all(&bytes);
                }
                IpcAction::SetTitle(title) => {
                    state.window.set_title(&title);
                }
                IpcAction::Close => {
                    event_loop.exit();
                }
                IpcAction::None => {}
            }
        }
    }

    needs_redraw
}

fn drain_pty(state: &mut AppState) -> usize {
    if state.pty_closed {
        return 0;
    }
    let mut total = 0;
    loop {
        match state.pty.try_read(&mut state.pty_buf) {
            Ok(0) => break,
            Ok(n) => {
                state.terminal.process(&state.pty_buf[..n]);
                total += n;
            }
            Err(_) => {
                state.pty_closed = true;
                break;
            }
        }
    }
    if total > 0 {
        if let Some(resp) = state.terminal.drain_responses() {
            let _ = state.pty.write_all(&resp);
        }
        if let Some(title) = state.terminal.take_title() {
            state.window.set_title(&title);
        }
        if let Some(b64_data) = state.terminal.take_osc52_clipboard() {
            if let Ok(decoded) = base64_decode(&b64_data) {
                copy_to_clipboard(&decoded);
            }
        }
    }
    total
}

fn render_grid(state: &mut AppState, config: &AppConfig) -> Result<()> {
    let size = state.window.inner_size();
    let (Some(width), Some(height)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
    else {
        return Ok(());
    };

    state
        .surface
        .resize(width, height)
        .map_err(|e| anyhow::anyhow!("failed to resize backbuffer: {e}"))?;

    let mut buffer = state
        .surface
        .buffer_mut()
        .map_err(|e| anyhow::anyhow!("failed to acquire backbuffer: {e}"))?;

    let buf_w = buffer.width().get() as usize;
    let buf_h = buffer.height().get() as usize;
    render_terminal_to_buffer(
        &mut buffer,
        buf_w,
        buf_h,
        &mut state.terminal,
        &mut state.atlas,
        config,
        &mut state.last_visual_state,
    );

    buffer
        .present()
        .map_err(|e| anyhow::anyhow!("failed presenting frame: {e}"))?;
    Ok(())
}
