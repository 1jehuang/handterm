# Handterm Performance Optimization Roadmap

Goal: reach the theoretical minimum resource usage for a Wayland terminal emulator.

## Current state

| Metric | handterm (CPU) | foot (standalone) | footclient |
|--------|---------------|-------------------|------------|
| Startup | ~25ms | ~30-35ms | ~25-30ms |
| RSS | 21MB | 24MB | 1.6MB |
| Binary | 3.3MB | 476KB | 27KB |
| Rendering | CPU (softbuffer) | CPU (custom pixman) | shares server |

### Where the 21MB goes

| Component | Size | Notes |
|-----------|------|-------|
| Softbuffer x2 (double-buffered framebuffer) | ~19.5MB | 93% of private memory |
| Handterm binary code pages | ~3MB | |
| Heap (font cache, terminal state) | ~1MB | Already small |
| Shared libs (libc, freetype, xkbcommon, wayland) | ~3MB | Shared across processes |
| Stack + misc | ~0.1MB | Irrelevant |

The two `memfd:softbuffer` SHM mappings at ~9.8MB each (2560x1600x4 bytes, double-buffered) dominate everything.

## Implemented

- Shared frontend scheduling/input/IPC helpers were extracted so the CPU and GPU frontends are less duplicated.
- CPU rendering now has an offscreen framebuffer harness that compares incremental rendering against forced full redraws.
- PTY-driven redraws are batched to a frame deadline instead of presenting every readiness notification immediately.
- Large PTY repaint bursts now get an additional scheduler settle step after the normal burst cap, based on dirty-grid intensity, with automated tests covering long TUI-style repaint bursts.
- Both frontends now compute a visual-scene signature and skip redundant presents when the visible scene is unchanged.
- Glyph cache misses no longer reopen the configured FreeType face.
- Keyboard/input encoding now lives in a dedicated module and supports negotiated kitty keyboard protocol with automated tests.
- CPU and GPU startup now print build/backend/debug information so the running binary is unambiguous.
- GPU frame planning/batching was split out of `gpu_app.rs` into a dedicated `gpu_frame.rs` module with automated tests.
- GPU hot-path allocations were reduced by reusing frame planning, text batching, image batching, and instance vectors across frames.
- The GPU path now skips redundant default-background quads, and `hand bench` measures GPU frame-prep throughput plus CPU render throughput.
- GPU batch construction now has transcript-driven parity coverage for fish startup, starship-style prompts, and chunked TUI help overlays, including kitty image placement assertions.
- Benchmarks now include transcript-shaped prompt and TUI replay workloads for both the CPU renderer and GPU frame-prep path so parity work is measured against realistic repaint patterns.
- A dedicated binary protocol module now exists with client/server message types, length-prefixed framing helpers, encode/decode tests, and a protocol roundtrip benchmark.
- An in-process server core now exists with per-window terminal ownership, dirty-cell snapshot emission, resize/close handling, and automated lifecycle/update tests.
- The server core now handles binary protocol client messages and emits both server updates and explicit PTY-side actions, with automated command-processing coverage.
- A `server-only` daemon runtime now exists with a real Unix-socket event loop, framed protocol parsing, client/window ownership tracking, and PTY polling/forwarding.
- A reusable protocol client transport now exists, and there is an end-to-end daemon smoke test covering `NewWindow`/`CloseWindow` roundtrips over the real socket protocol.
- The thin-client foundation now includes nonblocking protocol client reads plus terminal-side application of server cell/cursor/title/clipboard/bell updates.
- Real CPU and GPU thin-client window frontends now exist, with daemon reachability checks and automatic server startup so non-standalone launches can use the daemon/client flow on either backend.
- The daemon protocol now carries negotiated client DPI in `NewWindow`, explicit kitty image/placement state snapshots, and server-driven glyph/grapheme atlas updates that remote clients apply before rendering.
- Thin clients now consume kitty image state, track alternate-screen mode, and inject protocol glyph updates into their local atlas cache instead of ignoring them.
- The server now keeps per-DPI glyph atlases, emits incremental `AtlasUpdate` payloads from dirty cells/graphemes, and deduplicates already-uploaded glyphs per window so steady-state daemon traffic stays smaller.
- Remote frontends now drop live font rasterization sources after startup geometry is established, reducing retained client-side font state while preserving protocol-injected glyph rendering.
- The server now preserves terminal-generated PTY reply bytes on the server I/O path and translates protocol mouse input into PTY mouse sequences instead of dropping it.
- The grid/protocol/font stack is now grapheme-aware for UTF-8 printable clusters, and the font layer caches rasterized grapheme bitmaps so emoji sequences like variation-selector and ZWJ clusters can render as one cell span instead of being split into separate placeholder glyphs.
- The GPU frontend now prefers non-sRGB surface formats when available for better color parity, and GPU glyph quads expand to the uploaded glyph width so prompt icons with right-side overhang are not clipped.
- The CPU frontends now keep their own persistent software framebuffer and copy that into softbuffer at present time, instead of relying on softbuffer backbuffer persistence across frames. This has automated coverage because the CPU renderer is now tested for the “persistent software framebuffer -> fresh presented front buffer” path that matches live CPU presentation more closely.
- The font path now uses explicit FreeType light hinting for normal text, derives cell width from a representative monospace sample set instead of a single rendered `M`, and procedurally rasterizes shaded/block glyphs like `░▒▓` so they stay cell-aligned and crisp.
- True configured background opacity is now wired through the GPU backend using transparent windows plus a non-opaque surface alpha mode when available. The current CPU backend cannot support real opacity on Wayland because `softbuffer` presents `Xrgb8888` there, so CPU remains intentionally opaque.

