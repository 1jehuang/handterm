use crate::grid::Grid;
use crate::parser::{Action, Parser};

fn dec_special_to_unicode(b: u8) -> u32 {
    match b {
        b'j' => 0x2518, // ┘
        b'k' => 0x2510, // ┐
        b'l' => 0x250C, // ┌
        b'm' => 0x2514, // └
        b'n' => 0x253C, // ┼
        b'q' => 0x2500, // ─
        b't' => 0x251C, // ├
        b'u' => 0x2524, // ┤
        b'v' => 0x2534, // ┴
        b'w' => 0x252C, // ┬
        b'x' => 0x2502, // │
        b'a' => 0x2592, // ▒
        b'`' => 0x25C6, // ◆
        _ => b as u32,
    }
}

pub struct Terminal {
    pub grid: Grid,
    alt_grid: Option<Grid>,
    parser: Parser,
    pub cols: u16,
    pub rows: u16,
    pub cursor_visible: bool,
    pub title: Option<String>,
    response_buf: Vec<u8>,
    saved_cursor: Option<(usize, usize)>,
    mode_bracketed_paste: bool,
    mode_focus_events: bool,
    mode_alternate_scroll: bool,
    pub application_cursor_keys: bool,
    pub mouse_mode: MouseMode,
    pub mouse_encoding: MouseEncoding,
    pub cursor_style: CursorStyle,
    osc52_clipboard: Option<Vec<u8>>,
    pub bell: bool,
    charset_g0: Charset,
    charset_g1: Charset,
    active_charset: u8,
    kitty_images: Vec<KittyImage>,
    pub kitty_placements: Vec<KittyPlacement>,
    kitty_payload_buf: Vec<u8>,
    kitty_pending_id: u32,
    kitty_pending_fmt: u32,
    kitty_pending_width: u32,
    kitty_pending_height: u32,
    kitty_more_chunks: bool,
    kitty_generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Charset {
    Ascii,
    DecSpecialGraphics,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorStyle {
    Block,
    Underline,
    Bar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseMode {
    Off,
    X10,
    Normal,
    ButtonEvent,
    AnyEvent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEncoding {
    X10,
    Utf8,
    Sgr,
}

#[derive(Debug, Clone)]
pub struct KittyImage {
    pub id: u32,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct KittyPlacement {
    pub image_id: u32,
    pub col: usize,
    pub row: usize,
    pub cols: usize,
    pub rows: usize,
}

impl Terminal {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            grid: Grid::new(cols, rows, [0xcd, 0xd6, 0xf4], [0x00, 0x00, 0x00]),
            alt_grid: None,
            parser: Parser::new(),
            cols,
            rows,
            cursor_visible: true,
            title: None,
            response_buf: Vec::new(),
            saved_cursor: None,
            mode_bracketed_paste: false,
            mode_focus_events: false,
            mode_alternate_scroll: false,
            application_cursor_keys: false,
            mouse_mode: MouseMode::Off,
            mouse_encoding: MouseEncoding::X10,
            cursor_style: CursorStyle::Block,
            osc52_clipboard: None,
            bell: false,
            charset_g0: Charset::Ascii,
            charset_g1: Charset::Ascii,
            active_charset: 0,
            kitty_images: Vec::new(),
            kitty_placements: Vec::new(),
            kitty_payload_buf: Vec::new(),
            kitty_pending_id: 0,
            kitty_pending_fmt: 0,
            kitty_pending_width: 0,
            kitty_pending_height: 0,
            kitty_more_chunks: false,
            kitty_generation: 0,
        }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
        self.grid.resize(cols, rows);
        if let Some(ref mut alt) = self.alt_grid {
            alt.resize(cols, rows);
        }
    }

    pub fn drain_responses(&mut self) -> Option<Vec<u8>> {
        if self.response_buf.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut self.response_buf))
        }
    }

    pub fn take_title(&mut self) -> Option<String> {
        self.title.take()
    }

    pub fn bracketed_paste_mode(&self) -> bool {
        self.mode_bracketed_paste
    }

    pub fn take_osc52_clipboard(&mut self) -> Option<Vec<u8>> {
        self.osc52_clipboard.take()
    }

    pub fn focus_events_mode(&self) -> bool {
        self.mode_focus_events
    }

    pub fn alternate_scroll_mode(&self) -> bool {
        self.mode_alternate_scroll
    }

    pub fn in_alt_screen(&self) -> bool {
        self.alt_grid.is_some()
    }

