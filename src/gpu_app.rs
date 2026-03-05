use crate::config::AppConfig;
use crate::font::GlyphAtlas;
use crate::ipc::{IpcAction, IpcServer, Request, Response};
use crate::pty::PtyChild;
use crate::terminal::Terminal;
use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::Duration;
use wgpu::util::DeviceExt;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, Size};
use winit::event::{ElementState, Modifiers, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey, PhysicalKey};
use winit::window::{Window, WindowAttributes, WindowId};

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct CellInstance {
    pos: [f32; 2],
    uv_offset: [f32; 2],
    uv_size: [f32; 2],
    fg: [f32; 4],
    bg: [f32; 4],
    flags: u32,
    _pad: [u32; 3],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    screen_size: [f32; 2],
    cell_size: [f32; 2],
    atlas_size: [f32; 2],
    _pad: [f32; 2],
}

const ATLAS_WIDTH: u32 = 2048;
const ATLAS_HEIGHT: u32 = 2048;

const FLAG_HAS_GLYPH: u32 = 1;
const FLAG_UNDERLINE: u32 = 2;
const FLAG_STRIKETHROUGH: u32 = 4;
const FLAG_CURLY_UL: u32 = 8;
const FLAG_DOUBLE_UL: u32 = 16;
const FLAG_DOTTED_UL: u32 = 32;
const FLAG_DASHED_UL: u32 = 64;

struct GpuGlyphEntry {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    bearing_x: i32,
    bearing_y: i32,
}

pub fn run(config: AppConfig) -> Result<()> {
    let event_loop = EventLoop::new().context("failed to create event loop")?;
    event_loop.set_control_flow(ControlFlow::wait_duration(Duration::from_millis(8)));

    let socket_path = crate::ipc::default_socket_path();
    let ipc = IpcServer::bind(&socket_path).ok();
    if let Some(ref ipc) = ipc {
        eprintln!("handterm: listening on {}", ipc.path().display());
    }

    let mut app = GpuApp::new(config, ipc);
    event_loop
        .run_app(&mut app)
        .context("failed while running app")
}

struct GpuApp {
    config: AppConfig,
    state: Option<GpuState>,
    ipc: Option<IpcServer>,
}

struct GpuState {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniform_buffer: wgpu::Buffer,
    instance_buffer: wgpu::Buffer,
    max_instances: usize,
    atlas_texture: wgpu::Texture,
    glyph_map: std::collections::HashMap<u32, GpuGlyphEntry>,
    atlas_cursor_x: u32,
    atlas_cursor_y: u32,
    atlas_row_height: u32,
    terminal: Terminal,
    pty: PtyChild,
    pty_buf: Vec<u8>,
    atlas: GlyphAtlas,
    pty_closed: bool,
    modifiers: Modifiers,
    mouse_col: usize,
    mouse_row: usize,
    selecting: bool,
}

impl GpuApp {
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
            .with_title("handterm [gpu]")
            .with_transparent(self.config.style.background_opacity < 1.0)
            .with_inner_size(Size::Logical(LogicalSize::new(width, height)))
    }
}

const SHADER: &str = r#"
struct Uniforms {
    screen_size: vec2<f32>,
    cell_size: vec2<f32>,
    atlas_size: vec2<f32>,
    _pad: vec2<f32>,
};

struct CellInstance {
    @location(0) pos: vec2<f32>,
    @location(1) uv_offset: vec2<f32>,
    @location(2) uv_size: vec2<f32>,
    @location(3) fg: vec4<f32>,
    @location(4) bg: vec4<f32>,
    @location(5) flags: u32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) fg: vec4<f32>,
    @location(2) bg: vec4<f32>,
    @location(3) flags: u32,
    @location(4) local_pos: vec2<f32>,
    @location(5) cell_size: vec2<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;
