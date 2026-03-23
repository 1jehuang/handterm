use crate::color::{blend_rgba_over_rgb, blend_rgba_over_rgba};
use crate::protocol::{CellMetrics, GlyphBitmap};
#[cfg(feature = "local-fonts")]
use anyhow::Context;
use anyhow::Result;
#[cfg(feature = "local-fonts")]
use freetype::Face;
#[cfg(feature = "local-fonts")]
use freetype::bitmap::PixelMode;
#[cfg(feature = "local-fonts")]
use freetype::face::LoadFlag;
use std::collections::{HashMap, HashSet};

const CELL_WIDTH_SAMPLE_TEXT: &str = " !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~░▒▓";

pub struct GlyphAtlas {
    glyphs: HashMap<u32, RasterizedGlyph>,
    grapheme_glyphs: HashMap<Box<str>, RasterizedGlyph>,
    #[cfg(feature = "local-fonts")]
    primary_face: Option<Face>,
    #[cfg(feature = "local-fonts")]
    fallback_faces: Vec<Option<Face>>,
    #[cfg(feature = "local-fonts")]
    fallback_paths: Vec<String>,
    fallback_loaded: bool,
    font_size_pt: f64,
    dpi: u32,
    #[cfg(feature = "local-fonts")]
    glyph_sources: HashMap<u32, GlyphSource>,
    missing_glyphs: HashSet<u32>,
    font_path: String,
    pub cell_width: usize,
    pub cell_height: usize,
    pub baseline: usize,
    #[cfg(feature = "ligatures")]
    rb_face: Option<rustybuzz::Face<'static>>,
    #[cfg(feature = "ligatures")]
    fallback_rb_faces: Vec<Option<rustybuzz::Face<'static>>>,
    #[cfg(feature = "ligatures")]
    fallback_rb_loaded: bool,
}

pub struct GlyphData<'a> {
    pub pixels: &'a [u8],
    pub width: usize,
    pub height: usize,
    pub format: GlyphFormat,
    pub bearing_x: i32,
    pub bearing_y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlyphFormat {
    Alpha,
    Rgba,
}

#[cfg(feature = "local-fonts")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GlyphSource {
    Primary,
    Fallback(usize),
}

struct RasterizedGlyph {
    pixels: Vec<u8>,
    width: usize,
    height: usize,
    format: GlyphFormat,
    bearing_x: i32,
    bearing_y: i32,
    #[allow(dead_code)]
    advance: i32,
}

impl GlyphAtlas {
    pub fn new(font_size_pt: f64) -> Result<Self> {
        Self::new_with_dpi(font_size_pt, 96)
    }

    #[cfg(feature = "local-fonts")]
    pub fn new_with_dpi(font_size_pt: f64, dpi: u32) -> Result<Self> {
        let font_path = find_monospace_font(None)?;
        Self::from_font_path_dpi(&font_path, font_size_pt, dpi)
    }

    #[cfg(not(feature = "local-fonts"))]
    pub fn new_with_dpi(_font_size_pt: f64, _dpi: u32) -> Result<Self> {
        anyhow::bail!("local font loading is disabled in this build")
    }

    #[cfg(feature = "local-fonts")]
    pub fn with_family(family: &str, font_size_pt: f64) -> Result<Self> {
        Self::with_family_dpi(family, font_size_pt, 96)
    }

    #[cfg(not(feature = "local-fonts"))]
    pub fn with_family(_family: &str, _font_size_pt: f64) -> Result<Self> {
        anyhow::bail!("local font loading is disabled in this build")
    }

    #[cfg(feature = "local-fonts")]
    pub fn with_family_dpi(family: &str, font_size_pt: f64, dpi: u32) -> Result<Self> {
        if let Some(cached) = load_cached_font_path(family)
            && std::path::Path::new(&cached).exists()
        {
            return Self::from_font_path_dpi(&cached, font_size_pt, dpi);
        }
        let font_path = find_monospace_font(Some(family))?;
        save_cached_font_path(family, &font_path);
        Self::from_font_path_dpi(&font_path, font_size_pt, dpi)
    }

    #[cfg(not(feature = "local-fonts"))]
    pub fn with_family_dpi(_family: &str, _font_size_pt: f64, _dpi: u32) -> Result<Self> {
        anyhow::bail!("local font loading is disabled in this build")
    }

    #[cfg(feature = "local-fonts")]
    pub fn from_font_path(path: &str, font_size_pt: f64) -> Result<Self> {
        Self::from_font_path_dpi(path, font_size_pt, 96)
    }

    #[cfg(not(feature = "local-fonts"))]
    pub fn from_font_path(_path: &str, _font_size_pt: f64) -> Result<Self> {
        anyhow::bail!("local font loading is disabled in this build")
    }

