#[cfg(feature = "cpu")]
use crate::config::AppConfig;
#[cfg(feature = "cpu")]
use crate::font::GlyphAtlas;
#[cfg(feature = "gpu")]
use crate::gpu_frame::{
    AtlasImageRect, FrameBatchStyle, FrameTextBatches, GlyphAtlasEntry, build_frame_plan,
    fill_cell_infos, fill_image_instances, fill_text_batches,
};
use crate::grid::Grid;
use crate::parser::Parser;
use crate::protocol::{
    ClientMessage, CursorState, DirtyCell, KeyEvent, KeyEventKind, ServerMessage, WindowModes,
    decode_client_message, decode_server_message, encode_client_message, encode_server_message,
};
use crate::pty::PtyChild;
#[cfg(feature = "cpu")]
use crate::render::OffscreenRenderer;
use crate::terminal::Terminal;
use crate::workloads::{
    EMOJI_AND_SHADE_TRANSCRIPT, PROMPT_PREFIX, STARSHIP_PROMPT_TRANSCRIPT,
    TUI_HELP_WITH_IMAGE_TRANSCRIPT, TYPING_WORKLOAD,
};
use anyhow::{Result, bail};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct BenchResult {
    pub spawn_us: u128,
    pub shell_ready_us: u128,
    pub grid_alloc_us: u128,

    pub parser_ascii_mb_s: f64,
    pub parser_sgr_mb_s: f64,
    pub parser_mixed_mb_s: f64,

    pub grid_ascii_mb_s: f64,
    pub grid_utf8_mb_s: f64,
    pub grid_sgr_color_mb_s: f64,

    pub terminal_ascii_mb_s: f64,
    pub terminal_sgr_mb_s: f64,
    pub terminal_mixed_mb_s: f64,

    pub gpu_plan_cells_per_s: f64,
    pub gpu_batch_cells_per_s: f64,
    pub gpu_image_placements_per_s: f64,
    pub gpu_prompt_replay_fps: f64,
    pub gpu_tui_replay_fps: f64,
    pub gpu_emoji_replay_fps: f64,

    pub cpu_full_redraw_fps: f64,
    pub cpu_incremental_render_fps: f64,
    pub cpu_prompt_replay_fps: f64,
    pub cpu_tui_replay_fps: f64,
    pub cpu_emoji_replay_fps: f64,
    pub protocol_roundtrips_per_s: f64,

    pub memcpy_mb_s: f64,
    pub byte_scan_mb_s: f64,

    pub cell_write_ns: f64,
    pub cell_size_bytes: usize,
    pub grid_memory_kb: usize,
    pub scrollback_per_line_bytes: usize,
}

const BENCH_SIZE: usize = 64 * 1024 * 1024;

