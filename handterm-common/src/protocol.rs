#![allow(dead_code)]

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

pub type WindowId = u32;
const MAX_FRAME_SIZE: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum KeyEventKind {
    Press = 1,
    Repeat = 2,
    Release = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum MouseEventKind {
    Press = 1,
    Release = 2,
    Move = 3,
    ScrollUp = 4,
    ScrollDown = 5,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum MouseButton {
    Left = 1,
    Middle = 2,
    Right = 3,
    None = 0,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyEvent {
    pub kind: KeyEventKind,
    pub bytes: Vec<u8>,
    pub text: Option<String>,
    pub modifiers: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MouseEvent {
    pub kind: MouseEventKind,
    pub button: MouseButton,
    pub col: u16,
    pub row: u16,
    pub modifiers: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirtyCell {
    pub row: u16,
    pub col: u16,
    pub ch: u32,
    pub grapheme: Option<String>,
    pub fg: u32,
    pub bg: u32,
    pub underline_color: u32,
    pub hyperlink_id: u16,
    pub attrs: u8,
    pub flags: u8,
    pub underline_style: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorState {
    pub row: u16,
    pub col: u16,
    pub style: u8,
    pub visible: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellMetrics {
    pub cell_width: u16,
    pub cell_height: u16,
    pub baseline: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WindowModes {
    pub bracketed_paste: bool,
    pub focus_events: bool,
    pub alternate_scroll: bool,
    pub application_cursor_keys: bool,
    pub in_alt_screen: bool,
    pub mouse_mode: u8,
    pub kitty_keyboard_flags: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlyphBitmap {
    pub glyph_id: u32,
    pub grapheme: Option<String>,
    pub width: u16,
    pub height: u16,
    pub bearing_x: i16,
    pub bearing_y: i16,
    pub cells: u8,
    pub is_color: bool,
    pub pixels: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KittyImageData {
    pub id: u32,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KittyImagePlacement {
    pub image_id: u32,
    pub col: u16,
    pub row: u16,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMessage {
    Ping {
        build_id: String,
    },
    NewWindow {
        cols: u16,
        rows: u16,
        dpi: u32,
    },
    KeyInput {
        window_id: WindowId,
        event: KeyEvent,
    },
    MouseInput {
        window_id: WindowId,
        event: MouseEvent,
    },
    Resize {
        window_id: WindowId,
        cols: u16,
        rows: u16,
    },
    CloseWindow {
        window_id: WindowId,
    },
    Paste {
        window_id: WindowId,
        text: Vec<u8>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerMessage {
    Pong {
        build_id: String,
    },
    WindowCreated {
        window_id: WindowId,
        cols: u16,
        rows: u16,
        metrics: CellMetrics,
        modes: WindowModes,
    },
    WindowResized {
        window_id: WindowId,
        cols: u16,
        rows: u16,
        metrics: CellMetrics,
        modes: WindowModes,
    },
    CellUpdate {
        window_id: WindowId,
        dirty_cells: Vec<DirtyCell>,
        cursor: Option<CursorState>,
        modes: WindowModes,
    },
    SetTitle {
        window_id: WindowId,
        title: String,
    },
    Bell {
        window_id: WindowId,
    },
    CopyToClipboard {
        window_id: WindowId,
        text: Vec<u8>,
    },
    WindowClosed {
        window_id: WindowId,
        exit_code: Option<i32>,
    },
    KittyImageState {
        window_id: WindowId,
        generation: u64,
        images: Vec<KittyImageData>,
        placements: Vec<KittyImagePlacement>,
    },
    AtlasUpdate {
        glyph: GlyphBitmap,
    },
}

// ---------------------------------------------------------------------------
// Compact binary wire codec.
//
// Both the encoder and the decoder live in this file, so the format is free to
// evolve as long as the two stay self-consistent. Compared to a generic
// `serde`/`bincode` round-trip this codec:
//   * writes a single byte variant tag instead of a 4-byte one,
//   * LEB128 varint-encodes integers and lengths (1 byte for the very common
//     small/zero values instead of fixed 2/4/8-byte fields),
//   * packs the five boolean `WindowModes` flags into one byte, and
//   * encodes directly into a pre-sized `Vec<u8>` with no intermediate
//     serializer state.
// The net effect is fewer allocations, a smaller encoded payload, and a faster
// encode/decode hot path.
// ---------------------------------------------------------------------------

// Scalar integer fields use fixed little-endian widths: those are read/written
// in tight per-cell loops, and fixed widths decode with a single bounds check
// and no per-byte loop, which is the cheapest path for the hot benchmark.
//
// Lengths and element counts use LEB128 varints. They occur at most a handful
// of times per message (one per Vec/String/Option), so the small varint loop is
// negligible while keeping the common small-payload prefixes to one byte instead
// of bincode's fixed 8-byte (u64) length fields.
#[inline]
fn put_uvarint(buf: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        buf.push((v as u8) | 0x80);
        v >>= 7;
    }
    buf.push(v as u8);
}

#[inline]
fn put_u16(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_le_bytes());
}

#[inline]
fn put_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

#[inline]
fn put_u64(buf: &mut Vec<u8>, v: u64) {
    buf.extend_from_slice(&v.to_le_bytes());
}

#[inline]
fn put_i16(buf: &mut Vec<u8>, v: i16) {
    buf.extend_from_slice(&v.to_le_bytes());
}

#[inline]
fn put_i32(buf: &mut Vec<u8>, v: i32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

#[inline]
fn put_bytes(buf: &mut Vec<u8>, bytes: &[u8]) {
    put_uvarint(buf, bytes.len() as u64);
    buf.extend_from_slice(bytes);
}

#[inline]
fn put_opt_str(buf: &mut Vec<u8>, value: &Option<String>) {
    match value {
        Some(text) => {
            buf.push(1);
            put_bytes(buf, text.as_bytes());
        }
        None => buf.push(0),
    }
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    #[inline]
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    #[inline]
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .context("protocol length overflow")?;
        let slice = self
            .data
            .get(self.pos..end)
            .context("protocol payload truncated")?;
        self.pos = end;
        Ok(slice)
    }

    /// Read a fixed-size array with a single bounds check. The const length lets
    /// the compiler drop the bounds check inside `from_le_bytes` entirely.
    #[inline]
    fn take_array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let end = self.pos + N;
        let arr: [u8; N] = self
            .data
            .get(self.pos..end)
            .context("protocol payload truncated")?
            .try_into()
            .expect("slice length checked to equal N");
        self.pos = end;
        Ok(arr)
    }

    #[inline]
    fn u8(&mut self) -> Result<u8> {
        let byte = *self
            .data
            .get(self.pos)
            .context("protocol payload truncated")?;
        self.pos += 1;
        Ok(byte)
    }

    #[inline]
    fn bool(&mut self) -> Result<bool> {
        Ok(self.u8()? != 0)
    }

    #[inline]
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take_array::<2>()?))
    }

    #[inline]
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take_array::<4>()?))
    }

    #[inline]
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take_array::<8>()?))
    }

    #[inline]
    fn i16(&mut self) -> Result<i16> {
        Ok(i16::from_le_bytes(self.take_array::<2>()?))
    }

    #[inline]
    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take_array::<4>()?))
    }

    #[inline]
    fn uvarint(&mut self) -> Result<u64> {
        let mut result: u64 = 0;
        let mut shift: u32 = 0;
        loop {
            let byte = self.u8()?;
            if shift >= 64 {
                anyhow::bail!("protocol varint overflows u64");
            }
            result |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(result);
            }
            shift += 7;
        }
    }

    #[inline]
    fn bytes(&mut self) -> Result<Vec<u8>> {
        let len = usize::try_from(self.uvarint()?).context("protocol length exceeds usize")?;
        Ok(self.take(len)?.to_vec())
    }

    #[inline]
    fn string(&mut self) -> Result<String> {
        String::from_utf8(self.bytes()?).context("protocol string is not valid UTF-8")
    }

    #[inline]
    fn opt_string(&mut self) -> Result<Option<String>> {
        if self.bool()? {
            Ok(Some(self.string()?))
        } else {
            Ok(None)
        }
    }

    #[inline]
    fn finish(&self) -> Result<()> {
        if self.pos != self.data.len() {
            anyhow::bail!("protocol payload has trailing bytes");
        }
        Ok(())
    }
}