## Verification Standard

Optimization work is only complete when all three are true:

1. Automated correctness coverage exists for the code path.
2. Benchmark checkpoints show the expected improvement.
3. Documentation reflects measured reality, not intended architecture.

Manual/live checks are useful for debugging, but they are not the primary acceptance criterion.

### Theoretical floor for a Wayland terminal

| Component | Minimum |
|-----------|---------|
| GPU-rendered framebuffer | 0 RSS (lives in GPU VRAM) |
| Glyph atlas | ~256KB-1MB (GPU-side) |
| Terminal grid (visible, 200x80) | ~150KB |
| Scrollback (1000 lines) | ~500KB |
| PTY | ~64KB |
| Wayland/runtime overhead | ~2MB |
| **Total** | **~3-4MB** per standalone instance |

With a daemon model on top of GPU rendering, each additional window could be **<1MB**.

---

## Phase 1: Ship the GPU backend as default

**Status: in progress** - GPU frame planning/batching is significantly more structured and benchmarked than before, and GPU is now the preferred default backend when it is compiled in. Full framebuffer parity and broader live validation still remain.

### Tasks

1. Reach rendering parity with CPU for shell prompts, typing, resize, selection, and TUI repaint behavior.
2. Add stronger automated GPU parity tests against CPU/offscreen reference output and shared visual expectations.
   Status: partially done. Transcript-driven shared-visual tests now exist for the GPU batch builder, but end-to-end GPU framebuffer parity is still missing.
3. Verify glyph atlas upload and rendering across normal text, wide glyphs, and fallback/emoji paths.
4. Benchmark RSS, startup time, redraw throughput, and frame pacing on CPU vs GPU.
5. Keep validating the GPU default path against live shell/TUI workloads and continue tightening parity until CPU is no longer the fallback/debug backend.

### Expected result

~21MB -> ~8-10MB per instance.

### Benchmark gates

- `cargo run -- bench` before and after GPU parity work
- RSS measurement for idle window on CPU vs GPU
- startup timing for CPU vs GPU
- redraw throughput / frame pacing under shell typing and full-screen TUI repaint workloads

### Test gates

- parser/grid/terminal tests stay green
- CPU framebuffer parity tests stay green
- GPU parity tests exist and stay green
- transcript replay tests for fish/starship and at least one full-screen TUI workload

---

## Phase 2: Daemon/server mode

Split into a server process (owns PTYs, terminals, fonts) and thin client processes (own Wayland surfaces, GPU rendering).

### 2a: Wire protocol (`src/protocol.rs`)

Binary protocol over Unix socket (bincode or simple TLV, not JSON).

**Client -> Server:**
- `NewWindow { cols, rows, dpi }` - request a new terminal window for a specific client DPI
- `KeyInput { window_id, key_event }` - forward keyboard input
- `MouseInput { window_id, mouse_event }` - forward mouse input
- `Resize { window_id, cols, rows }` - window was resized
- `CloseWindow { window_id }` - window closed
- `Paste { window_id, text }` - clipboard paste