pub fn run_quick_bench(columns: u16, rows: u16) -> Result<BenchResult> {
    // === PTY spawn ===
    let t0 = Instant::now();
    let pty = PtyChild::spawn_default_shell(columns, rows)?;
    let spawn_us = t0.elapsed().as_micros();

    let marker = format!("handterm-ready-{}", std::process::id());
    let cmd = format!("printf '{}\\n'\\n", marker);
    let ready_start = Instant::now();
    pty.write_all(cmd.as_bytes())?;

    let mut read_buf = vec![0_u8; 16 * 1024];
    let mut got = String::new();
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let n = pty.try_read(&mut read_buf)?;
        if n == 0 {
            continue;
        }
        got.push_str(&String::from_utf8_lossy(&read_buf[..n]));
        if got.contains(&marker) {
            break;
        }
    }
    if !got.contains(&marker) {
        bail!("shell ready marker not observed within timeout");
    }
    let shell_ready_us = ready_start.elapsed().as_micros();

    // === Grid alloc ===
    let grid_start = Instant::now();
    let _grid = Grid::new(columns, rows, [0xff; 3], [0; 3]);
    let grid_alloc_us = grid_start.elapsed().as_micros();

    // === Theoretical floor: memcpy ===
    let src = vec![0x41u8; BENCH_SIZE];
    let mut dst = vec![0u8; BENCH_SIZE];
    let mc_start = Instant::now();
    dst.copy_from_slice(&src);
    std::hint::black_box(&dst);
    let memcpy_mb_s = mb_per_sec(BENCH_SIZE, mc_start.elapsed());

    // === Theoretical floor: byte scan ===
    let scan_start = Instant::now();
    let mut scan_count: usize = 0;
    for &b in src.iter() {
        if (0x20..=0x7e).contains(&b) {
            scan_count += 1;
        }
    }
    std::hint::black_box(scan_count);
    let byte_scan_mb_s = mb_per_sec(BENCH_SIZE, scan_start.elapsed());

    // === Parser throughput (pure parse, no grid) ===
    let parser_ascii_mb_s = bench_parser_throughput(&vec![b'A'; BENCH_SIZE]);

    let sgr_payload = build_sgr_payload(BENCH_SIZE);
    let parser_sgr_mb_s = bench_parser_throughput(&sgr_payload);

    let mixed_payload = build_mixed_payload(BENCH_SIZE);
    let parser_mixed_mb_s = bench_parser_throughput(&mixed_payload);

    // === Grid write throughput ===
    let grid_ascii_mb_s = bench_grid_write(columns, rows, &vec![b'x'; BENCH_SIZE]);

    let utf8_payload = build_utf8_payload(BENCH_SIZE);
    let grid_utf8_mb_s = bench_grid_write(columns, rows, &utf8_payload);

    let sgr_color_payload = build_sgr_color_payload(BENCH_SIZE);
    let grid_sgr_color_mb_s = bench_grid_write(columns, rows, &sgr_color_payload);

    // === Full terminal pipeline (parser + grid + state) ===
    let terminal_ascii_mb_s = bench_terminal_throughput(columns, rows, &vec![b'A'; BENCH_SIZE]);
    let terminal_sgr_mb_s = bench_terminal_throughput(columns, rows, &sgr_payload);
    let terminal_mixed_mb_s = bench_terminal_throughput(columns, rows, &mixed_payload);

    #[cfg(feature = "gpu")]
    let (
        gpu_plan_cells_per_s,
        gpu_batch_cells_per_s,
        gpu_image_placements_per_s,
        gpu_prompt_replay_fps,
        gpu_tui_replay_fps,
        gpu_emoji_replay_fps,
    ) = bench_gpu_frame_pipeline(columns.max(120), rows.max(72));
    #[cfg(not(feature = "gpu"))]
    let (
        gpu_plan_cells_per_s,
        gpu_batch_cells_per_s,
        gpu_image_placements_per_s,
        gpu_prompt_replay_fps,
        gpu_tui_replay_fps,
        gpu_emoji_replay_fps,
    ) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);

    #[cfg(feature = "cpu")]
    let (
        cpu_full_redraw_fps,
        cpu_incremental_render_fps,
        cpu_prompt_replay_fps,
        cpu_tui_replay_fps,
        cpu_emoji_replay_fps,
    ) = bench_cpu_render_pipeline(columns.max(120), rows.max(72));
    #[cfg(not(feature = "cpu"))]
    let (
        cpu_full_redraw_fps,
        cpu_incremental_render_fps,
        cpu_prompt_replay_fps,
        cpu_tui_replay_fps,
        cpu_emoji_replay_fps,
    ) = (0.0, 0.0, 0.0, 0.0, 0.0);

    // === Per-cell write cost ===
    let cell_write_ns = bench_cell_write_ns(columns, rows);
    let protocol_roundtrips_per_s = bench_protocol_roundtrips();

    // === Memory ===
    let cell_size_bytes = std::mem::size_of::<crate::grid::Cell>();
    let grid_memory_kb = (columns as usize * rows as usize * cell_size_bytes) / 1024;
    let scrollback_per_line_bytes = columns as usize * cell_size_bytes;

    Ok(BenchResult {
        spawn_us,
        shell_ready_us,
        grid_alloc_us,
        parser_ascii_mb_s,
        parser_sgr_mb_s,
        parser_mixed_mb_s,
        grid_ascii_mb_s,
        grid_utf8_mb_s,
        grid_sgr_color_mb_s,
        terminal_ascii_mb_s,
        terminal_sgr_mb_s,
        terminal_mixed_mb_s,
        gpu_plan_cells_per_s,
        gpu_batch_cells_per_s,
        gpu_image_placements_per_s,
        gpu_prompt_replay_fps,
        gpu_tui_replay_fps,
        gpu_emoji_replay_fps,
        cpu_full_redraw_fps,
        cpu_incremental_render_fps,
        cpu_prompt_replay_fps,
        cpu_tui_replay_fps,
        cpu_emoji_replay_fps,
        protocol_roundtrips_per_s,
        memcpy_mb_s,
        byte_scan_mb_s,
        cell_write_ns,
        cell_size_bytes,
        grid_memory_kb,
        scrollback_per_line_bytes,
    })
}

