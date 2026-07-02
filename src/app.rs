use crate::config::AppConfig;
use crate::fd_watcher::spawn_fd_watcher;
use crate::font::{GlyphAtlas, bootstrap_font_metrics_with_family_dpi};
use crate::frontend::{
    FrameDecision, FrameScheduler, KeyEventKind, RecentTextKeyEvent, RedrawWork, StartupTiming,
    VisualState, base64_decode, classify_redraw_work, key_to_bytes, remember_text_key_event,
    scroll_to_bytes, scrollback_wheel_delta, should_skip_duplicate_ime_input,
    should_skip_ime_commit_after_key_event, visual_signature,
};
use crate::host_commands::{
    HostControlRequest, host_list_windows_response, host_ls_response, parse_host_control_request,
    target_window_from_args,
};
use crate::host_input::{
    SyntheticInputTarget, apply_synthetic_ime_commit, apply_synthetic_key_event,
};
use crate::ipc::{IpcAction, IpcServer, Request, Response};
use crate::native_scroll::NativeScrollBridge;
use crate::platform::{copy_to_clipboard, open_url, paste_from_clipboard};
use crate::profiling::{ProcessCpuTime, emit_structured_profile_event};
use crate::pty::PtyChild;
use crate::render::render_terminal_to_buffer;
use crate::standalone_support::handle_ipc_request;
use crate::terminal::Terminal;
use anyhow::{Context, Result};
use serde_json::json;
use softbuffer::{Context as SoftContext, Surface};
use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, Size};
use winit::event::{ElementState, Ime, Modifiers, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::{ImePurpose, Window, WindowAttributes, WindowId as WinitWindowId};

#[derive(Debug, Clone)]
enum AppEvent {
    PtyReadable(u64),
    IpcReadable,
}

const FRAME_INTERVAL: Duration = Duration::from_millis(8);

pub fn run(config: AppConfig, startup_command: Option<String>) -> Result<()> {
    let event_loop = EventLoop::<AppEvent>::with_user_event()
        .build()
        .context("failed to create event loop")?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let proxy = event_loop.create_proxy();

    let socket_path = crate::ipc::default_socket_path();
    let ipc = IpcServer::bind(&socket_path).ok();
    eprintln!(
        "{}",
        crate::build_info::startup_banner(crate::backend::Backend::Cpu, Some(&socket_path))
    );
    if let Some(ref ipc) = ipc {
        eprintln!("handterm host listening on {}", ipc.path().display());
    } else {
        eprintln!("handterm: failed to bind {}", socket_path.display());
    }

    let mut app = HandtermApp::new(config, startup_command, ipc, proxy);
    event_loop
        .run_app(&mut app)
        .context("failed while running app")
}

struct HandtermApp {
    config: AppConfig,
    startup_command: Option<String>,
    windows: HashMap<WinitWindowId, HostWindowState>,
    window_ids: HashMap<u64, WinitWindowId>,
    next_window_id: u64,
    focused_window: Option<u64>,
    ipc: Option<IpcServer>,
    proxy: EventLoopProxy<AppEvent>,
    ipc_watcher_started: bool,
    ipc_watcher_stop: Option<Arc<AtomicBool>>,
    atlas_cache: HashMap<u32, GlyphAtlas>,
}

struct HostWindowState {
    id: u64,
    window: Arc<Window>,
    _context: SoftContext<Arc<Window>>,
    surface: Surface<Arc<Window>, Arc<Window>>,
    surface_size: (u32, u32),
    terminal: Terminal,
    pty: PtyChild,
    pty_buf: Vec<u8>,
    dpi: u32,
    pty_closed: bool,
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
    last_visual_state: Option<VisualState>,
    last_presented_signature: Option<u64>,
    scheduler: FrameScheduler,
    watcher_stop: Arc<AtomicBool>,
    cpu_time_started: Option<ProcessCpuTime>,
    startup_timing: StartupTiming,
    native_scroll: Option<NativeScrollBridge>,
}

impl SyntheticInputTarget for HostWindowState {
    fn label(&self) -> &'static str {
        "cpu"
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
        &mut self.hyper_modifier
    }

    fn meta_modifier_mut(&mut self) -> &mut bool {
        &mut self.meta_modifier
    }

    fn caps_lock_modifier_mut(&mut self) -> &mut bool {
        &mut self.caps_lock_modifier
    }

    fn num_lock_modifier_mut(&mut self) -> &mut bool {
        &mut self.num_lock_modifier
    }

    fn caps_lock_modifier(&self) -> bool {
        self.caps_lock_modifier
    }

    fn num_lock_modifier(&self) -> bool {
        self.num_lock_modifier
    }

    fn apply_modifier_transition(&mut self, logical_key: &Key, event_kind: KeyEventKind) {
        crate::frontend::apply_modifier_key_transition(
            &mut self.hyper_modifier,
            &mut self.meta_modifier,
            &mut self.caps_lock_modifier,
            &mut self.num_lock_modifier,
            logical_key,
            event_kind,
        );
    }

    fn reset_scrollback(&mut self) {
        self.terminal.grid.scroll_offset = 0;
        self.terminal.grid.all_dirty = true;
    }

    fn drain_pty(&mut self) -> bool {
        drain_pty(self) > 0
    }
}