**Server -> Client:**
- `WindowCreated { window_id }` - window ready
- `CellUpdate { window_id, dirty_cells }` - only changed cells
- `SetTitle { window_id, title }` - OSC title change
- `Bell { window_id }` - terminal bell
- `CopyToClipboard { window_id, text }` - OSC 52 clipboard
- `WindowClosed { window_id }` - PTY exited
- `KittyImageState { window_id, generation, images, placements }` - current kitty RGBA image state for that window
- `AtlasUpdate { glyph, ... }` - incremental glyph/grapheme bitmap upload for the negotiated client DPI

### 2b: Server process (`handterm --server`)

The server owns all heavy resources:

- **Font loading** (freetype, fontconfig, rustybuzz) - done once, shared
- **Glyph rasterization** - shared glyph cache across all windows
- **Terminal state** - one `Terminal` + `Grid` per window
- **PTY management** - one `PtyChild` per window
- **VT parsing** - one `Parser` per window

The server does NO rendering and NO Wayland interaction. It maintains terminal state and sends dirty cell updates to clients.

```rust
struct Server {
    font: GlyphAtlas,
    windows: HashMap<WindowId, WindowState>,
    listener: UnixListener,
    clients: Vec<ClientConnection>,
}

struct WindowState {
    terminal: Terminal,
    pty: PtyChild,
    pty_buf: Vec<u8>,
}
```

Event loop: `poll()` on all PTY fds + listener socket + client sockets. When PTY data arrives, process through terminal, diff dirty cells, send updates to owning client.

### Test gates

- protocol encode/decode tests
- server PTY/update integration tests
- multi-client synchronization tests
- disconnect/reconnect and window-close tests

### Benchmark gates

- server RSS with 1, 5, and 10 windows
- client RSS with 1, 5, and 10 windows
- socket round-trip latency
- dirty-cell update throughput

### 2c: Client process (`handterm` or `handterm --connect`)

Ultra-thin process:

- Opens Wayland window (winit)
- Connects to server socket
- Receives dirty cell updates
- Renders via wgpu (client owns the GPU surface)
- Forwards keyboard/mouse events to server

The client needs:
- wgpu device/surface/pipeline (local GPU state)
- A local copy of the cell grid (for rendering)
- The glyph atlas texture (uploaded once from server)

Rendering happens on the client side. Each client has its own Wayland surface naturally, so this is the simplest architecture.

### 2d: Glyph atlas sharing

When a client connects:
1. Server sends the current rasterized glyph atlas bitmap (one-time, ~few hundred KB)
2. Client uploads it as a GPU texture
3. When new glyphs are rasterized (e.g., first CJK character), server sends incremental atlas updates

Clients don't need freetype/fontconfig linked at all.

### Kitty graphics requirement

Kitty graphics protocol support is part of project completion. The terminal parses/stores kitty image payloads and placements, and both renderers now draw the currently implemented RGBA placement path, but broader protocol coverage and performance validation are still incomplete. Before Phase 2 is considered complete:

- image decode/place/delete flows need automated coverage
- CPU and GPU renderers must draw kitty image placements correctly
- protocol/renderer performance for image upload and placement updates must be measured

### 2e: CLI interface

```
handterm                    # Default: connect to server, or start one and connect
handterm --standalone       # Force standalone mode (no daemon, current behavior)
handterm --server-only      # Start server daemon without opening a window
```

Default behavior (`handterm` with no flags):
1. Check if a server is already running at `/run/user/$UID/handterm-server.sock`
2. If yes, connect to it and open a new window (fast path, ~1-2MB)
3. If no, fork a server process in the background, wait for socket, then connect

Status: mostly done. The default non-standalone path now ensures the server is running and launches either the CPU or GPU thin client automatically, with `--standalone` to force the legacy local-PTY mode. The remaining work here is shrinking the thin clients further, continuing GPU/live parity validation, and pushing total memory toward the long-term target.

This means the first `handterm` invocation starts the server and opens a window. Every subsequent `handterm` just connects to the existing server. The user never thinks about server vs client - it just works, and second+ windows are near-instant and ultra-lightweight.

### Expected result

- Server: ~4.4MB RSS measured in the current live-profiled build for `server-only`; earlier headless measurement was ~3.7MB on this machine
- Client: ~2-3MB each (GPU surface + cell grid copy)
- 10 windows: ~28MB total

---

## Phase 3: Optimize the client binary

### 3a: Workspace split

Split into a workspace so the client doesn't link font libraries:

```
handterm/
  Cargo.toml              (workspace root)
  handterm-server/
    Cargo.toml             (freetype, fontconfig, rustybuzz, nix)
    src/main.rs
  handterm-client/
    Cargo.toml             (wgpu, winit, bytemuck)
    src/main.rs
  handterm-common/
    Cargo.toml             (protocol types, cell types, grid types)
    src/lib.rs
```

Status: materially implemented. The repository is now a real Cargo workspace with `handterm-common`, `handterm-server`, `handterm-client`, plus backend-specific `handterm-client-cpu` and `handterm-client-gpu` packages. The terminal core (`grid`, `parser`, `protocol`, `terminal`) lives in `handterm-common`, remote clients bootstrap from server-provided cell metrics, and local font loading is optional for split client builds. Remaining work is to continue moving more client/server-specific code out of the shared root crate and reduce runtime client memory further.

The intended end state is still that the client binary drops: freetype (~600KB RSS), fontconfig, rustybuzz, and all their transitive deps.

### 3b: Pre-compiled shader

The WGSL shader is currently an inline string compiled at runtime. Cache the compiled pipeline or use `naga` for ahead-of-time compilation so subsequent launches skip shader compilation.

### 3c: Lazy GPU init

Don't create the wgpu instance until the Wayland surface is ready. Use async adapter request to overlap GPU init with other startup work.

### Expected result

- Client: ~1-2MB each
- 10 windows: ~15MB total

### Test gates

- crate boundary tests for shared/common types
- client/server startup smoke tests
- shader asset loading/compilation tests

### Benchmark gates

- client binary size
- client startup latency
- per-window RSS after workspace split

---

## Phase 4: Final micro-optimizations

### 4a: Compact render cell

The full `Cell` is 20 bytes. For the client-side copy (render only), use a compact 8-byte cell:

```rust
struct RenderCell {
    ch: u32,        // codepoint
    fg_bg: u16,     // palette index (not full RGB)
    attrs: u8,
    flags: u8,
}
```

For 200x80 = 16,000 cells: 128KB vs 320KB. Matters with scrollback.

### 4b: Incremental updates over the wire

Only send dirty cells, not the full grid:

```rust
CellUpdate {
    window_id: u32,
    cells: Vec<(u16, u16, RenderCell)>,  // (row, col, cell)
}
```

The existing dirty-tracking bitmap in `Grid` already identifies changed cells. Use it server-side to minimize socket traffic.

### 4c: Zero-copy buffer sharing (advanced)

Use `memfd` to share the cell grid between server and client without copying. Server writes directly to shared memory, client reads from it. Synchronize with eventfd.

### 4d: Connection pooling

Keep a single persistent connection per client. Multiplex window updates over it. Avoid reconnection overhead.

### Expected result

- Client: <1MB each
- Server: ~4MB
- 10 windows: ~13MB total

### Test gates

- incremental update correctness tests
- render-cell packing/unpacking tests
- shared-memory synchronization tests if zero-copy is added

### Benchmark gates

- wire/update size before vs after compact cells
- update latency before vs after zero-copy/shared memory
- total RSS before vs after Phase 4 changes

---

## Projected results summary

| Config | Per-window RSS | Server RSS | 10 windows total |
|--------|---------------|-----------|-----------------|
| Current (CPU standalone) | 21MB | N/A | 210MB |
| Phase 1 (GPU standalone) | ~8-10MB | N/A | ~90MB |
| Phase 2 (GPU + daemon) | ~2-3MB | ~5-6MB | ~28MB |
| Phase 3 (optimized client) | ~1-2MB | ~4-5MB | ~15MB |
| Phase 4 (zero-copy) | <1MB | ~4MB | ~13MB |

For comparison, foot daemon with 10 windows: ~1.6MB x 10 + ~25MB server = **~41MB**.

Fully optimized handterm would use **~13MB** for the same setup - roughly 3x less than foot.

Foot can never close this gap because it is architecturally committed to CPU rendering (SHM buffers always count against RSS). GPU rendering is the fundamental advantage.

---

## Recommended order of work

1. Stabilize rendering correctness and expand automated coverage first.
2. **Phase 1** - GPU parity plus measurement, then decide default backend.
3. Kitty graphics protocol end-to-end before calling terminal functionality complete.
4. **Phase 2** - daemon split. Start with protocol, then server, then client.
5. **Phase 3** - workspace split. After daemon is working.
6. **Phase 4** - only keep micro-optimizations that win in benchmarks.
