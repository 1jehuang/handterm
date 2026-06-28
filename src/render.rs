use crate::color::blend_rgba_over_rgb;
use crate::config::AppConfig;
use crate::font::GlyphAtlas;
use crate::frontend::{VisualState, compute_scrollbar_geometry, sync_visual_damage};
use crate::grid::{COLOR_DEFAULT, Cell};
use crate::terminal::{CursorStyle, TerminalView};
use crate::visual::{is_in_selection, resolve_cell_colors, resolve_underline_color};

#[cfg_attr(not(test), allow(dead_code))]
pub struct OffscreenRenderer {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u32>,
    last_visual_state: Option<VisualState>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl OffscreenRenderer {
    pub fn new(cols: u16, rows: u16, atlas: &GlyphAtlas) -> Self {
        let width = cols as usize * atlas.cell_width;
        let height = rows as usize * atlas.cell_height;
        Self::new_for_pixels(width, height)
    }

    pub fn new_for_pixels(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; width * height],
            last_visual_state: None,
        }
    }

    pub fn reset(&mut self) {
        self.pixels.fill(0);
        self.last_visual_state = None;
    }

    pub fn resize_pixels(&mut self, width: usize, height: usize) {
        if self.width == width && self.height == height {
            return;
        }
        self.width = width;
        self.height = height;
        self.pixels.resize(width * height, 0);
        self.reset();
    }

    pub fn render(
        &mut self,
        terminal: &mut impl TerminalView,
        atlas: &mut GlyphAtlas,
        config: &AppConfig,
    ) {
        render_terminal_to_buffer(
            &mut self.pixels,
            self.width,
            self.height,
            terminal,
            atlas,
            config,
            &mut self.last_visual_state,
        );
    }
}

