<div align="center">

# handterm

A Wayland-native terminal emulator focused on reaching the theoretical limits of performance and resource efficiency.

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-2024-orange.svg)](https://www.rust-lang.org)
[![Wayland](https://img.shields.io/badge/wayland-native-green.svg)](https://wayland.freedesktop.org)

~8,400 lines of Rust. Workspace-split packages. Single-process host architecture for low-overhead multi-window scaling.

![handterm screenshot](assets/screenshot.png)

</div>

## Why handterm

Every terminal emulator is "fast enough." Handterm asks a different question: **how close to the hardware limits can a terminal get?**

Each layer of the pipeline is independently benchmarked against its theoretical floor. The parser is measured against `memcpy`. Cell writes are timed in nanoseconds. Startup is measured in microseconds. The goal is not to be marginally faster, but to understand where the ceilings are and sit as close to them as the hardware allows.

Handterm has both a CPU renderer (softbuffer) and a GPU renderer (wgpu). The current best low-memory path is now a **single-process host architecture**: one long-lived process owns many windows and shares the heavy renderer/runtime state across them. The long-term target remains **<1 MB RSS per additional window**, and the current shared-GPU host is already close to that regime in practice.

## Benchmarks

All measurements below refer to the **current host-based handterm architecture**, not the older one-process-per-window standalone path.

**Test system:** Intel Core Ultra 7 256V, 15 GB RAM, Arch Linux, niri Wayland compositor, 2560x1600 @ 120 Hz.

### What the numbers mean now

Handterm currently has three relevant local architectures:

1. **CPU host** — one process, many CPU-rendered windows
2. **GPU host** — one process, many GPU-rendered windows
3. **daemon mode** — server plus separate thin clients (**still implemented, but soft-deprecated for normal local use** and no longer the best local low-RAM path)

### Current measured memory

#### Host memory scaling

| Setup | First window | Each additional |
|-------|-------------:|----------------:|
| handterm CPU host | ~37 MB | ~20 MB |
| **handterm GPU host** | **~61 MB** | **~1-2 MB** |
| handterm daemon server-only | ~4.4 MB | N/A |

The current best local low-RAM path is the **shared-GPU host**. It pays a large fixed first-window cost once, then scales cheaply for additional windows.

Measured shared-GPU host scaling on this machine/session:

| Windows | handterm GPU host RSS |
|--------:|-----------------------:|
| 1 | ~61.3 MB |
| 2 | ~63.1 MB |
| 3 | ~64.0 MB |
| 4 | ~65.0 MB |
| 5 | ~67.2 MB |
| 6 | ~68.1 MB |

That puts the incremental cost in roughly the **1-2 MB/window** range after the first window.

#### Why CPU host is still larger per added window

The CPU host path improved substantially, but the dominant remaining per-window cost is the Wayland/softbuffer SHM presentation buffers. In other words, the CPU host is no longer mainly limited by terminal state — it is limited by presentation-buffer memory.

### Current measured startup behavior

#### CPU host

- added-window startup from an existing host: ~**16 ms**

#### GPU host

The shared-GPU host is the best path for memory, but new-window startup is still the main remaining bottleneck. Recent measured `open-window` timings on the shared-GPU host are in the **tens of milliseconds**, with the first extra window sometimes noticeably slower than later ones.

Internal timing instrumentation shows the remaining cost is mostly in:

- **window / surface creation**

not PTY spawn.

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

### Multi-window efficiency

| Setup | First window | Each additional |
|-------|-------------:|----------------:|
| foot standalone | 24 MB | +24 MB |
| foot --server + footclient | 25 MB (server) + 1.6 MB | +1.6 MB |
| handterm CPU host | ~37 MB | ~20 MB |
| **handterm GPU host** | **~61 MB** | **~1-2 MB** |
| handterm daemon mode | ~4.4 MB server-only | current clients still too heavy |

Handterm still has a daemon/thin-client implementation, but it is now **soft-deprecated** for normal local use. After profiling both approaches, the most promising low-RAM local architecture is the **shared-GPU single-process host**. In current live measurements, the first GPU host window pays the full GPU/runtime cost once, and each additional window adds only about **1-2 MB RSS**.

Measured shared-GPU host scaling on this machine/session:

| Windows | handterm GPU host RSS |
|--------:|-----------------------:|
| 1 | ~61.3 MB |
| 2 | ~63.1 MB |
| 3 | ~64.0 MB |
| 4 | ~65.0 MB |
| 5 | ~67.2 MB |
| 6 | ~68.1 MB |

That puts the incremental cost in roughly the **1-2 MB/window** range after the first window.

### Feature comparison

| Feature | handterm | foot | alacritty | kitty | ghostty |
|---------|:--------:|:----:|:---------:|:-----:|:-------:|
| GPU rendering | ✅ | - | ✅ | ✅ | ✅ |
| CPU rendering | ✅ | ✅ | - | - | - |
| True color (24-bit) | ✅ | ✅ | ✅ | ✅ | ✅ |
| Ligatures | ✅ | ✅ | - | ✅ | ✅ |
| Sixel graphics | - | ✅ | - | - | ✅ |
| Kitty image protocol | partial | - | - | ✅ | ✅ |
| Daemon mode | ✅ | ✅ | - | - | - |
| Single-process multi-window host | ✅ | - | - | ✅ | ✅ |
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

# Synthetic frontend input dedupe (GPU by default; uses socket + isolated runtime dir)
./scripts/test_input_dedupe.sh

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
- GPU renderer via wgpu with instanced cell rendering and WGSL shaders (default when built)
- CPU renderer via softbuffer (`--backend cpu`, but Wayland presentation remains opaque)
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
- Kitty keyboard protocol set/query/push/pop + expanded CSI-u key encoding
- XTVERSION response
- OSC 10/11 color queries
- OSC 52 clipboard
- OSC 0/2 window title

**IPC / host control**
- Unix socket remote control (`handterm @ <command>`)
- Host window creation via `handterm open-window`
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

This default build includes both CPU and GPU frontends. Plain local `handterm` now follows the **host path by default**: when a compatible host is already running, repeated launches reuse that host and open another window in the same process instead of spawning a full new renderer process.

### Build with GPU rendering only

```bash
cargo build --release --features gpu --no-default-features
```

### Build with CPU rendering only

```bash
cargo build --release --features cpu --no-default-features
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

`background_opacity` is implemented by the GPU backend. If you force `--backend cpu`, Handterm will stay opaque on Wayland because `softbuffer` presents `Xrgb8888` there.

## Architecture

```
Single-process host
  |
  +-- window #1
  |     PTY -> parser -> terminal -> grid -> renderer -> Wayland surface
  |
  +-- window #2
  |     PTY -> parser -> terminal -> grid -> renderer -> Wayland surface
  |
  +-- window #N
        PTY -> parser -> terminal -> grid -> renderer -> Wayland surface

Shared across windows in the host:
- event loop
- IPC socket / control plane
- CPU or GPU renderer runtime foundation
- glyph/font resources per DPI
- on the GPU path: shared `wgpu` instance / adapter / device / queue / pipeline cache
```

Each layer is independently benchmarkable. The parser can be tested without a grid. The grid can be tested without a renderer. `handterm bench` measures every boundary.

### Source layout

```
handterm-common/
  src/
    grid.rs       Cell storage / scrollback
    parser.rs     VT parser
    protocol.rs   daemon protocol types
    terminal.rs   terminal state machine

src/
  app.rs          CPU single-process host runtime
  gpu_app.rs      GPU single-process host runtime
  gpu_runtime.rs  shared/per-window GPU runtime pieces
  render.rs       CPU renderer
  frontend.rs     shared scheduling/input helpers
  pty.rs          PTY spawn and I/O
  ipc.rs          host IPC server
  daemon.rs       daemon/server runtime
  remote_app.rs   CPU thin client
  remote_gpu_app.rs GPU thin client
```

## Development

```bash
cargo test --workspace
cargo run -- bench
cargo run -- print-config
```

## Roadmap

See [OPTIMIZATION.md](OPTIMIZATION.md) for the full performance roadmap.

**Architecture status:** the default and recommended path is the single-process host architecture. Daemon/thin-client mode remains available for compatibility and experimentation, but is soft-deprecated.

| Phase | Goal | Status |
|-------|------|--------|
| CPU rendering | Functional terminal with softbuffer | ✅ |
| GPU rendering | wgpu backend with instanced shaders | ✅ |
| Single-process CPU host | Shared-process multi-window CPU runtime | ✅ |
| Shared-GPU host | Low-overhead multi-window GPU runtime | ✅ |
| Server/client mode | Daemon architecture like foot --server | ✅ implemented, soft-deprecated |
| Workspace split | Thin client/server/common Cargo packages | ✅ foundation implemented |
| Startup/per-window polish | Push toward theoretical window-overhead floor | in progress |

**Current best path:** shared-GPU host at roughly **~1-2 MB per extra window** after the first window. The remaining work is shaving startup cost and pushing the incremental slope down further.

## License

MIT
