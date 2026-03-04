use crate::config::AppConfig;
use crate::pty::PtyChild;
use crate::terminal::Terminal;
use anyhow::{Context, Result};
use softbuffer::{Context as SoftContext, Surface};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, Size};
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId};

const CELL_WIDTH_PX: f64 = 9.0;
const CELL_HEIGHT_PX: f64 = 20.0;

pub fn run(config: AppConfig) -> Result<()> {
    let event_loop = EventLoop::new().context("failed to create event loop")?;
    event_loop.set_control_flow(ControlFlow::wait_duration(Duration::from_millis(8)));
    let mut app = HandtermApp::new(config);
    event_loop
        .run_app(&mut app)
        .context("failed while running app")
}

struct HandtermApp {
    config: AppConfig,
    state: Option<AppState>,
}

struct AppState {
    window: Arc<Window>,
    _context: SoftContext<Arc<Window>>,
    surface: Surface<Arc<Window>, Arc<Window>>,
    terminal: Terminal,
    pty: PtyChild,
    pty_buf: Vec<u8>,
}

impl HandtermApp {
    fn new(config: AppConfig) -> Self {
        Self {
            config,
            state: None,
        }
    }

    fn create_window_attributes(&self) -> WindowAttributes {
        let width = f64::from(self.config.window.columns) * CELL_WIDTH_PX;
        let height = f64::from(self.config.window.rows) * CELL_HEIGHT_PX;

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

        let window = Arc::new(
            event_loop
                .create_window(self.create_window_attributes())
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
                    state.window.request_redraw();
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed
                    && let Some(bytes) = key_to_bytes(&event.logical_key)
                {
                    let _ = state.pty.write_all(&bytes);
                }
            }
            WindowEvent::RedrawRequested => {
                drain_pty(state);
                render_grid(state, &self.config).expect("frame render should succeed");
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(state) = &mut self.state {
            let n = drain_pty(state);
            if n > 0 {
                state.window.request_redraw();
            }
        }
    }
}

fn drain_pty(state: &mut AppState) -> usize {
    let mut total = 0;
    loop {
        let n = state.pty.try_read(&mut state.pty_buf).unwrap_or(0);
        if n == 0 {
            break;
        }
        state.terminal.process(&state.pty_buf[..n]);
        total += n;
    }
    total
}

fn key_to_bytes(key: &Key) -> Option<Vec<u8>> {
    match key {
        Key::Character(s) => Some(s.as_bytes().to_vec()),
        Key::Named(named) => match named {
            NamedKey::Enter => Some(b"\r".to_vec()),
            NamedKey::Backspace => Some(b"\x7f".to_vec()),
            NamedKey::Tab => Some(b"\t".to_vec()),
            NamedKey::Escape => Some(b"\x1b".to_vec()),
            NamedKey::ArrowUp => Some(b"\x1b[A".to_vec()),
            NamedKey::ArrowDown => Some(b"\x1b[B".to_vec()),
            NamedKey::ArrowRight => Some(b"\x1b[C".to_vec()),
            NamedKey::ArrowLeft => Some(b"\x1b[D".to_vec()),
            NamedKey::Home => Some(b"\x1b[H".to_vec()),
            NamedKey::End => Some(b"\x1b[F".to_vec()),
            NamedKey::Delete => Some(b"\x1b[3~".to_vec()),
            NamedKey::PageUp => Some(b"\x1b[5~".to_vec()),
            NamedKey::PageDown => Some(b"\x1b[6~".to_vec()),
            NamedKey::Space => Some(b" ".to_vec()),
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

fn color_to_rgb(idx: u8) -> u32 {
    if (idx as usize) < PALETTE.len() {
        PALETTE[idx as usize]
    } else if (16..232).contains(&idx) {
        let c = idx - 16;
        let r = (c / 36) * 51;
        let g = ((c % 36) / 6) * 51;
        let b = (c % 6) * 51;
        ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
    } else if idx >= 232 {
        let v = 8 + (idx - 232) as u32 * 10;
        (v << 16) | (v << 8) | v
    } else {
        0xffffff
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
    let cell_w = (buf_w as f64 / grid.cols.max(1) as f64).floor() as usize;
    let cell_h = (buf_h as f64 / grid.rows.max(1) as f64).floor() as usize;

    buffer.fill(base_bg);

    let (cursor_col, cursor_row) = grid.cursor_pos();

    for row in 0..grid.rows {
        for col in 0..grid.cols {
            let cell = grid.cell_at(row, col);

            let fg = if cell.fg == 0 {
                base_fg
            } else {
                color_to_rgb(cell.fg)
            };
            let bg_color = if cell.bg == 0 {
                base_bg
            } else {
                color_to_rgb(cell.bg)
            };

            let is_cursor = row == cursor_row && col == cursor_col;

            let px_x = col * cell_w;
            let px_y = row * cell_h;

            if bg_color != base_bg || is_cursor {
                let fill = if is_cursor { base_fg } else { bg_color };
                for dy in 0..cell_h {
                    let y = px_y + dy;
                    if y >= buf_h {
                        break;
                    }
                    for dx in 0..cell_w {
                        let x = px_x + dx;
                        if x >= buf_w {
                            break;
                        }
                        buffer[y * buf_w + x] = fill;
                    }
                }
            }

            let ch = cell.ch;
            if ch > 0x20 && ch < 0x7f {
                let draw_fg = if is_cursor { base_bg } else { fg };
                draw_simple_char(
                    &mut buffer,
                    buf_w,
                    buf_h,
                    px_x,
                    px_y,
                    cell_w,
                    cell_h,
                    ch as u8,
                    draw_fg,
                );
            }
        }
    }

    buffer
        .present()
        .map_err(|e| anyhow::anyhow!("failed presenting frame: {e}"))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_simple_char(
    buffer: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    px_x: usize,
    px_y: usize,
    cell_w: usize,
    cell_h: usize,
    _ch: u8,
    fg: u32,
) {
    let mid_y = px_y + cell_h / 2;
    let start_x = px_x + 1;
    let end_x = (px_x + cell_w).saturating_sub(1).min(buf_w);

    if mid_y < buf_h {
        for x in start_x..end_x {
            buffer[mid_y * buf_w + x] = fg;
        }
    }
    let above = mid_y.saturating_sub(1);
    if above < buf_h && above >= px_y {
        for x in start_x..end_x {
            buffer[above * buf_w + x] = fg;
        }
    }
}
