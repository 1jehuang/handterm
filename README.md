# handterm

A Wayland-native terminal emulator that tries to hit the theoretical limits of performance. No GPU, no runtime, no framework - just a parser, a grid, and pixels.

~5k lines of Rust. 2.6 MB binary. 28 MB RSS.

## Philosophy

Most terminals are fast enough. handterm asks: how fast is *possible*?

Every layer is measured against its theoretical floor. The parser is benchmarked against raw `memcpy`. Cell writes are timed in nanoseconds. Startup is measured in microseconds. The goal isn't to be marginally faster - it's to understand where the ceilings are and sit as close to them as the hardware allows.

It's a toy terminal. It runs fish, neovim, and starship. It doesn't try to replace kitty.

## Benchmarks

All numbers from `handterm bench` on an Intel Core Ultra 7 256V.

### Pipeline throughput

Each layer of the terminal pipeline, measured independently:

| Stage | ASCII | SGR color | Mixed |
|-------|------:|----------:|------:|
| Theoretical floor (memcpy) | 5,944 MB/s | - | - |
| Theoretical floor (byte scan) | 2,088 MB/s | - | - |
| Parser (state machine only) | 279 MB/s | 362 MB/s | 339 MB/s |
| Grid write (parser + cells) | 363 MB/s | 328 MB/s | 259 MB/s |
| Full pipeline (parser + grid + terminal state) | 330 MB/s | 174 MB/s | 209 MB/s |

The parser runs at ~5% of memcpy speed. Per-byte state machine dispatch is the bottleneck - not memory bandwidth.

### Per-cell metrics

| Metric | Value |
|--------|------:|
| Cell struct size | 16 bytes |
| Cell write latency | 2.7 ns |
| Grid memory (80x24) | 30 KB |
| Grid memory (120x72) | 135 KB |
| Scrollback per line (80 cols) | 1,280 bytes |
| 10k line scrollback | 12.2 MB |

### Frame budget

| Grid size | Theoretical frames/sec | Full-screen write |
|-----------|----------------------:|-----------------:|
| 80x24 (classic) | 11,279 | 89 us |
| 120x72 (fullscreen HiDPI) | 2,507 | 399 us |

At 120x72 the terminal pipeline can fill the entire screen 2,507 times per second. A 144 Hz display needs 1.

### Startup

| Phase | Time |
|-------|-----:|
| PTY spawn (forkpty + exec) | 266 us |
| Shell ready | 42 us |
| Grid alloc | 9 us |

### Comparison with other terminals

Resource usage for a single idle terminal window:

| Terminal | Binary | Installed size | RSS (idle) | Shared libs | Renderer |
|----------|-------:|---------------:|-----------:|------------:|----------|
| **handterm** | **2.6 MB** | **2.6 MB** | **28 MB** | 12 | CPU (softbuffer) |
| foot | 477 KB | 778 KB | 9 MB | 22 | Wayland pixel buffer |
| alacritty | 8.9 MB | 8.6 MB | - | 12 | GPU (OpenGL) |
| kitty | 88 KB* | 61 MB | 375 MB | 5 | GPU (OpenGL) |

*kitty's binary is a Python launcher; the actual runtime is 18 MB of Python + C modules under `/usr/lib/kitty/`.

foot wins on RSS because it uses the Wayland pixel buffer protocol directly with no toolkit. handterm uses `winit` + `softbuffer` which adds overhead. kitty's RSS includes a full Python interpreter and GPU texture memory.

Throughput comparison (approximate, from published benchmarks and architecture):

| Terminal | Parser | Rendering | Architecture |
|----------|--------|-----------|-------------|
| **handterm** | 330 MB/s (ASCII) | CPU blit | Hand-rolled VT parser, softbuffer |
| foot | ~500 MB/s* | Wayland SHM | Custom parser, pixel buffer |
| alacritty | ~200-300 MB/s | GPU upload + shader | vte crate, OpenGL |
| kitty | ~100-200 MB/s | GPU upload + shader | C core, Python glue, OpenGL |

*foot's parser throughput is estimated from its [vtebench](https://github.com/alacritty/vtebench) results and PGO-optimized builds.

## Features

**Terminal emulation**
- VT100/VT220 parser: CSI, SGR, OSC, DCS, ESC sequences
- True color (24-bit RGB), 256 color, bold, dim, italic, underline, inverse, strikethrough
- DECAWM auto-wrap with pending wrap semantics
- Scroll regions, insert/delete lines and characters
- Alt screen, cursor save/restore, cursor styles (block, bar, underline)
- DEC special graphics (line drawing characters)
- Device attributes (DA1/DA2), device status reports

**Input**
- Full keyboard input with Ctrl, Shift, function keys
- Mouse reporting: X10, Normal, Button, Any-event, SGR encoding
- Bracketed paste mode
- Focus events

**Unicode**
- On-demand FreeType glyph rasterization
- Wide character support (CJK, emoji)
- DPI-aware rendering (HiDPI/Retina)
- Fontconfig font discovery with caching

**Shell integration**
- Kitty keyboard protocol query response
- XTVERSION response
- OSC 10/11 color queries
- OSC 52 clipboard
- OSC 0/2 window title

**Other**
- Text selection with mouse drag, copy-on-select via wl-copy
- 10,000 line scrollback with ring buffer
- Two-pass rendering (backgrounds then glyphs) for correct powerline/nerd font display
- Damage tracking with bitset dirty map
- IPC remote control via Unix socket
- Config file with kitty-compatible defaults

## Install

Requires Wayland, FreeType, and Fontconfig.

```bash
cargo install --path .
```

Or build and run directly:

```bash
cargo build --release
./target/release/handterm
```

## Configuration

Default config location: `~/.config/handterm/config.toml`

Generate a default config:

```bash
handterm init-config
```

```toml
[window]
columns = 80
rows = 24

[style]
font_family = "JetBrainsMono Nerd Font Light"
font_size = 11.0
background = "#000000"
foreground = "#cdd6f4"
cursor_color = "#f5e0dc"
background_opacity = 0.9
```

## Development

```bash
cargo test            # 78 tests (parser, terminal, grid, font, CLI)
cargo run -- bench    # full pipeline benchmark
cargo run -- print-config
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
Renderer (two-pass: backgrounds then glyphs)
  |
  v
softbuffer (CPU pixel blit to Wayland surface)
```

Each layer is independently benchmarkable. The parser can be tested without a grid. The grid can be tested without a renderer. `handterm bench` measures every boundary.

## What's missing

- GPU rendering (wgpu) - would eliminate the CPU blit bottleneck
- Sixel/kitty graphics protocol
- Ligatures
- Resize reflow for wrapped lines
- Font fallback chains
- Scrollbar
- Tabs/splits

## License

MIT
