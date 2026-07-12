use crate::config::AppConfig;
use crate::fd_watcher::spawn_fd_watcher;
use crate::font::{GlyphAtlas, bootstrap_font_metrics_with_family_dpi};
use crate::frontend::{
    FrameDecision, FrameScheduler, KeyEventKind, RecentTextKeyEvent, RedrawWork, SmoothScrollState,
    StartupTiming, ViewportScroll, base64_decode, key_to_bytes, remember_text_key_event,
    scroll_to_bytes, scrollback_wheel_delta, should_skip_duplicate_ime_input,
    should_skip_ime_commit_after_key_event,
};
use crate::gpu_runtime::{
    GpuSurfaceState, SharedGpuContext, SharedGpuInitProfile, create_shared_gpu_context_profiled,
    create_surface_state_for_window_with_shared_profiled_with_defaults,
    create_window_attributes_for_metrics, render_surface_state_profiled_with_scroll,
    render_surface_state_with_scroll, resize_surface_state, resume_surface_state,
};
use crate::host_commands::{
    HostControlRequest, host_list_windows_response, parse_host_control_request,
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
use crate::standalone_support::handle_ipc_request;
use crate::terminal::Terminal;
use anyhow::{Context, Result};
use serde_json::json;
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
const HOT_MODE_DURATION: Duration = Duration::from_millis(160);

/// Convert a winit scale factor into the integer DPI value the font/atlas stack
/// expects (96 DPI per 1.0 scale). Uses the same truncating conversion the GPU
/// host has always used so atlas-cache keys stay identical to prior behavior.
fn dpi_from_scale_factor(scale_factor: f64) -> u32 {
    (96.0 * scale_factor).max(1.0) as u32
}

/// Pick a usable display scale factor from the available monitor handles,
/// preferring the primary monitor. Returns `None` when no monitor reports a
/// positive, finite scale (e.g. a headless event loop), in which case the
/// caller falls back to probing via a hidden window.
fn monitor_scale_factor(
    primary: Option<f64>,
    available: impl IntoIterator<Item = f64>,
) -> Option<f64> {
    let usable = |scale: &f64| scale.is_finite() && *scale > 0.0;
    primary
        .filter(usable)
        .or_else(|| available.into_iter().find(usable))
}

pub fn run(config: AppConfig, startup_command: Option<String>) -> Result<()> {
    let shared_init_task = Some(GpuApp::spawn_shared_init_task());
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

    let mut app = GpuApp::new(config, startup_command, ipc, proxy, shared_init_task);
    event_loop
        .run_app(&mut app)
        .context("failed while running app")
}

/// Result of the background shared-GPU-context initialization: the context
/// itself plus timing/profiling data captured during creation.
type SharedGpuInit = (Arc<SharedGpuContext>, SharedGpuInitProfile);
/// Handle to the background thread performing shared GPU initialization.
type SharedGpuInitTask = std::thread::JoinHandle<Result<SharedGpuInit>>;

struct GpuApp {
    config: AppConfig,
    startup_command: Option<String>,
    shared: Option<Arc<SharedGpuContext>>,
    shared_init_task: Option<SharedGpuInitTask>,
    windows: HashMap<WinitWindowId, GpuWindowState>,
    window_ids: HashMap<u64, WinitWindowId>,
    next_window_id: u64,
    focused_window: Option<u64>,
    ipc: Option<IpcServer>,
    proxy: EventLoopProxy<GpuAppEvent>,
    ipc_watcher_started: bool,
    ipc_watcher_stop: Option<Arc<AtomicBool>>,
    atlas_cache: HashMap<u32, GlyphAtlas>,
    suspended: bool,
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
    hyper_modifier: bool,
    meta_modifier: bool,
    caps_lock_modifier: bool,
    num_lock_modifier: bool,
    pending_ime_commit: Option<String>,
    recent_text_key_event: Option<RecentTextKeyEvent>,
    mouse_col: usize,
    mouse_row: usize,
    selecting: bool,
    scheduler: FrameScheduler,
    watcher_stop: Arc<AtomicBool>,
    open_window_start: Option<Instant>,
    cpu_time_started: Option<ProcessCpuTime>,
    first_frame_logged: bool,
    startup_timing: StartupTiming,
    hot_until: Option<Instant>,
    next_hot_frame_at: Option<Instant>,
    smooth_scroll: SmoothScrollState,
    native_scroll: Option<NativeScrollBridge>,
}

impl SyntheticInputTarget for GpuWindowState {
    fn label(&self) -> &'static str {
        "gpu"
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

    fn before_pty_write(&mut self) {
        self.hot_until = Some(Instant::now() + HOT_MODE_DURATION);
        self.next_hot_frame_at = Some(Instant::now());
    }

    fn reset_scrollback(&mut self) {
        self.smooth_scroll.reset();
        self.terminal.grid.scroll_offset = 0;
        self.terminal.grid.all_dirty = true;
    }

    fn drain_pty(&mut self) -> bool {
        drain_pty(self) > 0
    }
}

impl GpuApp {
    fn spawn_shared_init_task() -> SharedGpuInitTask {
        std::thread::spawn(|| -> Result<SharedGpuInit> { create_shared_gpu_context_profiled() })
    }

    fn new(
        config: AppConfig,
        startup_command: Option<String>,
        ipc: Option<IpcServer>,
        proxy: EventLoopProxy<GpuAppEvent>,
        shared_init_task: Option<SharedGpuInitTask>,
    ) -> Self {
        Self {
            config,
            startup_command,
            shared: None,
            shared_init_task,
            windows: HashMap::new(),
            window_ids: HashMap::new(),
            next_window_id: 1,
            focused_window: None,
            ipc,
            proxy,
            ipc_watcher_started: false,
            ipc_watcher_stop: None,
            atlas_cache: HashMap::new(),
            suspended: false,
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
        if self.windows.is_empty()
            && let Err(err) = self.open_window(event_loop, None, None)
        {
            eprintln!("handterm gpu host: failed to open initial window: {err:#}");
            event_loop.exit();
            return;
        }
        self.start_ipc_watcher();
    }

    fn enter_hot_mode(state: &mut GpuWindowState, now: Instant) {
        state.hot_until = Some(now + HOT_MODE_DURATION);
        state.next_hot_frame_at = Some(now);
    }

    fn sync_scrollback_view(state: &mut GpuWindowState) {
        let max = state.terminal.grid.scrollback_len() as f32;
        state.smooth_scroll.clamp(max);
        let quantized = state.smooth_scroll.displayed_scroll_offset();
        if state.terminal.grid.scroll_offset != quantized {
            state.terminal.grid.scroll_offset = quantized;
            state.terminal.grid.all_dirty = true;
        }
    }

    fn reset_scrollback_view(state: &mut GpuWindowState) {
        state.smooth_scroll.reset();
        Self::sync_scrollback_view(state);
    }

    fn apply_scrollback_delta(state: &mut GpuWindowState, delta_rows: f32, up: bool) {
        state.smooth_scroll.apply_delta(
            delta_rows,
            up,
            state.terminal.grid.scrollback_len() as f32,
        );
        Self::sync_scrollback_view(state);
    }

    fn set_scrollback_target(state: &mut GpuWindowState, rows: f32) {
        state
            .smooth_scroll
            .jump_to(rows, state.terminal.grid.scrollback_len() as f32);
        Self::sync_scrollback_view(state);
    }

    fn current_viewport_scroll(state: &GpuWindowState, config: &AppConfig) -> f32 {
        if config.scrollback.smooth {
            state.smooth_scroll.display_rows
        } else {
            0.0
        }
    }

    fn mouse_row_for_position(
        state: &GpuWindowState,
        y_px: f64,
        cell_height: usize,
        config: &AppConfig,
    ) -> usize {
        if config.scrollback.smooth {
            ViewportScroll::from_scroll_rows(state.smooth_scroll.display_rows)
                .mouse_row_for_pixel_y(y_px as f32, cell_height as f32, state.terminal.grid.rows)
        } else {
            (y_px.max(0.0) as usize) / cell_height.max(1)
        }
    }

    fn wheel_delta_rows(
        config: &AppConfig,
        delta: &MouseScrollDelta,
        cell_height: f32,
    ) -> (bool, f32) {
        match delta {
            MouseScrollDelta::LineDelta(_, y) => {
                let lines = y.abs().max(1.0);
                let delta_rows = if config.scrollback.smooth {
                    lines * config.scrollback.smooth_speed.max(0.1)
                } else {
                    scrollback_wheel_delta(lines as usize) as f32
                };
                (*y > 0.0, delta_rows)
            }
            MouseScrollDelta::PixelDelta(pos) => {
                let ch = cell_height.max(1.0) as f64;
                let rows = if config.scrollback.smooth {
                    (pos.y.abs() as f32) / cell_height.max(1.0)
                } else {
                    (pos.y.abs() / ch).max(1.0) as f32
                };
                let delta_rows = if config.scrollback.smooth {
                    rows * config.scrollback.smooth_speed.max(0.1)
                } else {
                    scrollback_wheel_delta(rows as usize) as f32
                };
                (pos.y > 0.0, delta_rows.max(0.0))
            }
        }
    }

    fn resolve_dpi(&self, event_loop: &ActiveEventLoop) -> Result<u32> {
        if let Some(id) = self.focused_window
            && let Some(winit_id) = self.window_ids.get(&id)
            && let Some(state) = self.windows.get(winit_id)
        {
            return Ok(dpi_from_scale_factor(state.renderer.window.scale_factor()));
        }
        if let Some(state) = self.windows.values().next() {
            return Ok(dpi_from_scale_factor(state.renderer.window.scale_factor()));
        }
        // First window: read the display scale factor directly from a monitor
        // handle instead of creating (and immediately destroying) a throwaway
        // probe window. On macOS this resolves to `NSScreen.backingScaleFactor`
        // without any compositor window round-trip, which removes ~20ms from
        // first-window startup. Fall back to a probe window only if no monitor
        // is reported (e.g. headless without a display).
        if let Some(scale) = monitor_scale_factor(
            event_loop.primary_monitor().map(|m| m.scale_factor()),
            event_loop.available_monitors().map(|m| m.scale_factor()),
        ) {
            return Ok(dpi_from_scale_factor(scale));
        }
        let probe_window = event_loop
            .create_window(winit::window::Window::default_attributes().with_visible(false))
            .context("failed to create invisible probe window while resolving display dpi")?;
        let dpi = dpi_from_scale_factor(probe_window.scale_factor());
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
        let shared_warm = self.shared.is_some();
        let shared_init_task = if shared_warm {
            None
        } else {
            self.shared_init_task
                .take()
                .or_else(|| Some(Self::spawn_shared_init_task()))
        };
        let before_dpi = Instant::now();
        let dpi = self.resolve_dpi(event_loop)?;
        let dpi_ms = before_dpi.elapsed();
        let atlas_cached = self.atlas_cache.contains_key(&dpi);
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
            let before_atlas = Instant::now();
            self.ensure_atlas(dpi)?;
            let atlas = self.atlas_cache.get(&dpi).with_context(|| {
                format!("atlas cache missing after initialization for dpi {dpi}")
            })?;
            let _fallback_atlas_ms = before_atlas.elapsed();
            (atlas.cell_width.max(1), atlas.cell_height.max(1), None)
        };

        let before_window = Instant::now();
        let window = Arc::new(
            event_loop
                .create_window(create_window_attributes_for_metrics(
                    &self.config,
                    cell_width,
                    cell_height,
                    dpi,
                    "handterm [gpu host]",
                    crate::platform::spawn_monitor_geometry(event_loop),
                    existing_windows,
                ))
                .context("window creation should succeed")?,
        );
        let window_ms = before_window.elapsed();
        let before_terminal = Instant::now();
        let terminal = Terminal::new_with_scrollback(cols, rows, self.config.scrollback.lines);
        let terminal_ms = before_terminal.elapsed();
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
        let added_window_cwd = (existing_windows > 0).then(dirs::home_dir).flatten();
        let pty = PtyChild::spawn_default_shell_with_command_env_and_cwd(
            cols,
            rows,
            if existing_windows == 0 {
                self.startup_command.as_deref()
            } else {
                None
            },
            &native_scroll_env_refs,
            added_window_cwd.as_deref(),
        )
        .with_context(|| format!("failed to spawn PTY for {cols}x{rows} window"))?;
        let pty_ms = before_pty.elapsed();
        let pty_spawned_at = Instant::now();

        let before_atlas = Instant::now();
        self.ensure_atlas_with_hint(dpi, font_path_hint)?;
        let atlas_ms = before_atlas.elapsed();
        let before_shared_wait = Instant::now();
        let (shared, shared_profile, shared_wait_ms) = if let Some(shared) = &self.shared {
            (
                shared.clone(),
                SharedGpuInitProfile::default(),
                Duration::ZERO,
            )
        } else if let Some(task) = shared_init_task {
            let (shared, profile) = task
                .join()
                .map_err(|_| anyhow::anyhow!("shared gpu init thread panicked"))??;
            let wait = before_shared_wait.elapsed();
            self.shared = Some(shared.clone());
            (shared, profile, wait)
        } else {
            unreachable!("cold shared gpu init should either already exist or have a task")
        };
        let atlas = self
            .atlas_cache
            .get(&dpi)
            .with_context(|| format!("atlas cache missing before surface init for dpi {dpi}"))?;

        let before_surface = Instant::now();
        let preferred_surface_defaults = self
            .windows
            .values()
            .next()
            .map(|state| state.renderer.preferred_surface_defaults());
        let preferred_surface_defaults_available = preferred_surface_defaults.is_some();
        let (renderer, surface_profile) =
            create_surface_state_for_window_with_shared_profiled_with_defaults(
                shared,
                window,
                &self.config,
                atlas,
                preferred_surface_defaults,
                Some(window_ms),
            )
            .context("failed to initialize gpu surface state")?;
        let surface_total_ms = before_surface.elapsed();
        eprintln!("handterm: {}", renderer.surface_debug_summary());
        let stop = Arc::new(AtomicBool::new(false));

        let before_watcher = Instant::now();
        spawn_fd_watcher(
            &format!("handterm-gpu-pty-{id}"),
            pty.raw_fd(),
            -1,
            self.proxy.clone(),
            GpuAppEvent::PtyReadable(id),
            stop.clone(),
        );
        let watcher_ms = before_watcher.elapsed();

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
                hyper_modifier: false,
                meta_modifier: false,
                caps_lock_modifier: false,
                num_lock_modifier: false,
                pending_ime_commit: None,
                recent_text_key_event: None,
                mouse_col: 0,
                mouse_row: 0,
                selecting: false,
                scheduler: FrameScheduler::default(),
                watcher_stop: stop,
                open_window_start: Some(start),
                cpu_time_started,
                first_frame_logged: false,
                startup_timing: {
                    let mut timing = StartupTiming::new(start);
                    timing.mark_pty_spawned(pty_spawned_at);
                    timing
                },
                hot_until: None,
                next_hot_frame_at: None,
                smooth_scroll: SmoothScrollState::default(),
                native_scroll,
            },
        );
        if let Some(state) = self.windows.get(&winit_id) {
            state.renderer.window.request_redraw();
        }
        let open_cpu = cpu_time_started.and_then(|started| {
            ProcessCpuTime::capture().map(|current| current.delta_since(started))
        });
        let sp = &surface_profile;
        let host_setup_before_surface_ms = dpi_ms
            .saturating_add(bootstrap_ms)
            .saturating_add(window_ms)
            .saturating_add(terminal_ms)
            .saturating_add(pty_ms)
            .saturating_add(atlas_ms)
            .saturating_add(shared_wait_ms)
            .as_secs_f64()
            * 1000.0;
        let compositor_facing_ms = sp.compositor_facing_total().as_secs_f64() * 1000.0;
        let handterm_surface_setup_ms = sp.handterm_setup_total().as_secs_f64() * 1000.0;
        let surface_unaccounted_ms = sp.unaccounted_total().as_secs_f64() * 1000.0;
        eprintln!(
            "handterm gpu host: open-window id={id}\n\
             \x20 kind={open_kind} existing_windows={} shared_warm={} atlas_cached={} preferred_surface_defaults_available={} defaults_reused={} pipeline_cache_hit={}\n\
             \x20 total={:.2}ms host_setup_before_surface={:.2}ms watcher={:.2}ms\n\
             \x20 surface_total={:.2}ms compositor_facing={:.2}ms handterm_surface_setup={:.2}ms surface_unaccounted={:.2}ms\n\
             \x20 dpi={:.2}ms bootstrap={:.2}ms window={:.2}ms atlas={:.2}ms shared_total={:.2}ms shared_wait={:.2}ms terminal={:.2}ms pty={:.2}ms\n\
             \x20   shared_adapter={:.2}ms shared_device={:.2}ms shared_layout={:.2}ms shared_shaders={:.2}ms shared_sampler={:.2}ms shared_atlas={:.2}ms\n\
             \x20 surface_total={:.2}ms\n\
             \x20   window_create={:.2}ms ime={:.2}ms wgpu_surface={:.2}ms\n\
             \x20   default_config={:.2}ms caps={:.2}ms configure={:.2}ms\n\
             \x20   atlas_tex={:.2}ms uniform_buf={:.2}ms inst_bufs={:.2}ms\n\
             \x20   bind_group={:.2}ms pipeline={:.2}ms\n\
             \x20 host_cpu_user={:.2}ms host_cpu_system={:.2}ms host_cpu_total={:.2}ms",
            existing_windows,
            shared_warm,
            atlas_cached,
            preferred_surface_defaults_available,
            sp.reused_surface_defaults,
            sp.pipeline_cache_hit,
            start.elapsed().as_secs_f64() * 1000.0,
            host_setup_before_surface_ms,
            watcher_ms.as_secs_f64() * 1000.0,
            surface_total_ms.as_secs_f64() * 1000.0,
            compositor_facing_ms,
            handterm_surface_setup_ms,
            surface_unaccounted_ms,
            dpi_ms.as_secs_f64() * 1000.0,
            bootstrap_ms.as_secs_f64() * 1000.0,
            window_ms.as_secs_f64() * 1000.0,
            atlas_ms.as_secs_f64() * 1000.0,
            shared_profile.total.as_secs_f64() * 1000.0,
            shared_wait_ms.as_secs_f64() * 1000.0,
            terminal_ms.as_secs_f64() * 1000.0,
            pty_ms.as_secs_f64() * 1000.0,
            shared_profile.adapter_request.as_secs_f64() * 1000.0,
            shared_profile.device_request.as_secs_f64() * 1000.0,
            shared_profile
                .bind_group_layout
                .saturating_add(shared_profile.pipeline_layout)
                .as_secs_f64()
                * 1000.0,
            shared_profile.shader_modules.as_secs_f64() * 1000.0,
            shared_profile.sampler.as_secs_f64() * 1000.0,
            shared_profile.atlas_texture.as_secs_f64() * 1000.0,
            surface_total_ms.as_secs_f64() * 1000.0,
            sp.window_create.as_secs_f64() * 1000.0,
            sp.ime_setup.as_secs_f64() * 1000.0,
            sp.surface_create.as_secs_f64() * 1000.0,
            sp.default_config.as_secs_f64() * 1000.0,
            sp.capabilities.as_secs_f64() * 1000.0,
            sp.configure.as_secs_f64() * 1000.0,
            sp.atlas_texture.as_secs_f64() * 1000.0,
            sp.uniform_buffer.as_secs_f64() * 1000.0,
            sp.instance_buffers.as_secs_f64() * 1000.0,
            sp.bind_group.as_secs_f64() * 1000.0,
            sp.pipeline_lookup.as_secs_f64() * 1000.0,
            open_cpu.map(ProcessCpuTime::user_ms).unwrap_or(0.0),
            open_cpu.map(ProcessCpuTime::system_ms).unwrap_or(0.0),
            open_cpu.map(ProcessCpuTime::total_ms).unwrap_or(0.0),
        );
        emit_structured_profile_event(
            "gpu_host_open_window",
            json!({
                "id": id,
                "kind": open_kind,
                "existing_windows": existing_windows,
                "shared_warm": shared_warm,
                "atlas_cached": atlas_cached,
                "preferred_surface_defaults_available": preferred_surface_defaults_available,
                "defaults_reused": sp.reused_surface_defaults,
                "pipeline_cache_hit": sp.pipeline_cache_hit,
                "total_ms": start.elapsed().as_secs_f64() * 1000.0,
                "host_setup_before_surface_ms": host_setup_before_surface_ms,
                "watcher_ms": watcher_ms.as_secs_f64() * 1000.0,
                "surface_total_ms": surface_total_ms.as_secs_f64() * 1000.0,
                "compositor_facing_ms": compositor_facing_ms,
                "handterm_surface_setup_ms": handterm_surface_setup_ms,
                "surface_unaccounted_ms": surface_unaccounted_ms,
                "dpi_ms": dpi_ms.as_secs_f64() * 1000.0,
                "bootstrap_ms": bootstrap_ms.as_secs_f64() * 1000.0,
                "window_ms": window_ms.as_secs_f64() * 1000.0,
                "atlas_ms": atlas_ms.as_secs_f64() * 1000.0,
                "shared_ms": shared_profile.total.as_secs_f64() * 1000.0,
                "shared_wait_ms": shared_wait_ms.as_secs_f64() * 1000.0,
                "shared_profile": {
                    "adapter_request_ms": shared_profile.adapter_request.as_secs_f64() * 1000.0,
                    "device_request_ms": shared_profile.device_request.as_secs_f64() * 1000.0,
                    "bind_group_layout_ms": shared_profile.bind_group_layout.as_secs_f64() * 1000.0,
                    "shader_modules_ms": shared_profile.shader_modules.as_secs_f64() * 1000.0,
                    "pipeline_layout_ms": shared_profile.pipeline_layout.as_secs_f64() * 1000.0,
                    "sampler_ms": shared_profile.sampler.as_secs_f64() * 1000.0,
                    "atlas_texture_ms": shared_profile.atlas_texture.as_secs_f64() * 1000.0,
                    "total_ms": shared_profile.total.as_secs_f64() * 1000.0,
                },
                "terminal_ms": terminal_ms.as_secs_f64() * 1000.0,
                "pty_ms": pty_ms.as_secs_f64() * 1000.0,
                "surface": {
                    "window_create_ms": sp.window_create.as_secs_f64() * 1000.0,
                    "ime_setup_ms": sp.ime_setup.as_secs_f64() * 1000.0,
                    "surface_create_ms": sp.surface_create.as_secs_f64() * 1000.0,
                    "default_config_ms": sp.default_config.as_secs_f64() * 1000.0,
                    "capabilities_ms": sp.capabilities.as_secs_f64() * 1000.0,
                    "configure_ms": sp.configure.as_secs_f64() * 1000.0,
                    "atlas_texture_ms": sp.atlas_texture.as_secs_f64() * 1000.0,
                    "uniform_buffer_ms": sp.uniform_buffer.as_secs_f64() * 1000.0,
                    "instance_buffers_ms": sp.instance_buffers.as_secs_f64() * 1000.0,
                    "bind_group_ms": sp.bind_group.as_secs_f64() * 1000.0,
                    "pipeline_lookup_ms": sp.pipeline_lookup.as_secs_f64() * 1000.0,
                },
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

        let Some(target_id) = self.resolve_target_window_id(target_window_from_args(req)) else {
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
                    "backend": "gpu",
                    "window_id": target_id,
                    "scroll_offset": state.terminal.grid.scroll_offset,
                    "scrollback_len": state.terminal.grid.scrollback_len(),
                    "rows": state.terminal.grid.rows,
                    "smooth_supported": true,
                    "smooth_target_rows": state.smooth_scroll.target_rows,
                    "smooth_display_rows": state.smooth_scroll.display_rows,
                    "smooth_animating": state.smooth_scroll.is_animating(),
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
                .unwrap_or(0.0) as f32;
            if delta_rows > 0.0 {
                Self::enter_hot_mode(state, Instant::now());
                Self::apply_scrollback_delta(state, delta_rows, true);
            } else if delta_rows < 0.0 {
                Self::enter_hot_mode(state, Instant::now());
                Self::apply_scrollback_delta(state, -delta_rows, false);
            }
            state.scheduler.mark_redraw_needed();
            state.renderer.window.request_redraw();
            return (
                Response::ok(serde_json::json!({
                    "backend": "gpu",
                    "window_id": target_id,
                    "scroll_offset": state.terminal.grid.scroll_offset,
                    "scrollback_len": state.terminal.grid.scrollback_len(),
                    "rows": state.terminal.grid.rows,
                    "smooth_supported": true,
                    "smooth_target_rows": state.smooth_scroll.target_rows,
                    "smooth_display_rows": state.smooth_scroll.display_rows,
                    "smooth_animating": state.smooth_scroll.is_animating(),
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
                IpcAction::SyntheticKeyEvent { window, event } => {
                    if let Some(id) = self.resolve_target_window_id(window)
                        && let Some(winit_id) = self.window_ids.get(&id)
                        && let Some(state) = self.windows.get_mut(winit_id)
                    {
                        let changed = apply_synthetic_key_event(state, &event);
                        let work = crate::frontend::classify_redraw_work(&state.terminal, changed);
                        let should_redraw_now = if self.focused_window == Some(state.id) {
                            state.scheduler.mark_redraw_needed();
                            true
                        } else {
                            state
                                .scheduler
                                .mark_io_processed(Instant::now(), FRAME_INTERVAL, work)
                        };
                        if should_redraw_now {
                            state.renderer.window.request_redraw();
                        }
                    }
                }
                IpcAction::SyntheticImeCommit { window, text } => {
                    if let Some(id) = self.resolve_target_window_id(window)
                        && let Some(winit_id) = self.window_ids.get(&id)
                        && let Some(state) = self.windows.get_mut(winit_id)
                    {
                        let changed = apply_synthetic_ime_commit(state, &text);
                        let work = crate::frontend::classify_redraw_work(&state.terminal, changed);
                        let should_redraw_now = if self.focused_window == Some(state.id) {
                            state.scheduler.mark_redraw_needed();
                            true
                        } else {
                            state
                                .scheduler
                                .mark_io_processed(Instant::now(), FRAME_INTERVAL, work)
                        };
                        if should_redraw_now {
                            state.renderer.window.request_redraw();
                        }
                    }
                }
            }
        }
    }
}

impl ApplicationHandler<GpuAppEvent> for GpuApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.suspended = false;
        for state in self.windows.values_mut() {
            if let Err(error) = resume_surface_state(
                &mut state.renderer,
                self.config.style.background_opacity < 1.0,
            ) {
                eprintln!("handterm gpu host: failed to resume surface: {error:#}");
            } else {
                state.renderer.window.request_redraw();
            }
        }
        self.ensure_initial_window(event_loop);
    }

    fn suspended(&mut self, event_loop: &ActiveEventLoop) {
        self.suspended = true;
        event_loop.set_control_flow(ControlFlow::Wait);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: GpuAppEvent) {
        match event {
            GpuAppEvent::PtyReadable(window_id) => {
                if let Some(winit_id) = self.window_ids.get(&window_id)
                    && let Some(state) = self.windows.get_mut(winit_id)
                {
                    state.startup_timing.mark_pty_event(Instant::now());
                    let bytes_read = drain_pty(state);
                    if bytes_read > 0 {
                        Self::enter_hot_mode(state, Instant::now());
                        let work = crate::frontend::classify_redraw_work(&state.terminal, true);
                        let should_redraw_now = if self.focused_window == Some(state.id) {
                            state.scheduler.mark_redraw_needed();
                            true
                        } else {
                            state
                                .scheduler
                                .mark_io_processed(Instant::now(), FRAME_INTERVAL, work)
                        };
                        if should_redraw_now {
                            state.renderer.window.request_redraw();
                        }
                    }
                    if state.pty_closed {
                        let winit_id = *winit_id;
                        self.close_window(winit_id, event_loop);
                    }
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
                let Some(atlas) = atlas_cache.get_mut(&state.dpi) else {
                    eprintln!(
                        "handterm gpu host: no glyph atlas cached for dpi {}; dropping window event",
                        state.dpi
                    );
                    return;
                };

                match event {
                    WindowEvent::Resized(size) if size.width > 0 && size.height > 0 => {
                        let pad2 = 2 * self.config.window.padding_px(atlas.dpi()) as usize;
                        let new_cols = ((size.width as usize).saturating_sub(pad2)
                            / atlas.cell_width.max(1))
                            as u16;
                        let new_rows = ((size.height as usize).saturating_sub(pad2)
                            / atlas.cell_height.max(1))
                            as u16;
                        let new_cols = new_cols.max(1);
                        let new_rows = new_rows.max(1);

                        resize_surface_state(
                            &mut state.renderer,
                            atlas,
                            size.width,
                            size.height,
                            new_cols,
                            new_rows,
                            self.config.window.padding_px(atlas.dpi()),
                        );

                        if new_cols != state.terminal.cols || new_rows != state.terminal.rows {
                            state.terminal.resize(new_cols, new_rows);
                            let _ = state.pty.resize(new_cols, new_rows);
                        }

                        state.renderer.window.request_redraw();
                    }
                    WindowEvent::ModifiersChanged(new_modifiers) => {
                        state.modifiers = new_modifiers;
                    }
                    WindowEvent::Ime(Ime::Commit(text)) if !text.is_empty() => {
                        let ime_commit_text = crate::frontend::normalize_ime_dedupe_text(&text)
                            .unwrap_or_else(|| text.clone());
                        crate::frontend::trace_input(format!(
                            "gpu ime-commit raw={:?} normalized={:?}",
                            text, ime_commit_text
                        ));
                        if should_skip_ime_commit_after_key_event(
                            &mut state.recent_text_key_event,
                            &ime_commit_text,
                            Instant::now(),
                        ) {
                            crate::frontend::trace_input(
                                "gpu ime-commit skipped after key-event dedupe",
                            );
                            return;
                        }
                        Self::enter_hot_mode(state, Instant::now());
                        state.pending_ime_commit = Some(ime_commit_text);
                        let _ = state.pty.write_all(text.as_bytes());
                        if state.terminal.grid.scroll_offset > 0 {
                            Self::reset_scrollback_view(state);
                        }
                        state.terminal.grid.selection = None;
                        let changed = drain_pty(state) > 0;
                        let work = crate::frontend::classify_redraw_work(&state.terminal, changed);
                        let should_redraw_now = if *focused_window == Some(state.id) {
                            state.scheduler.mark_redraw_needed();
                            true
                        } else {
                            state
                                .scheduler
                                .mark_io_processed(Instant::now(), FRAME_INTERVAL, work)
                        };
                        if should_redraw_now {
                            state.renderer.window.request_redraw();
                        }
                    }
                    WindowEvent::KeyboardInput { event, .. } => {
                        if event.state == ElementState::Pressed {
                            let ctrl = state.modifiers.state().control_key();
                            let shift = state.modifiers.state().shift_key();

                            if ctrl
                                && shift
                                && !event.repeat
                                && let Key::Character(s) = &event.logical_key
                            {
                                let ch = s.chars().next().unwrap_or('\0').to_ascii_lowercase();
                                if ch == 'v' {
                                    if let Ok(text) = paste_from_clipboard() {
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
                                        let _ = copy_to_clipboard(text.as_bytes());
                                    }
                                    return;
                                }
                            }

                            if shift {
                                if let Key::Named(NamedKey::PageUp) = &event.logical_key {
                                    let max = state.terminal.grid.scrollback_len() as f32;
                                    let half = state.terminal.rows as f32 / 2.0;
                                    Self::set_scrollback_target(
                                        state,
                                        state.smooth_scroll.target_rows + half.min(max),
                                    );
                                    state.scheduler.mark_redraw_needed();
                                    return;
                                }
                                if let Key::Named(NamedKey::PageDown) = &event.logical_key {
                                    let half = state.terminal.rows as f32 / 2.0;
                                    Self::set_scrollback_target(
                                        state,
                                        (state.smooth_scroll.target_rows - half).max(0.0),
                                    );
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
                                "gpu key-event kind={:?} key={:?} text={:?} dedupe_text={:?} bytes={:?}",
                                event_kind, event.logical_key, event.text, ime_dedupe_text, bytes
                            ));
                            if should_skip_duplicate_ime_input(
                                &mut state.pending_ime_commit,
                                event_kind,
                                ime_dedupe_text.as_deref(),
                                Some(&bytes),
                            ) {
                                crate::frontend::trace_input("gpu key-event skipped by ime dedupe");
                                return;
                            }
                            remember_text_key_event(
                                &mut state.recent_text_key_event,
                                event_kind,
                                ime_dedupe_text.as_deref(),
                                Some(&bytes),
                                Instant::now(),
                            );
                            Self::enter_hot_mode(state, Instant::now());
                            let _ = state.pty.write_all(&bytes);
                            if state.terminal.grid.scroll_offset > 0 {
                                Self::reset_scrollback_view(state);
                            }
                            state.terminal.grid.selection = None;
                            let changed = drain_pty(state) > 0;
                            let work =
                                crate::frontend::classify_redraw_work(&state.terminal, changed);
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
                                state.renderer.window.request_redraw();
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
                        let cw = atlas.cell_width.max(1);
                        let ch = atlas.cell_height.max(1);
                        let pad = self.config.window.padding_px(atlas.dpi()) as f64;
                        state.mouse_col = (position.x - pad).max(0.0) as usize / cw;
                        state.mouse_row = Self::mouse_row_for_position(
                            state,
                            (position.y - pad).max(0.0),
                            ch,
                            &self.config,
                        );

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
                                        .cell_at_scroll(state.mouse_row, state.mouse_col);
                                    if cell.hyperlink_id != 0
                                        && let Some(url) =
                                            state.terminal.grid.hyperlink_url(cell.hyperlink_id)
                                    {
                                        let _ = open_url(url);
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
                                    let _ = copy_to_clipboard(text.as_bytes());
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
                        let (up, delta_rows) =
                            Self::wheel_delta_rows(&self.config, &delta, atlas.cell_height as f32);
                        let lines = delta_rows.ceil().max(1.0) as usize;
                        if let Some(bridge) = state.native_scroll.as_mut()
                            && let Some(pane) =
                                bridge.hovered_pane(state.mouse_col, state.mouse_row)
                            && bridge.send_scroll_delta(
                                pane,
                                if up {
                                    -delta_rows.max(0.1)
                                } else {
                                    delta_rows.max(0.1)
                                },
                            )
                        {
                            Self::enter_hot_mode(state, Instant::now());
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
                            Self::enter_hot_mode(state, Instant::now());
                            Self::apply_scrollback_delta(state, delta_rows, up);
                            state.scheduler.mark_redraw_needed();
                        }
                    }
                    WindowEvent::Focused(focused) => {
                        if focused {
                            *focused_window = Some(state.id);
                            Self::enter_hot_mode(state, Instant::now());
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
                        if self.suspended {
                            return;
                        }
                        if self.config.scrollback.smooth {
                            let _ = state.smooth_scroll.advance(
                                Instant::now(),
                                state.terminal.grid.scrollback_len() as f32,
                            );
                            Self::sync_scrollback_view(state);
                        }
                        let viewport_scroll = Self::current_viewport_scroll(state, &self.config);
                        if !state.first_frame_logged {
                            let render_profile = render_surface_state_profiled_with_scroll(
                                &mut state.renderer,
                                &mut state.terminal,
                                atlas,
                                &self.config,
                                viewport_scroll,
                            );
                            state.first_frame_logged = true;
                            // The initial window size was clamped on macOS to stop
                            // AppKit from auto-growing the fresh window to fill the
                            // display (which would inflate the GPU drawables). Now
                            // that the first frame is up at the intended grid size,
                            // lift the cap so the window is freely resizable again.
                            #[cfg(target_os = "macos")]
                            state
                                .renderer
                                .window
                                .set_max_inner_size(None::<winit::dpi::PhysicalSize<u32>>);
                            if let Some(rp) = render_profile {
                                let open_to_present = state
                                    .open_window_start
                                    .map(|s| s.elapsed().as_secs_f64() * 1000.0)
                                    .unwrap_or(0.0);
                                let first_present_cpu =
                                    state.cpu_time_started.and_then(|started| {
                                        ProcessCpuTime::capture()
                                            .map(|current| current.delta_since(started))
                                    });
                                state.startup_timing.mark_present(Instant::now());
                                if state.startup_timing.emit_if_ready("gpu host", state.id)
                                    && let Some(delta) = first_present_cpu
                                {
                                    eprintln!(
                                        "handterm gpu host: startup-cpu id={}\n\
                                         \x20 open_to_first_visible_present_user={:.2}ms open_to_first_visible_present_system={:.2}ms open_to_first_visible_present_total={:.2}ms",
                                        state.id,
                                        delta.user_ms(),
                                        delta.system_ms(),
                                        delta.total_ms(),
                                    );
                                }
                                eprintln!(
                                    "handterm gpu host: first-frame id={}\n\
                                     \x20 open_to_first_present={:.2}ms\n\
                                     \x20 acquire={:.2}ms display_list={:.2}ms upload={:.2}ms\n\
                                     \x20 encode={:.2}ms submit={:.2}ms present={:.2}ms\n\
                                     \x20 render_total={:.2}ms\n\
                                     \x20 host_cpu_user={:.2}ms host_cpu_system={:.2}ms host_cpu_total={:.2}ms",
                                    state.id,
                                    open_to_present,
                                    rp.acquire_surface.as_secs_f64() * 1000.0,
                                    rp.build_display_list.as_secs_f64() * 1000.0,
                                    rp.upload_buffers.as_secs_f64() * 1000.0,
                                    rp.encode_pass.as_secs_f64() * 1000.0,
                                    rp.submit.as_secs_f64() * 1000.0,
                                    rp.present.as_secs_f64() * 1000.0,
                                    rp.total.as_secs_f64() * 1000.0,
                                    first_present_cpu
                                        .map(ProcessCpuTime::user_ms)
                                        .unwrap_or(0.0),
                                    first_present_cpu
                                        .map(ProcessCpuTime::system_ms)
                                        .unwrap_or(0.0),
                                    first_present_cpu
                                        .map(ProcessCpuTime::total_ms)
                                        .unwrap_or(0.0),
                                );
                                emit_structured_profile_event(
                                    "gpu_host_first_frame",
                                    json!({
                                        "id": state.id,
                                        "open_to_first_present_ms": open_to_present,
                                        "render": {
                                            "acquire_surface_ms": rp.acquire_surface.as_secs_f64() * 1000.0,
                                            "build_display_list_ms": rp.build_display_list.as_secs_f64() * 1000.0,
                                            "upload_buffers_ms": rp.upload_buffers.as_secs_f64() * 1000.0,
                                            "encode_pass_ms": rp.encode_pass.as_secs_f64() * 1000.0,
                                            "submit_ms": rp.submit.as_secs_f64() * 1000.0,
                                            "present_ms": rp.present.as_secs_f64() * 1000.0,
                                            "total_ms": rp.total.as_secs_f64() * 1000.0,
                                        },
                                        "host_cpu": {
                                            "user_ms": first_present_cpu.map(ProcessCpuTime::user_ms).unwrap_or(0.0),
                                            "system_ms": first_present_cpu.map(ProcessCpuTime::system_ms).unwrap_or(0.0),
                                            "total_ms": first_present_cpu.map(ProcessCpuTime::total_ms).unwrap_or(0.0),
                                        }
                                    }),
                                );
                            }
                            state.open_window_start = None;
                        } else {
                            render_surface_state_with_scroll(
                                &mut state.renderer,
                                &mut state.terminal,
                                atlas,
                                &self.config,
                                viewport_scroll,
                            );
                            state.startup_timing.mark_present(Instant::now());
                            if state.startup_timing.emit_if_ready("gpu host", state.id)
                                && let Some(started) = state.cpu_time_started
                                && let Some(current) = ProcessCpuTime::capture()
                            {
                                let delta = current.delta_since(started);
                                eprintln!(
                                    "handterm gpu host: startup-cpu id={}\n\
                                     \x20 open_to_first_visible_present_user={:.2}ms open_to_first_visible_present_system={:.2}ms open_to_first_visible_present_total={:.2}ms",
                                    state.id,
                                    delta.user_ms(),
                                    delta.system_ms(),
                                    delta.total_ms(),
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
        if self.suspended {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }
        let mut earliest_deadline: Option<Instant> = None;
        let mut redraw_ids = Vec::new();
        let now = Instant::now();

        for (winit_id, state) in &mut self.windows {
            if self.config.scrollback.smooth && state.smooth_scroll.is_animating() {
                let next_frame_at = state.next_hot_frame_at.unwrap_or(now);
                if now >= next_frame_at {
                    redraw_ids.push(*winit_id);
                    state.next_hot_frame_at = Some(now + FRAME_INTERVAL);
                } else {
                    earliest_deadline = Some(match earliest_deadline {
                        Some(existing) => existing.min(next_frame_at),
                        None => next_frame_at,
                    });
                }
            }
            if self.focused_window == Some(state.id)
                && let Some(hot_until) = state.hot_until
            {
                if now < hot_until {
                    let next_frame_at = state.next_hot_frame_at.unwrap_or(now);
                    if now >= next_frame_at {
                        redraw_ids.push(*winit_id);
                        state.next_hot_frame_at = Some(now + FRAME_INTERVAL);
                    } else {
                        earliest_deadline = Some(match earliest_deadline {
                            Some(existing) => existing.min(next_frame_at),
                            None => next_frame_at,
                        });
                    }
                } else {
                    state.hot_until = None;
                    state.next_hot_frame_at = None;
                }
            }

            let scheduler = &mut state.scheduler;
            let decision: FrameDecision = scheduler.prepare_redraw(now, RedrawWork::default);
            if let Some(deadline) = decision.wait_until {
                earliest_deadline = Some(match earliest_deadline {
                    Some(existing) => existing.min(deadline),
                    None => deadline,
                });
            }
            if decision.request_redraw {
                redraw_ids.push(*winit_id);
            }
        }

        for winit_id in redraw_ids {
            if let Some(state) = self.windows.get(&winit_id) {
                state.renderer.window.request_redraw();
            }
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

fn drain_pty(state: &mut GpuWindowState) -> usize {
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
            state.renderer.window.set_title(&title);
        }
        if let Some(b64_data) = state.terminal.take_osc52_clipboard()
            && let Ok(decoded) = base64_decode(&b64_data)
        {
            let _ = copy_to_clipboard(&decoded);
        }
    }

    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::dpi::PhysicalPosition;

    #[test]
    fn smooth_pixel_wheel_delta_preserves_fractional_rows() {
        let config = AppConfig::default();
        let (_up, delta_rows) = GpuApp::wheel_delta_rows(
            &config,
            &MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, 4.0)),
            16.0,
        );

        assert!(
            delta_rows > 0.0 && delta_rows < 1.0,
            "expected fractional smooth scroll delta, got {delta_rows}"
        );
    }

    #[test]
    fn non_smooth_pixel_wheel_delta_still_steps_at_least_one_row() {
        let mut config = AppConfig::default();
        config.scrollback.smooth = false;

        let (_up, delta_rows) = GpuApp::wheel_delta_rows(
            &config,
            &MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, 4.0)),
            16.0,
        );

        assert!(delta_rows >= 1.0);
    }

    #[test]
    fn dpi_from_scale_factor_matches_96_dpi_convention() {
        assert_eq!(dpi_from_scale_factor(1.0), 96);
        assert_eq!(dpi_from_scale_factor(2.0), 192);
        assert_eq!(dpi_from_scale_factor(1.5), 144);
        // Fractional scales truncate, matching the host's historical behavior so
        // the atlas-cache key for a given display does not shift.
        assert_eq!(dpi_from_scale_factor(2.0 + 1.0 / 96.0), 193);
        // Degenerate scales never collapse to zero DPI.
        assert_eq!(dpi_from_scale_factor(0.0), 1);
    }

    #[test]
    fn monitor_scale_factor_prefers_primary_when_usable() {
        assert_eq!(
            monitor_scale_factor(Some(2.0), vec![1.0, 3.0]),
            Some(2.0),
            "primary monitor scale should win"
        );
    }

    #[test]
    fn monitor_scale_factor_falls_back_to_first_usable_available() {
        // No/invalid primary -> first finite, positive available scale is used.
        assert_eq!(monitor_scale_factor(None, vec![1.5, 2.0]), Some(1.5));
        assert_eq!(monitor_scale_factor(Some(0.0), vec![2.0]), Some(2.0));
        assert_eq!(
            monitor_scale_factor(Some(f64::NAN), vec![0.0, -1.0, 1.25]),
            Some(1.25)
        );
    }

    #[test]
    fn monitor_scale_factor_returns_none_when_no_display() {
        // Headless: nothing usable -> caller falls back to a probe window.
        assert_eq!(monitor_scale_factor(None, Vec::<f64>::new()), None);
        assert_eq!(monitor_scale_factor(Some(-1.0), vec![0.0]), None);
    }
}
