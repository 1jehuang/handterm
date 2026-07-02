# Handterm Performance Optimization Roadmap

Goal: reach the theoretical minimum resource usage for a terminal emulator (Wayland and macOS/Metal).

> **Architecture note (2026-07):** the daemon/thin-client mode described in some
> sections below was fully removed in commit `9eb2f6b` after profiling showed the
> single-process host was the better local low-RAM path. The workspace now has 2
> crates (`handterm` + `handterm-common`). Daemon-related sections are kept as
> historical design notes and are marked as such.

## Current state

| Metric | handterm CPU host | handterm GPU host | footclient |
|--------|------------------|-------------------|------------|
| First window RSS | ~27-37MB | ~60-61MB | 1.6MB |
| Additional window RSS | ~20MB | ~1-2MB | 1.6MB |
| New-window startup | ~16-43ms | first window ~255ms, added window ~57ms | ~25-30ms |
| Architecture | single-process host | single-process host | daemon thin client |

### What is currently limiting memory

| Path | Limiting factor | Notes |
|------|-----------------|-------|
| CPU host | Wayland/softbuffer SHM backbuffers | dominant per-window cost; terminal state is no longer the main issue |
| GPU host | fixed GPU/runtime cost on first window plus compositor surface configure | shared well across additional windows; current added-window hotspot is configure |
| Daemon thin clients (removed) | client process/runtime duplication | heavier than the host-based design; removed in `9eb2f6b` |

The biggest architectural lesson from the current implementation is that **shared-GPU single-process hosting** is the strongest path toward the theoretical per-window floor. CPU host mode improved a lot, but it runs into a hard Wayland/softbuffer SHM ceiling. GPU host mode pays a large fixed first-window cost, then scales at roughly **1-2 MB per extra window**.

## macOS (Apple Silicon, Metal) measured state

On macOS the shared-GPU host is the default/only GUI path (Metal via wgpu).

### Three-way comparison vs Ghostty 1.3.1 and kitty 0.47

Measured on an Apple Silicon laptop with `scripts/cross_terminal_bench.sh` (memory),
self-timing PTY payloads (throughput), and launch-to-shell timing (startup):

| Metric | handterm | Ghostty | kitty | winner |
|--------|---------:|--------:|------:|--------|
| Memory, single window (phys_footprint) | **~39 MB** | ~130 MB | ~161 MB | handterm 3-4x |
| Memory, per extra window | **~+11 MB** | shared | ~+63 MB | handterm |
| Startup, cold launch -> shell | **~83 ms** | ~201 ms | ~208 ms | handterm 2.4x |
| Throughput, realistic 80-col lines | **~242 MB/s** | ~91 MB/s | ~120 MB/s | handterm 2-2.6x |
| Throughput, pathological 1-char lines (`yes`) | ~28 MB/s | **~51 MB/s** | ~15 MB/s | Ghostty (handterm beats kitty) |

handterm wins every realistic metric. The single case it loses is a pathological
scroll storm (`yes`-style output: one character then a newline, forcing a full
scroll on every other byte). Profiling shows `Grid::scroll_up` is ~73% of that
workload, dominated by the per-line scrollback copy (~1.9 KB) plus blanking the
recycled row (~1.9 KB). Throughput climbs steeply with line length and is already
far ahead of both competitors by ~10-char lines:

| line length | handterm scroll throughput |
|------------:|---------------------------:|
| 1 char | ~34 MB/s |
| 10 chars | ~133 MB/s |
| 40 chars | ~246 MB/s |
| 79 chars | ~298 MB/s |

Closing the 1-char worst case to also beat Ghostty there requires a unified
ring-buffer grid (active screen + scrollback sharing one ring so a scroll is a
pointer advance with no per-line copy). That is a large, higher-risk refactor of
the terminal core (101 covering tests, plus selection/resize/alt-screen
interactions) and is deferred rather than risk silent scrollback corruption for a
pathological-only case.

### Remaining macOS-specific gaps

The two open macOS-specific gaps are:

- **Per-additional-window scaling is ~+10 MB/window on macOS** after fixing a
  window auto-grow bug (was ~+30 MB/window). Root cause: on a multi-display setup
  AppKit grows a freshly created window to fill the display it lands on (an 80x24
  request was observed settling at the full external-monitor height, e.g. a
  1608x2082 drawable instead of 1440x792). The Metal swapchain drawables are sized
  to the *actual* window, so the auto-grow tripled the dominant per-window cost
  (two ~13 MB IOSurfaces vs two ~4.5 MB ones). Fixed by clamping the initial
  `max_inner_size` to the requested grid size and lifting the clamp after the
  first frame, so the window opens grid-sized but stays freely resizable. The
  remaining ~+10 MB/window is the legitimate cost of two grid-sized Retina
  drawables plus per-window terminal/render state. Measured first-window
  footprint dropped ~65 MB -> ~39 MB and 3-window ~125 MB -> ~59 MB in the
  auto-grow case.