    #[cfg(feature = "local-fonts")]
    pub fn from_font_path_dpi(path: &str, font_size_pt: f64, dpi: u32) -> Result<Self> {
        let lib = freetype::Library::init().context("failed to init freetype")?;

        let face = lib.new_face(path, 0).context("failed to load font face")?;

        face.set_char_size((font_size_pt * 64.0) as isize, 0, dpi, 0)
            .context("failed to set char size")?;

        let metrics = face.size_metrics().context("no size metrics")?;
        let cell_height = (metrics.height >> 6) as usize;
        let baseline = (-metrics.descender >> 6) as usize;
        let cell_width = measure_cell_width(&face).context("failed to measure cell width")?;

        let fallback_paths: Vec<String> = find_emoji_font_paths()?
            .into_iter()
            .filter(|fallback| fallback != path)
            .collect();

        Ok(Self {
            glyphs: HashMap::with_capacity(128),
            grapheme_glyphs: HashMap::with_capacity(32),
            primary_face: Some(face),
            fallback_faces: Vec::new(),
            fallback_paths,
            fallback_loaded: false,
            font_size_pt,
            dpi,
            glyph_sources: HashMap::with_capacity(128),
            missing_glyphs: HashSet::with_capacity(32),
            font_path: path.to_string(),
            cell_width: cell_width.max(1),
            cell_height: cell_height.max(1),
            baseline,
            #[cfg(feature = "ligatures")]
            rb_face: init_rustybuzz(path),
            #[cfg(feature = "ligatures")]
            fallback_rb_faces: Vec::new(),
            #[cfg(feature = "ligatures")]
            fallback_rb_loaded: false,
        })
    }

    #[cfg(not(feature = "local-fonts"))]
    pub fn from_font_path_dpi(_path: &str, _font_size_pt: f64, _dpi: u32) -> Result<Self> {
        anyhow::bail!("local font loading is disabled in this build")
    }

    pub fn protocol_only(metrics: CellMetrics) -> Self {
        Self {
            glyphs: HashMap::with_capacity(128),
            grapheme_glyphs: HashMap::with_capacity(32),
            #[cfg(feature = "local-fonts")]
            primary_face: None,
            #[cfg(feature = "local-fonts")]
            fallback_faces: Vec::new(),
            #[cfg(feature = "local-fonts")]
            fallback_paths: Vec::new(),
            fallback_loaded: true,
            font_size_pt: 0.0,
            dpi: 96,
            #[cfg(feature = "local-fonts")]
            glyph_sources: HashMap::with_capacity(0),
            missing_glyphs: HashSet::with_capacity(0),
            font_path: String::new(),
            cell_width: usize::from(metrics.cell_width.max(1)),
            cell_height: usize::from(metrics.cell_height.max(1)),
            baseline: usize::from(metrics.baseline.max(1)),
            #[cfg(feature = "ligatures")]
            rb_face: None,
            #[cfg(feature = "ligatures")]
            fallback_rb_faces: Vec::new(),
            #[cfg(feature = "ligatures")]
            fallback_rb_loaded: true,
        }
    }

    pub fn ensure_glyph(&mut self, ch: u32) -> bool {
        if self.glyphs.contains_key(&ch) {
            return true;
        }
        if self.missing_glyphs.contains(&ch) {
            return false;
        }

        if let Some(glyph) = procedural_glyph(ch, self.cell_width, self.cell_height, self.baseline)
        {
            self.glyphs.insert(ch, glyph);
            #[cfg(feature = "local-fonts")]
            self.glyph_sources.insert(ch, GlyphSource::Primary);
            return true;
        }

        #[cfg(feature = "local-fonts")]
        if let Some(source) = self.glyph_sources.get(&ch).copied()
            && let Some(glyph) = self.rasterize_from_source(source, ch)
        {
            self.glyphs.insert(ch, glyph);
            return true;
        }

        #[cfg(feature = "local-fonts")]
        if let Some(glyph) = self
            .primary_face
            .as_ref()
            .and_then(|face| rasterize_primary_glyph(face, ch))
        {
            self.glyphs.insert(ch, glyph);
            #[cfg(feature = "local-fonts")]
            self.glyph_sources.insert(ch, GlyphSource::Primary);
            return true;
        }

        #[cfg(feature = "local-fonts")]
        if should_try_emoji_fallback(ch) {
            self.ensure_fallback_faces_loaded();
            for (index, face) in self.fallback_faces.iter().enumerate() {
                if let Some(glyph) = face
                    .as_ref()
                    .and_then(|face| rasterize_fallback_glyph(face, ch))
                {
                    self.glyphs.insert(ch, glyph);
                    #[cfg(feature = "local-fonts")]
                    self.glyph_sources.insert(ch, GlyphSource::Fallback(index));
                    return true;
                }
            }
        }

        self.missing_glyphs.insert(ch);
        false
    }

