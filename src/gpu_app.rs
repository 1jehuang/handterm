use crate::config::AppConfig;
use crate::font::GlyphAtlas;
use crate::frontend::{
    FrameDecision, FrameScheduler, KeyEventKind, RedrawWork, base64_decode,
    classify_redraw_work, copy_to_clipboard, key_to_bytes, open_url, paste_from_clipboard,
    scroll_to_bytes, spawn_fd_watcher,
};
use crate::gpu_runtime::{
    GpuSurfaceState, SharedGpuContext, create_shared_gpu_context, create_surface_state_with_shared,
    render_surface_state, resize_surface_state,
};
use crate::ipc::{IpcAction, IpcServer, Request, Response};
use crate::pty::PtyChild;
use crate::standalone_support::handle_ipc_request;
use crate::terminal::Terminal;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, Ime, Modifiers, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::WindowId as WinitWindowId;

#[derive(Debug, Clone)]
enum GpuAppEvent {
    PtyReadable(u64),
    IpcReadable,
}

const FRAME_INTERVAL: Duration = Duration::from_millis(8);

pub fn run(config: AppConfig) -> Result<()> {
    let event_loop = EventLoop::<GpuAppEvent>::with_user_event()
        .build()
        .context("failed to create event loop")?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let proxy = event_loop.create_proxy();
    let socket_path = crate::ipc::default_socket_path_for_backend(crate::backend::Backend::Gpu);
    let ipc = IpcServer::bind(&socket_path).ok();
    eprintln!(
        "{}",
        crate::build_info::startup_banner(crate::backend::Backend::Gpu, Some(&socket_path))
    );
    if let Some(ref ipc) = ipc {
        eprintln!("handterm gpu host listening on {}", ipc.path().display());
    } else {
        eprintln!("handterm: failed to bind {}", socket_path.display());
    }

    let shared = create_shared_gpu_context()?;
    let mut app = GpuApp::new(config, ipc, proxy, shared);
    event_loop
        .run_app(&mut app)
        .context("failed while running app")
}

struct GpuApp {
    config: AppConfig,
    shared: Arc<SharedGpuContext>,
    windows: HashMap<WinitWindowId, GpuWindowState>,
    window_ids: HashMap<u64, WinitWindowId>,
    next_window_id: u64,
    focused_window: Option<u64>,
    ipc: Option<IpcServer>,
    proxy: EventLoopProxy<GpuAppEvent>,
    ipc_watcher_started: bool,
    ipc_watcher_stop: Option<Arc<AtomicBool>>,
    atlas_cache: HashMap<u32, GlyphAtlas>,
}

struct GpuWindowState {
    id: u64,
    renderer: GpuSurfaceState,
    terminal: Terminal,
    pty: PtyChild,
    pty_buf: Vec<u8>,
    dpi: u32,
    pty_closed: bool,
    modifiers: Modifiers,
    mouse_col: usize,
    mouse_row: usize,
    selecting: bool,
    scheduler: FrameScheduler,
    watcher_stop: Arc<AtomicBool>,
}

impl GpuApp {
    fn new(
        config: AppConfig,
        ipc: Option<IpcServer>,
        proxy: EventLoopProxy<GpuAppEvent>,
        shared: Arc<SharedGpuContext>,
    ) -> Self {
        Self {
            config,
            shared,
            windows: HashMap::new(),
            window_ids: HashMap::new(),
            next_window_id: 1,
            focused_window: None,
            ipc,
            proxy,
            ipc_watcher_started: false,
            ipc_watcher_stop: None,
            atlas_cache: HashMap::new(),
        }
    }

    fn start_ipc_watcher(&mut self) {
        if self.ipc_watcher_started {
            return;
        }
        let Some(ipc) = &self.ipc else { return };

        let stop = Arc::new(AtomicBool::new(false));
        self.ipc_watcher_stop = Some(stop.clone());
        self.ipc_watcher_started = true;
        spawn_fd_watcher(
            "handterm-gpu-ipc-watcher",
            ipc.listener_raw_fd(),
            -1,
            self.proxy.clone(),
            GpuAppEvent::IpcReadable,
            stop,
        );
    }

