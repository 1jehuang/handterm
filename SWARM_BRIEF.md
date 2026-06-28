# Handterm macOS Performance + Resource Optimization Swarm Brief

You are one worker in a parallel swarm optimizing **handterm** (a Wayland/macOS-native
terminal in Rust) for **performance and resource efficiency on macOS (Apple Silicon, Metal)**.

## Hard rules (read first)

1. **Stay in your lane.** You own ONLY the files listed in your task prompt. Do NOT
   edit any other source file. If you believe a shared file must change, STOP and
   report it as a blocker instead of editing it. This keeps parallel merges clean.
2. **Never break the build or tests.** After every change run, in your worktree:
   - `cargo test --workspace` must stay green (currently 118 passing).
   - `cargo build --release` must succeed.
3. **Prove every win with numbers.** Use the deterministic gate:
   - `cargo build --release` then `./scripts/bench_capture.sh before 5`
   - make changes
   - `cargo build --release` then `./scripts/bench_capture.sh after 5`
   - compare `bench_out/before.json` vs `bench_out/after.json`.
   Only keep changes that show a real, repeatable improvement (or pure memory/no-regression).
4. **Add automated coverage** for any new logic (unit tests in the same crate).
5. **Commit to your own branch** with clear messages. Do NOT push. Do NOT merge.
   Do NOT touch git on other branches. The coordinator merges.
6. **Idiomatic, maintainable Rust.** No unsafe unless clearly justified and tested.
   No dependency additions without strong justification (report as blocker first).

## Build environment (already set in your worktree shell)

```
export PATH="$HOME/.cargo/bin:$PATH"
export PKG_CONFIG_PATH="/opt/homebrew/opt/freetype/lib/pkgconfig:/opt/homebrew/opt/fontconfig/lib/pkgconfig"
```
Your worktree has its own `target/` dir (do not share with siblings).

## Baseline (release, Apple Silicon M-series, 2026-06-28)

Single idle window phys_footprint: **handterm 66MB**, Ghostty 130MB, kitty 182MB.
handterm already wins single-window. The weak spots:

- **macOS per-window scaling: +30MB/window** (Linux target is ~1-2MB). Metal drawables.
- Parser ASCII: ~922 MB/s (only **8% of memcpy** ~18.6 GB/s).
- Grid write ASCII: ~326 MB/s (**3% of memcpy**).
- Terminal pipeline ASCII: ~292 MB/s.
- Cell size: 24 bytes. 10k scrollback (80col): 18.75 MB.
- Startup: dpi/font bootstrap ~24ms, atlas ~11ms, window ~14ms.
- GPU frame prep text batching: ~82 Mcells/s.
- CPU renderer full redraw: ~1412 fps.

## Verification standard (from OPTIMIZATION.md)

A change is complete only when all three hold:
1. Automated correctness coverage exists for the path.
2. Benchmark checkpoint shows the expected improvement (or no regression for pure
   memory/cleanup work).
3. Any doc you touch reflects measured reality.

## Reporting

When done (or blocked), report via swarm `report`:
- what you changed, files touched
- before/after numbers from bench_capture (paste the relevant metric lines)
- test status (`cargo test --workspace` result line)
- any cross-file blockers you hit

Be autonomous and thorough. Iterate until your metric is meaningfully improved or
you have clear evidence it is already near its floor. Do not stop after one pass.