- First-window startup (~40 ms `open_to_first_present`) and added-window startup
  (~14 ms) are dominated by shared GPU adapter/atlas bring-up, not font discovery
  or DPI probing, so those are the next startup levers rather than per-window glue.

The deterministic logic pipeline (`scripts/bench_capture.sh`) is far from its
memcpy ceiling and is the most productive optimization surface: a parallel
optimization pass landed measurable, regression-free wins across the parser, grid,
terminal, GPU frame-prep, CPU renderer, input encoding, and protocol codec layers
while growing automated coverage from 118 to 240+ tests.


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

Daemon-era work (the daemon/thin-client runtime was removed in `9eb2f6b`; the protocol types themselves survive in `handterm-common/src/protocol.rs`; listed for history):

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

Back to current, shipped work:

- The grid/protocol/font stack is now grapheme-aware for UTF-8 printable clusters, and the font layer caches rasterized grapheme bitmaps so emoji sequences like variation-selector and ZWJ clusters can render as one cell span instead of being split into separate placeholder glyphs.
- The GPU frontend now prefers non-sRGB surface formats when available for better color parity, and GPU glyph quads expand to the uploaded glyph width so prompt icons with right-side overhang are not clipped.
- The CPU frontends now keep their own persistent software framebuffer and copy that into softbuffer at present time, instead of relying on softbuffer backbuffer persistence across frames. This has automated coverage because the CPU renderer is now tested for the “persistent software framebuffer -> fresh presented front buffer” path that matches live CPU presentation more closely.
- The font path now uses explicit FreeType light hinting for normal text, derives cell width from a representative monospace sample set instead of a single rendered `M`, and procedurally rasterizes shaded/block glyphs like `░▒▓` so they stay cell-aligned and crisp.
- True configured background opacity is now wired through the GPU backend using transparent windows plus a non-opaque surface alpha mode when available. The current CPU backend cannot support real opacity on Wayland because `softbuffer` presents `Xrgb8888` there, so CPU remains intentionally opaque.
- Standalone CPU mode is now a real **single-process multi-window host** with a stable control socket, shared event loop, and host-side `open-window` support.
- Standalone GPU mode is now also a **single-process multi-window host**, with backend-specific host sockets and a shared `wgpu` instance/adapter/device/queue foundation reused across windows.
- The GPU runtime now splits shared context from per-window surface state, and repeated windows reuse the same device/queue plus a shared render-pipeline cache keyed by surface format.
- The CPU host path no longer keeps an extra full-window offscreen framebuffer per window; it renders directly into the presentation buffer, which materially reduced per-window memory.
- Font startup bootstrap now caches resolved font paths and measured cell metrics under the system cache dir, avoiding redundant font discovery work during startup sizing.

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

With a shared-GPU host model, each additional window can plausibly approach **~1 MB** in practice. The current implementation is already in the **~1-2 MB** range for added windows on the profiled machine.

---

## Phase 1: Ship the GPU backend as default

**Status: in progress** - GPU frame planning/batching is significantly more structured and benchmarked than before, and GPU is now the preferred default backend when it is compiled in. Full framebuffer parity and broader live validation still remain.

### Tasks

1. Reach rendering parity with CPU for shell prompts, typing, resize, selection, and TUI repaint behavior.
2. Add stronger automated GPU parity tests against CPU/offscreen reference output and shared visual expectations.
   Status: partially done, but now broad enough that the next confidence step should be a small targeted live-validation pass instead of endlessly adding more unit-style parity cases. Transcript-driven shared-visual tests now exist for the GPU batch builder, and end-to-end GPU framebuffer parity covers several transcript/probe cases plus visible interaction cases like selection highlight, incremental typing, line repaint, resize-driven layout changes, full-screen repaint, scrollback-plus-selection behavior, and cursor styles.
3. Verify glyph atlas upload and rendering across normal text, wide glyphs, and fallback/emoji paths.
4. Benchmark RSS, startup time, redraw throughput, and frame pacing on CPU vs GPU.
5. Keep validating the GPU default path against live shell/TUI workloads and continue tightening parity until CPU is no longer the fallback/debug backend.

Current safe live-validation path: `scripts/live_host_validation.sh` launches one isolated CPU or GPU host, verifies deterministic terminal text plus basic host control operations, opens one additional window, and cleans it up again without doing compositor-heavy benchmarking.

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

## Phase 2: Multi-window architecture

This phase originally had two parallel architectural tracks:

1. **single-process host** (the shipped local low-RAM path)
2. daemon/server mode (implemented, then removed in `9eb2f6b`; kept below as historical design notes)

### 2a: Single-process host mode

Status: materially implemented for both CPU and GPU.

Current measured results:

| Setup | First window | Each additional |
|-------|-------------:|----------------:|
| CPU host | ~37 MB | ~20 MB |
| GPU host | ~61 MB | ~1-2 MB |

