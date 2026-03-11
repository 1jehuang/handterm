use crate::font::GlyphAtlas;
use crate::grid::{Cell, Grid};
use crate::protocol::{CursorState, ServerMessage, WindowId, WindowModes};
use crate::terminal::{CursorStyle, KittyImage, KittyPlacement, MouseMode, TerminalView};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AppliedServerEffects {
    pub title: Option<String>,
    pub clipboard: Option<Vec<u8>>,
    pub bell: bool,
    pub closed: Option<Option<i32>>,
}

pub struct RemoteTerminalState {
    pub grid: Grid,
    alt_grid: Option<Grid>,
    pub cols: u16,
    pub rows: u16,
    pub cursor_visible: bool,
    pub cursor_style: CursorStyle,
    pub application_cursor_keys: bool,
    pub mouse_mode: MouseMode,
    mode_bracketed_paste: bool,
    mode_focus_events: bool,
    mode_alternate_scroll: bool,
    kitty_keyboard_flags: u8,
    kitty_images: Vec<KittyImage>,
    kitty_placements: Vec<KittyPlacement>,
    kitty_generation: u64,
}

impl RemoteTerminalState {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            grid: Grid::new_with_scrollback(cols, rows, [0xcd, 0xd6, 0xf4], [0x00, 0x00, 0x00], 0),
            alt_grid: None,
            cols,
            rows,
            cursor_visible: true,
            cursor_style: CursorStyle::Block,
            application_cursor_keys: false,
            mouse_mode: MouseMode::Off,
            mode_bracketed_paste: false,
            mode_focus_events: false,
            mode_alternate_scroll: false,
            kitty_keyboard_flags: 0,
            kitty_images: Vec::new(),
            kitty_placements: Vec::new(),
            kitty_generation: 0,
        }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
        self.grid.resize(cols, rows);
        if let Some(alt) = self.alt_grid.as_mut() {
            alt.resize(cols, rows);
        }
    }

    pub fn bracketed_paste_mode(&self) -> bool {
        self.mode_bracketed_paste
    }

    pub fn focus_events_mode(&self) -> bool {
        self.mode_focus_events
    }

    pub fn alternate_scroll_mode(&self) -> bool {
        self.mode_alternate_scroll
    }

    pub fn kitty_keyboard_flags(&self) -> u8 {
        self.kitty_keyboard_flags
    }

    pub fn in_alt_screen(&self) -> bool {
        self.alt_grid.is_some()
    }

    pub fn apply_server_message(&mut self, message: &ServerMessage) -> AppliedServerEffects {
        let mut effects = AppliedServerEffects::default();

        match message {
            ServerMessage::Pong { .. } => {}
            ServerMessage::WindowCreated { cols, rows, modes, .. }
            | ServerMessage::WindowResized { cols, rows, modes, .. } => {
                self.resize(*cols, *rows);
                self.apply_window_modes(*modes);
                self.grid.mark_all_dirty();
            }
            ServerMessage::CellUpdate {
                dirty_cells,
                cursor,
                modes,
                ..
            } => {
                self.apply_window_modes(*modes);
                for dirty in dirty_cells {
                    self.apply_dirty_cell(dirty);
                }
                self.apply_cursor_state(cursor.as_ref());
            }
            ServerMessage::SetTitle { title, .. } => {
                effects.title = Some(title.clone());
            }
            ServerMessage::Bell { .. } => {
                effects.bell = true;
            }
            ServerMessage::CopyToClipboard { text, .. } => {
                effects.clipboard = Some(text.clone());
            }
            ServerMessage::WindowClosed { exit_code, .. } => {
                effects.closed = Some(*exit_code);
            }
            ServerMessage::KittyImageState {
                generation,
                images,
                placements,
                ..
            } => {
                self.kitty_images = images
                    .iter()
                    .map(|image| KittyImage {
                        id: image.id,
                        width: image.width,
                        height: image.height,
                        data: image.data.clone(),
                    })
                    .collect();
                self.kitty_placements = placements
                    .iter()
                    .map(|placement| KittyPlacement {
                        image_id: placement.image_id,
                        col: placement.col as usize,
                        row: placement.row as usize,
                        cols: placement.cols as usize,
                        rows: placement.rows as usize,
                    })
                    .collect();
                self.kitty_generation = *generation;
                self.grid.mark_all_dirty();
            }
            ServerMessage::AtlasUpdate { .. } => {}
        }

        effects
    }

    fn apply_dirty_cell(&mut self, dirty: &crate::protocol::DirtyCell) {
        let underline_style = match dirty.underline_style {
            1 => crate::grid::UnderlineStyle::Single,
            2 => crate::grid::UnderlineStyle::Double,
            3 => crate::grid::UnderlineStyle::Curly,
            4 => crate::grid::UnderlineStyle::Dotted,
            5 => crate::grid::UnderlineStyle::Dashed,
            _ => crate::grid::UnderlineStyle::None,
        };

        let grapheme = dirty.grapheme.clone().map(Into::into);
        self.grid.set_cell_with_grapheme(
            dirty.row as usize,
            dirty.col as usize,
            Cell::from_snapshot(crate::grid::CellSnapshot {
                ch: dirty.ch,
                grapheme: grapheme.clone(),
                fg: dirty.fg,
                bg: dirty.bg,
                underline_color: dirty.underline_color,
                hyperlink_id: dirty.hyperlink_id,
                attrs: dirty.attrs,
                flags: dirty.flags,
                underline_style,
            }),
            grapheme,
        );
    }

    fn apply_cursor_state(&mut self, cursor: Option<&CursorState>) {
        match cursor {
            Some(cursor) => {
                self.grid.set_cursor(cursor.row as usize, cursor.col as usize);
                self.cursor_visible = cursor.visible;
                self.cursor_style = match cursor.style {
                    1 => CursorStyle::Underline,
                    2 => CursorStyle::Bar,
                    _ => CursorStyle::Block,
                };
            }
            None => {
                self.cursor_visible = false;
            }
        }
    }

    fn apply_window_modes(&mut self, modes: WindowModes) {
        self.mode_bracketed_paste = modes.bracketed_paste;
        self.mode_focus_events = modes.focus_events;
        self.mode_alternate_scroll = modes.alternate_scroll;
        self.application_cursor_keys = modes.application_cursor_keys;
        if modes.in_alt_screen {
            self.enter_alt_screen();
        } else {
            self.leave_alt_screen();
        }
        self.mouse_mode = match modes.mouse_mode {
            1 => MouseMode::X10,
            2 => MouseMode::Normal,
            3 => MouseMode::ButtonEvent,
            4 => MouseMode::AnyEvent,
            _ => MouseMode::Off,
        };
        self.kitty_keyboard_flags = modes.kitty_keyboard_flags;
    }

    fn enter_alt_screen(&mut self) {
        if self.alt_grid.is_some() {
            return;
        }
        let main = std::mem::replace(
            &mut self.grid,
            Grid::new_with_scrollback(self.cols, self.rows, [0xcd, 0xd6, 0xf4], [0x00, 0x00, 0x00], 0),
        );
        self.alt_grid = Some(main);
    }

    fn leave_alt_screen(&mut self) {
        if let Some(main) = self.alt_grid.take() {
            self.grid = main;
        }
    }
}