@group(0) @binding(1) var atlas_tex: texture_2d<f32>;
@group(0) @binding(2) var atlas_sampler: sampler;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, instance: CellInstance) -> VertexOutput {
    let corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    let corner = corners[vi];

    let pixel_pos = instance.pos + corner * uniforms.cell_size;
    let ndc = vec2<f32>(
        pixel_pos.x / uniforms.screen_size.x * 2.0 - 1.0,
        1.0 - pixel_pos.y / uniforms.screen_size.y * 2.0,
    );

    var out: VertexOutput;
    out.clip_position = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = (instance.uv_offset + corner * instance.uv_size) / uniforms.atlas_size;
    out.fg = instance.fg;
    out.bg = instance.bg;
    out.flags = instance.flags;
    out.local_pos = corner * uniforms.cell_size;
    out.cell_size = uniforms.cell_size;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    var color = in.bg;

    if (in.flags & 1u) != 0u {
        let alpha = textureSample(atlas_tex, atlas_sampler, in.uv).r;
        color = mix(color, in.fg, alpha);
    }

    let y = in.local_pos.y;
    let x = in.local_pos.x;
    let h = in.cell_size.y;
    let w = in.cell_size.x;

    // Underline styles
    if (in.flags & 2u) != 0u {
        let ul_y = h - 2.0;
        if y >= ul_y && y < ul_y + 1.0 {
            color = in.fg;
        }
    }
    if (in.flags & 8u) != 0u {
        let ul_y = h - 2.0;
        let phase = x / w * 6.28318530718;
        let wave = sin(phase) * 2.0;
        if abs(y - (ul_y + wave)) < 1.5 {
            color = in.fg;
        }
    }
    if (in.flags & 16u) != 0u {
        let ul_y1 = h - 2.0;
        let ul_y2 = h - 4.0;
        if (y >= ul_y1 && y < ul_y1 + 1.0) || (y >= ul_y2 && y < ul_y2 + 1.0) {
            color = in.fg;
        }
    }
    if (in.flags & 32u) != 0u {
        let ul_y = h - 2.0;
        if y >= ul_y && y < ul_y + 1.0 && u32(x) % 3u == 0u {
            color = in.fg;
        }
    }
    if (in.flags & 64u) != 0u {
        let ul_y = h - 2.0;
        let dash = u32(w) / 3u;
        let offset = u32(x);
        if y >= ul_y && y < ul_y + 1.0 && (offset < dash || (offset >= dash * 2u && offset < dash * 3u)) {
            color = in.fg;
        }
    }

    // Strikethrough
    if (in.flags & 4u) != 0u {
        let mid_y = h / 2.0;
        if y >= mid_y && y < mid_y + 1.0 {
            color = in.fg;
        }
    }

    return color;
}
"#;

