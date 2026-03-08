use crate::config::AppConfig;
use crate::font::{GlyphAtlas, GlyphFormat};
use crate::frontend::{
    FrameScheduler, VisualState, base64_decode, copy_to_clipboard, handle_ipc_request,
    key_to_bytes, open_url, paste_from_clipboard, scroll_to_bytes, spawn_pty_watcher,
};
use crate::ipc::{IpcAction, IpcServer};
use crate::pty::PtyChild;
use crate::terminal::Terminal;
use crate::visual::{is_in_selection, resolve_cell_colors, resolve_underline_color};
use anyhow::{Context, Result};
use std::time::{Duration, Instant};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use wgpu::util::DeviceExt;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, Size};
use winit::event::{ElementState, Modifiers, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::platform::wayland::WindowAttributesExtWayland;
use winit::window::{Window, WindowAttributes, WindowId};

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct CellInstance {
    pos: [f32; 2],
    size: [f32; 2],
    uv_offset: [f32; 2],
    uv_size: [f32; 2],
    fg: [f32; 4],
    bg: [f32; 4],
    deco: [f32; 4],
    flags: u32,
    _pad: [u32; 2],
}

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct ImageInstance {
    pos: [f32; 2],
    size: [f32; 2],
    uv_offset: [f32; 2],
    uv_size: [f32; 2],
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
const FLAG_COLOR_GLYPH: u32 = 128;
const FLAG_CURSOR_BAR: u32 = 256;
const FLAG_CURSOR_UNDERLINE: u32 = 512;

struct GpuGlyphEntry {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    is_color: bool,
}

struct GpuImageEntry {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

#[derive(Debug, Clone, Copy)]
struct CellInfo {
    row: usize,
    col: usize,
    ch: u32,
    cells: usize,
    cell: crate::grid::Cell,
    selected: bool,
    is_cursor_block: bool,
    cursor_style: Option<crate::terminal::CursorStyle>,
}

#[derive(Debug, Default)]
struct FramePlan {
    cell_infos: Vec<CellInfo>,
    image_placements: Vec<crate::terminal::KittyPlacement>,
}

#[derive(Debug, Clone)]
enum GpuAppEvent {
    PtyReadable,
}

const FRAME_INTERVAL: Duration = Duration::from_millis(8);

pub fn run(config: AppConfig) -> Result<()> {
    let event_loop = EventLoop::<GpuAppEvent>::with_user_event()
        .build()
        .context("failed to create event loop")?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let proxy = event_loop.create_proxy();

    let socket_path = crate::ipc::default_socket_path();
    let ipc = IpcServer::bind(&socket_path).ok();
    if let Some(ref ipc) = ipc {
        eprintln!("handterm: listening on {}", ipc.path().display());
    }

    let mut app = GpuApp::new(config, ipc, proxy);
    event_loop
        .run_app(&mut app)
        .context("failed while running app")
}

struct GpuApp {
    config: AppConfig,
    state: Option<GpuState>,
    ipc: Option<IpcServer>,
    proxy: EventLoopProxy<GpuAppEvent>,
    watcher_started: bool,
    watcher_stop: Option<Arc<AtomicBool>>,
    scheduler: FrameScheduler,
}

struct GpuState {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    image_pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniform_buffer: wgpu::Buffer,
    bg_instance_buffer: wgpu::Buffer,
    fg_instance_buffer: wgpu::Buffer,
    image_instance_buffer: wgpu::Buffer,
    max_instances: usize,
    max_image_instances: usize,
    atlas_texture: wgpu::Texture,
    glyph_map: std::collections::HashMap<u32, GpuGlyphEntry>,
    image_map: std::collections::HashMap<u32, GpuImageEntry>,
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
    last_visual_state: Option<VisualState>,
    last_kitty_generation: u64,
}

impl GpuApp {
    fn new(config: AppConfig, ipc: Option<IpcServer>, proxy: EventLoopProxy<GpuAppEvent>) -> Self {
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

        spawn_pty_watcher(
            "pty-watcher-gpu",
            pty_fd,
            ipc_fd,
            proxy,
            GpuAppEvent::PtyReadable,
            stop,
        );
    }

    fn create_window_attributes(&self, atlas: &GlyphAtlas) -> WindowAttributes {
        let width = self.config.window.columns as f64 * atlas.cell_width as f64;
        let height = self.config.window.rows as f64 * atlas.cell_height as f64;

        Window::default_attributes()
            .with_title("handterm [gpu]")
            .with_name("handterm", "handterm")
            .with_transparent(false)
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
    @location(1) size: vec2<f32>,
    @location(2) uv_offset: vec2<f32>,
    @location(3) uv_size: vec2<f32>,
    @location(4) fg: vec4<f32>,
    @location(5) bg: vec4<f32>,
    @location(6) deco: vec4<f32>,
    @location(7) flags: u32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) fg: vec4<f32>,
    @location(2) bg: vec4<f32>,
    @location(3) deco: vec4<f32>,
    @location(4) flags: u32,
    @location(5) local_pos: vec2<f32>,
    @location(6) cell_size: vec2<f32>,
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

    let pixel_pos = instance.pos + corner * instance.size;
    let ndc = vec2<f32>(
        pixel_pos.x / uniforms.screen_size.x * 2.0 - 1.0,
        1.0 - pixel_pos.y / uniforms.screen_size.y * 2.0,
    );

    var out: VertexOutput;
    out.clip_position = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = (instance.uv_offset + corner * instance.uv_size) / uniforms.atlas_size;
    out.fg = instance.fg;
    out.bg = instance.bg;
    out.deco = instance.deco;
    out.flags = instance.flags;
    out.local_pos = corner * instance.size;
    out.cell_size = instance.size;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    var color = in.bg;

    if (in.flags & 1u) != 0u {
        let glyph = textureSample(atlas_tex, atlas_sampler, in.uv);
        if (in.flags & 128u) != 0u {
            color = glyph + color * (1.0 - glyph.a);
        } else {
            color = mix(color, in.fg, glyph.a);
        }
    }

    let y = in.local_pos.y;
    let x = in.local_pos.x;
    let h = in.cell_size.y;
    let w = in.cell_size.x;

    // Underline styles
    if (in.flags & 2u) != 0u {
        let ul_y = h - 2.0;
        if y >= ul_y && y < ul_y + 1.0 {
            color = in.deco;
        }
    }
    if (in.flags & 8u) != 0u {
        let ul_y = h - 2.0;
        let phase = x / w * 6.28318530718;
        let wave = sin(phase) * 2.0;
        if abs(y - (ul_y + wave)) < 1.5 {
            color = in.deco;
        }
    }
    if (in.flags & 16u) != 0u {
        let ul_y1 = h - 2.0;
        let ul_y2 = h - 4.0;
        if (y >= ul_y1 && y < ul_y1 + 1.0) || (y >= ul_y2 && y < ul_y2 + 1.0) {
            color = in.deco;
        }
    }
    if (in.flags & 32u) != 0u {
        let ul_y = h - 2.0;
        if y >= ul_y && y < ul_y + 1.0 && u32(x) % 3u == 0u {
            color = in.deco;
        }
    }
    if (in.flags & 64u) != 0u {
        let ul_y = h - 2.0;
        let dash = u32(w) / 3u;
        let offset = u32(x);
        if y >= ul_y && y < ul_y + 1.0 && (offset < dash || (offset >= dash * 2u && offset < dash * 3u)) {
            color = in.deco;
        }
    }

    // Strikethrough
    if (in.flags & 4u) != 0u {
        let mid_y = h / 2.0;
        if y >= mid_y && y < mid_y + 1.0 {
            color = in.fg;
        }
    }

    if (in.flags & 256u) != 0u && x < min(2.0, w) {
        color = in.fg;
    }
    if (in.flags & 512u) != 0u {
        let cursor_y = h - min(2.0, h);
        if y >= cursor_y {
            color = in.fg;
        }
    }

    return color;
}
"#;

const IMAGE_SHADER: &str = r#"
struct Uniforms {
    screen_size: vec2<f32>,
    cell_size: vec2<f32>,
    atlas_size: vec2<f32>,
    _pad: vec2<f32>,
};

struct ImageInstance {
    @location(0) pos: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) uv_offset: vec2<f32>,
    @location(3) uv_size: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;
@group(0) @binding(1) var atlas_tex: texture_2d<f32>;
@group(0) @binding(2) var atlas_sampler: sampler;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, instance: ImageInstance) -> VertexOutput {
    let corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    let corner = corners[vi];
    let pixel_pos = instance.pos + corner * instance.size;
    let ndc = vec2<f32>(
        pixel_pos.x / uniforms.screen_size.x * 2.0 - 1.0,
        1.0 - pixel_pos.y / uniforms.screen_size.y * 2.0,
    );

    var out: VertexOutput;
    out.clip_position = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = (instance.uv_offset + corner * instance.uv_size) / uniforms.atlas_size;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(atlas_tex, atlas_sampler, in.uv);
}
"#;

impl ApplicationHandler<GpuAppEvent> for GpuApp {
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
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let atlas_view = atlas_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let atlas_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
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
        let bg_instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bg_instances"),
            size: (max_instances * std::mem::size_of::<CellInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let fg_instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fg_instances"),
            size: (max_instances * std::mem::size_of::<CellInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let max_image_instances = (cols as usize) * (rows as usize);
        let image_instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("image_instances"),
            size: (max_image_instances * std::mem::size_of::<ImageInstance>()) as u64,
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
        let image_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("image_shader"),
            source: wgpu::ShaderSource::Wgsl(IMAGE_SHADER.into()),
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
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    offset: 32,
                    shader_location: 4,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: 48,
                    shader_location: 5,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: 64,
                    shader_location: 6,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: 80,
                    shader_location: 7,
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

        let image_instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<ImageInstance>() as u64,
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
                    format: wgpu::VertexFormat::Float32x2,
                },
            ],
        };

        let image_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("image_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &image_shader,
                entry_point: Some("vs_main"),
                buffers: &[image_instance_layout],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &image_shader,
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
            image_pipeline,
            bind_group,
            uniform_buffer,
            bg_instance_buffer,
            fg_instance_buffer,
            image_instance_buffer,
            max_instances,
            max_image_instances,
            atlas_texture,
            glyph_map: std::collections::HashMap::with_capacity(256),
            image_map: std::collections::HashMap::with_capacity(32),
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
            last_visual_state: None,
            last_kitty_generation: 0,
        });

        if let Some(s) = &self.state {
            s.window.request_redraw();
        }

        self.start_pty_watcher();
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: GpuAppEvent) {
        match event {
            GpuAppEvent::PtyReadable => {
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
                        state.bg_instance_buffer =
                            state.device.create_buffer(&wgpu::BufferDescriptor {
                                label: Some("bg_instances"),
                                size: (needed * std::mem::size_of::<CellInstance>()) as u64,
                                usage: wgpu::BufferUsages::VERTEX
                                    | wgpu::BufferUsages::COPY_DST,
                                mapped_at_creation: false,
                            });
                        state.fg_instance_buffer =
                            state.device.create_buffer(&wgpu::BufferDescriptor {
                                label: Some("fg_instances"),
                                size: (needed * std::mem::size_of::<CellInstance>()) as u64,
                                usage: wgpu::BufferUsages::VERTEX
                                    | wgpu::BufferUsages::COPY_DST,
                                mapped_at_creation: false,
                            });
                    }
                    let needed_images = (new_cols as usize) * (new_rows as usize);
                    if needed_images > state.max_image_instances {
                        state.max_image_instances = needed_images;
                        state.image_instance_buffer =
                            state.device.create_buffer(&wgpu::BufferDescriptor {
                                label: Some("image_instances"),
                                size: (needed_images * std::mem::size_of::<ImageInstance>()) as u64,
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
                render_gpu(state, &self.config);
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
    state: &mut GpuState,
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
                copy_to_clipboard(&decoded);
            }
        }
    }
    total
}

fn build_gpu_glyph_tile(
    glyph: &crate::font::GlyphData<'_>,
    cells: usize,
    cell_width: usize,
    cell_height: usize,
    baseline: usize,
) -> (Vec<u8>, u32, u32) {
    let tile_width = (cells.max(1) * cell_width) as u32;
    let tile_height = cell_height as u32;
    let mut tile = vec![0u8; tile_width as usize * tile_height as usize * 4];
    let origin_y = cell_height as i32 - baseline as i32;
    let glyph_top = origin_y - glyph.bearing_y;
    let glyph_left = glyph.bearing_x;

    for gy in 0..glyph.height {
        let dst_y = glyph_top + gy as i32;
        if !(0..tile_height as i32).contains(&dst_y) {
            continue;
        }
        let dst_row = dst_y as usize * tile_width as usize * 4;

        for gx in 0..glyph.width {
            let dst_x = glyph_left + gx as i32;
            if !(0..tile_width as i32).contains(&dst_x) {
                continue;
            }
            let dst_offset = dst_row + dst_x as usize * 4;
            match glyph.format {
                GlyphFormat::Alpha => {
                    let alpha = glyph.pixels[gy * glyph.width + gx];
                    tile[dst_offset] = 0xff;
                    tile[dst_offset + 1] = 0xff;
                    tile[dst_offset + 2] = 0xff;
                    tile[dst_offset + 3] = alpha;
                }
                GlyphFormat::Rgba => {
                    let src_offset = (gy * glyph.width + gx) * 4;
                    tile[dst_offset..dst_offset + 4]
                        .copy_from_slice(&glyph.pixels[src_offset..src_offset + 4]);
                }
            }
        }
    }

    (tile, tile_width, tile_height)
}

fn ensure_glyph_in_atlas(state: &mut GpuState, ch: u32, cells: usize) -> Option<&GpuGlyphEntry> {
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
                is_color: glyph.format == GlyphFormat::Rgba,
            },
        );
        return state.glyph_map.get(&ch);
    }

    let is_color = glyph.format == GlyphFormat::Rgba;
    let (upload, upload_width, upload_height) = build_gpu_glyph_tile(
        &glyph,
        cells,
        state.atlas.cell_width,
        state.atlas.cell_height,
        state.atlas.baseline,
    );

    if state.atlas_cursor_x + upload_width > ATLAS_WIDTH {
        state.atlas_cursor_x = 0;
        state.atlas_cursor_y += state.atlas_row_height;
        state.atlas_row_height = 0;
    }

    if state.atlas_cursor_y + upload_height > ATLAS_HEIGHT {
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
        &upload,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(upload_width * 4),
            rows_per_image: None,
        },
        wgpu::Extent3d {
            width: upload_width,
            height: upload_height,
            depth_or_array_layers: 1,
        },
    );

    let entry = GpuGlyphEntry {
        x: state.atlas_cursor_x,
        y: state.atlas_cursor_y,
        width: upload_width,
        height: upload_height,
        is_color,
    };

    state.atlas_cursor_x += upload_width + 1;
    state.atlas_row_height = state.atlas_row_height.max(upload_height + 1);

    state.glyph_map.insert(ch, entry);
    state.glyph_map.get(&ch)
}