impl HandtermApp {
    fn new(
        config: AppConfig,
        startup_command: Option<String>,
        ipc: Option<IpcServer>,
        proxy: EventLoopProxy<AppEvent>,
    ) -> Self {
        Self {
            config,
            startup_command,
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
            "handterm-ipc-watcher",
            ipc.listener_raw_fd(),
            -1,
            self.proxy.clone(),
            AppEvent::IpcReadable,
            stop,
        );
    }

    fn ensure_initial_window(&mut self, event_loop: &ActiveEventLoop) {
        if self.windows.is_empty()
            && let Err(err) = self.open_window(event_loop, None, None)
        {
            eprintln!("handterm cpu host: failed to open initial window: {err:#}");
            event_loop.exit();
            return;
        }
        self.start_ipc_watcher();
    }

    fn resolve_dpi(&self, event_loop: &ActiveEventLoop) -> Result<u32> {
        if let Some(id) = self.focused_window
            && let Some(winit_id) = self.window_ids.get(&id)
            && let Some(state) = self.windows.get(winit_id)
        {
            return Ok((96.0 * state.window.scale_factor()) as u32);
        }
        if let Some(state) = self.windows.values().next() {
            return Ok((96.0 * state.window.scale_factor()) as u32);
        }
        let probe_window = event_loop
            .create_window(Window::default_attributes().with_visible(false))
            .context("failed to create invisible probe window while resolving display dpi")?;
        let dpi = (96.0 * probe_window.scale_factor()) as u32;
        drop(probe_window);
        Ok(dpi)
    }

    fn ensure_atlas(&mut self, dpi: u32) -> Result<()> {
        self.ensure_atlas_with_hint(dpi, None)
    }

    fn ensure_atlas_with_hint(&mut self, dpi: u32, font_path_hint: Option<&str>) -> Result<()> {
        if self.atlas_cache.contains_key(&dpi) {
            return Ok(());
        }
        let atlas = if let Some(path) = font_path_hint {
            GlyphAtlas::from_font_path_dpi(path, self.config.style.font_size, dpi)
        } else {
            GlyphAtlas::with_family_dpi(
                &self.config.style.font_family,
                self.config.style.font_size,
                dpi,
            )
            .or_else(|_| GlyphAtlas::new_with_dpi(self.config.style.font_size, dpi))
        }
        .context("failed to load font atlas")?;
        self.atlas_cache.insert(dpi, atlas);
        Ok(())
    }

    fn atlas_metrics(&self, dpi: u32) -> (usize, usize) {
        let atlas = self
            .atlas_cache
            .get(&dpi)
            .unwrap_or_else(|| panic!("atlas should exist for requested dpi {dpi}"));
        (atlas.cell_width.max(1), atlas.cell_height.max(1))
    }