impl TerminalView for RemoteTerminalState {
    fn grid(&self) -> &Grid {
        &self.grid
    }

    fn grid_mut(&mut self) -> &mut Grid {
        &mut self.grid
    }

    fn cols(&self) -> u16 {
        self.cols
    }

    fn rows(&self) -> u16 {
        self.rows
    }

    fn cursor_visible(&self) -> bool {
        self.cursor_visible
    }

    fn cursor_style(&self) -> CursorStyle {
        self.cursor_style
    }

    fn kitty_generation(&self) -> u64 {
        self.kitty_generation
    }

    fn kitty_placements(&self) -> &[KittyPlacement] {
        &self.kitty_placements
    }

    fn kitty_image(&self, id: u32) -> Option<&KittyImage> {
        self.kitty_images.iter().find(|image| image.id == id)
    }
}

pub fn terminal_size_for_pixels(width: u32, height: u32, atlas: &GlyphAtlas) -> (u16, u16) {
    let cols = (width as usize / atlas.cell_width.max(1)) as u16;
    let rows = (height as usize / atlas.cell_height.max(1)) as u16;
    (cols.max(1), rows.max(1))
}

pub fn modifier_bits(modifiers: winit::keyboard::ModifiersState) -> u8 {
    let mut bits = 0;
    if modifiers.shift_key() {
        bits |= 1;
    }
    if modifiers.control_key() {
        bits |= 2;
    }
    if modifiers.alt_key() {
        bits |= 4;
    }
    if modifiers.super_key() {
        bits |= 8;
    }
    bits
}