    fn ensure_initial_window(&mut self, event_loop: &ActiveEventLoop) {
        if self.windows.is_empty() {
            self.open_window(event_loop, None, None)
                .expect("initial gpu host window should open");
        }
        self.start_ipc_watcher();
    }

    fn resolve_dpi(&self, event_loop: &ActiveEventLoop) -> u32 {
        if let Some(id) = self.focused_window
            && let Some(winit_id) = self.window_ids.get(&id)
            && let Some(state) = self.windows.get(winit_id)
        {
            return (96.0 * state.renderer.window.scale_factor()) as u32;
        }
        if let Some(state) = self.windows.values().next() {
            return (96.0 * state.renderer.window.scale_factor()) as u32;
        }
        let probe_window = event_loop
            .create_window(winit::window::Window::default_attributes().with_visible(false))
            .expect("probe window should succeed");
        let dpi = (96.0 * probe_window.scale_factor()) as u32;
        drop(probe_window);
        dpi
    }

    fn ensure_atlas(&mut self, dpi: u32) -> Result<()> {
        if self.atlas_cache.contains_key(&dpi) {
            return Ok(());
        }
        let atlas = GlyphAtlas::with_family_dpi(
            &self.config.style.font_family,
            self.config.style.font_size,
            dpi,
        )
        .or_else(|_| GlyphAtlas::new_with_dpi(self.config.style.font_size, dpi))
        .context("failed to load font atlas")?;
        self.atlas_cache.insert(dpi, atlas);
        Ok(())
    }

