use crate::grid::Grid;
use crate::parser::Parser;
use crate::pty::PtyChild;
use crate::terminal::Terminal;
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
        if n == 0 { continue; }
        got.push_str(&String::from_utf8_lossy(&read_buf[..n]));
        if got.contains(&marker) { break; }
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
        if (0x20..=0x7e).contains(&b) { scan_count += 1; }
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

    // === Per-cell write cost ===
    let cell_write_ns = bench_cell_write_ns(columns, rows);

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
        r.parser_ascii_mb_s, parser_pct_of_memcpy,
        r.parser_sgr_mb_s,
        r.parser_mixed_mb_s,
        r.grid_ascii_mb_s, grid_pct_of_memcpy,
        r.grid_utf8_mb_s,
        r.grid_sgr_color_mb_s,
        r.terminal_ascii_mb_s, terminal_pct_of_memcpy,
        r.terminal_sgr_mb_s,
        r.terminal_mixed_mb_s,
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
    }
}
