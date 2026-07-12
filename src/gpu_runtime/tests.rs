// GPU runtime tests, including the CPU-parity harness that cross-checks
// the GPU framebuffer against the CPU renderer (render.rs).
// Moved verbatim from the tail of gpu_runtime.rs.

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
    let src_x = ((((dx as f32) + 0.5) * texture.width as f32 / draw_w.max(1) as f32) - 0.5).round()
        as isize;
    let src_y = ((((dy as f32) + 0.5) * texture.height as f32 / draw_h.max(1) as f32) - 0.5).round()
        as isize;
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

fn render_like_gpu_with_scroll(
    terminal: &mut Terminal,
    atlas: &mut GlyphAtlas,
    config: &AppConfig,
    scroll_rows: f32,
) -> Vec<u32> {
    let width = terminal.cols as usize * atlas.cell_width;
    let height = terminal.rows as usize * atlas.cell_height;
    let base_bg = config.style.background.as_u32_rgb();
    let base_fg = config.style.foreground.as_u32_rgb();
    let mut buffer = vec![base_bg; width * height];

    let effective_scroll_rows = terminal.grid().scroll_offset as f32 + scroll_rows.max(0.0);
    let viewport_scroll = ViewportScroll::from_scroll_rows(effective_scroll_rows);

    let mut cell_infos = Vec::new();
    if viewport_scroll == ViewportScroll::ZERO {
        fill_cell_infos(terminal, &mut cell_infos);
    } else {
        fill_cell_infos_with_scroll(terminal, &mut cell_infos, viewport_scroll);
    }

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
            cell_w: atlas.cell_width as f32,
            cell_h: atlas.cell_height as f32,
            viewport_offset_y: viewport_scroll.viewport_offset_y(atlas.cell_height as f32),
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
    if config.scrollback.scrollbar {
        append_scrollbar_overlay_instances(
            &mut batches.overlay_instances,
            base_fg,
            width as f32,
            height as f32,
            terminal.grid().scrollback_len(),
            terminal.grid().rows,
            effective_scroll_rows,
        );
    }

    let mut image_textures = std::collections::HashMap::new();
    let mut image_instances = Vec::new();
    if viewport_scroll == ViewportScroll::ZERO {
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
    } else {
        fill_image_instances_with_viewport_offset(
            terminal.kitty_placements(),
            atlas.cell_width as f32,
            atlas.cell_height as f32,
            viewport_scroll.viewport_offset_y(atlas.cell_height as f32),
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
    }

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

fn render_like_gpu(
    terminal: &mut Terminal,
    atlas: &mut GlyphAtlas,
    config: &AppConfig,
) -> Vec<u32> {
    render_like_gpu_with_scroll(terminal, atlas, config, 0.0)
}

fn sample_rgb(buffer: &[u32], width: usize, x: usize, y: usize) -> u32 {
    buffer[y * width + x] & 0x00ff_ffff
}

fn assert_gpu_framebuffer_matches_cpu(
    cols: u16,
    rows: u16,
    chunks: &[&[u8]],
    per_step_assert: impl Fn(&Terminal, usize),
) {
    assert_gpu_framebuffer_matches_cpu_with_dpi(cols, rows, 96, chunks, per_step_assert);
}

fn assert_gpu_framebuffer_matches_cpu_with_dpi(
    cols: u16,
    rows: u16,
    dpi: u32,
    chunks: &[&[u8]],
    per_step_assert: impl Fn(&Terminal, usize),
) {
    let config = AppConfig::default();
    let mut atlas =
        GlyphAtlas::with_family_dpi(&config.style.font_family, config.style.font_size, dpi)
            .or_else(|_| GlyphAtlas::new_with_dpi(config.style.font_size, dpi))
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
fn surface_profile_aggregates_compositor_and_handterm_work() {
    let profile = GpuSurfaceCreateProfile {
        window_create: Duration::from_millis(11),
        ime_setup: Duration::from_millis(2),
        surface_create: Duration::from_millis(13),
        default_config: Duration::from_millis(3),
        capabilities: Duration::from_millis(17),
        configure: Duration::from_millis(19),
        atlas_texture: Duration::from_millis(5),
        uniform_buffer: Duration::from_millis(7),
        instance_buffers: Duration::from_millis(23),
        bind_group: Duration::from_millis(29),
        pipeline_lookup: Duration::from_millis(31),
        total: Duration::from_millis(160),
        ..GpuSurfaceCreateProfile::default()
    };

    assert_eq!(profile.compositor_facing_total(), Duration::from_millis(60));
    assert_eq!(profile.handterm_setup_total(), Duration::from_millis(100));
    assert_eq!(profile.unaccounted_total(), Duration::ZERO);
}

#[test]
fn surface_profile_unaccounted_total_saturates_at_zero() {
    let profile = GpuSurfaceCreateProfile {
        window_create: Duration::from_millis(5),
        surface_create: Duration::from_millis(5),
        capabilities: Duration::from_millis(5),
        configure: Duration::from_millis(5),
        ime_setup: Duration::from_millis(5),
        default_config: Duration::from_millis(5),
        atlas_texture: Duration::from_millis(5),
        uniform_buffer: Duration::from_millis(5),
        instance_buffers: Duration::from_millis(5),
        bind_group: Duration::from_millis(5),
        pipeline_lookup: Duration::from_millis(5),
        total: Duration::from_millis(10),
        ..GpuSurfaceCreateProfile::default()
    };

    assert_eq!(profile.unaccounted_total(), Duration::ZERO);
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
fn build_surface_config_reuses_supported_preferred_defaults() {
    let size = winit::dpi::PhysicalSize::new(800, 600);
    let defaults = GpuSurfaceDefaults {
        format: wgpu::TextureFormat::Bgra8Unorm,
        present_mode: wgpu::PresentMode::Mailbox,
        alpha_mode: wgpu::CompositeAlphaMode::PreMultiplied,
    };

    let capabilities = wgpu::SurfaceCapabilities {
        formats: vec![defaults.format],
        present_modes: vec![defaults.present_mode],
        alpha_modes: vec![defaults.alpha_mode],
        usages: wgpu::TextureUsages::RENDER_ATTACHMENT,
    };
    let (config, reused) = build_surface_config(size, true, Some(defaults), Some(&capabilities))
        .expect("preferred defaults should build a surface config");

    assert!(reused);
    assert_eq!(config.width, 800);
    assert_eq!(config.height, 600);
    assert_eq!(config.format, defaults.format);
    assert_eq!(config.present_mode, defaults.present_mode);
    assert_eq!(config.alpha_mode, defaults.alpha_mode);
}

#[test]
fn build_surface_config_rejects_defaults_unsupported_by_new_surface() {
    let size = winit::dpi::PhysicalSize::new(800, 600);
    let defaults = GpuSurfaceDefaults {
        format: wgpu::TextureFormat::Rgba16Float,
        present_mode: wgpu::PresentMode::Mailbox,
        alpha_mode: wgpu::CompositeAlphaMode::PreMultiplied,
    };
    let capabilities = wgpu::SurfaceCapabilities {
        formats: vec![wgpu::TextureFormat::Bgra8Unorm],
        present_modes: vec![wgpu::PresentMode::Fifo],
        alpha_modes: vec![wgpu::CompositeAlphaMode::Opaque],
        usages: wgpu::TextureUsages::RENDER_ATTACHMENT,
    };

    let (config, reused) = build_surface_config(size, false, Some(defaults), Some(&capabilities))
        .expect("unsupported defaults should fall back to capabilities");

    assert!(!reused);
    assert_eq!(config.format, wgpu::TextureFormat::Bgra8Unorm);
    assert_eq!(config.present_mode, wgpu::PresentMode::Fifo);
    assert_eq!(config.alpha_mode, wgpu::CompositeAlphaMode::Opaque);
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
fn window_inner_size_is_requested_in_physical_pixels() {
    // The cell metrics are already physical pixels (rasterized at display
    // DPI). Requesting the window size in physical pixels keeps the GPU
    // surface exactly grid-sized; requesting it as logical would let winit
    // re-multiply by the HiDPI scale factor and quadruple drawable memory on
    // a 2x display. Pinning the request as `Size::Physical` guards that.
    let mut config = AppConfig::default();
    config.window.columns = 80;
    config.window.rows = 24;
    let cell_width = 18;
    let cell_height = 33;

    let attrs =
        create_window_attributes_for_metrics(&config, cell_width, cell_height, 96, "t", None, 0);
    let inner = attrs.inner_size.expect("inner size should be set");

    // The requested size is the cell grid plus the blank window padding on
    // each side (physical px, DPI-scaled; 96 dpi here so pad = padding pt).
    let pad2 = 2 * config.window.padding_px(96);
    let expect_w = (80 * cell_width) as u32 + pad2;
    let expect_h = (24 * cell_height) as u32 + pad2;

    match inner {
        Size::Physical(size) => {
            assert_eq!(size.width, expect_w);
            assert_eq!(size.height, expect_h);
        }
        Size::Logical(other) => panic!("expected physical inner size, got logical {other:?}"),
    }

    // Independent of any scale factor, the physical request is constant, so
    // the Metal drawable surface (and thus per-window IOSurface memory)
    // stays proportional to the grid rather than the grid times scale^2.
    let physical: winit::dpi::PhysicalSize<u32> = inner.to_physical(2.0);
    assert_eq!(physical.width, expect_w);
    assert_eq!(physical.height, expect_h);
}

#[cfg(target_os = "macos")]
#[test]
fn window_max_inner_size_is_clamped_to_grid_on_macos() {
    // On macOS the freshly created window is otherwise auto-grown by AppKit
    // to fill its display, which inflates the GPU swapchain drawables. The
    // attributes clamp the initial max size to the requested grid size so the
    // window opens grid-sized; gpu_app lifts the cap after the first frame so
    // the window remains resizable.
    let mut config = AppConfig::default();
    config.window.columns = 80;
    config.window.rows = 24;
    let cell_width = 18;
    let cell_height = 33;

    let attrs =
        create_window_attributes_for_metrics(&config, cell_width, cell_height, 96, "t", None, 0);
    let max = attrs
        .max_inner_size
        .expect("max inner size should be set on macOS");
    let pad2 = 2 * config.window.padding_px(96);
    match max {
        Size::Physical(size) => {
            assert_eq!(size.width, (80 * cell_width) as u32 + pad2);
            assert_eq!(size.height, (24 * cell_height) as u32 + pad2);
        }
        Size::Logical(other) => panic!("expected physical max inner size, got {other:?}"),
    }
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
fn gpu_framebuffer_matches_cpu_for_generic_emoji_probe_at_high_dpi() {
    let chunks: &[&[u8]] = &[
        "A🪸B A🫠B A🫡B\r\n".as_bytes(),
        "A🩷B A😀B A❤️B\r\n".as_bytes(),
        "A👨‍💻B A🇺🇸B A👍🏻B A1️⃣B".as_bytes(),
    ];

    for dpi in [144u32, 217] {
        assert_gpu_framebuffer_matches_cpu_with_dpi(32, 4, dpi, chunks, |terminal, idx| {
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
}

#[test]
fn gpu_framebuffer_matches_cpu_for_digit_probe_at_high_dpi() {
    let chunks: &[&[u8]] = &[
        "0123456789\r\n".as_bytes(),
        "9876543210\r\n".as_bytes(),
        "1111111111 0000000000".as_bytes(),
    ];

    for dpi in [144u32, 217] {
        assert_gpu_framebuffer_matches_cpu_with_dpi(24, 4, dpi, chunks, |_terminal, _idx| {});
    }
}

#[test]
fn gpu_framebuffer_matches_cpu_for_selection_highlight() {
    let config = AppConfig::default();
    let mut atlas =
        GlyphAtlas::with_family_dpi(&config.style.font_family, config.style.font_size, 96)
            .or_else(|_| GlyphAtlas::new_with_dpi(config.style.font_size, 96))
            .expect("should load font atlas for GPU framebuffer selection parity");
    let mut terminal = Terminal::new(12, 2);
    let mut cpu = OffscreenRenderer::new(12, 2, &atlas);

    terminal.process(b"select me\r\nsecond row");
    terminal.grid.selection = Some(crate::grid::Selection {
        start_col: 0,
        start_row: 0,
        end_col: 5,
        end_row: 0,
    });

    cpu.reset();
    cpu.render(&mut terminal, &mut atlas, &config);
    let gpu = render_like_gpu(&mut terminal, &mut atlas, &config);

    assert_eq!(
        gpu, cpu.pixels,
        "GPU framebuffer should match CPU output for a visible selection highlight"
    );
}

#[test]
fn gpu_framebuffer_matches_cpu_for_incremental_typing() {
    let config = AppConfig::default();
    let mut atlas =
        GlyphAtlas::with_family_dpi(&config.style.font_family, config.style.font_size, 96)
            .or_else(|_| GlyphAtlas::new_with_dpi(config.style.font_size, 96))
            .expect("should load font atlas for GPU incremental typing parity");
    let mut terminal = Terminal::new(32, 2);
    let mut cpu = OffscreenRenderer::new(32, 2, &atlas);

    terminal.process(b"\x1b[38;5;10m>\x1b[0m ");
    cpu.render(&mut terminal, &mut atlas, &config);

    for &byte in b"echo hello world" {
        terminal.process(&[byte]);
        cpu.reset();
        cpu.render(&mut terminal, &mut atlas, &config);
        let gpu = render_like_gpu(&mut terminal, &mut atlas, &config);
        assert_eq!(
            gpu, cpu.pixels,
            "GPU framebuffer should match CPU output after typing byte {byte:?}"
        );
    }
}

#[test]
fn gpu_framebuffer_matches_cpu_for_line_repaint() {
    let config = AppConfig::default();
    let mut atlas =
        GlyphAtlas::with_family_dpi(&config.style.font_family, config.style.font_size, 96)
            .or_else(|_| GlyphAtlas::new_with_dpi(config.style.font_size, 96))
            .expect("should load font atlas for GPU line repaint parity");
    let mut terminal = Terminal::new(32, 2);
    let mut cpu = OffscreenRenderer::new(32, 2, &atlas);

    terminal.process(b"\x1b[38;5;10m>\x1b[0m build");
    cpu.render(&mut terminal, &mut atlas, &config);

    terminal.process(b"\r\x1b[2K\x1b[38;5;196merror:\x1b[0m failed");
    cpu.reset();
    cpu.render(&mut terminal, &mut atlas, &config);
    let gpu = render_like_gpu(&mut terminal, &mut atlas, &config);

    assert_eq!(
        gpu, cpu.pixels,
        "GPU framebuffer should match CPU output after a line repaint"
    );
}

#[test]
fn gpu_framebuffer_matches_cpu_for_resize_driven_layout_change() {
    let config = AppConfig::default();
    let mut atlas =
        GlyphAtlas::with_family_dpi(&config.style.font_family, config.style.font_size, 96)
            .or_else(|_| GlyphAtlas::new_with_dpi(config.style.font_size, 96))
            .expect("should load font atlas for GPU resize parity");
    let mut terminal = Terminal::new(8, 2);
    let mut cpu = OffscreenRenderer::new(8, 2, &atlas);

    terminal.process(b"alpha\r\nbeta gamma");
    terminal.resize(5, 3);
    terminal.process(b"\x1b[2J\x1b[H123\r\n45");
    terminal.cursor_visible = false;

    cpu.resize_pixels(
        terminal.cols as usize * atlas.cell_width,
        terminal.rows as usize * atlas.cell_height,
    );
    cpu.render(&mut terminal, &mut atlas, &config);
    let gpu = render_like_gpu(&mut terminal, &mut atlas, &config);

    assert_eq!(
        gpu, cpu.pixels,
        "GPU framebuffer should match CPU output after a resize-driven layout change"
    );
}

#[test]
fn gpu_framebuffer_matches_cpu_for_full_screen_repaint() {
    let config = AppConfig::default();
    let mut atlas =
        GlyphAtlas::with_family_dpi(&config.style.font_family, config.style.font_size, 96)
            .or_else(|_| GlyphAtlas::new_with_dpi(config.style.font_size, 96))
            .expect("should load font atlas for GPU full-screen repaint parity");
    let mut terminal = Terminal::new(32, 6);
    let mut cpu = OffscreenRenderer::new(32, 6, &atlas);

    terminal.process(
        b"\x1b[?1049h\
          one\r\n\
          two\r\n\
          three\r\n\
          four\r\n\
          five\r\n",
    );
    cpu.render(&mut terminal, &mut atlas, &config);

    terminal.process(
        b"\x1b[2J\x1b[H\
          \x1b[38;5;39mstatus\x1b[0m\r\n\
          alpha beta gamma\r\n\
          delta epsilon\r\n\
          zeta eta theta\r\n\
          iota kappa\r\n",
    );
    cpu.reset();
    cpu.render(&mut terminal, &mut atlas, &config);
    let gpu = render_like_gpu(&mut terminal, &mut atlas, &config);

    assert_eq!(
        gpu, cpu.pixels,
        "GPU framebuffer should match CPU output after a full-screen repaint"
    );
}

#[test]
fn gpu_framebuffer_matches_cpu_for_scrollback_selection_interaction() {
    let config = AppConfig::default();
    let mut atlas =
        GlyphAtlas::with_family_dpi(&config.style.font_family, config.style.font_size, 96)
            .or_else(|_| GlyphAtlas::new_with_dpi(config.style.font_size, 96))
            .expect("should load font atlas for GPU scrollback selection parity");
    let mut terminal = Terminal::new_with_scrollback(8, 2, 8);
    let mut cpu = OffscreenRenderer::new(8, 2, &atlas);

    terminal.process(b"one\r\ntwo\r\nthree\r\nfour\r\nfive");
    terminal.grid.scroll_offset = 1;
    terminal.grid.selection = Some(crate::grid::Selection {
        start_col: 0,
        start_row: 0,
        end_col: 2,
        end_row: 1,
    });
    terminal.cursor_visible = false;

    cpu.reset();
    cpu.render(&mut terminal, &mut atlas, &config);
    let gpu = render_like_gpu(&mut terminal, &mut atlas, &config);

    assert_eq!(
        gpu, cpu.pixels,
        "GPU framebuffer should match CPU output for scrollback + selection interaction"
    );
}

#[test]
fn gpu_framebuffer_matches_cpu_for_cursor_styles() {
    let config = AppConfig::default();
    let mut atlas =
        GlyphAtlas::with_family_dpi(&config.style.font_family, config.style.font_size, 96)
            .or_else(|_| GlyphAtlas::new_with_dpi(config.style.font_size, 96))
            .expect("should load font atlas for GPU cursor-style parity");

    for cursor_style in [
        crate::terminal::CursorStyle::Bar,
        crate::terminal::CursorStyle::Underline,
    ] {
        let mut terminal = Terminal::new(8, 2);
        let mut cpu = OffscreenRenderer::new(8, 2, &atlas);
        terminal.process(b"cursor");
        terminal.cursor_style = cursor_style;

        cpu.reset();
        cpu.render(&mut terminal, &mut atlas, &config);
        let gpu = render_like_gpu(&mut terminal, &mut atlas, &config);

        assert_eq!(
            gpu, cpu.pixels,
            "GPU framebuffer should match CPU output for cursor style {:?}",
            cursor_style
        );
    }
}

#[test]
fn gpu_fractional_scroll_framebuffer_shifts_rows_by_partial_cell_height() {
    let config = AppConfig::default();
    let mut atlas =
        GlyphAtlas::with_family_dpi(&config.style.font_family, config.style.font_size, 96)
            .or_else(|_| GlyphAtlas::new_with_dpi(config.style.font_size, 96))
            .expect("should load font atlas for fractional scroll framebuffer test");
    let mut terminal = Terminal::new_with_scrollback(2, 2, 8);
    terminal.process(
        b"\x1b[41m  \x1b[0m\r\n\
          \x1b[42m  \x1b[0m\r\n\
          \x1b[44m  \x1b[0m\r\n",
    );

    let width = terminal.cols as usize * atlas.cell_width;
    let cell_h = atlas.cell_height;
    assert!(cell_h >= 4, "expected at least 4 px cell height");

    let integer_scroll = render_like_gpu_with_scroll(&mut terminal, &mut atlas, &config, 1.0);
    let fractional_scroll = render_like_gpu_with_scroll(&mut terminal, &mut atlas, &config, 0.25);
    let baseline = render_like_gpu_with_scroll(&mut terminal, &mut atlas, &config, 0.0);

    let x = atlas.cell_width + (atlas.cell_width / 2);

    let top_color = sample_rgb(&integer_scroll, width, x, cell_h / 2);
    let middle_color = sample_rgb(&integer_scroll, width, x, cell_h + cell_h / 2);
    let bottom_color = sample_rgb(&baseline, width, x, cell_h + cell_h / 2);

    assert_ne!(
        top_color, middle_color,
        "expected distinct older/current row colors"
    );
    assert_ne!(
        middle_color, bottom_color,
        "expected distinct current/newer row colors"
    );

    let mut runs: Vec<(u32, usize, usize)> = Vec::new();
    for y in 0..(cell_h * 2) {
        let color = sample_rgb(&fractional_scroll, width, x, y);
        if let Some((run_color, _start, end)) = runs.last_mut()
            && *run_color == color
        {
            *end = y + 1;
        } else {
            runs.push((color, y, y + 1));
        }
    }

    assert_eq!(
        runs.len(),
        3,
        "fractional scroll should produce exactly three visible color bands, got {runs:?}",
    );
    assert_eq!(
        runs[0].0, top_color,
        "top band should come from the older row"
    );
    assert_eq!(
        runs[1].0, middle_color,
        "middle band should come from the current row"
    );
    assert_eq!(
        runs[2].0, bottom_color,
        "bottom band should come from the newer row"
    );
    assert!(
        runs.iter()
            .all(|(_, start, end)| end.saturating_sub(*start) >= 2),
        "each visible band should span at least two pixels: {runs:?}",
    );
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
    assert_gpu_framebuffer_matches_cpu(32, 8, TUI_HELP_WITH_IMAGE_TRANSCRIPT, |terminal, idx| {
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
    });
}