    fn open_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        cols: Option<u16>,
        rows: Option<u16>,
    ) -> Result<u64> {
        let cols = cols.unwrap_or(self.config.window.columns).max(1);
        let rows = rows.unwrap_or(self.config.window.rows).max(1);
        let dpi = self.resolve_dpi(event_loop);
        self.ensure_atlas(dpi)?;
        let atlas = self.atlas_cache.get(&dpi).expect("atlas should exist for dpi");
        let renderer = create_surface_state_with_shared(
            self.shared.clone(),
            event_loop,
            &self.config,
            "handterm [gpu host]",
            atlas,
        )
        .expect("gpu surface state should initialize");
        eprintln!("handterm: {}", renderer.surface_debug_summary());
        let terminal = Terminal::new_with_scrollback(cols, rows, self.config.scrollback.lines);
        let pty = PtyChild::spawn_default_shell(cols, rows).expect("pty should spawn");
        let stop = Arc::new(AtomicBool::new(false));
        let id = self.next_window_id;
        self.next_window_id += 1;

        spawn_fd_watcher(
            &format!("handterm-gpu-pty-{id}"),
            pty.raw_fd(),
            -1,
            self.proxy.clone(),
            GpuAppEvent::PtyReadable(id),
            stop.clone(),
        );

        let winit_id = renderer.window.id();
        self.window_ids.insert(id, winit_id);
        self.focused_window = Some(id);
        self.windows.insert(
            winit_id,
            GpuWindowState {
                id,
                renderer,
                terminal,
                pty,
                pty_buf: vec![0u8; 64 * 1024],
                dpi,
                pty_closed: false,
                modifiers: Modifiers::default(),
                mouse_col: 0,
                mouse_row: 0,
                selecting: false,
                scheduler: FrameScheduler::default(),
                watcher_stop: stop,
            },
        );
        if let Some(state) = self.windows.get(&winit_id) {
            state.renderer.window.request_redraw();
        }
        Ok(id)
    }

    fn close_window(&mut self, winit_id: WinitWindowId, event_loop: &ActiveEventLoop) {
        if let Some(state) = self.windows.remove(&winit_id) {
            state.watcher_stop.store(true, Ordering::Relaxed);
            self.window_ids.remove(&state.id);
            if self.focused_window == Some(state.id) {
                self.focused_window = self.windows.values().next().map(|other| other.id);
            }
        }
        if self.windows.is_empty() {
            if let Some(stop) = &self.ipc_watcher_stop {
                stop.store(true, Ordering::Relaxed);
            }
            event_loop.exit();
        }
    }

    fn resolve_target_window_id(&self, requested: Option<u64>) -> Option<u64> {
        requested
            .or(self.focused_window)
            .or_else(|| self.windows.values().next().map(|state| state.id))
    }

    fn target_window_from_args(req: &Request) -> Option<u64> {
        req.args
            .as_object()
            .and_then(|o| o.get("window_id"))
            .and_then(|v| v.as_u64())
    }

    fn handle_host_ipc_request(&mut self, req: &Request) -> (Response, IpcAction) {
        if req.cmd == "open-window" {
            let cols = req
                .args
                .as_object()
                .and_then(|o| o.get("cols"))
                .and_then(|v| v.as_u64())
                .and_then(|v| u16::try_from(v).ok());
            let rows = req
                .args
                .as_object()
                .and_then(|o| o.get("rows"))
                .and_then(|v| v.as_u64())
                .and_then(|v| u16::try_from(v).ok());
            return (Response::ok_empty(), IpcAction::OpenWindow { cols, rows });
        }

        if req.cmd == "focus-window" {
            let Some(window_id) = Self::target_window_from_args(req) else {
                return (
                    Response::err("missing 'window_id' argument"),
                    IpcAction::None,
                );
            };
            return (Response::ok_empty(), IpcAction::FocusWindow(window_id));
        }

        let Some(target_id) = self.resolve_target_window_id(Self::target_window_from_args(req)) else {
            return (Response::err("no target window available"), IpcAction::None);
        };
        let Some(winit_id) = self.window_ids.get(&target_id).copied() else {
            return (Response::err("unknown target window"), IpcAction::None);
        };
        let Some(state) = self.windows.get_mut(&winit_id) else {
            return (Response::err("target window is not active"), IpcAction::None);
        };
        handle_ipc_request(&mut state.terminal, req)
    }

    fn process_ipc_actions(&mut self, event_loop: &ActiveEventLoop) {
        let Some(mut ipc) = self.ipc.take() else { return };
        let actions = ipc.poll(&mut |req| self.handle_host_ipc_request(req));
        self.ipc = Some(ipc);

        for action in actions {
            match action {
                IpcAction::None => {}
                IpcAction::OpenWindow { cols, rows } => {
                    let _ = self.open_window(event_loop, cols, rows);
                }
                IpcAction::FocusWindow(window_id) => {
                    if let Some(winit_id) = self.window_ids.get(&window_id)
                        && let Some(state) = self.windows.get(winit_id)
                    {
                        state.renderer.window.focus_window();
                        self.focused_window = Some(window_id);
                    }
                }
                IpcAction::SendText { window, bytes } => {
                    if let Some(id) = self.resolve_target_window_id(window)
                        && let Some(winit_id) = self.window_ids.get(&id)
                        && let Some(state) = self.windows.get_mut(winit_id)
                    {
                        let _ = state.pty.write_all(&bytes);
                    }
                }
                IpcAction::SetTitle { window, title } => {
                    if let Some(id) = self.resolve_target_window_id(window)
                        && let Some(winit_id) = self.window_ids.get(&id)
                        && let Some(state) = self.windows.get(winit_id)
                    {
                        state.renderer.window.set_title(&title);
                    }
                }
                IpcAction::Close { window } => {
                    if let Some(id) = self.resolve_target_window_id(window)
                        && let Some(winit_id) = self.window_ids.get(&id).copied()
                    {
                        self.close_window(winit_id, event_loop);
                    }
                }
            }
        }
    }
}