    fn create_window_attributes(
        &self,
        cell_width: usize,
        cell_height: usize,
        cols: u16,
        rows: u16,
    ) -> WindowAttributes {
        let width = cols as f64 * cell_width as f64;
        let height = rows as f64 * cell_height as f64;

        let attrs = crate::platform::with_app_id(
            Window::default_attributes().with_title("handterm [cpu host]"),
            "handterm",
        )
        .with_transparent(false)
        .with_inner_size(Size::Logical(LogicalSize::new(width, height)));
        crate::platform::with_decorations(attrs, self.config.window.decorations)
    }

    fn open_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        cols: Option<u16>,
        rows: Option<u16>,
    ) -> Result<u64> {
        let start = Instant::now();
        let cpu_time_started = ProcessCpuTime::capture();
        let existing_windows = self.windows.len();
        let open_kind = if existing_windows == 0 {
            "first-window"
        } else {
            "add-window"
        };
        let cols = cols.unwrap_or(self.config.window.columns).max(1);
        let rows = rows.unwrap_or(self.config.window.rows).max(1);
        let before_dpi = Instant::now();
        let dpi = self.resolve_dpi(event_loop)?;
        let dpi_ms = before_dpi.elapsed();
        let before_bootstrap = Instant::now();
        let bootstrap = bootstrap_font_metrics_with_family_dpi(
            &self.config.style.font_family,
            self.config.style.font_size,
            dpi,
        )
        .ok();
        let bootstrap_ms = before_bootstrap.elapsed();
        let (cell_width, cell_height, font_path_hint) = if let Some(metrics) = bootstrap.as_ref() {
            (
                metrics.cell_width.max(1),
                metrics.cell_height.max(1),
                Some(metrics.font_path.as_str()),
            )
        } else {
            self.ensure_atlas(dpi)?;
            let (cell_width, cell_height) = self.atlas_metrics(dpi);
            (cell_width, cell_height, None)
        };
        let before_window = Instant::now();
        let window = Arc::new(
            event_loop
                .create_window(self.create_window_attributes(cell_width, cell_height, cols, rows))
                .context("window creation should succeed")?,
        );
        let window_ms = before_window.elapsed();
        window.set_ime_allowed(true);
        window.set_ime_purpose(ImePurpose::Terminal);

        let before_atlas = Instant::now();
        self.ensure_atlas_with_hint(dpi, font_path_hint)?;
        let atlas_ms = before_atlas.elapsed();

        let context = SoftContext::new(window.clone())
            .map_err(|e| anyhow::anyhow!("softbuffer context should be created: {e}"))?;
        let surface = Surface::new(&context, window.clone())
            .map_err(|e| anyhow::anyhow!("softbuffer surface should be created: {e}"))?;
        let terminal = Terminal::new_with_scrollback(cols, rows, self.config.scrollback.lines);
        let id = self.next_window_id;
        self.next_window_id += 1;
        let native_scroll = NativeScrollBridge::new(id).ok();
        let native_scroll_envs = native_scroll.as_ref().map(|bridge| bridge.child_envs(id));
        let native_scroll_env_refs = native_scroll_envs
            .as_ref()
            .map(|envs| {
                envs.iter()
                    .map(|(key, value)| (*key, value.as_str()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let before_pty = Instant::now();
        let pty = PtyChild::spawn_default_shell_with_command_and_env(
            cols,
            rows,
            self.startup_command.as_deref(),
            &native_scroll_env_refs,
        )
        .context("pty should spawn")?;
        let pty_spawned_at = Instant::now();
        let stop = Arc::new(AtomicBool::new(false));

        spawn_fd_watcher(
            &format!("handterm-pty-{id}"),
            pty.raw_fd(),
            -1,
            self.proxy.clone(),
            AppEvent::PtyReadable(id),
            stop.clone(),
        );

        let winit_id = window.id();
        self.window_ids.insert(id, winit_id);
        self.focused_window = Some(id);
        self.windows.insert(
            winit_id,
            HostWindowState {
                id,
                window,
                _context: context,
                surface,
                surface_size: (0, 0),
                terminal,
                pty,
                pty_buf: vec![0u8; 64 * 1024],
                dpi,
                pty_closed: false,
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
                last_visual_state: None,
                last_presented_signature: None,
                scheduler: FrameScheduler::default(),
                watcher_stop: stop,
                cpu_time_started,
                startup_timing: {
                    let mut timing = StartupTiming::new(start);
                    timing.mark_pty_spawned(pty_spawned_at);
                    timing
                },
                native_scroll,
            },
        );
        if let Some(state) = self.windows.get(&winit_id) {
            state.window.request_redraw();
        }
        let open_cpu = cpu_time_started.and_then(|started| {
            ProcessCpuTime::capture().map(|current| current.delta_since(started))
        });
        eprintln!(
            "handterm cpu host: open-window id={id}\n\
             \x20 total={:.2}ms dpi={:.2}ms bootstrap={:.2}ms window={:.2}ms atlas={:.2}ms pty={:.2}ms\n\
             \x20 host_cpu_user={:.2}ms host_cpu_system={:.2}ms host_cpu_total={:.2}ms",
            start.elapsed().as_secs_f64() * 1000.0,
            dpi_ms.as_secs_f64() * 1000.0,
            bootstrap_ms.as_secs_f64() * 1000.0,
            window_ms.as_secs_f64() * 1000.0,
            atlas_ms.as_secs_f64() * 1000.0,
            Instant::now().duration_since(before_pty).as_secs_f64() * 1000.0,
            open_cpu.map(ProcessCpuTime::user_ms).unwrap_or(0.0),
            open_cpu.map(ProcessCpuTime::system_ms).unwrap_or(0.0),
            open_cpu.map(ProcessCpuTime::total_ms).unwrap_or(0.0),
        );
        emit_structured_profile_event(
            "cpu_host_open_window",
            json!({
                "id": id,
                "kind": open_kind,
                "existing_windows": existing_windows,
                "total_ms": start.elapsed().as_secs_f64() * 1000.0,
                "dpi_ms": dpi_ms.as_secs_f64() * 1000.0,
                "bootstrap_ms": bootstrap_ms.as_secs_f64() * 1000.0,
                "window_ms": window_ms.as_secs_f64() * 1000.0,
                "atlas_ms": atlas_ms.as_secs_f64() * 1000.0,
                "pty_ms": Instant::now().duration_since(before_pty).as_secs_f64() * 1000.0,
                "host_cpu": {
                    "user_ms": open_cpu.map(ProcessCpuTime::user_ms).unwrap_or(0.0),
                    "system_ms": open_cpu.map(ProcessCpuTime::system_ms).unwrap_or(0.0),
                    "total_ms": open_cpu.map(ProcessCpuTime::total_ms).unwrap_or(0.0),
                }
            }),
        );
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

    fn handle_host_ipc_request(&mut self, req: &Request) -> (Response, IpcAction) {
        match parse_host_control_request(req) {
            Ok(Some(HostControlRequest::ListWindows)) => {
                let windows: Vec<u64> = self.windows.values().map(|state| state.id).collect();
                return (
                    host_list_windows_response(windows, self.focused_window),
                    IpcAction::None,
                );
            }
            Ok(Some(control)) => return crate::host_commands::into_ipc_action(control),
            Ok(None) => {}
            Err(response) => return (response, IpcAction::None),
        }

        if self.windows.is_empty() {
            return match req.cmd.as_str() {
                "ls" => (host_ls_response(true), IpcAction::None),
                _ => (Response::err("no open windows in host"), IpcAction::None),
            };
        }

        let requested = target_window_from_args(req);
        let Some(target_id) = self.resolve_target_window_id(requested) else {
            return (Response::err("no target window available"), IpcAction::None);
        };
        let Some(winit_id) = self.window_ids.get(&target_id).copied() else {
            return (Response::err("unknown target window"), IpcAction::None);
        };
        let Some(state) = self.windows.get_mut(&winit_id) else {
            return (
                Response::err("target window is not active"),
                IpcAction::None,
            );
        };

        if req.cmd == "get-scroll-state" {
            return (
                Response::ok(serde_json::json!({
                    "backend": "cpu",
                    "window_id": target_id,
                    "scroll_offset": state.terminal.grid.scroll_offset,
                    "scrollback_len": state.terminal.grid.scrollback_len(),
                    "rows": state.terminal.grid.rows,
                    "smooth_supported": false,
                    "native_scroll_connected": state.native_scroll.as_ref().map(|_| true).unwrap_or(false),
                })),
                IpcAction::None,
            );
        }

        if req.cmd == "apply-scroll-delta" {
            let delta_rows = req
                .args
                .as_object()
                .and_then(|o| o.get("delta_rows"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let steps = delta_rows.abs().ceil() as usize;
            let max = state.terminal.grid.scrollback_len();
            if delta_rows > 0.0 {
                state.terminal.grid.scroll_offset =
                    (state.terminal.grid.scroll_offset + steps).min(max);
            } else {
                state.terminal.grid.scroll_offset =
                    state.terminal.grid.scroll_offset.saturating_sub(steps);
            }
            state.scheduler.mark_redraw_needed();
            state.window.request_redraw();
            return (
                Response::ok(serde_json::json!({
                    "backend": "cpu",
                    "window_id": target_id,
                    "scroll_offset": state.terminal.grid.scroll_offset,
                    "scrollback_len": state.terminal.grid.scrollback_len(),
                    "rows": state.terminal.grid.rows,
                    "smooth_supported": false,
                })),
                IpcAction::None,
            );
        }

        handle_ipc_request(&mut state.terminal, req)
    }

    fn process_ipc_actions(&mut self, event_loop: &ActiveEventLoop) {
        let Some(mut ipc) = self.ipc.take() else {
            return;
        };
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
                        state.window.focus_window();
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
                        state.window.set_title(&title);
                    }
                }
                IpcAction::Close { window } => {
                    if let Some(id) = self.resolve_target_window_id(window)
                        && let Some(winit_id) = self.window_ids.get(&id).copied()
                    {
                        self.close_window(winit_id, event_loop);
                    }
                }
                IpcAction::SyntheticKeyEvent { window, event } => {
                    if let Some(id) = self.resolve_target_window_id(window)
                        && let Some(winit_id) = self.window_ids.get(&id)
                        && let Some(state) = self.windows.get_mut(winit_id)
                    {
                        let changed = apply_synthetic_key_event(state, &event);
                        let work = classify_redraw_work(&state.terminal, changed);
                        let should_redraw_now = if self.focused_window == Some(state.id) {
                            state.scheduler.mark_redraw_needed();
                            true
                        } else {
                            state
                                .scheduler
                                .mark_io_processed(Instant::now(), FRAME_INTERVAL, work)
                        };
                        if should_redraw_now {
                            state.window.request_redraw();
                        }
                    }
                }
                IpcAction::SyntheticImeCommit { window, text } => {
                    if let Some(id) = self.resolve_target_window_id(window)
                        && let Some(winit_id) = self.window_ids.get(&id)
                        && let Some(state) = self.windows.get_mut(winit_id)
                    {
                        let changed = apply_synthetic_ime_commit(state, &text);
                        let work = classify_redraw_work(&state.terminal, changed);
                        let should_redraw_now = if self.focused_window == Some(state.id) {
                            state.scheduler.mark_redraw_needed();
                            true
                        } else {
                            state
                                .scheduler
                                .mark_io_processed(Instant::now(), FRAME_INTERVAL, work)
                        };
                        if should_redraw_now {
                            state.window.request_redraw();
                        }
                    }
                }
            }
        }
    }
}

impl ApplicationHandler<AppEvent> for HandtermApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.ensure_initial_window(event_loop);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::PtyReadable(window_id) => {
                if let Some(winit_id) = self.window_ids.get(&window_id)
                    && let Some(state) = self.windows.get_mut(winit_id)
                {
                    state.startup_timing.mark_pty_event(Instant::now());
                    let bytes_read = drain_pty(state);
                    if bytes_read > 0 {
                        let work = classify_redraw_work(&state.terminal, true);
                        let should_redraw_now = if self.focused_window == Some(state.id) {
                            state.scheduler.mark_redraw_needed();
                            true
                        } else {
                            state
                                .scheduler
                                .mark_io_processed(Instant::now(), FRAME_INTERVAL, work)
                        };
                        if should_redraw_now {
                            state.window.request_redraw();
                        }
                    }
                    if state.pty_closed {
                        let winit_id = *winit_id;
                        self.close_window(winit_id, event_loop);
                    }
                }
            }
            AppEvent::IpcReadable => self.process_ipc_actions(event_loop),
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WinitWindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                self.close_window(window_id, event_loop);
            }
            _ => {
                let atlas_cache = &mut self.atlas_cache;
                let windows = &mut self.windows;
                let config = &self.config;
                let focused_window = &mut self.focused_window;
                let Some(state) = windows.get_mut(&window_id) else {
                    return;
                };

                let (cell_width, cell_height) = {
                    let Some(atlas) = atlas_cache.get(&state.dpi) else {
                        eprintln!(
                            "handterm cpu host: no glyph atlas cached for dpi {}; dropping window event",
                            state.dpi
                        );
                        return;
                    };
                    (atlas.cell_width.max(1), atlas.cell_height.max(1))
                };

                match event {
                    WindowEvent::Resized(size) => {
                        if let (Some(width), Some(height)) =
                            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
                        {
                            state.surface.resize(width, height).expect(
                                "softbuffer surface resize failed while handling window resize",
                            );
                            state.surface_size = (width.get(), height.get());
                            state.last_visual_state = None;
                            state.last_presented_signature = None;

                            let new_cols = (width.get() as usize / cell_width) as u16;
                            let new_rows = (height.get() as usize / cell_height) as u16;
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
                    WindowEvent::Ime(Ime::Commit(text)) if !text.is_empty() => {
                        let ime_commit_text = crate::frontend::normalize_ime_dedupe_text(&text)
                            .unwrap_or_else(|| text.clone());
                        crate::frontend::trace_input(format!(
                            "cpu ime-commit raw={:?} normalized={:?}",
                            text, ime_commit_text
                        ));
                        if should_skip_ime_commit_after_key_event(
                            &mut state.recent_text_key_event,
                            &ime_commit_text,
                            Instant::now(),
                        ) {
                            crate::frontend::trace_input(
                                "cpu ime-commit skipped after key-event dedupe",
                            );
                            return;
                        }
                        state.pending_ime_commit = Some(ime_commit_text);
                        let _ = state.pty.write_all(text.as_bytes());
                        if state.terminal.grid.scroll_offset > 0 {
                            state.terminal.grid.scroll_offset = 0;
                            state.terminal.grid.all_dirty = true;
                        }
                        state.terminal.grid.selection = None;
                        let changed = drain_pty(state) > 0;
                        let work = classify_redraw_work(&state.terminal, changed);
                        let should_redraw_now = if *focused_window == Some(state.id) {
                            state.scheduler.mark_redraw_needed();
                            true
                        } else {
                            state
                                .scheduler
                                .mark_io_processed(Instant::now(), FRAME_INTERVAL, work)
                        };
                        if should_redraw_now {
                            state.window.request_redraw();
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
                                    state.terminal.grid.scroll_offset =
                                        state.terminal.grid.scroll_offset.saturating_sub(half);
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

                        let ime_dedupe_text = crate::frontend::key_ime_dedupe_text(
                            &event.logical_key,
                            event.text.as_deref(),
                        );

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
                            crate::frontend::trace_input(format!(
                                "cpu key-event kind={:?} key={:?} text={:?} dedupe_text={:?} bytes={:?}",
                                event_kind, event.logical_key, event.text, ime_dedupe_text, bytes
                            ));
                            if should_skip_duplicate_ime_input(
                                &mut state.pending_ime_commit,
                                event_kind,
                                ime_dedupe_text.as_deref(),
                                Some(&bytes),
                            ) {
                                crate::frontend::trace_input("cpu key-event skipped by ime dedupe");
                                return;
                            }
                            remember_text_key_event(
                                &mut state.recent_text_key_event,
                                event_kind,
                                ime_dedupe_text.as_deref(),
                                Some(&bytes),
                                Instant::now(),
                            );
                            let _ = state.pty.write_all(&bytes);
                            if state.terminal.grid.scroll_offset > 0 {
                                state.terminal.grid.scroll_offset = 0;
                                state.terminal.grid.all_dirty = true;
                            }
                            state.terminal.grid.selection = None;
                            let changed = drain_pty(state) > 0;
                            let work = classify_redraw_work(&state.terminal, changed);
                            let should_redraw_now = if *focused_window == Some(state.id) {
                                state.scheduler.mark_redraw_needed();
                                true
                            } else {
                                state.scheduler.mark_io_processed(
                                    Instant::now(),
                                    FRAME_INTERVAL,
                                    work,
                                )
                            };
                            if should_redraw_now {
                                state.window.request_redraw();
                            }
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
                        state.mouse_col = position.x as usize / cell_width;
                        state.mouse_row = position.y as usize / cell_height;

                        if state.selecting {
                            if let Some(ref mut sel) = state.terminal.grid.selection {
                                sel.end_col = state.mouse_col;
                                sel.end_row = state.mouse_row;
                            }
                            state.scheduler.mark_redraw_needed();
                        }
                    }
                    WindowEvent::MouseInput {
                        state: btn_state,
                        button,
                        ..
                    } => {
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
                            && let Some(bytes) = state.terminal.encode_mouse(
                                btn,
                                state.mouse_col,
                                state.mouse_row,
                                pressed,
                            )
                        {
                            let _ = state.pty.write_all(&bytes);
                        }
                    }
                    WindowEvent::MouseWheel { delta, .. } => {
                        let (up, lines) = match delta {
                            MouseScrollDelta::LineDelta(_, y) => {
                                (y > 0.0, y.abs().max(1.0) as usize)
                            }
                            MouseScrollDelta::PixelDelta(pos) => {
                                let ch = cell_height as f64;
                                (pos.y > 0.0, (pos.y.abs() / ch).max(1.0) as usize)
                            }
                        };
                        if let Some(bridge) = state.native_scroll.as_mut()
                            && let Some(pane) =
                                bridge.hovered_pane(state.mouse_col, state.mouse_row)
                            && bridge.send_scroll_delta(
                                pane,
                                if up { -(lines as f32) } else { lines as f32 },
                            )
                        {
                            return;
                        }
                        if state.terminal.mouse_mode != crate::terminal::MouseMode::Off {
                            for _ in 0..lines {
                                if let Some(bytes) = state.terminal.encode_mouse_scroll(
                                    up,
                                    state.mouse_col,
                                    state.mouse_row,
                                ) {
                                    let _ = state.pty.write_all(&bytes);
                                }
                            }
                        } else if state.terminal.alternate_scroll_mode()
                            && state.terminal.in_alt_screen()
                        {
                            let bytes = scroll_to_bytes(up, state.terminal.application_cursor_keys);
                            for _ in 0..lines {
                                let _ = state.pty.write_all(&bytes);
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
                            state.scheduler.mark_redraw_needed();
                        }
                    }
                    WindowEvent::Focused(focused) => {
                        if focused {
                            *focused_window = Some(state.id);
                        }
                        if state.terminal.focus_events_mode() {
                            if focused {
                                let _ = state.pty.write_all(b"\x1b[I");
                            } else {
                                let _ = state.pty.write_all(b"\x1b[O");
                            }
                        }
                    }
                    WindowEvent::RedrawRequested => {
                        let Some(atlas) = atlas_cache.get_mut(&state.dpi) else {
                            eprintln!(
                                "handterm cpu host: no glyph atlas cached for dpi {}; skipping redraw",
                                state.dpi
                            );
                            return;
                        };
                        if let Err(err) = render_grid(state, atlas, config) {
                            eprintln!(
                                "handterm cpu host: frame render failed, skipping frame: {err:#}"
                            );
                            return;
                        }
                        state.startup_timing.mark_present(Instant::now());
                        if state.startup_timing.emit_if_ready("cpu host", state.id)
                            && let Some(started) = state.cpu_time_started
                            && let Some(current) = ProcessCpuTime::capture()
                        {
                            let delta = current.delta_since(started);
                            eprintln!(
                                "handterm cpu host: startup-cpu id={}\n\
                                 \x20 open_to_first_visible_present_user={:.2}ms open_to_first_visible_present_system={:.2}ms open_to_first_visible_present_total={:.2}ms",
                                state.id,
                                delta.user_ms(),
                                delta.system_ms(),
                                delta.total_ms(),
                            );
                            if let Some(snapshot) = state.startup_timing.snapshot_if_ready() {
                                emit_structured_profile_event(
                                    "cpu_host_startup",
                                    json!({
                                        "id": state.id,
                                        "open_to_pty_spawn_ms": snapshot.open_to_pty_spawn_ms,
                                        "open_to_first_pty_event_ms": snapshot.open_to_first_pty_event_ms,
                                        "open_to_first_pty_read_ms": snapshot.open_to_first_pty_read_ms,
                                        "open_to_first_visible_output_ms": snapshot.open_to_first_visible_output_ms,
                                        "bytes_before_visible": snapshot.bytes_before_visible,
                                        "open_to_first_present_ms": snapshot.open_to_first_present_ms,
                                        "open_to_first_visible_present_ms": snapshot.open_to_first_visible_present_ms,
                                        "first_read_to_visible_present_ms": snapshot.first_read_to_visible_present_ms,
                                        "first_visible_to_present_ms": snapshot.first_visible_to_present_ms,
                                        "host_cpu": {
                                            "user_ms": delta.user_ms(),
                                            "system_ms": delta.system_ms(),
                                            "total_ms": delta.total_ms(),
                                        }
                                    }),
                                );
                            }
                        }
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
            let scheduler = &mut state.scheduler;
            let decision: FrameDecision =
                scheduler.prepare_redraw(Instant::now(), RedrawWork::default);
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
                state.window.request_redraw();
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

fn drain_pty(state: &mut HostWindowState) -> usize {
    if state.pty_closed {
        return 0;
    }
    let mut total = 0;
    loop {
        match state.pty.try_read(&mut state.pty_buf) {
            Ok(0) => break,
            Ok(n) => {
                state.startup_timing.mark_pty_read(Instant::now(), n);
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
        state
            .startup_timing
            .maybe_mark_visible(Instant::now(), &state.terminal);
        if let Some(resp) = state.terminal.drain_responses() {
            let _ = state.pty.write_all(&resp);
        }
        if let Some(title) = state.terminal.take_title() {
            state.window.set_title(&title);
        }
        if let Some(b64_data) = state.terminal.take_osc52_clipboard()
            && let Ok(decoded) = base64_decode(&b64_data)
        {
            copy_to_clipboard(&decoded);
        }
    }
    total
}

fn render_grid(
    state: &mut HostWindowState,
    atlas: &mut GlyphAtlas,
    config: &AppConfig,
) -> Result<()> {
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

    let mut buffer = state
        .surface
        .buffer_mut()
        .map_err(|e| anyhow::anyhow!("failed to acquire backbuffer: {e}"))?;
    render_terminal_to_buffer(
        buffer.as_mut(),
        width.get() as usize,
        height.get() as usize,
        &mut state.terminal,
        atlas,
        config,
        &mut state.last_visual_state,
    );

    buffer
        .present()
        .map_err(|e| anyhow::anyhow!("failed presenting frame: {e}"))?;
    state.last_presented_signature = Some(signature);
    Ok(())
}
