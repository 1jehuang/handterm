use crate::config::AppConfig;
use anyhow::{Context, Result};
use softbuffer::{Context as SoftContext, Surface};
use std::num::NonZeroU32;
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, Size};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowAttributes, WindowId};

const CELL_WIDTH_PX: f64 = 9.0;
const CELL_HEIGHT_PX: f64 = 20.0;

pub fn run(config: AppConfig) -> Result<()> {
    let event_loop = EventLoop::new().context("failed to create event loop")?;
    let mut app = HandtermApp::new(config);
    event_loop
        .run_app(&mut app)
        .context("failed while running app")
}

struct HandtermApp {
    config: AppConfig,
    state: Option<State>,
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

        let window = Arc::new(
            event_loop
                .create_window(self.create_window_attributes())
                .expect("window creation should succeed"),
        );

        let context =
            SoftContext::new(window.clone()).expect("softbuffer context should be created");
        let surface =
            Surface::new(&context, window.clone()).expect("softbuffer surface should be created");

        self.state = Some(State {
            window,
            _context: context,
            surface,
        });

        if let Some(state) = &self.state {
            state.window.request_redraw();
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
            WindowEvent::RedrawRequested => {
                render_frame(state, &self.config).expect("frame render should succeed");
            }
            _ => {}
        }
    }
}

struct State {
    window: Arc<Window>,
    _context: SoftContext<Arc<Window>>,
    surface: Surface<Arc<Window>, Arc<Window>>,
}

fn render_frame(state: &mut State, config: &AppConfig) -> Result<()> {
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

    let base = config.style.background.as_u32_rgb();
    let fg = config.style.foreground;
    let accent = ((fg.r as u32) << 16) | ((fg.g as u32) << 8) | fg.b as u32;

    let w = buffer.width().get() as usize;
    let h = buffer.height().get() as usize;

    for (idx, pixel) in buffer.iter_mut().enumerate() {
        let x = idx % w;
        let y = idx / w;

        // Subtle foreground-tinted gradient keeps the startup surface readable.
        let tint = (((x * 255) / (w.max(1))) as u32 + ((y * 255) / (h.max(1))) as u32) / 2;
        let blend = 8 + (tint / 32);

        let r = ((base >> 16) & 0xff) + ((((accent >> 16) & 0xff) * blend) / 255);
        let g = ((base >> 8) & 0xff) + ((((accent >> 8) & 0xff) * blend) / 255);
        let b = (base & 0xff) + (((accent & 0xff) * blend) / 255);

        *pixel = ((r.min(255)) << 16) | ((g.min(255)) << 8) | b.min(255);
    }

    buffer
        .present()
        .map_err(|e| anyhow::anyhow!("failed presenting frame: {e}"))?;
    Ok(())
}