impl ApplicationHandler<GpuAppEvent> for GpuApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.ensure_initial_window(event_loop);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: GpuAppEvent) {
        match event {
            GpuAppEvent::PtyReadable(window_id) => {
                if let Some(winit_id) = self.window_ids.get(&window_id)
                    && let Some(state) = self.windows.get_mut(winit_id)
                {
                    state.scheduler.mark_io_ready(Instant::now(), FRAME_INTERVAL);
                }
            }
            GpuAppEvent::IpcReadable => self.process_ipc_actions(event_loop),
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WinitWindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => self.close_window(window_id, event_loop),
            _ => {
                let atlas_cache = &mut self.atlas_cache;
                let windows = &mut self.windows;
                let focused_window = &mut self.focused_window;
                let Some(state) = windows.get_mut(&window_id) else {
                    return;
                };
                let atlas = atlas_cache
                    .get_mut(&state.dpi)
                    .expect("atlas should exist for window dpi");

                match event {
                    WindowEvent::Resized(size) => {
                        if size.width > 0 && size.height > 0 {
                            let new_cols = (size.width as usize / atlas.cell_width.max(1)) as u16;
                            let new_rows = (size.height as usize / atlas.cell_height.max(1)) as u16;
                            let new_cols = new_cols.max(1);
                            let new_rows = new_rows.max(1);

                            resize_surface_state(
                                &mut state.renderer,
                                atlas,
                                size.width,
                                size.height,
                                new_cols,
                                new_rows,
                            );

                            if new_cols != state.terminal.cols || new_rows != state.terminal.rows {
                                state.terminal.resize(new_cols, new_rows);
                                let _ = state.pty.resize(new_cols, new_rows);
                            }

                            state.renderer.window.request_redraw();
                        }
                    }
                    WindowEvent::ModifiersChanged(new_modifiers) => {
                        state.modifiers = new_modifiers;
                    }
                    WindowEvent::Ime(Ime::Commit(text)) => {
                        if !text.is_empty() {
                            let _ = state.pty.write_all(text.as_bytes());
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

                            if ctrl && shift && let Key::Character(s) = &event.logical_key {
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
                                }
                                if ch == 'c' {
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
                                    state.scheduler.mark_redraw_needed();
                                    return;
                                }
                                if let Key::Named(NamedKey::PageDown) = &event.logical_key {
                                    let half = state.terminal.rows as usize / 2;
                                    state.terminal.grid.scroll_offset = state
                                        .terminal
                                        .grid
                                        .scroll_offset
                                        .saturating_sub(half);
                                    state.scheduler.mark_redraw_needed();
                                    return;
                                }
                            }
                        }

                        let event_kind = match (event.state, event.repeat) {
                            (ElementState::Pressed, true) => KeyEventKind::Repeat,
                            (ElementState::Pressed, false) => KeyEventKind::Press,
                            (ElementState::Released, _) => KeyEventKind::Release,
                        };

                        if let Some(bytes) = key_to_bytes(
                            &event.logical_key,
                            event.text.as_deref(),
                            Some(&event.physical_key),
                            state.terminal.application_cursor_keys,
                            state.modifiers.state(),
                            state.terminal.kitty_keyboard_flags(),
                            event_kind,
                        ) {
                            let _ = state.pty.write_all(&bytes);
                            if state.terminal.grid.scroll_offset > 0 {
                                state.terminal.grid.scroll_offset = 0;
                                state.terminal.grid.all_dirty = true;
                            }
                            state.terminal.grid.selection = None;
                        }
                    }
                    WindowEvent::CursorMoved { position, .. } => {
                        let cw = atlas.cell_width.max(1);
                        let ch = atlas.cell_height.max(1);
                        state.mouse_col = position.x as usize / cw;
                        state.mouse_row = position.y as usize / ch;

                        if state.selecting {
                            if let Some(ref mut sel) = state.terminal.grid.selection {
                                sel.end_col = state.mouse_col;
                                sel.end_row = state.mouse_row;
                            }
                            state.scheduler.mark_redraw_needed();
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
                                state.scheduler.mark_redraw_needed();
                            } else {
                                state.selecting = false;
                                let text = state.terminal.grid.get_selection_text();
                                if !text.is_empty() {
                                    copy_to_clipboard(text.as_bytes());
                                }
                            }
                        }

                        if state.terminal.mouse_mode != crate::terminal::MouseMode::Off
                            && let Some(bytes) = state
                                .terminal
                                .encode_mouse(btn, state.mouse_col, state.mouse_row, pressed)
                        {
                            let _ = state.pty.write_all(&bytes);
                        }
                    }
                    WindowEvent::MouseWheel { delta, .. } => {
                        let (up, lines) = match delta {
                            MouseScrollDelta::LineDelta(_, y) => (y > 0.0, y.abs().max(1.0) as usize),
                            MouseScrollDelta::PixelDelta(pos) => {
                                let ch = atlas.cell_height.max(1) as f64;
                                (pos.y > 0.0, (pos.y.abs() / ch).max(1.0) as usize)
                            }
                        };
                        if state.terminal.mouse_mode != crate::terminal::MouseMode::Off {
                            for _ in 0..lines {
                                if let Some(bytes) = state
                                    .terminal
                                    .encode_mouse_scroll(up, state.mouse_col, state.mouse_row)
                                {
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
                                state.terminal.grid.scroll_offset =
                                    (state.terminal.grid.scroll_offset + lines * 3).min(max);
                            } else {
                                state.terminal.grid.scroll_offset = state
                                    .terminal
                                    .grid
                                    .scroll_offset
                                    .saturating_sub(lines * 3);
                            }
                            state.scheduler.mark_redraw_needed();
                        }
                    }
                    WindowEvent::Focused(focused) => {
                        if focused {
                            *focused_window = Some(state.id);
                        }
                        if state.terminal.focus_events_mode() {
                            let _ = if focused {
                                state.pty.write_all(b"\x1b[I")
                            } else {
                                state.pty.write_all(b"\x1b[O")
                            };
                        }
                    }
                    WindowEvent::RedrawRequested => {
                        render_surface_state(
                            &mut state.renderer,
                            &mut state.terminal,
                            atlas,
                            &self.config,
                        );
                    }
                    _ => {}
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let mut earliest_deadline: Option<Instant> = None;
        let mut redraw_ids = Vec::new();
        let mut closed_windows = Vec::new();

        for (winit_id, state) in &mut self.windows {
            let mut scheduler = std::mem::take(&mut state.scheduler);
            let decision: FrameDecision =
                scheduler.prepare_redraw(Instant::now(), || process_pending_pty_io(state));
            state.scheduler = scheduler;
            if let Some(deadline) = decision.wait_until {
                earliest_deadline = Some(match earliest_deadline {
                    Some(existing) => existing.min(deadline),
                    None => deadline,
                });
            }
            if decision.request_redraw {
                redraw_ids.push(*winit_id);
            }
            if state.pty_closed {
                closed_windows.push(*winit_id);
            }
        }

        for winit_id in redraw_ids {
            if let Some(state) = self.windows.get(&winit_id) {
                state.renderer.window.request_redraw();
            }
        }

        for winit_id in closed_windows {
            self.close_window(winit_id, event_loop);
        }

        if self.windows.is_empty() {
            event_loop.set_control_flow(ControlFlow::Wait);
        } else if let Some(deadline) = earliest_deadline {
            event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
}

fn process_pending_pty_io(state: &mut GpuWindowState) -> RedrawWork {
    let needs_redraw = drain_pty(state) > 0;
    classify_redraw_work(&state.terminal, needs_redraw)
}

fn drain_pty(state: &mut GpuWindowState) -> usize {
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
            state.renderer.window.set_title(&title);
        }
        if let Some(b64_data) = state.terminal.take_osc52_clipboard()
            && let Ok(decoded) = base64_decode(&b64_data)
        {
            copy_to_clipboard(&decoded);
        }
    }

    total
}