fn mb_per_sec(bytes: usize, elapsed: Duration) -> f64 {
    let secs = elapsed.as_secs_f64().max(1e-9);
    (bytes as f64 / (1024.0 * 1024.0)) / secs
}

fn bench_parser_throughput(payload: &[u8]) -> f64 {
    let mut parser = Parser::new();
    let start = Instant::now();
    for &b in payload.iter() {
        std::hint::black_box(parser.advance(b));
    }
    mb_per_sec(payload.len(), start.elapsed())
}

fn bench_grid_write(cols: u16, rows: u16, payload: &[u8]) -> f64 {
    let mut grid = Grid::new(cols, rows, [0xff; 3], [0; 3]);
    let start = Instant::now();
    grid.write_bytes(payload);
    mb_per_sec(payload.len(), start.elapsed())
}

fn bench_terminal_throughput(cols: u16, rows: u16, payload: &[u8]) -> f64 {
    let mut term = Terminal::new(cols, rows);
    let start = Instant::now();
    term.process(payload);
    mb_per_sec(payload.len(), start.elapsed())
}

fn bench_cell_write_ns(cols: u16, rows: u16) -> f64 {
    let mut grid = Grid::new(cols, rows, [0xff; 3], [0; 3]);
    let n = 10_000_000usize;
    let payload = vec![b'X'; n];
    let start = Instant::now();
    grid.write_bytes(&payload);
    let elapsed_ns = start.elapsed().as_nanos() as f64;
    elapsed_ns / n as f64
}

fn bench_protocol_roundtrips() -> f64 {
    let client = ClientMessage::KeyInput {
        window_id: 7,
        event: KeyEvent {
            kind: KeyEventKind::Press,
            bytes: b"\x1b[A".to_vec(),
            text: None,
            modifiers: 0b101,
        },
    };
    let server = ServerMessage::CellUpdate {
        window_id: 7,
        dirty_cells: vec![
            DirtyCell {
                row: 0,
                col: 0,
                ch: 'h' as u32,
                grapheme: None,
                fg: 0x112233,
                bg: 0x000000,
                underline_color: 0x112233,
                hyperlink_id: 0,
                attrs: 0,
                flags: 0,
                underline_style: 0,
            },
            DirtyCell {
                row: 0,
                col: 1,
                ch: 'i' as u32,
                grapheme: None,
                fg: 0x112233,
                bg: 0x000000,
                underline_color: 0x112233,
                hyperlink_id: 0,
                attrs: 0,
                flags: 0,
                underline_style: 0,
            },
        ],
        cursor: Some(CursorState {
            row: 0,
            col: 2,
            style: 1,
            visible: true,
        }),
        modes: WindowModes::default(),
    };

    let iterations = 20_000usize;
    let start = Instant::now();
    for _ in 0..iterations {
        let client_bytes =
            encode_client_message(&client).expect("client protocol message should encode");
        let server_bytes =
            encode_server_message(&server).expect("server protocol message should encode");
        std::hint::black_box(
            decode_client_message(&client_bytes).expect("client protocol message should decode"),
        );
        std::hint::black_box(
            decode_server_message(&server_bytes).expect("server protocol message should decode"),
        );
    }
    (iterations as f64 * 2.0) / start.elapsed().as_secs_f64().max(1e-9)
}