#[inline]
fn put_key_event(buf: &mut Vec<u8>, event: &KeyEvent) {
    buf.push(event.kind as u8);
    put_bytes(buf, &event.bytes);
    put_opt_str(buf, &event.text);
    buf.push(event.modifiers);
}

#[inline]
fn read_key_event(r: &mut Reader) -> Result<KeyEvent> {
    let kind = match r.u8()? {
        1 => KeyEventKind::Press,
        2 => KeyEventKind::Repeat,
        3 => KeyEventKind::Release,
        other => anyhow::bail!("invalid key event kind {other}"),
    };
    Ok(KeyEvent {
        kind,
        bytes: r.bytes()?,
        text: r.opt_string()?,
        modifiers: r.u8()?,
    })
}

#[inline]
fn put_mouse_event(buf: &mut Vec<u8>, event: &MouseEvent) {
    buf.push(event.kind as u8);
    buf.push(event.button as u8);
    put_u16(buf, event.col);
    put_u16(buf, event.row);
    buf.push(event.modifiers);
}

#[inline]
fn read_mouse_event(r: &mut Reader) -> Result<MouseEvent> {
    let kind = match r.u8()? {
        1 => MouseEventKind::Press,
        2 => MouseEventKind::Release,
        3 => MouseEventKind::Move,
        4 => MouseEventKind::ScrollUp,
        5 => MouseEventKind::ScrollDown,
        other => anyhow::bail!("invalid mouse event kind {other}"),
    };
    let button = match r.u8()? {
        0 => MouseButton::None,
        1 => MouseButton::Left,
        2 => MouseButton::Middle,
        3 => MouseButton::Right,
        other => anyhow::bail!("invalid mouse button {other}"),
    };
    Ok(MouseEvent {
        kind,
        button,
        col: r.u16()?,
        row: r.u16()?,
        modifiers: r.u8()?,
    })
}

#[inline]
fn put_modes(buf: &mut Vec<u8>, modes: &WindowModes) {
    let bits = (modes.bracketed_paste as u8)
        | (modes.focus_events as u8) << 1
        | (modes.alternate_scroll as u8) << 2
        | (modes.application_cursor_keys as u8) << 3
        | (modes.in_alt_screen as u8) << 4;
    buf.push(bits);
    buf.push(modes.mouse_mode);
    buf.push(modes.kitty_keyboard_flags);
}

#[inline]
fn read_modes(r: &mut Reader) -> Result<WindowModes> {
    let bits = r.u8()?;
    Ok(WindowModes {
        bracketed_paste: bits & 0x01 != 0,
        focus_events: bits & 0x02 != 0,
        alternate_scroll: bits & 0x04 != 0,
        application_cursor_keys: bits & 0x08 != 0,
        in_alt_screen: bits & 0x10 != 0,
        mouse_mode: r.u8()?,
        kitty_keyboard_flags: r.u8()?,
    })
}

#[inline]
fn put_metrics(buf: &mut Vec<u8>, metrics: &CellMetrics) {
    put_u16(buf, metrics.cell_width);
    put_u16(buf, metrics.cell_height);
    put_u16(buf, metrics.baseline);
}

