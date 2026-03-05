use anyhow::{Context, Result};
use freetype::face::LoadFlag;
use freetype::Library;
use std::collections::HashMap;

pub struct GlyphAtlas {
    glyphs: HashMap<u32, RasterizedGlyph>,
    lib: Library,
    font_path: String,
    font_size_pt: f64,
    dpi: u32,
    pub cell_width: usize,
    pub cell_height: usize,
    pub baseline: usize,
}

struct RasterizedGlyph {
    bitmap: Vec<u8>,
    width: usize,
    height: usize,
    bearing_x: i32,
    bearing_y: i32,
    #[allow(dead_code)]
    advance: i32,
}

impl GlyphAtlas {
    pub fn new(font_size_pt: f64) -> Result<Self> {
        Self::new_with_dpi(font_size_pt, 96)
    }

    pub fn new_with_dpi(font_size_pt: f64, dpi: u32) -> Result<Self> {
        let font_path = find_monospace_font(None)?;
        Self::from_font_path_dpi(&font_path, font_size_pt, dpi)
    }

    pub fn with_family(family: &str, font_size_pt: f64) -> Result<Self> {
        Self::with_family_dpi(family, font_size_pt, 96)
    }

    pub fn with_family_dpi(family: &str, font_size_pt: f64, dpi: u32) -> Result<Self> {
        if let Some(cached) = load_cached_font_path(family) {
            if std::path::Path::new(&cached).exists() {
                return Self::from_font_path_dpi(&cached, font_size_pt, dpi);
            }
        }
        let font_path = find_monospace_font(Some(family))?;
        save_cached_font_path(family, &font_path);
        Self::from_font_path_dpi(&font_path, font_size_pt, dpi)
    }

    pub fn from_font_path(path: &str, font_size_pt: f64) -> Result<Self> {
        Self::from_font_path_dpi(path, font_size_pt, 96)
    }

    pub fn from_font_path_dpi(path: &str, font_size_pt: f64, dpi: u32) -> Result<Self> {
        let lib = Library::init().context("failed to init freetype")?;

        let face = lib
            .new_face(path, 0)
            .context("failed to load font face")?;

        face.set_char_size((font_size_pt * 64.0) as isize, 0, dpi, 0)
            .context("failed to set char size")?;

        let metrics = face.size_metrics().context("no size metrics")?;
        let cell_height = (metrics.height >> 6) as usize;
        let baseline = (-metrics.descender >> 6) as usize;

        face.load_char('M' as usize, LoadFlag::RENDER)
            .context("failed to load 'M'")?;
        let cell_width = (face.glyph().advance().x >> 6) as usize;

        Ok(Self {
            glyphs: HashMap::with_capacity(128),
            lib,
            font_path: path.to_string(),
            font_size_pt,
            dpi,
            cell_width: cell_width.max(1),
            cell_height: cell_height.max(1),
            baseline,
        })
    }