impl ApplicationHandler for GpuApp {
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

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });

        let surface = instance
            .create_surface(window.clone())
            .expect("surface creation should succeed");

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .expect("no suitable GPU adapter found");

        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("handterm"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            },
        ))
        .expect("device creation should succeed");

        let size = window.inner_size();
        let surface_config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .expect("surface should be compatible");
        surface.configure(&device, &surface_config);

        let atlas_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph_atlas"),
            size: wgpu::Extent3d {
                width: ATLAS_WIDTH,
                height: ATLAS_HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let atlas_view = atlas_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let atlas_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let uniforms = Uniforms {
            screen_size: [size.width as f32, size.height as f32],
            cell_size: [atlas.cell_width as f32, atlas.cell_height as f32],
            atlas_size: [ATLAS_WIDTH as f32, ATLAS_HEIGHT as f32],
            _pad: [0.0; 2],
        };
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("uniforms"),
            contents: bytemuck::bytes_of(&uniforms),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let max_instances = (cols as usize) * (rows as usize) * 2;
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instances"),
            size: (max_instances * std::mem::size_of::<CellInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("bind_group_layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bind_group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&atlas_sampler),
                },
            ],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            immediate_size: 0,
        });

        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<CellInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    offset: 8,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    offset: 16,
                    shader_location: 2,
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    offset: 24,
                    shader_location: 3,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: 40,
                    shader_location: 4,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: 56,
                    shader_location: 5,
                    format: wgpu::VertexFormat::Uint32,
                },
            ],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("render_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[instance_layout],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let terminal = Terminal::new(cols, rows);
        let pty = PtyChild::spawn_default_shell(cols, rows).expect("pty should spawn");

        self.state = Some(GpuState {
            window,
            surface,
            device,
            queue,
            surface_config,
            pipeline,
            bind_group,
            uniform_buffer,
            instance_buffer,
            max_instances,
            atlas_texture,
            glyph_map: std::collections::HashMap::with_capacity(256),
            atlas_cursor_x: 0,
            atlas_cursor_y: 0,
            atlas_row_height: 0,
            terminal,
            pty,
            pty_buf: vec![0u8; 64 * 1024],
            atlas,
            pty_closed: false,
            modifiers: Modifiers::default(),
            mouse_col: 0,
            mouse_row: 0,
            selecting: false,
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
                if size.width > 0 && size.height > 0 {
                    state.surface_config.width = size.width;
                    state.surface_config.height = size.height;
                    state.surface.configure(&state.device, &state.surface_config);

                    let new_cols =
                        (size.width as usize / state.atlas.cell_width.max(1)) as u16;
                    let new_rows =
                        (size.height as usize / state.atlas.cell_height.max(1)) as u16;
                    let new_cols = new_cols.max(1);
                    let new_rows = new_rows.max(1);

                    if new_cols != state.terminal.cols || new_rows != state.terminal.rows {
                        state.terminal.resize(new_cols, new_rows);
                        let _ = state.pty.resize(new_cols, new_rows);
                    }

                    let needed = (new_cols as usize) * (new_rows as usize) * 2;
                    if needed > state.max_instances {
                        state.max_instances = needed;
                        state.instance_buffer =
                            state.device.create_buffer(&wgpu::BufferDescriptor {
                                label: Some("instances"),
                                size: (needed * std::mem::size_of::<CellInstance>()) as u64,
                                usage: wgpu::BufferUsages::VERTEX
                                    | wgpu::BufferUsages::COPY_DST,
                                mapped_at_creation: false,
                            });
                    }

                    let uniforms = Uniforms {
                        screen_size: [size.width as f32, size.height as f32],
                        cell_size: [
                            state.atlas.cell_width as f32,
                            state.atlas.cell_height as f32,
                        ],
                        atlas_size: [ATLAS_WIDTH as f32, ATLAS_HEIGHT as f32],
                        _pad: [0.0; 2],
                    };
                    state
                        .queue
                        .write_buffer(&state.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

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
                                            let _ =
                                                std::io::Write::write_all(stdin, text.as_bytes());
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
                            state.terminal.grid.scroll_offset =
                                (state.terminal.grid.scroll_offset + half).min(max);
                            state.window.request_redraw();
                            return;
                        }
                        if let Key::Named(NamedKey::PageDown) = &event.logical_key {
                            let half = state.terminal.rows as usize / 2;
                            state.terminal.grid.scroll_offset =
                                state.terminal.grid.scroll_offset.saturating_sub(half);
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
                    state.window.request_redraw();
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
                            if cell.hyperlink_id != 0 {
                                if let Some(url) =
                                    state.terminal.grid.hyperlink_url(cell.hyperlink_id)
                                {
                                    let _ = std::process::Command::new("xdg-open")
                                        .arg(url)
                                        .spawn();
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
                        state.window.request_redraw();
                    } else {
                        state.selecting = false;
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
                    }
                }

                if state.terminal.mouse_mode != crate::terminal::MouseMode::Off {
                    if let Some(bytes) = state.terminal.encode_mouse(
                        btn,
                        state.mouse_col,
                        state.mouse_row,
                        pressed,
                    ) {
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
                        if let Some(bytes) = state.terminal.encode_mouse_scroll(
                            up,
                            state.mouse_col,
                            state.mouse_row,
                        ) {
                            let _ = state.pty.write_all(&bytes);
                        }
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
                    state.window.request_redraw();
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
                drain_pty(state);
                render_gpu(state, &self.config);
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

fn handle_ipc_request(req: &Request, state: &mut GpuState) -> (Response, IpcAction) {
    match req.cmd.as_str() {
        "get-text" => {
            let text = state.terminal.grid.get_all_text();
            (Response::ok(serde_json::json!({ "text": text })), IpcAction::None)
        }
        "send-text" => {
            let text = req.args.as_object()
                .and_then(|o| o.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if text.is_empty() {
                (Response::err("missing 'text' argument"), IpcAction::None)
            } else {
                (Response::ok_empty(), IpcAction::SendText(text.as_bytes().to_vec()))
            }
        }
        _ => (Response::err(format!("unknown command: {}", req.cmd)), IpcAction::None),
    }
}

fn drain_pty(state: &mut GpuState) -> usize {
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
                let mut child = std::process::Command::new("wl-copy")
                    .stdin(std::process::Stdio::piped())
                    .spawn()
                    .ok();
                if let Some(ref mut c) = child {
                    if let Some(ref mut stdin) = c.stdin {
                        let _ = std::io::Write::write_all(stdin, &decoded);
                    }
                }
            }
        }
    }
    total
}

fn ensure_glyph_in_atlas(state: &mut GpuState, ch: u32) -> Option<&GpuGlyphEntry> {
    if state.glyph_map.contains_key(&ch) {
        return state.glyph_map.get(&ch);
    }

    if !state.atlas.ensure_glyph(ch) {
        return None;
    }

    let glyph = state.atlas.get_glyph(ch)?;

    if glyph.width == 0 || glyph.height == 0 {
        state.glyph_map.insert(
            ch,
            GpuGlyphEntry {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
                bearing_x: glyph.bearing_x,
                bearing_y: glyph.bearing_y,
            },
        );
        return state.glyph_map.get(&ch);
    }

    if state.atlas_cursor_x + glyph.width as u32 > ATLAS_WIDTH {
        state.atlas_cursor_x = 0;
        state.atlas_cursor_y += state.atlas_row_height;
        state.atlas_row_height = 0;
    }

    if state.atlas_cursor_y + glyph.height as u32 > ATLAS_HEIGHT {
        return None;
    }

    state.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &state.atlas_texture,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: state.atlas_cursor_x,
                y: state.atlas_cursor_y,
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        },
        &glyph.bitmap,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(glyph.width as u32),
            rows_per_image: None,
        },
        wgpu::Extent3d {
            width: glyph.width as u32,
            height: glyph.height as u32,
            depth_or_array_layers: 1,
        },
    );

    let entry = GpuGlyphEntry {
        x: state.atlas_cursor_x,
        y: state.atlas_cursor_y,
        width: glyph.width as u32,
        height: glyph.height as u32,
        bearing_x: glyph.bearing_x,
        bearing_y: glyph.bearing_y,
    };

    state.atlas_cursor_x += glyph.width as u32 + 1;
    state.atlas_row_height = state.atlas_row_height.max(glyph.height as u32 + 1);

    state.glyph_map.insert(ch, entry);
    state.glyph_map.get(&ch)
}

fn color_to_rgb_f32(c: u32) -> [f32; 4] {
    let rgb = crate::color::to_rgb(c);
    [
        ((rgb >> 16) & 0xff) as f32 / 255.0,
        ((rgb >> 8) & 0xff) as f32 / 255.0,
        (rgb & 0xff) as f32 / 255.0,
        1.0,
    ]
}

fn render_gpu(state: &mut GpuState, config: &AppConfig) {
    let output = match state.surface.get_current_texture() {
        Ok(t) => t,
        Err(wgpu::SurfaceError::Lost) => {
            state.surface.configure(&state.device, &state.surface_config);
            return;
        }
        Err(_) => return,
    };

    let view = output
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());

    let base_bg = config.style.background.as_u32_rgb();
    let base_fg = config.style.foreground.as_u32_rgb();
    let base_bg_f = [
        ((base_bg >> 16) & 0xff) as f32 / 255.0,
        ((base_bg >> 8) & 0xff) as f32 / 255.0,
        (base_bg & 0xff) as f32 / 255.0,
        1.0,
    ];
    let base_fg_f = [
        ((base_fg >> 16) & 0xff) as f32 / 255.0,
        ((base_fg >> 8) & 0xff) as f32 / 255.0,
        (base_fg & 0xff) as f32 / 255.0,
        1.0,
    ];

    let cell_w = state.atlas.cell_width as f32;
    let cell_h = state.atlas.cell_height as f32;
    let grid_rows = state.terminal.grid.rows;
    let grid_cols = state.terminal.grid.cols;
    let (cursor_col, cursor_row) = state.terminal.grid.cursor_pos();
    let show_cursor = state.terminal.cursor_visible && state.terminal.grid.scroll_offset == 0;
    let cursor_style = state.terminal.cursor_style;

    struct CellInfo {
        row: usize,
        col: usize,
        ch: u32,
        fg: u32,
        bg: u32,
        attrs: u8,
        underline_style: crate::grid::UnderlineStyle,
        is_cursor_block: bool,
    }

    let mut cell_infos: Vec<CellInfo> = Vec::with_capacity(grid_rows * grid_cols);

    for row in 0..grid_rows {
        for col in 0..grid_cols {
            let cell = state.terminal.grid.cell_at_scroll(row, col);
            if cell.flags & crate::grid::FLAG_WIDE_CONT != 0 {
                continue;
            }

            let is_cursor = show_cursor && row == cursor_row && col == cursor_col;
            let is_cursor_block = is_cursor && cursor_style == crate::terminal::CursorStyle::Block;

            cell_infos.push(CellInfo {
                row,
                col,
                ch: cell.ch,
                fg: cell.fg,
                bg: cell.bg,
                attrs: cell.attrs,
                underline_style: cell.underline_style,
                is_cursor_block,
            });
        }
    }

    let mut instances: Vec<CellInstance> = Vec::with_capacity(cell_infos.len());

    for ci in &cell_infos {
        let mut fg = if ci.fg == crate::grid::COLOR_DEFAULT {
            base_fg_f
        } else {
            color_to_rgb_f32(ci.fg)
        };
        let mut bg = if ci.bg == crate::grid::COLOR_DEFAULT {
            base_bg_f
        } else {
            color_to_rgb_f32(ci.bg)
        };

        if ci.attrs & crate::grid::ATTR_INVERSE != 0 {
            std::mem::swap(&mut fg, &mut bg);
        }
        if ci.is_cursor_block {
            std::mem::swap(&mut fg, &mut bg);
        }

        let mut flags = 0u32;
        let has_glyph = ci.ch > 0x20;

        let (uv_offset, uv_size) = if has_glyph {
            if let Some(entry) = ensure_glyph_in_atlas(state, ci.ch) {
                if entry.width > 0 {
                    flags |= FLAG_HAS_GLYPH;
                    (
                        [entry.x as f32, entry.y as f32],
                        [entry.width as f32, entry.height as f32],
                    )
                } else {
                    ([0.0, 0.0], [0.0, 0.0])
                }
            } else {
                ([0.0, 0.0], [0.0, 0.0])
            }
        } else {
            ([0.0, 0.0], [0.0, 0.0])
        };

        if ci.attrs & crate::grid::ATTR_UNDERLINE != 0 {
            use crate::grid::UnderlineStyle;
            match ci.underline_style {
                UnderlineStyle::None | UnderlineStyle::Single => flags |= FLAG_UNDERLINE,
                UnderlineStyle::Double => flags |= FLAG_DOUBLE_UL,
                UnderlineStyle::Curly => flags |= FLAG_CURLY_UL,
                UnderlineStyle::Dotted => flags |= FLAG_DOTTED_UL,
                UnderlineStyle::Dashed => flags |= FLAG_DASHED_UL,
            }
        }
        if ci.attrs & crate::grid::ATTR_STRIKETHROUGH != 0 {
            flags |= FLAG_STRIKETHROUGH;
        }

        instances.push(CellInstance {
            pos: [ci.col as f32 * cell_w, ci.row as f32 * cell_h],
            uv_offset,
            uv_size,
            fg,
            bg,
            flags,
            _pad: [0; 3],
        });
    }

    let instance_data = bytemuck::cast_slice(&instances);
    state
        .queue
        .write_buffer(&state.instance_buffer, 0, instance_data);

    let mut encoder = state
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("render"),
        });

    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("main"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: base_bg_f[0] as f64,
                        g: base_bg_f[1] as f64,
                        b: base_bg_f[2] as f64,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            ..Default::default()
        });

        pass.set_pipeline(&state.pipeline);
        pass.set_bind_group(0, &state.bind_group, &[]);
        pass.set_vertex_buffer(0, state.instance_buffer.slice(..));
        pass.draw(0..6, 0..instances.len() as u32);
    }

    state.queue.submit(std::iter::once(encoder.finish()));
    output.present();
    state.terminal.grid.clear_dirty();
}

fn key_to_bytes(
    key: &Key,
    _physical: &PhysicalKey,
    app_cursor: bool,
    ctrl: bool,
) -> Option<Vec<u8>> {
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
