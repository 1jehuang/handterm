use crate::config::AppConfig;
use crate::font::GlyphAtlas;
use crate::frontend::{VisualState, sync_visual_damage};
use crate::grid::{COLOR_DEFAULT, Cell};
use crate::terminal::{CursorStyle, Terminal};
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

    pub fn render(
        &mut self,
        terminal: &mut Terminal,
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
    terminal: &mut Terminal,
    atlas: &mut GlyphAtlas,
    config: &AppConfig,
    last_visual_state: &mut Option<VisualState>,
) {
    let current_visual = VisualState::capture(terminal);
    sync_visual_damage(&mut terminal.grid, *last_visual_state, current_visual);

    let base_bg = config.style.background.as_u32_rgb();
    let base_fg = config.style.foreground.as_u32_rgb();

    let grid = &terminal.grid;
    let has_selection = grid.selection.is_some();
    let scrolled = grid.scroll_offset > 0;
    let full_redraw = grid.all_dirty || has_selection || scrolled;
    if full_redraw {
        buffer.fill(base_bg);
    }

    let (cursor_col, cursor_row) = grid.cursor_pos();
    let show_cursor = terminal.cursor_visible && grid.scroll_offset == 0;
    let cursor_style = terminal.cursor_style;
    let selection = grid.selection;
    let cell_w = atlas.cell_width;
    let cell_h = atlas.cell_height;

    for row in 0..grid.rows {
        for col in 0..grid.cols {
            let is_cursor = show_cursor && row == cursor_row && col == cursor_col;

            if !full_redraw && !is_cursor && !grid.is_cell_dirty(row, col) {
                continue;
            }

            let cell = grid.cell_at_scroll(row, col);
            let has_custom_bg = cell.bg != COLOR_DEFAULT;
            let is_dirty = grid.is_cell_dirty(row, col);

            if !full_redraw && !is_cursor && !has_custom_bg && !is_dirty {
                continue;
            }

            let is_cursor_block = is_cursor && cursor_style == CursorStyle::Block;
            let selected = is_in_selection(selection, row, col);
            let colors = resolve_cell_colors(cell, base_fg, base_bg, is_cursor_block, selected);

            atlas.draw_bg(buffer, buf_w, buf_h, col, row, colors.bg);
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
                full_redraw || (0..grid.cols).any(|c| grid.is_cell_dirty(row, c)) || (show_cursor && row == cursor_row);
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
                            atlas.draw_shaped_glyph(buffer, buf_w, buf_h, col, row, sg.codepoint, sg.cells, fg);
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
                    let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1).max(1);
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
                let has_content = cell.ch > 0x20;

                let same_run =
                    has_content && !run_text.is_empty() && colors.fg == run_fg && cell.attrs == run_attrs;

                if !same_run && !run_text.is_empty() {
                    flush_run(atlas, buffer, buf_w, buf_h, &run_text, run_start, row, run_fg);
                    run_text.clear();
                }

                if has_content {
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
                    draw_cursor(buffer, buf_w, buf_h, col, row, cell_w, cell_h, cursor_style, base_fg);
                }
            }

            if !run_text.is_empty() {
                flush_run(atlas, buffer, buf_w, buf_h, &run_text, run_start, row, run_fg);
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

                draw_text_decorations(buffer, buf_w, buf_h, cell_w, cell_h, col, row, cell, colors.fg);
            }
        }
    }

    #[cfg(not(feature = "ligatures"))]
    {
        for row in 0..grid.rows {
            for col in 0..grid.cols {
                let is_cursor = show_cursor && row == cursor_row && col == cursor_col;

                if !full_redraw && !is_cursor && !grid.is_cell_dirty(row, col) {
                    continue;
                }

                let cell = grid.cell_at_scroll(row, col);

                if cell.flags & crate::grid::FLAG_WIDE_CONT != 0 && !is_cursor {
                    continue;
                }

                let has_content = cell.ch > 0x20;
                let is_cursor_block = is_cursor && cursor_style == CursorStyle::Block;
                let selected = is_in_selection(selection, row, col);
                let colors = resolve_cell_colors(cell, base_fg, base_bg, is_cursor_block, selected);

                if has_content {
                    atlas.draw_glyph(buffer, buf_w, buf_h, col, row, cell.ch, colors.fg);
                }

                if cell.attrs != 0 {
                    draw_text_decorations(buffer, buf_w, buf_h, cell_w, cell_h, col, row, cell, colors.fg);
                }

                if is_cursor && cursor_style != CursorStyle::Block {
                    draw_cursor(buffer, buf_w, buf_h, col, row, cell_w, cell_h, cursor_style, base_fg);
                }
            }
        }
    }

    terminal.grid.clear_dirty();
    *last_visual_state = Some(current_visual);
}

fn draw_cursor(
    buffer: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    col: usize,
    row: usize,
    cell_w: usize,
    cell_h: usize,
    cursor_style: CursorStyle,
    color: u32,
) {
    let px_x = col * cell_w;
    let px_y = row * cell_h;
    match cursor_style {
        CursorStyle::Bar => {
            let bar_w = 2.min(cell_w);
            for y in px_y..(px_y + cell_h).min(buf_h) {
                for x in px_x..(px_x + bar_w).min(buf_w) {
                    buffer[y * buf_w + x] = color;
                }
            }
        }
        CursorStyle::Underline => {
            let ul_h = 2.min(cell_h);
            let y_start = (px_y + cell_h).saturating_sub(ul_h);
            for y in y_start..(px_y + cell_h).min(buf_h) {
                for x in px_x..(px_x + cell_w).min(buf_w) {
                    buffer[y * buf_w + x] = color;
                }
            }
        }
        CursorStyle::Block => {}
    }
}

fn draw_text_decorations(
    buffer: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    cell_w: usize,
    cell_h: usize,
    col: usize,
    row: usize,
    cell: &Cell,
    actual_fg: u32,
) {
    use crate::grid::*;

    let px_x = col * cell_w;
    let px_y = row * cell_h;

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
                        if (x - px_x) % 3 == 0 {
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
    terminal: &Terminal,
    cell_w: usize,
    cell_h: usize,
) {
    for placement in &terminal.kitty_placements {
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
    let alpha = rgba[3] as u32;
    if alpha == 0 {
        return;
    }
    let bg = *pixel;
    let bg_r = (bg >> 16) & 0xff;
    let bg_g = (bg >> 8) & 0xff;
    let bg_b = bg & 0xff;
    let inv = 255 - alpha;

    let r = rgba[0] as u32 + (bg_r * inv) / 255;
    let g = rgba[1] as u32 + (bg_g * inv) / 255;
    let b = rgba[2] as u32 + (bg_b * inv) / 255;
    *pixel = (r.min(255) << 16) | (g.min(255) << 8) | b.min(255);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_atlas(config: &AppConfig) -> GlyphAtlas {
        GlyphAtlas::new(config.style.font_size).expect("should load a monospace font for rendering")
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
            assert_eq!(incremental.pixels, full.pixels, "incremental render diverged after typing byte {byte:?}");
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
            renderer.pixels.iter().any(|&p| p == 0xff0000),
            "expected kitty image to draw a red pixel"
        );
    }
}