fn ensure_kitty_image_in_atlas(state: &mut GpuState, image_id: u32) -> Option<&GpuImageEntry> {
    if state.last_kitty_generation != state.terminal.kitty_generation() {
        state.image_map.clear();
        state.last_kitty_generation = state.terminal.kitty_generation();
    }

    if state.image_map.contains_key(&image_id) {
        return state.image_map.get(&image_id);
    }

    let image = state.terminal.kitty_image(image_id)?;
    if image.width == 0 || image.height == 0 {
        return None;
    }
    if image.data.len() != (image.width as usize) * (image.height as usize) * 4 {
        return None;
    }

    if state.atlas_cursor_x + image.width > ATLAS_WIDTH {
        state.atlas_cursor_x = 0;
        state.atlas_cursor_y += state.atlas_row_height;
        state.atlas_row_height = 0;
    }

    if state.atlas_cursor_y + image.height > ATLAS_HEIGHT {
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
        &image.data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(image.width * 4),
            rows_per_image: None,
        },
        wgpu::Extent3d {
            width: image.width,
            height: image.height,
            depth_or_array_layers: 1,
        },
    );

    let entry = GpuImageEntry {
        x: state.atlas_cursor_x,
        y: state.atlas_cursor_y,
        width: image.width,
        height: image.height,
    };

    state.atlas_cursor_x += image.width + 1;
    state.atlas_row_height = state.atlas_row_height.max(image.height + 1);

    state.image_map.insert(image_id, entry);
    state.image_map.get(&image_id)
}

