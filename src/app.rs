use crate::config::AppConfig;
use crate::font::GlyphAtlas;
use crate::ipc::{IpcAction, IpcServer, Request, Response};
use crate::pty::PtyChild;
use crate::terminal::Terminal;
use anyhow::{Context, Result};
use softbuffer::{Context as SoftContext, Surface};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, Size};
use winit::event::{ElementState, Modifiers, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey, PhysicalKey};
use winit::window::{Window, WindowAttributes, WindowId};

pub fn run(config: AppConfig) -> Result<()> {
    let event_loop = EventLoop::new().context("failed to create event loop")?;
    event_loop.set_control_flow(ControlFlow::wait_duration(Duration::from_millis(8)));

    let socket_path = crate::ipc::default_socket_path();
    let ipc = IpcServer::bind(&socket_path).ok();
    if let Some(ref ipc) = ipc {
        eprintln!("handterm: listening on {}", ipc.path().display());
    }

    let mut app = HandtermApp::new(config, ipc);
    event_loop
        .run_app(&mut app)
        .context("failed while running app")
}

struct HandtermApp {
    config: AppConfig,
    state: Option<AppState>,
    ipc: Option<IpcServer>,
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
}

impl HandtermApp {
    fn new(config: AppConfig, ipc: Option<IpcServer>) -> Self {
        Self {
            config,
            state: None,
            ipc,
        }
    }

    fn create_window_attributes(&self, atlas: &GlyphAtlas) -> WindowAttributes {
        let width = self.config.window.columns as f64 * atlas.cell_width as f64;
        let height = self.config.window.rows as f64 * atlas.cell_height as f64;

        Window::default_attributes()
            .with_title("handterm")
            .with_transparent(self.config.style.background_opacity < 1.0)
            .with_inner_size(Size::Logical(LogicalSize::new(width, height)))
    }
}

