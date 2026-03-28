use crate::terminal::{CursorStyle, KittyPlacement, TerminalView};
use crate::visual::{is_in_selection, resolve_cell_colors, resolve_underline_color};

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct CellInstance {
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub uv_offset: [f32; 2],
    pub uv_size: [f32; 2],
    pub fg: [f32; 4],
    pub bg: [f32; 4],
    pub deco: [f32; 4],
    pub flags: u32,
    pub _pad: [u32; 2],
}

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct ImageInstance {
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub uv_offset: [f32; 2],
    pub uv_size: [f32; 2],
}

pub(crate) const FLAG_HAS_GLYPH: u32 = 1;
pub(crate) const FLAG_UNDERLINE: u32 = 2;
pub(crate) const FLAG_STRIKETHROUGH: u32 = 4;
pub(crate) const FLAG_CURLY_UL: u32 = 8;
pub(crate) const FLAG_DOUBLE_UL: u32 = 16;
pub(crate) const FLAG_DOTTED_UL: u32 = 32;
pub(crate) const FLAG_DASHED_UL: u32 = 64;
pub(crate) const FLAG_COLOR_GLYPH: u32 = 128;
pub(crate) const FLAG_CURSOR_BAR: u32 = 256;
pub(crate) const FLAG_CURSOR_UNDERLINE: u32 = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GlyphAtlasEntry {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub left_pad: u32,
    pub top_pad: u32,
    pub is_color: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AtlasImageRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct CellInfo {
    pub row: usize,
    pub col: usize,
    pub ch: u32,
    pub grapheme: Option<Box<str>>,
    pub cells: usize,
    pub cell: crate::grid::Cell,
    pub selected: bool,
    pub is_cursor_block: bool,
    pub cursor_style: Option<CursorStyle>,
}

#[derive(Debug, Default)]
pub(crate) struct FramePlan {
    pub cell_infos: Vec<CellInfo>,
    pub image_placements: Vec<KittyPlacement>,
}

#[derive(Debug, Default)]
pub(crate) struct FrameTextBatches {
    pub bg_instances: Vec<CellInstance>,
    pub fg_instances: Vec<CellInstance>,
    pub overlay_instances: Vec<CellInstance>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct FrameBatchStyle {
    pub base_fg: u32,
    pub base_bg: u32,
    pub base_fg_f: [f32; 4],
    pub background_alpha: f32,
    pub cell_w: f32,
    pub cell_h: f32,
    pub viewport_offset_y: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ViewportScroll {
    pub sample_offset: usize,
    pub fractional_rows: f32,
}

impl ViewportScroll {
    pub const ZERO: Self = Self {
        sample_offset: 0,
        fractional_rows: 0.0,
    };

    pub fn from_scroll_rows(scroll_rows: f32) -> Self {
        const EPSILON: f32 = 0.001;

        let clamped = scroll_rows.max(0.0);
        let sample_offset = clamped.ceil() as usize;
        let fractional_rows = if sample_offset > 0 {
            sample_offset as f32 - clamped
        } else {
            0.0
        };

        Self {
            sample_offset,
            fractional_rows: if fractional_rows.abs() < EPSILON {
                0.0
            } else {
                fractional_rows
            },
        }
    }

    pub fn extra_visible_rows(self) -> usize {
        usize::from(self.fractional_rows > 0.0)
    }

    pub fn viewport_offset_y(self, cell_h: f32) -> f32 {
        -(self.fractional_rows * cell_h)
    }
}

pub(crate) fn image_instance_for_placement(
    placement: &KittyPlacement,
    entry: AtlasImageRect,
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

#[allow(dead_code)]
pub(crate) fn build_frame_plan(terminal: &impl TerminalView) -> FramePlan {
    let mut plan = FramePlan::default();
    fill_frame_plan(terminal, &mut plan);
    plan
}

pub(crate) fn fill_frame_plan(terminal: &impl TerminalView, plan: &mut FramePlan) {
    fill_cell_infos(terminal, &mut plan.cell_infos);
    plan.image_placements.clear();
    plan.image_placements.reserve(
        terminal
            .kitty_placements()
            .len()
            .saturating_sub(plan.image_placements.capacity()),
    );
    plan.image_placements
        .extend_from_slice(terminal.kitty_placements());
}

pub(crate) fn fill_cell_infos(terminal: &impl TerminalView, cell_infos: &mut Vec<CellInfo>) {
    fill_cell_infos_with_scroll(terminal, cell_infos, ViewportScroll::ZERO);
}

pub(crate) fn fill_cell_infos_with_scroll(
    terminal: &impl TerminalView,
    cell_infos: &mut Vec<CellInfo>,
    viewport_scroll: ViewportScroll,
) {
    let grid = terminal.grid();
    let (cursor_col, cursor_row) = grid.cursor_pos();
    let show_cursor = terminal.cursor_visible() && viewport_scroll.sample_offset == 0;
    let cursor_style = terminal.cursor_style();
    let selection = grid.selection;
    let visible_rows = grid.rows + viewport_scroll.extra_visible_rows();

    cell_infos.clear();
    let target_cells = visible_rows * grid.cols;
    cell_infos.reserve(target_cells.saturating_sub(cell_infos.capacity()));

    for row in 0..visible_rows {
        for col in 0..grid.cols {
            let cell = grid.cell_at_scrollback_offset(viewport_scroll.sample_offset, row, col);
            if cell.flags & crate::grid::FLAG_WIDE_CONT != 0 {
                continue;
            }

            let is_cursor = show_cursor && row == cursor_row && col == cursor_col;
            let is_cursor_block = is_cursor && cursor_style == CursorStyle::Block;
            let cursor_overlay = if is_cursor && !is_cursor_block {
                Some(cursor_style)
            } else {
                None
            };

            cell_infos.push(CellInfo {
                row,
                col,
                ch: cell.ch,
                grapheme: terminal
                    .grid()
                    .cell_grapheme_at_scrollback_offset(viewport_scroll.sample_offset, row, col)
                    .map(Into::into),
                cells: cell_span(cell),
                cell: *cell,
                selected: is_in_selection(selection, row, col),
                is_cursor_block,
                cursor_style: cursor_overlay,
            });
        }
    }
}

pub(crate) fn build_cell_instances(
    ci: &CellInfo,
    style: FrameBatchStyle,
    glyph_entry: Option<GlyphAtlasEntry>,
) -> (
    Option<CellInstance>,
    Option<CellInstance>,
    Option<CellInstance>,
) {
    let colors = resolve_cell_colors(
        &ci.cell,
        style.base_fg,
        style.base_bg,
        ci.is_cursor_block,
        ci.selected,
    );
    let fg = rgb_to_f32(colors.fg);
    let bg = if colors.bg == style.base_bg {
        rgb_to_f32_alpha(colors.bg, style.background_alpha)
    } else {
        rgb_to_f32(colors.bg)
    };
    let deco = rgb_to_f32(resolve_underline_color(&ci.cell, colors.fg));

    let mut flags = 0u32;
    let mut uv_offset = [0.0, 0.0];
    let mut uv_size = [0.0, 0.0];
    let mut glyph_left_pad = 0.0f32;
    let mut glyph_top_pad = 0.0f32;

    if (ci.ch > 0x20 || ci.grapheme.is_some())
        && let Some(entry) = glyph_entry
        && entry.width > 0
    {
        flags |= FLAG_HAS_GLYPH;
        if entry.is_color {
            flags |= FLAG_COLOR_GLYPH;
        }
        uv_offset = [entry.x as f32, entry.y as f32];
        uv_size = [entry.width as f32, entry.height as f32];
        glyph_left_pad = entry.left_pad as f32;
        glyph_top_pad = entry.top_pad as f32;
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

    let bg_instance = (colors.bg != style.base_bg).then_some(CellInstance {
        pos: [
            ci.col as f32 * style.cell_w,
            ci.row as f32 * style.cell_h + style.viewport_offset_y,
        ],
        size: [style.cell_w * ci.cells as f32, style.cell_h],
        uv_offset: [0.0, 0.0],
        uv_size: [0.0, 0.0],
        fg: [0.0, 0.0, 0.0, 0.0],
        bg,
        deco: [0.0, 0.0, 0.0, 0.0],
        flags: 0,
        _pad: [0; 2],
    });

    let fg_instance = if flags != 0 {
        let glyph_width = glyph_entry
            .map(|entry| entry.width as f32)
            .unwrap_or(style.cell_w * ci.cells as f32);
        Some(CellInstance {
            pos: [
                ci.col as f32 * style.cell_w - glyph_left_pad,
                ci.row as f32 * style.cell_h + style.viewport_offset_y - glyph_top_pad,
            ],
            size: [
                glyph_width.max(style.cell_w * ci.cells as f32 + glyph_left_pad),
                uv_size[1].max(style.cell_h + glyph_top_pad),
            ],
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
        Some(CursorStyle::Bar) => Some(CellInstance {
            pos: [
                ci.col as f32 * style.cell_w,
                ci.row as f32 * style.cell_h + style.viewport_offset_y,
            ],
            size: [style.cell_w, style.cell_h],
            uv_offset: [0.0, 0.0],
            uv_size: [0.0, 0.0],
            fg: style.base_fg_f,
            bg: [0.0, 0.0, 0.0, 0.0],
            deco: [0.0, 0.0, 0.0, 0.0],
            flags: FLAG_CURSOR_BAR,
            _pad: [0; 2],
        }),
        Some(CursorStyle::Underline) => Some(CellInstance {
            pos: [
                ci.col as f32 * style.cell_w,
                ci.row as f32 * style.cell_h + style.viewport_offset_y,
            ],
            size: [style.cell_w, style.cell_h],
            uv_offset: [0.0, 0.0],
            uv_size: [0.0, 0.0],
            fg: style.base_fg_f,
            bg: [0.0, 0.0, 0.0, 0.0],
            deco: [0.0, 0.0, 0.0, 0.0],
            flags: FLAG_CURSOR_UNDERLINE,
            _pad: [0; 2],
        }),
        _ => None,
    };

    (bg_instance, fg_instance, overlay)
}

#[allow(dead_code)]
pub(crate) fn build_text_batches<F>(
    cell_infos: &[CellInfo],
    style: FrameBatchStyle,
    glyph_entry_for: F,
) -> FrameTextBatches
where
    F: FnMut(&CellInfo) -> Option<GlyphAtlasEntry>,
{
    let mut batches = FrameTextBatches::default();
    fill_text_batches(cell_infos, style, &mut batches, glyph_entry_for);
    batches
}

pub(crate) fn fill_text_batches<F>(
    cell_infos: &[CellInfo],
    style: FrameBatchStyle,
    batches: &mut FrameTextBatches,
    mut glyph_entry_for: F,
) where
    F: FnMut(&CellInfo) -> Option<GlyphAtlasEntry>,
{
    batches.bg_instances.clear();
    batches.fg_instances.clear();
    batches.overlay_instances.clear();
    batches.bg_instances.reserve(
        cell_infos
            .len()
            .saturating_sub(batches.bg_instances.capacity()),
    );
    batches.fg_instances.reserve(
        cell_infos
            .len()
            .saturating_sub(batches.fg_instances.capacity()),
    );
    batches
        .overlay_instances
        .reserve(1usize.saturating_sub(batches.overlay_instances.capacity()));

    for ci in cell_infos {
        let glyph_entry = if ci.ch > 0x20 || ci.grapheme.is_some() {
            glyph_entry_for(ci)
        } else {
            None
        };
        let (bg_instance, fg_instance, overlay_instance) =
            build_cell_instances(ci, style, glyph_entry);

        if let Some(bg_instance) = bg_instance {
            batches.bg_instances.push(bg_instance);
        }
        if let Some(fg_instance) = fg_instance {
            batches.fg_instances.push(fg_instance);
        }
        if let Some(overlay_instance) = overlay_instance {
            batches.overlay_instances.push(overlay_instance);
        }
    }
}

#[allow(dead_code)]
pub(crate) fn build_image_instances<F>(
    placements: &[KittyPlacement],
    cell_w: f32,
    cell_h: f32,
    image_rect_for: F,
) -> Vec<ImageInstance>
where
    F: FnMut(&KittyPlacement) -> Option<AtlasImageRect>,
{
    let mut image_instances = Vec::new();
    fill_image_instances(
        placements,
        cell_w,
        cell_h,
        &mut image_instances,
        image_rect_for,
    );
    image_instances
}

pub(crate) fn fill_image_instances<F>(
    placements: &[KittyPlacement],
    cell_w: f32,
    cell_h: f32,
    image_instances: &mut Vec<ImageInstance>,
    image_rect_for: F,
) where
    F: FnMut(&KittyPlacement) -> Option<AtlasImageRect>,
{
    fill_image_instances_with_viewport_offset(
        placements,
        cell_w,
        cell_h,
        0.0,
        image_instances,
        image_rect_for,
    );
}

pub(crate) fn fill_image_instances_with_viewport_offset<F>(
    placements: &[KittyPlacement],
    cell_w: f32,
    cell_h: f32,
    viewport_offset_y: f32,
    image_instances: &mut Vec<ImageInstance>,
    mut image_rect_for: F,
) where
    F: FnMut(&KittyPlacement) -> Option<AtlasImageRect>,
{
    image_instances.clear();
    image_instances.reserve(placements.len().saturating_sub(image_instances.capacity()));
    for placement in placements {
        if let Some(entry) = image_rect_for(placement) {
            let mut instance = image_instance_for_placement(placement, entry, cell_w, cell_h);
            instance.pos[1] += viewport_offset_y;
            image_instances.push(instance);
        }
    }
}

fn cell_span(cell: &crate::grid::Cell) -> usize {
    if cell.flags & crate::grid::FLAG_WIDE != 0 {
        2
    } else {
        1
    }
}

fn rgb_to_f32(rgb: u32) -> [f32; 4] {
    rgb_to_f32_alpha(rgb, 1.0)
}

fn rgb_to_f32_alpha(rgb: u32, alpha: f32) -> [f32; 4] {
    [
        ((rgb >> 16) & 0xff) as f32 / 255.0,
        ((rgb >> 8) & 0xff) as f32 / 255.0,
        (rgb & 0xff) as f32 / 255.0,
        alpha,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::font::GlyphAtlas;
    use crate::grid::{ATTR_STRIKETHROUGH, ATTR_UNDERLINE};
    use crate::render::OffscreenRenderer;
    use crate::terminal::Terminal;
    use crate::visual::{resolve_cell_colors, resolve_underline_color};
    use crate::workloads::{
        EMOJI_AND_SHADE_TRANSCRIPT, FISH_STARTUP_TRANSCRIPT, STARSHIP_PROMPT_TRANSCRIPT,
        TUI_HELP_WITH_IMAGE_TRANSCRIPT,
    };

    fn test_style() -> FrameBatchStyle {
        FrameBatchStyle {
            base_fg: AppConfig::default().style.foreground.as_u32_rgb(),
            base_bg: AppConfig::default().style.background.as_u32_rgb(),
            base_fg_f: rgb_to_f32(AppConfig::default().style.foreground.as_u32_rgb()),
            background_alpha: 1.0,
            cell_w: 8.0,
            cell_h: 16.0,
            viewport_offset_y: 0.0,
        }
    }

    #[test]
    fn viewport_scroll_uses_extra_row_for_fractional_scrollback() {
        let scroll = ViewportScroll::from_scroll_rows(0.25);
        assert_eq!(scroll.sample_offset, 1);
        assert_eq!(scroll.extra_visible_rows(), 1);
        assert!((scroll.viewport_offset_y(16.0) + 12.0).abs() < f32::EPSILON);

        let exact = ViewportScroll::from_scroll_rows(2.0);
        assert_eq!(exact.sample_offset, 2);
        assert_eq!(exact.extra_visible_rows(), 0);
        assert_eq!(exact.viewport_offset_y(16.0), 0.0);
    }

    #[test]
    fn fractional_scrollback_adds_one_visible_row_to_frame_plan() {
        let mut terminal = Terminal::new_with_scrollback(2, 2, 8);
        terminal.process(b"1\r\n2\r\n3\r\n");
        assert!(terminal.grid.scrollback_len() >= 1);

        let mut baseline = Vec::new();
        fill_cell_infos(&terminal, &mut baseline);

        let mut fractional = Vec::new();
        fill_cell_infos_with_scroll(&terminal, &mut fractional, ViewportScroll::from_scroll_rows(0.25));

        assert_eq!(baseline.len(), 4);
        assert_eq!(fractional.len(), 6);
        assert_eq!(fractional[0].row, 0);
        assert_eq!(fractional[4].row, 2);
    }

    fn test_glyph_entry(ci: &CellInfo) -> Option<GlyphAtlasEntry> {
        (ci.ch > 0x20 || ci.grapheme.is_some()).then_some(GlyphAtlasEntry {
            x: (ci.col as u32) * 8,
            y: (ci.row as u32) * 16,
            width: 8 * ci.cells as u32,
            height: 16,
            left_pad: 0,
            top_pad: 0,
            is_color: ci.ch > 0xffff,
        })
    }

    fn assert_batches_match_terminal_visuals(terminal: &Terminal) {
        let style = test_style();
        let mut cell_infos = Vec::new();
        fill_cell_infos(terminal, &mut cell_infos);

        let mut batches = FrameTextBatches::default();
        fill_text_batches(&cell_infos, style, &mut batches, test_glyph_entry);

        let mut expected_bg = Vec::new();
        let mut expected_fg = Vec::new();
        let mut expected_overlay = Vec::new();

        for ci in &cell_infos {
            let colors = resolve_cell_colors(
                &ci.cell,
                style.base_fg,
                style.base_bg,
                ci.is_cursor_block,
                ci.selected,
            );
            let deco = rgb_to_f32(resolve_underline_color(&ci.cell, colors.fg));
            let pos = [ci.col as f32 * style.cell_w, ci.row as f32 * style.cell_h];
            let size = [style.cell_w * ci.cells as f32, style.cell_h];

            if colors.bg != style.base_bg {
                expected_bg.push((
                    pos,
                    size,
                    rgb_to_f32_alpha(colors.bg, style.background_alpha),
                ));
            }

            let mut expected_flags = 0u32;
            if ci.ch > 0x20 || ci.grapheme.is_some() {
                expected_flags |= FLAG_HAS_GLYPH;
                if ci.ch > 0xffff {
                    expected_flags |= FLAG_COLOR_GLYPH;
                }
            }
            if ci.cell.attrs & ATTR_UNDERLINE != 0 {
                use crate::grid::UnderlineStyle;
                match ci.cell.underline_style {
                    UnderlineStyle::None | UnderlineStyle::Single => {
                        expected_flags |= FLAG_UNDERLINE
                    }
                    UnderlineStyle::Double => expected_flags |= FLAG_DOUBLE_UL,
                    UnderlineStyle::Curly => expected_flags |= FLAG_CURLY_UL,
                    UnderlineStyle::Dotted => expected_flags |= FLAG_DOTTED_UL,
                    UnderlineStyle::Dashed => expected_flags |= FLAG_DASHED_UL,
                }
            }
            if ci.cell.attrs & ATTR_STRIKETHROUGH != 0 {
                expected_flags |= FLAG_STRIKETHROUGH;
            }
            if expected_flags != 0 {
                expected_fg.push((pos, size, rgb_to_f32(colors.fg), deco, expected_flags));
            }

            let overlay_flags = match ci.cursor_style {
                Some(CursorStyle::Bar) => Some(FLAG_CURSOR_BAR),
                Some(CursorStyle::Underline) => Some(FLAG_CURSOR_UNDERLINE),
                _ => None,
            };
            if let Some(flags) = overlay_flags {
                expected_overlay.push((pos, [style.cell_w, style.cell_h], style.base_fg_f, flags));
            }
        }

        assert_eq!(
            batches.bg_instances.len(),
            expected_bg.len(),
            "background batch count diverged"
        );
        for (instance, (pos, size, bg)) in batches.bg_instances.iter().zip(expected_bg.iter()) {
            assert_eq!(instance.pos, *pos);
            assert_eq!(instance.size, *size);
            assert_eq!(instance.bg, *bg);
            assert_eq!(instance.flags, 0);
        }

        assert_eq!(
            batches.fg_instances.len(),
            expected_fg.len(),
            "foreground batch count diverged"
        );
        for (instance, (pos, size, fg, deco, flags)) in
            batches.fg_instances.iter().zip(expected_fg.iter())
        {
            assert_eq!(instance.pos, *pos);
            assert_eq!(instance.size, *size);
            assert_eq!(instance.fg, *fg);
            assert_eq!(instance.deco, *deco);
            assert_eq!(instance.flags, *flags);
        }

        assert_eq!(
            batches.overlay_instances.len(),
            expected_overlay.len(),
            "overlay batch count diverged"
        );
        for (instance, (pos, size, fg, flags)) in batches
            .overlay_instances
            .iter()
            .zip(expected_overlay.iter())
        {
            assert_eq!(instance.pos, *pos);
            assert_eq!(instance.size, *size);
            assert_eq!(instance.fg, *fg);
            assert_eq!(instance.flags, *flags);
        }
    }

    fn assert_images_match_terminal_placements(terminal: &Terminal) {
        let mut image_instances = Vec::new();
        fill_image_instances(
            &terminal.kitty_placements,
            8.0,
            16.0,
            &mut image_instances,
            |placement| {
                Some(AtlasImageRect {
                    x: placement.image_id * 10,
                    y: placement.image_id * 20,
                    width: placement.cols.max(1) as u32 * 8,
                    height: placement.rows.max(1) as u32 * 16,
                })
            },
        );

        assert_eq!(image_instances.len(), terminal.kitty_placements.len());
        for (instance, placement) in image_instances.iter().zip(terminal.kitty_placements.iter()) {
            assert_eq!(
                instance.pos,
                [placement.col as f32 * 8.0, placement.row as f32 * 16.0]
            );
            assert_eq!(
                instance.size,
                [
                    placement.cols.max(1) as f32 * 8.0,
                    placement.rows.max(1) as f32 * 16.0
                ]
            );
            assert_eq!(
                instance.uv_size,
                [
                    placement.cols.max(1) as f32 * 8.0,
                    placement.rows.max(1) as f32 * 16.0
                ]
            );
        }
    }

    fn image_instances_for_terminal(terminal: &Terminal) -> Vec<ImageInstance> {
        build_image_instances(terminal.kitty_placements(), 8.0, 16.0, |placement| {
            let image = terminal.kitty_image(placement.image_id)?;
            if image.data.len() != (image.width as usize) * (image.height as usize) * 4 {
                return None;
            }
            Some(AtlasImageRect {
                x: 0,
                y: 0,
                width: image.width,
                height: image.height,
            })
        })
    }

    fn new_atlas(config: &AppConfig) -> GlyphAtlas {
        GlyphAtlas::new(config.style.font_size)
            .expect("should load a monospace font for GPU parity tests")
    }

    fn cpu_cell_is_visibly_non_default(
        renderer: &OffscreenRenderer,
        col: usize,
        row: usize,
        cell_w: usize,
        cell_h: usize,
        base_bg: u32,
    ) -> bool {
        let px_x = col * cell_w;
        let px_y = row * cell_h;
        let x_end = (px_x + cell_w).min(renderer.width);
        let y_end = (px_y + cell_h).min(renderer.height);

        for y in px_y..y_end {
            let row_start = y * renderer.width;
            for x in px_x..x_end {
                if renderer.pixels[row_start + x] != base_bg {
                    return true;
                }
            }
        }
        false
    }

    fn mark_cells_for_pixel_rect(
        visible: &mut [bool],
        grid_size: (usize, usize),
        rect: (i32, i32, usize, usize),
        cell_size: (usize, usize),
    ) {
        let (cols, rows) = grid_size;
        let (left, top, width, height) = rect;
        let (cell_w, cell_h) = cell_size;
        if width == 0 || height == 0 {
            return;
        }

        let right = left.saturating_add(width as i32);
        let bottom = top.saturating_add(height as i32);
        if right <= 0 || bottom <= 0 {
            return;
        }

        let start_col = (left.max(0) as usize) / cell_w.max(1);
        let start_row = (top.max(0) as usize) / cell_h.max(1);
        let end_col = ((right.max(0) as usize).saturating_sub(1) / cell_w.max(1) + 1).min(cols);
        let end_row = ((bottom.max(0) as usize).saturating_sub(1) / cell_h.max(1) + 1).min(rows);

        for row in start_row.min(rows)..end_row {
            let row_offset = row * cols;
            for col in start_col.min(cols)..end_col {
                visible[row_offset + col] = true;
            }
        }
    }

    fn assert_gpu_visible_cells_match_cpu_framebuffer(terminal: &mut Terminal) {
        let config = AppConfig::default();
        let mut atlas = new_atlas(&config);
        let mut renderer = OffscreenRenderer::new(terminal.cols, terminal.rows, &atlas);
        renderer.render(terminal, &mut atlas, &config);

        let style = FrameBatchStyle {
            base_fg: config.style.foreground.as_u32_rgb(),
            base_bg: config.style.background.as_u32_rgb(),
            base_fg_f: rgb_to_f32(config.style.foreground.as_u32_rgb()),
            background_alpha: 1.0,
            cell_w: atlas.cell_width as f32,
            cell_h: atlas.cell_height as f32,
            viewport_offset_y: 0.0,
        };

        let mut cell_infos = Vec::new();
        fill_cell_infos(terminal, &mut cell_infos);

        let cols = terminal.cols as usize;
        let rows = terminal.rows as usize;
        let mut gpu_visible = vec![false; cols * rows];

        for ci in &cell_infos {
            let colors = resolve_cell_colors(
                &ci.cell,
                style.base_fg,
                style.base_bg,
                ci.is_cursor_block,
                ci.selected,
            );
            if colors.bg != style.base_bg {
                mark_cells_for_pixel_rect(
                    &mut gpu_visible,
                    (cols, rows),
                    (
                        (ci.col * atlas.cell_width) as i32,
                        (ci.row * atlas.cell_height) as i32,
                        atlas.cell_width * ci.cells.max(1),
                        atlas.cell_height,
                    ),
                    (atlas.cell_width, atlas.cell_height),
                );
            }

            if ci.ch > 0x20 || ci.grapheme.is_some() {
                let glyph = if let Some(grapheme) = ci.grapheme.as_deref() {
                    atlas.ensure_grapheme(grapheme);
                    atlas.get_grapheme_glyph(grapheme)
                } else {
                    atlas.ensure_glyph(ci.ch);
                    atlas.get_glyph(ci.ch)
                };

                if let Some(glyph) = glyph {
                    let px_x = ci.col * atlas.cell_width;
                    let px_y = ci.row * atlas.cell_height;
                    let origin_y = px_y as i32 + (atlas.cell_height as i32 - atlas.baseline as i32);
                    let glyph_top = origin_y - glyph.bearing_y;
                    let glyph_left = px_x as i32 + glyph.bearing_x;
                    mark_cells_for_pixel_rect(
                        &mut gpu_visible,
                        (cols, rows),
                        (glyph_left, glyph_top, glyph.width, glyph.height),
                        (atlas.cell_width, atlas.cell_height),
                    );
                }
            }

            if ci.cell.attrs & (ATTR_UNDERLINE | ATTR_STRIKETHROUGH) != 0 {
                mark_cells_for_pixel_rect(
                    &mut gpu_visible,
                    (cols, rows),
                    (
                        (ci.col * atlas.cell_width) as i32,
                        (ci.row * atlas.cell_height) as i32,
                        atlas.cell_width * ci.cells.max(1),
                        atlas.cell_height,
                    ),
                    (atlas.cell_width, atlas.cell_height),
                );
            }

            if ci.cursor_style.is_some() {
                mark_cells_for_pixel_rect(
                    &mut gpu_visible,
                    (cols, rows),
                    (
                        (ci.col * atlas.cell_width) as i32,
                        (ci.row * atlas.cell_height) as i32,
                        atlas.cell_width,
                        atlas.cell_height,
                    ),
                    (atlas.cell_width, atlas.cell_height),
                );
            }
        }

        for placement in terminal.kitty_placements() {
            mark_cells_for_pixel_rect(
                &mut gpu_visible,
                (cols, rows),
                (
                    (placement.col * atlas.cell_width) as i32,
                    (placement.row * atlas.cell_height) as i32,
                    placement.cols.max(1) * atlas.cell_width,
                    placement.rows.max(1) * atlas.cell_height,
                ),
                (atlas.cell_width, atlas.cell_height),
            );
        }

        for row in 0..terminal.rows as usize {
            for col in 0..terminal.cols as usize {
                let gpu_visible = gpu_visible[row * cols + col];

                let cpu_visible = cpu_cell_is_visibly_non_default(
                    &renderer,
                    col,
                    row,
                    atlas.cell_width,
                    atlas.cell_height,
                    style.base_bg,
                );

                assert_eq!(
                    gpu_visible,
                    cpu_visible,
                    "GPU/CPU visible-cell parity diverged at row={} col={} char={:?} grapheme={:?}",
                    row,
                    col,
                    terminal.grid.cell_char(row, col),
                    terminal.grid.cell_grapheme_at(row, col),
                );
            }
        }
    }

    #[test]
    fn kitty_placement_maps_to_image_instance_geometry() {
        let placement = KittyPlacement {
            image_id: 7,
            col: 2,
            row: 1,
            cols: 3,
            rows: 2,
        };
        let entry = AtlasImageRect {
            x: 10,
            y: 20,
            width: 30,
            height: 40,
        };

        let instance = image_instance_for_placement(&placement, entry, 8.0, 16.0);

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
        terminal.cursor_style = CursorStyle::Bar;
        terminal.grid.set_cursor(0, 0);

        let plan = build_frame_plan(&terminal);
        assert_eq!(plan.cell_infos.len(), 7);

        let ascii = plan
            .cell_infos
            .iter()
            .find(|ci| ci.row == 0 && ci.col == 0)
            .expect("ascii cell should exist");
        assert_eq!(ascii.cells, 1);
        assert_eq!(ascii.cursor_style, Some(CursorStyle::Bar));

        let wide = plan
            .cell_infos
            .iter()
            .find(|ci| ci.row == 0 && ci.col == 1)
            .expect("wide cell should exist");
        assert_eq!(wide.cells, 2);
        assert!(wide.selected);
    }

    #[test]
    fn frame_plan_carries_tui_help_overlay_and_kitty_images() {
        let mut terminal = Terminal::new(6, 3);
        terminal.process(b"\x1b[?1049hstart\r\nready\r\n");
        terminal.process(b"\x1b_Ga=T,i=5,f=32,s=1,v=1,c=2,r=1;+////wAAAP8=\x1b\\");
        terminal.process(b"\x1b[H/help\r\nhelp text\r\n");

        let plan = build_frame_plan(&terminal);

        assert_eq!(plan.image_placements.len(), 1);
        assert!(plan.cell_infos.iter().any(|ci| ci.row == 0 && ci.ch > 0x20));
        assert!(plan.cell_infos.iter().any(|ci| ci.row == 1 && ci.ch > 0x20));
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
            grapheme: None,
            cells: 1,
            cell,
            selected: true,
            is_cursor_block: false,
            cursor_style: None,
        };

        let (bg, fg, overlay) = build_cell_instances(
            &ci,
            FrameBatchStyle {
                base_fg: 0xffffff,
                base_bg: 0x000000,
                base_fg_f: [1.0, 1.0, 1.0, 1.0],
                background_alpha: 1.0,
                cell_w: 8.0,
                cell_h: 16.0,
                viewport_offset_y: 0.0,
            },
            None,
        );

        let bg = bg.expect("selected cell should emit a background instance");
        assert_eq!(bg.bg, rgb_to_f32(crate::color::to_rgb(2)));
        assert_eq!(bg.size, [8.0, 16.0]);
        let fg = fg.expect("underline pass should exist");
        assert_eq!(fg.deco, rgb_to_f32(0x00ff00));
        assert!(fg.flags & FLAG_UNDERLINE != 0);
        assert!(overlay.is_none());
    }

    #[test]
    fn build_cell_instances_marks_color_glyphs_and_cursor_overlay() {
        let ci = CellInfo {
            row: 1,
            col: 2,
            ch: 0x1f600,
            grapheme: None,
            cells: 2,
            cell: crate::grid::Cell::BLANK,
            selected: false,
            is_cursor_block: false,
            cursor_style: Some(CursorStyle::Bar),
        };

        let (_, fg, overlay) = build_cell_instances(
            &ci,
            FrameBatchStyle {
                base_fg: 0xffffff,
                base_bg: 0x000000,
                base_fg_f: [0.25, 0.5, 0.75, 1.0],
                background_alpha: 1.0,
                cell_w: 8.0,
                cell_h: 16.0,
                viewport_offset_y: 0.0,
            },
            Some(GlyphAtlasEntry {
                x: 32,
                y: 64,
                width: 16,
                height: 16,
                left_pad: 0,
                top_pad: 0,
                is_color: true,
            }),
        );

        let fg = fg.expect("glyph pass should exist");
        assert!(fg.flags & FLAG_HAS_GLYPH != 0);
        assert!(fg.flags & FLAG_COLOR_GLYPH != 0);

        let overlay = overlay.expect("bar cursor should render as overlay");
        assert_eq!(overlay.pos, [16.0, 16.0]);
        assert_eq!(overlay.size, [8.0, 16.0]);
        assert!(overlay.flags & FLAG_CURSOR_BAR != 0);
    }

    #[test]
    fn build_text_batches_separates_background_foreground_and_overlay() {
        let mut terminal = Terminal::new(4, 2);
        terminal.process("ab".as_bytes());
        terminal.cursor_style = CursorStyle::Underline;
        terminal.grid.set_cursor(0, 0);
        let plan = build_frame_plan(&terminal);

        let batches = build_text_batches(
            &plan.cell_infos,
            FrameBatchStyle {
                base_fg: 0xffffff,
                base_bg: 0x000000,
                base_fg_f: [1.0, 1.0, 1.0, 1.0],
                background_alpha: 1.0,
                cell_w: 8.0,
                cell_h: 16.0,
                viewport_offset_y: 0.0,
            },
            |_ci| {
                Some(GlyphAtlasEntry {
                    x: 0,
                    y: 0,
                    width: 8,
                    height: 16,
                    left_pad: 0,
                    top_pad: 0,
                    is_color: false,
                })
            },
        );

        assert_eq!(batches.bg_instances.len(), 0);
        assert_eq!(batches.fg_instances.len(), 2);
        assert_eq!(batches.overlay_instances.len(), 1);
        assert!(batches.overlay_instances[0].flags & FLAG_CURSOR_UNDERLINE != 0);
    }

    #[test]
    fn grapheme_clusters_flow_through_frame_plan_and_batches() {
        let mut terminal = Terminal::new(8, 2);
        terminal.process("❤️ 👨‍💻".as_bytes());

        let plan = build_frame_plan(&terminal);
        let heart = plan
            .cell_infos
            .iter()
            .find(|ci| ci.row == 0 && ci.col == 0)
            .expect("heart cell should exist");
        assert_eq!(heart.grapheme.as_deref(), Some("❤️"));
        assert_eq!(heart.cells, 2);

        let coder = plan
            .cell_infos
            .iter()
            .find(|ci| ci.row == 0 && ci.col == 3)
            .expect("coder cell should exist");
        assert_eq!(coder.grapheme.as_deref(), Some("👨‍💻"));
        assert_eq!(coder.cells, 2);

        let batches = build_text_batches(&plan.cell_infos, test_style(), test_glyph_entry);
        assert!(
            batches
                .fg_instances
                .iter()
                .any(|instance| instance.flags & FLAG_HAS_GLYPH != 0 && instance.pos == [0.0, 0.0]),
            "heart grapheme should produce a glyph instance"
        );
        assert!(
            batches.fg_instances.iter().any(
                |instance| instance.flags & FLAG_HAS_GLYPH != 0 && instance.pos == [24.0, 0.0]
            ),
            "coder grapheme should produce a glyph instance"
        );
    }

    #[test]
    fn default_background_cells_skip_background_instances() {
        let ci = CellInfo {
            row: 0,
            col: 0,
            ch: 'a' as u32,
            grapheme: None,
            cells: 1,
            cell: crate::grid::Cell::BLANK,
            selected: false,
            is_cursor_block: false,
            cursor_style: None,
        };

        let (bg, _fg, _overlay) = build_cell_instances(
            &ci,
            FrameBatchStyle {
                base_fg: 0xffffff,
                base_bg: 0x000000,
                base_fg_f: [1.0, 1.0, 1.0, 1.0],
                background_alpha: 1.0,
                cell_w: 8.0,
                cell_h: 16.0,
                viewport_offset_y: 0.0,
            },
            Some(GlyphAtlasEntry {
                x: 0,
                y: 0,
                width: 8,
                height: 16,
                left_pad: 0,
                top_pad: 0,
                is_color: false,
            }),
        );

        assert!(bg.is_none());
    }

    #[test]
    fn glyph_instances_expand_to_uploaded_glyph_width() {
        let ci = CellInfo {
            row: 0,
            col: 0,
            ch: 'x' as u32,
            grapheme: None,
            cells: 1,
            cell: crate::grid::Cell::from_snapshot(crate::grid::CellSnapshot {
                ch: 'x' as u32,
                grapheme: None,
                fg: 0xffffff,
                bg: 0x000000,
                underline_color: 0,
                hyperlink_id: 0,
                attrs: 0,
                flags: 0,
                underline_style: crate::grid::UnderlineStyle::None,
            }),
            selected: false,
            is_cursor_block: false,
            cursor_style: None,
        };

        let (_, fg, _) = build_cell_instances(
            &ci,
            FrameBatchStyle {
                base_fg: 0xffffff,
                base_bg: 0x000000,
                base_fg_f: [1.0, 1.0, 1.0, 1.0],
                background_alpha: 1.0,
                cell_w: 8.0,
                cell_h: 16.0,
                viewport_offset_y: 0.0,
            },
            Some(GlyphAtlasEntry {
                x: 0,
                y: 0,
                width: 13,
                height: 16,
                left_pad: 0,
                top_pad: 0,
                is_color: false,
            }),
        );

        assert_eq!(fg.expect("glyph instance should exist").size, [13.0, 16.0]);
    }

    #[test]
    fn glyph_instances_expand_left_for_left_pad() {
        let ci = CellInfo {
            row: 0,
            col: 2,
            ch: 'x' as u32,
            grapheme: None,
            cells: 1,
            cell: crate::grid::Cell::BLANK,
            selected: false,
            is_cursor_block: false,
            cursor_style: None,
        };

        let (_, fg, _) = build_cell_instances(
            &ci,
            FrameBatchStyle {
                base_fg: 0xffffff,
                base_bg: 0x000000,
                base_fg_f: [1.0, 1.0, 1.0, 1.0],
                background_alpha: 1.0,
                cell_w: 8.0,
                cell_h: 16.0,
                viewport_offset_y: 0.0,
            },
            Some(GlyphAtlasEntry {
                x: 0,
                y: 0,
                width: 9,
                height: 16,
                left_pad: 1,
                top_pad: 0,
                is_color: false,
            }),
        );

        let fg = fg.expect("glyph instance should exist");
        assert_eq!(fg.pos, [15.0, 0.0]);
        assert_eq!(fg.size, [9.0, 16.0]);
    }

    #[test]
    fn explicit_background_instances_stay_opaque_with_window_opacity() {
        let mut cell = crate::grid::Cell::BLANK;
        cell.bg = 0x112233;
        let ci = CellInfo {
            row: 0,
            col: 0,
            ch: ' ' as u32,
            grapheme: None,
            cells: 1,
            cell,
            selected: false,
            is_cursor_block: false,
            cursor_style: None,
        };

        let (bg, _, _) = build_cell_instances(
            &ci,
            FrameBatchStyle {
                base_fg: 0xffffff,
                base_bg: 0x000000,
                base_fg_f: [1.0, 1.0, 1.0, 1.0],
                background_alpha: 0.9,
                cell_w: 8.0,
                cell_h: 16.0,
                viewport_offset_y: 0.0,
            },
            None,
        );

        assert_eq!(bg.expect("background instance should exist").bg[3], 1.0);
    }

    #[test]
    fn build_image_instances_only_emits_resolved_images() {
        let placements = vec![
            KittyPlacement {
                image_id: 1,
                col: 0,
                row: 0,
                cols: 1,
                rows: 1,
            },
            KittyPlacement {
                image_id: 2,
                col: 2,
                row: 1,
                cols: 2,
                rows: 1,
            },
        ];

        let images = build_image_instances(&placements, 8.0, 16.0, |placement| {
            if placement.image_id == 2 {
                Some(AtlasImageRect {
                    x: 10,
                    y: 20,
                    width: 30,
                    height: 40,
                })
            } else {
                None
            }
        });

        assert_eq!(images.len(), 1);
        assert_eq!(images[0].pos, [16.0, 16.0]);
        assert_eq!(images[0].size, [16.0, 16.0]);
    }

    #[test]
    fn fish_startup_transcript_batches_match_visual_expectations() {
        let mut terminal = Terminal::new(80, 24);

        for chunk in FISH_STARTUP_TRANSCRIPT {
            terminal.process(chunk);
            assert_batches_match_terminal_visuals(&terminal);
        }
    }

    #[test]
    fn starship_prompt_transcript_batches_match_visual_expectations() {
        let mut terminal = Terminal::new(80, 24);

        for chunk in STARSHIP_PROMPT_TRANSCRIPT {
            terminal.process(chunk);
            assert_batches_match_terminal_visuals(&terminal);
        }
    }

    #[test]
    fn tui_help_overlay_transcript_batches_match_visuals_and_images() {
        let mut terminal = Terminal::new(32, 8);

        for chunk in TUI_HELP_WITH_IMAGE_TRANSCRIPT {
            terminal.process(chunk);
            assert_batches_match_terminal_visuals(&terminal);
            assert_images_match_terminal_placements(&terminal);
        }
    }

    #[test]
    fn kitty_image_clear_screen_drops_gpu_image_instances() {
        let mut terminal = Terminal::new(8, 4);
        terminal.process(b"\x1b_Ga=T,i=7,f=32,s=1,v=1,c=2,r=1;/wAA/w==\x1b\\");

        let plan = build_frame_plan(&terminal);
        assert_eq!(plan.image_placements.len(), 1);
        let images = image_instances_for_terminal(&terminal);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].pos, [0.0, 0.0]);
        assert_eq!(images[0].size, [16.0, 16.0]);

        terminal.process(b"\x1b[2J");
        let plan = build_frame_plan(&terminal);
        assert!(plan.image_placements.is_empty());
        assert!(image_instances_for_terminal(&terminal).is_empty());
    }

    #[test]
    fn kitty_image_alt_screen_hides_and_restores_gpu_instances() {
        let mut terminal = Terminal::new(8, 4);
        terminal.process(b"\x1b_Ga=T,i=7,f=32,s=1,v=1,c=1,r=1;/wAA/w==\x1b\\");

        assert_eq!(build_frame_plan(&terminal).image_placements.len(), 1);
        assert_eq!(image_instances_for_terminal(&terminal).len(), 1);

        terminal.process(b"\x1b[?1049h");
        assert!(build_frame_plan(&terminal).image_placements.is_empty());
        assert!(image_instances_for_terminal(&terminal).is_empty());

        terminal.process(b"\x1b[?1049l");
        let plan = build_frame_plan(&terminal);
        assert_eq!(plan.image_placements.len(), 1);
        let images = image_instances_for_terminal(&terminal);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].pos, [0.0, 0.0]);
        assert_eq!(images[0].size, [8.0, 16.0]);
    }

    #[test]
    fn kitty_image_delete_drops_gpu_instances_and_matches_cpu() {
        let mut terminal = Terminal::new(8, 4);
        terminal.process(b"\x1b_Ga=T,i=7,f=32,s=1,v=1,c=1,r=1;/wAA/w==\x1b\\");
        assert_gpu_visible_cells_match_cpu_framebuffer(&mut terminal);
        assert_eq!(image_instances_for_terminal(&terminal).len(), 1);

        terminal.process(b"\x1b_Ga=d,i=7\x1b\\");
        assert!(build_frame_plan(&terminal).image_placements.is_empty());
        assert!(image_instances_for_terminal(&terminal).is_empty());
        assert_gpu_visible_cells_match_cpu_framebuffer(&mut terminal);
    }

    #[test]
    fn kitty_image_alt_screen_visibility_matches_cpu_render() {
        let mut terminal = Terminal::new(8, 4);
        terminal.process(b"\x1b_Ga=T,i=7,f=32,s=1,v=1,c=1,r=1;/wAA/w==\x1b\\");
        assert_gpu_visible_cells_match_cpu_framebuffer(&mut terminal);

        terminal.process(b"\x1b[?1049h");
        assert_gpu_visible_cells_match_cpu_framebuffer(&mut terminal);

        terminal.process(b"\x1b[?1049l");
        assert_gpu_visible_cells_match_cpu_framebuffer(&mut terminal);
    }

    #[test]
    fn emoji_and_shade_transcript_batches_match_visual_expectations() {
        let mut terminal = Terminal::new(16, 4);

        for chunk in EMOJI_AND_SHADE_TRANSCRIPT {
            terminal.process(chunk);
            assert_batches_match_terminal_visuals(&terminal);
        }

        let plan = build_frame_plan(&terminal);
        assert!(
            plan.cell_infos
                .iter()
                .any(|ci| ci.grapheme.as_deref() == Some("❤️"))
        );
        assert!(
            plan.cell_infos
                .iter()
                .any(|ci| ci.grapheme.as_deref() == Some("👨‍💻"))
        );
        assert!(plan.cell_infos.iter().any(|ci| ci.ch == '░' as u32));
    }

    #[test]
    fn emoji_and_shade_transcript_gpu_visible_cells_match_cpu_render() {
        let mut terminal = Terminal::new(16, 4);

        for chunk in EMOJI_AND_SHADE_TRANSCRIPT {
            terminal.process(chunk);
            assert_gpu_visible_cells_match_cpu_framebuffer(&mut terminal);
        }
    }
}