fn rgb_to_f32(rgb: u32) -> [f32; 4] {
    [
        ((rgb >> 16) & 0xff) as f32 / 255.0,
        ((rgb >> 8) & 0xff) as f32 / 255.0,
        (rgb & 0xff) as f32 / 255.0,
        1.0,
    ]
}

fn image_instance_for_placement(
    placement: &crate::terminal::KittyPlacement,
    entry: &GpuImageEntry,
    cell_w: f32,
    cell_h: f32,
) -> ImageInstance {
    ImageInstance {
        pos: [placement.col as f32 * cell_w, placement.row as f32 * cell_h],
        size: [
            placement.cols.max(1) as f32 * cell_w,
            placement.rows.max(1) as f32 * cell_h,
        ],
        uv_offset: [entry.x as f32, entry.y as f32],
        uv_size: [entry.width as f32, entry.height as f32],
    }
}

fn cell_span(cell: &crate::grid::Cell) -> usize {
    if cell.flags & crate::grid::FLAG_WIDE != 0 {
        2
    } else {
        1
    }
}

fn build_frame_plan(terminal: &Terminal) -> FramePlan {
    let grid = &terminal.grid;
    let (cursor_col, cursor_row) = grid.cursor_pos();
    let show_cursor = terminal.cursor_visible && grid.scroll_offset == 0;
    let cursor_style = terminal.cursor_style;
    let selection = grid.selection;

    let mut cell_infos = Vec::with_capacity(grid.rows * grid.cols);
    for row in 0..grid.rows {
        for col in 0..grid.cols {
            let cell = grid.cell_at_scroll(row, col);
            if cell.flags & crate::grid::FLAG_WIDE_CONT != 0 {
                continue;
            }

            let is_cursor = show_cursor && row == cursor_row && col == cursor_col;
            let is_cursor_block = is_cursor && cursor_style == crate::terminal::CursorStyle::Block;
            let cursor_overlay = if is_cursor && !is_cursor_block {
                Some(cursor_style)
            } else {
                None
            };

            cell_infos.push(CellInfo {
                row,
                col,
                ch: cell.ch,
                cells: cell_span(cell),
                cell: *cell,
                selected: is_in_selection(selection, row, col),
                is_cursor_block,
                cursor_style: cursor_overlay,
            });
        }
    }

    FramePlan {
        cell_infos,
        image_placements: terminal.kitty_placements.clone(),
    }
}