    pub fn encode_mouse(&self, button: u8, col: usize, row: usize, pressed: bool) -> Option<Vec<u8>> {
        if self.mouse_mode == MouseMode::Off {
            return None;
        }
        let cx = col + 1;
        let cy = row + 1;

        match self.mouse_encoding {
            MouseEncoding::Sgr => {
                let ch = if pressed { 'M' } else { 'm' };
                Some(format!("\x1b[<{};{};{}{}", button, cx, cy, ch).into_bytes())
            }
            MouseEncoding::X10 | MouseEncoding::Utf8 => {
                if !pressed && self.mouse_mode != MouseMode::X10 {
                    let cb = 3 + 32;
                    if cx <= 223 && cy <= 223 {
                        Some(vec![0x1b, b'[', b'M', cb, (cx as u8) + 32, (cy as u8) + 32])
                    } else {
                        None
                    }
                } else if pressed {
                    let cb = button + 32;
                    if cx <= 223 && cy <= 223 {
                        Some(vec![0x1b, b'[', b'M', cb, (cx as u8) + 32, (cy as u8) + 32])
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
        }
    }

    pub fn encode_mouse_scroll(&self, up: bool, col: usize, row: usize) -> Option<Vec<u8>> {
        if self.mouse_mode == MouseMode::Off {
            return None;
        }
        let button = if up { 64 } else { 65 };
        let cx = col + 1;
        let cy = row + 1;

        match self.mouse_encoding {
            MouseEncoding::Sgr => {
                Some(format!("\x1b[<{};{};{}M", button, cx, cy).into_bytes())
            }
            MouseEncoding::X10 | MouseEncoding::Utf8 => {
                let cb = button + 32;
                if cx <= 223 && cy <= 223 {
                    Some(vec![0x1b, b'[', b'M', cb, (cx as u8) + 32, (cy as u8) + 32])
                } else {
                    None
                }
            }
        }
    }

    pub fn process(&mut self, data: &[u8]) {
        let use_line_drawing = self.active_charset_is_dec_special();
        let len = data.len();
        let mut i = 0;

        while i < len {
            if self.parser.is_ground() && !use_line_drawing {
                let run_start = i;
                while i < len {
                    let b = data[i];
                    if b.wrapping_sub(0x20) < 0x5f {
                        i += 1;
                    } else {
                        break;
                    }
                }
                if i > run_start {
                    self.grid.write_bytes(&data[run_start..i]);
                    continue;
                }
            }

            let byte = data[i];
            i += 1;
            let action = self.parser.advance(byte);

            match action {
                Action::Print(_) => {
                    let run_start = i - 1;
                    while i < len {
                        let next_action = self.parser.advance(data[i]);
                        match next_action {
                            Action::Print(_) => { i += 1; }
                            _ => {
                                if use_line_drawing {
                                    self.write_bytes_translated(&data[run_start..i]);
                                } else {
                                    self.grid.write_bytes(&data[run_start..i]);
                                }
                                i += 1;
                                self.handle_action(next_action);
                                break;
                            }
                        }
                    }
                    if i >= len {
                        if use_line_drawing {
                            self.write_bytes_translated(&data[run_start..i]);
                        } else {
                            self.grid.write_bytes(&data[run_start..i]);
                        }
                    }
                }
                _ => {
                    self.handle_action(action);
                }
            }
        }
    }

    fn active_charset_is_dec_special(&self) -> bool {
        let cs = if self.active_charset == 0 {
            self.charset_g0
        } else {
            self.charset_g1
        };
        cs == Charset::DecSpecialGraphics
    }

    fn write_bytes_translated(&mut self, bytes: &[u8]) {
        for &b in bytes {
            let ch = dec_special_to_unicode(b);
            if ch >= 0x80 {
                self.grid.put_char(ch);
            } else {
                self.grid.write_bytes(&[b]);
            }
        }
    }

    fn handle_action(&mut self, action: Action) {
        match action {
            Action::Execute(byte) => self.execute(byte),
            Action::CsiDispatch {
                params_count: _,
                intermediate,
                final_byte,
            } => self.csi_dispatch(intermediate, final_byte),
            Action::EscDispatch {
                intermediate,
                final_byte,
            } => self.esc_dispatch(intermediate, final_byte),
            Action::OscDispatch(data) => self.osc_dispatch(&data),
            Action::ApcDispatch(data) => self.apc_dispatch(&data),
            Action::Print(_) | Action::Nop => {}
        }
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' | 0x0b | 0x0c => self.grid.line_feed(),
            b'\r' => self.grid.carriage_return(),
            b'\t' => self.grid.tab(),
            0x08 => self.grid.backspace(),
            0x07 => self.bell = true,
            0x0e => self.active_charset = 1,
            0x0f => self.active_charset = 0,
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, intermediate: u8, final_byte: u8) {
        let p = &self.parser;
        match (intermediate, final_byte) {
            (0, b'A') => self.grid.move_cursor_up(p.param(0, 1) as usize),
            (0, b'B') => self.grid.move_cursor_down(p.param(0, 1) as usize),
            (0, b'C') => self.grid.move_cursor_right(p.param(0, 1) as usize),
            (0, b'D') => self.grid.move_cursor_left(p.param(0, 1) as usize),
            (0, b'E') => {
                let n = p.param(0, 1) as usize;
                self.grid.move_cursor_down(n);
                self.grid.carriage_return();
            }
            (0, b'F') => {
                let n = p.param(0, 1) as usize;
                self.grid.move_cursor_up(n);
                self.grid.carriage_return();
            }
            (0, b'H') | (0, b'f') => {
                let row = p.param(0, 1).saturating_sub(1) as usize;
                let col = p.param(1, 1).saturating_sub(1) as usize;
                self.grid.set_cursor(row, col);
            }
            (0, b'J') => match p.param(0, 0) {
                0 => self.grid.erase_below(),
                1 => self.grid.erase_above(),
                2 | 3 => self.grid.erase_all(),
                _ => {}
            },
            (0, b'K') => match p.param(0, 0) {
                0 => self.grid.erase_line_right(),
                1 => self.grid.erase_line_left(),
                2 => self.grid.erase_line_all(),
                _ => {}
            },
            (0, b'm') => self.handle_sgr(),
            (0, b'L') => self.grid.insert_lines(p.param(0, 1) as usize),
            (0, b'M') => self.grid.delete_lines(p.param(0, 1) as usize),
            (0, b'@') => self.grid.insert_chars(p.param(0, 1) as usize),
            (0, b'P') => self.grid.delete_chars(p.param(0, 1) as usize),
            (0, b'X') => self.grid.erase_chars(p.param(0, 1) as usize),
            (0, b'd') => {
                let row = p.param(0, 1).saturating_sub(1) as usize;
                self.grid.set_cursor_row(row);
            }
            (0, b'G') | (0, b'`') => {
                let col = p.param(0, 1).saturating_sub(1) as usize;
                self.grid.set_cursor_col(col);
            }
            (0, b'S') => self.grid.scroll_up_n(p.param(0, 1) as usize),
            (0, b'T') => self.grid.scroll_down_n(p.param(0, 1) as usize),
            (0, b't') => {
                match p.param(0, 0) {
                    8 => {
                        let rows = p.param(1, 0);
                        let cols = p.param(2, 0);
                        if rows > 0 && cols > 0 {
                            // Window resize request - report current size
                        }
                    }
                    18 => {
                        let resp = format!("\x1b[8;{};{}t", self.rows, self.cols);
                        self.response_buf.extend_from_slice(resp.as_bytes());
                    }
                    _ => {}
                }
            }
            (0, b'r') => {
                let top = p.param(0, 1).saturating_sub(1) as usize;
                let bottom = p.param(1, self.rows) as usize;
                self.grid.set_scroll_region(top, bottom);
            }
            // DA1 - Device Attributes
            (0, b'c') | (b'>', b'c') => {
                if intermediate == b'>' {
                    // DA2: report VT220
                    self.response_buf.extend_from_slice(b"\x1b[>1;1;0c");
                } else {
                    // DA1: report VT220 with ANSI color
                    self.response_buf.extend_from_slice(b"\x1b[?62;22c");
                }
            }
            // DSR - Device Status Report
            (0, b'n') => {
                match p.param(0, 0) {
                    5 => {
                        // Status report: OK
                        self.response_buf.extend_from_slice(b"\x1b[0n");
                    }
                    6 => {
                        // Cursor position report
                        let (col, row) = self.grid.cursor_pos();
                        let resp = format!("\x1b[{};{}R", row + 1, col + 1);
                        self.response_buf.extend_from_slice(resp.as_bytes());
                    }
                    _ => {}
                }
            }
            // Kitty keyboard protocol query - respond with flags=0 (no enhancement)
            (b'?', b'u') => {
                self.response_buf.extend_from_slice(b"\x1b[?0u");
            }
            // Kitty keyboard protocol push/pop - silently accept
            (b'>', b'u') | (b'<', b'u') => {}
            // XTVERSION query
            (b'>', b'q') => {
                self.response_buf.extend_from_slice(b"\x1bP>|handterm(0.1)\x1b\\");
            }
            // Private mode set
            (b'?', b'h') => {
                let params: Vec<u16> = self.parser.params().to_vec();
                for param in &params {
                    match param {
                        1 => self.application_cursor_keys = true,
                        7 => self.grid.autowrap = true,
                        12 => {}   // Cursor blink
                        25 => self.cursor_visible = true,   // DECTCEM show cursor
                        47 | 1047 => self.enter_alt_screen(),
                        1049 => {
                            self.save_cursor();
                            self.enter_alt_screen();
                        }
                        2004 => self.mode_bracketed_paste = true,
                        1004 => self.mode_focus_events = true,
                        1007 => self.mode_alternate_scroll = true,
                        9 => self.mouse_mode = MouseMode::X10,
                        1000 => self.mouse_mode = MouseMode::Normal,
                        1002 => self.mouse_mode = MouseMode::ButtonEvent,
                        1003 => self.mouse_mode = MouseMode::AnyEvent,
                        1005 => self.mouse_encoding = MouseEncoding::Utf8,
                        1006 => self.mouse_encoding = MouseEncoding::Sgr,
                        _ => {}
                    }
                }
            }
            // Private mode reset
            (b'?', b'l') => {
                let params: Vec<u16> = self.parser.params().to_vec();
                for param in &params {
                    match param {
                        1 => self.application_cursor_keys = false,
                        7 => self.grid.autowrap = false,
                        12 => {}
                        25 => self.cursor_visible = false,  // DECTCEM hide cursor
                        47 | 1047 => self.leave_alt_screen(),
                        1049 => {
                            self.leave_alt_screen();
                            self.restore_cursor();
                        }
                        2004 => self.mode_bracketed_paste = false,
                        1004 => self.mode_focus_events = false,
                        1007 => self.mode_alternate_scroll = false,
                        9 | 1000 | 1002 | 1003 => self.mouse_mode = MouseMode::Off,
                        1005 | 1006 => self.mouse_encoding = MouseEncoding::X10,
                        _ => {}
                    }
                }
            }
            // Cursor save/restore (ANSI.SYS style)
            (0, b's') => self.save_cursor(),
            (0, b'u') => self.restore_cursor(),
            (b' ', b'q') => {
                match self.parser.param(0, 0) {
                    0 | 1 | 2 => self.cursor_style = CursorStyle::Block,
                    3 | 4 => self.cursor_style = CursorStyle::Underline,
                    5 | 6 => self.cursor_style = CursorStyle::Bar,
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn handle_sgr(&mut self) {
        let params = self.parser.params();
        if params.is_empty() {
            self.grid.reset_attrs();
            return;
        }

        let mut i = 0;
        while i < params.len() {
            match params[i] {
                0 => self.grid.reset_attrs(),
                1 => self.grid.set_bold(true),
                2 => self.grid.set_dim(true),
                3 => self.grid.set_italic(true),
                4 => {
                    if i + 1 < params.len() && params[i] == 4 {
                        match params[i + 1] {
                            0 => self.grid.set_underline_style(crate::grid::UnderlineStyle::None),
                            1 => self.grid.set_underline_style(crate::grid::UnderlineStyle::Single),
                            2 => self.grid.set_underline_style(crate::grid::UnderlineStyle::Double),
                            3 => self.grid.set_underline_style(crate::grid::UnderlineStyle::Curly),
                            4 => self.grid.set_underline_style(crate::grid::UnderlineStyle::Dotted),
                            5 => self.grid.set_underline_style(crate::grid::UnderlineStyle::Dashed),
                            _ => self.grid.set_underline_style(crate::grid::UnderlineStyle::Single),
                        }
                        i += 1;
                    } else {
                        self.grid.set_underline_style(crate::grid::UnderlineStyle::Single);
                    }
                }
                7 => self.grid.set_inverse(true),
                9 => self.grid.set_strikethrough(true),
                22 => {
                    self.grid.set_bold(false);
                    self.grid.set_dim(false);
                }
                23 => self.grid.set_italic(false),
                24 => self.grid.set_underline_style(crate::grid::UnderlineStyle::None),
                27 => self.grid.set_inverse(false),
                29 => self.grid.set_strikethrough(false),
                30..=37 => self.grid.set_fg((params[i] - 30) as u32),
                38 => {
                    if i + 1 < params.len() {
                        if params[i + 1] == 5 && i + 2 < params.len() {
                            self.grid.set_fg(params[i + 2] as u32);
                            i += 2;
                        } else if params[i + 1] == 2 && i + 4 < params.len() {
                            let r = params[i + 2] as u8;
                            let g = params[i + 3] as u8;
                            let b = params[i + 4] as u8;
                            self.grid.set_fg_rgb(r, g, b);
                            i += 4;
                        }
                    }
                }
                39 => self.grid.set_fg(0),
                40..=47 => self.grid.set_bg((params[i] - 40) as u32),
                48 => {
                    if i + 1 < params.len() {
                        if params[i + 1] == 5 && i + 2 < params.len() {
                            self.grid.set_bg(params[i + 2] as u32);
                            i += 2;
                        } else if params[i + 1] == 2 && i + 4 < params.len() {
                            let r = params[i + 2] as u8;
                            let g = params[i + 3] as u8;
                            let b = params[i + 4] as u8;
                            self.grid.set_bg_rgb(r, g, b);
                            i += 4;
                        }
                    }
                }
                49 => self.grid.set_bg(0),
                58 => {
                    if i + 1 < params.len() {
                        if params[i + 1] == 5 && i + 2 < params.len() {
                            self.grid.set_underline_color(params[i + 2] as u32);
                            i += 2;
                        } else if params[i + 1] == 2 && i + 4 < params.len() {
                            let r = params[i + 2] as u8;
                            let g = params[i + 3] as u8;
                            let b = params[i + 4] as u8;
                            self.grid.set_underline_color_rgb(r, g, b);
                            i += 4;
                        }
                    }
                }
                59 => self.grid.reset_underline_color(),
                90..=97 => self.grid.set_fg((params[i] - 90 + 8) as u32),
                100..=107 => self.grid.set_bg((params[i] - 100 + 8) as u32),
                _ => {}
            }
            i += 1;
        }
    }

    fn esc_dispatch(&mut self, intermediate: u8, final_byte: u8) {
        match (intermediate, final_byte) {
            (0, b'M') => self.grid.reverse_index(),
            (0, b'D') => self.grid.line_feed(),
            (0, b'E') => {
                self.grid.carriage_return();
                self.grid.line_feed();
            }
            (0, b'c') => {
                *self = Self::new(self.cols, self.rows);
            }
            (0, b'7') => self.save_cursor(),
            (0, b'8') => self.restore_cursor(),
            (b'(', b'0') => self.charset_g0 = Charset::DecSpecialGraphics,
            (b'(', b'B') => self.charset_g0 = Charset::Ascii,
            (b')', b'0') => self.charset_g1 = Charset::DecSpecialGraphics,
            (b')', b'B') => self.charset_g1 = Charset::Ascii,
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, data: &[u8]) {
        if let Some(semi) = data.iter().position(|&b| b == b';') {
            let cmd = &data[..semi];
            let payload = &data[semi + 1..];
            match cmd {
                b"0" | b"2" => {
                    if let Ok(title) = std::str::from_utf8(payload) {
                        self.title = Some(title.to_string());
                    }
                }
                b"1" => {}
                b"10" => {
                    if payload == b"?" {
                        self.response_buf.extend_from_slice(
                            b"\x1b]10;rgb:cd/d6/f4\x1b\\",
                        );
                    }
                }
                b"11" => {
                    if payload == b"?" {
                        self.response_buf.extend_from_slice(
                            b"\x1b]11;rgb:00/00/00\x1b\\",
                        );
                    }
                }
                b"52" => {
                    if let Some(semi2) = payload.iter().position(|&b| b == b';') {
                        let b64_data = &payload[semi2 + 1..];
                        if b64_data != b"?" {
                            self.osc52_clipboard = Some(b64_data.to_vec());
                        }
                    }
                }
                b"8" => {
                    if let Some(semi2) = payload.iter().position(|&b| b == b';') {
                        let url = &payload[semi2 + 1..];
                        if let Ok(url_str) = std::str::from_utf8(url) {
                            if url_str.is_empty() {
                                self.grid.clear_hyperlink();
                            } else {
                                self.grid.set_hyperlink(url_str);
                            }
                        }
                    } else if payload.is_empty() {
                        self.grid.clear_hyperlink();
                    }
                }
                _ => {}
            }
        }
    }

    fn save_cursor(&mut self) {
        self.saved_cursor = Some(self.grid.cursor_pos());
    }

    fn restore_cursor(&mut self) {
        if let Some((col, row)) = self.saved_cursor {
            self.grid.set_cursor(row, col);
        }
    }

    fn apc_dispatch(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        if data[0] == b'G' {
            self.handle_kitty_graphics(&data[1..]);
        }
    }

    fn handle_kitty_graphics(&mut self, data: &[u8]) {
        let (control, payload) = if let Some(pos) = data.iter().position(|&b| b == b';') {
            (&data[..pos], &data[pos + 1..])
        } else {
            (data, &[][..])
        };

        let mut action = 0u8;
        let mut img_id = 0u32;
        let mut fmt = 32u32;
        let mut width = 0u32;
        let mut height = 0u32;
        let mut more = false;
        let mut cols = 0u32;
        let mut rows_param = 0u32;

        for kv in control.split(|&b| b == b',') {
            if kv.len() < 3 || kv[1] != b'=' {
                continue;
            }
            let key = kv[0];
            let val = &kv[2..];
            let val_num = || -> u32 {
                val.iter().fold(0u32, |acc, &b| {
                    if b.is_ascii_digit() {
                        acc.saturating_mul(10).saturating_add((b - b'0') as u32)
                    } else {
                        acc
                    }
                })
            };
            match key {
                b'a' => action = val[0],
                b'i' => img_id = val_num(),
                b'f' => fmt = val_num(),
                b's' => width = val_num(),
                b'v' => height = val_num(),
                b'm' => more = val_num() == 1,
                b'c' => cols = val_num(),
                b'r' => rows_param = val_num(),
                _ => {}
            }
        }

        match action {
            b't' | b'T' | 0 => {
                if self.kitty_more_chunks || more {
                    self.kitty_payload_buf.extend_from_slice(payload);
                    if img_id > 0 { self.kitty_pending_id = img_id; }
                    if fmt > 0 { self.kitty_pending_fmt = fmt; }
                    if width > 0 { self.kitty_pending_width = width; }
                    if height > 0 { self.kitty_pending_height = height; }
                    self.kitty_more_chunks = more;
                    if !more {
                        let full_payload = std::mem::take(&mut self.kitty_payload_buf);
                        let final_id = self.kitty_pending_id;
                        let final_fmt = self.kitty_pending_fmt;
                        let final_w = self.kitty_pending_width;
                        let final_h = self.kitty_pending_height;
                        self.kitty_pending_id = 0;
                        self.kitty_pending_fmt = 0;
                        self.kitty_pending_width = 0;
                        self.kitty_pending_height = 0;
                        self.finalize_kitty_image(final_id, final_fmt, final_w, final_h, &full_payload, action, cols, rows_param);
                    }
                    return;
                }
                self.finalize_kitty_image(img_id, fmt, width, height, payload, action, cols, rows_param);
            }
            b'p' => {
                if let Some(_img) = self.kitty_images.iter().find(|i| i.id == img_id) {
                    let (col, row) = self.grid.cursor_pos();
                    self.kitty_placements.push(KittyPlacement {
                        image_id: img_id,
                        col,
                        row,
                        cols: if cols > 0 { cols as usize } else { 1 },
                        rows: if rows_param > 0 { rows_param as usize } else { 1 },
                    });
                    self.kitty_generation = self.kitty_generation.wrapping_add(1);
                    self.grid.mark_all_dirty();
                }
            }
            b'd' => {
                self.kitty_images.retain(|i| i.id != img_id);
                self.kitty_placements.retain(|p| p.image_id != img_id);
                self.kitty_generation = self.kitty_generation.wrapping_add(1);
                self.grid.mark_all_dirty();
            }
            _ => {}
        }

        if img_id > 0 {
            let resp = format!("\x1b_Gi={};OK\x1b\\", img_id);
            self.response_buf.extend_from_slice(resp.as_bytes());
        }
    }

    fn finalize_kitty_image(&mut self, id: u32, _fmt: u32, width: u32, height: u32, payload: &[u8], action: u8, cols: u32, rows_param: u32) {
        let decoded = if let Ok(d) = self.base64_decode_kitty(payload) { d } else { return };

        let actual_id = if id > 0 { id } else {
            (self.kitty_images.len() as u32) + 1
        };

        let image = KittyImage {
            id: actual_id,
            width,
            height,
            data: decoded,
        };

        self.kitty_images.retain(|i| i.id != actual_id);
        self.kitty_images.push(image);

        if action == b'T' || action == 0 {
            let (col, row) = self.grid.cursor_pos();
            self.kitty_placements.push(KittyPlacement {
                image_id: actual_id,
                col,
                row,
                cols: if cols > 0 { cols as usize } else { (width / self.grid.cols.max(1) as u32).max(1) as usize },
                rows: if rows_param > 0 { rows_param as usize } else { (height / self.grid.rows.max(1) as u32).max(1) as usize },
            });
        }
        self.kitty_generation = self.kitty_generation.wrapping_add(1);
        self.grid.mark_all_dirty();

        if actual_id > 0 {
            let resp = format!("\x1b_Gi={};OK\x1b\\", actual_id);
            self.response_buf.extend_from_slice(resp.as_bytes());
        }
    }

    fn base64_decode_kitty(&self, input: &[u8]) -> Result<Vec<u8>, ()> {
        const TABLE: [u8; 256] = {
            let mut t = [0xffu8; 256];
            let mut i = 0u8;
            while i < 26 {
                t[(b'A' + i) as usize] = i;
                t[(b'a' + i) as usize] = i + 26;
                i += 1;
            }
            let mut d = 0u8;
            while d < 10 {
                t[(b'0' + d) as usize] = d + 52;
                d += 1;
            }
            t[b'+' as usize] = 62;
            t[b'/' as usize] = 63;
            t
        };
        let mut out = Vec::with_capacity(input.len() * 3 / 4);
        let mut buf = 0u32;
        let mut bits = 0u32;
        for &b in input {
            if b == b'=' || b == b'\n' || b == b'\r' { continue; }
            let val = TABLE[b as usize];
            if val == 0xff { return Err(()); }
            buf = (buf << 6) | val as u32;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((buf >> bits) as u8);
                buf &= (1 << bits) - 1;
            }
        }
        Ok(out)
    }

    #[allow(dead_code)]
    pub fn kitty_image(&self, id: u32) -> Option<&KittyImage> {
        self.kitty_images.iter().find(|i| i.id == id)
    }

    pub fn kitty_generation(&self) -> u64 {
        self.kitty_generation
    }

    fn enter_alt_screen(&mut self) {
        if self.alt_grid.is_some() {
            return;
        }
        let main = std::mem::replace(
            &mut self.grid,
            Grid::new(self.cols, self.rows, [0xcd, 0xd6, 0xf4], [0x00, 0x00, 0x00]),
        );
        self.alt_grid = Some(main);
    }

    fn leave_alt_screen(&mut self) {
        if let Some(main) = self.alt_grid.take() {
            self.grid = main;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn processes_plain_text() {
        let mut t = Terminal::new(80, 24);
        t.process(b"hello");
        assert_eq!(t.grid.cell_char(0, 0), 'h');
        assert_eq!(t.grid.cell_char(0, 4), 'o');
    }

    #[test]
    fn processes_sgr_and_text() {
        let mut t = Terminal::new(80, 24);
        t.process(b"\x1b[1;31mred\x1b[0m");
        assert_eq!(t.grid.cell_char(0, 0), 'r');
        assert_eq!(t.grid.cell_char(0, 1), 'e');
        assert_eq!(t.grid.cell_char(0, 2), 'd');
    }

    #[test]
    fn cursor_movement() {
        let mut t = Terminal::new(80, 24);
        t.process(b"\x1b[5;10Hx");
        assert_eq!(t.grid.cell_char(4, 9), 'x');
    }

    #[test]
    fn erase_display() {
        let mut t = Terminal::new(10, 2);
        t.process(b"abcdefghij");
        t.process(b"\x1b[2J");
        assert_eq!(t.grid.cell_char(0, 0), ' ');
    }

    #[test]
    fn da1_response() {
        let mut t = Terminal::new(80, 24);
        t.process(b"\x1b[c");
        let resp = t.drain_responses().unwrap();
        assert_eq!(resp, b"\x1b[?62;22c");
    }

    #[test]
    fn da2_response() {
        let mut t = Terminal::new(80, 24);
        t.process(b"\x1b[>c");
        let resp = t.drain_responses().unwrap();
        assert_eq!(resp, b"\x1b[>1;1;0c");
    }

    #[test]
    fn dsr_cursor_position() {
        let mut t = Terminal::new(80, 24);
        t.process(b"\x1b[5;10H");
        t.process(b"\x1b[6n");
        let resp = t.drain_responses().unwrap();
        assert_eq!(resp, b"\x1b[5;10R");
    }

    #[test]
    fn cursor_visibility() {
        let mut t = Terminal::new(80, 24);
        assert!(t.cursor_visible);
        t.process(b"\x1b[?25l");
        assert!(!t.cursor_visible);
        t.process(b"\x1b[?25h");
        assert!(t.cursor_visible);
    }

    #[test]
    fn alt_screen() {
        let mut t = Terminal::new(80, 24);
        t.process(b"main");
        assert_eq!(t.grid.cell_char(0, 0), 'm');

        t.process(b"\x1b[?1049h");
        assert_eq!(t.grid.cell_char(0, 0), ' ');
        t.process(b"alt");
        assert_eq!(t.grid.cell_char(0, 0), 'a');

        t.process(b"\x1b[?1049l");
        assert_eq!(t.grid.cell_char(0, 0), 'm');
    }

    #[test]
    fn osc_title() {
        let mut t = Terminal::new(80, 24);
        t.process(b"\x1b]0;My Title\x07");
        assert_eq!(t.take_title().unwrap(), "My Title");
    }

    #[test]
    fn cursor_save_restore() {
        let mut t = Terminal::new(80, 24);
        t.process(b"\x1b[5;10H");
        t.process(b"\x1b7");
        t.process(b"\x1b[1;1H");
        assert_eq!(t.grid.cursor_pos(), (0, 0));
        t.process(b"\x1b8");
        assert_eq!(t.grid.cursor_pos(), (9, 4));
    }

    #[test]
    fn application_cursor_keys() {
        let mut t = Terminal::new(80, 24);
        assert!(!t.application_cursor_keys);
        t.process(b"\x1b[?1h");
        assert!(t.application_cursor_keys);
        t.process(b"\x1b[?1l");
        assert!(!t.application_cursor_keys);
    }

    #[test]
    fn fish_startup_queries_no_leak() {
        let mut t = Terminal::new(80, 24);

        let fish_init: &[u8] = b"\x1b[?u\x1b[>0q\x1b]11;?\x1b\\\x1b[?1049h\
            \x1bP+q696e646e\x1b\\\
            \x1bP+q71756572792d6f732d6e616d65\x1b\\\
            \x1b[?1049l\x1b[0c";

        t.process(fish_init);

        for row in 0..24 {
            for col in 0..80 {
                let ch = t.grid.cell_char(row, col);
                assert!(
                    ch == ' ' || ch == '\0',
                    "unexpected char '{}' (U+{:04X}) at row={} col={}",
                    ch,
                    ch as u32,
                    row,
                    col,
                );
            }
        }
    }

    #[test]
    fn starship_prompt_renders_text() {
        let mut t = Terminal::new(80, 24);

        // Simplified starship-like prompt with truecolor SGR + powerline chars
        let prompt: &[u8] = b"\x1b[J\n\x1b[38;2;243;139;168m\
            \x1b[48;2;243;139;168;38;2;17;17;27m jeremy\
            \x1b[48;2;250;179;135;38;2;243;139;168m\
            \x1b[38;2;17;17;27m ~/code \
            \x1b[0m\x1b[38;2;180;190;254m \x1b[1;38;2;166;227;161m\xe2\x9d\xaf\x1b[0m ";

        t.process(prompt);

        // "jeremy" should appear on row 1 (row 0 had the \n after ESC[J)
        let mut row1_text = String::new();
        for col in 0..80 {
            let ch = t.grid.cell_char(1, col);
            if ch != ' ' && ch != '\0' {
                row1_text.push(ch);
            }
        }
        assert!(
            row1_text.contains("jeremy"),
            "expected 'jeremy' in row 1, got: {:?}",
            row1_text,
        );
    }

    #[test]
    fn starship_exact_bytes_no_raw_escapes() {
        let mut t = Terminal::new(80, 24);

        // Exact starship output from hex dump (fish startup)
        let prompt: &[u8] = &[
            0x1b, 0x5b, 0x4a, // ESC[J
            0x0a,             // newline
            0x1b, 0x5b, 0x33, 0x38, 0x3b, 0x32, 0x3b, 0x32, 0x34, 0x33, 0x3b,
            0x31, 0x33, 0x39, 0x3b, 0x31, 0x36, 0x38, 0x6d, // ESC[38;2;243;139;168m
            0xee, 0x82, 0xb6, // U+E0B6 (powerline)
            0x1b, 0x5b, 0x34, 0x38, 0x3b, 0x32, 0x3b, 0x32, 0x34, 0x33, 0x3b,
            0x31, 0x33, 0x39, 0x3b, 0x31, 0x36, 0x38, 0x3b, 0x33, 0x38, 0x3b,
            0x32, 0x3b, 0x31, 0x37, 0x3b, 0x31, 0x37, 0x3b, 0x32, 0x37,
            0x6d, // ESC[48;2;243;139;168;38;2;17;17;27m
            0xf3, 0xb0, 0xa3, 0x87, // U+F0E07 (nerd font icon)
            0x20, // space
            0x6a, 0x65, 0x72, 0x65, 0x6d, 0x79, // "jeremy"
            0x1b, 0x5b, 0x30, 0x6d, // ESC[0m
        ];

        t.process(prompt);

        let mut text = String::new();
        for col in 0..80 {
            let ch = t.grid.cell_char(1, col);
            if ch != ' ' && ch != '\0' {
                text.push(ch);
            }
        }

        assert!(text.contains("jeremy"), "row 1 should contain 'jeremy', got: {:?}", text);
        assert!(!text.contains("38;"), "raw SGR params leaked: {:?}", text);
        assert!(!text.contains("48;"), "raw SGR params leaked: {:?}", text);
        assert!(!text.contains("["), "raw CSI bracket leaked: {:?}", text);
    }

    #[test]
    fn combined_sgr_fg_bg_truecolor() {
        let mut t = Terminal::new(80, 24);
        t.process(b"\x1b[48;2;243;139;168;38;2;17;17;27mX");
        let cell = t.grid.cell_at(0, 0);
        assert_eq!(cell.ch, b'X' as u32);
        assert_ne!(cell.fg, crate::grid::COLOR_DEFAULT, "fg should be set");
        assert_ne!(cell.bg, crate::grid::COLOR_DEFAULT, "bg should be set");
    }

    #[test]
    fn csi_less_than_intermediate_parsed() {
        let mut t = Terminal::new(80, 24);
        t.process(b"\x1b[<u");
        for col in 0..80 {
            let ch = t.grid.cell_char(0, col);
            assert!(ch == ' ' || ch == '\0', "CSI < u leaked char '{}' at col {}", ch, col);
        }
    }

    #[test]
    fn csi_question_u_responds() {
        let mut t = Terminal::new(80, 24);
        t.process(b"\x1b[?u");
        let resp = t.drain_responses().unwrap();
        assert_eq!(resp, b"\x1b[?0u");
    }

    #[test]
    fn xtversion_responds() {
        let mut t = Terminal::new(80, 24);
        t.process(b"\x1b[>0q");
        let resp = t.drain_responses().unwrap();
        assert!(resp.starts_with(b"\x1bP>|handterm"), "XTVERSION: {:?}", String::from_utf8_lossy(&resp));
    }

    #[test]
    fn osc_10_fg_query_responds() {
        let mut t = Terminal::new(80, 24);
        t.process(b"\x1b]10;?\x07");
        let resp = t.drain_responses().unwrap();
        assert!(resp.starts_with(b"\x1b]10;rgb:"), "OSC 10: {:?}", String::from_utf8_lossy(&resp));
    }

    #[test]
    fn osc_11_bg_query_responds() {
        let mut t = Terminal::new(80, 24);
        t.process(b"\x1b]11;?\x07");
        let resp = t.drain_responses().unwrap();
        assert!(resp.starts_with(b"\x1b]11;rgb:"), "OSC 11: {:?}", String::from_utf8_lossy(&resp));
    }

    #[test]
    fn dcs_string_silently_consumed() {
        let mut t = Terminal::new(80, 24);
        t.process(b"\x1bP+q696e646e\x1b\\");
        for col in 0..80 {
            let ch = t.grid.cell_char(0, col);
            assert!(ch == ' ' || ch == '\0', "DCS leaked char '{}' at col {}", ch, col);
        }
    }

    #[test]
    fn osc_st_terminator() {
        let mut t = Terminal::new(80, 24);
        t.process(b"\x1b]0;My Title\x1b\\visible");
        assert_eq!(t.take_title().unwrap(), "My Title");
        assert_eq!(t.grid.cell_char(0, 0), 'v');
        assert_eq!(t.grid.cell_char(0, 6), 'e');
    }

    #[test]
    fn sgr_256_color() {
        let mut t = Terminal::new(80, 24);
        t.process(b"\x1b[38;5;196mR\x1b[48;5;21mB");
        let r_cell = t.grid.cell_at(0, 0);
        assert_eq!(r_cell.ch, b'R' as u32);
        assert_eq!(r_cell.fg, 196);
        let b_cell = t.grid.cell_at(0, 1);
        assert_eq!(b_cell.ch, b'B' as u32);
        assert_eq!(b_cell.bg, 21);
    }

    #[test]
    fn sgr_bright_colors() {
        let mut t = Terminal::new(80, 24);
        t.process(b"\x1b[91mA\x1b[102mB");
        let a = t.grid.cell_at(0, 0);
        assert_eq!(a.fg, 9);
        let b = t.grid.cell_at(0, 1);
        assert_eq!(b.bg, 10);
    }

    #[test]
    fn scroll_region_and_index() {
        let mut t = Terminal::new(10, 5);
        t.process(b"\x1b[2;4r");
        t.process(b"\x1b[2;1HAAA\x1b[3;1HBBB\x1b[4;1HCCC");
        t.process(b"\x1b[4;1H\n"); // LF at bottom of scroll region -> scroll within region
        // After scroll within region 2-4: row 1 had AAA, row 2 had BBB, row 3 had CCC
        // Scroll moves: BBB->row1 pos, CCC->row2 pos, blank->row3 pos (within region)
        let c = t.grid.cell_char(1, 0);
        assert!(c == 'B', "after scroll region LF: row 1 = '{}' (expected B)", c);
    }

    #[test]
    fn insert_delete_lines() {
        let mut t = Terminal::new(10, 5);
        t.process(b"\x1b[1;1HAAAA\x1b[2;1HBBBB\x1b[3;1HCCCC");
        t.process(b"\x1b[2;1H");
        t.process(b"\x1b[1L");
        // insert_lines scrolls down within scroll region
        // row 0 should shift to row 1, blank at row 0
        assert_eq!(t.grid.cell_char(0, 0), ' ');
        assert_eq!(t.grid.cell_char(1, 0), 'A');
    }

    #[test]
    fn insert_delete_chars() {
        let mut t = Terminal::new(10, 5);
        t.process(b"ABCDE");
        t.process(b"\x1b[1;2H");
        t.process(b"\x1b[1P");
        assert_eq!(t.grid.cell_char(0, 0), 'A');
        assert_eq!(t.grid.cell_char(0, 1), 'C');
        assert_eq!(t.grid.cell_char(0, 2), 'D');
    }

    #[test]
    fn erase_chars() {
        let mut t = Terminal::new(10, 5);
        t.process(b"ABCDE");
        t.process(b"\x1b[1;2H");
        t.process(b"\x1b[2X");
        assert_eq!(t.grid.cell_char(0, 0), 'A');
        assert_eq!(t.grid.cell_char(0, 1), ' ');
        assert_eq!(t.grid.cell_char(0, 2), ' ');
        assert_eq!(t.grid.cell_char(0, 3), 'D');
    }

    #[test]
    fn cursor_horizontal_absolute() {
        let mut t = Terminal::new(80, 24);
        t.process(b"ABCDE\x1b[3GX");
        assert_eq!(t.grid.cell_char(0, 2), 'X');
    }

    #[test]
    fn vertical_position_absolute() {
        let mut t = Terminal::new(80, 24);
        t.process(b"\x1b[5dX");
        assert_eq!(t.grid.cell_char(4, 0), 'X');
    }

    #[test]
    fn cursor_next_prev_line() {
        let mut t = Terminal::new(80, 24);
        t.process(b"\x1b[3;5H");
        t.process(b"\x1b[2EX");
        assert_eq!(t.grid.cell_char(4, 0), 'X');

        let mut t2 = Terminal::new(80, 24);
        t2.process(b"\x1b[5;5H");
        t2.process(b"\x1b[2FX");
        assert_eq!(t2.grid.cell_char(2, 0), 'X');
    }

    #[test]
    fn tab_stops() {
        let mut t = Terminal::new(80, 24);
        t.process(b"\tX");
        assert_eq!(t.grid.cell_char(0, 8), 'X');
    }

    #[test]
    fn reverse_index() {
        let mut t = Terminal::new(10, 5);
        t.process(b"LINE1\nLINE2");
        t.process(b"\x1b[1;1H");
        t.process(b"\x1bM");
        assert_eq!(t.grid.cell_char(0, 0), ' ');
        assert_eq!(t.grid.cell_char(1, 0), 'L');
    }

    #[test]
    fn line_drawing_charset() {
        let mut t = Terminal::new(80, 24);
        t.process(b"\x1b(0");
        t.process(b"q");
        let ch = t.grid.cell_char(0, 0);
        assert_eq!(ch, '\u{2500}', "expected box-drawing horizontal, got '{}'", ch);
        t.process(b"\x1b(B");
        t.process(b"q");
        assert_eq!(t.grid.cell_char(0, 1), 'q');
    }

    #[test]
    fn utf8_multibyte() {
        let mut t = Terminal::new(80, 24);
        t.process("héllo".as_bytes());
        assert_eq!(t.grid.cell_char(0, 0), 'h');
        assert_eq!(t.grid.cell_char(0, 1), 'é');
        assert_eq!(t.grid.cell_char(0, 2), 'l');
    }

    #[test]
    fn attrs_bold_dim_inverse() {
        let mut t = Terminal::new(80, 24);
        t.process(b"\x1b[1mB\x1b[2mD\x1b[7mI\x1b[0mN");
        let b = t.grid.cell_at(0, 0);
        assert!(b.attrs & crate::grid::ATTR_BOLD != 0);
        let d = t.grid.cell_at(0, 1);
        assert!(d.attrs & crate::grid::ATTR_DIM != 0);
        let i = t.grid.cell_at(0, 2);
        assert!(i.attrs & crate::grid::ATTR_INVERSE != 0);
        let n = t.grid.cell_at(0, 3);
        assert_eq!(n.attrs, 0);
    }

    #[test]
    fn bracketed_paste_mode() {
        let mut t = Terminal::new(80, 24);
        assert!(!t.bracketed_paste_mode());
        t.process(b"\x1b[?2004h");
        assert!(t.bracketed_paste_mode());
        t.process(b"\x1b[?2004l");
        assert!(!t.bracketed_paste_mode());
    }

    #[test]
    fn focus_events_mode() {
        let mut t = Terminal::new(80, 24);
        assert!(!t.focus_events_mode());
        t.process(b"\x1b[?1004h");
        assert!(t.focus_events_mode());
        t.process(b"\x1b[?1004l");
        assert!(!t.focus_events_mode());
    }

    #[test]
    fn mouse_modes() {
        let mut t = Terminal::new(80, 24);
        assert_eq!(t.mouse_mode, MouseMode::Off);
        t.process(b"\x1b[?1000h");
        assert_eq!(t.mouse_mode, MouseMode::Normal);
        t.process(b"\x1b[?1002h");
        assert_eq!(t.mouse_mode, MouseMode::ButtonEvent);
        t.process(b"\x1b[?1003h");
        assert_eq!(t.mouse_mode, MouseMode::AnyEvent);
        t.process(b"\x1b[?1003l");
        assert_eq!(t.mouse_mode, MouseMode::Off);
    }

    #[test]
    fn mouse_sgr_encoding() {
        let mut t = Terminal::new(80, 24);
        t.process(b"\x1b[?1000h\x1b[?1006h");
        assert_eq!(t.mouse_encoding, MouseEncoding::Sgr);
        let resp = t.encode_mouse(0, 5, 10, true).unwrap();
        assert_eq!(resp, b"\x1b[<0;6;11M");
        let resp = t.encode_mouse(0, 5, 10, false).unwrap();
        assert_eq!(resp, b"\x1b[<0;6;11m");
    }

    #[test]
    fn cursor_style_decscusr() {
        let mut t = Terminal::new(80, 24);
        assert_eq!(t.cursor_style, CursorStyle::Block);
        t.process(b"\x1b[5 q");
        assert_eq!(t.cursor_style, CursorStyle::Bar);
        t.process(b"\x1b[3 q");
        assert_eq!(t.cursor_style, CursorStyle::Underline);
        t.process(b"\x1b[1 q");
        assert_eq!(t.cursor_style, CursorStyle::Block);
    }

    #[test]
    fn dsr_status_report() {
        let mut t = Terminal::new(80, 24);
        t.process(b"\x1b[5n");
        let resp = t.drain_responses().unwrap();
        assert_eq!(resp, b"\x1b[0n");
    }

    #[test]
    fn window_size_report() {
        let mut t = Terminal::new(80, 24);
        t.process(b"\x1b[18t");
        let resp = t.drain_responses().unwrap();
        assert_eq!(resp, b"\x1b[8;24;80t");
    }

    #[test]
    fn full_reset() {
        let mut t = Terminal::new(80, 24);
        t.process(b"\x1b[1mhello\x1b[?25l");
        assert!(!t.cursor_visible);
        t.process(b"\x1bc");
        assert!(t.cursor_visible);
        assert_eq!(t.grid.cell_char(0, 0), ' ');
    }

    #[test]
    fn erase_line_variants() {
        let mut t = Terminal::new(10, 1);
        t.process(b"ABCDEFGHIJ");
        t.process(b"\x1b[1;5H");
        t.process(b"\x1b[0K");
        assert_eq!(t.grid.cell_char(0, 3), 'D');
        assert_eq!(t.grid.cell_char(0, 4), ' ');
        assert_eq!(t.grid.cell_char(0, 9), ' ');

        let mut t2 = Terminal::new(10, 1);
        t2.process(b"ABCDEFGHIJ");
        t2.process(b"\x1b[1;5H");
        t2.process(b"\x1b[1K");
        assert_eq!(t2.grid.cell_char(0, 0), ' ');
        assert_eq!(t2.grid.cell_char(0, 4), ' ');
        assert_eq!(t2.grid.cell_char(0, 5), 'F');

        let mut t3 = Terminal::new(10, 1);
        t3.process(b"ABCDEFGHIJ");
        t3.process(b"\x1b[1;5H");
        t3.process(b"\x1b[2K");
        for col in 0..10 {
            assert_eq!(t3.grid.cell_char(0, col), ' ');
        }
    }

    #[test]
    fn scroll_up_down() {
        let mut t = Terminal::new(10, 3);
        t.process(b"\x1b[1;1HAAA\x1b[2;1HBBB\x1b[3;1HCCC");
        t.process(b"\x1b[1S");
        assert_eq!(t.grid.cell_char(0, 0), 'B');
        assert_eq!(t.grid.cell_char(1, 0), 'C');
        assert_eq!(t.grid.cell_char(2, 0), ' ');

        let mut t2 = Terminal::new(10, 3);
        t2.process(b"\x1b[1;1HAAA\x1b[2;1HBBB\x1b[3;1HCCC");
        t2.process(b"\x1b[1T");
        assert_eq!(t2.grid.cell_char(0, 0), ' ');
        assert_eq!(t2.grid.cell_char(1, 0), 'A');
        assert_eq!(t2.grid.cell_char(2, 0), 'B');
    }

    #[test]
    fn autowrap_mode() {
        let mut t = Terminal::new(5, 2);
        t.process(b"\x1b[?7h");
        assert!(t.grid.autowrap);
        t.process(b"ABCDEFG");
        assert_eq!(t.grid.cell_char(0, 4), 'E');
        assert_eq!(t.grid.cell_char(1, 0), 'F');

        let mut t2 = Terminal::new(5, 2);
        t2.process(b"\x1b[?7l");
        assert!(!t2.grid.autowrap);
        t2.process(b"ABCDEFG");
        assert_eq!(t2.grid.cell_char(0, 4), 'G');
        assert_eq!(t2.grid.cell_char(1, 0), ' ');
    }

    #[test]
    fn kitty_graphics_upload_places_and_deletes_image() {
        let mut t = Terminal::new(8, 4);
        t.process(b"\x1b_Ga=T,i=7,f=32,s=1,v=1,c=1,r=1;/wAA/w==\x1b\\");

        let image = t.kitty_image(7).expect("kitty image should exist");
        assert_eq!(image.width, 1);
        assert_eq!(image.height, 1);
        assert_eq!(image.data, vec![0xff, 0x00, 0x00, 0xff]);
        assert_eq!(t.kitty_placements.len(), 1);
        assert_eq!(t.kitty_placements[0].image_id, 7);

        t.process(b"\x1b_Ga=d,i=7\x1b\\");
        assert!(t.kitty_image(7).is_none());
        assert!(t.kitty_placements.is_empty());
    }
}