pub fn render_terminal_to_buffer(
    buffer: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    terminal: &mut impl TerminalView,
    atlas: &mut GlyphAtlas,
    config: &AppConfig,
    last_visual_state: &mut Option<VisualState>,
) {
    let current_visual = VisualState::capture(terminal);
    sync_visual_damage(terminal.grid_mut(), *last_visual_state, current_visual);

    let base_bg = config.style.background.as_u32_rgb();
    let base_fg = config.style.foreground.as_u32_rgb();

    let grid = terminal.grid();
    let has_selection = grid.selection.is_some();
    let scrolled = grid.scroll_offset > 0;
    let full_redraw = grid.all_dirty || has_selection || scrolled || has_complex_dirty_cells(grid);
    if full_redraw {
        buffer.fill(base_bg);
    }

    let (cursor_col, cursor_row) = grid.cursor_pos();
    let show_cursor = terminal.cursor_visible() && grid.scroll_offset == 0;
    let cursor_style = terminal.cursor_style();
    let selection = grid.selection;
    let cell_w = atlas.cell_width;
    let cell_h = atlas.cell_height;

    for row in 0..grid.rows {
        let row_dirty = full_redraw || grid.row_has_dirty_cells(row);
        if !row_dirty && !(show_cursor && row == cursor_row) {
            continue;
        }
        for col in 0..grid.cols {
            let is_cursor = show_cursor && row == cursor_row && col == cursor_col;

            if !full_redraw && !is_cursor && !grid.is_cell_dirty(row, col) {
                continue;
            }

            let cell = grid.cell_at_scroll(row, col);

            if !full_redraw {
                let has_custom_bg = cell.bg != COLOR_DEFAULT;
                let is_dirty = grid.is_cell_dirty(row, col);
                if !is_cursor && !has_custom_bg && !is_dirty {
                    continue;
                }
            }

            let is_cursor_block = is_cursor && cursor_style == CursorStyle::Block;

            // Fast path: on a full redraw the whole buffer was already cleared to
            // `base_bg`, and a default-background cell with no inverse attribute,
            // no selection, and no block cursor resolves to exactly `base_bg`. Such
            // cells need no work at all, so skip the color resolve, selection test,
            // and fill. This is the overwhelmingly common case for typical content.
            if full_redraw
                && !is_cursor_block
                && selection.is_none()
                && cell.bg == COLOR_DEFAULT
                && cell.attrs & crate::grid::ATTR_INVERSE == 0
            {
                continue;
            }

            let selected = is_in_selection(selection, row, col);
            let colors = resolve_cell_colors(cell, base_fg, base_bg, is_cursor_block, selected);

            // For cells that survive the fast path on a full redraw, only paint a
            // background that actually differs from the already-cleared `base_bg`.
            // For incremental redraws the buffer persists between frames, so dirty
            // cells must always be repainted to clear their previous contents.
            if !full_redraw || colors.bg != base_bg {
                atlas.draw_bg(buffer, buf_w, buf_h, col, row, colors.bg);
            }
        }
    }

    draw_kitty_images(buffer, buf_w, buf_h, terminal, cell_w, cell_h);

    #[cfg(feature = "ligatures")]
    {
        let mut run_text = String::with_capacity(grid.cols);
        let mut run_start: usize = 0;
        let mut run_fg: u32 = 0;
        let mut run_attrs: u8 = 0;

        for row in 0..grid.rows {
            let any_dirty =
                full_redraw || grid.row_has_dirty_cells(row) || (show_cursor && row == cursor_row);
            if !any_dirty {
                continue;
            }
            let row_redraw_all = !full_redraw;

            if row_redraw_all {
                for col in 0..grid.cols {
                    let is_cursor_here = show_cursor && row == cursor_row && col == cursor_col;
                    let is_cursor_block = is_cursor_here && cursor_style == CursorStyle::Block;
                    let selected = is_in_selection(selection, row, col);
                    let cell = grid.cell_at_scroll(row, col);
                    let colors =
                        resolve_cell_colors(cell, base_fg, base_bg, is_cursor_block, selected);
                    atlas.draw_bg(buffer, buf_w, buf_h, col, row, colors.bg);
                }
            }

            run_text.clear();

            let flush_run = |atlas: &mut GlyphAtlas,
                             buffer: &mut [u32],
                             buf_w: usize,
                             buf_h: usize,
                             text: &str,
                             start_col: usize,
                             row: usize,
                             fg: u32| {
                if text.is_empty() {
                    return;
                }
                if text.is_ascii() {
                    let shaped = atlas.shape_run(text);
                    let char_count = text.chars().count();
                    let is_ligature = shaped.len() < char_count;

                    if is_ligature {
                        let mut col = start_col;
                        for sg in &shaped {
                            atlas.draw_shaped_glyph(
                                buffer,
                                buf_w,
                                buf_h,
                                col,
                                row,
                                sg.codepoint,
                                sg.cells,
                                fg,
                            );
                            col += sg.cells;
                        }
                        return;
                    }
                }

                let mut col = start_col;
                for ch in text.chars() {
                    if ch as u32 > 0x20 {
                        atlas.draw_glyph(buffer, buf_w, buf_h, col, row, ch as u32, fg);
                    }
                    let w = unicode_width::UnicodeWidthChar::width(ch)
                        .unwrap_or(1)
                        .max(1);
                    col += w;
                }
            };

            for col in 0..grid.cols {
                let cell = grid.cell_at_scroll(row, col);
                if cell.flags & crate::grid::FLAG_WIDE_CONT != 0 {
                    continue;
                }

                let is_cursor_here = show_cursor && row == cursor_row && col == cursor_col;
                let is_cursor_block = is_cursor_here && cursor_style == CursorStyle::Block;
                let selected = is_in_selection(selection, row, col);
                let colors = resolve_cell_colors(cell, base_fg, base_bg, is_cursor_block, selected);
                let grapheme = grid.cell_grapheme_at_scroll(row, col);
                let has_content = cell.ch > 0x20 || grapheme.is_some();

                let same_run = has_content
                    && grapheme.is_none()
                    && !run_text.is_empty()
                    && colors.fg == run_fg
                    && cell.attrs == run_attrs;

                if !same_run && !run_text.is_empty() {
                    flush_run(
                        atlas, buffer, buf_w, buf_h, &run_text, run_start, row, run_fg,
                    );
                    run_text.clear();
                }

                if let Some(grapheme) = grapheme {
                    atlas.draw_grapheme(buffer, buf_w, buf_h, col, row, grapheme, colors.fg);
                } else if has_content {
                    if run_text.is_empty() {
                        run_start = col;
                        run_fg = colors.fg;
                        run_attrs = cell.attrs;
                    }
                    if let Some(ch) = char::from_u32(cell.ch) {
                        run_text.push(ch);
                    }
                }

                if is_cursor_here && cursor_style != CursorStyle::Block {
                    draw_cursor(
                        buffer,
                        (buf_w, buf_h),
                        CellRect::from_cell(col, row, cell_w, cell_h),
                        cursor_style,
                        base_fg,
                    );
                }
            }

            if !run_text.is_empty() {
                flush_run(
                    atlas, buffer, buf_w, buf_h, &run_text, run_start, row, run_fg,
                );
            }

            for col in 0..grid.cols {
                let cell = grid.cell_at_scroll(row, col);
                if cell.attrs == 0 {
                    continue;
                }
                if !full_redraw && !row_redraw_all && !grid.is_cell_dirty(row, col) {
                    continue;
                }
                let is_cursor_here = show_cursor && row == cursor_row && col == cursor_col;
                let is_cursor_block = is_cursor_here && cursor_style == CursorStyle::Block;
                let selected = is_in_selection(selection, row, col);
                let colors = resolve_cell_colors(cell, base_fg, base_bg, is_cursor_block, selected);

                draw_text_decorations(
                    buffer,
                    (buf_w, buf_h),
                    CellRect::from_cell(col, row, cell_w, cell_h),
                    cell,
                    colors.fg,
                );
            }
        }
    }

    #[cfg(not(feature = "ligatures"))]
    {
        for row in 0..grid.rows {
            let row_dirty = full_redraw || grid.row_has_dirty_cells(row);
            if !row_dirty && !(show_cursor && row == cursor_row) {
                continue;
            }
            for col in 0..grid.cols {
                let is_cursor = show_cursor && row == cursor_row && col == cursor_col;

                if !full_redraw && !is_cursor && !grid.is_cell_dirty(row, col) {
                    continue;
                }

                let cell = grid.cell_at_scroll(row, col);

                if cell.flags & crate::grid::FLAG_WIDE_CONT != 0 && !is_cursor {
                    continue;
                }

                let grapheme = grid.cell_grapheme_at_scroll(row, col);
                let has_content = cell.ch > 0x20 || grapheme.is_some();
                let is_cursor_block = is_cursor && cursor_style == CursorStyle::Block;
                let selected = is_in_selection(selection, row, col);
                let colors = resolve_cell_colors(cell, base_fg, base_bg, is_cursor_block, selected);

                if let Some(grapheme) = grapheme {
                    atlas.draw_grapheme(buffer, buf_w, buf_h, col, row, grapheme, colors.fg);
                } else if has_content {
                    atlas.draw_glyph(buffer, buf_w, buf_h, col, row, cell.ch, colors.fg);
                }

                if cell.attrs != 0 {
                    draw_text_decorations(
                        buffer,
                        (buf_w, buf_h),
                        CellRect::from_cell(col, row, cell_w, cell_h),
                        cell,
                        colors.fg,
                    );
                }

                if is_cursor && cursor_style != CursorStyle::Block {
                    draw_cursor(
                        buffer,
                        (buf_w, buf_h),
                        CellRect::from_cell(col, row, cell_w, cell_h),
                        cursor_style,
                        base_fg,
                    );
                }
            }
        }
    }

    if config.scrollback.scrollbar {
        draw_scrollback_scrollbar(
            buffer,
            buf_w,
            buf_h,
            grid.scrollback_len(),
            grid.rows,
            grid.scroll_offset as f32,
            base_fg,
        );
    }

    terminal.grid_mut().clear_dirty();
    *last_visual_state = Some(current_visual);
}