#[cfg(feature = "gpu")]
fn bench_gpu_frame_pipeline(cols: u16, rows: u16) -> (f64, f64, f64, f64, f64, f64) {
    let mut term = Terminal::new(cols, rows);
    let payload = build_dense_terminal_payload(cols, rows);
    term.process(&payload);
    term.grid.selection = Some(crate::grid::Selection {
        start_col: 1,
        start_row: 1,
        end_col: (cols as usize / 2).max(1),
        end_row: (rows as usize / 2).max(1),
    });
    term.process(b"\x1b_Ga=T,i=7,f=32,s=1,v=1,c=1,r=1;/wAA/w==\x1b\\");

    let iterations = 200usize;
    let mut cell_infos = Vec::with_capacity(cols as usize * rows as usize);
    let plan_start = Instant::now();
    let mut planned_cells = 0usize;
    for _ in 0..iterations {
        fill_cell_infos(&term, &mut cell_infos);
        planned_cells += cell_infos.len();
    }
    let gpu_plan_cells_per_s = planned_cells as f64 / plan_start.elapsed().as_secs_f64().max(1e-9);

    let frame_plan = build_frame_plan(&term);
    let mut batches = FrameTextBatches::default();
    let batch_start = Instant::now();
    let mut batched_cells = 0usize;
    for _ in 0..iterations {
        fill_text_batches(
            &frame_plan.cell_infos,
            FrameBatchStyle {
                base_fg: 0xffffff,
                base_bg: 0x000000,
                base_fg_f: [1.0, 1.0, 1.0, 1.0],
                background_alpha: 1.0,
                cell_w: 8.0,
                cell_h: 16.0,
                viewport_offset_y: 0.0,
            },
            &mut batches,
            |ci| {
                (ci.ch > 0x20 || ci.grapheme.is_some()).then_some(GlyphAtlasEntry {
                    x: 0,
                    y: 0,
                    width: (ci.cells * 8) as u32,
                    height: 16,
                    left_pad: 0,
                    top_pad: 0,
                    is_color: ci.ch > 0xffff,
                })
            },
        );
        batched_cells += frame_plan.cell_infos.len();
    }
    let gpu_batch_cells_per_s =
        batched_cells as f64 / batch_start.elapsed().as_secs_f64().max(1e-9);

    let placements = if term.kitty_placements.is_empty() {
        vec![crate::terminal::KittyPlacement {
            image_id: 1,
            col: 0,
            row: 0,
            cols: 1,
            rows: 1,
        }]
    } else {
        let mut placements = Vec::with_capacity(128);
        while placements.len() < 128 {
            placements.extend_from_slice(&term.kitty_placements);
        }
        placements.truncate(128);
        placements
    };
    let mut image_instances = Vec::with_capacity(placements.len());
    let image_start = Instant::now();
    let mut image_count = 0usize;
    for _ in 0..iterations {
        fill_image_instances(&placements, 8.0, 16.0, &mut image_instances, |_placement| {
            Some(AtlasImageRect {
                x: 0,
                y: 0,
                width: 8,
                height: 16,
            })
        });
        image_count += image_instances.len();
    }
    let gpu_image_placements_per_s =
        image_count as f64 / image_start.elapsed().as_secs_f64().max(1e-9);

    let (gpu_prompt_replay_fps, gpu_tui_replay_fps, gpu_emoji_replay_fps) =
        bench_gpu_transcript_replay();

    (
        gpu_plan_cells_per_s,
        gpu_batch_cells_per_s,
        gpu_image_placements_per_s,
        gpu_prompt_replay_fps,
        gpu_tui_replay_fps,
        gpu_emoji_replay_fps,
    )
}

