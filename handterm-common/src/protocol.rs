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

pub fn encode_client_message(message: &ClientMessage) -> Result<Vec<u8>> {
    bincode::serialize(message).context("failed to serialize client protocol message")
}

pub fn decode_client_message(bytes: &[u8]) -> Result<ClientMessage> {
    bincode::deserialize(bytes).context("failed to deserialize client protocol message")
}

pub fn encode_server_message(message: &ServerMessage) -> Result<Vec<u8>> {
    bincode::serialize(message).context("failed to serialize server protocol message")
}

pub fn decode_server_message(bytes: &[u8]) -> Result<ServerMessage> {
    bincode::deserialize(bytes).context("failed to deserialize server protocol message")
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
}