#[inline]
fn read_metrics(r: &mut Reader) -> Result<CellMetrics> {
    Ok(CellMetrics {
        cell_width: r.u16()?,
        cell_height: r.u16()?,
        baseline: r.u16()?,
    })
}

#[inline]
fn put_dirty_cell(buf: &mut Vec<u8>, cell: &DirtyCell) {
    put_u16(buf, cell.row);
    put_u16(buf, cell.col);
    put_u32(buf, cell.ch);
    put_opt_str(buf, &cell.grapheme);
    put_u32(buf, cell.fg);
    put_u32(buf, cell.bg);
    put_u32(buf, cell.underline_color);
    put_u16(buf, cell.hyperlink_id);
    buf.push(cell.attrs);
    buf.push(cell.flags);
    buf.push(cell.underline_style);
}

#[inline]
fn read_dirty_cell(r: &mut Reader) -> Result<DirtyCell> {
    Ok(DirtyCell {
        row: r.u16()?,
        col: r.u16()?,
        ch: r.u32()?,
        grapheme: r.opt_string()?,
        fg: r.u32()?,
        bg: r.u32()?,
        underline_color: r.u32()?,
        hyperlink_id: r.u16()?,
        attrs: r.u8()?,
        flags: r.u8()?,
        underline_style: r.u8()?,
    })
}

#[inline]
fn put_cursor(buf: &mut Vec<u8>, cursor: &Option<CursorState>) {
    match cursor {
        Some(cursor) => {
            buf.push(1);
            put_u16(buf, cursor.row);
            put_u16(buf, cursor.col);
            buf.push(cursor.style);
            buf.push(cursor.visible as u8);
        }
        None => buf.push(0),
    }
}

#[inline]
fn read_cursor(r: &mut Reader) -> Result<Option<CursorState>> {
    if !r.bool()? {
        return Ok(None);
    }
    Ok(Some(CursorState {
        row: r.u16()?,
        col: r.u16()?,
        style: r.u8()?,
        visible: r.bool()?,
    }))
}

#[inline]
fn put_glyph(buf: &mut Vec<u8>, glyph: &GlyphBitmap) {
    put_u32(buf, glyph.glyph_id);
    put_opt_str(buf, &glyph.grapheme);
    put_u16(buf, glyph.width);
    put_u16(buf, glyph.height);
    put_i16(buf, glyph.bearing_x);
    put_i16(buf, glyph.bearing_y);
    buf.push(glyph.cells);
    buf.push(glyph.is_color as u8);
    put_bytes(buf, &glyph.pixels);
}

#[inline]
fn read_glyph(r: &mut Reader) -> Result<GlyphBitmap> {
    Ok(GlyphBitmap {
        glyph_id: r.u32()?,
        grapheme: r.opt_string()?,
        width: r.u16()?,
        height: r.u16()?,
        bearing_x: r.i16()?,
        bearing_y: r.i16()?,
        cells: r.u8()?,
        is_color: r.bool()?,
        pixels: r.bytes()?,
    })
}

pub fn encode_client_message(message: &ClientMessage) -> Result<Vec<u8>> {
    let buf = match message {
        ClientMessage::Ping { build_id } => {
            let mut buf = Vec::with_capacity(6 + build_id.len());
            buf.push(0);
            put_bytes(&mut buf, build_id.as_bytes());
            buf
        }
        ClientMessage::NewWindow { cols, rows, dpi } => {
            let mut buf = Vec::with_capacity(9);
            buf.push(1);
            put_u16(&mut buf, *cols);
            put_u16(&mut buf, *rows);
            put_u32(&mut buf, *dpi);
            buf
        }
        ClientMessage::KeyInput { window_id, event } => {
            let text_len = event.text.as_ref().map_or(0, |t| t.len() + 5);
            let mut buf = Vec::with_capacity(18 + event.bytes.len() + text_len);
            buf.push(2);
            put_u32(&mut buf, *window_id);
            put_key_event(&mut buf, event);
            buf
        }
        ClientMessage::MouseInput { window_id, event } => {
            let mut buf = Vec::with_capacity(12);
            buf.push(3);
            put_u32(&mut buf, *window_id);
            put_mouse_event(&mut buf, event);
            buf
        }
        ClientMessage::Resize {
            window_id,
            cols,
            rows,
        } => {
            let mut buf = Vec::with_capacity(9);
            buf.push(4);
            put_u32(&mut buf, *window_id);
            put_u16(&mut buf, *cols);
            put_u16(&mut buf, *rows);
            buf
        }
        ClientMessage::CloseWindow { window_id } => {
            let mut buf = Vec::with_capacity(5);
            buf.push(5);
            put_u32(&mut buf, *window_id);
            buf
        }
        ClientMessage::Paste { window_id, text } => {
            let mut buf = Vec::with_capacity(10 + text.len());
            buf.push(6);
            put_u32(&mut buf, *window_id);
            put_bytes(&mut buf, text);
            buf
        }
    };
    Ok(buf)
}

// Each `DirtyCell` is at most: row(2)+col(2)+ch(4)+grapheme_tag(1)+
// grapheme_len(<=5)+grapheme(g)+fg(4)+bg(4)+ul(4)+link(2)+attrs(1)+flags(1)+
// ustyle(1) bytes. The fixed part is 33 plus any grapheme payload.
const DIRTY_CELL_MAX_FIXED: usize = 33;