impl ApplicationHandler for HandtermApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let cols = self.config.window.columns;
        let rows = self.config.window.rows;

        let atlas = GlyphAtlas::with_family(&self.config.style.font_family, self.config.style.font_size)
            .or_else(|_| GlyphAtlas::new(self.config.style.font_size))
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
        });

        if let Some(s) = &self.state {
            s.window.request_redraw();
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
                                if let Ok(output) = std::process::Command::new("wl-paste")
                                    .arg("--no-newline")
                                    .output()
                                {
                                    let text = output.stdout;
                                    if !text.is_empty() {
                                        if state.terminal.bracketed_paste_mode() {
                                            let _ = state.pty.write_all(b"\x1b[200~");
                                            let _ = state.pty.write_all(&text);
                                            let _ = state.pty.write_all(b"\x1b[201~");
                                        } else {
                                            let _ = state.pty.write_all(&text);
                                        }
                                    }
                                }
                                return;
                            } else if ch == 'c' {
                                let text = state.terminal.grid.get_selection_text();
                                if !text.is_empty() {
                                    let mut child = std::process::Command::new("wl-copy")
                                        .stdin(std::process::Stdio::piped())
                                        .spawn()
                                        .ok();
                                    if let Some(ref mut c) = child {
                                        if let Some(ref mut stdin) = c.stdin {
                                            let _ = std::io::Write::write_all(stdin, text.as_bytes());
                                        }
                                    }
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
                            state.window.request_redraw();
                            return;
                        }
                        if let Key::Named(NamedKey::PageDown) = &event.logical_key {
                            let half = state.terminal.rows as usize / 2;
                            state.terminal.grid.scroll_offset = state.terminal.grid.scroll_offset.saturating_sub(half);
                            state.window.request_redraw();
                            return;
                        }
                    }

                    if let Some(bytes) = key_to_bytes(
                        &event.logical_key,
                        &event.physical_key,
                        state.terminal.application_cursor_keys,
                        ctrl,
                    ) {
                        let _ = state.pty.write_all(&bytes);
                        state.terminal.grid.scroll_offset = 0;
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let cw = state.atlas.cell_width.max(1);
                let ch = state.atlas.cell_height.max(1);
                state.mouse_col = position.x as usize / cw;
                state.mouse_row = position.y as usize / ch;
            }
            WindowEvent::MouseInput { state: btn_state, button, .. } => {
                let btn = match button {
                    MouseButton::Left => 0u8,
                    MouseButton::Middle => 1,
                    MouseButton::Right => 2,
                    _ => return,
                };
                let pressed = btn_state == ElementState::Pressed;
                if let Some(bytes) = state.terminal.encode_mouse(btn, state.mouse_col, state.mouse_row, pressed) {
                    let _ = state.pty.write_all(&bytes);
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
                } else {
                    let max = state.terminal.grid.scrollback_len();
                    if up {
                        state.terminal.grid.scroll_offset = (state.terminal.grid.scroll_offset + lines * 3).min(max);
                    } else {
                        state.terminal.grid.scroll_offset = state.terminal.grid.scroll_offset.saturating_sub(lines * 3);
                    }
                    state.window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                drain_pty(state);
                render_grid(state, &self.config).expect("frame render should succeed");
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(state) = &mut self.state {
            let n = drain_pty(state);
            if n > 0 {
                state.window.request_redraw();
            }
            if state.pty_closed {
                event_loop.exit();
            }
        }

        if let (Some(ipc), Some(state)) = (&mut self.ipc, &mut self.state) {
            let actions = ipc.poll(&mut |req| handle_ipc_request(req, state));
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
    }
}

fn handle_ipc_request(req: &Request, state: &mut AppState) -> (Response, IpcAction) {
    match req.cmd.as_str() {
        "get-text" => {
            let text = if let Some(obj) = req.args.as_object() {
                let start = obj
                    .get("start_row")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize;
                let end = obj
                    .get("end_row")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(state.terminal.grid.rows as u64)
                    as usize;
                state.terminal.grid.get_text(start, end)
            } else {
                state.terminal.grid.get_all_text()
            };
            (
                Response::ok(serde_json::json!({ "text": text })),
                IpcAction::None,
            )
        }
        "send-text" => {
            let text = req
                .args
                .as_object()
                .and_then(|o| o.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if text.is_empty() {
                (Response::err("missing 'text' argument"), IpcAction::None)
            } else {
                (
                    Response::ok_empty(),
                    IpcAction::SendText(text.as_bytes().to_vec()),
                )
            }
        }
        "send-key" => {
            let key = req
                .args
                .as_object()
                .and_then(|o| o.get("key"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let bytes = match key {
                "enter" | "return" => Some(b"\r".to_vec()),
                "tab" => Some(b"\t".to_vec()),
                "escape" | "esc" => Some(b"\x1b".to_vec()),
                "backspace" => Some(b"\x7f".to_vec()),
                "up" => Some(b"\x1b[A".to_vec()),
                "down" => Some(b"\x1b[B".to_vec()),
                "right" => Some(b"\x1b[C".to_vec()),
                "left" => Some(b"\x1b[D".to_vec()),
                "home" => Some(b"\x1b[H".to_vec()),
                "end" => Some(b"\x1b[F".to_vec()),
                "delete" => Some(b"\x1b[3~".to_vec()),
                "page_up" => Some(b"\x1b[5~".to_vec()),
                "page_down" => Some(b"\x1b[6~".to_vec()),
                "space" => Some(b" ".to_vec()),
                k if k.starts_with("ctrl+") && k.len() == 6 => {
                    let ch = k.as_bytes()[5];
                    if ch.is_ascii_alphabetic() {
                        Some(vec![ch.to_ascii_lowercase() - b'a' + 1])
                    } else {
                        None
                    }
                }
                _ => None,
            };
            match bytes {
                Some(b) => (Response::ok_empty(), IpcAction::SendText(b)),
                None => (
                    Response::err(format!("unknown key: {key}")),
                    IpcAction::None,
                ),
            }
        }
        "get-cursor" => {
            let (col, row) = state.terminal.grid.cursor_pos();
            (
                Response::ok(serde_json::json!({ "row": row, "col": col })),
                IpcAction::None,
            )
        }
        "get-size" => (
            Response::ok(serde_json::json!({
                "cols": state.terminal.cols,
                "rows": state.terminal.rows,
            })),
            IpcAction::None,
        ),
        "set-title" => {
            let title = req
                .args
                .as_object()
                .and_then(|o| o.get("title"))
                .and_then(|v| v.as_str())
                .unwrap_or("handterm")
                .to_string();
            (Response::ok_empty(), IpcAction::SetTitle(title))
        }
        "close" => (Response::ok_empty(), IpcAction::Close),
        "ls" => (
            Response::ok(serde_json::json!({
                "commands": [
                    "get-text", "send-text", "send-key",
                    "get-cursor", "get-size", "set-title",
                    "close", "ls"
                ]
            })),
            IpcAction::None,
        ),
        _ => (
            Response::err(format!("unknown command: {}", req.cmd)),
            IpcAction::None,
        ),
    }
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
            use std::process::{Command, Stdio};
            if let Ok(decoded) = base64_decode(&b64_data) {
                if let Ok(mut child) = Command::new("wl-copy").stdin(Stdio::piped()).spawn() {
                    if let Some(ref mut stdin) = child.stdin {
                        let _ = std::io::Write::write_all(stdin, &decoded);
                    }
                }
            }
        }
        state.terminal.grid.scroll_offset = 0;
    }
    total
}

fn key_to_bytes(key: &Key, _physical: &PhysicalKey, app_cursor: bool, ctrl: bool) -> Option<Vec<u8>> {
    if ctrl {
        if let Key::Character(s) = key {
            let ch = s.chars().next()?;
            if ch.is_ascii_alphabetic() {
                return Some(vec![ch.to_ascii_lowercase() as u8 - b'a' + 1]);
            }
            match ch {
                '@' | ' ' | '`' => return Some(vec![0]),
                '[' | '\x1b' => return Some(vec![0x1b]),
                '\\' => return Some(vec![0x1c]),
                ']' => return Some(vec![0x1d]),
                '^' | '~' => return Some(vec![0x1e]),
                '_' | '/' => return Some(vec![0x1f]),
                _ => {}
            }
        }
    }
    match key {
        Key::Character(s) => Some(s.as_bytes().to_vec()),
        Key::Named(named) => match named {
            NamedKey::Enter => Some(b"\r".to_vec()),
            NamedKey::Backspace => Some(b"\x7f".to_vec()),
            NamedKey::Tab => Some(b"\t".to_vec()),
            NamedKey::Escape => Some(b"\x1b".to_vec()),
            NamedKey::ArrowUp if app_cursor => Some(b"\x1bOA".to_vec()),
            NamedKey::ArrowDown if app_cursor => Some(b"\x1bOB".to_vec()),
            NamedKey::ArrowRight if app_cursor => Some(b"\x1bOC".to_vec()),
            NamedKey::ArrowLeft if app_cursor => Some(b"\x1bOD".to_vec()),
            NamedKey::Home if app_cursor => Some(b"\x1bOH".to_vec()),
            NamedKey::End if app_cursor => Some(b"\x1bOF".to_vec()),
            NamedKey::ArrowUp => Some(b"\x1b[A".to_vec()),
            NamedKey::ArrowDown => Some(b"\x1b[B".to_vec()),
            NamedKey::ArrowRight => Some(b"\x1b[C".to_vec()),
            NamedKey::ArrowLeft => Some(b"\x1b[D".to_vec()),
            NamedKey::Home => Some(b"\x1b[H".to_vec()),
            NamedKey::End => Some(b"\x1b[F".to_vec()),
            NamedKey::Delete => Some(b"\x1b[3~".to_vec()),
            NamedKey::PageUp => Some(b"\x1b[5~".to_vec()),
            NamedKey::PageDown => Some(b"\x1b[6~".to_vec()),
            NamedKey::Space => {
                if ctrl {
                    Some(vec![0])
                } else {
                    Some(b" ".to_vec())
                }
            }
            NamedKey::Insert => Some(b"\x1b[2~".to_vec()),
            NamedKey::F1 => Some(b"\x1bOP".to_vec()),
            NamedKey::F2 => Some(b"\x1bOQ".to_vec()),
            NamedKey::F3 => Some(b"\x1bOR".to_vec()),
            NamedKey::F4 => Some(b"\x1bOS".to_vec()),
            NamedKey::F5 => Some(b"\x1b[15~".to_vec()),
            NamedKey::F6 => Some(b"\x1b[17~".to_vec()),
            NamedKey::F7 => Some(b"\x1b[18~".to_vec()),
            NamedKey::F8 => Some(b"\x1b[19~".to_vec()),
            NamedKey::F9 => Some(b"\x1b[20~".to_vec()),
            NamedKey::F10 => Some(b"\x1b[21~".to_vec()),
            NamedKey::F11 => Some(b"\x1b[23~".to_vec()),
            NamedKey::F12 => Some(b"\x1b[24~".to_vec()),
            _ => None,
        },
        _ => None,
    }
}

const PALETTE: [u32; 16] = [
    0x000000, // 0 black
    0xcc0000, // 1 red
    0x4e9a06, // 2 green
    0xc4a000, // 3 yellow
    0x3465a4, // 4 blue
    0x75507b, // 5 magenta
    0x06989a, // 6 cyan
    0xd3d7cf, // 7 white
    0x555753, // 8 bright black
    0xef2929, // 9 bright red
    0x8ae234, // 10 bright green
    0xfce94f, // 11 bright yellow
    0x729fcf, // 12 bright blue
    0xad7fa8, // 13 bright magenta
    0x34e2e2, // 14 bright cyan
    0xeeeeec, // 15 bright white
];

fn base64_decode(input: &[u8]) -> Result<Vec<u8>, ()> {
    const TABLE: [u8; 256] = {
        let mut t = [0xffu8; 256];
        let mut i = 0u8;
        while i < 26 {
            t[(b'A' + i) as usize] = i;
            t[(b'a' + i) as usize] = i + 26;
            i += 1;
        }
        let mut d = 0u8;
        while d < 10 {
            t[(b'0' + d) as usize] = d + 52;
            d += 1;
        }
        t[b'+' as usize] = 62;
        t[b'/' as usize] = 63;
        t
    };
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0u32;
    for &b in input {
        if b == b'=' || b == b'\n' || b == b'\r' {
            continue;
        }
        let val = TABLE[b as usize];
        if val == 0xff {
            return Err(());
        }
        buf = (buf << 6) | val as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

fn color_to_rgb(c: u32) -> u32 {
    use crate::grid::COLOR_FLAG_RGB;
    if c & COLOR_FLAG_RGB != 0 {
        c & 0x00FF_FFFF
    } else {
        let idx = c as u8;
        if (idx as usize) < PALETTE.len() {
            PALETTE[idx as usize]
        } else if (16..232).contains(&idx) {
            let v = idx - 16;
            let r = (v / 36) * 51;
            let g = ((v % 36) / 6) * 51;
            let b = (v % 6) * 51;
            ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
        } else if idx >= 232 {
            let v = 8 + (idx - 232) as u32 * 10;
            (v << 16) | (v << 8) | v
        } else {
            0xffffff
        }
    }
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

    let base_bg = config.style.background.as_u32_rgb();
    let base_fg = config.style.foreground.as_u32_rgb();

    let grid = &state.terminal.grid;
    let atlas = &mut state.atlas;

    buffer.fill(base_bg);

    let (cursor_col, cursor_row) = grid.cursor_pos();
    let show_cursor = state.terminal.cursor_visible && grid.scroll_offset == 0;
    let cursor_style = state.terminal.cursor_style;

    for row in 0..grid.rows {
        for col in 0..grid.cols {
            let cell = grid.cell_at_scroll(row, col);
            let is_cursor = show_cursor && row == cursor_row && col == cursor_col;

            let has_content = cell.ch > 0x20;
            let has_custom_bg = cell.bg != crate::grid::COLOR_DEFAULT;
            let has_attrs = cell.attrs != 0;

            if !is_cursor && !has_content && !has_custom_bg {
                continue;
            }

            let mut fg = if cell.fg == crate::grid::COLOR_DEFAULT {
                base_fg
            } else {
                color_to_rgb(cell.fg)
            };
            let mut bg = if cell.bg == crate::grid::COLOR_DEFAULT {
                base_bg
            } else {
                color_to_rgb(cell.bg)
            };

            if has_attrs {
                use crate::grid::*;
                if cell.attrs & ATTR_BOLD != 0 && cell.fg != crate::grid::COLOR_DEFAULT && (cell.fg & crate::grid::COLOR_FLAG_RGB == 0) && (cell.fg as u8) < 8 {
                    fg = color_to_rgb(cell.fg + 8);
                }
                if cell.attrs & ATTR_DIM != 0 {
                    let r = ((fg >> 16) & 0xff) * 2 / 3;
                    let g = ((fg >> 8) & 0xff) * 2 / 3;
                    let b = (fg & 0xff) * 2 / 3;
                    fg = (r << 16) | (g << 8) | b;
                }
                if cell.attrs & ATTR_INVERSE != 0 {
                    std::mem::swap(&mut fg, &mut bg);
                }
            }

            let is_block_cursor = is_cursor && cursor_style == crate::terminal::CursorStyle::Block;
            let actual_fg = if is_block_cursor { base_bg } else { fg };
            let actual_bg = if is_block_cursor { base_fg } else { bg };

            atlas.draw_char(
                &mut buffer,
                buf_w,
                buf_h,
                col,
                row,
                cell.ch,
                actual_fg,
                actual_bg,
            );

            if has_attrs {
                use crate::grid::*;
                let px_x = col * atlas.cell_width;
                let px_y = row * atlas.cell_height;
                let cw = atlas.cell_width;
                let ch_h = atlas.cell_height;
                let draw_fg = actual_fg;

                if cell.attrs & ATTR_UNDERLINE != 0 {
                    let y = (px_y + ch_h).saturating_sub(1);
                    if y < buf_h {
                        for x in px_x..(px_x + cw).min(buf_w) {
                            buffer[y * buf_w + x] = draw_fg;
                        }
                    }
                }
                if cell.attrs & ATTR_STRIKETHROUGH != 0 {
                    let y = px_y + ch_h / 2;
                    if y < buf_h {
                        for x in px_x..(px_x + cw).min(buf_w) {
                            buffer[y * buf_w + x] = draw_fg;
                        }
                    }
                }
            }

            if is_cursor && cursor_style != crate::terminal::CursorStyle::Block {
                let px_x = col * atlas.cell_width;
                let px_y = row * atlas.cell_height;
                let cw = atlas.cell_width;
                let ch = atlas.cell_height;

                match cursor_style {
                    crate::terminal::CursorStyle::Bar => {
                        let bar_w = 2.min(cw);
                        for y in px_y..(px_y + ch).min(buf_h) {
                            for x in px_x..(px_x + bar_w).min(buf_w) {
                                buffer[y * buf_w + x] = base_fg;
                            }
                        }
                    }
                    crate::terminal::CursorStyle::Underline => {
                        let ul_h = 2.min(ch);
                        let y_start = (px_y + ch).saturating_sub(ul_h);
                        for y in y_start..(px_y + ch).min(buf_h) {
                            for x in px_x..(px_x + cw).min(buf_w) {
                                buffer[y * buf_w + x] = base_fg;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    buffer
        .present()
        .map_err(|e| anyhow::anyhow!("failed presenting frame: {e}"))?;
    Ok(())
}
