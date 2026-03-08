# Terminal Emulator Benchmarks

All measurements taken on the same machine, same session, no other GPU-intensive work running.

**Test system:** Intel Core Ultra 7 256V, 15 GB RAM, Arch Linux, niri Wayland compositor, 2560x1600 display at 120 Hz.

**Methodology:** Each terminal was launched 3 times. Startup time is measured by polling `niri msg windows` at 2ms intervals until the window appears. Memory is read from `/proc/<pid>/status` after 1 second of idle. Binary size is the on-disk ELF. Install size includes runtime data (`/usr/lib/<name>/` etc).

## Startup time

Time from `exec()` to window visible on the Wayland compositor.

| Terminal | Best | Median | Worst |
|----------|-----:|-------:|------:|
| **handterm** | **28 ms** | **30 ms** | **31 ms** |
| foot | 33 ms | 41 ms | 42 ms |
| alacritty | 91 ms | 128 ms | 145 ms |
| kitty | 186 ms | 247 ms | 247 ms |
| ghostty | 641 ms | 707 ms | 807 ms |

handterm starts ~1.4x faster than foot, ~4x faster than alacritty, ~8x faster than kitty, and ~24x faster than ghostty.

## Memory usage (single idle window)

RSS (Resident Set Size) of a single idle window with default config and a shell prompt.

| Terminal | RSS | vs handterm |
|----------|----:|------------:|
| **handterm** | **23 MB** | 1.0x |
| foot | 24 MB | 1.04x |
| footclient | 1.6 MB\* | 0.07x |
| alacritty | 84 MB | 3.7x |
| kitty | 117 MB | 5.1x |
| ghostty | 154 MB | 6.7x |

\*footclient shares a server process (25 MB). Total for first window is ~27 MB; each additional window adds ~1.6 MB.

### Memory breakdown

| Component | handterm | foot | alacritty | kitty | ghostty |
|-----------|--------:|-----:|----------:|------:|--------:|
| Framebuffers (SHM) | ~20 MB | ~20 MB | - | - | - |
| GPU context | - | - | ~50 MB | ~80 MB | ~100 MB |
| Font cache + heap | ~1 MB | ~1 MB | ~5 MB | ~10 MB | ~10 MB |
| Binary code pages | ~3 MB | ~0.5 MB | ~9 MB | ~18 MB | ~26 MB |
| Shared libs | varies | varies | varies | varies | varies |

CPU renderers (handterm, foot) pay for framebuffers in RSS. GPU renderers (alacritty, kitty, ghostty) pay for GPU driver context, shader compilation, and texture atlases.

## Binary and install size

| Terminal | Binary | Install total | Language |
|----------|-------:|:-------------:|:--------:|
| foot | 477 KB | ~1 MB | C |
| **handterm** | **3.4 MB** | **3.4 MB** | Rust |
| alacritty | 8.9 MB | ~9 MB | Rust |
| kitty | 88 KB\* | ~18 MB | C + Python |
| ghostty | 26 MB | ~29 MB | Zig |

\*kitty's binary is a Python launcher; the real code lives in `/usr/lib/kitty/` (18 MB of `.so` files and Python).

## Thread count

Threads at idle with a single shell prompt.

| Terminal | Threads |
|----------|---------:|
| **handterm** | **2** |
| kitty | 7 |
| foot | 9 |
| alacritty | 10 |
| ghostty | 25 |

## Shared library dependencies

Number of unique `.so` files mapped into the process.

| Terminal | Shared libs |
|----------|------------:|
| foot | 22 |
| **handterm** | **24** |
| alacritty | 52 |
| kitty | 85 |
| ghostty | 163 |

## Virtual memory (VSZ)

Total virtual address space mapped (not all resident).

| Terminal | VSZ |
|----------|----:|
| **handterm** | **119 MB** |
| kitty | 463 MB |
| alacritty | 726 MB |
| foot | 1,529 MB |
| ghostty | 2,000 MB |

foot's high VSZ is from mmap'd font files and Wayland protocol buffers; most is not resident. GPU terminals reserve large virtual ranges for driver allocations.

## Daemon mode (multi-window efficiency)

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

## Feature comparison

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

## Pipeline throughput (handterm-specific)

From `handterm bench`. These measure the terminal's internal processing speed, not rendering.

| Stage | ASCII | SGR color | Mixed |
|-------|------:|----------:|------:|
| Theoretical floor (memcpy) | 5,944 MB/s | - | - |
| Theoretical floor (byte scan) | 2,088 MB/s | - | - |
| Parser (state machine) | 279 MB/s | 362 MB/s | 339 MB/s |
| Grid write (parser + cells) | 363 MB/s | 328 MB/s | 259 MB/s |
| Full pipeline | 330 MB/s | 174 MB/s | 209 MB/s |

At 120x72 (HiDPI fullscreen), the pipeline can repaint the entire screen **2,507 times per second**. A 120 Hz display needs 1.

## Codebase size

| Terminal | Lines of code | Language | Dependencies |
|----------|-------------:|:--------:|:------------:|
| **handterm** | **~8,300** | Rust | 16 direct, ~290 total |
| foot | ~30,000 | C | system libs only |
| alacritty | ~30,000 | Rust | ~100+ crates |
| kitty | ~60,000 | C + Python | system libs + Python stdlib |
| ghostty | ~100,000+ | Zig | vendored deps |

## How to reproduce

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