fn build_cell_instances(
    ci: &CellInfo,
    base_fg: u32,
    base_bg: u32,
    base_fg_f: [f32; 4],
    glyph_entry: Option<&GpuGlyphEntry>,
    cell_w: f32,
    cell_h: f32,
) -> (CellInstance, Option<CellInstance>, Option<CellInstance>) {
    let colors = resolve_cell_colors(
        &ci.cell,
        base_fg,
        base_bg,
        ci.is_cursor_block,
        ci.selected,
    );
    let fg = rgb_to_f32(colors.fg);
    let bg = rgb_to_f32(colors.bg);
    let deco = rgb_to_f32(resolve_underline_color(&ci.cell, colors.fg));

    let mut flags = 0u32;
    let mut uv_offset = [0.0, 0.0];
    let mut uv_size = [0.0, 0.0];

    if ci.ch > 0x20
        && let Some(entry) = glyph_entry
        && entry.width > 0
    {
        flags |= FLAG_HAS_GLYPH;
        if entry.is_color {
            flags |= FLAG_COLOR_GLYPH;
        }
        uv_offset = [entry.x as f32, entry.y as f32];
        uv_size = [entry.width as f32, entry.height as f32];
    }

    if ci.cell.attrs & crate::grid::ATTR_UNDERLINE != 0 {
        use crate::grid::UnderlineStyle;
        match ci.cell.underline_style {
            UnderlineStyle::None | UnderlineStyle::Single => flags |= FLAG_UNDERLINE,
            UnderlineStyle::Double => flags |= FLAG_DOUBLE_UL,
            UnderlineStyle::Curly => flags |= FLAG_CURLY_UL,
            UnderlineStyle::Dotted => flags |= FLAG_DOTTED_UL,
            UnderlineStyle::Dashed => flags |= FLAG_DASHED_UL,
        }
    }
    if ci.cell.attrs & crate::grid::ATTR_STRIKETHROUGH != 0 {
        flags |= FLAG_STRIKETHROUGH;
    }

    let bg_instance = CellInstance {
        pos: [ci.col as f32 * cell_w, ci.row as f32 * cell_h],
        size: [cell_w * ci.cells as f32, cell_h],
        uv_offset: [0.0, 0.0],
        uv_size: [0.0, 0.0],
        fg: [0.0, 0.0, 0.0, 0.0],
        bg,
        deco: [0.0, 0.0, 0.0, 0.0],
        flags: 0,
        _pad: [0; 2],
    };

    let fg_instance = if flags != 0 {
        Some(CellInstance {
            pos: [ci.col as f32 * cell_w, ci.row as f32 * cell_h],
            size: [cell_w * ci.cells as f32, cell_h],
            uv_offset,
            uv_size,
            fg,
            bg: [0.0, 0.0, 0.0, 0.0],
            deco,
            flags,
            _pad: [0; 2],
        })
    } else {
        None
    };

    let overlay = match ci.cursor_style {
        Some(crate::terminal::CursorStyle::Bar) => Some(CellInstance {
            pos: [ci.col as f32 * cell_w, ci.row as f32 * cell_h],
            size: [cell_w, cell_h],
            uv_offset: [0.0, 0.0],
            uv_size: [0.0, 0.0],
            fg: base_fg_f,
            bg: [0.0, 0.0, 0.0, 0.0],
            deco: [0.0, 0.0, 0.0, 0.0],
            flags: FLAG_CURSOR_BAR,
            _pad: [0; 2],
        }),
        Some(crate::terminal::CursorStyle::Underline) => Some(CellInstance {
            pos: [ci.col as f32 * cell_w, ci.row as f32 * cell_h],
            size: [cell_w, cell_h],
            uv_offset: [0.0, 0.0],
            uv_size: [0.0, 0.0],
            fg: base_fg_f,
            bg: [0.0, 0.0, 0.0, 0.0],
            deco: [0.0, 0.0, 0.0, 0.0],
            flags: FLAG_CURSOR_UNDERLINE,
            _pad: [0; 2],
        }),
        _ => None,
    };

    (bg_instance, fg_instance, overlay)
}

