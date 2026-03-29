use crate::config::AppConfig;
use crate::font::{GlyphAtlas, GlyphFormat};
use crate::frontend::{ViewportScroll, VisualState, visual_signature};
use crate::gpu_frame::{
    AtlasImageRect, CellInfo, CellInstance, FrameBatchStyle, FrameTextBatches, GlyphAtlasEntry,
    ImageInstance, append_scrollbar_overlay_instances, fill_cell_infos,
    fill_cell_infos_with_scroll, fill_image_instances, fill_image_instances_with_viewport_offset,
    fill_text_batches,
};
use crate::terminal::TerminalView;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};
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

pub(crate) struct GpuGlyphEntry {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    left_pad: u32,
    top_pad: u32,
    is_color: bool,
}

pub(crate) struct GpuImageEntry {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

pub struct SharedAtlasState {
    pub(crate) atlas_texture: wgpu::Texture,
    pub(crate) atlas_view: wgpu::TextureView,
    pub(crate) glyph_map: HashMap<u32, GpuGlyphEntry>,
    pub(crate) grapheme_map: HashMap<Box<str>, GpuGlyphEntry>,
    pub(crate) image_map: HashMap<u32, GpuImageEntry>,
    pub(crate) atlas_cursor_x: u32,
    pub(crate) atlas_cursor_y: u32,
    pub(crate) atlas_row_height: u32,
    pub(crate) last_kitty_generation: u64,
}

pub struct SharedGpuContext {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    bind_group_layout: wgpu::BindGroupLayout,
    shader: wgpu::ShaderModule,
    image_shader: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    atlas_sampler: wgpu::Sampler,
    pipeline_cache: Mutex<HashMap<wgpu::TextureFormat, SharedPipelines>>,
    pub atlas: Mutex<SharedAtlasState>,
}

struct SharedPipelines {
    text: wgpu::RenderPipeline,
    image: wgpu::RenderPipeline,
}

pub struct GpuSurfaceState {
    pub shared: Arc<SharedGpuContext>,
    pub window: Arc<Window>,
    pub surface: wgpu::Surface<'static>,
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
    frame_cells: Vec<CellInfo>,
    text_batches: FrameTextBatches,
    image_instances: Vec<ImageInstance>,
    pub last_visual_state: Option<VisualState>,
    pub last_presented_signature: Option<u64>,
    pub last_viewport_scroll_quantized: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
pub struct GpuSurfaceDefaults {
    pub format: wgpu::TextureFormat,
    pub present_mode: wgpu::PresentMode,
    pub alpha_mode: wgpu::CompositeAlphaMode,
}

fn build_surface_config(
    size: winit::dpi::PhysicalSize<u32>,
    transparency: bool,
    preferred_defaults: Option<GpuSurfaceDefaults>,
    surface_caps: Option<&wgpu::SurfaceCapabilities>,
) -> Result<(wgpu::SurfaceConfiguration, bool)> {
    if let Some(defaults) = preferred_defaults {
        return Ok((
            wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: defaults.format,
                width: size.width.max(1),
                height: size.height.max(1),
                present_mode: defaults.present_mode,
                desired_maximum_frame_latency: 1,
                alpha_mode: defaults.alpha_mode,
                view_formats: vec![],
            },
            true,
        ));
    }

    let surface_caps = surface_caps.context("surface should be compatible")?;
    anyhow::ensure!(
        !surface_caps.formats.is_empty(),
        "surface should be compatible"
    );
    Ok((
        wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: select_surface_format(surface_caps, surface_caps.formats[0]),
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: select_present_mode(surface_caps),
            desired_maximum_frame_latency: 1,
            alpha_mode: select_alpha_mode(surface_caps, transparency),
            view_formats: vec![],
        },
        false,
    ))
}

#[derive(Debug, Clone, Copy, Default)]
pub struct GpuSurfaceCreateProfile {
    pub window_create: Duration,
    pub ime_setup: Duration,
    pub surface_create: Duration,
    pub default_config: Duration,
    pub capabilities: Duration,
    pub configure: Duration,
    pub atlas_texture: Duration,
    pub uniform_buffer: Duration,
    pub instance_buffers: Duration,
    pub bind_group: Duration,
    pub pipeline_lookup: Duration,
    pub total: Duration,
    pub pipeline_cache_hit: bool,
    pub reused_surface_defaults: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct GpuRenderProfile {
    pub acquire_surface: Duration,
    pub build_display_list: Duration,
    pub upload_buffers: Duration,
    pub encode_pass: Duration,
    pub submit: Duration,
    pub present: Duration,
    pub total: Duration,
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

    pub fn preferred_surface_defaults(&self) -> GpuSurfaceDefaults {
        GpuSurfaceDefaults {
            format: self.surface_config.format,
            present_mode: self.surface_config.present_mode,
            alpha_mode: self.surface_config.alpha_mode,
        }
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
            color = glyph;
        } else {
            color = vec4<f32>(in.fg.rgb, glyph.a * in.fg.a);
        }
    }

    let y = in.local_pos.y;
    let x = in.local_pos.x;
    let h = in.cell_size.y;
    let w = in.cell_size.x;

    if (in.flags & 2u) != 0u {
        let ul_y = h - 2.0;
        if y >= ul_y && y < ul_y + 1.0 {
            color = vec4<f32>(in.deco.rgb, 1.0);
        }
    }
    if (in.flags & 8u) != 0u {
        let ul_y = h - 2.0;
        let phase = x / w * 6.28318530718;
        let wave = sin(phase) * 2.0;
        if abs(y - (ul_y + wave)) < 1.5 {
            color = vec4<f32>(in.deco.rgb, 1.0);
        }
    }
    if (in.flags & 16u) != 0u {
        let ul_y1 = h - 2.0;
        let ul_y2 = h - 4.0;
        if (y >= ul_y1 && y < ul_y1 + 1.0) || (y >= ul_y2 && y < ul_y2 + 1.0) {
            color = vec4<f32>(in.deco.rgb, 1.0);
        }
    }
    if (in.flags & 32u) != 0u {
        let ul_y = h - 2.0;
        if y >= ul_y && y < ul_y + 1.0 && u32(x) % 3u == 0u {
            color = vec4<f32>(in.deco.rgb, 1.0);
        }
    }
    if (in.flags & 64u) != 0u {
        let ul_y = h - 2.0;
        let dash = u32(w) / 3u;
        let offset = u32(x);
        if y >= ul_y && y < ul_y + 1.0 && (offset < dash || (offset >= dash * 2u && offset < dash * 3u)) {
            color = vec4<f32>(in.deco.rgb, 1.0);
        }
    }

    if (in.flags & 4u) != 0u {
        let mid_y = h / 2.0;
        if y >= mid_y && y < mid_y + 1.0 {
            color = vec4<f32>(in.fg.rgb, 1.0);
        }
    }

    if (in.flags & 256u) != 0u && x < min(2.0, w) {
        color = vec4<f32>(in.fg.rgb, 1.0);
    }
    if (in.flags & 512u) != 0u {
        let cursor_y = h - min(2.0, h);
        if y >= cursor_y {
            color = vec4<f32>(in.fg.rgb, 1.0);
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

fn cell_instance_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
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
    }
}

fn image_instance_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
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
    }
}