    pub fn ensure_grapheme(&mut self, grapheme: &str) -> bool {
        if self.grapheme_glyphs.contains_key(grapheme) {
            return true;
        }
        if grapheme.chars().count() == 1
            && let Some(ch) = grapheme.chars().next()
        {
            return self.ensure_glyph(ch as u32);
        }

        #[cfg(feature = "ligatures")]
        {
            if let Some(glyph) = self.rasterize_grapheme_cluster(grapheme) {
                self.grapheme_glyphs.insert(grapheme.into(), glyph);
                return true;
            }
        }

        if let Some(glyph) = self.rasterize_grapheme_fallback(grapheme) {
            self.grapheme_glyphs.insert(grapheme.into(), glyph);
            return true;
        }

        false
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_bg(
        &self,
        buffer: &mut [u32],
        buf_w: usize,
        buf_h: usize,
        cell_x: usize,
        cell_y: usize,
        bg: u32,
    ) {
        let px_x = cell_x * self.cell_width;
        let px_y = cell_y * self.cell_height;
        let x_end = (px_x + self.cell_width).min(buf_w);
        let y_end = (px_y + self.cell_height).min(buf_h);

        for y in px_y..y_end {
            let row_start = y * buf_w + px_x;
            let row_end = y * buf_w + x_end;
            buffer[row_start..row_end].fill(bg);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_glyph(
        &mut self,
        buffer: &mut [u32],
        buf_w: usize,
        buf_h: usize,
        cell_x: usize,
        cell_y: usize,
        ch: u32,
        fg: u32,
    ) {
        self.ensure_glyph(ch);

        let Some(glyph) = self.glyphs.get(&ch) else {
            return;
        };
        self.draw_rasterized_glyph(buffer, buf_w, buf_h, cell_x, cell_y, glyph, fg);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_rasterized_glyph(
        &self,
        buffer: &mut [u32],
        buf_w: usize,
        buf_h: usize,
        cell_x: usize,
        cell_y: usize,
        glyph: &RasterizedGlyph,
        fg: u32,
    ) {
        let px_x = cell_x * self.cell_width;
        let px_y = cell_y * self.cell_height;
        let ch_height = self.cell_height;

        let origin_y = px_y as i32 + (ch_height as i32 - self.baseline as i32);
        let glyph_top = origin_y - glyph.bearing_y;
        let glyph_left = px_x as i32 + glyph.bearing_x;

        let gy_start = if glyph_top < 0 {
            (-glyph_top) as usize
        } else {
            0
        };
        let gy_end = glyph
            .height
            .min(((buf_h as i32) - glyph_top).max(0) as usize);

        let gx_start = if glyph_left < 0 {
            (-glyph_left) as usize
        } else {
            0
        };
        let gx_end = glyph
            .width
            .min(((buf_w as i32) - glyph_left).max(0) as usize);

        for gy in gy_start..gy_end {
            let sy = (glyph_top + gy as i32) as usize;
            let bmp_row = gy * glyph.width;
            let screen_row = sy * buf_w;

            for gx in gx_start..gx_end {
                let sx = (glyph_left + gx as i32) as usize;
                let pixel = &mut buffer[screen_row + sx];
                match glyph.format {
                    GlyphFormat::Alpha => {
                        blend_alpha_pixel(pixel, fg, glyph.pixels[bmp_row + gx] as u32)
                    }
                    GlyphFormat::Rgba => {
                        let offset = (bmp_row + gx) * 4;
                        blend_rgba_pixel(pixel, &glyph.pixels[offset..offset + 4]);
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_grapheme(
        &mut self,
        buffer: &mut [u32],
        buf_w: usize,
        buf_h: usize,
        cell_x: usize,
        cell_y: usize,
        grapheme: &str,
        fg: u32,
    ) {
        if grapheme.chars().count() == 1
            && let Some(ch) = grapheme.chars().next()
        {
            self.draw_glyph(buffer, buf_w, buf_h, cell_x, cell_y, ch as u32, fg);
            return;
        }

        self.ensure_grapheme(grapheme);
        let Some(glyph) = self.grapheme_glyphs.get(grapheme) else {
            return;
        };
        self.draw_rasterized_glyph(buffer, buf_w, buf_h, cell_x, cell_y, glyph, fg);
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
        self.draw_bg(buffer, buf_w, buf_h, cell_x, cell_y, bg);
        if ch > 0x20 {
            self.draw_glyph(buffer, buf_w, buf_h, cell_x, cell_y, ch, fg);
        }
    }

    #[allow(dead_code)]
    pub fn has_glyph(&self, ch: u32) -> bool {
        self.glyphs.contains_key(&ch)
    }

    pub fn get_glyph(&self, ch: u32) -> Option<GlyphData<'_>> {
        self.glyphs.get(&ch).map(|g| GlyphData {
            pixels: &g.pixels,
            width: g.width,
            height: g.height,
            format: g.format,
            bearing_x: g.bearing_x,
            bearing_y: g.bearing_y,
        })
    }

    pub fn get_grapheme_glyph(&self, grapheme: &str) -> Option<GlyphData<'_>> {
        if grapheme.chars().count() == 1
            && let Some(ch) = grapheme.chars().next()
        {
            return self.get_glyph(ch as u32);
        }

        self.grapheme_glyphs.get(grapheme).map(|g| GlyphData {
            pixels: &g.pixels,
            width: g.width,
            height: g.height,
            format: g.format,
            bearing_x: g.bearing_x,
            bearing_y: g.bearing_y,
        })
    }

    pub fn insert_protocol_glyph(&mut self, glyph: &GlyphBitmap) {
        let rasterized = RasterizedGlyph {
            pixels: glyph.pixels.clone(),
            width: glyph.width as usize,
            height: glyph.height as usize,
            format: if glyph.is_color {
                GlyphFormat::Rgba
            } else {
                GlyphFormat::Alpha
            },
            bearing_x: glyph.bearing_x as i32,
            bearing_y: glyph.bearing_y as i32,
            advance: i32::from(glyph.cells.max(1)) * self.cell_width as i32,
        };

        if let Some(grapheme) = &glyph.grapheme {
            self.grapheme_glyphs
                .insert(grapheme.clone().into_boxed_str(), rasterized);
        } else {
            self.glyphs.insert(glyph.glyph_id, rasterized);
            self.missing_glyphs.remove(&glyph.glyph_id);
        }
    }

    pub fn drop_font_sources(&mut self) {
        #[cfg(feature = "local-fonts")]
        {
            self.primary_face = None;
            self.fallback_faces.clear();
            self.fallback_paths.clear();
        }
        self.fallback_loaded = true;
        #[cfg(feature = "ligatures")]
        {
            self.rb_face = None;
            self.fallback_rb_faces.clear();
            self.fallback_rb_loaded = true;
        }
    }

    #[cfg(feature = "ligatures")]
    pub fn shape_run(&mut self, text: &str) -> Vec<ShapedGlyph> {
        let Some(ref face) = self.rb_face else {
            return text
                .chars()
                .map(|ch| ShapedGlyph {
                    codepoint: ch as u32,
                    cluster: 0,
                    cells: if unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1) > 1 {
                        2
                    } else {
                        1
                    },
                })
                .collect();
        };

        let mut buffer = rustybuzz::UnicodeBuffer::new();
        buffer.push_str(text);
        let output = rustybuzz::shape(face, &[], buffer);
        let info = output.glyph_infos();
        let _positions = output.glyph_positions();

        let mut result = Vec::with_capacity(info.len());
        for (i, gi) in info.iter().enumerate() {
            let cluster_start = gi.cluster as usize;
            let cluster_end = if i + 1 < info.len() {
                info[i + 1].cluster as usize
            } else {
                text.len()
            };

            let cluster_str = &text[cluster_start..cluster_end.min(text.len())];
            let cells: usize = cluster_str
                .chars()
                .map(|c| unicode_width::UnicodeWidthChar::width(c).unwrap_or(1))
                .sum();
            let cells = cells.max(1);

            self.ensure_glyph_id(gi.glyph_id);

            result.push(ShapedGlyph {
                codepoint: gi.glyph_id,
                cluster: gi.cluster,
                cells,
            });
        }
        result
    }

    #[cfg(feature = "ligatures")]
    fn ensure_glyph_id(&mut self, glyph_id: u32) -> bool {
        if self.glyphs.contains_key(&(glyph_id | 0x8000_0000)) {
            return true;
        }

        if let Some(primary_face) = self.primary_face.as_ref()
            && primary_face.load_glyph(glyph_id, text_load_flags()).is_ok()
            && let Some(g) = rasterize_loaded(primary_face)
        {
            self.glyphs.insert(glyph_id | 0x8000_0000, g);
            return true;
        }
        false
    }

    #[cfg(feature = "ligatures")]
    pub fn get_shaped_glyph(&self, glyph_id: u32) -> Option<GlyphData<'_>> {
        self.glyphs
            .get(&(glyph_id | 0x8000_0000))
            .map(|g| GlyphData {
                pixels: &g.pixels,
                width: g.width,
                height: g.height,
                format: g.format,
                bearing_x: g.bearing_x,
                bearing_y: g.bearing_y,
            })
    }

    #[cfg(feature = "ligatures")]
    #[allow(clippy::too_many_arguments)]
    pub fn draw_shaped_glyph(
        &mut self,
        buffer: &mut [u32],
        buf_w: usize,
        buf_h: usize,
        cell_x: usize,
        cell_y: usize,
        glyph_id: u32,
        cells: usize,
        fg: u32,
    ) {
        self.ensure_glyph_id(glyph_id);

        let key = glyph_id | 0x8000_0000;
        let Some(glyph) = self.glyphs.get(&key) else {
            return;
        };

        let px_x = cell_x * self.cell_width;
        let px_y = cell_y * self.cell_height;
        let ch_height = self.cell_height;
        let span_width = cells * self.cell_width;

        let origin_y = px_y as i32 + (ch_height as i32 - self.baseline as i32);
        let glyph_top = origin_y - glyph.bearing_y;
        let glyph_left = px_x as i32 + glyph.bearing_x;

        let gy_start = if glyph_top < 0 {
            (-glyph_top) as usize
        } else {
            0
        };
        let gy_end = glyph
            .height
            .min(((buf_h as i32) - glyph_top).max(0) as usize);
        let gx_start = if glyph_left < 0 {
            (-glyph_left) as usize
        } else {
            0
        };
        let gx_end = glyph
            .width
            .min(((buf_w as i32) - glyph_left).max(0) as usize);
        let _ = span_width;

        for gy in gy_start..gy_end {
            let sy = (glyph_top + gy as i32) as usize;
            let bmp_row = gy * glyph.width;
            let screen_row = sy * buf_w;

            for gx in gx_start..gx_end {
                let sx = (glyph_left + gx as i32) as usize;
                let pixel = &mut buffer[screen_row + sx];
                match glyph.format {
                    GlyphFormat::Alpha => {
                        blend_alpha_pixel(pixel, fg, glyph.pixels[bmp_row + gx] as u32)
                    }
                    GlyphFormat::Rgba => {
                        let offset = (bmp_row + gx) * 4;
                        blend_rgba_pixel(pixel, &glyph.pixels[offset..offset + 4]);
                    }
                }
            }
        }
    }

    #[cfg(feature = "local-fonts")]
    fn rasterize_from_source(&self, source: GlyphSource, ch: u32) -> Option<RasterizedGlyph> {
        match source {
            GlyphSource::Primary => self
                .primary_face
                .as_ref()
                .and_then(|face| rasterize_primary_glyph(face, ch)),
            GlyphSource::Fallback(index) => self
                .fallback_faces
                .get(index)
                .and_then(|face| face.as_ref())
                .and_then(|face| rasterize_fallback_glyph(face, ch)),
        }
    }

    fn rasterize_grapheme_fallback(&mut self, grapheme: &str) -> Option<RasterizedGlyph> {
        let ch = grapheme.chars().find(|ch| !ch.is_control())? as u32;

        let glyph = procedural_glyph(ch, self.cell_width, self.cell_height, self.baseline);
        #[cfg(feature = "local-fonts")]
        let glyph = glyph
            .or_else(|| {
                self.primary_face
                    .as_ref()
                    .and_then(|face| rasterize_primary_glyph(face, ch))
            })
            .or_else(|| {
                if should_try_emoji_fallback(ch) {
                    self.ensure_fallback_faces_loaded();
                    self.fallback_faces
                        .iter()
                        .filter_map(|face| face.as_ref())
                        .find_map(|face| rasterize_fallback_glyph(face, ch))
                } else {
                    None
                }
            });
        glyph
    }

    #[cfg(feature = "ligatures")]
    fn rasterize_grapheme_cluster(&mut self, grapheme: &str) -> Option<RasterizedGlyph> {
        self.ensure_fallback_rb_faces_loaded();
        self.rb_face
            .as_ref()
            .zip(self.primary_face.as_ref())
            .and_then(|(rb_face, primary_face)| {
                rasterize_grapheme_sequence(
                    primary_face,
                    rb_face,
                    grapheme,
                    text_load_flags(),
                    self.cell_height,
                    self.baseline,
                )
            })
            .or_else(|| {
                self.fallback_faces
                    .iter()
                    .zip(self.fallback_rb_faces.iter())
                    .find_map(|(face, rb_face)| {
                        face.as_ref()
                            .zip(rb_face.as_ref())
                            .and_then(|(face, rb_face)| {
                                rasterize_grapheme_sequence(
                                    face,
                                    rb_face,
                                    grapheme,
                                    LoadFlag::RENDER | LoadFlag::COLOR,
                                    self.cell_height,
                                    self.baseline,
                                )
                            })
                    })
            })
    }

    #[cfg(feature = "local-fonts")]
    fn ensure_fallback_faces_loaded(&mut self) {
        if self.fallback_loaded {
            return;
        }

        self.fallback_faces = self
            .fallback_paths
            .iter()
            .map(|path| load_fallback_face(path, self.font_size_pt, self.dpi))
            .collect();
        self.fallback_loaded = true;
    }

    #[cfg(not(feature = "local-fonts"))]
    fn ensure_fallback_faces_loaded(&mut self) {
        self.fallback_loaded = true;
    }

    #[cfg(feature = "ligatures")]
    fn ensure_fallback_rb_faces_loaded(&mut self) {
        if self.fallback_rb_loaded {
            return;
        }

        self.fallback_rb_faces = self
            .fallback_paths
            .iter()
            .map(|path| init_rustybuzz(path))
            .collect();
        self.fallback_rb_loaded = true;
    }
}

#[cfg(feature = "ligatures")]
pub struct ShapedGlyph {
    pub codepoint: u32,
    #[allow(dead_code)]
    pub cluster: u32,
    pub cells: usize,
}

#[cfg(feature = "local-fonts")]
fn rasterize_primary_glyph(face: &freetype::Face, ch: u32) -> Option<RasterizedGlyph> {
    rasterize_char(face, ch, text_load_flags())
}

#[cfg(feature = "local-fonts")]
fn rasterize_fallback_glyph(face: &freetype::Face, ch: u32) -> Option<RasterizedGlyph> {
    rasterize_char(face, ch, LoadFlag::RENDER | LoadFlag::COLOR)
}

#[cfg(feature = "local-fonts")]
fn load_fallback_face(path: &str, font_size_pt: f64, dpi: u32) -> Option<Face> {
    let lib = freetype::Library::init().ok()?;
    let face = lib.new_face(path, 0).ok()?;
    face.set_char_size((font_size_pt * 64.0) as isize, 0, dpi, 0)
        .ok()?;
    Some(face)
}

#[cfg(feature = "local-fonts")]
fn text_load_flags() -> LoadFlag {
    LoadFlag::RENDER | LoadFlag::TARGET_LIGHT
}

#[cfg(feature = "local-fonts")]
fn measure_cell_width(face: &freetype::Face) -> Result<usize> {
    let mut max_advance = 0usize;

    for ch in CELL_WIDTH_SAMPLE_TEXT.chars() {
        face.load_char(ch as usize, text_load_flags())
            .with_context(|| format!("failed to load sample glyph U+{:04X}", ch as u32))?;
        max_advance = max_advance.max((face.glyph().advance().x >> 6) as usize);
    }

    Ok(max_advance.max(1))
}

#[cfg(feature = "local-fonts")]
fn rasterize_char(face: &freetype::Face, ch: u32, flags: LoadFlag) -> Option<RasterizedGlyph> {
    face.load_char(ch as usize, flags).ok()?;
    rasterize_loaded(face)
}

#[cfg(feature = "ligatures")]
fn rasterize_grapheme_sequence(
    face: &freetype::Face,
    rb_face: &rustybuzz::Face<'static>,
    grapheme: &str,
    flags: LoadFlag,
    cell_height: usize,
    baseline: usize,
) -> Option<RasterizedGlyph> {
    let mut buffer = rustybuzz::UnicodeBuffer::new();
    buffer.push_str(grapheme);
    let output = rustybuzz::shape(rb_face, &[], buffer);
    let infos = output.glyph_infos();
    let positions = output.glyph_positions();
    if infos.is_empty() || infos.iter().any(|info| info.glyph_id == 0) {
        return None;
    }

    let origin_y = cell_height as i32 - baseline as i32;
    let mut pen_x = 0i32;
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;
    let mut any_color = false;

    for (info, pos) in infos.iter().zip(positions.iter()) {
        face.load_glyph(info.glyph_id, flags).ok()?;
        let glyph = face.glyph();
        let bmp = glyph.bitmap();
        let width = bmp.width();
        let height = bmp.rows();
        let x = pen_x + (pos.x_offset >> 6) + glyph.bitmap_left();
        let y = origin_y - (pos.y_offset >> 6) - glyph.bitmap_top();
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x + width);
        max_y = max_y.max(y + height);
        any_color |= matches!(bmp.pixel_mode().ok()?, PixelMode::Bgra);
        pen_x += pos.x_advance >> 6;
    }

    if min_x == i32::MAX || min_y == i32::MAX || max_x <= min_x || max_y <= min_y {
        return None;
    }

    let width = (max_x - min_x) as usize;
    let height = (max_y - min_y) as usize;
    let format = if any_color {
        GlyphFormat::Rgba
    } else {
        GlyphFormat::Alpha
    };
    let mut pixels = if format == GlyphFormat::Rgba {
        vec![0u8; width * height * 4]
    } else {
        vec![0u8; width * height]
    };

    pen_x = 0;
    for (info, pos) in infos.iter().zip(positions.iter()) {
        face.load_glyph(info.glyph_id, flags).ok()?;
        let glyph = face.glyph();
        let bmp = glyph.bitmap();
        let bmp_width = bmp.width() as usize;
        let bmp_height = bmp.rows() as usize;
        if bmp_width == 0 || bmp_height == 0 {
            pen_x += pos.x_advance >> 6;
            continue;
        }

        let pitch = bmp.pitch().unsigned_abs() as usize;
        let (src_pixels, src_format) = copy_bitmap(
            bmp.buffer(),
            bmp_width,
            bmp_height,
            pitch,
            bmp.pixel_mode().ok()?,
        )?;
        let x = pen_x + (pos.x_offset >> 6) + glyph.bitmap_left() - min_x;
        let y = origin_y - (pos.y_offset >> 6) - glyph.bitmap_top() - min_y;

        for gy in 0..bmp_height {
            let dst_y = y + gy as i32;
            if !(0..height as i32).contains(&dst_y) {
                continue;
            }
            for gx in 0..bmp_width {
                let dst_x = x + gx as i32;
                if !(0..width as i32).contains(&dst_x) {
                    continue;
                }
                match (format, src_format) {
                    (GlyphFormat::Alpha, GlyphFormat::Alpha) => {
                        let dst = dst_y as usize * width + dst_x as usize;
                        pixels[dst] = pixels[dst].max(src_pixels[gy * bmp_width + gx]);
                    }
                    _ => {
                        let dst = (dst_y as usize * width + dst_x as usize) * 4;
                        match src_format {
                            GlyphFormat::Alpha => {
                                let alpha = src_pixels[gy * bmp_width + gx];
                                let rgba = [255, 255, 255, alpha];
                                blend_rgba_bytes(&mut pixels[dst..dst + 4], &rgba);
                            }
                            GlyphFormat::Rgba => {
                                let src = (gy * bmp_width + gx) * 4;
                                blend_rgba_bytes(
                                    &mut pixels[dst..dst + 4],
                                    &src_pixels[src..src + 4],
                                );
                            }
                        }
                    }
                }
            }
        }

        pen_x += pos.x_advance >> 6;
    }

    Some(RasterizedGlyph {
        pixels,
        width,
        height,
        format,
        bearing_x: min_x,
        bearing_y: origin_y - min_y,
        advance: pen_x,
    })
}

fn procedural_glyph(
    ch: u32,
    cell_width: usize,
    cell_height: usize,
    baseline: usize,
) -> Option<RasterizedGlyph> {
    let width = cell_width.max(1);
    let height = cell_height.max(1);
    let mut pixels = vec![0u8; width * height];

    match ch {
        0x2588 => pixels.fill(255),
        0x2580 => {
            for y in 0..height.div_ceil(2) {
                pixels[y * width..(y + 1) * width].fill(255);
            }
        }
        0x2584 => {
            for y in height / 2..height {
                pixels[y * width..(y + 1) * width].fill(255);
            }
        }
        0x258C => {
            let fill = width.div_ceil(2);
            for y in 0..height {
                pixels[y * width..y * width + fill].fill(255);
            }
        }
        0x2590 => {
            let fill_start = width / 2;
            for y in 0..height {
                pixels[y * width + fill_start..(y + 1) * width].fill(255);
            }
        }
        0x2591..=0x2593 => {
            let threshold = match ch {
                0x2591 => 4,
                0x2592 => 8,
                0x2593 => 12,
                _ => unreachable!(),
            };
            const BAYER_4X4: [[u8; 4]; 4] =
                [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
            for y in 0..height {
                for x in 0..width {
                    if BAYER_4X4[y % 4][x % 4] < threshold {
                        pixels[y * width + x] = 255;
                    }
                }
            }
        }
        _ => return None,
    }

    Some(RasterizedGlyph {
        pixels,
        width,
        height,
        format: GlyphFormat::Alpha,
        bearing_x: 0,
        bearing_y: (height.saturating_sub(baseline)) as i32,
        advance: width as i32,
    })
}

#[cfg(feature = "local-fonts")]
fn rasterize_loaded(face: &freetype::Face) -> Option<RasterizedGlyph> {
    let glyph = face.glyph();
    let bmp = glyph.bitmap();
    let width = bmp.width() as usize;
    let height = bmp.rows() as usize;
    let pitch = bmp.pitch().unsigned_abs() as usize;
    let buffer = bmp.buffer();
    let pixel_mode = bmp.pixel_mode().ok()?;

    let (pixels, format) = copy_bitmap(buffer, width, height, pitch, pixel_mode)?;

    Some(RasterizedGlyph {
        pixels,
        width,
        height,
        format,
        bearing_x: glyph.bitmap_left(),
        bearing_y: glyph.bitmap_top(),
        advance: (glyph.advance().x >> 6) as i32,
    })
}

#[cfg(feature = "local-fonts")]
fn copy_bitmap(
    buffer: &[u8],
    width: usize,
    height: usize,
    pitch: usize,
    pixel_mode: PixelMode,
) -> Option<(Vec<u8>, GlyphFormat)> {
    match pixel_mode {
        PixelMode::Gray => {
            let mut pixels = vec![0u8; width * height];
            for y in 0..height {
                let src = &buffer[y * pitch..y * pitch + width];
                let dst = &mut pixels[y * width..(y + 1) * width];
                dst.copy_from_slice(src);
            }
            Some((pixels, GlyphFormat::Alpha))
        }
        PixelMode::Mono => {
            let mut pixels = vec![0u8; width * height];
            for y in 0..height {
                let row = &buffer[y * pitch..(y + 1) * pitch];
                for x in 0..width {
                    let byte = row[x / 8];
                    let bit = 7 - (x % 8);
                    pixels[y * width + x] = if (byte >> bit) & 1 == 1 { 255 } else { 0 };
                }
            }
            Some((pixels, GlyphFormat::Alpha))
        }
        PixelMode::Bgra => {
            let mut pixels = vec![0u8; width * height * 4];
            for y in 0..height {
                let src = &buffer[y * pitch..y * pitch + width * 4];
                let dst = &mut pixels[y * width * 4..(y + 1) * width * 4];
                for x in 0..width {
                    let src_offset = x * 4;
                    let dst_offset = x * 4;
                    dst[dst_offset] = src[src_offset + 2];
                    dst[dst_offset + 1] = src[src_offset + 1];
                    dst[dst_offset + 2] = src[src_offset];
                    dst[dst_offset + 3] = src[src_offset + 3];
                }
            }
            Some((pixels, GlyphFormat::Rgba))
        }
        _ => None,
    }
}

fn blend_alpha_pixel(pixel: &mut u32, fg: u32, alpha: u32) {
    if alpha == 0 {
        return;
    }
    if alpha == 255 {
        *pixel = fg;
        return;
    }

    let fg_r = (fg >> 16) & 0xff;
    let fg_g = (fg >> 8) & 0xff;
    let fg_b = fg & 0xff;
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

fn blend_rgba_pixel(pixel: &mut u32, rgba: &[u8]) {
    blend_rgba_over_rgb(pixel, rgba);
}

fn blend_rgba_bytes(dst: &mut [u8], rgba: &[u8]) {
    blend_rgba_over_rgba(dst, rgba);
}

#[cfg(feature = "ligatures")]
fn init_rustybuzz(font_path: &str) -> Option<rustybuzz::Face<'static>> {
    let font_data = std::fs::read(font_path).ok()?;
    let font_data: &'static [u8] = Box::leak(font_data.into_boxed_slice());
    rustybuzz::Face::from_slice(font_data, 0)
}

#[cfg(feature = "local-fonts")]
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

#[cfg(feature = "local-fonts")]
fn find_emoji_font_paths() -> Result<Vec<String>> {
    let fc = fontconfig::Fontconfig::new().context("failed to init fontconfig")?;
    let mut paths = Vec::new();
    let mut seen = HashSet::new();

    for family in [
        "Noto Color Emoji",
        "Noto Emoji",
        "Apple Color Emoji",
        "Segoe UI Emoji",
    ] {
        if let Some(font) = fc.find(family, None)
            && let Some(path) = font.path.to_str()
            && seen.insert(path.to_string())
        {
            paths.push(path.to_string());
        }
    }

    Ok(paths)
}

fn should_try_emoji_fallback(ch: u32) -> bool {
    matches!(
        ch,
        0x00A9
            | 0x00AE
            | 0x203C
            | 0x2049
            | 0x2122
            | 0x2139
            | 0x3030
            | 0x303D
            | 0x3297
            | 0x3299
            | 0xFE0F
            | 0x1F004
            | 0x1F0CF
            | 0x1F18E
    ) || (0x2194..=0x21FF).contains(&ch)
        || (0x2300..=0x23FF).contains(&ch)
        || (0x2460..=0x24FF).contains(&ch)
        || (0x25A0..=0x27BF).contains(&ch)
        || (0x2B00..=0x2BFF).contains(&ch)
        || (0x1F000..=0x1FAFF).contains(&ch)
}

fn font_cache_path() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".cache").join("handterm").join("font_path"))
}

fn load_cached_font_path(family: &str) -> Option<String> {
    let cache = font_cache_path()?;
    let content = std::fs::read_to_string(&cache).ok()?;
    for line in content.lines() {
        if let Some((k, v)) = line.split_once('=')
            && k == family
        {
            return Some(v.to_string());
        }
    }
    None
}

fn save_cached_font_path(family: &str, path: &str) {
    let Some(cache) = font_cache_path() else {
        return;
    };
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
    #[cfg(feature = "local-fonts")]
    fn loads_system_monospace_font() {
        let mut atlas = GlyphAtlas::new(14.0).expect("should load a monospace font");
        assert!(atlas.cell_width > 0);
        assert!(atlas.cell_height > 0);
        assert!(atlas.ensure_glyph(b'A' as u32));
        assert!(atlas.ensure_glyph(b'@' as u32));
    }

    #[test]
    #[cfg(feature = "local-fonts")]
    fn renders_glyph_to_buffer() {
        let mut atlas = GlyphAtlas::new(14.0).unwrap();
        let w = atlas.cell_width * 2;
        let h = atlas.cell_height * 2;
        let mut buf = vec![0u32; w * h];
        atlas.draw_char(&mut buf, w, h, 0, 0, b'A' as u32, 0xffffff, 0x000000);
        let non_black = buf.iter().filter(|&&p| p != 0x000000).count();
        assert!(non_black > 0, "glyph 'A' should have visible pixels");
    }

    #[test]
    #[cfg(feature = "local-fonts")]
    fn converts_bgra_bitmap_to_rgba() {
        let bgra = [0x33, 0x22, 0x11, 0x80];
        let (pixels, format) = copy_bitmap(&bgra, 1, 1, 4, PixelMode::Bgra).unwrap();
        assert_eq!(format, GlyphFormat::Rgba);
        assert_eq!(pixels, vec![0x11, 0x22, 0x33, 0x80]);
    }

    #[test]
    fn only_emoji_ranges_use_fallback_path() {
        assert!(should_try_emoji_fallback('😀' as u32));
        assert!(should_try_emoji_fallback('❤' as u32));
        assert!(!should_try_emoji_fallback('A' as u32));
        assert!(!should_try_emoji_fallback('é' as u32));
    }

    #[test]
    fn procedural_shade_glyphs_use_full_cell_metrics() {
        let atlas = GlyphAtlas::new(14.0).expect("should load a monospace font");
        let glyph = procedural_glyph(0x2591, atlas.cell_width, atlas.cell_height, atlas.baseline)
            .expect("light shade should be procedural");
        assert_eq!(glyph.width, atlas.cell_width);
        assert_eq!(glyph.height, atlas.cell_height);
        assert!(glyph.pixels.contains(&255));
        assert!(glyph.pixels.contains(&0));
    }

    #[test]
    fn procedural_block_glyphs_fill_expected_regions() {
        let glyph =
            procedural_glyph(0x258C, 8, 4, 1).expect("left half block should be procedural");
        for y in 0..4 {
            for x in 0..8 {
                let filled = glyph.pixels[y * 8 + x] == 255;
                assert_eq!(filled, x < 4, "unexpected left-half fill at ({x}, {y})");
            }
        }
    }

    #[test]
    #[cfg(feature = "local-fonts")]
    fn grapheme_clusters_cache_as_single_glyphs() {
        let mut atlas = GlyphAtlas::new(14.0).expect("should load a monospace font");
        if !atlas.ensure_grapheme("❤️") {
            return;
        }
        let glyph = atlas
            .get_grapheme_glyph("❤️")
            .expect("cached grapheme glyph should exist");
        assert!(glyph.width > 0);
        assert!(glyph.height > 0);
    }

    #[test]
    #[cfg(feature = "local-fonts")]
    fn grapheme_fallback_draws_visible_pixels_without_cluster_shape() {
        let mut atlas = GlyphAtlas::new(14.0).expect("should load a monospace font");
        let w = atlas.cell_width;
        let h = atlas.cell_height;
        let mut buf = vec![0u32; w * h];

        atlas.draw_grapheme(&mut buf, w, h, 0, 0, "👍🏻", 0xffffff);

        assert!(
            buf.iter().any(|&pixel| pixel != 0),
            "grapheme fallback should draw visible pixels"
        );
    }

    #[test]
    fn protocol_only_atlas_can_store_and_render_color_emoji_grapheme() {
        let metrics = CellMetrics {
            cell_width: 8,
            cell_height: 8,
            baseline: 6,
        };
        let mut atlas = GlyphAtlas::protocol_only(metrics);
        atlas.insert_protocol_glyph(&GlyphBitmap {
            glyph_id: '❤' as u32,
            grapheme: Some("❤️".to_string()),
            width: 2,
            height: 2,
            bearing_x: 0,
            bearing_y: 2,
            cells: 1,
            is_color: true,
            pixels: vec![
                255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255,
            ],
        });

        let mut buf = vec![0u32; atlas.cell_width * atlas.cell_height];
        atlas.draw_grapheme(
            &mut buf,
            atlas.cell_width,
            atlas.cell_height,
            0,
            0,
            "❤️",
            0xffffff,
        );

        assert!(buf.contains(&0xff0000));
    }

    #[test]
    fn rgba_glyph_pixels_alpha_blend_against_existing_rgb() {
        let mut pixel = 0x204060;
        blend_rgba_pixel(&mut pixel, &[0xff, 0x00, 0x00, 0x80]);
        assert_eq!(pixel, 0x8f1f2f);
    }
}
