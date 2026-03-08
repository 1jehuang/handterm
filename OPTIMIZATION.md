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
- Glyph cache misses no longer reopen the configured FreeType face.
- A narrow emoji fallback path exists in the font layer, but full emoji correctness and renderer parity are not finished.

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

**Status: in progress** - `gpu_app.rs` exists, but GPU rendering is not yet at parity with the stable CPU renderer, so CPU remains the release default.

### Tasks

1. Reach rendering parity with CPU for shell prompts, typing, resize, selection, and TUI repaint behavior.
2. Add automated GPU parity tests against CPU/offscreen reference output.
3. Verify glyph atlas upload and rendering across normal text, wide glyphs, and fallback/emoji paths.
4. Benchmark RSS, startup time, redraw throughput, and frame pacing on CPU vs GPU.
5. Only after those gates pass, reconsider making GPU the default release backend.

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
- `NewWindow { cols, rows }` - request a new terminal window
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
- `AtlasUpdate { glyph_id, bitmap, metrics }` - new glyph rasterized

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

Kitty graphics protocol support is part of project completion. The terminal already parses/stores kitty image payloads and placements, but the renderers do not yet draw them. Before Phase 2 is considered complete:

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

This means the first `handterm` invocation starts the server and opens a window. Every subsequent `handterm` just connects to the existing server. The user never thinks about server vs client - it just works, and second+ windows are near-instant and ultra-lightweight.

### Expected result

- Server: ~5-6MB (font cache + terminal state, no framebuffers)
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

The client binary drops: freetype (~600KB RSS), fontconfig, rustybuzz, and all their transitive deps.

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