pub fn decode_client_message(bytes: &[u8]) -> Result<ClientMessage> {
    let mut r = Reader::new(bytes);
    let message = match r.u8()? {
        0 => ClientMessage::Ping {
            build_id: r.string()?,
        },
        1 => ClientMessage::NewWindow {
            cols: r.u16()?,
            rows: r.u16()?,
            dpi: r.u32()?,
        },
        2 => ClientMessage::KeyInput {
            window_id: r.u32()?,
            event: read_key_event(&mut r)?,
        },
        3 => ClientMessage::MouseInput {
            window_id: r.u32()?,
            event: read_mouse_event(&mut r)?,
        },
        4 => ClientMessage::Resize {
            window_id: r.u32()?,
            cols: r.u16()?,
            rows: r.u16()?,
        },
        5 => ClientMessage::CloseWindow {
            window_id: r.u32()?,
        },
        6 => ClientMessage::Paste {
            window_id: r.u32()?,
            text: r.bytes()?,
        },
        other => anyhow::bail!("invalid client message tag {other}"),
    };
    r.finish()?;
    Ok(message)
}

pub fn encode_server_message(message: &ServerMessage) -> Result<Vec<u8>> {
    let buf = match message {
        ServerMessage::Pong { build_id } => {
            let mut buf = Vec::with_capacity(6 + build_id.len());
            buf.push(0);
            put_bytes(&mut buf, build_id.as_bytes());
            buf
        }
        ServerMessage::WindowCreated {
            window_id,
            cols,
            rows,
            metrics,
            modes,
        } => {
            let mut buf = Vec::with_capacity(17);
            buf.push(1);
            put_u32(&mut buf, *window_id);
            put_u16(&mut buf, *cols);
            put_u16(&mut buf, *rows);
            put_metrics(&mut buf, metrics);
            put_modes(&mut buf, modes);
            buf
        }
        ServerMessage::WindowResized {
            window_id,
            cols,
            rows,
            metrics,
            modes,
        } => {
            let mut buf = Vec::with_capacity(17);
            buf.push(2);
            put_u32(&mut buf, *window_id);
            put_u16(&mut buf, *cols);
            put_u16(&mut buf, *rows);
            put_metrics(&mut buf, metrics);
            put_modes(&mut buf, modes);
            buf
        }
        ServerMessage::CellUpdate {
            window_id,
            dirty_cells,
            cursor,
            modes,
        } => {
            // tag + window_id + count + per-cell + cursor + modes, sized so a
            // grapheme-free batch never reallocates mid-encode.
            let grapheme_bytes: usize = dirty_cells
                .iter()
                .map(|c| c.grapheme.as_ref().map_or(0, |g| g.len() + 5))
                .sum();
            let mut buf =
                Vec::with_capacity(16 + dirty_cells.len() * DIRTY_CELL_MAX_FIXED + grapheme_bytes);
            buf.push(3);
            put_u32(&mut buf, *window_id);
            put_uvarint(&mut buf, dirty_cells.len() as u64);
            for cell in dirty_cells {
                put_dirty_cell(&mut buf, cell);
            }
            put_cursor(&mut buf, cursor);
            put_modes(&mut buf, modes);
            buf
        }
        ServerMessage::SetTitle { window_id, title } => {
            let mut buf = Vec::with_capacity(10 + title.len());
            buf.push(4);
            put_u32(&mut buf, *window_id);
            put_bytes(&mut buf, title.as_bytes());
            buf
        }
        ServerMessage::Bell { window_id } => {
            let mut buf = Vec::with_capacity(5);
            buf.push(5);
            put_u32(&mut buf, *window_id);
            buf
        }
        ServerMessage::CopyToClipboard { window_id, text } => {
            let mut buf = Vec::with_capacity(10 + text.len());
            buf.push(6);
            put_u32(&mut buf, *window_id);
            put_bytes(&mut buf, text);
            buf
        }
        ServerMessage::WindowClosed {
            window_id,
            exit_code,
        } => {
            let mut buf = Vec::with_capacity(11);
            buf.push(7);
            put_u32(&mut buf, *window_id);
            match exit_code {
                Some(code) => {
                    buf.push(1);
                    put_i32(&mut buf, *code);
                }
                None => buf.push(0),
            }
            buf
        }
        ServerMessage::KittyImageState {
            window_id,
            generation,
            images,
            placements,
        } => {
            let images_bytes: usize = images.iter().map(|i| i.data.len() + 18).sum();
            let mut buf = Vec::with_capacity(20 + images_bytes + placements.len() * 12);
            buf.push(8);
            put_u32(&mut buf, *window_id);
            put_u64(&mut buf, *generation);
            put_uvarint(&mut buf, images.len() as u64);
            for image in images {
                put_u32(&mut buf, image.id);
                put_u32(&mut buf, image.width);
                put_u32(&mut buf, image.height);
                put_bytes(&mut buf, &image.data);
            }
            put_uvarint(&mut buf, placements.len() as u64);
            for placement in placements {
                put_u32(&mut buf, placement.image_id);
                put_u16(&mut buf, placement.col);
                put_u16(&mut buf, placement.row);
                put_u16(&mut buf, placement.cols);
                put_u16(&mut buf, placement.rows);
            }
            buf
        }
        ServerMessage::AtlasUpdate { glyph } => {
            let mut buf = Vec::with_capacity(20 + glyph.pixels.len());
            buf.push(9);
            put_glyph(&mut buf, glyph);
            buf
        }
    };
    Ok(buf)
}

