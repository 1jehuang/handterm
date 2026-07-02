//! Render a wide glyph/emoji gamut offscreen and dump it as a PPM image.
//!
//! Usage: cargo run --release --example render_gamut [output.ppm] [dpi]
//!
//! This drives the exact same Terminal -> GlyphAtlas -> OffscreenRenderer
//! pipeline the CPU frontend uses, so the output image shows what handterm
//! would actually put on screen for each glyph category.

use handterm::config::AppConfig;
use handterm::font::GlyphAtlas;
use handterm::render::OffscreenRenderer;
use handterm::terminal::Terminal;

const GAMUT_LINES: &[&str] = &[
    "=== ASCII ===",
    "The quick brown fox jumps 0123456789 !@#$%^&*()_+-=[]{}|;:\",.<>/?",
    "=== Box drawing ===",
    "\u{250c}\u{2500}\u{2500}\u{2500}\u{2500}\u{2510}  \u{2554}\u{2550}\u{2550}\u{2550}\u{2550}\u{2557}  \u{256d}\u{2500}\u{2500}\u{2500}\u{2500}\u{256e}",
    "\u{2502} bx \u{2502}  \u{2551} db \u{2551}  \u{2502} rd \u{2502}",
    "\u{2514}\u{2500}\u{2500}\u{2500}\u{2500}\u{2518}  \u{255a}\u{2550}\u{2550}\u{2550}\u{2550}\u{255d}  \u{2570}\u{2500}\u{2500}\u{2500}\u{2500}\u{256f}",
    "=== Blocks & shades ===",
    "\u{2588}\u{2593}\u{2592}\u{2591} \u{2580}\u{2584}\u{258c}\u{2590} \u{2596}\u{2597}\u{2598}\u{259d}",
    "=== Braille ===",
    "\u{28ff}\u{28f7}\u{28e7}\u{28c7}\u{2847}\u{2807}\u{2803}\u{2801} \u{2801}\u{2803}\u{2807}\u{2847}\u{28c7}\u{28e7}\u{28f7}\u{28ff}",
    "=== Powerline / Nerd ===",
    "\u{e0b0} \u{e0b1} \u{e0b2} \u{e0b3} \u{e718} \u{f07b} \u{e702} \u{f121}",
    "=== CJK & fullwidth ===",
    "\u{6f22}\u{5b57} \u{30c6}\u{30b9}\u{30c8} \u{d55c}\u{ae00} \u{ff28}\u{ff45}\u{ff4c}\u{ff4c}\u{ff4f}|align",
    "=== Arrows & math ===",
    "\u{2192} \u{2190} \u{2191} \u{2193} \u{21d2} \u{2211} \u{220f} \u{221a} \u{2260} \u{2264} \u{2265} \u{b1} \u{221e} \u{222b}",
    "=== Emoji singles ===",
    "\u{1f600} \u{1f389} \u{2764}\u{fe0f} \u{2705} \u{26a0}\u{fe0f} \u{1f680} \u{1f525} \u{1f4a9}|align",
    "=== ZWJ / modifiers / flags ===",
    "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466} \u{1f469}\u{200d}\u{1f4bb} \u{1f3f3}\u{fe0f}\u{200d}\u{1f308} \u{1f44d}\u{1f3fd} \u{1f1fa}\u{1f1f8} \u{1f1ef}\u{1f1f5} 1\u{fe0f}\u{20e3}|align",
    "=== Combining ===",
    "e\u{301} a\u{300} n\u{303} o\u{308} c\u{327}|align",
];

fn main() {
    let mut args = std::env::args().skip(1);
    let out_path = args
        .next()
        .unwrap_or_else(|| "bench_out/glyph_gamut.ppm".to_string());
    let dpi: u32 = args
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(96);

    let config = AppConfig::default();
    let mut atlas =
        GlyphAtlas::with_family_dpi(&config.style.font_family, config.style.font_size, dpi)
            .or_else(|_| GlyphAtlas::new_with_dpi(config.style.font_size, dpi))
            .expect("should load a monospace font atlas");

    let cols: u16 = 72;
    let rows: u16 = GAMUT_LINES.len() as u16 + 1;
    let mut terminal = Terminal::new(cols, rows);
    for line in GAMUT_LINES {
        terminal.process(line.as_bytes());
        terminal.process(b"\r\n");
    }

    let mut renderer = OffscreenRenderer::new(cols, rows, &atlas);
    renderer.render(&mut terminal, &mut atlas, &config);

    write_ppm(&renderer, &out_path);
    println!(
        "wrote {out_path} ({}x{} px, dpi {dpi}, cell {}x{})",
        renderer.width, renderer.height, atlas.cell_width, atlas.cell_height
    );
}

fn write_ppm(renderer: &OffscreenRenderer, path: &str) {
    let mut out = Vec::with_capacity(renderer.width * renderer.height * 3 + 64);
    out.extend_from_slice(format!("P6\n{} {}\n255\n", renderer.width, renderer.height).as_bytes());
    for &pixel in &renderer.pixels {
        out.push((pixel >> 16) as u8);
        out.push((pixel >> 8) as u8);
        out.push(pixel as u8);
    }
    std::fs::write(path, out).expect("should write ppm output");
}
