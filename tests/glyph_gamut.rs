//! Wide-gamut glyph rendering probe.
//!
//! Diagnostic integration test that feeds a terminal grid a broad range of
//! content (ASCII, accents, combining marks, box drawing, braille, powerline,
//! nerd-font icons, CJK, emoji, ZWJ sequences, flags, keycaps, ...) and
//! verifies at the pixel level, per category, that:
//!
//! (a) non-background pixels are drawn inside the expected cell span,
//! (b) wide glyphs occupy exactly the expected two-cell span
//!     (`FLAG_WIDE` head + `FLAG_WIDE_CONT` continuation),
//! (c) no glyph renders as the notdef/tofu box: every cell is compared
//!     against the raster of a known-missing codepoint (U+10FFFD), and
//!     visually distinct samples in a category must not raster identically,
//! (d) following text stays column-aligned: every sample's head cell must
//!     land at its predicted column, and a trailing `|` sentinel raster is
//!     compared pixel-for-pixel against an isolated `|` raster.
//!
//! The probe runs at 96 dpi (1.0x) and 192 dpi (2.0x). Failures are collected
//! across all categories and DPIs and reported together with pixel evidence.

#![cfg(feature = "local-fonts")]

use handterm::config::AppConfig;
use handterm::font::GlyphAtlas;
use handterm::grid::{FLAG_WIDE, FLAG_WIDE_CONT};
use handterm::render::OffscreenRenderer;
use handterm::terminal::Terminal;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Base of a pool of codepoints no font on the system should map (plane-16
/// private use). `font::tests::missing_codepoint_does_not_resolve_to_notdef_box`
/// guarantees the atlas refuses to resolve such codepoints through `.notdef`.
/// Each category consumes its own codepoint from the pool because the atlas
/// caches per-codepoint results (including negative ones), and the tofu
/// reference must reflect the atlas fallback state at the time the category
/// renders.
const MISSING_CODEPOINT_BASE: u32 = 0x10FF80;
/// Two-space separator between samples so horizontal glyph overhang from one
/// sample cannot contaminate its neighbor's pixel band.
const SEPARATOR: &str = "  ";
/// Sentinel appended after each category row to verify column alignment.
const SENTINEL: char = '|';
/// Content is placed on the middle row of a three-row grid so marks drawn
/// above the cell (combining accents) and below it (descenders) still land
/// inside the captured pixel band.
const CONTENT_ROW: usize = 1;
const GRID_ROWS: u16 = 3;
/// (dpi, human-readable scale) pairs the probe runs at.
const DPI_SCALES: &[(u32, &str)] = &[(96, "1.0x"), (192, "2.0x")];

struct Category {
    name: &'static str,
    samples: Vec<String>,
}

fn owned(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn categories() -> Vec<Category> {
    vec![
        Category {
            name: "ascii_printable",
            samples: ('!'..='~').map(String::from).collect(),
        },
        Category {
            name: "latin1_accents",
            samples: owned(&["é", "à", "ü", "ñ", "ç", "Ä", "ß"]),
        },
        Category {
            // Includes the bare base characters: if a combining mark is
            // dropped, the pairwise-identical raster check flags it.
            name: "combining_diacritics",
            samples: owned(&["e", "e\u{0301}", "a", "a\u{0300}", "o\u{0302}"]),
        },
        Category {
            name: "box_drawing",
            samples: owned(&[
                "─", "│", "┌", "┐", "└", "┘", "├", "┤", "┬", "┴", "┼", "╔", "═",
            ]),
        },
        Category {
            name: "block_elements",
            samples: owned(&["▀", "▄", "█", "▌", "▐", "░", "▒", "▓", "▁", "▏"]),
        },
        Category {
            name: "braille",
            samples: owned(&["⠁", "⠼", "⠿", "⡇", "⣿"]),
        },
        Category {
            name: "powerline",
            samples: owned(&["\u{e0b0}", "\u{e0b1}", "\u{e0b2}", "\u{e0b3}"]),
        },
        Category {
            // Devicons (U+E700s) and Font Awesome (U+F000s) from the
            // configured nerd font.
            name: "nerd_font_icons",
            samples: owned(&["\u{e702}", "\u{e718}", "\u{f001}", "\u{f005}", "\u{f00c}"]),
        },
        Category {
            name: "cjk",
            samples: owned(&["漢", "字", "テ", "ス", "ト", "한", "글"]),
        },
        Category {
            name: "fullwidth_forms",
            samples: owned(&["Ａ", "Ｂ", "１", "２", "？"]),
        },
        Category {
            name: "arrows_math",
            samples: owned(&["→", "∑", "≠", "±"]),
        },
        Category {
            name: "emoji_single",
            samples: owned(&["😀", "🎉", "❤️", "✅", "⚠️"]),
        },
        Category {
            name: "variation_selector",
            samples: owned(&["❤️", "❤"]),
        },
        Category {
            name: "zwj_sequences",
            samples: owned(&[
                "👨\u{200d}👩\u{200d}👧\u{200d}👦",
                "👩\u{200d}💻",
                "🏳️\u{200d}🌈",
            ]),
        },
        Category {
            // Includes the unmodified base: if the skin-tone modifier is
            // ignored, the pairwise-identical raster check flags it.
            name: "skin_tone_modifiers",
            samples: owned(&["👍", "👍🏽"]),
        },
        Category {
            name: "flags",
            samples: owned(&["🇺🇸", "🇯🇵"]),
        },
        Category {
            name: "keycap",
            samples: owned(&["1️⃣"]),
        },
    ]
}

/// Column span of `text` computed the same way the grid places clusters:
/// per extended grapheme cluster, `width(cluster).clamp(1, 2)`.
fn grid_cols(text: &str) -> usize {
    UnicodeSegmentation::graphemes(text, true)
        .map(|g| UnicodeWidthStr::width(g).clamp(1, 2))
        .sum()
}

/// Renders `content` on `CONTENT_ROW` of a fresh terminal wide enough to hold
/// it, using the full-redraw path of the CPU renderer.
fn render_line(
    config: &AppConfig,
    atlas: &mut GlyphAtlas,
    content: &str,
) -> (OffscreenRenderer, Terminal) {
    let cols = (grid_cols(content) + 4) as u16;
    let mut terminal = Terminal::new(cols, GRID_ROWS);
    terminal.process(b"\r\n");
    terminal.process(content.as_bytes());
    terminal.cursor_visible = false;
    let mut renderer = OffscreenRenderer::new(cols, GRID_ROWS, atlas);
    renderer.render(&mut terminal, atlas, config);
    (renderer, terminal)
}

/// Extracts the full-height pixel band covering `span` columns starting at
/// `col_start`. Cell-relative drawing makes bands from different columns and
/// terminals directly comparable.
fn band_pixels(
    renderer: &OffscreenRenderer,
    col_start: usize,
    span: usize,
    cell_w: usize,
) -> Vec<u32> {
    let x0 = col_start * cell_w;
    let x1 = ((col_start + span) * cell_w).min(renderer.width);
    let mut out = Vec::with_capacity((x1 - x0) * renderer.height);
    for y in 0..renderer.height {
        let row = y * renderer.width;
        out.extend_from_slice(&renderer.pixels[row + x0..row + x1]);
    }
    out
}

/// Number of non-background pixels plus the band-relative (x, y) of the first
/// one, as pixel-level evidence.
fn ink_evidence(band: &[u32], band_w: usize, bg: u32) -> (usize, Option<(usize, usize)>) {
    let count = band.iter().filter(|&&p| p != bg).count();
    let first = band
        .iter()
        .position(|&p| p != bg)
        .map(|i| (i % band_w, i / band_w));
    (count, first)
}

struct ProbeContext {
    config: AppConfig,
    bg: u32,
    /// Raster of one cell containing an isolated `|` sentinel.
    sentinel_cell: Vec<u32>,
}

impl ProbeContext {
    fn new(config: AppConfig, atlas: &mut GlyphAtlas) -> Self {
        let bg = config.style.background.as_u32_rgb();
        let (sentinel_renderer, _) = render_line(&config, atlas, &format!(" {SENTINEL}"));
        let sentinel_cell = band_pixels(&sentinel_renderer, 1, 1, atlas.cell_width);
        assert!(
            sentinel_cell.iter().any(|&p| p != bg),
            "isolated sentinel {SENTINEL:?} must render visible pixels"
        );

        Self {
            config,
            bg,
            sentinel_cell,
        }
    }

    /// Renders the tofu reference for one category: the raster the renderer
    /// currently produces for a codepoint no real font covers. Rendered fresh
    /// per category (with a distinct codepoint, since negative lookups are
    /// cached) so it reflects whatever fallback faces the atlas has loaded so
    /// far, exactly as a long-lived terminal session would.
    fn notdef_cell(&self, atlas: &mut GlyphAtlas, category_index: usize) -> Vec<u32> {
        let missing = char::from_u32(MISSING_CODEPOINT_BASE + category_index as u32)
            .expect("missing-codepoint pool stays inside plane 16");
        let (renderer, terminal) = render_line(&self.config, atlas, &missing.to_string());
        assert_eq!(
            terminal.grid.cell_at(CONTENT_ROW, 0).ch,
            missing as u32,
            "missing-codepoint reference cell should hold U+{:06X}",
            missing as u32
        );
        band_pixels(&renderer, 0, 1, atlas.cell_width)
    }
}

/// Probes one category at one DPI. Returns (pass-evidence line, failures).
fn probe_category(
    ctx: &ProbeContext,
    atlas: &mut GlyphAtlas,
    category: &Category,
    category_index: usize,
) -> (String, Vec<String>) {
    let cell_w = atlas.cell_width;
    let notdef_cell = ctx.notdef_cell(atlas, category_index);
    let notdef_has_ink = notdef_cell.iter().any(|&p| p != ctx.bg);
    let joined = category.samples.join(SEPARATOR);
    let content = format!("{joined}{SEPARATOR}{SENTINEL}");
    let (renderer, terminal) = render_line(&ctx.config, atlas, &content);

    let mut failures = Vec::new();
    let mut bands: Vec<(usize, usize, Vec<u32>)> = Vec::new(); // (sample idx, span, band)
    let mut min_ink = usize::MAX;
    let mut max_ink = 0usize;

    let mut col = 0usize;
    for (idx, sample) in category.samples.iter().enumerate() {
        let span = grid_cols(sample);
        let band = band_pixels(&renderer, col, span, cell_w);
        let band_w = span * cell_w;

        // (d) placement: the head cell must land at the predicted column.
        let head = terminal.grid.cell_at(CONTENT_ROW, col);
        let cluster = terminal.grid.cell_grapheme_at(CONTENT_ROW, col);
        let first_char = sample.chars().next().expect("samples are non-empty");
        let placed =
            cluster == Some(sample.as_str()) || (cluster.is_none() && head.ch == first_char as u32);
        if !placed {
            failures.push(format!(
                "sample {sample:?}: head cell at col {col} holds ch=U+{:04X} grapheme={cluster:?}, \
                 expected the sample (misalignment after earlier cells)",
                head.ch
            ));
        }

        // (a) ink inside the expected cell span.
        let (ink, first_ink) = ink_evidence(&band, band_w, ctx.bg);
        min_ink = min_ink.min(ink);
        max_ink = max_ink.max(ink);
        if ink == 0 {
            failures.push(format!(
                "sample {sample:?}: no non-background pixels in cols {col}..{} \
                 (band {band_w}x{} px, bg=#{:06x})",
                col + span,
                renderer.height,
                ctx.bg
            ));
        }

        // (b) wide glyphs must occupy exactly the expected two-cell span.
        if span == 2 {
            let cont = terminal.grid.cell_at(CONTENT_ROW, col + 1);
            if head.flags & FLAG_WIDE == 0 {
                failures.push(format!(
                    "sample {sample:?}: expected FLAG_WIDE on head cell at col {col}, \
                     flags={:#04x}",
                    head.flags
                ));
            }
            if cont.flags & FLAG_WIDE_CONT == 0 {
                failures.push(format!(
                    "sample {sample:?}: expected FLAG_WIDE_CONT at col {}, flags={:#04x}",
                    col + 1,
                    cont.flags
                ));
            }
        } else if head.flags & FLAG_WIDE != 0 {
            failures.push(format!(
                "sample {sample:?}: unexpected FLAG_WIDE on single-width cell at col {col}"
            ));
        }

        // (c) tofu: no cell of the sample may match the raster the renderer
        // currently produces for a known-missing codepoint. When that
        // reference is blank the blank case is already covered by (a), so
        // only a non-blank (drawn notdef box) reference is compared here.
        if notdef_has_ink {
            let tofu_cell =
                (0..span).find(|i| band_pixels(&renderer, col + i, 1, cell_w) == notdef_cell);
            if let Some(i) = tofu_cell {
                failures.push(format!(
                    "sample {sample:?}: cell at col {} renders pixel-identical to the \
                     known-missing codepoint notdef box ({} ink px): tofu",
                    col + i,
                    notdef_cell.iter().filter(|&&p| p != ctx.bg).count()
                ));
            }
        }

        if ink > 0 {
            bands.push((idx, span, band));
        }

        col += span + SEPARATOR.len();
    }

    // (c) tofu, pairwise: visually distinct samples must not raster
    // identically (a shared fallback box, dropped combining mark, or ignored
    // emoji modifier all collapse distinct samples onto one raster).
    for i in 0..bands.len() {
        for j in (i + 1)..bands.len() {
            let (ia, sa, ba) = &bands[i];
            let (ib, sb, bb) = &bands[j];
            if sa == sb && category.samples[*ia] != category.samples[*ib] && ba == bb {
                failures.push(format!(
                    "samples {:?} and {:?} render pixel-identical non-blank rasters \
                     ({} px band): shared fallback/tofu suspicion",
                    category.samples[*ia],
                    category.samples[*ib],
                    ba.len()
                ));
            }
        }
    }

    // (d) sentinel: the trailing `|` must land at the predicted column and
    // match the isolated `|` raster pixel-for-pixel.
    let sentinel_col = col;
    let sentinel_char = terminal.grid.cell_char(CONTENT_ROW, sentinel_col);
    if sentinel_char != SENTINEL {
        failures.push(format!(
            "sentinel: expected {SENTINEL:?} at col {sentinel_col}, found {sentinel_char:?} \
             (following text lost column alignment)"
        ));
    } else {
        let sentinel_band = band_pixels(&renderer, sentinel_col, 1, cell_w);
        if sentinel_band != ctx.sentinel_cell {
            let diff = sentinel_band
                .iter()
                .zip(&ctx.sentinel_cell)
                .filter(|(a, b)| a != b)
                .count();
            failures.push(format!(
                "sentinel: raster at col {sentinel_col} differs from isolated {SENTINEL:?} \
                 raster in {diff} px (neighbor bleed or misalignment)"
            ));
        }
    }

    let evidence = format!(
        "{} samples over {} cols, ink per sample {}..{} px",
        category.samples.len(),
        sentinel_col + 1,
        if min_ink == usize::MAX { 0 } else { min_ink },
        max_ink
    );
    (evidence, failures)
}

#[test]
fn glyph_gamut_probe_renders_expected_pixels_at_1x_and_2x_dpi() {
    let config = AppConfig::default();
    let mut all_failures: Vec<String> = Vec::new();

    for &(dpi, scale) in DPI_SCALES {
        let mut atlas =
            GlyphAtlas::with_family_dpi(&config.style.font_family, config.style.font_size, dpi)
                .or_else(|_| GlyphAtlas::new_with_dpi(config.style.font_size, dpi))
                .expect("should load a monospace font atlas for requested dpi");
        println!(
            "=== glyph gamut probe @ dpi {dpi} ({scale}), cell {}x{} px, font {:?} ===",
            atlas.cell_width, atlas.cell_height, config.style.font_family
        );

        let ctx = ProbeContext::new(config.clone(), &mut atlas);
        for (category_index, category) in categories().iter().enumerate() {
            let (evidence, failures) = probe_category(&ctx, &mut atlas, category, category_index);
            if failures.is_empty() {
                println!("  {:<24} PASS  ({evidence})", category.name);
            } else {
                println!("  {:<24} FAIL  ({evidence})", category.name);
                for failure in &failures {
                    println!("    - {failure}");
                    all_failures.push(format!("[dpi {dpi} {scale}] {}: {failure}", category.name));
                }
            }
        }
    }

    assert!(
        all_failures.is_empty(),
        "glyph gamut probe failures:\n{}",
        all_failures.join("\n")
    );
}