#[cfg(feature = "cpu")]
fn bench_cpu_render_pipeline(cols: u16, rows: u16) -> (f64, f64, f64, f64, f64) {
    let config = AppConfig::default();
    let mut atlas = GlyphAtlas::new(config.style.font_size)
        .expect("default font atlas should load for render benchmark");

    let mut full_terminal = Terminal::new(cols, rows);
    let payload = build_dense_terminal_payload(cols, rows);
    full_terminal.process(&payload);
    full_terminal.process(b"\x1b[38;2;255;120;80mstatus\x1b[0m");
    let mut full_renderer = OffscreenRenderer::new(cols, rows, &atlas);
    let iterations = 120usize;
    let full_start = Instant::now();
    for _ in 0..iterations {
        full_renderer.reset();
        full_renderer.render(&mut full_terminal, &mut atlas, &config);
    }
    let cpu_full_redraw_fps = iterations as f64 / full_start.elapsed().as_secs_f64().max(1e-9);

    let mut incremental_terminal = Terminal::new(cols, rows);
    incremental_terminal.process(PROMPT_PREFIX);
    let mut incremental_renderer = OffscreenRenderer::new(cols, rows, &atlas);
    incremental_renderer.render(&mut incremental_terminal, &mut atlas, &config);
    let incremental_start = Instant::now();
    let mut frames = 0usize;
    for _ in 0..32 {
        for &byte in TYPING_WORKLOAD {
            incremental_terminal.process(&[byte]);
            incremental_renderer.render(&mut incremental_terminal, &mut atlas, &config);
            frames += 1;
        }
        incremental_terminal.process(b"\r\x1b[2K");
        incremental_terminal.process(PROMPT_PREFIX);
        incremental_renderer.render(&mut incremental_terminal, &mut atlas, &config);
        frames += 1;
    }
    let cpu_incremental_render_fps =
        frames as f64 / incremental_start.elapsed().as_secs_f64().max(1e-9);

    let (cpu_prompt_replay_fps, cpu_tui_replay_fps, cpu_emoji_replay_fps) =
        bench_cpu_transcript_replay(&config, &mut atlas);

    (
        cpu_full_redraw_fps,
        cpu_incremental_render_fps,
        cpu_prompt_replay_fps,
        cpu_tui_replay_fps,
        cpu_emoji_replay_fps,
    )
}

#[cfg(feature = "gpu")]
fn bench_gpu_transcript_replay() -> (f64, f64, f64) {
    let style = FrameBatchStyle {
        base_fg: 0xffffff,
        base_bg: 0x000000,
        base_fg_f: [1.0, 1.0, 1.0, 1.0],
        background_alpha: 1.0,
        cell_w: 8.0,
        cell_h: 16.0,
        viewport_offset_y: 0.0,
    };
    let iterations = 200usize;
    let mut cell_infos = Vec::new();
    let mut batches = FrameTextBatches::default();
    let mut image_instances = Vec::new();

    let prompt_start = Instant::now();
    let mut prompt_frames = 0usize;
    for _ in 0..iterations {
        let mut terminal = Terminal::new(80, 24);
        for chunk in STARSHIP_PROMPT_TRANSCRIPT {
            terminal.process(chunk);
            fill_cell_infos(&terminal, &mut cell_infos);
            fill_text_batches(&cell_infos, style, &mut batches, |ci| {
                (ci.ch > 0x20 || ci.grapheme.is_some()).then_some(GlyphAtlasEntry {
                    x: 0,
                    y: 0,
                    width: (ci.cells * 8) as u32,
                    height: 16,
                    left_pad: 0,
                    top_pad: 0,
                    is_color: ci.ch > 0xffff,
                })
            });
            prompt_frames += 1;
        }
    }
    let gpu_prompt_replay_fps =
        prompt_frames as f64 / prompt_start.elapsed().as_secs_f64().max(1e-9);

    let tui_start = Instant::now();
    let mut tui_frames = 0usize;
    for _ in 0..iterations {
        let mut terminal = Terminal::new(32, 8);
        for chunk in TUI_HELP_WITH_IMAGE_TRANSCRIPT {
            terminal.process(chunk);
            fill_cell_infos(&terminal, &mut cell_infos);
            fill_text_batches(&cell_infos, style, &mut batches, |ci| {
                (ci.ch > 0x20 || ci.grapheme.is_some()).then_some(GlyphAtlasEntry {
                    x: 0,
                    y: 0,
                    width: (ci.cells * 8) as u32,
                    height: 16,
                    left_pad: 0,
                    top_pad: 0,
                    is_color: ci.ch > 0xffff,
                })
            });
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
            tui_frames += 1;
        }
    }
    let gpu_tui_replay_fps = tui_frames as f64 / tui_start.elapsed().as_secs_f64().max(1e-9);

    let emoji_start = Instant::now();
    let mut emoji_frames = 0usize;
    for _ in 0..iterations {
        let mut terminal = Terminal::new(16, 4);
        for chunk in EMOJI_AND_SHADE_TRANSCRIPT {
            terminal.process(chunk);
            fill_cell_infos(&terminal, &mut cell_infos);
            fill_text_batches(&cell_infos, style, &mut batches, |ci| {
                (ci.ch > 0x20 || ci.grapheme.is_some()).then_some(GlyphAtlasEntry {
                    x: 0,
                    y: 0,
                    width: (ci.cells * 8) as u32,
                    height: 16,
                    left_pad: 0,
                    top_pad: 0,
                    is_color: ci.ch > 0xffff,
                })
            });
            emoji_frames += 1;
        }
    }
    let gpu_emoji_replay_fps = emoji_frames as f64 / emoji_start.elapsed().as_secs_f64().max(1e-9);

    (
        gpu_prompt_replay_fps,
        gpu_tui_replay_fps,
        gpu_emoji_replay_fps,
    )
}

