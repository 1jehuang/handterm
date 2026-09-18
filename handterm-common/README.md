# Embedding handterm-common

This crate contains the terminal grid/parser, protocol types, Kitty graphics
state, and window-system-independent `color`, `visual`, and `input` helpers.
The standalone crate re-exports color/visual helpers and adapts winit keyboard
events to the common encoder. Embedders do not need winit to use these APIs.

## Graphics resource limits

Untrusted bytes passed to `Terminal::process` use fixed per-terminal limits:

| Resource | Limit |
| --- | --- |
| Individual APC/DCS parser buffer | 1 MiB (existing parser cap) |
| Complete base64 upload, across continuation chunks | 32 MiB |
| Zlib output, including compressed PNG containers | 32 MiB |
| PNG decoder internal allocation budget | 32 MiB |
| Normalized RGBA bytes per image | 16 MiB |
| Stored RGBA image data | 64 MiB and 1,024 images |
| Placements, active plus saved main screen | 4,096 |
| Each retained control-string event queue | 4 MiB and 256 events |
| Unread Kitty graphics replies | 64 KiB |

Image dimensions are checked before pixel allocation. PNG text/ICC metadata is
ignored. Spare base64/inflate capacity is not retained by small RGBA images.
An oversized chunked upload releases its buffer and discards continuations
through `m=0`. A delete command or screen switch also aborts a partial upload.
The initial chunk owns the format, action, dimensions, placement size, and quiet
flag. Storage exhaustion rejects the new image/placement rather than evicting
unrelated images. Replacements account for the old image's released quota.

These are component limits, not a total RSS ceiling: decoding has bounded
transient buffers, queues mirror some events, and renderers may copy images to
CPU/GPU caches. Drain replies after each processing batch and consume event
queues when needed. Queues evict their oldest events at either limit. The parser
still truncates oversized individual control strings at its existing cap, so
send large graphics in protocol-sized chunks (normally 4 KiB payloads).

The limits protect the shell-output graphics path, not arbitrary caller mutation
of public fields, pre-deserialized `ServerMessage` payloads, or all other terminal
features. Treat remote protocol input as a separate trust boundary. Synchronous
PNG/zlib decoding is bounded in space, not a promise of bounded UI latency.

## Placement behavior and compatibility

Use `Terminal::kitty_placements()` or `TerminalView::kitty_placements()`, not the
raw public vector, to paint the current viewport. Live placements move with
linefeeds, automatic wrap, and scrolling regions, including reverse scrolling.
A placement is removed when its anchor leaves the scrolling region. Image data
remains available for another placement. Main-screen placements are hidden and
saved on alternate-screen entry and restored on exit. Image IDs/data are shared
between screens, so deletion or replacement also removes stale saved placements.
Screen switches and placement movement invalidate `kitty_generation()`.

Full Kitty scrollback anchoring and partial-image clipping are not implemented.
Placements are entirely suppressed while `grid.scroll_offset != 0`, so live
images do not float over historical text. Off-screen images are not restored by
scrolling back. Viewport changes must trigger rendering even if the graphics
generation did not change. For predictable size, supply explicit `c` and `r`:
implicit pixel-to-cell sizing remains approximate because core has no font cell
metrics. This remains a subset of Kitty graphics (direct RGB/RGBA/PNG uploads,
zlib, basic placement/deletion), not complete protocol conformance.

`COLOR_DEFAULT` is now `0x4000_0000`, distinct from indexed black (`0`). RGB colors
continue to use `COLOR_FLAG_RGB`. SGR 39/49/59 restore defaults. The xterm 256-color
cube uses levels 0, 95, 135, 175, 215, 255. Serialized cells and server/client peers
must use matching revisions: old cells encoded both default and black as zero,
so that ambiguity cannot be losslessly migrated. Always use the exported constant
and shared `visual::resolve_cell_colors` instead of testing for numeric zero.

## Keyboard integration

Map host events to `input::{Key, NamedKey, PhysicalKey, ModifiersState,
KeyEventKind}` and call `input::key_to_bytes_into` with the terminal's current
application-cursor mode and Kitty keyboard flags. This shares legacy and Kitty
encoding while reusing an output buffer. Supply produced text separately from
logical keys, and physical positions when available for keypad/alternate-key
reporting. Preserve press/repeat/release, lock states, and left/right modifier
information. IME composition, shortcut policy, and host event deduplication stay
in the frontend. Existing word-delete shortcut behavior is preserved by the
shared encoder.

Validate the headless core with `cargo test -p handterm-common`.