impl SharedGpuContext {
    fn pipelines_for_format_profiled(
        &self,
        format: wgpu::TextureFormat,
    ) -> (wgpu::RenderPipeline, wgpu::RenderPipeline, bool, Duration) {
        let start = Instant::now();
        let mut cache = self
            .pipeline_cache
            .lock()
            .expect("gpu pipeline cache poisoned");
        let cache_hit = cache.contains_key(&format);
        let entry = cache.entry(format).or_insert_with(|| {
            let text = self
                .device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("render_pipeline"),
                    layout: Some(&self.pipeline_layout),
                    vertex: wgpu::VertexState {
                        module: &self.shader,
                        entry_point: Some("vs_main"),
                        buffers: &[cell_instance_layout()],
                        compilation_options: Default::default(),
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &self.shader,
                        entry_point: Some("fs_main"),
                        targets: &[Some(wgpu::ColorTargetState {
                            format,
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

            let image = self
                .device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("image_pipeline"),
                    layout: Some(&self.pipeline_layout),
                    vertex: wgpu::VertexState {
                        module: &self.image_shader,
                        entry_point: Some("vs_main"),
                        buffers: &[image_instance_layout()],
                        compilation_options: Default::default(),
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &self.image_shader,
                        entry_point: Some("fs_main"),
                        targets: &[Some(wgpu::ColorTargetState {
                            format,
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

            SharedPipelines { text, image }
        });

        (
            entry.text.clone(),
            entry.image.clone(),
            cache_hit,
            start.elapsed(),
        )
    }

    #[allow(dead_code)]
    fn pipelines_for_format(
        &self,
        format: wgpu::TextureFormat,
    ) -> (wgpu::RenderPipeline, wgpu::RenderPipeline) {
        let (text, image, _, _) = self.pipelines_for_format_profiled(format);
        (text, image)
    }
}

pub fn create_window_attributes(
    config: &AppConfig,
    atlas: &GlyphAtlas,
    title: &str,
) -> WindowAttributes {
    create_window_attributes_for_metrics(config, atlas.cell_width, atlas.cell_height, title)
}

pub fn create_window_attributes_for_metrics(
    config: &AppConfig,
    cell_width: usize,
    cell_height: usize,
    title: &str,
) -> WindowAttributes {
    let width = config.window.columns as f64 * cell_width as f64;
    let height = config.window.rows as f64 * cell_height as f64;

    Window::default_attributes()
        .with_title(title)
        .with_name("handterm", "handterm")
        .with_transparent(transparency_requested(config.style.background_opacity))
        .with_inner_size(Size::Logical(LogicalSize::new(width, height)))
}

pub fn create_shared_gpu_context() -> Result<Arc<SharedGpuContext>> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        ..Default::default()
    });

    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
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
    let atlas_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });

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

    Ok(Arc::new(SharedGpuContext {
        instance,
        adapter,
        device,
        queue,
        bind_group_layout,
        shader,
        image_shader,
        pipeline_layout,
        atlas_sampler,
        pipeline_cache: Mutex::new(HashMap::new()),
        atlas: Mutex::new(SharedAtlasState {
            atlas_texture,
            atlas_view,
            glyph_map: HashMap::with_capacity(256),
            grapheme_map: HashMap::with_capacity(32),
            image_map: HashMap::with_capacity(32),
            atlas_cursor_x: 0,
            atlas_cursor_y: 0,
            atlas_row_height: 0,
            last_kitty_generation: 0,
        }),
    }))
}

pub fn create_surface_state(
    event_loop: &ActiveEventLoop,
    config: &AppConfig,
    title: &str,
    atlas: &GlyphAtlas,
) -> Result<GpuSurfaceState> {
    let shared = create_shared_gpu_context()?;
    let (state, _) =
        create_surface_state_with_shared_profiled(shared, event_loop, config, title, atlas)?;
    Ok(state)
}

pub fn create_surface_state_with_shared(
    shared: Arc<SharedGpuContext>,
    event_loop: &ActiveEventLoop,
    config: &AppConfig,
    title: &str,
    atlas: &GlyphAtlas,
) -> Result<GpuSurfaceState> {
    let (state, _) = create_surface_state_with_shared_profiled_with_defaults(
        shared, event_loop, config, title, atlas, None,
    )?;
    Ok(state)
}

pub fn create_surface_state_with_shared_profiled(
    shared: Arc<SharedGpuContext>,
    event_loop: &ActiveEventLoop,
    config: &AppConfig,
    title: &str,
    atlas: &GlyphAtlas,
) -> Result<(GpuSurfaceState, GpuSurfaceCreateProfile)> {
    create_surface_state_with_shared_profiled_with_defaults(
        shared, event_loop, config, title, atlas, None,
    )
}

pub fn create_surface_state_with_shared_profiled_with_defaults(
    shared: Arc<SharedGpuContext>,
    event_loop: &ActiveEventLoop,
    config: &AppConfig,
    title: &str,
    atlas: &GlyphAtlas,
    preferred_defaults: Option<GpuSurfaceDefaults>,
) -> Result<(GpuSurfaceState, GpuSurfaceCreateProfile)> {
    let step_start = Instant::now();
    let window = Arc::new(
        event_loop
            .create_window(create_window_attributes(config, atlas, title))
            .context("window creation should succeed")?,
    );
    let window_create = step_start.elapsed();

    create_surface_state_for_window_with_shared_profiled_with_defaults(
        shared,
        window,
        config,
        atlas,
        preferred_defaults,
        Some(window_create),
    )
}

pub fn create_surface_state_for_window_with_shared_profiled_with_defaults(
    shared: Arc<SharedGpuContext>,
    window: Arc<Window>,
    config: &AppConfig,
    atlas: &GlyphAtlas,
    preferred_defaults: Option<GpuSurfaceDefaults>,
    precomputed_window_create: Option<Duration>,
) -> Result<(GpuSurfaceState, GpuSurfaceCreateProfile)> {
    let total_start = Instant::now();
    let window_create = precomputed_window_create.unwrap_or(Duration::ZERO);

    let step_start = Instant::now();
    window.set_ime_allowed(true);
    window.set_ime_purpose(ImePurpose::Terminal);
    let ime_setup = step_start.elapsed();

    let step_start = Instant::now();
    let surface = shared
        .instance
        .create_surface(window.clone())
        .context("surface creation should succeed")?;
    let surface_create = step_start.elapsed();

    let size = window.inner_size();

    let (surface_config, capabilities, default_config, reused_surface_defaults) =
        if preferred_defaults.is_some() {
            let (surface_config, reused_surface_defaults) = build_surface_config(
                size,
                transparency_requested(config.style.background_opacity),
                preferred_defaults,
                None,
            )?;
            (
                surface_config,
                Duration::ZERO,
                Duration::ZERO,
                reused_surface_defaults,
            )
        } else {
            let step_start = Instant::now();
            let surface_caps = surface.get_capabilities(&shared.adapter);
            let capabilities = step_start.elapsed();

            let step_start = Instant::now();
            let (surface_config, reused_surface_defaults) = build_surface_config(
                size,
                transparency_requested(config.style.background_opacity),
                None,
                Some(&surface_caps),
            )?;
            let default_config = step_start.elapsed();
            (
                surface_config,
                capabilities,
                default_config,
                reused_surface_defaults,
            )
        };

    let step_start = Instant::now();
    surface.configure(&shared.device, &surface_config);
    let configure = step_start.elapsed();

    let step_start = Instant::now();
    let atlas_state = shared.atlas.lock().expect("shared atlas lock poisoned");
    let atlas_texture_create = step_start.elapsed();

    let uniforms = Uniforms {
        screen_size: [size.width as f32, size.height as f32],
        cell_size: [atlas.cell_width as f32, atlas.cell_height as f32],
        atlas_size: [ATLAS_WIDTH as f32, ATLAS_HEIGHT as f32],
        _pad: [0.0; 2],
    };

    let step_start = Instant::now();
    let uniform_buffer = shared
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("uniforms"),
            contents: bytemuck::bytes_of(&uniforms),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
    let uniform_buffer_create = step_start.elapsed();

    let max_instances = (config.window.columns as usize) * (config.window.rows as usize);
    let max_fg_instances = max_instances + 4;
    let step_start = Instant::now();
    let bg_instance_buffer = shared.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("bg_instances"),
        size: (max_instances * std::mem::size_of::<CellInstance>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let fg_instance_buffer = shared.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("fg_instances"),
        size: (max_fg_instances * std::mem::size_of::<CellInstance>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let max_image_instances = (config.window.columns as usize) * (config.window.rows as usize);
    let image_instance_buffer = shared.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("image_instances"),
        size: (max_image_instances * std::mem::size_of::<ImageInstance>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let instance_buffers = step_start.elapsed();

    let step_start = Instant::now();
    let bind_group = shared.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("bind_group"),
        layout: &shared.bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&atlas_state.atlas_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&shared.atlas_sampler),
            },
        ],
    });
    let bind_group_create = step_start.elapsed();
    drop(atlas_state);

    let (pipeline, image_pipeline, pipeline_cache_hit, pipeline_lookup) =
        shared.pipelines_for_format_profiled(surface_config.format);

    Ok((
        GpuSurfaceState {
            shared,
            window,
            surface,
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
            frame_cells: Vec::with_capacity(max_instances),
            text_batches: FrameTextBatches {
                bg_instances: Vec::with_capacity(max_instances),
                fg_instances: Vec::with_capacity(max_instances),
                overlay_instances: Vec::with_capacity(4),
            },
            image_instances: Vec::with_capacity(max_image_instances),
            last_visual_state: None,
            last_presented_signature: None,
            last_viewport_scroll_quantized: None,
        },
        GpuSurfaceCreateProfile {
            window_create,
            ime_setup,
            surface_create,
            default_config,
            capabilities,
            configure,
            atlas_texture: atlas_texture_create,
            uniform_buffer: uniform_buffer_create,
            instance_buffers,
            bind_group: bind_group_create,
            pipeline_lookup,
            total: total_start.elapsed(),
            pipeline_cache_hit,
            reused_surface_defaults,
        },
    ))
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

    if state.surface_config.width == width && state.surface_config.height == height {
        return;
    }

    state.surface_config.width = width;
    state.surface_config.height = height;
    state
        .surface
        .configure(&state.shared.device, &state.surface_config);
    state.last_presented_signature = None;

    let needed = (cols as usize) * (rows as usize) * 2;
    if needed > state.max_instances {
        state.max_instances = needed;
        let needed_fg = needed + 4;
        state.bg_instance_buffer = state.shared.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bg_instances"),
            size: (needed * std::mem::size_of::<CellInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        state.fg_instance_buffer = state.shared.device.create_buffer(&wgpu::BufferDescriptor {
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
        state.image_instance_buffer = state.shared.device.create_buffer(&wgpu::BufferDescriptor {
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
        .shared
        .queue
        .write_buffer(&state.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
}

pub fn render_surface_state(
    state: &mut GpuSurfaceState,
    terminal: &mut impl TerminalView,
    atlas: &mut GlyphAtlas,
    config: &AppConfig,
) {
    render_surface_state_with_scroll(state, terminal, atlas, config, 0.0);
}

pub fn render_surface_state_with_scroll(
    state: &mut GpuSurfaceState,
    terminal: &mut impl TerminalView,
    atlas: &mut GlyphAtlas,
    config: &AppConfig,
    scroll_rows: f32,
) {
    let _ = render_surface_state_profiled_with_scroll(state, terminal, atlas, config, scroll_rows);
}

pub fn render_surface_state_profiled(
    state: &mut GpuSurfaceState,
    terminal: &mut impl TerminalView,
    atlas: &mut GlyphAtlas,
    config: &AppConfig,
) -> Option<GpuRenderProfile> {
    render_surface_state_profiled_with_scroll(state, terminal, atlas, config, 0.0)
}

pub fn render_surface_state_profiled_with_scroll(
    state: &mut GpuSurfaceState,
    terminal: &mut impl TerminalView,
    atlas: &mut GlyphAtlas,
    config: &AppConfig,
    scroll_rows: f32,
) -> Option<GpuRenderProfile> {
    let total_start = Instant::now();
    let current_visual = VisualState::capture(terminal);
    let signature = visual_signature(terminal);
    let viewport_scroll = ViewportScroll::from_scroll_rows(scroll_rows);
    let viewport_quantized = (scroll_rows.max(0.0) * 1024.0).round() as u32;
    if state.last_presented_signature == Some(signature)
        && state.last_viewport_scroll_quantized == Some(viewport_quantized)
    {
        terminal.grid_mut().clear_dirty();
        state.last_visual_state = Some(current_visual);
        return None;
    }

    let step_start = Instant::now();
    let output = match state.surface.get_current_texture() {
        Ok(texture) => texture,
        Err(wgpu::SurfaceError::Lost) => {
            state
                .surface
                .configure(&state.shared.device, &state.surface_config);
            state.last_presented_signature = None;
            state.last_viewport_scroll_quantized = None;
            return None;
        }
        Err(_) => return None,
    };
    let acquire_surface = step_start.elapsed();

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
    let step_start = Instant::now();
    let mut frame_cells = std::mem::take(&mut state.frame_cells);
    if viewport_scroll == ViewportScroll::ZERO {
        fill_cell_infos(terminal, &mut frame_cells);
    } else {
        fill_cell_infos_with_scroll(terminal, &mut frame_cells, viewport_scroll);
    }
    let mut text_batches = std::mem::take(&mut state.text_batches);
    let mut atlas_state = state
        .shared
        .atlas
        .lock()
        .expect("shared atlas lock poisoned");
    fill_text_batches(
        &frame_cells,
        FrameBatchStyle {
            base_fg,
            base_bg,
            base_fg_f,
            background_alpha,
            cell_w,
            cell_h,
            viewport_offset_y: viewport_scroll.viewport_offset_y(cell_h),
        },
        &mut text_batches,
        |ci| {
            if let Some(ref grapheme) = ci.grapheme {
                ensure_grapheme_in_atlas(
                    &mut atlas_state,
                    &state.shared.queue,
                    atlas,
                    grapheme,
                    ci.cells,
                )
                .map(|entry| GlyphAtlasEntry {
                    x: entry.x,
                    y: entry.y,
                    width: entry.width,
                    height: entry.height,
                    left_pad: entry.left_pad,
                    top_pad: entry.top_pad,
                    is_color: entry.is_color,
                })
            } else {
                ensure_glyph_in_atlas(
                    &mut atlas_state,
                    &state.shared.queue,
                    atlas,
                    ci.ch,
                    ci.cells,
                )
                .map(|entry| GlyphAtlasEntry {
                    x: entry.x,
                    y: entry.y,
                    width: entry.width,
                    height: entry.height,
                    left_pad: entry.left_pad,
                    top_pad: entry.top_pad,
                    is_color: entry.is_color,
                })
            }
        },
    );
    if config.scrollback.scrollbar {
        append_scrollbar_overlay_instances(
            &mut text_batches.overlay_instances,
            base_fg,
            state.surface_config.width as f32,
            state.surface_config.height as f32,
            terminal.grid().scrollback_len(),
            terminal.grid().rows,
            scroll_rows,
        );
    }

    let image_placements = terminal.kitty_placements().to_vec();
    let mut image_instances = std::mem::take(&mut state.image_instances);
    if viewport_scroll == ViewportScroll::ZERO {
        fill_image_instances(
            &image_placements,
            cell_w,
            cell_h,
            &mut image_instances,
            |placement| {
                ensure_kitty_image_in_atlas(
                    &mut atlas_state,
                    &state.shared.queue,
                    terminal,
                    placement.image_id,
                )
                .map(|entry| AtlasImageRect {
                    x: entry.x,
                    y: entry.y,
                    width: entry.width,
                    height: entry.height,
                })
            },
        );
    } else {
        fill_image_instances_with_viewport_offset(
            &image_placements,
            cell_w,
            cell_h,
            viewport_scroll.viewport_offset_y(cell_h),
            &mut image_instances,
            |placement| {
                ensure_kitty_image_in_atlas(
                    &mut atlas_state,
                    &state.shared.queue,
                    terminal,
                    placement.image_id,
                )
                .map(|entry| AtlasImageRect {
                    x: entry.x,
                    y: entry.y,
                    width: entry.width,
                    height: entry.height,
                })
            },
        );
    }
    drop(atlas_state);

    state.frame_cells = frame_cells;
    state.text_batches = text_batches;
    state.image_instances = image_instances;
    let build_display_list = step_start.elapsed();

    let step_start = Instant::now();
    state.shared.queue.write_buffer(
        &state.bg_instance_buffer,
        0,
        bytemuck::cast_slice(&state.text_batches.bg_instances),
    );
    state.shared.queue.write_buffer(
        &state.fg_instance_buffer,
        0,
        bytemuck::cast_slice(&state.text_batches.fg_instances),
    );
    if !state.text_batches.overlay_instances.is_empty() {
        let offset =
            (state.text_batches.fg_instances.len() * std::mem::size_of::<CellInstance>()) as u64;
        state.shared.queue.write_buffer(
            &state.fg_instance_buffer,
            offset,
            bytemuck::cast_slice(&state.text_batches.overlay_instances),
        );
    }
    if !state.image_instances.is_empty() {
        state.shared.queue.write_buffer(
            &state.image_instance_buffer,
            0,
            bytemuck::cast_slice(&state.image_instances),
        );
    }
    let upload_buffers = step_start.elapsed();

    let step_start = Instant::now();
    let mut encoder = state
        .shared
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
            let end = (state.text_batches.fg_instances.len()
                + state.text_batches.overlay_instances.len()) as u32;
            pass.draw(0..6, start..end);
        }
    }
    let encode_pass = step_start.elapsed();

    let step_start = Instant::now();
    state.shared.queue.submit(std::iter::once(encoder.finish()));
    let submit = step_start.elapsed();

    let step_start = Instant::now();
    output.present();
    let present = step_start.elapsed();
    terminal.grid_mut().clear_dirty();
    state.last_visual_state = Some(current_visual);
    state.last_presented_signature = Some(signature);
    state.last_viewport_scroll_quantized = Some(viewport_quantized);

    Some(GpuRenderProfile {
        acquire_surface,
        build_display_list,
        upload_buffers,
        encode_pass,
        submit,
        present,
        total: total_start.elapsed(),
    })
}

fn build_gpu_glyph_tile(
    glyph: &crate::font::GlyphData<'_>,
    cells: usize,
    cell_width: usize,
    cell_height: usize,
    baseline: usize,
) -> (Vec<u8>, u32, u32, u32, u32) {
    let nominal_width = (cells.max(1) * cell_width) as i32;
    let left_pad = (-glyph.bearing_x).max(0) as u32;
    let glyph_right = glyph.bearing_x + glyph.width as i32;
    let tile_width = (nominal_width + left_pad as i32)
        .max(glyph_right + left_pad as i32)
        .max(1) as u32;
    let tile_height = {
        let origin_y = cell_height as i32 - baseline as i32;
        let glyph_top = origin_y - glyph.bearing_y;
        let top_pad = (-glyph_top).max(0) as u32;
        let glyph_bottom = glyph_top + glyph.height as i32;
        (cell_height as i32 + top_pad as i32)
            .max(glyph_bottom + top_pad as i32)
            .max(1) as u32
    };
    let mut tile = vec![0u8; tile_width as usize * tile_height as usize * 4];
    let origin_y = cell_height as i32 - baseline as i32;
    let glyph_top = origin_y - glyph.bearing_y;
    let top_pad = (-glyph_top).max(0) as u32;
    let glyph_top = glyph_top + top_pad as i32;
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

    (tile, tile_width, tile_height, left_pad, top_pad)
}

fn ensure_glyph_in_atlas<'a>(
    atlas_state: &'a mut SharedAtlasState,
    queue: &wgpu::Queue,
    atlas: &mut GlyphAtlas,
    ch: u32,
    cells: usize,
) -> Option<&'a GpuGlyphEntry> {
    if atlas_state.glyph_map.contains_key(&ch) {
        return atlas_state.glyph_map.get(&ch);
    }

    if !atlas.ensure_glyph(ch) {
        return None;
    }

    let glyph = atlas.get_glyph(ch)?;
    if glyph.width == 0 || glyph.height == 0 {
        atlas_state.glyph_map.insert(
            ch,
            GpuGlyphEntry {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
                left_pad: 0,
                top_pad: 0,
                is_color: glyph.format == GlyphFormat::Rgba,
            },
        );
        return atlas_state.glyph_map.get(&ch);
    }

    let is_color = glyph.format == GlyphFormat::Rgba;
    let (upload, upload_width, upload_height, left_pad, top_pad) = build_gpu_glyph_tile(
        &glyph,
        cells,
        atlas.cell_width,
        atlas.cell_height,
        atlas.baseline,
    );

    if atlas_state.atlas_cursor_x + upload_width > ATLAS_WIDTH {
        atlas_state.atlas_cursor_x = 0;
        atlas_state.atlas_cursor_y += atlas_state.atlas_row_height;
        atlas_state.atlas_row_height = 0;
    }

    if atlas_state.atlas_cursor_y + upload_height > ATLAS_HEIGHT {
        return None;
    }

    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &atlas_state.atlas_texture,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: atlas_state.atlas_cursor_x,
                y: atlas_state.atlas_cursor_y,
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
        x: atlas_state.atlas_cursor_x,
        y: atlas_state.atlas_cursor_y,
        width: upload_width,
        height: upload_height,
        left_pad,
        top_pad,
        is_color,
    };
    atlas_state.atlas_cursor_x += upload_width + 1;
    atlas_state.atlas_row_height = atlas_state.atlas_row_height.max(upload_height + 1);
    atlas_state.glyph_map.insert(ch, entry);
    atlas_state.glyph_map.get(&ch)
}

fn ensure_grapheme_in_atlas<'a>(
    atlas_state: &'a mut SharedAtlasState,
    queue: &wgpu::Queue,
    atlas: &mut GlyphAtlas,
    grapheme: &str,
    cells: usize,
) -> Option<&'a GpuGlyphEntry> {
    if atlas_state.grapheme_map.contains_key(grapheme) {
        return atlas_state.grapheme_map.get(grapheme);
    }

    if !atlas.ensure_grapheme(grapheme) {
        return None;
    }

    let glyph = atlas.get_grapheme_glyph(grapheme)?;
    if glyph.width == 0 || glyph.height == 0 {
        atlas_state.grapheme_map.insert(
            grapheme.into(),
            GpuGlyphEntry {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
                left_pad: 0,
                top_pad: 0,
                is_color: glyph.format == GlyphFormat::Rgba,
            },
        );
        return atlas_state.grapheme_map.get(grapheme);
    }

    let (upload, upload_width, upload_height, left_pad, top_pad) = build_gpu_glyph_tile(
        &glyph,
        cells,
        atlas.cell_width,
        atlas.cell_height,
        atlas.baseline,
    );

    if atlas_state.atlas_cursor_x + upload_width > ATLAS_WIDTH {
        atlas_state.atlas_cursor_x = 0;
        atlas_state.atlas_cursor_y += atlas_state.atlas_row_height;
        atlas_state.atlas_row_height = 0;
    }
    if atlas_state.atlas_cursor_y + upload_height > ATLAS_HEIGHT {
        return None;
    }

    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &atlas_state.atlas_texture,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: atlas_state.atlas_cursor_x,
                y: atlas_state.atlas_cursor_y,
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
        x: atlas_state.atlas_cursor_x,
        y: atlas_state.atlas_cursor_y,
        width: upload_width,
        height: upload_height,
        left_pad,
        top_pad,
        is_color: glyph.format == GlyphFormat::Rgba,
    };
    atlas_state.atlas_cursor_x += upload_width + 1;
    atlas_state.atlas_row_height = atlas_state.atlas_row_height.max(upload_height + 1);
    atlas_state.grapheme_map.insert(grapheme.into(), entry);
    atlas_state.grapheme_map.get(grapheme)
}