fn render_gpu(state: &mut GpuState, config: &AppConfig) {
    let current_visual = VisualState::capture(&state.terminal);
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
    let frame_plan = build_frame_plan(&state.terminal);
    let cell_infos = frame_plan.cell_infos;

    let mut bg_instances: Vec<CellInstance> = Vec::with_capacity(cell_infos.len());
    let mut fg_instances: Vec<CellInstance> = Vec::with_capacity(cell_infos.len());
    let mut overlay_instances: Vec<CellInstance> = Vec::with_capacity(1);

    for ci in &cell_infos {
        let glyph_entry = if ci.ch > 0x20 {
            ensure_glyph_in_atlas(state, ci.ch, ci.cells)
        } else {
            None
        };
        let (bg_instance, fg_instance, overlay_instance) =
            build_cell_instances(ci, base_fg, base_bg, base_fg_f, glyph_entry, cell_w, cell_h);

        bg_instances.push(bg_instance);
        if let Some(fg_instance) = fg_instance {
            fg_instances.push(fg_instance);
        }
        if let Some(overlay_instance) = overlay_instance {
            overlay_instances.push(overlay_instance);
        }
    }

    let mut image_instances: Vec<ImageInstance> =
        Vec::with_capacity(frame_plan.image_placements.len());
    for placement in &frame_plan.image_placements {
        if let Some(entry) = ensure_kitty_image_in_atlas(state, placement.image_id) {
            image_instances.push(image_instance_for_placement(placement, entry, cell_w, cell_h));
        }
    }

    state.queue.write_buffer(
        &state.bg_instance_buffer,
        0,
        bytemuck::cast_slice(&bg_instances),
    );
    state.queue.write_buffer(
        &state.fg_instance_buffer,
        0,
        bytemuck::cast_slice(&fg_instances),
    );
    if !overlay_instances.is_empty() {
        let offset = (fg_instances.len() * std::mem::size_of::<CellInstance>()) as u64;
        state.queue.write_buffer(
            &state.fg_instance_buffer,
            offset,
            bytemuck::cast_slice(&overlay_instances),
        );
    }
    if !image_instances.is_empty() {
        state.queue.write_buffer(
            &state.image_instance_buffer,
            0,
            bytemuck::cast_slice(&image_instances),
        );
    }

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

        pass.set_bind_group(0, &state.bind_group, &[]);
        pass.set_pipeline(&state.pipeline);
        pass.set_vertex_buffer(0, state.bg_instance_buffer.slice(..));
        pass.draw(0..6, 0..bg_instances.len() as u32);

        if !image_instances.is_empty() {
            pass.set_pipeline(&state.image_pipeline);
            pass.set_vertex_buffer(0, state.image_instance_buffer.slice(..));
            pass.draw(0..6, 0..image_instances.len() as u32);
        }

        if !fg_instances.is_empty() {
            pass.set_pipeline(&state.pipeline);
            pass.set_vertex_buffer(0, state.fg_instance_buffer.slice(..));
            pass.draw(0..6, 0..fg_instances.len() as u32);
        }
        if !overlay_instances.is_empty() {
            pass.set_pipeline(&state.pipeline);
            pass.set_vertex_buffer(0, state.fg_instance_buffer.slice(..));
            let start = fg_instances.len() as u32;
            let end = (fg_instances.len() + overlay_instances.len()) as u32;
            pass.draw(0..6, start..end);
        }
    }

    state.queue.submit(std::iter::once(encoder.finish()));
    output.present();
    state.terminal.grid.clear_dirty();
    state.last_visual_state = Some(current_visual);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kitty_placement_maps_to_image_instance_geometry() {
        let placement = crate::terminal::KittyPlacement {
            image_id: 7,
            col: 2,
            row: 1,
            cols: 3,
            rows: 2,
        };
        let entry = GpuImageEntry {
            x: 10,
            y: 20,
            width: 30,
            height: 40,
        };

        let instance = image_instance_for_placement(&placement, &entry, 8.0, 16.0);

        assert_eq!(
            instance,
            ImageInstance {
                pos: [16.0, 16.0],
                size: [24.0, 32.0],
                uv_offset: [10.0, 20.0],
                uv_size: [30.0, 40.0],
            }
        );
    }

    #[test]
    fn frame_plan_tracks_wide_cells_selection_and_cursor_overlay() {
        let mut terminal = Terminal::new(4, 2);
        terminal.process("a界".as_bytes());
        terminal.grid.selection = Some(crate::grid::Selection {
            start_col: 1,
            start_row: 0,
            end_col: 1,
            end_row: 0,
        });
        terminal.cursor_style = crate::terminal::CursorStyle::Bar;
        terminal.grid.set_cursor(0, 0);

        let plan = build_frame_plan(&terminal);
        assert_eq!(plan.cell_infos.len(), 7);

        let ascii = plan
            .cell_infos
            .iter()
            .find(|ci| ci.row == 0 && ci.col == 0)
            .expect("ascii cell should exist");
        assert_eq!(ascii.cells, 1);
        assert_eq!(ascii.cursor_style, Some(crate::terminal::CursorStyle::Bar));

        let wide = plan
            .cell_infos
            .iter()
            .find(|ci| ci.row == 0 && ci.col == 1)
            .expect("wide cell should exist");
        assert_eq!(wide.cells, 2);
        assert!(wide.selected);
    }

    #[test]
    fn gpu_glyph_tile_spans_requested_cells() {
        let pixels = [255u8, 128, 64, 32];
        let glyph = crate::font::GlyphData {
            pixels: &pixels,
            width: 2,
            height: 2,
            format: GlyphFormat::Alpha,
            bearing_x: 0,
            bearing_y: 2,
        };

        let (tile, width, height) = build_gpu_glyph_tile(&glyph, 2, 8, 4, 2);

        assert_eq!(width, 16);
        assert_eq!(height, 4);
        assert_eq!(tile.len(), 16 * 4 * 4);
        assert_eq!(&tile[0..8], &[0xff, 0xff, 0xff, 255, 0xff, 0xff, 0xff, 128]);
    }

    #[test]
    fn build_cell_instances_respects_selection_and_custom_underline_color() {
        let mut cell = crate::grid::Cell::BLANK;
        cell.ch = 'x' as u32;
        cell.fg = 2;
        cell.bg = 4;
        cell.attrs = crate::grid::ATTR_UNDERLINE | crate::grid::ATTR_HAS_UCOLOR;
        cell.underline_color = 0x8000_ff00;
        cell.underline_style = crate::grid::UnderlineStyle::Single;

        let ci = CellInfo {
            row: 0,
            col: 0,
            ch: cell.ch,
            cells: 1,
            cell,
            selected: true,
            is_cursor_block: false,
            cursor_style: None,
        };

        let (bg, fg, overlay) = build_cell_instances(
            &ci,
            0xffffff,
            0x000000,
            [1.0, 1.0, 1.0, 1.0],
            None,
            8.0,
            16.0,
        );

        assert_eq!(bg.bg, rgb_to_f32(crate::color::to_rgb(2)));
        assert_eq!(bg.size, [8.0, 16.0]);
        let fg = fg.expect("underline pass should exist");
        assert_eq!(fg.deco, rgb_to_f32(0x00ff00));
        assert!(fg.flags & FLAG_UNDERLINE != 0);
        assert!(overlay.is_none());
    }

    #[test]
    fn build_cell_instances_emits_non_block_cursor_overlay() {
        let ci = CellInfo {
            row: 1,
            col: 2,
            ch: ' ' as u32,
            cells: 1,
            cell: crate::grid::Cell::BLANK,
            selected: false,
            is_cursor_block: false,
            cursor_style: Some(crate::terminal::CursorStyle::Bar),
        };

        let (_, fg, overlay) = build_cell_instances(
            &ci,
            0xffffff,
            0x000000,
            [0.25, 0.5, 0.75, 1.0],
            None,
            8.0,
            16.0,
        );

        assert!(fg.is_none());
        let overlay = overlay.expect("bar cursor should render as overlay");
        assert_eq!(overlay.pos, [16.0, 16.0]);
        assert_eq!(overlay.size, [8.0, 16.0]);
        assert!(overlay.flags & FLAG_CURSOR_BAR != 0);
        assert_eq!(overlay.fg, [0.25, 0.5, 0.75, 1.0]);
    }
}