#[cfg(feature = "cpu")]
fn bench_cpu_transcript_replay(config: &AppConfig, atlas: &mut GlyphAtlas) -> (f64, f64, f64) {
    let iterations = 120usize;
    let mut prompt_renderer = OffscreenRenderer::new(80, 24, atlas);

    let prompt_start = Instant::now();
    let mut prompt_frames = 0usize;
    for _ in 0..iterations {
        let mut terminal = Terminal::new(80, 24);
        prompt_renderer.reset();
        for chunk in STARSHIP_PROMPT_TRANSCRIPT {
            terminal.process(chunk);
            prompt_renderer.render(&mut terminal, atlas, config);
            prompt_frames += 1;
        }
    }
    let cpu_prompt_replay_fps =
        prompt_frames as f64 / prompt_start.elapsed().as_secs_f64().max(1e-9);

    let mut tui_renderer = OffscreenRenderer::new(32, 8, atlas);
    let tui_start = Instant::now();
    let mut tui_frames = 0usize;
    for _ in 0..iterations {
        let mut terminal = Terminal::new(32, 8);
        tui_renderer.reset();
        for chunk in TUI_HELP_WITH_IMAGE_TRANSCRIPT {
            terminal.process(chunk);
            tui_renderer.render(&mut terminal, atlas, config);
            tui_frames += 1;
        }
    }
    let cpu_tui_replay_fps = tui_frames as f64 / tui_start.elapsed().as_secs_f64().max(1e-9);

    let mut emoji_renderer = OffscreenRenderer::new(16, 4, atlas);
    let emoji_start = Instant::now();
    let mut emoji_frames = 0usize;
    for _ in 0..iterations {
        let mut terminal = Terminal::new(16, 4);
        emoji_renderer.reset();
        for chunk in EMOJI_AND_SHADE_TRANSCRIPT {
            terminal.process(chunk);
            emoji_renderer.render(&mut terminal, atlas, config);
            emoji_frames += 1;
        }
    }
    let cpu_emoji_replay_fps = emoji_frames as f64 / emoji_start.elapsed().as_secs_f64().max(1e-9);

    (
        cpu_prompt_replay_fps,
        cpu_tui_replay_fps,
        cpu_emoji_replay_fps,
    )
}

fn build_sgr_payload(target_size: usize) -> Vec<u8> {
    let chunk = b"\x1b[1;31mR\x1b[32mG\x1b[34mB\x1b[0m ";
    let mut buf = Vec::with_capacity(target_size);
    while buf.len() < target_size {
        buf.extend_from_slice(chunk);
    }
    buf.truncate(target_size);
    buf
}

