use crate::grid::Grid;
use crate::parser::{Action, Parser};

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

    pub fn process(&mut self, data: &[u8]) {
        let mut ascii_start: Option<usize> = None;

        for (i, &byte) in data.iter().enumerate() {
            let action = self.parser.advance(byte);

            match action {
                Action::Print(_b) => {
                    if ascii_start.is_none() {
                        ascii_start = Some(i);
                    }
                }
                _ => {
                    if let Some(start) = ascii_start.take() {
                        self.grid.write_bytes(&data[start..i]);
                    }
                    self.handle_action(action);
                }
            }
        }

        if let Some(start) = ascii_start {
            self.grid.write_bytes(&data[start..]);
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
            Action::Print(_) | Action::Nop => {}
        }
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' | 0x0b | 0x0c => self.grid.line_feed(),
            b'\r' => self.grid.carriage_return(),
            b'\t' => self.grid.tab(),
            0x08 => self.grid.backspace(),
            0x07 => {}
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
            // Private mode set
            (b'?', b'h') => {
                let params: Vec<u16> = self.parser.params().to_vec();
                for param in &params {
                    match param {
                        1 => {}    // DECCKM - application cursor keys (TODO)
                        7 => {}    // DECAWM - auto wrap (TODO)
                        12 => {}   // Cursor blink
                        25 => self.cursor_visible = true,   // DECTCEM show cursor
                        47 | 1047 => self.enter_alt_screen(),
                        1049 => {
                            self.save_cursor();
                            self.enter_alt_screen();
                        }
                        2004 => self.mode_bracketed_paste = true,
                        1004 => self.mode_focus_events = true,
                        _ => {}
                    }
                }
            }
            // Private mode reset
            (b'?', b'l') => {
                let params: Vec<u16> = self.parser.params().to_vec();
                for param in &params {
                    match param {
                        1 => {}
                        7 => {}
                        12 => {}
                        25 => self.cursor_visible = false,  // DECTCEM hide cursor
                        47 | 1047 => self.leave_alt_screen(),
                        1049 => {
                            self.leave_alt_screen();
                            self.restore_cursor();
                        }
                        2004 => self.mode_bracketed_paste = false,
                        1004 => self.mode_focus_events = false,
                        _ => {}
                    }
                }
            }
            // Cursor save/restore (ANSI.SYS style)
            (0, b's') => self.save_cursor(),
            (0, b'u') => self.restore_cursor(),
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
                4 => self.grid.set_underline(true),
                7 => self.grid.set_inverse(true),
                9 => self.grid.set_strikethrough(true),
                22 => {
                    self.grid.set_bold(false);
                    self.grid.set_dim(false);
                }
                23 => self.grid.set_italic(false),
                24 => self.grid.set_underline(false),
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
                b"1" => {
                    // Icon name - ignore
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
    use super::Terminal;

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
}