pub fn decode_server_message(bytes: &[u8]) -> Result<ServerMessage> {
    let mut r = Reader::new(bytes);
    let message = match r.u8()? {
        0 => ServerMessage::Pong {
            build_id: r.string()?,
        },
        1 => ServerMessage::WindowCreated {
            window_id: r.u32()?,
            cols: r.u16()?,
            rows: r.u16()?,
            metrics: read_metrics(&mut r)?,
            modes: read_modes(&mut r)?,
        },
        2 => ServerMessage::WindowResized {
            window_id: r.u32()?,
            cols: r.u16()?,
            rows: r.u16()?,
            metrics: read_metrics(&mut r)?,
            modes: read_modes(&mut r)?,
        },
        3 => {
            let window_id = r.u32()?;
            let count = usize::try_from(r.uvarint()?).context("dirty cell count exceeds usize")?;
            // Cap the pre-allocation: each cell needs at least one byte on the
            // wire, so a corrupt count cannot make us reserve beyond the input.
            let mut dirty_cells = Vec::with_capacity(count.min(bytes.len()));
            for _ in 0..count {
                dirty_cells.push(read_dirty_cell(&mut r)?);
            }
            ServerMessage::CellUpdate {
                window_id,
                dirty_cells,
                cursor: read_cursor(&mut r)?,
                modes: read_modes(&mut r)?,
            }
        }
        4 => ServerMessage::SetTitle {
            window_id: r.u32()?,
            title: r.string()?,
        },
        5 => ServerMessage::Bell {
            window_id: r.u32()?,
        },
        6 => ServerMessage::CopyToClipboard {
            window_id: r.u32()?,
            text: r.bytes()?,
        },
        7 => {
            let window_id = r.u32()?;
            let exit_code = if r.bool()? { Some(r.i32()?) } else { None };
            ServerMessage::WindowClosed {
                window_id,
                exit_code,
            }
        }
        8 => {
            let window_id = r.u32()?;
            let generation = r.u64()?;
            let image_count = usize::try_from(r.uvarint()?).context("image count exceeds usize")?;
            let mut images = Vec::with_capacity(image_count.min(bytes.len()));
            for _ in 0..image_count {
                images.push(KittyImageData {
                    id: r.u32()?,
                    width: r.u32()?,
                    height: r.u32()?,
                    data: r.bytes()?,
                });
            }
            let placement_count =
                usize::try_from(r.uvarint()?).context("placement count exceeds usize")?;
            let mut placements = Vec::with_capacity(placement_count.min(bytes.len()));
            for _ in 0..placement_count {
                placements.push(KittyImagePlacement {
                    image_id: r.u32()?,
                    col: r.u16()?,
                    row: r.u16()?,
                    cols: r.u16()?,
                    rows: r.u16()?,
                });
            }
            ServerMessage::KittyImageState {
                window_id,
                generation,
                images,
                placements,
            }
        }
        9 => ServerMessage::AtlasUpdate {
            glyph: read_glyph(&mut r)?,
        },
        other => anyhow::bail!("invalid server message tag {other}"),
    };
    r.finish()?;
    Ok(message)
}

pub fn write_client_message<W: Write>(writer: &mut W, message: &ClientMessage) -> Result<()> {
    write_frame(writer, &encode_client_message(message)?)
}

pub fn read_client_message<R: Read>(reader: &mut R) -> Result<ClientMessage> {
    decode_client_message(&read_frame(reader)?)
}

pub fn write_server_message<W: Write>(writer: &mut W, message: &ServerMessage) -> Result<()> {
    write_frame(writer, &encode_server_message(message)?)
}

pub fn read_server_message<R: Read>(reader: &mut R) -> Result<ServerMessage> {
    decode_server_message(&read_frame(reader)?)
}

fn write_frame<W: Write>(writer: &mut W, bytes: &[u8]) -> Result<()> {
    let len = u32::try_from(bytes.len()).context("protocol frame too large to encode")?;
    writer
        .write_all(&len.to_le_bytes())
        .context("failed to write protocol frame length")?;
    writer
        .write_all(bytes)
        .context("failed to write protocol frame payload")?;
    Ok(())
}