fn build_sgr_color_payload(target_size: usize) -> Vec<u8> {
    let chunk = b"\x1b[38;2;255;100;50mH\x1b[48;2;0;40;80me\x1b[0ml";
    let mut buf = Vec::with_capacity(target_size);
    while buf.len() < target_size {
        buf.extend_from_slice(chunk);
    }
    buf.truncate(target_size);
    buf
}

fn build_utf8_payload(target_size: usize) -> Vec<u8> {
    let text = "héllo wörld 你好 ";
    let mut buf = Vec::with_capacity(target_size);
    while buf.len() < target_size {
        buf.extend_from_slice(text.as_bytes());
    }
    buf.truncate(target_size);
    buf
}

fn build_mixed_payload(target_size: usize) -> Vec<u8> {
    let chunk = b"\x1b[1;38;2;200;100;50mHello\x1b[0m world \x1b[?25l\x1b[10;20H\x1b[K\x1b[?25h";
    let mut buf = Vec::with_capacity(target_size);
    while buf.len() < target_size {
        buf.extend_from_slice(chunk);
    }
    buf.truncate(target_size);
    buf
}

#[cfg(any(feature = "gpu", feature = "cpu"))]
fn build_dense_terminal_payload(cols: u16, rows: u16) -> Vec<u8> {
    let mut buf = Vec::with_capacity(cols as usize * rows as usize * 12);
    for row in 0..rows {
        for col in 0..cols {
            if (row + col) % 11 == 0 {
                buf.extend_from_slice("界".as_bytes());
            } else {
                let ch = b'a' + (col % 26) as u8;
                buf.push(ch);
            }
        }
        if row + 1 < rows {
            buf.extend_from_slice(b"\r\n");
        }
    }
    buf
}

