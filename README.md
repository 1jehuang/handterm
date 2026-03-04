# handterm

`handterm` is a Wayland-native terminal emulator project focused on low startup latency and minimal overhead.

This repository currently contains the foundation: CLI, config loading, style defaults, and a Wayland-only starter surface.

## Why "handterm"

The name is a nod to `foot`: lean, practical, and fast.

## Current status

- Wayland-only runtime scaffolding with `winit`
- Software startup surface via `softbuffer`
- Config model with defaults matching the author's current kitty styling
- CLI commands:
  - `handterm print-config`
  - `handterm init-config`
- Unit + integration tests for config and CLI behavior

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
```

Run the app (Wayland):

```bash
cargo run
```

## Roadmap

1. PTY process management and shell spawn
2. Terminal grid + parser (core ANSI/VT sequences)
3. GPU renderer and damage tracking
4. Input stack (keyboard, mouse, bracketed paste)
5. Daemon mode for ultra-fast window spawning
6. Extended compatibility (kitty keyboard/graphics, OSC 8/52)

## License

MIT
