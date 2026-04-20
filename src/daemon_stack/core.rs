use crate::font::{GlyphAtlas, GlyphFormat};
use crate::protocol::{
    CellMetrics, ClientMessage, CursorState, DirtyCell, GlyphBitmap, KittyImageData,
    KittyImagePlacement, MouseEvent, ServerMessage, WindowId,
};
use crate::terminal::Terminal;
use std::collections::{BTreeMap, BTreeSet, HashSet};

pub struct ServerCore {
    next_window_id: WindowId,
    scrollback_limit: usize,
    windows: BTreeMap<WindowId, ServerWindow>,
    atlases_by_dpi: BTreeMap<u32, GlyphAtlas>,
    protocol_build_id: String,
    font_family: String,
    font_size: f64,
}

struct ServerWindow {
    terminal: Terminal,
    last_kitty_generation: u64,
    dpi: u32,
    sent_glyphs: HashSet<ProtocolGlyphKey>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ProtocolGlyphKey {
    Codepoint(u32),
    Grapheme(Box<str>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerIoAction {
    SpawnWindow {
        window_id: WindowId,
        cols: u16,
        rows: u16,
    },
    Write {
        window_id: WindowId,
        bytes: Vec<u8>,
    },
    Resize {
        window_id: WindowId,
        cols: u16,
        rows: u16,
    },
    Close {
        window_id: WindowId,
    },
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ServerHandleResult {
    pub messages: Vec<ServerMessage>,
    pub io_actions: Vec<ServerIoAction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerError {
    UnknownWindow(WindowId),
}

impl ServerCore {
    pub fn new_with_style(
        scrollback_limit: usize,
        font_family: String,
        font_size: f64,
        protocol_build_id: String,
    ) -> Self {
        Self {
            next_window_id: 1,
            scrollback_limit,
            windows: BTreeMap::new(),
            atlases_by_dpi: BTreeMap::new(),
            protocol_build_id,
            font_family,
            font_size,
        }
    }

    pub fn has_window(&self, window_id: WindowId) -> bool {
        self.windows.contains_key(&window_id)
    }

    pub fn window_scrollback_limit(&self, window_id: WindowId) -> Option<usize> {
        self.windows
            .get(&window_id)
            .map(|window| window.terminal.scrollback_limit())
    }

    pub fn window_scrollback_len(&self, window_id: WindowId) -> Option<usize> {
        self.windows
            .get(&window_id)
            .map(|window| window.terminal.grid.scrollback_len())
    }

    pub fn create_window(&mut self, cols: u16, rows: u16, dpi: u32) -> ServerMessage {
        let window_id = self.next_window_id;
        self.next_window_id = self.next_window_id.wrapping_add(1).max(1);
        let metrics = self.cell_metrics_for_dpi(dpi);
        self.windows.insert(
            window_id,
            ServerWindow {
                terminal: Terminal::new_with_scrollback(cols, rows, self.scrollback_limit),
                last_kitty_generation: 0,
                dpi,
                sent_glyphs: HashSet::new(),
            },
        );
        ServerMessage::WindowCreated {
            window_id,
            cols,
            rows,
            metrics,
            modes: self
                .windows
                .get(&window_id)
                .expect("created window should exist")
                .terminal
                .window_modes(),
        }
    }

    pub fn close_window(
        &mut self,
        window_id: WindowId,
        exit_code: Option<i32>,
    ) -> Option<ServerMessage> {
        self.windows
            .remove(&window_id)
            .map(|_| ServerMessage::WindowClosed {
                window_id,
                exit_code,
            })
    }

    pub fn resize_window(
        &mut self,
        window_id: WindowId,
        cols: u16,
        rows: u16,
    ) -> Option<Vec<ServerMessage>> {
        let dpi = self.windows.get(&window_id)?.dpi;
        let metrics = self.cell_metrics_for_dpi(dpi);
        let window = self.windows.get_mut(&window_id)?;
        window.terminal.resize(cols, rows);
        let mut messages = vec![ServerMessage::WindowResized {
            window_id,
            cols,
            rows,
            metrics,
            modes: window.terminal.window_modes(),
        }];
        messages.extend(self.collect_update_batch(window_id).messages);
        Some(messages)
    }

    pub fn process_output(
        &mut self,
        window_id: WindowId,
        bytes: &[u8],
    ) -> Option<ServerHandleResult> {
        let window = self.windows.get_mut(&window_id)?;
        window.terminal.process(bytes);
        Some(self.collect_update_batch(window_id))
    }

    pub fn snapshot_window(&mut self, window_id: WindowId) -> Option<ServerHandleResult> {
        self.windows.get(&window_id)?;
        Some(self.collect_update_batch(window_id))
    }

    pub fn handle_client_message(
        &mut self,
        message: ClientMessage,
    ) -> Result<ServerHandleResult, ServerError> {
        match message {
            ClientMessage::Ping { .. } => Ok(ServerHandleResult {
                messages: vec![ServerMessage::Pong {
                    build_id: self.protocol_build_id.clone(),
                }],
                io_actions: Vec::new(),
            }),
            ClientMessage::NewWindow { cols, rows, dpi } => {
                let created = self.create_window(cols, rows, dpi);
                let window_id = match created {
                    ServerMessage::WindowCreated { window_id, .. } => window_id,
                    _ => unreachable!("create_window only emits WindowCreated"),
                };
                Ok(ServerHandleResult {
                    messages: vec![created],
                    io_actions: vec![ServerIoAction::SpawnWindow {
                        window_id,
                        cols,
                        rows,
                    }],
                })
            }
            ClientMessage::KeyInput { window_id, event } => {
                self.require_window(window_id)?;
                Ok(ServerHandleResult {
                    messages: Vec::new(),
                    io_actions: vec![ServerIoAction::Write {
                        window_id,
                        bytes: event.bytes,
                    }],
                })
            }
            ClientMessage::Paste { window_id, text } => {
                self.require_window(window_id)?;
                Ok(ServerHandleResult {
                    messages: Vec::new(),
                    io_actions: vec![ServerIoAction::Write {
                        window_id,
                        bytes: text,
                    }],
                })
            }
            ClientMessage::MouseInput { window_id, event } => {
                let window = self
                    .windows
                    .get(&window_id)
                    .ok_or(ServerError::UnknownWindow(window_id))?;
                let bytes = encode_mouse_event(&window.terminal, &event);
                Ok(ServerHandleResult {
                    messages: Vec::new(),
                    io_actions: bytes
                        .into_iter()
                        .map(|bytes| ServerIoAction::Write { window_id, bytes })
                        .collect(),
                })
            }
            ClientMessage::Resize {
                window_id,
                cols,
                rows,
            } => {
                let messages = self
                    .resize_window(window_id, cols, rows)
                    .ok_or(ServerError::UnknownWindow(window_id))?;
                Ok(ServerHandleResult {
                    messages,
                    io_actions: vec![ServerIoAction::Resize {
                        window_id,
                        cols,
                        rows,
                    }],
                })
            }
            ClientMessage::CloseWindow { window_id } => {
                let closed = self
                    .close_window(window_id, None)
                    .ok_or(ServerError::UnknownWindow(window_id))?;
                Ok(ServerHandleResult {
                    messages: vec![closed],
                    io_actions: vec![ServerIoAction::Close { window_id }],
                })
            }
        }
    }

    fn require_window(&self, window_id: WindowId) -> Result<(), ServerError> {
        if self.has_window(window_id) {
            Ok(())
        } else {
            Err(ServerError::UnknownWindow(window_id))
        }
    }

    fn cell_metrics_for_dpi(&mut self, dpi: u32) -> CellMetrics {
        let atlas = self.atlases_by_dpi.entry(dpi).or_insert_with(|| {
            GlyphAtlas::with_family_dpi(&self.font_family, self.font_size, dpi)
                .or_else(|_| GlyphAtlas::new_with_dpi(self.font_size, dpi))
                .expect("server glyph atlas should initialize for negotiated dpi")
        });
        CellMetrics {
            cell_width: atlas.cell_width.min(u16::MAX as usize) as u16,
            cell_height: atlas.cell_height.min(u16::MAX as usize) as u16,
            baseline: atlas.baseline.min(u16::MAX as usize) as u16,
        }
    }

    fn collect_update_batch(&mut self, window_id: WindowId) -> ServerHandleResult {
        let (windows, atlases_by_dpi) = (&mut self.windows, &mut self.atlases_by_dpi);
        let window = windows
            .get_mut(&window_id)
            .expect("collect_update_batch requires an existing window");
        let mut result = ServerHandleResult::default();
        let terminal = &mut window.terminal;
        let atlas = atlases_by_dpi.entry(window.dpi).or_insert_with(|| {
            GlyphAtlas::with_family_dpi(&self.font_family, self.font_size, window.dpi)
                .or_else(|_| GlyphAtlas::new_with_dpi(self.font_size, window.dpi))
                .expect("server glyph atlas should initialize for negotiated dpi")
        });
        let dirty_cells = dirty_cells_from_terminal(terminal);
        let cursor = cursor_state_from_terminal(terminal);
        let atlas_updates =
            atlas_updates_from_dirty_cells(atlas, &dirty_cells, &mut window.sent_glyphs);
        if !dirty_cells.is_empty() || terminal.grid.all_dirty {
            result.messages.push(ServerMessage::CellUpdate {
                window_id,
                dirty_cells,
                cursor,
                modes: terminal.window_modes(),
            });
        }
        result.messages.extend(
            atlas_updates
                .into_iter()
                .map(|glyph| ServerMessage::AtlasUpdate { glyph }),
        );
        if window.last_kitty_generation != terminal.kitty_generation() {
            result.messages.push(ServerMessage::KittyImageState {
                window_id,
                generation: terminal.kitty_generation(),
                images: terminal
                    .kitty_images()
                    .iter()
                    .map(|image| KittyImageData {
                        id: image.id,
                        width: image.width,
                        height: image.height,
                        data: image.data.clone(),
                    })
                    .collect(),
                placements: terminal
                    .kitty_placements()
                    .iter()
                    .map(|placement| KittyImagePlacement {
                        image_id: placement.image_id,
                        col: placement.col as u16,
                        row: placement.row as u16,
                        cols: placement.cols as u16,
                        rows: placement.rows as u16,
                    })
                    .collect(),
            });
            window.last_kitty_generation = terminal.kitty_generation();
        }
        if let Some(title) = terminal.take_title() {
            result
                .messages
                .push(ServerMessage::SetTitle { window_id, title });
        }
        if terminal.take_bell() {
            result.messages.push(ServerMessage::Bell { window_id });
        }
        if let Some(text) = terminal.take_osc52_clipboard() {
            result
                .messages
                .push(ServerMessage::CopyToClipboard { window_id, text });
        }
        if let Some(bytes) = terminal.drain_responses() {
            result
                .io_actions
                .push(ServerIoAction::Write { window_id, bytes });
        }
        terminal.grid.clear_dirty();
        result
    }
}

fn atlas_updates_from_dirty_cells(
    atlas: &mut GlyphAtlas,
    dirty_cells: &[DirtyCell],
    sent_glyphs: &mut HashSet<ProtocolGlyphKey>,
) -> Vec<GlyphBitmap> {
    let mut seen = BTreeSet::new();
    let mut updates = Vec::new();

    for cell in dirty_cells {
        let key = (cell.ch, cell.grapheme.clone());
        if !seen.insert(key.clone()) {
            continue;
        }

        let cells = if (cell.flags & crate::grid::FLAG_WIDE) != 0 {
            2
        } else {
            1
        };
        if let Some(grapheme) = key.1 {
            let protocol_key = ProtocolGlyphKey::Grapheme(grapheme.clone().into_boxed_str());
            if sent_glyphs.contains(&protocol_key) {
                continue;
            }
            if !atlas.ensure_grapheme(&grapheme) {
                continue;
            }
            let Some(glyph) = atlas.get_grapheme_glyph(&grapheme) else {
                continue;
            };
            updates.push(GlyphBitmap {
                glyph_id: cell.ch,
                grapheme: Some(grapheme),
                width: glyph.width as u16,
                height: glyph.height as u16,
                bearing_x: glyph.bearing_x as i16,
                bearing_y: glyph.bearing_y as i16,
                cells,
                is_color: matches!(glyph.format, GlyphFormat::Rgba),
                pixels: glyph.pixels.to_vec(),
            });
            sent_glyphs.insert(protocol_key);
            continue;
        }

        let protocol_key = ProtocolGlyphKey::Codepoint(cell.ch);
        if sent_glyphs.contains(&protocol_key) {
            continue;
        }
        if !atlas.ensure_glyph(cell.ch) {
            continue;
        }
        let Some(glyph) = atlas.get_glyph(cell.ch) else {
            continue;
        };
        updates.push(GlyphBitmap {
            glyph_id: cell.ch,
            grapheme: None,
            width: glyph.width as u16,
            height: glyph.height as u16,
            bearing_x: glyph.bearing_x as i16,
            bearing_y: glyph.bearing_y as i16,
            cells,
            is_color: matches!(glyph.format, GlyphFormat::Rgba),
            pixels: glyph.pixels.to_vec(),
        });
        sent_glyphs.insert(protocol_key);
    }

    updates
}

fn encode_mouse_event(terminal: &Terminal, event: &MouseEvent) -> Option<Vec<u8>> {
    let button = match event.button {
        crate::protocol::MouseButton::Left => 0,
        crate::protocol::MouseButton::Middle => 1,
        crate::protocol::MouseButton::Right => 2,
        crate::protocol::MouseButton::None => 0,
    };
    match event.kind {
        crate::protocol::MouseEventKind::Press => {
            terminal.encode_mouse(button, event.col as usize, event.row as usize, true)
        }
        crate::protocol::MouseEventKind::Release => {
            terminal.encode_mouse(button, event.col as usize, event.row as usize, false)
        }
        crate::protocol::MouseEventKind::ScrollUp => {
            terminal.encode_mouse_scroll(true, event.col as usize, event.row as usize)
        }
        crate::protocol::MouseEventKind::ScrollDown => {
            terminal.encode_mouse_scroll(false, event.col as usize, event.row as usize)
        }
        crate::protocol::MouseEventKind::Move => None,
    }
}

fn cursor_state_from_terminal(terminal: &Terminal) -> Option<CursorState> {
    if !terminal.cursor_visible || terminal.grid.scroll_offset != 0 {
        return None;
    }
    let (col, row) = terminal.grid.cursor_pos();
    Some(CursorState {
        row: row as u16,
        col: col as u16,
        style: terminal.cursor_style as u8,
        visible: true,
    })
}

fn dirty_cells_from_terminal(terminal: &Terminal) -> Vec<DirtyCell> {
    let grid = &terminal.grid;
    let mut out = Vec::new();
    for row in 0..grid.rows {
        for col in 0..grid.cols {
            if !grid.is_cell_dirty(row, col) {
                continue;
            }
            let cell = grid.cell_at(row, col);
            out.push(DirtyCell {
                row: row as u16,
                col: col as u16,
                ch: cell.ch,
                grapheme: grid.cell_grapheme_at(row, col).map(ToString::to_string),
                fg: cell.fg,
                bg: cell.bg,
                underline_color: cell.underline_color,
                hyperlink_id: cell.hyperlink_id,
                attrs: cell.attrs,
                flags: cell.flags,
                underline_style: cell.underline_style as u8,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_server() -> ServerCore {
        ServerCore::new_with_style(
            10_000,
            "JetBrainsMono Nerd Font Light".to_string(),
            11.0,
            "test-build".to_string(),
        )
    }

    fn created_window_id(message: ServerMessage) -> WindowId {
        match message {
            ServerMessage::WindowCreated { window_id, .. } => window_id,
            other => panic!("expected WindowCreated, got {other:?}"),
        }
    }

    #[test]
    fn server_core_creates_and_closes_windows() {
        let mut server = test_server();
        let window_id = created_window_id(server.create_window(80, 24, 96));

        assert!(server.has_window(window_id));
        assert_eq!(
            server.close_window(window_id, Some(0)),
            Some(ServerMessage::WindowClosed {
                window_id,
                exit_code: Some(0),
            })
        );
        assert!(!server.has_window(window_id));
    }

    #[test]
    fn server_core_uses_configured_scrollback_limit_for_new_windows() {
        let mut server = ServerCore::new_with_style(
            0,
            "JetBrainsMono Nerd Font Light".to_string(),
            11.0,
            "test-build".to_string(),
        );
        let window_id = created_window_id(server.create_window(4, 2, 96));
        let window = server.windows.get(&window_id).expect("window should exist");

        assert_eq!(window.terminal.scrollback_limit(), 0);

        let updates = server
            .process_output(window_id, b"abcdefghij")
            .expect("window should exist");
        assert!(!updates.messages.is_empty());

        let window = server.windows.get(&window_id).expect("window should exist");
        assert_eq!(window.terminal.grid.scrollback_len(), 0);
    }

    #[test]
    fn server_core_emits_dirty_cells_and_cursor_updates() {
        let mut server = test_server();
        let window_id = created_window_id(server.create_window(8, 2, 96));

        let updates = server
            .process_output(window_id, b"hi")
            .expect("window should exist");

        let cell_update = updates
            .messages
            .iter()
            .find_map(|message| match message {
                ServerMessage::CellUpdate {
                    window_id: id,
                    dirty_cells,
                    cursor,
                    ..
                } if *id == window_id => Some((dirty_cells, cursor)),
                _ => None,
            })
            .expect("cell update should be emitted");

        assert!(cell_update.0.iter().any(|cell| cell.ch == 'h' as u32));
        assert!(cell_update.0.iter().any(|cell| cell.ch == 'i' as u32));
        assert_eq!(
            cell_update.1,
            &Some(CursorState {
                row: 0,
                col: 2,
                style: 0,
                visible: true,
            })
        );
    }

    #[test]
    fn server_core_emits_title_bell_and_clipboard_side_effects() {
        let mut server = test_server();
        let window_id = created_window_id(server.create_window(8, 2, 96));

        let updates = server
            .process_output(
                window_id,
                b"\x07\x1b]0;handterm server\x07\x1b]52;c;Zm9v\x07",
            )
            .expect("window should exist");

        assert!(updates.messages.iter().any(|message| {
            matches!(
                message,
                ServerMessage::SetTitle { window_id: id, title }
                if *id == window_id && title == "handterm server"
            )
        }));
        assert!(updates.messages.iter().any(|message| {
            matches!(message, ServerMessage::Bell { window_id: id } if *id == window_id)
        }));
        assert!(updates.messages.iter().any(|message| {
            matches!(
                message,
                ServerMessage::CopyToClipboard { window_id: id, text }
                if *id == window_id && text == b"Zm9v"
            )
        }));
    }

    #[test]
    fn resize_window_marks_full_snapshot_dirty() {
        let mut server = test_server();
        let window_id = created_window_id(server.create_window(4, 2, 96));
        server.process_output(window_id, b"ab");

        let updates = server
            .resize_window(window_id, 6, 3)
            .expect("window should exist");

        let dirty_count = updates
            .iter()
            .find_map(|message| match message {
                ServerMessage::CellUpdate { dirty_cells, .. } => Some(dirty_cells.len()),
                _ => None,
            })
            .expect("resize should emit cell update");
        assert_eq!(dirty_count, 18);
    }

    #[test]
    fn handle_client_message_emits_spawn_and_forward_actions() {
        let mut server = test_server();
        let created = server
            .handle_client_message(ClientMessage::NewWindow {
                cols: 80,
                rows: 24,
                dpi: 96,
            })
            .expect("new window should succeed");

        let window_id = match created.messages.as_slice() {
            [ServerMessage::WindowCreated { window_id, .. }] => *window_id,
            other => panic!("expected one WindowCreated message, got {other:?}"),
        };
        assert_eq!(
            created.io_actions,
            vec![ServerIoAction::SpawnWindow {
                window_id,
                cols: 80,
                rows: 24,
            }]
        );

        let key = server
            .handle_client_message(ClientMessage::KeyInput {
                window_id,
                event: crate::protocol::KeyEvent {
                    kind: crate::protocol::KeyEventKind::Press,
                    bytes: b"ls\n".to_vec(),
                    text: Some("ls".to_string()),
                    modifiers: 0,
                },
            })
            .expect("key input should succeed");
        assert!(key.messages.is_empty());
        assert_eq!(
            key.io_actions,
            vec![ServerIoAction::Write {
                window_id,
                bytes: b"ls\n".to_vec(),
            }]
        );
    }

    #[test]
    fn handle_client_message_resize_and_close_emit_messages_and_actions() {
        let mut server = test_server();
        let window_id = created_window_id(server.create_window(4, 2, 96));

        let resized = server
            .handle_client_message(ClientMessage::Resize {
                window_id,
                cols: 6,
                rows: 3,
            })
            .expect("resize should succeed");
        assert!(resized.messages.iter().any(|message| {
            matches!(message, ServerMessage::WindowResized { window_id: id, cols: 6, rows: 3, .. } if *id == window_id)
        }));
        assert!(resized.messages.iter().any(|message| {
            matches!(message, ServerMessage::CellUpdate { window_id: id, dirty_cells, .. } if *id == window_id && dirty_cells.len() == 18)
        }));
        assert_eq!(
            resized.io_actions,
            vec![ServerIoAction::Resize {
                window_id,
                cols: 6,
                rows: 3,
            }]
        );

        let closed = server
            .handle_client_message(ClientMessage::CloseWindow { window_id })
            .expect("close should succeed");
        assert_eq!(
            closed.messages,
            vec![ServerMessage::WindowClosed {
                window_id,
                exit_code: None,
            }]
        );
        assert_eq!(closed.io_actions, vec![ServerIoAction::Close { window_id }]);
    }

    #[test]
    fn handle_client_message_rejects_unknown_windows() {
        let mut server = test_server();
        let err = server
            .handle_client_message(ClientMessage::Paste {
                window_id: 99,
                text: b"pwd\n".to_vec(),
            })
            .expect_err("unknown window should fail");
        assert_eq!(err, ServerError::UnknownWindow(99));
    }

    #[test]
    fn process_output_emits_terminal_response_io_actions() {
        let mut server = test_server();
        let window_id = created_window_id(server.create_window(8, 2, 96));

        let updates = server
            .process_output(window_id, b"\x1b[6n")
            .expect("window should exist");

        assert_eq!(
            updates.io_actions,
            vec![ServerIoAction::Write {
                window_id,
                bytes: b"\x1b[1;1R".to_vec(),
            }]
        );
    }

    #[test]
    fn handle_client_mouse_input_encodes_pty_bytes() {
        let mut server = test_server();
        let window_id = created_window_id(server.create_window(8, 2, 96));
        let window = server
            .windows
            .get_mut(&window_id)
            .expect("window should exist");
        window.terminal.process(b"\x1b[?1006h\x1b[?1000h");

        let result = server
            .handle_client_message(ClientMessage::MouseInput {
                window_id,
                event: MouseEvent {
                    kind: crate::protocol::MouseEventKind::Press,
                    button: crate::protocol::MouseButton::Left,
                    col: 4,
                    row: 2,
                    modifiers: 0,
                },
            })
            .expect("mouse input should encode");

        assert_eq!(
            result.io_actions,
            vec![ServerIoAction::Write {
                window_id,
                bytes: b"\x1b[<0;5;3M".to_vec(),
            }]
        );
    }

    #[test]
    fn handle_client_ping_replies_with_current_build_id() {
        let mut server = test_server();
        let result = server
            .handle_client_message(ClientMessage::Ping {
                build_id: "old-build".to_string(),
            })
            .expect("ping should succeed");

        assert_eq!(
            result.messages,
            vec![ServerMessage::Pong {
                build_id: "test-build".to_string(),
            }]
        );
        assert!(result.io_actions.is_empty());
    }

    #[test]
    fn server_core_emits_kitty_image_state_when_generation_changes() {
        let mut server = test_server();
        let window_id = created_window_id(server.create_window(8, 2, 96));

        let updates = server
            .process_output(
                window_id,
                b"\x1b_Ga=T,i=7,f=32,s=1,v=1,c=1,r=1;/wAA/w==\x1b\\",
            )
            .expect("window should exist");

        let kitty = updates
            .messages
            .iter()
            .find_map(|message| match message {
                ServerMessage::KittyImageState {
                    window_id: id,
                    generation,
                    images,
                    placements,
                } if *id == window_id => Some((generation, images, placements)),
                _ => None,
            })
            .expect("kitty state should be emitted");

        assert_eq!(*kitty.0, 1);
        assert_eq!(kitty.1.len(), 1);
        assert_eq!(kitty.1[0].id, 7);
        assert_eq!(kitty.2.len(), 1);
        assert_eq!(kitty.2[0].image_id, 7);
    }

    #[test]
    fn server_core_only_sends_new_atlas_updates_once_per_window() {
        let mut server = test_server();
        let window_id = created_window_id(server.create_window(8, 2, 96));

        let first = server
            .process_output(window_id, b"aaaa")
            .expect("window should exist");
        let first_atlas_updates = first
            .messages
            .iter()
            .filter(|message| matches!(message, ServerMessage::AtlasUpdate { .. }))
            .count();
        assert!(
            first_atlas_updates >= 1,
            "first glyph upload batch should not be empty"
        );

        let second = server
            .process_output(window_id, b"a")
            .expect("window should exist");
        let second_atlas_updates = second
            .messages
            .iter()
            .filter(|message| matches!(message, ServerMessage::AtlasUpdate { .. }))
            .count();
        assert_eq!(
            second_atlas_updates, 0,
            "already-uploaded glyphs should not be resent"
        );
    }
}
