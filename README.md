<div align="center">

# handterm

A Wayland-native terminal emulator focused on reaching the theoretical limits of performance and resource efficiency.

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-2024-orange.svg)](https://www.rust-lang.org)
[![Wayland](https://img.shields.io/badge/wayland-native-green.svg)](https://wayland.freedesktop.org)

~8,400 lines of Rust. 3.4 MB binary. 28ms to first frame.

![handterm screenshot](assets/screenshot.png)

</div>

## Why handterm

Every terminal emulator is "fast enough." Handterm asks a different question: **how close to the hardware limits can a terminal get?**

Each layer of the pipeline is independently benchmarked against its theoretical floor. The parser is measured against `memcpy`. Cell writes are timed in nanoseconds. Startup is measured in microseconds. The goal is not to be marginally faster, but to understand where the ceilings are and sit as close to them as the hardware allows.

Handterm has both a CPU renderer (softbuffer) and a GPU renderer (wgpu), with a [roadmap to server/client architecture](OPTIMIZATION.md) that targets **<1 MB RSS per window** - roughly 3x less memory than foot's daemon mode.

## Benchmarks

All measurements taken on the same machine, same session, no other GPU-intensive work running.

**Test system:** Intel Core Ultra 7 256V, 15 GB RAM, Arch Linux, niri Wayland compositor, 2560x1600 @ 120 Hz.

**Methodology:** Each terminal launched 3 times. Startup measured by polling `niri msg windows` at 2ms intervals. Memory from `/proc/<pid>/status` after 1s idle. Binary size is the on-disk ELF.

### Startup time

Time from `exec()` to window visible on the Wayland compositor.

| Terminal | Best | Median | Worst |
|----------|-----:|-------:|------:|
| **handterm** | **28 ms** | **30 ms** | **31 ms** |
| foot | 33 ms | 41 ms | 42 ms |
| alacritty | 91 ms | 128 ms | 145 ms |
| kitty | 186 ms | 247 ms | 247 ms |
| ghostty | 641 ms | 707 ms | 807 ms |

handterm starts ~1.4x faster than foot, ~4x faster than alacritty, ~8x faster than kitty, and ~24x faster than ghostty.

### Memory usage (single idle window)

| Terminal | RSS | vs handterm |
|----------|----:|------------:|
| **handterm** | **23 MB** | 1.0x |
| foot | 24 MB | 1.04x |
| footclient | 1.6 MB\* | 0.07x |
| alacritty | 84 MB | 3.7x |
| kitty | 117 MB | 5.1x |
| ghostty | 154 MB | 6.7x |

\*footclient shares a server process (25 MB). Total for first window is ~27 MB; each additional adds ~1.6 MB.

#### Memory breakdown

| Component | handterm | foot | alacritty | kitty | ghostty |
|-----------|--------:|-----:|----------:|------:|--------:|
| Framebuffers (SHM) | ~20 MB | ~20 MB | - | - | - |
| GPU context | - | - | ~50 MB | ~80 MB | ~100 MB |
| Font cache + heap | ~1 MB | ~1 MB | ~5 MB | ~10 MB | ~10 MB |
| Binary code pages | ~3 MB | ~0.5 MB | ~9 MB | ~18 MB | ~26 MB |
| Shared libs | varies | varies | varies | varies | varies |

CPU renderers (handterm, foot) pay for framebuffers in RSS. GPU renderers pay for driver context, shader compilation, and texture atlases.

### Binary and install size

| Terminal | Binary | Install total | Language |
|----------|-------:|:-------------:|:--------:|
| foot | 477 KB | ~1 MB | C |
| **handterm** | **3.4 MB** | **3.4 MB** | Rust |
| alacritty | 8.9 MB | ~9 MB | Rust |
| kitty | 88 KB\* | ~18 MB | C + Python |
| ghostty | 26 MB | ~29 MB | Zig |

\*kitty's binary is a Python launcher; the real code lives in `/usr/lib/kitty/` (18 MB of `.so` files and Python).

### Thread count

| Terminal | Threads |
|----------|---------:|
| **handterm** | **2** |
| kitty | 7 |
| foot | 9 |
| alacritty | 10 |
| ghostty | 25 |

### Shared library dependencies

Number of unique `.so` files mapped into the process.

| Terminal | Shared libs |
|----------|------------:|
| foot | 22 |
| **handterm** | **24** |
| alacritty | 52 |
| kitty | 85 |
| ghostty | 163 |

### Virtual memory (VSZ)

Total virtual address space mapped (not all resident).

| Terminal | VSZ |
|----------|----:|
| **handterm** | **119 MB** |
| kitty | 463 MB |
| alacritty | 726 MB |
| foot | 1,529 MB |
| ghostty | 2,000 MB |

foot's high VSZ is from mmap'd font files and Wayland protocol buffers; most is not resident. GPU terminals reserve large virtual ranges for driver allocations.

### Daemon mode (multi-window efficiency)

| Setup | First window | Each additional |
|-------|-------------:|----------------:|
| foot standalone | 24 MB | +24 MB |
| foot --server + footclient | 25 MB (server) + 1.6 MB | +1.6 MB |
| **handterm** (planned server mode) | ~13 MB (server) | **<1 MB** |

handterm's planned daemon mode (see [OPTIMIZATION.md](OPTIMIZATION.md)) targets <1 MB per additional window by sharing the font cache, GPU context, and grid memory across a thin client/server split.

| Windows | foot standalone | foot daemon | handterm daemon (target) |
|--------:|---------------:|------------:|-------------------------:|
| 1 | 24 MB | 27 MB | 13 MB |
| 5 | 120 MB | 33 MB | 17 MB |
| 10 | 240 MB | 41 MB | 22 MB |

### Feature comparison

| Feature | handterm | foot | alacritty | kitty | ghostty |
|---------|:--------:|:----:|:---------:|:-----:|:-------:|
| GPU rendering | ✅ | - | ✅ | ✅ | ✅ |
| CPU rendering | ✅ | ✅ | - | - | - |
| True color (24-bit) | ✅ | ✅ | ✅ | ✅ | ✅ |
| Ligatures | ✅ | ✅ | - | ✅ | ✅ |
| Sixel graphics | - | ✅ | - | - | ✅ |
| Kitty image protocol | - | - | - | ✅ | ✅ |
| Daemon mode | planned | ✅ | - | - | - |
| Tabs | - | - | - | ✅ | ✅ |
| Splits/panes | - | - | - | ✅ | ✅ |
| Bracketed paste | ✅ | ✅ | ✅ | ✅ | ✅ |
| Mouse reporting | ✅ | ✅ | ✅ | ✅ | ✅ |
| OSC 52 clipboard | ✅ | ✅ | ✅ | ✅ | ✅ |
| Kitty keyboard protocol | partial | ✅ | - | ✅ | ✅ |
| IPC / remote control | ✅ | - | - | ✅ | - |
| X11 support | - | - | ✅ | ✅ | ✅ |
| macOS support | - | - | ✅ | ✅ | ✅ |
| Font shaping engine | rustybuzz | harfbuzz | built-in | harfbuzz | harfbuzz |
| Config format | TOML | INI | TOML | conf | custom |
| Scrollback (default) | 10,000 | 10,000 | 10,000 | 2,000 | 10,000 |

### Pipeline throughput

From `handterm bench`. Internal processing speed, not rendering.

| Stage | ASCII | SGR color | Mixed |
|-------|------:|----------:|------:|
| Theoretical floor (memcpy) | 5,944 MB/s | - | - |
| Theoretical floor (byte scan) | 2,088 MB/s | - | - |
| Parser (state machine) | 279 MB/s | 362 MB/s | 339 MB/s |
| Grid write (parser + cells) | 363 MB/s | 328 MB/s | 259 MB/s |
| Full pipeline | 330 MB/s | 174 MB/s | 209 MB/s |

At 120x72 (HiDPI fullscreen), the pipeline can repaint the entire screen **2,507 times per second**. A 120 Hz display needs 1.

### Codebase size

| Terminal | Lines of code | Language | Dependencies |
|----------|-------------:|:--------:|:------------:|
| **handterm** | **~8,400** | Rust | 16 direct, ~290 total |
| alacritty | ~34,000 | Rust | ~100+ crates |
| foot | ~55,000 | C | system libs only |
| kitty | ~116,000 | C + Python | system libs + Python stdlib |
| ghostty | ~230,000 | Zig | vendored deps |

### How to reproduce

```bash
# Startup time (requires niri compositor)
before=$(niri msg windows | grep -c "Window ID")
start=$(date +%s%N)
<terminal> &
pid=$!
while [ $(niri msg windows | grep -c "Window ID") -eq $before ]; do sleep 0.002; done
end=$(date +%s%N)
echo "$(( (end - start) / 1000000 )) ms"
kill $pid

# Memory
<terminal> &
pid=$!
sleep 1
grep VmRSS /proc/$pid/status

# Pipeline throughput
handterm bench
```

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