pub fn format_bench_results(r: &BenchResult) -> String {
    let cols_small = 80usize;
    let rows_small = 24usize;
    let cells_small = cols_small * rows_small;
    let bytes_small = cells_small * r.cell_size_bytes;

    let cols_full = 120usize;
    let rows_full = 72usize;
    let cells_full = cols_full * rows_full;
    let bytes_full = cells_full * r.cell_size_bytes;

    let fps_small = r.terminal_ascii_mb_s * 1024.0 * 1024.0 / bytes_small as f64;
    let fps_full = r.terminal_ascii_mb_s * 1024.0 * 1024.0 / bytes_full as f64;
    let parser_pct_of_memcpy = (r.parser_ascii_mb_s / r.memcpy_mb_s) * 100.0;
    let grid_pct_of_memcpy = (r.grid_ascii_mb_s / r.memcpy_mb_s) * 100.0;
    let terminal_pct_of_memcpy = (r.terminal_ascii_mb_s / r.memcpy_mb_s) * 100.0;

    format!(
        "\
=== handterm benchmark results ===

--- Theoretical Floors ---
  memcpy (64MB)           : {:.0} MB/s
  byte scan (64MB)        : {:.0} MB/s

--- Parser (no grid, pure state machine) ---
  ASCII                   : {:.0} MB/s  ({:.0}% of memcpy)
  SGR color sequences     : {:.0} MB/s
  Mixed (SGR+cursor+erase): {:.0} MB/s

--- Grid Write (parser + cell writes) ---
  ASCII                   : {:.1} MB/s  ({:.0}% of memcpy)
  UTF-8 mixed             : {:.1} MB/s
  SGR true-color          : {:.1} MB/s

--- Full Terminal Pipeline (parser + grid + state) ---
  ASCII                   : {:.1} MB/s  ({:.0}% of memcpy)
  SGR color               : {:.1} MB/s
  Mixed realistic         : {:.1} MB/s

--- GPU Frame Prep (CPU-side) ---
  cell info fill          : {:.1} Mcells/s
  text batching           : {:.1} Mcells/s
  image batching          : {:.1} Kplacements/s
  prompt replay           : {:.0} fps
  TUI help replay         : {:.0} fps
  emoji replay            : {:.0} fps

--- CPU Renderer ---
  offscreen full redraw   : {:.0} fps
  incremental typing      : {:.0} fps
  prompt replay           : {:.0} fps
  TUI help replay         : {:.0} fps
  emoji replay            : {:.0} fps

--- Protocol ---
  message roundtrips      : {:.0} msg/s

--- Per-Cell Metrics ---
  cell size               : {} bytes
  cell write latency      : {:.1} ns/cell
  grid memory (80x24)     : {} KB
  grid memory (120x72)    : {} KB
  scrollback/line (80col) : {} bytes
  10k scrollback          : {} KB

--- Startup ---
  PTY spawn               : {} us
  shell ready             : {} us
  grid alloc              : {} us

--- Derived ---
  frames/sec (80x24)      : {:.0}
  frames/sec (120x72)     : {:.0}
  full-screen write 80x24 : {:.1} us ({} bytes)
  full-screen write 120x72: {:.1} us ({} bytes)",
        r.memcpy_mb_s,
        r.byte_scan_mb_s,
        r.parser_ascii_mb_s,
        parser_pct_of_memcpy,
        r.parser_sgr_mb_s,
        r.parser_mixed_mb_s,
        r.grid_ascii_mb_s,
        grid_pct_of_memcpy,
        r.grid_utf8_mb_s,
        r.grid_sgr_color_mb_s,
        r.terminal_ascii_mb_s,
        terminal_pct_of_memcpy,
        r.terminal_sgr_mb_s,
        r.terminal_mixed_mb_s,
        r.gpu_plan_cells_per_s / 1_000_000.0,
        r.gpu_batch_cells_per_s / 1_000_000.0,
        r.gpu_image_placements_per_s / 1_000.0,
        r.gpu_prompt_replay_fps,
        r.gpu_tui_replay_fps,
        r.gpu_emoji_replay_fps,
        r.cpu_full_redraw_fps,
        r.cpu_incremental_render_fps,
        r.cpu_prompt_replay_fps,
        r.cpu_tui_replay_fps,
        r.cpu_emoji_replay_fps,
        r.protocol_roundtrips_per_s,
        r.cell_size_bytes,
        r.cell_write_ns,
        r.grid_memory_kb,
        cells_full * r.cell_size_bytes / 1024,
        r.scrollback_per_line_bytes,
        r.scrollback_per_line_bytes * 10000 / 1024,
        r.spawn_us,
        r.shell_ready_us,
        r.grid_alloc_us,
        fps_small,
        fps_full,
        (bytes_small as f64 / (r.terminal_ascii_mb_s * 1024.0 * 1024.0)) * 1_000_000.0,
        bytes_small,
        (bytes_full as f64 / (r.terminal_ascii_mb_s * 1024.0 * 1024.0)) * 1_000_000.0,
        bytes_full,
    )
}

#[cfg(test)]
mod tests {
    use super::run_quick_bench;

    #[test]
    fn quick_bench_runs_and_produces_metrics() {
        let out = run_quick_bench(80, 24).expect("bench should run");
        assert!(out.spawn_us < 2_000_000);
        assert!(out.shell_ready_us < 2_000_000);
        assert!(out.grid_ascii_mb_s > 1.0);
        #[cfg(feature = "gpu")]
        {
            assert!(out.gpu_plan_cells_per_s > 1.0);
            assert!(out.gpu_batch_cells_per_s > 1.0);
            assert!(out.gpu_prompt_replay_fps > 1.0);
            assert!(out.gpu_tui_replay_fps > 1.0);
            assert!(out.gpu_emoji_replay_fps > 1.0);
        }
        #[cfg(feature = "cpu")]
        {
            assert!(out.cpu_full_redraw_fps > 1.0);
            assert!(out.cpu_incremental_render_fps > 1.0);
            assert!(out.cpu_prompt_replay_fps > 1.0);
            assert!(out.cpu_tui_replay_fps > 1.0);
            assert!(out.cpu_emoji_replay_fps > 1.0);
        }
        assert!(out.protocol_roundtrips_per_s > 1.0);
    }
}