fn ensure_kitty_image_in_atlas<'a>(
    atlas_state: &'a mut SharedAtlasState,
    queue: &wgpu::Queue,
    terminal: &impl TerminalView,
    image_id: u32,
) -> Option<&'a GpuImageEntry> {
    if atlas_state.last_kitty_generation != terminal.kitty_generation() {
        atlas_state.image_map.clear();
        atlas_state.last_kitty_generation = terminal.kitty_generation();
    }

    if atlas_state.image_map.contains_key(&image_id) {
        return atlas_state.image_map.get(&image_id);
    }

    let image = terminal.kitty_image(image_id)?;
    if image.width == 0 || image.height == 0 {
        return None;
    }
    if image.data.len() != (image.width as usize) * (image.height as usize) * 4 {
        return None;
    }

    if atlas_state.atlas_cursor_x + image.width > ATLAS_WIDTH {
        atlas_state.atlas_cursor_x = 0;
        atlas_state.atlas_cursor_y += atlas_state.atlas_row_height;
        atlas_state.atlas_row_height = 0;
    }
    if atlas_state.atlas_cursor_y + image.height > ATLAS_HEIGHT {
        return None;
    }

    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &atlas_state.atlas_texture,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: atlas_state.atlas_cursor_x,
                y: atlas_state.atlas_cursor_y,
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
        x: atlas_state.atlas_cursor_x,
        y: atlas_state.atlas_cursor_y,
        width: image.width,
        height: image.height,
    };
    atlas_state.atlas_cursor_x += image.width + 1;
    atlas_state.atlas_row_height = atlas_state.atlas_row_height.max(image.height + 1);
    atlas_state.image_map.insert(image_id, entry);
    atlas_state.image_map.get(&image_id)
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

