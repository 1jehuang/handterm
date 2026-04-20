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
  - when `HANDTERM_PROFILE_JSON=1` is set, the GPU host also emits machine-parsable JSON profile events for `gpu_host_open_window` and `gpu_host_first_frame`
- a new safe live add-window sample on this session measured:
  - `total=30.68ms`
  - `host_setup_before_surface=13.93ms`
  - `compositor_facing=25.93ms`
  - `handterm_surface_setup=2.62ms`
  - `open_to_first_present=32.54ms`
- a later safe 3-run sample series with the same refined profiling path showed:
  - first-window `total`: `107.85-317.82ms`, median `165.80ms`
  - first-window `open_to_first_present`: `110.21-322.54ms`, median `169.35ms`
  - add-window `total`: `16.30-28.00ms`, median `20.12ms`
  - add-window `open_to_first_present`: `17.98-31.22ms`, median `21.90ms`
  - add-window `compositor_facing`: `14.28-25.22ms`, median `16.80ms`
  - add-window `handterm_surface_setup`: `1.06-1.68ms`, median `1.31ms`
- a follow-up safe 3-run machine-parsed verification with `HANDTERM_PROFILE_JSON=1` reported the same overall result:
  - add-window `total`: `14.58-27.85ms`, median `18.91ms`
  - add-window `open_to_first_present`: `16.72-30.24ms`, median `20.46ms`
  - add-window `compositor_facing`: `12.21-25.78ms`, median `16.31ms`
  - add-window `handterm_surface_setup`: `0.96-1.43ms`, median `1.00ms`
- interpretation: the add-window path is still primarily blocked by compositor-facing window/surface configure work, while handterm-side GPU surface setup is now a much smaller portion of the total
- more specifically, the validated add-window JSON samples show `configure` dominating the compositor-facing bucket, while `window_create` and `surface_create` are comparatively small contributors
- because the current JSON mode already exposes `window_create`, `surface_create`, `configure`, and first-present timing cleanly, more profiling tooling is not the immediate bottleneck; the main remaining question is what can realistically change around compositor-facing behavior
- relevant protocol/runtime notes line up with that conclusion:
  - Winit’s Wayland docs note that windows do not appear until you draw/present to them
  - XDG surface lifecycle requires the compositor `configure` / client `ack_configure` / render-and-commit handshake
- that means handterm can measure this path well and avoid adding extra work to it, but it may not be able to meaningfully eliminate the dominant configure wait without changing the broader window-lifecycle strategy or accepting different semantics

### CPU host limitation

The CPU host path is now structurally correct, but it is not the likely theoretical winner because the remaining cost is dominated by presentation buffers.

## Remaining Work

### 1. Finish spawn profiling safely

A previous profiling attempt opened too many real windows in a live compositor session.
That should **not** be repeated in the same way.

#### Remaining profiling work

- replace compositor-heavy benchmarking with safer host-internal instrumentation
- measure repeated `open-window` calls without opening excessive live windows unnecessarily
- prefer the new `scripts/profile_host_json_series.sh` workflow for small machine-parsed host sample series, because it uses `HANDTERM_PROFILE_JSON=1` plus an isolated host socket instead of compositor-heavy scraping
- when medians across separate host launches matter, prefer the same script's repeat support instead of stuffing more live windows into one compositor session
- the same script now emits raw JSONL, a human text summary, and a machine-parsable aggregate summary JSON file for downstream analysis, including grouped rollups plus per-session and per-window machine-readable detail, with a stable schema/version marker for downstream tooling
- the raw structured profiling event lines now also carry their own stable schema/version marker, so JSONL consumers do not have to rely on the aggregate summary file alone for compatibility signaling
- the aggregation script now also validates the expected raw-event schema/version while harvesting JSONL, so schema drift is surfaced immediately instead of silently mixing incompatible samples
- a later broader safe JSON-series run with 3 added windows still showed the same answer: add-window time remained dominated by the compositor-facing bucket, especially `configure`, so the overall bottleneck interpretation did not materially change
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
Keep profiling runs to small window counts by default. The new JSON-series script refuses larger runs unless explicitly overridden.

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
- first-window shared GPU bring-up can now overlap with other window-prep work, and the profiling logs now distinguish `shared_ms` (total init work) from `shared_wait_ms` (blocking wait left on the critical path)
- one safe post-overlap first-window sample still showed shared bring-up dominating the critical path (`shared_ms≈357ms`, `shared_wait_ms≈331ms`), although overlap saved about `26ms` of wait time in that run
- one safe shared-init subphase sample showed `atlas_texture≈55ms` and `device_request≈23ms` as the largest measured explicit subphases, while shader/layout setup stayed relatively small; if more first-window reduction is still worth pursuing, the next concrete candidates are atlas texture setup and adapter/device work rather than the shader/layout pieces
- simple atlas-texture deferral does not currently look like the best next slice, because the first real text render path still needs a valid atlas view/bind group; a meaningful reduction there likely needs a more invasive placeholder-or-growable atlas design
- a smaller fixed initial atlas was also tried against the existing test matrix, but one safe rebuilt-binary sample did not show a clear startup win, so that does not currently look like the best next atlas-texture direction either
- shared GPU init now starts prewarming from app startup instead of only after the first real window exists, but a later safe sample still showed first-window startup dominated by shared bring-up, so the remaining meaningful wins are still likely inside shared init itself rather than elsewhere in `open_window`
- a later safe GPU live-validation sample with the earlier app-start prewarm showed a much smaller `shared_wait_ms` (about `74ms`) while `shared_total_ms` still stayed around `193ms`, which supports the idea that more overlap is helping even though total shared-init work remains significant
- after moving prewarm up to `run()`, there is no obviously simple earlier overlap point left without pushing GPU work further up into CLI/config/process startup, so additional adapter/device overlap now looks like a higher-complexity tradeoff rather than the next easy win
- latest safe add-window sample suggests handterm-side GPU setup is already relatively small, so any further local wins are likely lower leverage items such as buffer reuse/pooling or minor setup deferral rather than the main bottleneck
- spare/precreated-window strategies are not the preferred next move right now, because they would mostly shift configure cost earlier while adding memory, lifecycle, focus, and semantics tradeoffs

