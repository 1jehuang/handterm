use crate::config::AppConfig;
use crate::font::{GlyphAtlas, GlyphFormat};
use crate::frontend::{VisualState, visual_signature};
use crate::gpu_frame::{
    AtlasImageRect, CellInfo, CellInstance, FrameBatchStyle, FrameTextBatches, GlyphAtlasEntry,
    ImageInstance, fill_cell_infos, fill_image_instances, fill_text_batches,
};
use crate::terminal::TerminalView;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::Arc;
use wgpu::util::DeviceExt;
use winit::dpi::{LogicalSize, Size};
use winit::event_loop::ActiveEventLoop;
use winit::platform::wayland::WindowAttributesExtWayland;
use winit::window::{ImePurpose, Window, WindowAttributes};

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    screen_size: [f32; 2],
    cell_size: [f32; 2],
    atlas_size: [f32; 2],
    _pad: [f32; 2],
}

const ATLAS_WIDTH: u32 = 2048;
const ATLAS_HEIGHT: u32 = 1024;

struct GpuGlyphEntry {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    left_pad: u32,
    is_color: bool,
}

struct GpuImageEntry {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

pub struct GpuSurfaceState {
    pub window: Arc<Window>,
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub surface_config: wgpu::SurfaceConfiguration,
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
    glyph_map: HashMap<u32, GpuGlyphEntry>,
    grapheme_map: HashMap<Box<str>, GpuGlyphEntry>,
    image_map: HashMap<u32, GpuImageEntry>,
    atlas_cursor_x: u32,
    atlas_cursor_y: u32,
    atlas_row_height: u32,
    frame_cells: Vec<CellInfo>,
    text_batches: FrameTextBatches,
    image_instances: Vec<ImageInstance>,
    last_kitty_generation: u64,
    pub last_visual_state: Option<VisualState>,
    pub last_presented_signature: Option<u64>,
}

impl GpuSurfaceState {
    pub fn surface_debug_summary(&self) -> String {
        format!(
            "surface format={:?} alpha_mode={:?} size={}x{}",
            self.surface_config.format,
            self.surface_config.alpha_mode,
            self.surface_config.width,
            self.surface_config.height
        )
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

pub fn create_window_attributes(
    config: &AppConfig,
    atlas: &GlyphAtlas,
    title: &str,
) -> WindowAttributes {
    let width = config.window.columns as f64 * atlas.cell_width as f64;
    let height = config.window.rows as f64 * atlas.cell_height as f64;

    Window::default_attributes()
        .with_title(title)
        .with_name("handterm", "handterm")
        .with_transparent(transparency_requested(config.style.background_opacity))
        .with_inner_size(Size::Logical(LogicalSize::new(width, height)))
}

pub fn create_surface_state(
    event_loop: &ActiveEventLoop,
    config: &AppConfig,
    title: &str,
    atlas: &GlyphAtlas,
) -> Result<GpuSurfaceState> {
    let window = Arc::new(
        event_loop
            .create_window(create_window_attributes(config, atlas, title))
            .context("window creation should succeed")?,
    );
    window.set_ime_allowed(true);
    window.set_ime_purpose(ImePurpose::Terminal);

    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        ..Default::default()
    });