fn draw_scrollback_scrollbar(
    buffer: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    scrollback_rows: usize,
    visible_rows: usize,
    scroll_rows: f32,
    fg: u32,
) {
    const SCROLLBAR_WIDTH_PX: usize = 1;
    const MIN_THUMB_PX: f32 = 24.0;
    const TRACK_ALPHA: u8 = 26;
    const THUMB_ALPHA: u8 = 140;

    let Some(geometry) = compute_scrollbar_geometry(
        scrollback_rows,
        visible_rows,
        scroll_rows,
        buf_h as f32,
        MIN_THUMB_PX,
    ) else {
        return;
    };

    let x_start = buf_w.saturating_sub(SCROLLBAR_WIDTH_PX);
    draw_alpha_rect(
        buffer,
        buf_w,
        buf_h,
        AlphaRect {
            x: x_start,
            y: 0,
            width: SCROLLBAR_WIDTH_PX,
            height: buf_h,
            rgb: fg,
            alpha: TRACK_ALPHA,
        },
    );
    draw_alpha_rect(
        buffer,
        buf_w,
        buf_h,
        AlphaRect {
            x: x_start,
            y: geometry.thumb_y_px.floor() as usize,
            width: SCROLLBAR_WIDTH_PX,
            height: geometry.thumb_h_px.ceil() as usize,
            rgb: fg,
            alpha: THUMB_ALPHA,
        },
    );
}

struct AlphaRect {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    rgb: u32,
    alpha: u8,
}

fn draw_alpha_rect(buffer: &mut [u32], buf_w: usize, buf_h: usize, rect: AlphaRect) {
    let x_end = (rect.x + rect.width).min(buf_w);
    let y_end = (rect.y + rect.height).min(buf_h);
    for py in rect.y.min(buf_h)..y_end {
        for px in rect.x.min(buf_w)..x_end {
            let idx = py * buf_w + px;
            let rgba = [
                ((rect.rgb >> 16) & 0xff) as u8,
                ((rect.rgb >> 8) & 0xff) as u8,
                (rect.rgb & 0xff) as u8,
                rect.alpha,
            ];
            blend_rgba_over_rgb(&mut buffer[idx], &rgba);
        }
    }
}