### 3. Decide future of daemon mode

Daemon mode still works, but it is no longer the most promising local low-RAM path.

#### Remaining decision work

- decide whether daemon mode should remain:
  - secondary/reference architecture
  - experimental mode
  - or long-term maintained alongside the host path

Current recommendation: treat daemon mode as a **secondary/reference architecture** rather than the primary local path. It remains useful for comparison, compatibility, and thinner client/server experiments, but the shared-GPU host is the stronger default local architecture.

### 4. Continue codebase structure cleanup

The current split is good, but not final.

#### Remaining structure work

- continue moving package-specific logic out of the root crate
- further reduce cross-linking between client/server/standalone code
- keep the shared core clean and minimal

Highest-value next cleanup slice: move daemon/client/server-specific runtime and entrypoint logic out of the root crate and into the already-created workspace packages, because those split packages currently remain thin wrappers while the root crate still owns most of the real implementation.

Current map of root-owned daemon/client/server-specific surface area:
- `src/daemon.rs`
- `src/server.rs`
- `src/client.rs`
- `src/remote_app.rs`
- `src/remote_gpu_app.rs`
- daemon-specific branches in `src/runtime.rs` (`server-only` / `client-only`)

Smallest safe refactor before a true package move: isolate the daemon/client/server launch and runtime glue behind clearer module boundaries first. A direct move into the existing split binary packages would otherwise create awkward dependency cycles, because those packages currently depend on the root crate.

That launch-glue isolation is now done, and it is a reasonable stopping point unless there is a strong reason to keep pushing the package split immediately.

If the split continues later, the next smallest safer slice is likely `src/daemon.rs` + `src/server.rs` before tackling `src/client.rs` and the remote frontends, because the remote CPU/GPU frontends still pull in much more windowing/rendering/UI surface area.

For now, it is reasonable to pause the deeper daemon/server extraction here and avoid forcing another architecture pass immediately.

Concrete dependencies to plan for in that deeper `src/daemon.rs` + `src/server.rs` move:
- PTY/process management (`crate::pty::PtyChild`)
- config inputs (`crate::config::AppConfig`)
- font/protocol rendering support still owned by the root crate (`crate::font::{GlyphAtlas, GlyphFormat}`)
- protocol/common terminal types, which are already much closer to the shared-core boundary

After the new `src/daemon_stack/` boundary, the next smallest real coupling reduction is probably inside `daemon_stack::core`: stop depending on root-config defaults and root grid constants directly, and push those defaults in from a narrower boundary. `font` and `pty` remain larger follow-up moves.

That defaults/constants cut is now done. The small `build_info` / protocol-build-id plumbing cut is also done: the `daemon_stack` boundary now receives protocol build IDs from the outer `crate::daemon` wrapper instead of reaching back into root build metadata directly. At this point, the remaining higher-friction couplings are mostly `font` and `pty`, so the current `daemon_stack` boundary is probably a sufficient maintainability win unless there is a strong reason to keep prioritizing the package split.

### 5. Keep docs aligned with measured reality

README and OPTIMIZATION have been updated substantially, but any future changes should continue to reflect measured results, not intended architecture.

For now, `HANDTERM_PROFILE_JSON=1` should stay as an **internal opt-in diagnostics aid** rather than a prominently documented README feature. It is useful for safe machine-parsed profiling, but it is not a primary product surface.

Likewise, `HANDTERM_HOST_SOCKET` should stay as an **internal diagnostics/testing hook** for isolated host-control workflows such as profiling scripts. It is useful for tests and local instrumentation, but it should not be treated as a primary user-facing product feature unless there is a stronger advanced-workflow use case later.

The new CPU-host structured profiling events are useful for the same internal tooling path, but they do not need a separate CPU-specific public docs note right now. The current internal profiling notes are enough unless CPU-host startup becomes a much higher-priority investigation track later.

The remaining Kitty image `partial*` gaps are no longer the preferred next protocol slice. Handterm now covers the core inline raw/compressed RGB/RGBA/PNG upload-place-delete path, and the remaining gaps are lower-leverage features such as non-inline transports and richer placement/operation parameters.

If work shifts away from daemon/package cleanup and away from Kitty protocol follow-through, the next notable non-Kitty feature gap to evaluate is Sixel support, since it remains one of the clearest missing graphics/protocol capabilities in the feature table.

A first enabling slice is now in place: the parser/terminal path can surface DCS payloads instead of only silently consuming them. Even with that in place, Sixel still does not currently look like the right immediate next slice, because it would need a broader terminal/image-model path beyond this parser plumbing.

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

## **deliberately choose one major roadmap track**

Do not reopen another small protocol or architecture side quest by default.

If architecture cleanup resumes, only keep pushing the daemon/package split if there is a strong reason to tackle the larger remaining `font` and `pty` couplings.

Otherwise, shift to a different clearly scoped roadmap item. The GPU host startup investigation has already reached the point where the dominant remaining wait looks mostly compositor/protocol-bound unless handterm adopts more invasive lifecycle tradeoffs, and the latest validated samples suggest handterm-side GPU setup is already relatively small.