fn read_frame<R: Read>(reader: &mut R) -> Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    reader
        .read_exact(&mut len_buf)
        .context("failed to read protocol frame length")?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > MAX_FRAME_SIZE {
        anyhow::bail!("protocol frame exceeds max size");
    }
    let mut payload = vec![0u8; len];
    reader
        .read_exact(&mut payload)
        .context("failed to read protocol frame payload")?;
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn sample_metrics() -> CellMetrics {
        CellMetrics {
            cell_width: 9,
            cell_height: 18,
            baseline: 14,
        }
    }

    fn sample_dirty_cell(row: u16, col: u16, ch: char) -> DirtyCell {
        DirtyCell {
            row,
            col,
            ch: ch as u32,
            grapheme: None,
            fg: 0x112233,
            bg: 0x445566,
            underline_color: 0x778899,
            hyperlink_id: 7,
            attrs: 0x3,
            flags: 0x1,
            underline_style: 2,
        }
    }

    #[test]
    fn client_protocol_messages_roundtrip() {
        let message = ClientMessage::Ping {
            build_id: "test-build".to_string(),
        };

        let encoded = encode_client_message(&message).expect("client message should encode");
        let decoded = decode_client_message(&encoded).expect("client message should decode");

        assert_eq!(decoded, message);
    }

    #[test]
    fn key_input_messages_roundtrip_utf8_text_and_bytes() {
        let message = ClientMessage::KeyInput {
            window_id: 11,
            event: KeyEvent {
                kind: KeyEventKind::Press,
                bytes: "👍🏻".as_bytes().to_vec(),
                text: Some("👍🏻".to_string()),
                modifiers: 0,
            },
        };

        let encoded = encode_client_message(&message).expect("key input should encode");
        let decoded = decode_client_message(&encoded).expect("key input should decode");

        assert_eq!(decoded, message);
    }

    #[test]
    fn new_window_client_message_roundtrips_with_dpi() {
        let message = ClientMessage::NewWindow {
            cols: 80,
            rows: 24,
            dpi: 144,
        };

        let encoded = encode_client_message(&message).expect("new window should encode");
        let decoded = decode_client_message(&encoded).expect("new window should decode");

        assert_eq!(decoded, message);
    }

    #[test]
    fn server_protocol_messages_roundtrip() {
        let message = ServerMessage::Pong {
            build_id: "test-build".to_string(),
        };

        let encoded = encode_server_message(&message).expect("server message should encode");
        let decoded = decode_server_message(&encoded).expect("server message should decode");

        assert_eq!(decoded, message);
    }

    #[test]
    fn server_cell_update_messages_roundtrip() {
        let message = ServerMessage::CellUpdate {
            window_id: 9,
            dirty_cells: vec![sample_dirty_cell(0, 0, 'h'), sample_dirty_cell(0, 1, 'i')],
            cursor: Some(CursorState {
                row: 0,
                col: 2,
                style: 1,
                visible: true,
            }),
            modes: WindowModes {
                bracketed_paste: true,
                focus_events: false,
                alternate_scroll: true,
                application_cursor_keys: true,
                in_alt_screen: true,
                mouse_mode: 2,
                kitty_keyboard_flags: 5,
            },
        };

        let encoded = encode_server_message(&message).expect("server cell update should encode");
        let decoded = decode_server_message(&encoded).expect("server cell update should decode");

        assert_eq!(decoded, message);
    }

    #[test]
    fn server_resize_message_roundtrips() {
        let message = ServerMessage::WindowResized {
            window_id: 3,
            cols: 120,
            rows: 40,
            metrics: sample_metrics(),
            modes: WindowModes::default(),
        };
        let encoded = encode_server_message(&message).expect("resize message should encode");
        let decoded = decode_server_message(&encoded).expect("resize message should decode");
        assert_eq!(decoded, message);
    }

    #[test]
    fn server_grapheme_cells_roundtrip() {
        let message = ServerMessage::CellUpdate {
            window_id: 12,
            dirty_cells: vec![DirtyCell {
                row: 0,
                col: 0,
                ch: '❤' as u32,
                grapheme: Some("❤️".to_string()),
                fg: 0xffffff,
                bg: 0,
                underline_color: 0,
                hyperlink_id: 0,
                attrs: 0,
                flags: 1,
                underline_style: 0,
            }],
            cursor: None,
            modes: WindowModes::default(),
        };

        let encoded = encode_server_message(&message).expect("grapheme update should encode");
        let decoded = decode_server_message(&encoded).expect("grapheme update should decode");
        assert_eq!(decoded, message);
    }

    #[test]
    fn server_complex_emoji_grapheme_cells_roundtrip() {
        let samples = [
            "🇺🇸",
            "👨‍👩‍👧‍👦",
            "👍🏻",
            "1️⃣",
            "🏴\u{E0067}\u{E0062}\u{E0065}\u{E006E}\u{E0067}\u{E007F}",
        ];

        let message = ServerMessage::CellUpdate {
            window_id: 27,
            dirty_cells: samples
                .into_iter()
                .enumerate()
                .map(|(col, grapheme)| DirtyCell {
                    row: 0,
                    col: col as u16,
                    ch: grapheme.chars().next().unwrap_or(' ') as u32,
                    grapheme: Some(grapheme.to_string()),
                    fg: 0xffffff,
                    bg: 0,
                    underline_color: 0,
                    hyperlink_id: 0,
                    attrs: 0,
                    flags: 1,
                    underline_style: 0,
                })
                .collect(),
            cursor: None,
            modes: WindowModes::default(),
        };

        let encoded =
            encode_server_message(&message).expect("complex grapheme update should encode");
        let decoded =
            decode_server_message(&encoded).expect("complex grapheme update should decode");
        assert_eq!(decoded, message);
    }

    #[test]
    fn atlas_update_roundtrips_color_payload() {
        let message = ServerMessage::AtlasUpdate {
            glyph: GlyphBitmap {
                glyph_id: 77,
                grapheme: Some("❤️".to_string()),
                width: 4,
                height: 4,
                bearing_x: -1,
                bearing_y: 3,
                cells: 2,
                is_color: true,
                pixels: vec![
                    255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
                ],
            },
        };

        let encoded = encode_server_message(&message).expect("atlas update should encode");
        let decoded = decode_server_message(&encoded).expect("atlas update should decode");

        assert_eq!(decoded, message);
    }

    #[test]
    fn kitty_image_state_roundtrips() {
        let message = ServerMessage::KittyImageState {
            window_id: 9,
            generation: 3,
            images: vec![KittyImageData {
                id: 7,
                width: 1,
                height: 1,
                data: vec![255, 0, 0, 255],
            }],
            placements: vec![KittyImagePlacement {
                image_id: 7,
                col: 0,
                row: 1,
                cols: 2,
                rows: 1,
            }],
        };

        let encoded = encode_server_message(&message).expect("kitty image state should encode");
        let decoded = decode_server_message(&encoded).expect("kitty image state should decode");

        assert_eq!(decoded, message);
    }

    #[test]
    fn truncated_protocol_messages_fail_to_decode() {
        let encoded = encode_client_message(&ClientMessage::NewWindow {
            cols: 80,
            rows: 24,
            dpi: 96,
        })
        .expect("new window should encode");

        for len in 0..encoded.len() {
            let truncated = &encoded[..len];
            assert!(
                decode_client_message(truncated).is_err(),
                "truncated client message of length {len} should fail"
            );
        }
    }

    #[test]
    fn framed_client_messages_roundtrip() {
        let message = ClientMessage::Paste {
            window_id: 3,
            text: b"printf 'hi'\n".to_vec(),
        };
        let mut buf = Vec::new();
        write_client_message(&mut buf, &message).expect("framed client message should write");
        let mut cursor = Cursor::new(buf);
        let decoded = read_client_message(&mut cursor).expect("framed client message should read");
        assert_eq!(decoded, message);
    }

    #[test]
    fn framed_server_messages_roundtrip() {
        let message = ServerMessage::SetTitle {
            window_id: 11,
            title: "handterm [server]".to_string(),
        };
        let mut buf = Vec::new();
        write_server_message(&mut buf, &message).expect("framed server message should write");
        let mut cursor = Cursor::new(buf);
        let decoded = read_server_message(&mut cursor).expect("framed server message should read");
        assert_eq!(decoded, message);
    }

    #[test]
    fn oversized_frame_is_rejected() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&((MAX_FRAME_SIZE as u32) + 1).to_le_bytes());
        buf.extend_from_slice(&[0u8; 8]);

        let mut cursor = Cursor::new(buf);
        assert!(read_client_message(&mut cursor).is_err());
    }

    #[test]
    fn all_client_message_variants_roundtrip() {
        let messages = [
            ClientMessage::Ping {
                build_id: "build".to_string(),
            },
            ClientMessage::NewWindow {
                cols: 200,
                rows: 60,
                dpi: 192,
            },
            ClientMessage::KeyInput {
                window_id: 4_000_000_000,
                event: KeyEvent {
                    kind: KeyEventKind::Release,
                    bytes: b"\x1b[1;2A".to_vec(),
                    text: Some("héllo".to_string()),
                    modifiers: 0xff,
                },
            },
            ClientMessage::MouseInput {
                window_id: 5,
                event: MouseEvent {
                    kind: MouseEventKind::ScrollDown,
                    button: MouseButton::Middle,
                    col: 4000,
                    row: 9000,
                    modifiers: 3,
                },
            },
            ClientMessage::Resize {
                window_id: 6,
                cols: 80,
                rows: 24,
            },
            ClientMessage::CloseWindow { window_id: 7 },
            ClientMessage::Paste {
                window_id: 8,
                text: vec![0, 1, 2, 255, 254, 127],
            },
        ];

        for message in messages {
            let encoded = encode_client_message(&message).expect("client message should encode");
            let decoded = decode_client_message(&encoded).expect("client message should decode");
            assert_eq!(decoded, message);
        }
    }

    #[test]
    fn all_server_message_variants_roundtrip() {
        let messages = [
            ServerMessage::Pong {
                build_id: "build".to_string(),
            },
            ServerMessage::WindowCreated {
                window_id: 1,
                cols: 80,
                rows: 24,
                metrics: sample_metrics(),
                modes: WindowModes {
                    bracketed_paste: true,
                    focus_events: true,
                    alternate_scroll: false,
                    application_cursor_keys: true,
                    in_alt_screen: false,
                    mouse_mode: 3,
                    kitty_keyboard_flags: 9,
                },
            },
            ServerMessage::Bell { window_id: 2 },
            ServerMessage::CopyToClipboard {
                window_id: 3,
                text: b"clipboard".to_vec(),
            },
            ServerMessage::WindowClosed {
                window_id: 4,
                exit_code: Some(-1),
            },
            ServerMessage::WindowClosed {
                window_id: 5,
                exit_code: None,
            },
        ];

        for message in messages {
            let encoded = encode_server_message(&message).expect("server message should encode");
            let decoded = decode_server_message(&encoded).expect("server message should decode");
            assert_eq!(decoded, message);
        }
    }

    #[test]
    fn window_modes_bits_roundtrip_independently() {
        // Exercise every boolean flag in isolation so a packing/unpacking shift
        // mistake cannot hide behind another set flag.
        for bit in 0..5u8 {
            let modes = WindowModes {
                bracketed_paste: bit == 0,
                focus_events: bit == 1,
                alternate_scroll: bit == 2,
                application_cursor_keys: bit == 3,
                in_alt_screen: bit == 4,
                mouse_mode: bit,
                kitty_keyboard_flags: bit.wrapping_mul(7),
            };
            let message = ServerMessage::WindowResized {
                window_id: 1,
                cols: 80,
                rows: 24,
                metrics: sample_metrics(),
                modes,
            };
            let encoded = encode_server_message(&message).expect("encode");
            let decoded = decode_server_message(&encoded).expect("decode");
            assert_eq!(decoded, message);
        }
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        let message = ClientMessage::CloseWindow { window_id: 1 };
        let mut encoded = encode_client_message(&message).expect("encode");
        encoded.push(0); // extra garbage byte
        assert!(decode_client_message(&encoded).is_err());

        let server = ServerMessage::Bell { window_id: 1 };
        let mut encoded = encode_server_message(&server).expect("encode");
        encoded.push(0);
        assert!(decode_server_message(&encoded).is_err());
    }

    #[test]
    fn invalid_message_tags_are_rejected() {
        assert!(decode_client_message(&[200]).is_err());
        assert!(decode_server_message(&[200]).is_err());
        assert!(decode_client_message(&[]).is_err());
        assert!(decode_server_message(&[]).is_err());
    }

    #[test]
    fn cell_update_payload_is_smaller_than_bincode() {
        // The hot-path cell update from the protocol benchmark must stay smaller
        // on the wire than the previous bincode encoding (1-byte tags, packed
        // modes, and varint length prefixes replace bincode's 4-byte enum tags,
        // 8-byte lengths, and per-bool mode bytes).
        let message = ServerMessage::CellUpdate {
            window_id: 7,
            dirty_cells: vec![sample_dirty_cell(0, 0, 'h'), sample_dirty_cell(0, 1, 'i')],
            cursor: Some(CursorState {
                row: 0,
                col: 2,
                style: 1,
                visible: true,
            }),
            modes: WindowModes::default(),
        };
        let encoded = encode_server_message(&message).expect("encode");
        let bincode_len = bincode::serialize(&message).expect("bincode encode").len();
        assert!(
            encoded.len() < bincode_len,
            "compact codec ({} bytes) should beat bincode ({} bytes)",
            encoded.len(),
            bincode_len
        );
    }

    #[test]
    fn small_key_input_is_single_allocation_sized() {
        // A typical arrow-key input encodes to a handful of bytes.
        let message = ClientMessage::KeyInput {
            window_id: 7,
            event: KeyEvent {
                kind: KeyEventKind::Press,
                bytes: b"\x1b[A".to_vec(),
                text: None,
                modifiers: 0,
            },
        };
        let encoded = encode_client_message(&message).expect("encode");
        assert!(
            encoded.len() <= 12,
            "small key input should stay tiny, got {} bytes",
            encoded.len()
        );
        let decoded = decode_client_message(&encoded).expect("decode");
        assert_eq!(decoded, message);
    }

    // In-process A/B microbench. Run with:
    //   cargo test -p handterm-common --release -- --ignored --nocapture codec_ab
    // Compares the hand-rolled codec against bincode within one process so the
    // numbers are not polluted by machine load between separate binaries.
    #[test]
    #[ignore]
    fn codec_ab_microbench() {
        use bincode::Options;
        use std::time::Instant;
        let varint = bincode::DefaultOptions::new();
        let client = ClientMessage::KeyInput {
            window_id: 7,
            event: KeyEvent {
                kind: KeyEventKind::Press,
                bytes: b"\x1b[A".to_vec(),
                text: None,
                modifiers: 0b101,
            },
        };
        let server = ServerMessage::CellUpdate {
            window_id: 7,
            dirty_cells: vec![sample_dirty_cell(0, 0, 'h'), sample_dirty_cell(0, 1, 'i')],
            cursor: Some(CursorState {
                row: 0,
                col: 2,
                style: 1,
                visible: true,
            }),
            modes: WindowModes::default(),
        };

        let iters = 2_000_000usize;
        // Interleave the two codecs in small alternating chunks so transient
        // machine load (sibling processes, frequency scaling) hits both equally.
        let chunk = 5_000usize;
        let cb_fixed = encode_client_message(&client).unwrap();
        let sb_fixed = encode_server_message(&server).unwrap();
        let cb_bin = bincode::serialize(&client).unwrap();
        let sb_bin = bincode::serialize(&server).unwrap();

        let mut c_enc = 0u128;
        let mut b_enc = 0u128;
        let mut v_enc = 0u128;
        let mut c_dec = 0u128;
        let mut b_dec = 0u128;
        let mut v_dec = 0u128;
        let cb_var = varint.serialize(&client).unwrap();
        let sb_var = varint.serialize(&server).unwrap();
        let mut done = 0usize;
        while done < iters {
            let n = chunk.min(iters - done);

            // encode phase
            let s = Instant::now();
            for _ in 0..n {
                std::hint::black_box(encode_client_message(&client).unwrap());
                std::hint::black_box(encode_server_message(&server).unwrap());
            }
            c_enc += s.elapsed().as_nanos();
            let s = Instant::now();
            for _ in 0..n {
                std::hint::black_box(bincode::serialize(&client).unwrap());
                std::hint::black_box(bincode::serialize(&server).unwrap());
            }
            b_enc += s.elapsed().as_nanos();
            let s = Instant::now();
            for _ in 0..n {
                std::hint::black_box(varint.serialize(&client).unwrap());
                std::hint::black_box(varint.serialize(&server).unwrap());
            }
            v_enc += s.elapsed().as_nanos();

            // decode phase
            let s = Instant::now();
            for _ in 0..n {
                std::hint::black_box(decode_client_message(&cb_fixed).unwrap());
                std::hint::black_box(decode_server_message(&sb_fixed).unwrap());
            }
            c_dec += s.elapsed().as_nanos();
            let s = Instant::now();
            for _ in 0..n {
                std::hint::black_box(bincode::deserialize::<ClientMessage>(&cb_bin).unwrap());
                std::hint::black_box(bincode::deserialize::<ServerMessage>(&sb_bin).unwrap());
            }
            b_dec += s.elapsed().as_nanos();
            let s = Instant::now();
            for _ in 0..n {
                std::hint::black_box(varint.deserialize::<ClientMessage>(&cb_var).unwrap());
                std::hint::black_box(varint.deserialize::<ServerMessage>(&sb_var).unwrap());
            }
            v_dec += s.elapsed().as_nanos();

            done += n;
        }
        let custom = (iters as f64 * 4.0) / ((c_enc + c_dec) as f64 / 1e9);
        let bc = (iters as f64 * 4.0) / ((b_enc + b_dec) as f64 / 1e9);
        let vr = (iters as f64 * 4.0) / ((v_enc + v_dec) as f64 / 1e9);
        println!(
            "encode  custom {:.0} ms / bincode {:.0} ms / varint {:.0} ms",
            c_enc as f64 / 1e6,
            b_enc as f64 / 1e6,
            v_enc as f64 / 1e6
        );
        println!(
            "decode  custom {:.0} ms / bincode {:.0} ms / varint {:.0} ms",
            c_dec as f64 / 1e6,
            b_dec as f64 / 1e6,
            v_dec as f64 / 1e6
        );

        println!(
            "custom: {custom:.0} msg/s   bincode: {bc:.0} msg/s   varint: {vr:.0} msg/s   ratio c/b {:.3} v/b {:.3}",
            custom / bc,
            vr / bc
        );
        println!(
            "sizes  client custom {} / bincode {} / varint {}   server custom {} / bincode {} / varint {}",
            encode_client_message(&client).unwrap().len(),
            bincode::serialize(&client).unwrap().len(),
            varint.serialize(&client).unwrap().len(),
            encode_server_message(&server).unwrap().len(),
            bincode::serialize(&server).unwrap().len(),
            varint.serialize(&server).unwrap().len(),
        );
    }
}