pub fn should_apply_message(window_id: Option<WindowId>, message: &ServerMessage) -> bool {
    match (window_id, message_window_id(message)) {
        (_, None) => true,
        (None, Some(_)) => matches!(message, ServerMessage::WindowCreated { .. }),
        (Some(expected), Some(found)) => expected == found,
    }
}

    pub fn message_window_id(message: &ServerMessage) -> Option<WindowId> {
    match message {
        ServerMessage::Pong { .. } => None,
        ServerMessage::WindowCreated { window_id, .. }
        | ServerMessage::WindowResized { window_id, .. }
        | ServerMessage::CellUpdate { window_id, .. }
        | ServerMessage::SetTitle { window_id, .. }
        | ServerMessage::Bell { window_id }
        | ServerMessage::CopyToClipboard { window_id, .. }
        | ServerMessage::WindowClosed { window_id, .. }
        | ServerMessage::KittyImageState { window_id, .. } => Some(*window_id),
        ServerMessage::AtlasUpdate { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{CellMetrics, KittyImageData, KittyImagePlacement, WindowModes};

    fn sample_metrics() -> CellMetrics {
        CellMetrics {
            cell_width: 9,
            cell_height: 18,
            baseline: 14,
        }
    }

    #[test]
    fn window_messages_are_filtered_to_the_active_remote_window() {
        assert!(should_apply_message(
            None,
            &ServerMessage::WindowCreated {
                window_id: 7,
                cols: 80,
                rows: 24,
                metrics: sample_metrics(),
                modes: WindowModes::default(),
            }
        ));
        assert!(!should_apply_message(
            None,
            &ServerMessage::SetTitle {
                window_id: 7,
                title: "x".to_string(),
            }
        ));
        assert!(should_apply_message(
            Some(7),
            &ServerMessage::CellUpdate {
                window_id: 7,
                dirty_cells: Vec::new(),
                cursor: None,
                modes: WindowModes::default(),
            }
        ));
        assert!(!should_apply_message(
            Some(7),
            &ServerMessage::CellUpdate {
                window_id: 9,
                dirty_cells: Vec::new(),
                cursor: None,
                modes: WindowModes::default(),
            }
        ));
    }

    #[test]
    fn modifier_bits_match_expected_protocol_mask() {
        let modifiers = winit::keyboard::ModifiersState::SHIFT
            | winit::keyboard::ModifiersState::CONTROL
            | winit::keyboard::ModifiersState::ALT;
        assert_eq!(modifier_bits(modifiers), 0b0111);
    }

    #[test]
    fn remote_terminal_tracks_alt_screen_mode() {
        let mut terminal = RemoteTerminalState::new(4, 2);
        terminal.apply_server_message(&ServerMessage::WindowCreated {
            window_id: 1,
            cols: 4,
            rows: 2,
            metrics: sample_metrics(),
            modes: WindowModes {
                in_alt_screen: true,
                ..WindowModes::default()
            },
        });

        assert!(terminal.in_alt_screen());

        terminal.apply_server_message(&ServerMessage::WindowResized {
            window_id: 1,
            cols: 4,
            rows: 2,
            metrics: sample_metrics(),
            modes: WindowModes::default(),
        });

        assert!(!terminal.in_alt_screen());
    }

    #[test]
    fn remote_terminal_applies_kitty_image_state() {
        let mut terminal = RemoteTerminalState::new(4, 2);
        terminal.apply_server_message(&ServerMessage::KittyImageState {
            window_id: 1,
            generation: 2,
            images: vec![KittyImageData {
                id: 5,
                width: 1,
                height: 1,
                data: vec![255, 0, 0, 255],
            }],
            placements: vec![KittyImagePlacement {
                image_id: 5,
                col: 1,
                row: 0,
                cols: 1,
                rows: 1,
            }],
        });

        assert_eq!(terminal.kitty_generation(), 2);
        assert_eq!(terminal.kitty_placements().len(), 1);
        assert_eq!(terminal.kitty_placements()[0].image_id, 5);
        assert_eq!(terminal.kitty_image(5).expect("image should exist").data, vec![255, 0, 0, 255]);
    }
}