    let surface = instance
        .create_surface(window.clone())
        .context("surface creation should succeed")?;

    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: Some(&surface),
        force_fallback_adapter: false,
    }))
    .context("no suitable GPU adapter found")?;

    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("handterm"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        ..Default::default()
    }))
    .context("device creation should succeed")?;

    let size = window.inner_size();
    let mut surface_config = surface
        .get_default_config(&adapter, size.width.max(1), size.height.max(1))
        .context("surface should be compatible")?;
    let surface_caps = surface.get_capabilities(&adapter);
    surface_config.format = select_surface_format(&surface_caps, surface_config.format);
    surface_config.alpha_mode = select_alpha_mode(
        &surface_caps,
        transparency_requested(config.style.background_opacity),
    );
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

    let max_instances = (config.window.columns as usize) * (config.window.rows as usize);
    let max_fg_instances = max_instances + 4;
    let bg_instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("bg_instances"),
        size: (max_instances * std::mem::size_of::<CellInstance>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let fg_instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("fg_instances"),
        size: (max_fg_instances * std::mem::size_of::<CellInstance>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let max_image_instances = (config.window.columns as usize) * (config.window.rows as usize);
    let image_instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("image_instances"),
        size: (max_image_instances * std::mem::size_of::<ImageInstance>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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

    Ok(GpuSurfaceState {
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
        glyph_map: HashMap::with_capacity(256),
        grapheme_map: HashMap::with_capacity(32),
        image_map: HashMap::with_capacity(32),
        atlas_cursor_x: 0,
        atlas_cursor_y: 0,
        atlas_row_height: 0,
        frame_cells: Vec::with_capacity(max_instances),
        text_batches: FrameTextBatches {
            bg_instances: Vec::with_capacity(max_instances),
            fg_instances: Vec::with_capacity(max_instances),
            overlay_instances: Vec::with_capacity(4),
        },
        image_instances: Vec::with_capacity(max_image_instances),
        last_kitty_generation: 0,
        last_visual_state: None,
        last_presented_signature: None,
    })
}

pub fn resize_surface_state(
    state: &mut GpuSurfaceState,
    atlas: &GlyphAtlas,
    width: u32,
    height: u32,
    cols: u16,
    rows: u16,
) {
    if width == 0 || height == 0 {
        return;
    }

    state.surface_config.width = width;
    state.surface_config.height = height;
    state.surface.configure(&state.device, &state.surface_config);
    state.last_presented_signature = None;

    let needed = (cols as usize) * (rows as usize) * 2;
    if needed > state.max_instances {
        state.max_instances = needed;
        let needed_fg = needed + 4;
        state.bg_instance_buffer = state.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bg_instances"),
            size: (needed * std::mem::size_of::<CellInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        state.fg_instance_buffer = state.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fg_instances"),
            size: (needed_fg * std::mem::size_of::<CellInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        state
            .text_batches
            .bg_instances
            .reserve(needed.saturating_sub(state.text_batches.bg_instances.capacity()));
        state
            .text_batches
            .fg_instances
            .reserve(needed.saturating_sub(state.text_batches.fg_instances.capacity()));
        state
            .frame_cells
            .reserve(needed.saturating_sub(state.frame_cells.capacity()));
    }

    let needed_images = (cols as usize) * (rows as usize);
    if needed_images > state.max_image_instances {
        state.max_image_instances = needed_images;
        state.image_instance_buffer = state.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("image_instances"),
            size: (needed_images * std::mem::size_of::<ImageInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        state
            .image_instances
            .reserve(needed_images.saturating_sub(state.image_instances.capacity()));
    }

    let uniforms = Uniforms {
        screen_size: [width as f32, height as f32],
        cell_size: [atlas.cell_width as f32, atlas.cell_height as f32],
        atlas_size: [ATLAS_WIDTH as f32, ATLAS_HEIGHT as f32],
        _pad: [0.0; 2],
    };
    state
        .queue
        .write_buffer(&state.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
}

pub fn render_surface_state(
    state: &mut GpuSurfaceState,
    terminal: &mut impl TerminalView,
    atlas: &mut GlyphAtlas,
    config: &AppConfig,
) {
    let current_visual = VisualState::capture(terminal);
    let signature = visual_signature(terminal);
    if state.last_presented_signature == Some(signature) {
        terminal.grid_mut().clear_dirty();
        state.last_visual_state = Some(current_visual);
        return;
    }

    let output = match state.surface.get_current_texture() {
        Ok(texture) => texture,
        Err(wgpu::SurfaceError::Lost) => {
            state.surface.configure(&state.device, &state.surface_config);
            state.last_presented_signature = None;
            return;
        }
        Err(_) => return,
    };

    let view = output
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());

    let base_bg = config.style.background.as_u32_rgb();
    let base_fg = config.style.foreground.as_u32_rgb();
    let background_alpha = clamp_background_alpha(config.style.background_opacity);
    let base_bg_f = [
        ((base_bg >> 16) & 0xff) as f32 / 255.0,
        ((base_bg >> 8) & 0xff) as f32 / 255.0,
        (base_bg & 0xff) as f32 / 255.0,
        background_alpha,
    ];
    let base_fg_f = [
        ((base_fg >> 16) & 0xff) as f32 / 255.0,
        ((base_fg >> 8) & 0xff) as f32 / 255.0,
        (base_fg & 0xff) as f32 / 255.0,
        1.0,
    ];

    let cell_w = atlas.cell_width as f32;
    let cell_h = atlas.cell_height as f32;
    let mut frame_cells = std::mem::take(&mut state.frame_cells);
    fill_cell_infos(terminal, &mut frame_cells);
    let mut text_batches = std::mem::take(&mut state.text_batches);
    fill_text_batches(
        &frame_cells,
        FrameBatchStyle {
            base_fg,
            base_bg,
            base_fg_f,
            background_alpha,
            cell_w,
            cell_h,
        },
        &mut text_batches,
        |ci| {
            if let Some(ref grapheme) = ci.grapheme {
                ensure_grapheme_in_atlas(state, atlas, grapheme).map(|entry| GlyphAtlasEntry {
                    x: entry.x,
                    y: entry.y,
                    width: entry.width,
                    height: entry.height,
                    left_pad: entry.left_pad,
                    is_color: entry.is_color,
                })
            } else {
                ensure_glyph_in_atlas(state, atlas, ci.ch, ci.cells).map(|entry| GlyphAtlasEntry {
                    x: entry.x,
                    y: entry.y,
                    width: entry.width,
                    height: entry.height,
                    left_pad: entry.left_pad,
                    is_color: entry.is_color,
                })
            }
        },
    );

    let image_placements = terminal.kitty_placements().to_vec();
    let mut image_instances = std::mem::take(&mut state.image_instances);
    fill_image_instances(
        &image_placements,
        cell_w,
        cell_h,
        &mut image_instances,
        |placement| {
            ensure_kitty_image_in_atlas(state, terminal, placement.image_id).map(|entry| {
                AtlasImageRect {
                    x: entry.x,
                    y: entry.y,
                    width: entry.width,
                    height: entry.height,
                }
            })
        },
    );

    state.frame_cells = frame_cells;
    state.text_batches = text_batches;
    state.image_instances = image_instances;

    state.queue.write_buffer(
        &state.bg_instance_buffer,
        0,
        bytemuck::cast_slice(&state.text_batches.bg_instances),
    );
    state.queue.write_buffer(
        &state.fg_instance_buffer,
        0,
        bytemuck::cast_slice(&state.text_batches.fg_instances),
    );
    if !state.text_batches.overlay_instances.is_empty() {
        let offset =
            (state.text_batches.fg_instances.len() * std::mem::size_of::<CellInstance>()) as u64;
        state.queue.write_buffer(
            &state.fg_instance_buffer,
            offset,
            bytemuck::cast_slice(&state.text_batches.overlay_instances),
        );
    }
    if !state.image_instances.is_empty() {
        state.queue.write_buffer(
            &state.image_instance_buffer,
            0,
            bytemuck::cast_slice(&state.image_instances),
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
                        a: base_bg_f[3] as f64,
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
        pass.draw(0..6, 0..state.text_batches.bg_instances.len() as u32);

        if !state.image_instances.is_empty() {
            pass.set_pipeline(&state.image_pipeline);
            pass.set_vertex_buffer(0, state.image_instance_buffer.slice(..));
            pass.draw(0..6, 0..state.image_instances.len() as u32);
        }

        if !state.text_batches.fg_instances.is_empty() {
            pass.set_pipeline(&state.pipeline);
            pass.set_vertex_buffer(0, state.fg_instance_buffer.slice(..));
            pass.draw(0..6, 0..state.text_batches.fg_instances.len() as u32);
        }
        if !state.text_batches.overlay_instances.is_empty() {
            pass.set_pipeline(&state.pipeline);
            pass.set_vertex_buffer(0, state.fg_instance_buffer.slice(..));
            let start = state.text_batches.fg_instances.len() as u32;
            let end =
                (state.text_batches.fg_instances.len() + state.text_batches.overlay_instances.len())
                    as u32;
            pass.draw(0..6, start..end);
        }
    }

    state.queue.submit(std::iter::once(encoder.finish()));
    output.present();
    terminal.grid_mut().clear_dirty();
    state.last_visual_state = Some(current_visual);
    state.last_presented_signature = Some(signature);
}

fn build_gpu_glyph_tile(
    glyph: &crate::font::GlyphData<'_>,
    cells: usize,
    cell_width: usize,
    cell_height: usize,
    baseline: usize,
) -> (Vec<u8>, u32, u32, u32) {
    let nominal_width = (cells.max(1) * cell_width) as i32;
    let left_pad = (-glyph.bearing_x).max(0) as u32;
    let glyph_right = glyph.bearing_x + glyph.width as i32;
    let tile_width = (nominal_width + left_pad as i32)
        .max(glyph_right + left_pad as i32)
        .max(1) as u32;
    let tile_height = cell_height as u32;
    let mut tile = vec![0u8; tile_width as usize * tile_height as usize * 4];
    let origin_y = cell_height as i32 - baseline as i32;
    let glyph_top = origin_y - glyph.bearing_y;
    let glyph_left = glyph.bearing_x + left_pad as i32;

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

    (tile, tile_width, tile_height, left_pad)
}

fn ensure_glyph_in_atlas<'a>(
    state: &'a mut GpuSurfaceState,
    atlas: &mut GlyphAtlas,
    ch: u32,
    cells: usize,
) -> Option<&'a GpuGlyphEntry> {
    if state.glyph_map.contains_key(&ch) {
        return state.glyph_map.get(&ch);
    }

    if !atlas.ensure_glyph(ch) {
        return None;
    }

    let glyph = atlas.get_glyph(ch)?;
    if glyph.width == 0 || glyph.height == 0 {
        state.glyph_map.insert(
            ch,
            GpuGlyphEntry {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
                left_pad: 0,
                is_color: glyph.format == GlyphFormat::Rgba,
            },
        );
        return state.glyph_map.get(&ch);
    }

    let is_color = glyph.format == GlyphFormat::Rgba;
    let (upload, upload_width, upload_height, left_pad) = build_gpu_glyph_tile(
        &glyph,
        cells,
        atlas.cell_width,
        atlas.cell_height,
        atlas.baseline,
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
        left_pad,
        is_color,
    };
    state.atlas_cursor_x += upload_width + 1;
    state.atlas_row_height = state.atlas_row_height.max(upload_height + 1);
    state.glyph_map.insert(ch, entry);
    state.glyph_map.get(&ch)
}

fn ensure_grapheme_in_atlas<'a>(
    state: &'a mut GpuSurfaceState,
    atlas: &mut GlyphAtlas,
    grapheme: &str,
) -> Option<&'a GpuGlyphEntry> {
    if state.grapheme_map.contains_key(grapheme) {
        return state.grapheme_map.get(grapheme);
    }

    if !atlas.ensure_grapheme(grapheme) {
        return None;
    }

    let glyph = atlas.get_grapheme_glyph(grapheme)?;
    if glyph.width == 0 || glyph.height == 0 {
        state.grapheme_map.insert(
            grapheme.into(),
            GpuGlyphEntry {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
                left_pad: 0,
                is_color: glyph.format == GlyphFormat::Rgba,
            },
        );
        return state.grapheme_map.get(grapheme);
    }

    let (upload, upload_width, upload_height, left_pad) = build_gpu_glyph_tile(
        &glyph,
        1,
        atlas.cell_width,
        atlas.cell_height,
        atlas.baseline,
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
        left_pad,
        is_color: glyph.format == GlyphFormat::Rgba,
    };
    state.atlas_cursor_x += upload_width + 1;
    state.atlas_row_height = state.atlas_row_height.max(upload_height + 1);
    state.grapheme_map.insert(grapheme.into(), entry);
    state.grapheme_map.get(grapheme)
}

fn ensure_kitty_image_in_atlas<'a>(
    state: &'a mut GpuSurfaceState,
    terminal: &impl TerminalView,
    image_id: u32,
) -> Option<&'a GpuImageEntry> {
    if state.last_kitty_generation != terminal.kitty_generation() {
        state.image_map.clear();
        state.last_kitty_generation = terminal.kitty_generation();
    }

    if state.image_map.contains_key(&image_id) {
        return state.image_map.get(&image_id);
    }

    let image = terminal.kitty_image(image_id)?;
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

fn select_surface_format(
    capabilities: &wgpu::SurfaceCapabilities,
    fallback: wgpu::TextureFormat,
) -> wgpu::TextureFormat {
    capabilities
        .formats
        .iter()
        .copied()
        .find(|format| !format.is_srgb())
        .unwrap_or(fallback)
}

fn transparency_requested(background_opacity: f64) -> bool {
    background_opacity < 1.0
}

fn clamp_background_alpha(background_opacity: f64) -> f32 {
    background_opacity.clamp(0.0, 1.0) as f32
}

fn select_alpha_mode(
    capabilities: &wgpu::SurfaceCapabilities,
    transparency_requested: bool,
) -> wgpu::CompositeAlphaMode {
    if !transparency_requested {
        return wgpu::CompositeAlphaMode::Opaque;
    }

    for mode in [
        wgpu::CompositeAlphaMode::PreMultiplied,
        wgpu::CompositeAlphaMode::PostMultiplied,
        wgpu::CompositeAlphaMode::Inherit,
        wgpu::CompositeAlphaMode::Auto,
    ] {
        if capabilities.alpha_modes.contains(&mode) {
            return mode;
        }
    }

    wgpu::CompositeAlphaMode::Opaque
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let (tile, width, height, left_pad) = build_gpu_glyph_tile(&glyph, 2, 8, 4, 2);

        assert_eq!(width, 16);
        assert_eq!(height, 4);
        assert_eq!(left_pad, 0);
        assert_eq!(tile.len(), 16 * 4 * 4);
        assert_eq!(&tile[0..8], &[0xff, 0xff, 0xff, 255, 0xff, 0xff, 0xff, 128]);
    }

    #[test]
    fn gpu_glyph_tile_expands_for_right_overhang() {
        let pixels = [255u8, 255, 255, 255];
        let glyph = crate::font::GlyphData {
            pixels: &pixels,
            width: 4,
            height: 1,
            format: GlyphFormat::Alpha,
            bearing_x: 6,
            bearing_y: 1,
        };

        let (_tile, width, height, left_pad) = build_gpu_glyph_tile(&glyph, 1, 8, 4, 1);

        assert_eq!(width, 10);
        assert_eq!(height, 4);
        assert_eq!(left_pad, 0);
    }

    #[test]
    fn gpu_glyph_tile_preserves_left_overhang() {
        let pixels = [255u8, 255, 255, 255];
        let glyph = crate::font::GlyphData {
            pixels: &pixels,
            width: 4,
            height: 1,
            format: GlyphFormat::Alpha,
            bearing_x: -2,
            bearing_y: 1,
        };

        let (_tile, width, height, left_pad) = build_gpu_glyph_tile(&glyph, 1, 8, 4, 1);

        assert_eq!(width, 10);
        assert_eq!(height, 4);
        assert_eq!(left_pad, 2);
    }

    #[test]
    fn prefers_non_srgb_surface_format_when_available() {
        let capabilities = wgpu::SurfaceCapabilities {
            formats: vec![
                wgpu::TextureFormat::Bgra8UnormSrgb,
                wgpu::TextureFormat::Bgra8Unorm,
            ],
            present_modes: vec![wgpu::PresentMode::Fifo],
            alpha_modes: vec![wgpu::CompositeAlphaMode::Opaque],
            usages: wgpu::TextureUsages::RENDER_ATTACHMENT,
        };

        assert_eq!(
            select_surface_format(&capabilities, wgpu::TextureFormat::Bgra8UnormSrgb),
            wgpu::TextureFormat::Bgra8Unorm
        );
    }

    #[test]
    fn requests_transparent_window_only_for_partial_opacity() {
        assert!(transparency_requested(0.9));
        assert!(transparency_requested(0.0));
        assert!(!transparency_requested(1.0));
        assert!(!transparency_requested(1.5));
    }

    #[test]
    fn clamps_background_alpha_into_unit_interval() {
        assert_eq!(clamp_background_alpha(-0.5), 0.0);
        assert_eq!(clamp_background_alpha(0.0), 0.0);
        assert_eq!(clamp_background_alpha(0.25), 0.25);
        assert_eq!(clamp_background_alpha(1.0), 1.0);
        assert_eq!(clamp_background_alpha(1.5), 1.0);
    }

    #[test]
    fn prefers_premultiplied_alpha_when_transparency_is_requested() {
        let capabilities = wgpu::SurfaceCapabilities {
            formats: vec![wgpu::TextureFormat::Bgra8Unorm],
            present_modes: vec![wgpu::PresentMode::Fifo],
            alpha_modes: vec![
                wgpu::CompositeAlphaMode::Opaque,
                wgpu::CompositeAlphaMode::PreMultiplied,
            ],
            usages: wgpu::TextureUsages::RENDER_ATTACHMENT,
        };

        assert_eq!(
            select_alpha_mode(&capabilities, true),
            wgpu::CompositeAlphaMode::PreMultiplied
        );
        assert_eq!(
            select_alpha_mode(&capabilities, false),
            wgpu::CompositeAlphaMode::Opaque
        );
    }

    #[test]
    fn falls_back_to_auto_alpha_when_thats_all_the_compositor_offers() {
        let capabilities = wgpu::SurfaceCapabilities {
            formats: vec![wgpu::TextureFormat::Bgra8Unorm],
            present_modes: vec![wgpu::PresentMode::Fifo],
            alpha_modes: vec![wgpu::CompositeAlphaMode::Auto],
            usages: wgpu::TextureUsages::RENDER_ATTACHMENT,
        };

        assert_eq!(
            select_alpha_mode(&capabilities, true),
            wgpu::CompositeAlphaMode::Auto
        );
    }

    #[test]
    fn falls_back_to_opaque_alpha_when_compositor_has_no_transparent_mode() {
        let capabilities = wgpu::SurfaceCapabilities {
            formats: vec![wgpu::TextureFormat::Bgra8Unorm],
            present_modes: vec![wgpu::PresentMode::Fifo],
            alpha_modes: vec![wgpu::CompositeAlphaMode::Opaque],
            usages: wgpu::TextureUsages::RENDER_ATTACHMENT,
        };

        assert_eq!(
            select_alpha_mode(&capabilities, true),
            wgpu::CompositeAlphaMode::Opaque
        );
    }
}