fn select_present_mode(capabilities: &wgpu::SurfaceCapabilities) -> wgpu::PresentMode {
    for mode in [
        wgpu::PresentMode::Mailbox,
        wgpu::PresentMode::Fifo,
        wgpu::PresentMode::AutoVsync,
    ] {
        if capabilities.present_modes.contains(&mode) {
            return mode;
        }
    }

    capabilities
        .present_modes
        .first()
        .copied()
        .unwrap_or(wgpu::PresentMode::Fifo)
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
    use crate::color::blend_rgba_over_rgb;
    use crate::config::AppConfig;
    use crate::font::GlyphAtlas;
    use crate::gpu_frame::{
        FLAG_COLOR_GLYPH, FLAG_CURLY_UL, FLAG_CURSOR_BAR, FLAG_CURSOR_UNDERLINE, FLAG_DASHED_UL,
        FLAG_DOTTED_UL, FLAG_DOUBLE_UL, FLAG_HAS_GLYPH, FLAG_STRIKETHROUGH, FLAG_UNDERLINE,
    };
    use crate::render::OffscreenRenderer;
    use crate::terminal::Terminal;
    use crate::workloads::{
        EMOJI_AND_SHADE_TRANSCRIPT, STARSHIP_PROMPT_TRANSCRIPT, TUI_HELP_WITH_IMAGE_TRANSCRIPT,
    };

    #[derive(Clone)]
    struct TestAtlasTexture {
        pixels: Vec<u8>,
        width: u32,
        height: u32,
    }

    fn rgba_bytes(color: [f32; 4]) -> [u8; 4] {
        [
            (color[0].clamp(0.0, 1.0) * 255.0).round() as u8,
            (color[1].clamp(0.0, 1.0) * 255.0).round() as u8,
            (color[2].clamp(0.0, 1.0) * 255.0).round() as u8,
            (color[3].clamp(0.0, 1.0) * 255.0).round() as u8,
        ]
    }

    fn sample_texture(
        texture: &TestAtlasTexture,
        dx: usize,
        dy: usize,
        draw_w: usize,
        draw_h: usize,
    ) -> [f32; 4] {
        let src_x = ((((dx as f32) + 0.5) * texture.width as f32 / draw_w.max(1) as f32) - 0.5)
            .round() as isize;
        let src_y = ((((dy as f32) + 0.5) * texture.height as f32 / draw_h.max(1) as f32) - 0.5)
            .round() as isize;
        let src_x = src_x.clamp(0, texture.width.saturating_sub(1) as isize) as usize;
        let src_y = src_y.clamp(0, texture.height.saturating_sub(1) as isize) as usize;
        let offset = (src_y * texture.width as usize + src_x) * 4;
        [
            texture.pixels[offset] as f32 / 255.0,
            texture.pixels[offset + 1] as f32 / 255.0,
            texture.pixels[offset + 2] as f32 / 255.0,
            texture.pixels[offset + 3] as f32 / 255.0,
        ]
    }

    fn shader_color_for_cell_instance(
        instance: &CellInstance,
        texture: Option<&TestAtlasTexture>,
        dx: usize,
        dy: usize,
        draw_w: usize,
        draw_h: usize,
    ) -> [f32; 4] {
        let mut color = instance.bg;

        if instance.flags & FLAG_HAS_GLYPH != 0
            && let Some(texture) = texture
        {
            let glyph = sample_texture(texture, dx, dy, draw_w, draw_h);
            if instance.flags & FLAG_COLOR_GLYPH != 0 {
                color = glyph;
            } else {
                color = [
                    instance.fg[0],
                    instance.fg[1],
                    instance.fg[2],
                    glyph[3] * instance.fg[3],
                ];
            }
        }

        let x = dx as f32 + 0.5;
        let y = dy as f32 + 0.5;
        let h = instance.size[1].max(1.0);
        let w = instance.size[0].max(1.0);

        if instance.flags & FLAG_UNDERLINE != 0 {
            let ul_y = h - 2.0;
            if y >= ul_y && y < ul_y + 1.0 {
                color = [instance.deco[0], instance.deco[1], instance.deco[2], 1.0];
            }
        }
        if instance.flags & FLAG_CURLY_UL != 0 {
            let ul_y = h - 2.0;
            let phase = x / w * std::f32::consts::TAU;
            let wave = phase.sin() * 2.0;
            if (y - (ul_y + wave)).abs() < 1.5 {
                color = [instance.deco[0], instance.deco[1], instance.deco[2], 1.0];
            }
        }
        if instance.flags & FLAG_DOUBLE_UL != 0 {
            let ul_y1 = h - 2.0;
            let ul_y2 = h - 4.0;
            if (y >= ul_y1 && y < ul_y1 + 1.0) || (y >= ul_y2 && y < ul_y2 + 1.0) {
                color = [instance.deco[0], instance.deco[1], instance.deco[2], 1.0];
            }
        }
        if instance.flags & FLAG_DOTTED_UL != 0 {
            let ul_y = h - 2.0;
            if y >= ul_y && y < ul_y + 1.0 && dx.is_multiple_of(3) {
                color = [instance.deco[0], instance.deco[1], instance.deco[2], 1.0];
            }
        }
        if instance.flags & FLAG_DASHED_UL != 0 {
            let ul_y = h - 2.0;
            let dash = (w as u32 / 3).max(1);
            let offset = dx as u32;
            if y >= ul_y
                && y < ul_y + 1.0
                && (offset < dash || (offset >= dash * 2 && offset < dash * 3))
            {
                color = [instance.deco[0], instance.deco[1], instance.deco[2], 1.0];
            }
        }
        if instance.flags & FLAG_STRIKETHROUGH != 0 {
            let mid_y = h / 2.0;
            if y >= mid_y && y < mid_y + 1.0 {
                color = [instance.fg[0], instance.fg[1], instance.fg[2], 1.0];
            }
        }
        if instance.flags & FLAG_CURSOR_BAR != 0 && x < 2.0_f32.min(w) {
            color = [instance.fg[0], instance.fg[1], instance.fg[2], 1.0];
        }
        if instance.flags & FLAG_CURSOR_UNDERLINE != 0 {
            let cursor_y = h - h.min(2.0);
            if y >= cursor_y {
                color = [instance.fg[0], instance.fg[1], instance.fg[2], 1.0];
            }
        }

        color
    }

    fn draw_cell_instances(
        buffer: &mut [u32],
        buf_w: usize,
        buf_h: usize,
        instances: &[CellInstance],
        textures: &std::collections::HashMap<(u32, u32, u32, u32), TestAtlasTexture>,
    ) {
        for instance in instances {
            let raw_px_x = instance.pos[0].floor() as isize;
            let raw_px_y = instance.pos[1].floor() as isize;
            let px_x = raw_px_x.max(0) as usize;
            let px_y = raw_px_y.max(0) as usize;
            let draw_w = instance.size[0].ceil().max(0.0) as usize;
            let draw_h = instance.size[1].ceil().max(0.0) as usize;
            let raw_x_end = raw_px_x + draw_w as isize;
            let raw_y_end = raw_px_y + draw_h as isize;
            let x_end = raw_x_end.clamp(0, buf_w as isize) as usize;
            let y_end = raw_y_end.clamp(0, buf_h as isize) as usize;
            let texture = textures.get(&(
                instance.uv_offset[0] as u32,
                instance.uv_offset[1] as u32,
                instance.uv_size[0] as u32,
                instance.uv_size[1] as u32,
            ));

            for y in px_y..y_end {
                for x in px_x..x_end {
                    let dx = (x as isize - raw_px_x) as usize;
                    let dy = (y as isize - raw_px_y) as usize;
                    let rgba = rgba_bytes(shader_color_for_cell_instance(
                        instance, texture, dx, dy, draw_w, draw_h,
                    ));
                    blend_rgba_over_rgb(&mut buffer[y * buf_w + x], &rgba);
                }
            }
        }
    }

    fn draw_image_instances(
        buffer: &mut [u32],
        buf_w: usize,
        buf_h: usize,
        instances: &[ImageInstance],
        textures: &std::collections::HashMap<(u32, u32, u32, u32), TestAtlasTexture>,
    ) {
        for instance in instances {
            let raw_px_x = instance.pos[0].floor() as isize;
            let raw_px_y = instance.pos[1].floor() as isize;
            let px_x = raw_px_x.max(0) as usize;
            let px_y = raw_px_y.max(0) as usize;
            let draw_w = instance.size[0].ceil().max(0.0) as usize;
            let draw_h = instance.size[1].ceil().max(0.0) as usize;
            let raw_x_end = raw_px_x + draw_w as isize;
            let raw_y_end = raw_px_y + draw_h as isize;
            let x_end = raw_x_end.clamp(0, buf_w as isize) as usize;
            let y_end = raw_y_end.clamp(0, buf_h as isize) as usize;
            let Some(texture) = textures.get(&(
                instance.uv_offset[0] as u32,
                instance.uv_offset[1] as u32,
                instance.uv_size[0] as u32,
                instance.uv_size[1] as u32,
            )) else {
                continue;
            };

            for y in px_y..y_end {
                for x in px_x..x_end {
                    let dx = (x as isize - raw_px_x) as usize;
                    let dy = (y as isize - raw_px_y) as usize;
                    let rgba = rgba_bytes(sample_texture(texture, dx, dy, draw_w, draw_h));
                    blend_rgba_over_rgb(&mut buffer[y * buf_w + x], &rgba);
                }
            }
        }
    }

    fn render_like_gpu(
        terminal: &mut Terminal,
        atlas: &mut GlyphAtlas,
        config: &AppConfig,
    ) -> Vec<u32> {
        let width = terminal.cols as usize * atlas.cell_width;
        let height = terminal.rows as usize * atlas.cell_height;
        let base_bg = config.style.background.as_u32_rgb();
        let base_fg = config.style.foreground.as_u32_rgb();
        let mut buffer = vec![base_bg; width * height];

        let mut cell_infos = Vec::new();
        fill_cell_infos(terminal, &mut cell_infos);

        let mut glyph_textures = std::collections::HashMap::new();
        let mut next_x = 0u32;
        let mut batches = FrameTextBatches::default();
        fill_text_batches(
            &cell_infos,
            FrameBatchStyle {
                base_fg,
                base_bg,
                base_fg_f: [
                    ((base_fg >> 16) & 0xff) as f32 / 255.0,
                    ((base_fg >> 8) & 0xff) as f32 / 255.0,
                    (base_fg & 0xff) as f32 / 255.0,
                    1.0,
                ],
                background_alpha: clamp_background_alpha(config.style.background_opacity),
                cell_w: atlas.cell_width as f32,
                cell_h: atlas.cell_height as f32,
                viewport_offset_y: 0.0,
            },
            &mut batches,
            |ci| {
                let glyph = if let Some(grapheme) = ci.grapheme.as_deref() {
                    atlas.ensure_grapheme(grapheme);
                    atlas
                        .get_grapheme_glyph(grapheme)
                        .map(|glyph| (glyph, ci.cells))
                } else {
                    atlas.ensure_glyph(ci.ch);
                    atlas.get_glyph(ci.ch).map(|glyph| (glyph, ci.cells))
                }?;
                let is_color = glyph.0.format == GlyphFormat::Rgba;
                let (tile, tile_width, tile_height, left_pad, top_pad) = build_gpu_glyph_tile(
                    &glyph.0,
                    glyph.1,
                    atlas.cell_width,
                    atlas.cell_height,
                    atlas.baseline,
                );
                let entry = GlyphAtlasEntry {
                    x: next_x,
                    y: 0,
                    width: tile_width,
                    height: tile_height,
                    left_pad,
                    top_pad,
                    is_color,
                };
                glyph_textures.insert(
                    (entry.x, entry.y, entry.width, entry.height),
                    TestAtlasTexture {
                        pixels: tile,
                        width: tile_width,
                        height: tile_height,
                    },
                );
                next_x = next_x.saturating_add(tile_width + 1);
                Some(entry)
            },
        );

        let mut image_textures = std::collections::HashMap::new();
        let mut image_instances = Vec::new();
        fill_image_instances(
            terminal.kitty_placements(),
            atlas.cell_width as f32,
            atlas.cell_height as f32,
            &mut image_instances,
            |placement| {
                let image = terminal.kitty_image(placement.image_id)?;
                if image.data.len() != (image.width as usize) * (image.height as usize) * 4 {
                    return None;
                }
                let rect = AtlasImageRect {
                    x: next_x,
                    y: 1,
                    width: image.width,
                    height: image.height,
                };
                image_textures.insert(
                    (rect.x, rect.y, rect.width, rect.height),
                    TestAtlasTexture {
                        pixels: image.data.clone(),
                        width: image.width,
                        height: image.height,
                    },
                );
                next_x = next_x.saturating_add(image.width.max(1) + 1);
                Some(rect)
            },
        );

        draw_cell_instances(
            &mut buffer,
            width,
            height,
            &batches.bg_instances,
            &glyph_textures,
        );
        draw_image_instances(
            &mut buffer,
            width,
            height,
            &image_instances,
            &image_textures,
        );
        draw_cell_instances(
            &mut buffer,
            width,
            height,
            &batches.fg_instances,
            &glyph_textures,
        );
        draw_cell_instances(
            &mut buffer,
            width,
            height,
            &batches.overlay_instances,
            &glyph_textures,
        );

        buffer
    }

    fn assert_gpu_framebuffer_matches_cpu(
        cols: u16,
        rows: u16,
        chunks: &[&[u8]],
        per_step_assert: impl Fn(&Terminal, usize),
    ) {
        let config = AppConfig::default();
        let mut atlas = GlyphAtlas::new(config.style.font_size)
            .expect("should load font atlas for GPU framebuffer parity");
        let mut terminal = Terminal::new(cols, rows);
        let mut cpu = OffscreenRenderer::new(cols, rows, &atlas);

        for (idx, chunk) in chunks.iter().enumerate() {
            terminal.process(chunk);
            cpu.reset();
            cpu.render(&mut terminal, &mut atlas, &config);
            let gpu = render_like_gpu(&mut terminal, &mut atlas, &config);
            if let Some((pixel_idx, (gpu_px, cpu_px))) = gpu
                .iter()
                .zip(cpu.pixels.iter())
                .enumerate()
                .find(|(_, (gpu_px, cpu_px))| gpu_px != cpu_px)
            {
                let x = pixel_idx % cpu.width;
                let y = pixel_idx / cpu.width;
                let mut cell_infos = Vec::new();
                fill_cell_infos(&terminal, &mut cell_infos);
                let mut batches = FrameTextBatches::default();
                let mut glyph_textures = std::collections::HashMap::new();
                let mut next_x = 0u32;
                fill_text_batches(
                    &cell_infos,
                    FrameBatchStyle {
                        base_fg: config.style.foreground.as_u32_rgb(),
                        base_bg: config.style.background.as_u32_rgb(),
                        base_fg_f: [
                            ((config.style.foreground.as_u32_rgb() >> 16) & 0xff) as f32 / 255.0,
                            ((config.style.foreground.as_u32_rgb() >> 8) & 0xff) as f32 / 255.0,
                            (config.style.foreground.as_u32_rgb() & 0xff) as f32 / 255.0,
                            1.0,
                        ],
                        background_alpha: clamp_background_alpha(config.style.background_opacity),
                        cell_w: atlas.cell_width as f32,
                        cell_h: atlas.cell_height as f32,
                        viewport_offset_y: 0.0,
                    },
                    &mut batches,
                    |ci| {
                        let glyph = if let Some(grapheme) = ci.grapheme.as_deref() {
                            atlas.ensure_grapheme(grapheme);
                            atlas
                                .get_grapheme_glyph(grapheme)
                                .map(|glyph| (glyph, ci.cells))
                        } else {
                            atlas.ensure_glyph(ci.ch);
                            atlas.get_glyph(ci.ch).map(|glyph| (glyph, ci.cells))
                        }?;
                        let is_color = glyph.0.format == GlyphFormat::Rgba;
                        let (tile, tile_width, tile_height, left_pad, top_pad) =
                            build_gpu_glyph_tile(
                                &glyph.0,
                                glyph.1,
                                atlas.cell_width,
                                atlas.cell_height,
                                atlas.baseline,
                            );
                        let entry = GlyphAtlasEntry {
                            x: next_x,
                            y: 0,
                            width: tile_width,
                            height: tile_height,
                            left_pad,
                            top_pad,
                            is_color,
                        };
                        glyph_textures.insert(
                            (entry.x, entry.y, entry.width, entry.height),
                            TestAtlasTexture {
                                pixels: tile,
                                width: tile_width,
                                height: tile_height,
                            },
                        );
                        next_x = next_x.saturating_add(tile_width + 1);
                        Some(entry)
                    },
                );
                let pixel_in_instance = |instance: &CellInstance| {
                    let px = x as f32 + 0.5;
                    let py = y as f32 + 0.5;
                    px >= instance.pos[0]
                        && px < instance.pos[0] + instance.size[0]
                        && py >= instance.pos[1]
                        && py < instance.pos[1] + instance.size[1]
                };
                let bg_hits = batches
                    .bg_instances
                    .iter()
                    .filter(|i| pixel_in_instance(i))
                    .count();
                let fg_hits = batches
                    .fg_instances
                    .iter()
                    .filter(|i| pixel_in_instance(i))
                    .count();
                let overlay_hits = batches
                    .overlay_instances
                    .iter()
                    .filter(|i| pixel_in_instance(i))
                    .count();
                let first_cell = cell_infos.first();
                let first_fg = batches.fg_instances.first();
                panic!(
                    "GPU framebuffer parity diverged after replay chunk {idx} at pixel ({x},{y}): gpu=0x{gpu_px:06x} cpu=0x{cpu_px:06x} cell=({}, {}) bg_hits={} fg_hits={} overlay_hits={} first_cell={:?} first_fg={:?}",
                    x / atlas.cell_width,
                    y / atlas.cell_height,
                    bg_hits,
                    fg_hits,
                    overlay_hits,
                    first_cell,
                    first_fg,
                );
            }
            per_step_assert(&terminal, idx);
        }
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

        let (tile, width, height, left_pad, top_pad) = build_gpu_glyph_tile(&glyph, 2, 8, 4, 2);

        assert_eq!(width, 16);
        assert_eq!(height, 4);
        assert_eq!(left_pad, 0);
        assert_eq!(top_pad, 0);
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

        let (_tile, width, height, left_pad, top_pad) = build_gpu_glyph_tile(&glyph, 1, 8, 4, 1);

        assert_eq!(width, 10);
        assert_eq!(height, 4);
        assert_eq!(left_pad, 0);
        assert_eq!(top_pad, 0);
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

        let (_tile, width, height, left_pad, top_pad) = build_gpu_glyph_tile(&glyph, 1, 8, 4, 1);

        assert_eq!(width, 10);
        assert_eq!(height, 4);
        assert_eq!(left_pad, 2);
        assert_eq!(top_pad, 0);
    }

    #[test]
    fn gpu_glyph_tile_preserves_top_overhang() {
        let pixels = [255u8, 255, 255, 255];
        let glyph = crate::font::GlyphData {
            pixels: &pixels,
            width: 2,
            height: 2,
            format: GlyphFormat::Alpha,
            bearing_x: 0,
            bearing_y: 5,
        };

        let (_tile, width, height, left_pad, top_pad) = build_gpu_glyph_tile(&glyph, 1, 8, 4, 1);

        assert_eq!(width, 8);
        assert_eq!(height, 6);
        assert_eq!(left_pad, 0);
        assert_eq!(top_pad, 2);
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

    #[test]
    fn build_surface_config_reuses_preferred_defaults_without_capabilities() {
        let size = winit::dpi::PhysicalSize::new(800, 600);
        let defaults = GpuSurfaceDefaults {
            format: wgpu::TextureFormat::Bgra8Unorm,
            present_mode: wgpu::PresentMode::Mailbox,
            alpha_mode: wgpu::CompositeAlphaMode::PreMultiplied,
        };

        let (config, reused) = build_surface_config(size, true, Some(defaults), None)
            .expect("preferred defaults should build a surface config");

        assert!(reused);
        assert_eq!(config.width, 800);
        assert_eq!(config.height, 600);
        assert_eq!(config.format, defaults.format);
        assert_eq!(config.present_mode, defaults.present_mode);
        assert_eq!(config.alpha_mode, defaults.alpha_mode);
    }

    #[test]
    fn build_surface_config_uses_capabilities_when_defaults_absent() {
        let size = winit::dpi::PhysicalSize::new(640, 480);
        let capabilities = wgpu::SurfaceCapabilities {
            formats: vec![
                wgpu::TextureFormat::Bgra8UnormSrgb,
                wgpu::TextureFormat::Bgra8Unorm,
            ],
            present_modes: vec![wgpu::PresentMode::Fifo, wgpu::PresentMode::AutoVsync],
            alpha_modes: vec![
                wgpu::CompositeAlphaMode::Opaque,
                wgpu::CompositeAlphaMode::PostMultiplied,
            ],
            usages: wgpu::TextureUsages::RENDER_ATTACHMENT,
        };

        let (config, reused) = build_surface_config(size, true, None, Some(&capabilities))
            .expect("capabilities should build a surface config");

        assert!(!reused);
        assert_eq!(config.width, 640);
        assert_eq!(config.height, 480);
        assert_eq!(config.format, wgpu::TextureFormat::Bgra8Unorm);
        assert_eq!(config.present_mode, wgpu::PresentMode::Fifo);
        assert_eq!(config.alpha_mode, wgpu::CompositeAlphaMode::PostMultiplied);
    }

    #[test]
    fn gpu_framebuffer_matches_cpu_for_emoji_and_shade_transcript() {
        assert_gpu_framebuffer_matches_cpu(16, 4, EMOJI_AND_SHADE_TRANSCRIPT, |terminal, idx| {
            if idx == EMOJI_AND_SHADE_TRANSCRIPT.len() - 1 {
                assert_eq!(terminal.grid.cell_grapheme_at(0, 7), Some("❤️"));
                assert_eq!(terminal.grid.cell_grapheme_at(0, 10), Some("👨‍💻"));
            }
        });
    }

    #[test]
    fn gpu_framebuffer_matches_cpu_for_generic_emoji_probe() {
        let chunks: &[&[u8]] = &[
            "A🪸B A🫠B A🫡B\r\n".as_bytes(),
            "A🩷B A😀B A❤️B\r\n".as_bytes(),
            "A👨‍💻B A🇺🇸B A👍🏻B A1️⃣B".as_bytes(),
        ];

        assert_gpu_framebuffer_matches_cpu(32, 4, chunks, |terminal, idx| {
            if idx == chunks.len() - 1 {
                assert_eq!(terminal.grid.cell_char(0, 3), 'B');
                assert_eq!(terminal.grid.cell_char(1, 3), 'B');
                assert_eq!(terminal.grid.cell_char(2, 3), 'B');
                assert_eq!(terminal.grid.cell_grapheme_at(1, 11), Some("❤️"));
                assert_eq!(terminal.grid.cell_grapheme_at(2, 1), Some("👨‍💻"));
                assert_eq!(terminal.grid.cell_grapheme_at(2, 6), Some("🇺🇸"));
                assert_eq!(terminal.grid.cell_grapheme_at(2, 11), Some("👍🏻"));
                assert_eq!(terminal.grid.cell_grapheme_at(2, 16), Some("1️⃣"));
            }
        });
    }

    #[test]
    fn gpu_framebuffer_matches_cpu_for_jcode_like_glyph_probe() {
        let chunks: &[&[u8]] = &[
            "⟨client⟩\r\n".as_bytes(),
            "Ancient Coral 🪸\r\n".as_bytes(),
            "● an  ● or  ● oa  ● cu  ● cp  ● ge(oauth)  ○ ag\r\n".as_bytes(),
            "⠼ connecting… 3.6s · websocket/persistent-fresh 󰌘".as_bytes(),
        ];

        assert_gpu_framebuffer_matches_cpu(64, 6, chunks, |terminal, idx| {
            if idx == chunks.len() - 1 {
                assert_eq!(terminal.grid.cell_char(1, 14), '🪸');
                assert_eq!(terminal.grid.cell_char(3, 0), '⠼');
                assert_eq!(terminal.grid.cell_char(3, 48), '󰌘');
            }
        });
    }

    #[test]
    fn gpu_framebuffer_matches_cpu_for_starship_prompt_transcript() {
        assert_gpu_framebuffer_matches_cpu(80, 24, STARSHIP_PROMPT_TRANSCRIPT, |terminal, idx| {
            if idx == STARSHIP_PROMPT_TRANSCRIPT.len() - 1 {
                let row = (0..80)
                    .filter_map(|col| match terminal.grid.cell_char(1, col) {
                        ' ' | '\0' => None,
                        ch => Some(ch),
                    })
                    .collect::<String>();
                assert!(row.contains("jeremy"));
            }
        });
    }

    #[test]
    fn gpu_framebuffer_matches_cpu_for_tui_help_image_transcript() {
        assert_gpu_framebuffer_matches_cpu(
            32,
            8,
            TUI_HELP_WITH_IMAGE_TRANSCRIPT,
            |terminal, idx| {
                if idx == 2 {
                    assert_eq!(terminal.kitty_placements().len(), 1);
                }
                if idx == TUI_HELP_WITH_IMAGE_TRANSCRIPT.len() - 1 {
                    assert!(terminal.kitty_placements().is_empty());
                    assert!(
                        terminal.kitty_image(5).is_some(),
                        "image metadata should still exist even after the visible placement is cleared"
                    );
                }
            },
        );
    }
}
