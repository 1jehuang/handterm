# handterm

`handterm` is a Wayland-native terminal emulator project focused on low startup latency and minimal overhead.

This repository currently contains the foundation: CLI, config loading, style defaults, and a Wayland-only starter surface.

## Why "handterm"

The name is a nod to `foot`: lean, practical, and fast.

## Current status

- Wayland-only runtime with `winit` + `softbuffer` CPU rendering
- PTY spawn and shell management
- Full terminal grid with ring buffer and 10k line scrollback
- VT100/VT220 parser (CSI, SGR, OSC, DCS)
- True color (24-bit RGB), bold, dim, italic, underline, inverse, strikethrough
- DECAWM auto-wrap with pending wrap semantics
- Mouse reporting (X10, Normal, Button, Any, SGR encoding)
- Text selection with clipboard copy (OSC 52, wl-copy)
- Unicode with on-demand FreeType glyph rasterization
- Wide character and DEC special graphics support
- Alt screen, cursor styles, bracketed paste, focus events
- IPC remote control via Unix socket
- Damage tracking (bitset dirty map, skip unchanged cells)
- Config model with defaults matching kitty styling
- CLI commands: `print-config`, `init-config`, `bench`

## Default style baseline (mirrors kitty config)

- Background: `#000000`
- Foreground: `#cdd6f4`
- Cursor: `#f5e0dc`
- Font family: `JetBrainsMono Nerd Font Light`
- Font size: `11`
- Background opacity: `0.9`
- Background blur target: `20`
- Initial geometry: `80x24`
- Scrollback lines: `10000`

## Local development

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo run -- print-config
cargo run -- bench
```

Run the app (Wayland):

```bash
cargo run
```

## Roadmap

1. ~~PTY process management and shell spawn~~ Done
2. ~~Terminal grid + parser (core ANSI/VT sequences)~~ Done
3. GPU renderer (wgpu) and ~~damage tracking~~ Done
4. ~~Input stack (keyboard, mouse, bracketed paste)~~ Done
5. Daemon mode for ultra-fast window spawning
6. Extended compatibility (kitty keyboard/graphics, OSC 8/52)

## Measurable performance baseline

`handterm bench` reports throughput at every layer of the pipeline, compared to theoretical floors (memcpy, byte scan). Key metrics on Intel Core Ultra 7 256V:

| Metric | Value |
|--------|-------|
| Terminal ASCII throughput | ~400 MB/s |
| Cell write latency | ~2.8 ns |
| PTY spawn | ~150 us |
| Grid alloc | ~12 us |
| Frames/sec (80x24) | ~12,000+ |

Run `handterm bench` for the full breakdown including parser, grid write, and full terminal pipeline throughput with ASCII, SGR, and mixed workloads.

## License

MIT
