<div align="center">

# handterm

A Wayland-native terminal emulator focused on reaching the theoretical limits of performance and resource efficiency.

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-2024-orange.svg)](https://www.rust-lang.org)
[![Wayland](https://img.shields.io/badge/wayland-native-green.svg)](https://wayland.freedesktop.org)

~8,300 lines of Rust. 3.4 MB binary. 28ms to first frame.

![handterm screenshot](assets/screenshot.png)

</div>

## Why handterm

Every terminal emulator is "fast enough." Handterm asks a different question: **how close to the hardware limits can a terminal get?**

Each layer of the pipeline is independently benchmarked against its theoretical floor. The parser is measured against `memcpy`. Cell writes are timed in nanoseconds. Startup is measured in microseconds. The goal is not to be marginally faster, but to understand where the ceilings are and sit as close to them as the hardware allows.

Handterm has both a CPU renderer (softbuffer) and a GPU renderer (wgpu), with a [roadmap to server/client architecture](OPTIMIZATION.md) that targets **<1 MB RSS per window** - roughly 3x less memory than foot's daemon mode.

## Performance

All numbers measured on an Intel Core Ultra 7 256V, Arch Linux, niri Wayland compositor.
Full methodology and reproduction steps in [BENCHMARKS.md](BENCHMARKS.md).

### Startup time (to window visible)

| Terminal | Time | vs handterm |
|----------|-----:|------------:|
| **handterm** | **28 ms** | - |
| foot | 33 ms | 1.2x slower |
| alacritty | 91 ms | 3.3x slower |
| kitty | 186 ms | 6.6x slower |
| ghostty | 641 ms | 22.9x slower |

### Memory (single idle window)

| Terminal | RSS | Threads | Shared libs |
|----------|----:|--------:|------------:|
| **handterm** | **23 MB** | **2** | **24** |
| foot | 24 MB | 9 | 22 |
| alacritty | 84 MB | 10 | 52 |
| kitty | 117 MB | 7 | 85 |
| ghostty | 154 MB | 25 | 163 |

### Binary and install size

| Terminal | Binary | Install total | Language |
|----------|-------:|:-------------:|:--------:|
| foot | 477 KB | ~1 MB | C |
| **handterm** | **3.4 MB** | **3.4 MB** | Rust |
| alacritty | 8.9 MB | ~9 MB | Rust |
| kitty | 88 KB\* | ~18 MB | C + Python |
| ghostty | 26 MB | ~29 MB | Zig |

\*kitty's binary is a Python launcher; the real code lives in `/usr/lib/kitty/` (18 MB).

### Pipeline throughput

From `handterm bench`. Internal processing speed, not rendering.

| Stage | ASCII | SGR color | Mixed |
|-------|------:|----------:|------:|
| Theoretical floor (memcpy) | 5,944 MB/s | - | - |
| Parser (state machine) | 279 MB/s | 362 MB/s | 339 MB/s |
| Full pipeline (parser + grid + state) | 330 MB/s | 174 MB/s | 209 MB/s |

At 120x72 (HiDPI fullscreen), the pipeline can repaint the entire screen **2,507 times per second**.

See [BENCHMARKS.md](BENCHMARKS.md) for the complete comparison: feature matrix, daemon mode projections, codebase size, virtual memory, and memory breakdowns.

## Features

**Terminal emulation**
- VT100/VT220 parser: CSI, SGR, OSC, DCS, ESC sequences
- True color (24-bit RGB), 256 color palette, bold, dim, italic, inverse, strikethrough
- Underline styles: single, double, curly, dotted, dashed (with custom colors)
- DECAWM auto-wrap with pending wrap semantics
- Scroll regions, insert/delete lines and characters
- Alt screen, cursor save/restore, cursor styles (block, bar, underline)
- DEC special graphics (line drawing characters)
- Device attributes (DA1/DA2), device status reports

**Rendering**
- CPU renderer via softbuffer (default)
- GPU renderer via wgpu with instanced cell rendering and WGSL shaders
- Two-pass rendering (backgrounds then glyphs) for correct powerline/nerd font display
- Damage tracking with bitset dirty map
- Ligature support via rustybuzz text shaping
- DPI-aware rendering (HiDPI)

**Input and interaction**
- Full keyboard input with Ctrl, Shift, function keys
- Mouse reporting: X10, Normal, Button, Any-event, SGR encoding
- Bracketed paste mode
- Focus events
- Text selection with mouse drag, copy-on-select via wl-copy
- 10,000 line scrollback with ring buffer

**Unicode**
- On-demand FreeType glyph rasterization
- Wide character support (CJK, emoji)
- Fontconfig font discovery with caching

**Shell integration**
- Kitty keyboard protocol query response
- XTVERSION response
- OSC 10/11 color queries
- OSC 52 clipboard
- OSC 0/2 window title

**IPC**
- Unix socket remote control (`handterm @ <command>`)
- Commands: get-text, send-text, send-key, get-cursor, get-size, set-title, close

## Install

Requires Wayland, FreeType, and Fontconfig.

```bash
# From source
cargo install --path .

# Or build directly
cargo build --release
./target/release/handterm
```

### Build with GPU rendering

```bash
cargo build --release --features gpu --no-default-features
```

## Configuration

Config file: `~/.config/handterm/config.toml`

Generate defaults:

```bash
handterm init-config
```

Example:

```toml
[style]
font_family = "JetBrainsMono Nerd Font Light"
font_size = 11.0
background = "#000000"
foreground = "#cdd6f4"
cursor = "#f5e0dc"
background_opacity = 0.9

[window]
columns = 80
rows = 24

[scrollback]
lines = 10000

[performance]
repaint_delay_ms = 5
sync_to_monitor = true
```

## Architecture

```
PTY (forkpty)
  |
  v
Parser (byte-at-a-time state machine)
  |
  v
Terminal (CSI/SGR/OSC dispatch, mode tracking)
  |
  v
Grid (ring buffer cells, damage tracking)
  |
  v
Renderer (CPU: two-pass pixel blit / GPU: instanced wgpu pipeline)
  |
  v
Wayland surface (softbuffer SHM / wgpu swapchain)
```

Each layer is independently benchmarkable. The parser can be tested without a grid. The grid can be tested without a renderer. `handterm bench` measures every boundary.

### Source layout

```
src/
  main.rs        Entry point and CLI
  parser.rs      VT state machine (439 lines)
  terminal.rs    Sequence dispatch, mode state (1,458 lines)
  grid.rs        Cell storage, ring buffer, dirty tracking (1,117 lines)
  font.rs        FreeType rasterization, glyph cache, ligatures (696 lines)
  render.rs      CPU renderer (546 lines)
  gpu_app.rs     GPU renderer with wgpu + WGSL (1,197 lines)
  app.rs         CPU app / winit event loop (504 lines)
  frontend.rs    Shared input handling, frame scheduling (528 lines)
  pty.rs         PTY spawn and I/O (132 lines)
  ipc.rs         Unix socket IPC server (259 lines)
  config.rs      TOML config loading (186 lines)
  color.rs       Hex color type (114 lines)
  metrics.rs     Built-in benchmarks (313 lines)
  cli.rs         Clap CLI definitions (37 lines)
```

## Development

```bash
cargo test              # 88 tests (parser, terminal, grid, font, config, CLI)
cargo run -- bench      # full pipeline benchmark
cargo run -- print-config
```

## Roadmap

See [OPTIMIZATION.md](OPTIMIZATION.md) for the full performance roadmap.

| Phase | Goal | Status |
|-------|------|--------|
| CPU rendering | Functional terminal with softbuffer | ✅ |
| GPU rendering | wgpu backend with instanced shaders | ✅ |
| GPU as default | Eliminate CPU framebuffer memory overhead | planned |
| Server/client mode | Daemon architecture like foot --server | planned |
| Workspace split | Thin client binary without font libs | planned |
| Zero-copy IPC | Shared memory cell grid between server and client | planned |

**Target: <1 MB per window, ~13 MB total for 10 windows** (vs foot's ~41 MB).

## License

MIT