    fn ensure_glyph(&mut self, ch: u32) -> bool {
        if self.glyphs.contains_key(&ch) {
            return true;
        }
        let Ok(face) = self.lib.new_face(&self.font_path, 0) else {
            return false;
        };
        if face
            .set_char_size((self.font_size_pt * 64.0) as isize, 0, self.dpi, 0)
            .is_err()
        {
            return false;
        }
        if let Some(g) = rasterize_one(&face, ch) {
            self.glyphs.insert(ch, g);
            true
        } else {
            false
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_char(
        &mut self,
        buffer: &mut [u32],
        buf_w: usize,
        buf_h: usize,
        cell_x: usize,
        cell_y: usize,
        ch: u32,
        fg: u32,
        bg: u32,
    ) {
        let px_x = cell_x * self.cell_width;
        let px_y = cell_y * self.cell_height;
        let cw = self.cell_width;
        let ch_height = self.cell_height;

        let x_end = (px_x + cw).min(buf_w);
        let y_end = (px_y + ch_height).min(buf_h);

        for y in px_y..y_end {
            let row_start = y * buf_w + px_x;
            let row_end = y * buf_w + x_end;
            buffer[row_start..row_end].fill(bg);
        }

        self.ensure_glyph(ch);

        let Some(glyph) = self.glyphs.get(&ch) else {
            return;
        };

        let origin_y = px_y as i32 + (ch_height as i32 - self.baseline as i32);
        let glyph_top = origin_y - glyph.bearing_y;
        let glyph_left = px_x as i32 + glyph.bearing_x;

        let fg_r = (fg >> 16) & 0xff;
        let fg_g = (fg >> 8) & 0xff;
        let fg_b = fg & 0xff;

        let gy_start = if glyph_top < 0 {
            (-glyph_top) as usize
        } else {
            0
        };
        let gy_end = glyph.height.min(((buf_h as i32) - glyph_top).max(0) as usize);

        let gx_start = if glyph_left < 0 {
            (-glyph_left) as usize
        } else {
            0
        };
        let gx_end = glyph.width.min(((buf_w as i32) - glyph_left).max(0) as usize);

        for gy in gy_start..gy_end {
            let sy = (glyph_top + gy as i32) as usize;
            let bmp_row = gy * glyph.width;
            let screen_row = sy * buf_w;

            for gx in gx_start..gx_end {
                let alpha = glyph.bitmap[bmp_row + gx] as u32;
                if alpha == 0 {
                    continue;
                }

                let sx = (glyph_left + gx as i32) as usize;
                let pixel = &mut buffer[screen_row + sx];

                if alpha == 255 {
                    *pixel = fg;
                } else {
                    let bg_pixel = *pixel;
                    let bg_r = (bg_pixel >> 16) & 0xff;
                    let bg_g = (bg_pixel >> 8) & 0xff;
                    let bg_b = bg_pixel & 0xff;
                    let inv = 255 - alpha;
                    let r = (fg_r * alpha + bg_r * inv) / 255;
                    let g = (fg_g * alpha + bg_g * inv) / 255;
                    let b = (fg_b * alpha + bg_b * inv) / 255;
                    *pixel = (r << 16) | (g << 8) | b;
                }
            }
        }
    }

    #[allow(dead_code)]
    pub fn has_glyph(&self, ch: u32) -> bool {
        self.glyphs.contains_key(&ch)
    }
}

fn rasterize_one(face: &freetype::Face, ch: u32) -> Option<RasterizedGlyph> {
    face.load_char(ch as usize, LoadFlag::RENDER).ok()?;
    let glyph = face.glyph();
    let bmp = glyph.bitmap();
    let width = bmp.width() as usize;
    let height = bmp.rows() as usize;
    let pitch = bmp.pitch().unsigned_abs() as usize;
    let buffer = bmp.buffer();

    let mut bitmap = vec![0u8; width * height];
    for y in 0..height {
        for x in 0..width {
            bitmap[y * width + x] = buffer[y * pitch + x];
        }
    }

    Some(RasterizedGlyph {
        bitmap,
        width,
        height,
        bearing_x: glyph.bitmap_left(),
        bearing_y: glyph.bitmap_top(),
        advance: (glyph.advance().x >> 6) as i32,
    })
}

fn find_monospace_font(preferred_family: Option<&str>) -> Result<String> {
    let fc = fontconfig::Fontconfig::new().context("failed to init fontconfig")?;

    if let Some(family) = preferred_family
        && let Some(font) = fc.find(family, None)
        && let Some(path) = font.path.to_str()
    {
        return Ok(path.to_string());
    }

    let fallbacks = [
        "JetBrains Mono",
        "Fira Code",
        "Source Code Pro",
        "DejaVu Sans Mono",
        "Liberation Mono",
        "monospace",
    ];

    for name in &fallbacks {
        if let Some(font) = fc.find(name, None)
            && let Some(path) = font.path.to_str()
        {
            return Ok(path.to_string());
        }
    }

    anyhow::bail!("no monospace font found via fontconfig")
}

fn font_cache_path() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".cache").join("handterm").join("font_path"))
}

fn load_cached_font_path(family: &str) -> Option<String> {
    let cache = font_cache_path()?;
    let content = std::fs::read_to_string(&cache).ok()?;
    for line in content.lines() {
        if let Some((k, v)) = line.split_once('=') {
            if k == family {
                return Some(v.to_string());
            }
        }
    }
    None
}

fn save_cached_font_path(family: &str, path: &str) {
    let Some(cache) = font_cache_path() else { return };
    if let Some(parent) = cache.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut content = std::fs::read_to_string(&cache).unwrap_or_default();
    let entry = format!("{}={}\n", family, path);
    if !content.contains(&entry) {
        content.push_str(&entry);
        let _ = std::fs::write(&cache, content);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_system_monospace_font() {
        let mut atlas = GlyphAtlas::new(14.0).expect("should load a monospace font");
        assert!(atlas.cell_width > 0);
        assert!(atlas.cell_height > 0);
        assert!(atlas.ensure_glyph(b'A' as u32));
        assert!(atlas.ensure_glyph(b'@' as u32));
    }

    #[test]
    fn renders_glyph_to_buffer() {
        let mut atlas = GlyphAtlas::new(14.0).unwrap();
        let w = atlas.cell_width * 2;
        let h = atlas.cell_height * 2;
        let mut buf = vec![0u32; w * h];
        atlas.draw_char(&mut buf, w, h, 0, 0, b'A' as u32, 0xffffff, 0x000000);
        let non_black = buf.iter().filter(|&&p| p != 0x000000).count();
        assert!(non_black > 0, "glyph 'A' should have visible pixels");
    }
}
