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
use winit::dpi::{PhysicalPosition, PhysicalSize, Size};
use winit::event_loop::ActiveEventLoop;
use winit::window::{ImePurpose, Window, WindowAttributes};

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    screen_size: [f32; 2],
    cell_size: [f32; 2],
    atlas_size: [f32; 2],
    grid_offset: [f32; 2],
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

fn create_shared_atlas_state(device: &wgpu::Device) -> (SharedAtlasState, Duration) {
    let start = Instant::now();
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
    (
        SharedAtlasState {
            atlas_texture,
            atlas_view,
            glyph_map: HashMap::with_capacity(256),
            grapheme_map: HashMap::with_capacity(32),
            image_map: HashMap::with_capacity(32),
            atlas_cursor_x: 0,
            atlas_cursor_y: 0,
            atlas_row_height: 0,
            last_kitty_generation: 0,
        },
        start.elapsed(),
    )
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SharedGpuInitProfile {
    pub adapter_request: Duration,
    pub device_request: Duration,
    pub bind_group_layout: Duration,
    pub shader_modules: Duration,
    pub pipeline_layout: Duration,
    pub sampler: Duration,
    pub atlas_texture: Duration,
    pub total: Duration,
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

impl GpuSurfaceCreateProfile {
    pub fn compositor_facing_total(self) -> Duration {
        self.window_create
            .saturating_add(self.surface_create)
            .saturating_add(self.capabilities)
            .saturating_add(self.configure)
    }

    pub fn handterm_setup_total(self) -> Duration {
        self.ime_setup
            .saturating_add(self.default_config)
            .saturating_add(self.atlas_texture)
            .saturating_add(self.uniform_buffer)
            .saturating_add(self.instance_buffers)
            .saturating_add(self.bind_group)
            .saturating_add(self.pipeline_lookup)
    }

    pub fn unaccounted_total(self) -> Duration {
        self.total.saturating_sub(
            self.compositor_facing_total()
                .saturating_add(self.handterm_setup_total()),
        )
    }
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

const SHADER: &str = include_str!("shaders/terminal.wgsl");

const IMAGE_SHADER: &str = include_str!("shaders/image.wgsl");

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
}

pub fn create_window_attributes(
    config: &AppConfig,
    atlas: &GlyphAtlas,
    title: &str,
    spawn_monitor: Option<(PhysicalPosition<i32>, PhysicalSize<u32>)>,
    cascade_index: usize,
) -> WindowAttributes {
    create_window_attributes_for_metrics(
        config,
        atlas.cell_width,
        atlas.cell_height,
        atlas.dpi(),
        title,
        spawn_monitor,
        cascade_index,
    )
}

pub fn create_window_attributes_for_metrics(
    config: &AppConfig,
    cell_width: usize,
    cell_height: usize,
    dpi: u32,
    title: &str,
    spawn_monitor: Option<(PhysicalPosition<i32>, PhysicalSize<u32>)>,
    cascade_index: usize,
) -> WindowAttributes {
    // `cell_width`/`cell_height` are already in *physical* pixels: they come
    // from rasterizing the font at the display DPI (`font_size_pt * dpi / 72`).
    // The window inner size must therefore be requested in physical pixels.
    //
    // Using `Size::Logical` here was a latent bug on HiDPI displays: winit
    // multiplies a logical size by the monitor scale factor, so on a 2x retina
    // Mac an 80x24 grid produced a 4x-too-large surface. On scale-factor-1
    // platforms (typical Linux) logical and physical coincide, so the bug was
    // invisible there and only inflated macOS per-window footprint.
    // Blank padding (physical px) surrounds the cell grid on all sides so
    // glyphs are not clipped by rounded window corners (see WindowConfig).
    let pad = 2.0 * config.window.padding_px(dpi) as f64;
    let width = config.window.columns as f64 * cell_width as f64 + pad;
    let height = config.window.rows as f64 * cell_height as f64 + pad;

    let inner = PhysicalSize::new(
        width.round().max(1.0) as u32,
        height.round().max(1.0) as u32,
    );
    let attrs =
        crate::platform::with_app_id(Window::default_attributes().with_title(title), "handterm")
            .with_transparent(transparency_requested(config.style.background_opacity))
            .with_inner_size(Size::Physical(inner));
    let attrs = crate::platform::with_decorations(attrs, config.window.decorations);
    // Spawn position policy (config `window.position`): centered by default,
    // cascading extra windows. Wayland ignores position hints; macOS/X11
    // honor them.
    let attrs = crate::platform::with_initial_position(
        attrs,
        crate::platform::initial_window_position(
            config.window.position,
            inner,
            spawn_monitor,
            cascade_index,
        ),
    );

    // On macOS, AppKit otherwise grows a freshly created window to fill the
    // display it lands on (observed: an 80x24 request settling at the full
    // monitor height). The GPU swapchain drawables are sized to the *actual*
    // window, so that auto-grow inflated the dominant per-window memory cost
    // (two ~13 MB IOSurfaces instead of two ~4.5 MB ones). Clamp the initial
    // max size to the requested grid size to defeat the auto-grow. The cap is
    // lifted again once the first frame is presented (see gpu_app), so the
    // window stays freely resizable in steady state.
    #[cfg(target_os = "macos")]
    let attrs = attrs.with_max_inner_size(Size::Physical(inner));

    attrs
}

pub fn create_shared_gpu_context_profiled() -> Result<(Arc<SharedGpuContext>, SharedGpuInitProfile)>
{
    let total_start = Instant::now();
    // Pick the native graphics backend for the platform: Metal on macOS,
    // Vulkan on Linux/other. Falling back to `Backends::all()` keeps things
    // working if a platform exposes a different native backend.
    #[cfg(target_os = "macos")]
    let backends = wgpu::Backends::METAL;
    #[cfg(target_os = "linux")]
    let backends = wgpu::Backends::VULKAN;
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let backends = wgpu::Backends::all();

    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends,
        ..Default::default()
    });

    let step_start = Instant::now();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .context("no suitable GPU adapter found")?;
    let adapter_request = step_start.elapsed();

    let step_start = Instant::now();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("handterm"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        ..Default::default()
    }))
    .context("device creation should succeed")?;
    let device_request = step_start.elapsed();

    let step_start = Instant::now();
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
    let bind_group_layout_create = step_start.elapsed();

    let step_start = Instant::now();
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("shader"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let image_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("image_shader"),
        source: wgpu::ShaderSource::Wgsl(IMAGE_SHADER.into()),
    });
    let shader_modules = step_start.elapsed();

    let step_start = Instant::now();
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("pipeline_layout"),
        bind_group_layouts: &[&bind_group_layout],
        immediate_size: 0,
    });
    let pipeline_layout_create = step_start.elapsed();

    let step_start = Instant::now();
    let atlas_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });
    let atlas_sampler_create = step_start.elapsed();

    let (atlas_state, atlas_texture_create) = create_shared_atlas_state(&device);

    let shared = Arc::new(SharedGpuContext {
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
        atlas: Mutex::new(atlas_state),
    });

    Ok((
        shared,
        SharedGpuInitProfile {
            adapter_request,
            device_request,
            bind_group_layout: bind_group_layout_create,
            shader_modules,
            pipeline_layout: pipeline_layout_create,
            sampler: atlas_sampler_create,
            atlas_texture: atlas_texture_create,
            total: total_start.elapsed(),
        },
    ))
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
            .create_window(create_window_attributes(
                config,
                atlas,
                title,
                crate::platform::spawn_monitor_geometry(event_loop),
                0,
            ))
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

    let pad = config.window.padding_px(atlas.dpi()) as f32;
    let uniforms = Uniforms {
        screen_size: [size.width as f32, size.height as f32],
        cell_size: [atlas.cell_width as f32, atlas.cell_height as f32],
        atlas_size: [ATLAS_WIDTH as f32, ATLAS_HEIGHT as f32],
        grid_offset: [pad, pad],
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
    padding_px: u32,
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

    let pad = padding_px as f32;
    let uniforms = Uniforms {
        screen_size: [width as f32, height as f32],
        cell_size: [atlas.cell_width as f32, atlas.cell_height as f32],
        atlas_size: [ATLAS_WIDTH as f32, ATLAS_HEIGHT as f32],
        grid_offset: [pad, pad],
    };
    state
        .shared
        .queue
        .write_buffer(&state.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
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
mod tests;