fn has_complex_dirty_cells(grid: &crate::grid::Grid) -> bool {
    for row in 0..grid.rows {
        if !grid.row_has_dirty_cells(row) {
            continue;
        }
        for col in 0..grid.cols {
            if !grid.is_cell_dirty(row, col) {
                continue;
            }
            let cell = grid.cell_at_scroll(row, col);
            if cell.ch > 0x7f
                || grid.cell_grapheme_at_scroll(row, col).is_some()
                || cell.flags & (crate::grid::FLAG_WIDE | crate::grid::FLAG_WIDE_CONT) != 0
            {
                return true;
            }
        }
    }
    false
}

#[derive(Debug, Clone, Copy)]
struct CellRect {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

impl CellRect {
    fn from_cell(col: usize, row: usize, cell_w: usize, cell_h: usize) -> Self {
        Self {
            x: col * cell_w,
            y: row * cell_h,
            width: cell_w,
            height: cell_h,
        }
    }
}

fn draw_cursor(
    buffer: &mut [u32],
    dims: (usize, usize),
    cell: CellRect,
    cursor_style: CursorStyle,
    color: u32,
) {
    let (buf_w, buf_h) = dims;
    match cursor_style {
        CursorStyle::Bar => {
            let bar_w = 2.min(cell.width);
            for y in cell.y..(cell.y + cell.height).min(buf_h) {
                for x in cell.x..(cell.x + bar_w).min(buf_w) {
                    buffer[y * buf_w + x] = color;
                }
            }
        }
        CursorStyle::Underline => {
            let ul_h = 2.min(cell.height);
            let y_start = (cell.y + cell.height).saturating_sub(ul_h);
            for y in y_start..(cell.y + cell.height).min(buf_h) {
                for x in cell.x..(cell.x + cell.width).min(buf_w) {
                    buffer[y * buf_w + x] = color;
                }
            }
        }
        CursorStyle::Block => {}
    }
}

fn draw_text_decorations(
    buffer: &mut [u32],
    dims: (usize, usize),
    rect: CellRect,
    cell: &Cell,
    actual_fg: u32,
) {
    use crate::grid::*;

    let (buf_w, buf_h) = dims;
    let px_x = rect.x;
    let px_y = rect.y;
    let cell_w = rect.width;
    let cell_h = rect.height;

    if cell.attrs & ATTR_UNDERLINE != 0 {
        let ul_color = resolve_underline_color(cell, actual_fg);
        let y_base = (px_y + cell_h).saturating_sub(2);
        match cell.underline_style {
            UnderlineStyle::Single | UnderlineStyle::None => {
                if y_base < buf_h {
                    for x in px_x..(px_x + cell_w).min(buf_w) {
                        buffer[y_base * buf_w + x] = ul_color;
                    }
                }
            }
            UnderlineStyle::Double => {
                let y1 = y_base;
                let y2 = y_base.saturating_sub(2);
                for &y in &[y1, y2] {
                    if y < buf_h {
                        for x in px_x..(px_x + cell_w).min(buf_w) {
                            buffer[y * buf_w + x] = ul_color;
                        }
                    }
                }
            }
            UnderlineStyle::Curly => {
                let x_start = px_x;
                let x_end = (px_x + cell_w).min(buf_w);
                let amplitude = 2.0_f32;
                let period = cell_w as f32;
                for x in x_start..x_end {
                    let phase = (x - px_x) as f32 / period * std::f32::consts::TAU;
                    let dy = (phase.sin() * amplitude) as i32;
                    let y = (y_base as i32 + dy).max(0) as usize;
                    if y < buf_h {
                        buffer[y * buf_w + x] = ul_color;
                        if y + 1 < buf_h {
                            buffer[(y + 1) * buf_w + x] = ul_color;
                        }
                    }
                }
            }
            UnderlineStyle::Dotted => {
                if y_base < buf_h {
                    for x in px_x..(px_x + cell_w).min(buf_w) {
                        if (x - px_x).is_multiple_of(3) {
                            buffer[y_base * buf_w + x] = ul_color;
                        }
                    }
                }
            }
            UnderlineStyle::Dashed => {
                if y_base < buf_h {
                    let dash = cell_w / 3;
                    for x in px_x..(px_x + cell_w).min(buf_w) {
                        let offset = x - px_x;
                        if offset < dash || (offset >= dash * 2 && offset < dash * 3) {
                            buffer[y_base * buf_w + x] = ul_color;
                        }
                    }
                }
            }
        }
    }
    if cell.attrs & ATTR_STRIKETHROUGH != 0 {
        let y = px_y + cell_h / 2;
        if y < buf_h {
            for x in px_x..(px_x + cell_w).min(buf_w) {
                buffer[y * buf_w + x] = actual_fg;
            }
        }
    }
}

fn draw_kitty_images(
    buffer: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    terminal: &impl TerminalView,
    cell_w: usize,
    cell_h: usize,
) {
    for placement in terminal.kitty_placements() {
        let Some(image) = terminal.kitty_image(placement.image_id) else {
            continue;
        };
        if image.width == 0 || image.height == 0 {
            continue;
        }
        if image.data.len() != (image.width as usize) * (image.height as usize) * 4 {
            continue;
        }

        let px_x = placement.col * cell_w;
        let px_y = placement.row * cell_h;
        let px_w = placement.cols.max(1) * cell_w;
        let px_h = placement.rows.max(1) * cell_h;
        let draw_w = px_w.min(buf_w.saturating_sub(px_x));
        let draw_h = px_h.min(buf_h.saturating_sub(px_y));

        if draw_w == 0 || draw_h == 0 {
            continue;
        }

        for dy in 0..draw_h {
            let src_y = dy * image.height as usize / px_h.max(1);
            let dst_y = px_y + dy;
            let row_start = dst_y * buf_w;

            for dx in 0..draw_w {
                let src_x = dx * image.width as usize / px_w.max(1);
                let src_offset = (src_y * image.width as usize + src_x) * 4;
                let pixel = &mut buffer[row_start + px_x + dx];
                blend_rgba(pixel, &image.data[src_offset..src_offset + 4]);
            }
        }
    }
}

fn blend_rgba(pixel: &mut u32, rgba: &[u8]) {
    blend_rgba_over_rgb(pixel, rgba);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::Terminal;
    use crate::workloads::{
        EMOJI_AND_SHADE_TRANSCRIPT, FISH_STARTUP_TRANSCRIPT, STARSHIP_PROMPT_TRANSCRIPT,
        TUI_HELP_OVERLAY_TRANSCRIPT,
    };
    use unicode_segmentation::UnicodeSegmentation;
    use unicode_width::UnicodeWidthStr;

    fn new_atlas(config: &AppConfig) -> GlyphAtlas {
        GlyphAtlas::new(config.style.font_size).expect("should load a monospace font for rendering")
    }

    fn new_atlas_with_dpi(config: &AppConfig, dpi: u32) -> GlyphAtlas {
        GlyphAtlas::with_family_dpi(&config.style.font_family, config.style.font_size, dpi)
            .or_else(|_| GlyphAtlas::new_with_dpi(config.style.font_size, dpi))
            .expect("should load a monospace font atlas for requested dpi")
    }

    fn extract_cell_pixels(
        renderer: &OffscreenRenderer,
        col: usize,
        row: usize,
        cell_w: usize,
        cell_h: usize,
    ) -> Vec<u32> {
        let mut out = Vec::with_capacity(cell_w * cell_h);
        let x0 = col * cell_w;
        let y0 = row * cell_h;
        for y in y0..(y0 + cell_h).min(renderer.height) {
            let row_start = y * renderer.width;
            out.extend_from_slice(
                &renderer.pixels[row_start + x0..row_start + (x0 + cell_w).min(renderer.width)],
            );
        }
        out
    }

    fn cell_region_has_non_bg(
        renderer: &OffscreenRenderer,
        col: usize,
        row: usize,
        span_cols: usize,
        cell_w: usize,
        cell_h: usize,
        bg: u32,
    ) -> bool {
        let x0 = col * cell_w;
        let x1 = ((col + span_cols.max(1)) * cell_w).min(renderer.width);
        let y0 = row * cell_h;
        let y1 = ((row + 1) * cell_h).min(renderer.height);
        for y in y0..y1 {
            let row_start = y * renderer.width;
            if renderer.pixels[row_start + x0..row_start + x1]
                .iter()
                .any(|&pixel| pixel != bg)
            {
                return true;
            }
        }
        false
    }

    fn replay_chunks_match_full_redraw(
        cols: u16,
        rows: u16,
        chunks: &[&[u8]],
        per_step_assert: impl Fn(&Terminal, usize),
    ) {
        let config = AppConfig::default();
        let mut atlas = new_atlas(&config);
        let mut terminal = Terminal::new(cols, rows);
        let mut incremental = OffscreenRenderer::new(cols, rows, &atlas);
        let mut full = OffscreenRenderer::new(cols, rows, &atlas);

        for (idx, chunk) in chunks.iter().enumerate() {
            terminal.process(chunk);
            incremental.render(&mut terminal, &mut atlas, &config);
            full.reset();
            full.render(&mut terminal, &mut atlas, &config);
            assert_eq!(
                incremental.pixels, full.pixels,
                "incremental render diverged after replay chunk {idx}"
            );
            per_step_assert(&terminal, idx);
        }
    }

    #[test]
    fn incremental_typing_matches_full_redraw() {
        let config = AppConfig::default();
        let cols = 32;
        let rows = 2;
        let mut atlas = new_atlas(&config);
        let mut terminal = Terminal::new(cols, rows);
        let mut incremental = OffscreenRenderer::new(cols, rows, &atlas);
        let mut full = OffscreenRenderer::new(cols, rows, &atlas);

        terminal.process(b"\x1b[38;5;10m>\x1b[0m ");
        incremental.render(&mut terminal, &mut atlas, &config);

        for &byte in b"echo hello world" {
            terminal.process(&[byte]);
            incremental.render(&mut terminal, &mut atlas, &config);
            full.reset();
            full.render(&mut terminal, &mut atlas, &config);
            assert_eq!(
                incremental.pixels, full.pixels,
                "incremental render diverged after typing byte {byte:?}"
            );
        }
    }

    #[test]
    fn line_repaint_matches_full_redraw() {
        let config = AppConfig::default();
        let cols = 32;
        let rows = 2;
        let mut atlas = new_atlas(&config);
        let mut terminal = Terminal::new(cols, rows);
        let mut incremental = OffscreenRenderer::new(cols, rows, &atlas);
        let mut full = OffscreenRenderer::new(cols, rows, &atlas);

        terminal.process(b"\x1b[38;5;10m>\x1b[0m build");
        incremental.render(&mut terminal, &mut atlas, &config);

        terminal.process(b"\r\x1b[2K\x1b[38;5;196merror:\x1b[0m failed");
        incremental.render(&mut terminal, &mut atlas, &config);
        full.reset();
        full.render(&mut terminal, &mut atlas, &config);

        assert_eq!(incremental.pixels, full.pixels);
    }

    #[test]
    fn full_screen_repaint_matches_full_redraw() {
        let config = AppConfig::default();
        let cols = 32;
        let rows = 6;
        let mut atlas = new_atlas(&config);
        let mut terminal = Terminal::new(cols, rows);
        let mut incremental = OffscreenRenderer::new(cols, rows, &atlas);
        let mut full = OffscreenRenderer::new(cols, rows, &atlas);

        terminal.process(
            b"\x1b[?1049h\
              one\r\n\
              two\r\n\
              three\r\n\
              four\r\n\
              five\r\n",
        );
        incremental.render(&mut terminal, &mut atlas, &config);

        terminal.process(
            b"\x1b[2J\x1b[H\
              \x1b[38;5;39mstatus\x1b[0m\r\n\
              alpha beta gamma\r\n\
              delta epsilon\r\n\
              zeta eta theta\r\n\
              iota kappa\r\n",
        );
        incremental.render(&mut terminal, &mut atlas, &config);
        full.reset();
        full.render(&mut terminal, &mut atlas, &config);

        assert_eq!(incremental.pixels, full.pixels);
    }

    #[test]
    fn kitty_image_renders_into_framebuffer() {
        let config = AppConfig::default();
        let cols = 4;
        let rows = 2;
        let mut atlas = new_atlas(&config);
        let mut terminal = Terminal::new(cols, rows);
        let mut renderer = OffscreenRenderer::new(cols, rows, &atlas);

        terminal.process(b"\x1b_Ga=T,i=5,f=32,s=1,v=1,c=1,r=1;/wAA/w==\x1b\\");
        renderer.render(&mut terminal, &mut atlas, &config);

        assert!(
            renderer.pixels.contains(&0xff0000),
            "expected kitty image to draw a red pixel"
        );
    }

    #[test]
    fn kitty_image_alpha_blends_with_background() {
        let config = AppConfig::default();
        let cols = 2;
        let rows = 1;
        let mut atlas = new_atlas(&config);
        let mut terminal = Terminal::new(cols, rows);
        let mut renderer = OffscreenRenderer::new(cols, rows, &atlas);

        let mut cell = crate::grid::Cell::BLANK;
        cell.ch = ' ' as u32;
        cell.fg = crate::grid::COLOR_DEFAULT;
        cell.bg = crate::grid::COLOR_FLAG_RGB | 0x20_40_60;
        terminal.grid.set_cell(0, 0, cell);
        terminal.cursor_visible = false;
        terminal.process(b"\x1b[H\x1b_Ga=T,i=5,f=32,s=1,v=1,c=1,r=1;/wAAgA==\x1b\\");
        renderer.render(&mut terminal, &mut atlas, &config);

        let expected = 0x8f1f2f;
        assert_eq!(renderer.pixels[0], expected);
    }

    #[test]
    fn full_redraw_fast_path_preserves_custom_and_inverse_backgrounds() {
        // The full-redraw background pass skips cells that resolve to base_bg
        // (the buffer is pre-cleared to it). This guards that custom-bg, inverse,
        // and default cells all still produce the correct background after the
        // fast-path skip is applied.
        let config = AppConfig::default();
        let cols = 3u16;
        let rows = 1u16;
        let mut atlas = new_atlas(&config);
        let mut terminal = Terminal::new(cols, rows);
        terminal.cursor_visible = false;

        // col 0: default background (should stay base_bg, skipped by fast path).
        // col 1: explicit RGB background.
        let mut custom = crate::grid::Cell::BLANK;
        custom.ch = ' ' as u32;
        custom.bg = crate::grid::COLOR_FLAG_RGB | 0x11_22_33;
        terminal.grid.set_cell(0, 1, custom);
        // col 2: inverse attribute swaps fg/bg, so its background becomes base_fg.
        let mut inverse = crate::grid::Cell::BLANK;
        inverse.ch = ' ' as u32;
        inverse.attrs = crate::grid::ATTR_INVERSE;
        terminal.grid.set_cell(0, 2, inverse);

        let mut renderer = OffscreenRenderer::new(cols, rows, &atlas);
        renderer.render(&mut terminal, &mut atlas, &config);

        let base_bg = config.style.background.as_u32_rgb();
        let base_fg = config.style.foreground.as_u32_rgb();
        let cw = atlas.cell_width;

        // Sample the top-left pixel of each cell's background.
        assert_eq!(renderer.pixels[0], base_bg, "default cell should be base_bg");
        assert_eq!(
            renderer.pixels[cw], 0x11_22_33,
            "custom-bg cell should keep its explicit background"
        );
        assert_eq!(
            renderer.pixels[2 * cw],
            base_fg,
            "inverse cell background should become base_fg"
        );
    }

    #[test]
    fn fish_startup_transcript_replay_matches_full_redraw() {
        replay_chunks_match_full_redraw(80, 24, FISH_STARTUP_TRANSCRIPT, |terminal, _| {
            for row in 0..24 {
                for col in 0..80 {
                    let ch = terminal.grid.cell_char(row, col);
                    assert!(
                        ch == ' ' || ch == '\0',
                        "fish startup leaked visible char '{}' at row={} col={}",
                        ch,
                        row,
                        col
                    );
                }
            }
        });
    }

    #[test]
    fn starship_prompt_transcript_replay_matches_full_redraw() {
        replay_chunks_match_full_redraw(80, 24, STARSHIP_PROMPT_TRANSCRIPT, |terminal, idx| {
            if idx == STARSHIP_PROMPT_TRANSCRIPT.len() - 1 {
                let mut text = String::new();
                for col in 0..80 {
                    let ch = terminal.grid.cell_char(1, col);
                    if ch != ' ' && ch != '\0' {
                        text.push(ch);
                    }
                }
                assert!(
                    text.contains("jeremy"),
                    "row 1 should contain 'jeremy', got: {:?}",
                    text
                );
            }
        });
    }

    #[test]
    fn tui_help_overlay_transcript_replay_matches_full_redraw() {
        replay_chunks_match_full_redraw(32, 8, TUI_HELP_OVERLAY_TRANSCRIPT, |terminal, idx| {
            if idx == TUI_HELP_OVERLAY_TRANSCRIPT.len() - 1 {
                let mut row0 = String::new();
                for col in 0..32 {
                    let ch = terminal.grid.cell_char(0, col);
                    if ch != ' ' && ch != '\0' {
                        row0.push(ch);
                    }
                }
                assert!(
                    row0.contains("/help"),
                    "expected help overlay in top row, got: {:?}",
                    row0
                );
            }
        });
    }

    #[test]
    fn emoji_and_shade_transcript_replay_matches_full_redraw() {
        replay_chunks_match_full_redraw(16, 4, EMOJI_AND_SHADE_TRANSCRIPT, |terminal, idx| {
            if idx == EMOJI_AND_SHADE_TRANSCRIPT.len() - 1 {
                assert_eq!(terminal.grid.cell_grapheme_at(0, 7), Some("❤️"));
                assert_eq!(terminal.grid.cell_grapheme_at(0, 10), Some("👨‍💻"));
                let row1 = (0..16)
                    .filter_map(|col| match terminal.grid.cell_char(1, col) {
                        ' ' | '\0' => None,
                        ch => Some(ch),
                    })
                    .collect::<String>();
                assert_eq!(row1, "░░░░░░░░░░");
            }
            if idx >= 1 {
                assert!(
                    terminal.grid.get_text(0, 16).contains("❤️"),
                    "top row should retain the heart cluster after replay chunk {idx}"
                );
            }
        });
    }

    #[test]
    fn generic_emoji_probe_replay_matches_full_redraw() {
        let chunks: &[&[u8]] = &[
            "A🪸B A🫠B A🫡B\r\n".as_bytes(),
            "A🩷B A😀B A❤️B\r\n".as_bytes(),
            "A👨‍💻B A🇺🇸B A👍🏻B A1️⃣B".as_bytes(),
        ];

        replay_chunks_match_full_redraw(32, 4, chunks, |terminal, idx| {
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
    fn jcode_like_glyph_probe_replay_matches_full_redraw() {
        let chunks: &[&[u8]] = &[
            "⟨client⟩\r\n".as_bytes(),
            "Ancient Coral 🪸\r\n".as_bytes(),
            "● an  ● or  ● oa  ● cu  ● cp  ● ge(oauth)  ○ ag\r\n".as_bytes(),
            "⠼ connecting… 3.6s · websocket/persistent-fresh 󰌘".as_bytes(),
        ];

        replay_chunks_match_full_redraw(64, 6, chunks, |terminal, idx| {
            if idx == chunks.len() - 1 {
                assert_eq!(terminal.grid.cell_grapheme_at(1, 14), None);
                assert_eq!(terminal.grid.cell_char(1, 14), '🪸');
                assert_eq!(terminal.grid.cell_char(3, 0), '⠼');
                assert_eq!(terminal.grid.cell_char(3, 48), '󰌘');
            }
        });
    }

    #[test]
    fn persistent_cpu_framebuffer_can_present_into_fresh_front_buffers() {
        let config = AppConfig::default();
        let cols = 32;
        let rows = 2;
        let mut atlas = new_atlas(&config);
        let mut terminal = Terminal::new(cols, rows);
        let mut persistent = OffscreenRenderer::new(cols, rows, &atlas);
        let mut full = OffscreenRenderer::new(cols, rows, &atlas);

        terminal.process(b"\x1b[38;5;10m>\x1b[0m echo");
        persistent.render(&mut terminal, &mut atlas, &config);

        terminal.process(b" hello");
        persistent.render(&mut terminal, &mut atlas, &config);

        let mut presented = vec![0u32; persistent.pixels.len()];
        presented.copy_from_slice(&persistent.pixels);

        full.reset();
        full.render(&mut terminal, &mut atlas, &config);

        assert_eq!(presented, full.pixels);
    }

    #[test]
    #[cfg(feature = "local-fonts")]
    fn configured_font_emoji_probe_draws_pixels_at_high_dpi() {
        let config = AppConfig::default();
        let sample = "😀 🪸 🫎 🐦‍⬛ ❤️ 👍🏻 1️⃣ 🇺🇸 👨‍💻";

        for dpi in [96u32, 144, 217] {
            let mut atlas = new_atlas_with_dpi(&config, dpi);
            let mut terminal = Terminal::new(32, 2);
            terminal.cursor_visible = false;
            terminal.process(sample.as_bytes());
            let mut renderer = OffscreenRenderer::new(terminal.cols, terminal.rows, &atlas);
            renderer.render(&mut terminal, &mut atlas, &config);
            let bg = config.style.background.as_u32_rgb();

            let mut col = 0usize;
            for grapheme in UnicodeSegmentation::graphemes(sample, true) {
                let cells = UnicodeWidthStr::width(grapheme).clamp(1, 2);
                if grapheme != " " {
                    assert!(
                        cell_region_has_non_bg(
                            &renderer,
                            col,
                            0,
                            cells,
                            atlas.cell_width,
                            atlas.cell_height,
                            bg,
                        ),
                        "grapheme {:?} rendered no visible pixels at dpi {}",
                        grapheme,
                        dpi,
                    );
                }
                col += cells;
            }
        }
    }

    #[test]
    #[cfg(feature = "local-fonts")]
    fn digit_cells_are_stable_with_neighbors_at_high_dpi() {
        let config = AppConfig::default();
        let sequence = "0123456789";

        for dpi in [96u32, 144, 217] {
            let mut atlas = new_atlas_with_dpi(&config, dpi);
            let mut seq_terminal = Terminal::new(sequence.len() as u16, 1);
            seq_terminal.cursor_visible = false;
            seq_terminal.process(sequence.as_bytes());
            let mut seq_renderer =
                OffscreenRenderer::new(seq_terminal.cols, seq_terminal.rows, &atlas);
            seq_renderer.render(&mut seq_terminal, &mut atlas, &config);

            for (idx, ch) in sequence.chars().enumerate() {
                let isolated = format!(" {} ", ch);
                let mut isolated_terminal = Terminal::new(3, 1);
                isolated_terminal.cursor_visible = false;
                isolated_terminal.process(isolated.as_bytes());
                let mut isolated_renderer =
                    OffscreenRenderer::new(isolated_terminal.cols, isolated_terminal.rows, &atlas);
                isolated_renderer.render(&mut isolated_terminal, &mut atlas, &config);

                assert_eq!(
                    extract_cell_pixels(
                        &isolated_renderer,
                        1,
                        0,
                        atlas.cell_width,
                        atlas.cell_height,
                    ),
                    extract_cell_pixels(&seq_renderer, idx, 0, atlas.cell_width, atlas.cell_height,),
                    "digit {:?} changed appearance in sequence context at dpi {}",
                    ch,
                    dpi,
                );
            }
        }
    }
}