Current conclusion:

- **CPU host** improved substantially, but is limited by Wayland/softbuffer SHM backbuffers.
- **GPU host** is now the best low-RAM path because the heavy GPU runtime cost is paid once and shared across all windows.

Remaining work in this track:

- reduce added-window startup time on the GPU host
- continue shaving the GPU-host incremental slope below the current ~1-2 MB/window range where possible
- keep the CPU host as a simpler/reference path, but treat GPU host as the primary route toward the theoretical limit

### 2b: Daemon/server mode (removed, historical)

Status: implemented, then **removed in `9eb2f6b`** after the host path won.
Everything from here through section 2e describes the removed architecture and
is retained only as a design reference.

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

Status (historical): this default-connect flow existed before the daemon removal. Plain local `handterm` now follows the host path instead: it reuses a compatible running host process when one exists and opens another window there, with `--standalone` to force a fresh process.

This means the first `handterm` invocation starts the server and opens a window. Every subsequent `handterm` just connects to the existing server. The user never thinks about server vs client - it just works, and second+ windows are near-instant and ultra-lightweight.

### Expected result (historical)

- Server: ~4.4MB RSS measured in the last live-profiled daemon build for `server-only`
- Client: heavier than the shared-host approach, which is why the track was removed
- The protocol/design notes above remain useful if a remote/isolation use case ever justifies a revival

---

## Phase 3: Optimize the winning path

The old assumption here was “optimize daemon thin clients until they win.”

The conclusion that ended that track:

- **optimize the shared-GPU host first**, because it is already closest to the theoretical per-window floor
- daemon mode never beat the host path locally and was removed in `9eb2f6b`

### 3a: Workspace split (historical)

The original plan split the workspace so a thin client would not link font libraries:

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

Status: implemented during the daemon era, then **collapsed back to 2 crates in `9eb2f6b`** when the daemon/thin-client track was removed. Today the workspace is `handterm` (frontends, host runtime, fonts, PTY) plus `handterm-common` (grid, parser, protocol, terminal core). The shared-core boundary from this split survives and is what keeps the terminal core independently testable and benchmarkable.

### 3b: Shared GPU startup optimization

The GPU host now shares device/queue state and caches pipelines across windows, but new-window startup is still higher than ideal. The latest profiling shows the main remaining cost is compositor-facing surface configure rather than font lookup or pipeline creation. Remaining work includes:

- reduce per-window setup in the GPU host further
- continue avoiding repeated shader/pipeline work
- measure where Wayland window creation vs GPU surface setup dominates the remaining latency

### 3c: Lazy GPU init

Don't create the wgpu instance until the Wayland surface is ready. Use async adapter request to overlap GPU init with other startup work.

### Expected result

- shared-GPU host additional window: push from ~1-2 MB toward ~1 MB
- shared-GPU host added-window startup: continue pushing downward from current tens-of-ms regime

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

### 4d: Host/daemon coexistence cleanup

Done by removal: daemon mode no longer exists (`9eb2f6b`), so the control plane and docs only need to describe the CPU and GPU host architectures.

### Expected result

- shared-GPU host: approach **~1 MB** per extra window

### Test gates

- incremental update correctness tests
- render-cell packing/unpacking tests
- shared-memory synchronization tests if zero-copy is added

### Benchmark gates

- wire/update size before vs after compact cells
- update latency before vs after zero-copy/shared memory
- total RSS before vs after Phase 4 changes

---

## Projected / measured results summary

| Config | First window RSS | Additional window RSS | Notes |
|--------|------------------:|----------------------:|-------|
| Old CPU standalone | ~21MB | +21MB | pre-host baseline |
| CPU host | ~37MB | +20MB | limited by SHM backbuffers |
| GPU host | ~61MB | ~1-2MB | current best path |
| Daemon server-only (removed) | ~4.4MB | N/A | last measured before removal |
| Old GPU daemon client (removed) | ~53MB | +53MB/process | heavier than host path |

For comparison, foot daemon with 10 windows is roughly **~41MB** total on the comparison system. The current shared-GPU host trajectory is much more promising for low incremental window overhead once the first window has paid the fixed GPU/runtime cost.

The current best measured handterm path is not “10 tiny daemon clients,” but rather “one shared GPU host plus cheap added windows.”

Foot can never close this gap because it is architecturally committed to CPU rendering (SHM buffers always count against RSS). GPU rendering is the fundamental advantage.

---

## Recommended order of work

1. Stabilize rendering correctness and expand automated coverage first.
2. Treat **shared-GPU host** as the primary low-RAM architecture for local multi-window use.
3. Keep profiling/optimizing added-window startup time on the GPU host.
4. Daemon mode is removed; do not reintroduce it unless a remote/isolation use case offers a tradeoff the shared host cannot match.
5. Only keep micro-optimizations that show real benchmark wins.
