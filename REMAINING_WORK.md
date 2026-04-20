# Remaining Work

This file summarizes the current state of the `handterm` repository and what remains to be done when work resumes.

## Current State

### Implemented architectural directions

- `handterm-common` exists and owns the extracted shared terminal core:
  - `grid`
  - `parser`
  - `protocol`
  - `terminal`
- The repository is a real Cargo workspace.
- Package split exists for:
  - `handterm`
  - `handterm-common`
  - `handterm-server`
  - `handterm-client`
  - `handterm-client-cpu`
  - `handterm-client-gpu`
- Local font loading is optional for split client builds.
- Root crate features are sliced for:
  - `standalone`
  - `daemon-client`
  - `daemon-server`
  - `cli`
  - `local-fonts`
  - backend features

### Multi-window host work completed

- CPU standalone was reworked into a **single-process multi-window host**.
- GPU standalone was reworked into a **shared-GPU single-process multi-window host**.
- Host-level IPC/control exists, including:
  - `open-window`
  - `focus-window`
  - `list-windows`
- Backend-specific host sockets exist:
  - CPU: `handterm-cpu.sock`
  - GPU: `handterm-gpu.sock`
- Plain local `handterm` now defaults to the **host path** rather than the old daemon-client path.

### Daemon/thin-client work still present

- Daemon/server mode still exists and is functional.
- Thin clients still exist for CPU and GPU.
- Remote clients bootstrap from server-provided cell metrics.
- Remote clients consume server-side glyph/image sync.

## Current Measured Results

### Shared GPU host (best current path)

Measured on the current machine/session:

- 1 window: ~60.5-61.3 MB RSS
- 2 windows: ~63.1 MB RSS
- 3 windows: ~64.0 MB RSS
- 4 windows: ~65.0 MB RSS
- 5 windows: ~67.2 MB RSS
- 6 windows: ~68.1 MB RSS

Interpretation:

- first window pays the fixed GPU/runtime cost
- additional windows are roughly **~1-2 MB each**

### CPU host

Measured on the current machine/session:

- first window: ~27.3-37.1 MB RSS
- additional windows: ~20 MB each
- open-window latency: ~16-43 ms depending on whether this is a warm host/additional-window path vs fresh startup path

Interpretation:

- CPU host improved significantly
- remaining per-window memory is largely due to **Wayland/softbuffer SHM backbuffers**

### Recent live comparison vs kitty

Recent live startup/RSS comparison on the current machine/session:

- handterm GPU host: startup ~255 ms, RSS ~60.5 MB
- handterm CPU host: startup ~43 ms, RSS ~27.3 MB
- kitty: startup ~353 ms, RSS ~100.1 MB

### Daemon server-only

- server-only: ~4.4 MB RSS in the current live-profiled build
- earlier headless measurement was ~3.7 MB RSS

## Important Conclusions

### Winning architecture for local low-RAM multi-window use

The best current architecture is:

## **shared-GPU single-process host**

not the old process-per-window local mode, and not the current daemon thin-client path.

### Current bottleneck

The main remaining bottleneck is no longer per-window RAM scaling on the shared-GPU host.

The main remaining bottleneck is:

## **new-window spawn/startup cost**

especially the cost of window/surface creation on some openings.

The recent internal GPU profiling now shows this more precisely:

- first GPU window is dominated by shared GPU bring-up plus compositor surface configure
- added GPU windows are currently dominated by **surface configure** on the profiled compositor session
- a recent measured added-window sample was ~56.9 ms internal, with ~43.6 ms in surface setup and ~42.4 ms specifically in configure
- the refined host profiling now logs:
  - `kind=first-window` vs `kind=add-window`
  - warm-cache flags such as `shared_warm`, `atlas_cached`, `defaults_reused`, and `pipeline_cache_hit`
  - aggregate buckets: `host_setup_before_surface`, `compositor_facing`, `handterm_surface_setup`, and `surface_unaccounted`
- a new safe live add-window sample on this session measured:
  - `total=30.68ms`
  - `host_setup_before_surface=13.93ms`
  - `compositor_facing=25.93ms`
  - `handterm_surface_setup=2.62ms`
  - `open_to_first_present=32.54ms`
- interpretation: the add-window path is still primarily blocked by compositor-facing window/surface configure work, while handterm-side GPU surface setup is now a much smaller portion of the total

### CPU host limitation

The CPU host path is now structurally correct, but it is not the likely theoretical winner because the remaining cost is dominated by presentation buffers.

## Remaining Work

### 1. Finish spawn profiling safely

A previous profiling attempt opened too many real windows in a live compositor session.
That should **not** be repeated in the same way.

#### Remaining profiling work

- replace compositor-heavy benchmarking with safer host-internal instrumentation
- measure repeated `open-window` calls without opening excessive live windows unnecessarily
- collect:
  - total open-window time
  - window/surface creation time
  - PTY spawn time
  - host CPU time deltas (**now emitted by the host open-window / startup profiling logs; keep using those instead of compositor-heavy external loops**)
  - the new aggregate buckets (`host_setup_before_surface`, `compositor_facing`, `handterm_surface_setup`) so future investigations can quickly tell whether a regression is inside handterm or mostly compositor-facing

#### Important safety note

Do **not** hammer `niri msg` in a tight loop.
Do **not** signal or restart the compositor.
Do **not** open huge numbers of real windows on the live desktop without explicit need.

### 2. Optimize GPU host add-window startup

This is the biggest remaining optimization target.

#### Areas to investigate

- Wayland window creation overhead
- surface creation overhead
- first-few-window GPU warmup effects
- any remaining per-window GPU resource creation that can be cached/shared

#### Already done here

- shared GPU context
- shared device/queue
- shared pipeline cache by surface format

#### Likely next work

- additional instrumentation in `src/gpu_app.rs`
- additional instrumentation in `src/gpu_runtime.rs`
- identify whether the remaining cost is mostly compositor-side or handterm-side

### 3. Decide future of daemon mode

Daemon mode still works, but it is no longer the most promising local low-RAM path.

#### Remaining decision work

- decide whether daemon mode should remain:
  - secondary/reference architecture
  - experimental mode
  - or long-term maintained alongside the host path

### 4. Continue codebase structure cleanup

The current split is good, but not final.

#### Remaining structure work

- continue moving package-specific logic out of the root crate
- further reduce cross-linking between client/server/standalone code
- keep the shared core clean and minimal

### 5. Keep docs aligned with measured reality

README and OPTIMIZATION have been updated substantially, but any future changes should continue to reflect measured results, not intended architecture.

## Current Known Risks / Notes

### Niri interaction warning

A compositor signal/reload attempt was previously unsafe and should not be repeated.

#### Rule

- do not send manual signals to `niri`
- do not kill/restart the compositor
- use only safe/read-only checks unless the user explicitly wants compositor changes

### User keybindings

The user config was edited so that:

- `Alt+[` points to the built handterm binary in the repo
- `Alt+,` is intended to open jcode in handterm

However, compositor reload behavior should be handled safely and carefully.

## Best Next Step When Resuming

If work resumes, the best next task is:

## **focus on safe profiling and optimization of shared-GPU host add-window startup**

That is the most meaningful remaining path toward the practical theoretical limit.
