use anyhow::{Context, Result};
use freetype::face::LoadFlag;
use freetype::Library;
use std::collections::HashMap;

pub struct GlyphAtlas {
    glyphs: HashMap<u32, RasterizedGlyph>,
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
        let font_path = find_monospace_font(None)?;
        Self::from_font_path(&font_path, font_size_pt)
    }

    pub fn with_family(family: &str, font_size_pt: f64) -> Result<Self> {
        let font_path = find_monospace_font(Some(family))?;
        Self::from_font_path(&font_path, font_size_pt)
    }

    pub fn from_font_path(path: &str, font_size_pt: f64) -> Result<Self> {
        let lib = Library::init().context("failed to init freetype")?;
        let face = lib
            .new_face(path, 0)
            .context("failed to load font face")?;

        face.set_char_size((font_size_pt * 64.0) as isize, 0, 96, 0)
            .context("failed to set char size")?;

        let metrics = face.size_metrics().context("no size metrics")?;
        let cell_height = (metrics.height >> 6) as usize;
        let baseline = (-metrics.descender >> 6) as usize;

        face.load_char('M' as usize, LoadFlag::RENDER)
            .context("failed to load 'M'")?;
        let cell_width = (face.glyph().advance().x >> 6) as usize;

        let mut glyphs = HashMap::with_capacity(128);

        for ch in 0x20u32..=0x7e {
            if face
                .load_char(ch as usize, LoadFlag::RENDER)
                .is_err()
            {
                continue;
            }
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

            glyphs.insert(
                ch,
                RasterizedGlyph {
                    bitmap,
                    width,
                    height,
                    bearing_x: glyph.bitmap_left(),
                    bearing_y: glyph.bitmap_top(),
                    advance: (glyph.advance().x >> 6) as i32,
                },
            );
        }

        let _ = cell_width.max(1);
        let _ = cell_height.max(1);

        Ok(Self {
            glyphs,
            cell_width: cell_width.max(1),
            cell_height: cell_height.max(1),
            baseline,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_char(
        &self,
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

        for dy in 0..self.cell_height {
            let y = px_y + dy;
            if y >= buf_h {
                break;
            }
            for dx in 0..self.cell_width {
                let x = px_x + dx;
                if x >= buf_w {
                    break;
                }
                buffer[y * buf_w + x] = bg;
            }
        }

        let Some(glyph) = self.glyphs.get(&ch) else {
            return;
        };

        let origin_y = px_y as i32 + (self.cell_height as i32 - self.baseline as i32);
        let glyph_top = origin_y - glyph.bearing_y;
        let glyph_left = px_x as i32 + glyph.bearing_x;

        let fg_r = (fg >> 16) & 0xff;
        let fg_g = (fg >> 8) & 0xff;
        let fg_b = fg & 0xff;

        for gy in 0..glyph.height {
            let screen_y = glyph_top + gy as i32;
            if screen_y < 0 || screen_y as usize >= buf_h {
                continue;
            }
            let sy = screen_y as usize;

            for gx in 0..glyph.width {
                let screen_x = glyph_left + gx as i32;
                if screen_x < 0 || screen_x as usize >= buf_w {
                    continue;
                }
                let sx = screen_x as usize;

                let alpha = glyph.bitmap[gy * glyph.width + gx] as u32;
                if alpha == 0 {
                    continue;
                }

                let pixel = &mut buffer[sy * buf_w + sx];

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_system_monospace_font() {
        let atlas = GlyphAtlas::new(14.0).expect("should load a monospace font");
        assert!(atlas.cell_width > 0);
        assert!(atlas.cell_height > 0);
        assert!(atlas.has_glyph(b'A' as u32));
        assert!(atlas.has_glyph(b'@' as u32));
    }

    #[test]
    fn renders_glyph_to_buffer() {
        let atlas = GlyphAtlas::new(14.0).unwrap();
        let w = atlas.cell_width * 2;
        let h = atlas.cell_height * 2;
        let mut buf = vec![0u32; w * h];
        atlas.draw_char(&mut buf, w, h, 0, 0, b'A' as u32, 0xffffff, 0x000000);
        let non_black = buf.iter().filter(|&&p| p != 0x000000).count();
        assert!(non_black > 0, "glyph 'A' should have visible pixels");
    }
}
