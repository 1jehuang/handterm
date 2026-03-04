#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct Cell {
    pub ch: u32,
    pub fg: u32,
    pub bg: u32,
    pub attrs: u8,
    pub flags: u8,
}

pub const COLOR_DEFAULT: u32 = 0;
pub const COLOR_FLAG_RGB: u32 = 0x8000_0000;

pub const ATTR_BOLD: u8 = 0x01;
pub const ATTR_DIM: u8 = 0x02;
pub const ATTR_ITALIC: u8 = 0x04;
pub const ATTR_UNDERLINE: u8 = 0x08;
pub const ATTR_INVERSE: u8 = 0x10;
pub const ATTR_STRIKETHROUGH: u8 = 0x20;

pub const FLAG_WIDE: u8 = 0x01;
pub const FLAG_WIDE_CONT: u8 = 0x02;

#[inline]
fn decode_utf8(bytes: &[u8]) -> (u32, usize) {
    let b0 = bytes[0];
    if b0 < 0x80 {
        (b0 as u32, 1)
    } else if b0 < 0xc0 {
        (0xFFFD, 1)
    } else if b0 < 0xe0 {
        if bytes.len() >= 2 && (bytes[1] & 0xc0) == 0x80 {
            let cp = ((b0 as u32 & 0x1f) << 6) | (bytes[1] as u32 & 0x3f);
            (cp, 2)
        } else {
            (0xFFFD, 1)
        }
    } else if b0 < 0xf0 {
        if bytes.len() >= 3 && (bytes[1] & 0xc0) == 0x80 && (bytes[2] & 0xc0) == 0x80 {
            let cp = ((b0 as u32 & 0x0f) << 12)
                | ((bytes[1] as u32 & 0x3f) << 6)
                | (bytes[2] as u32 & 0x3f);
            (cp, 3)
        } else {
            (0xFFFD, 1)
        }
    } else if b0 < 0xf8 {
        if bytes.len() >= 4
            && (bytes[1] & 0xc0) == 0x80
            && (bytes[2] & 0xc0) == 0x80
            && (bytes[3] & 0xc0) == 0x80
        {
            let cp = ((b0 as u32 & 0x07) << 18)
                | ((bytes[1] as u32 & 0x3f) << 12)
                | ((bytes[2] as u32 & 0x3f) << 6)
                | (bytes[3] as u32 & 0x3f);
            (cp, 4)
        } else {
            (0xFFFD, 1)
        }
    } else {
        (0xFFFD, 1)
    }
}

impl Cell {
    pub const BLANK: Self = Self {
        ch: b' ' as u32,
        fg: COLOR_DEFAULT,
        bg: COLOR_DEFAULT,
        attrs: 0,
        flags: 0,
    };

    #[allow(dead_code)]
    pub fn char_display(&self) -> char {
        char::from_u32(self.ch).unwrap_or(' ')
    }
}

pub struct Grid {
    pub cols: usize,
    pub rows: usize,
    cursor_col: usize,
    cursor_row: usize,
    cells: Vec<Cell>,
    top_row: usize,
    current_fg: u32,
    current_bg: u32,
    current_attrs: u8,
    scroll_top: usize,
    scroll_bottom: usize,
    scrollback: Vec<Vec<Cell>>,
    scrollback_max: usize,
    pub scroll_offset: usize,
}

impl Grid {
    pub fn new(cols: u16, rows: u16, _default_fg: [u8; 3], _default_bg: [u8; 3]) -> Self {
        let cols = cols as usize;
        let rows = rows as usize;
        Self {
            cols,
            rows,
            cursor_col: 0,
            cursor_row: 0,
            cells: vec![Cell::BLANK; cols * rows],
            top_row: 0,
            current_fg: COLOR_DEFAULT,
            current_bg: COLOR_DEFAULT,
            current_attrs: 0,
            scroll_top: 0,
            scroll_bottom: rows,
            scrollback: Vec::new(),
            scrollback_max: 10000,
            scroll_offset: 0,
        }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        let new_cols = cols.max(1) as usize;
        let new_rows = rows.max(1) as usize;

        if new_cols == self.cols && new_rows == self.rows {
            return;
        }

        let old_cols = self.cols;
        let old_rows = self.rows;
        let mut new_cells = vec![Cell::BLANK; new_cols * new_rows];

        let copy_rows = old_rows.min(new_rows);
        let copy_cols = old_cols.min(new_cols);

        for r in 0..copy_rows {
            let src_phys = self.physical_row(r);
            let src_start = src_phys * old_cols;
            let dst_start = r * new_cols;
            for c in 0..copy_cols {
                new_cells[dst_start + c] = self.cells[src_start + c];
            }
        }

        self.cells = new_cells;
        self.cols = new_cols;
        self.rows = new_rows;
        self.top_row = 0;
        self.scroll_top = 0;
        self.scroll_bottom = new_rows;
        self.cursor_col = self.cursor_col.min(new_cols.saturating_sub(1));
        self.cursor_row = self.cursor_row.min(new_rows.saturating_sub(1));
    }

    pub fn cursor_pos(&self) -> (usize, usize) {
        (self.cursor_col, self.cursor_row)
    }

    #[inline(always)]
    fn physical_row(&self, logical_row: usize) -> usize {
        (self.top_row + logical_row) % self.rows
    }

    fn cell_index_at(&self, logical_row: usize, col: usize) -> usize {
        self.physical_row(logical_row) * self.cols + col
    }

    pub fn cell_at(&self, row: usize, col: usize) -> &Cell {
        let idx = self.cell_index_at(row, col);
        &self.cells[idx]
    }

    pub fn get_selection_text(&self) -> String {
        String::new()
    }

    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }

    pub fn cell_at_scroll(&self, row: usize, col: usize) -> &Cell {
        if self.scroll_offset == 0 {
            return self.cell_at(row, col);
        }
        let sb_len = self.scrollback.len();
        if self.scroll_offset > sb_len {
            return &Cell::BLANK;
        }
        let sb_start = sb_len - self.scroll_offset;
        let line_in_sb = sb_start + row;
        if line_in_sb < sb_len {
            let sb_row = &self.scrollback[line_in_sb];
            if col < sb_row.len() {
                &sb_row[col]
            } else {
                &Cell::BLANK
            }
        } else {
            let grid_row = line_in_sb - sb_len;
            if grid_row < self.rows && col < self.cols {
                self.cell_at(grid_row, col)
            } else {
                &Cell::BLANK
            }
        }
    }

    #[allow(dead_code)]
    pub fn cell_char(&self, row: usize, col: usize) -> char {
        if row >= self.rows || col >= self.cols {
            return ' ';
        }
        self.cell_at(row, col).char_display()
    }

    pub fn get_text(&self, start_row: usize, end_row: usize) -> String {
        let end = end_row.min(self.rows);
        let mut out = String::with_capacity(self.cols * (end - start_row) + end - start_row);
        for row in start_row..end {
            for col in 0..self.cols {
                out.push(self.cell_at(row, col).char_display());
            }
            let trimmed = out.trim_end_matches(' ');
            let trimmed_len = trimmed.len();
            out.truncate(trimmed_len);
            if row + 1 < end {
                out.push('\n');
            }
        }
        out
    }

    pub fn get_all_text(&self) -> String {
        self.get_text(0, self.rows)
    }

    #[inline]
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        let mut i = 0;
        let len = bytes.len();

        while i < len {
            let b = unsafe { *bytes.get_unchecked(i) };

            if (0x20..=0x7e).contains(&b) {
                let run_start = i;
                i += 1;
                while i < len {
                    let next = unsafe { *bytes.get_unchecked(i) };
                    if !(0x20..=0x7e).contains(&next) {
                        break;
                    }
                    i += 1;
                }
                self.write_ascii_run(&bytes[run_start..i]);
            } else if b >= 0xc0 {
                let (cp, consumed) = decode_utf8(&bytes[i..]);
                if cp != 0 {
                    self.put_char(cp);
                }
                i += consumed;
            } else if b >= 0x80 {
                i += 1;
            } else {
                match b {
                    b'\n' => self.line_feed(),
                    b'\r' => self.cursor_col = 0,
                    b'\t' => self.tab(),
                    _ => {}
                }
                i += 1;
            }
        }
    }

    #[inline]
    fn write_ascii_run(&mut self, run: &[u8]) {
        let mut ri = 0;
        let run_len = run.len();

        while ri < run_len {
            if self.cursor_row >= self.rows {
                return;
            }

            let phys_row = self.physical_row(self.cursor_row);
            let row_base = phys_row * self.cols;
            let remaining_in_row = self.cols - self.cursor_col;
            let chunk_len = remaining_in_row.min(run_len - ri);

            let dest_start = row_base + self.cursor_col;
            let fg = self.current_fg;
            let bg = self.current_bg;
            let attrs = self.current_attrs;

            unsafe {
                let base_ptr = self.cells.as_mut_ptr().add(dest_start);
                let src_ptr = run.as_ptr().add(ri);
                for j in 0..chunk_len {
                    let cell = &mut *base_ptr.add(j);
                    cell.ch = *src_ptr.add(j) as u32;
                    cell.fg = fg;
                    cell.bg = bg;
                    cell.attrs = attrs;
                }
            }

            ri += chunk_len;
            self.cursor_col += chunk_len;

            if self.cursor_col >= self.cols {
                self.cursor_col = 0;
                if self.cursor_row + 1 >= self.scroll_bottom {
                    self.scroll_up();
                } else {
                    self.cursor_row += 1;
                }
            }
        }
    }

    pub fn put_char(&mut self, ch: u32) {
        if self.cursor_row >= self.rows || self.cursor_col >= self.cols {
            return;
        }

        let width = if let Some(c) = char::from_u32(ch) {
            unicode_width::UnicodeWidthChar::width(c).unwrap_or(1)
        } else {
            1
        };

        if width == 2 && self.cursor_col + 1 >= self.cols {
            let idx = self.cell_index_at(self.cursor_row, self.cursor_col);
            self.cells[idx] = Cell::BLANK;
            self.cursor_col = 0;
            if self.cursor_row + 1 >= self.scroll_bottom {
                self.scroll_up();
            } else {
                self.cursor_row += 1;
            }
        }

        let idx = self.cell_index_at(self.cursor_row, self.cursor_col);
        let cell = &mut self.cells[idx];
        cell.ch = ch;
        cell.fg = self.current_fg;
        cell.bg = self.current_bg;
        cell.attrs = self.current_attrs;
        cell.flags = if width == 2 { FLAG_WIDE } else { 0 };

        self.cursor_col += 1;

        if width == 2 && self.cursor_col < self.cols {
            let idx2 = self.cell_index_at(self.cursor_row, self.cursor_col);
            let cell2 = &mut self.cells[idx2];
            cell2.ch = 0;
            cell2.fg = self.current_fg;
            cell2.bg = self.current_bg;
            cell2.attrs = self.current_attrs;
            cell2.flags = FLAG_WIDE_CONT;
            self.cursor_col += 1;
        }

        if self.cursor_col >= self.cols {
            self.cursor_col = 0;
            if self.cursor_row + 1 >= self.scroll_bottom {
                self.scroll_up();
            } else {
                self.cursor_row += 1;
            }
        }
    }

    pub fn line_feed(&mut self) {
        if self.cursor_row + 1 >= self.scroll_bottom {
            self.scroll_up();
        } else {
            self.cursor_row += 1;
        }
    }

    pub fn carriage_return(&mut self) {
        self.cursor_col = 0;
    }

    pub fn tab(&mut self) {
        let next = ((self.cursor_col / 8) + 1) * 8;
        self.cursor_col = next.min(self.cols.saturating_sub(1));
    }

    pub fn backspace(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        }
    }

    pub fn reverse_index(&mut self) {
        if self.cursor_row == self.scroll_top {
            self.scroll_down();
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
        }
    }

    pub fn set_cursor(&mut self, row: usize, col: usize) {
        self.cursor_row = row.min(self.rows.saturating_sub(1));
        self.cursor_col = col.min(self.cols.saturating_sub(1));
    }

    pub fn set_cursor_row(&mut self, row: usize) {
        self.cursor_row = row.min(self.rows.saturating_sub(1));
    }

    pub fn set_cursor_col(&mut self, col: usize) {
        self.cursor_col = col.min(self.cols.saturating_sub(1));
    }

    pub fn move_cursor_up(&mut self, n: usize) {
        self.cursor_row = self.cursor_row.saturating_sub(n);
    }

    pub fn move_cursor_down(&mut self, n: usize) {
        self.cursor_row = (self.cursor_row + n).min(self.rows.saturating_sub(1));
    }

    pub fn move_cursor_right(&mut self, n: usize) {
        self.cursor_col = (self.cursor_col + n).min(self.cols.saturating_sub(1));
    }

    pub fn move_cursor_left(&mut self, n: usize) {
        self.cursor_col = self.cursor_col.saturating_sub(n);
    }

    pub fn erase_all(&mut self) {
        for r in 0..self.rows {
            self.erase_row(r);
        }
    }

    pub fn erase_below(&mut self) {
        self.erase_line_right();
        for r in (self.cursor_row + 1)..self.rows {
            self.erase_row(r);
        }
    }

    pub fn erase_above(&mut self) {
        self.erase_line_left();
        for r in 0..self.cursor_row {
            self.erase_row(r);
        }
    }

    fn erase_row(&mut self, row: usize) {
        let phys = self.physical_row(row);
        let start = phys * self.cols;
        let end = start + self.cols;
        self.cells[start..end].fill(Cell::BLANK);
    }

    pub fn erase_line_right(&mut self) {
        if self.cursor_row >= self.rows {
            return;
        }
        let phys = self.physical_row(self.cursor_row);
        let start = phys * self.cols + self.cursor_col;
        let end = phys * self.cols + self.cols;
        self.cells[start..end].fill(Cell::BLANK);
    }

    pub fn erase_line_left(&mut self) {
        if self.cursor_row >= self.rows {
            return;
        }
        let phys = self.physical_row(self.cursor_row);
        let start = phys * self.cols;
        let end = phys * self.cols + self.cursor_col + 1;
        self.cells[start..end.min(start + self.cols)].fill(Cell::BLANK);
    }

    pub fn erase_line_all(&mut self) {
        self.erase_row(self.cursor_row);
    }

    pub fn erase_chars(&mut self, n: usize) {
        if self.cursor_row >= self.rows {
            return;
        }
        let phys = self.physical_row(self.cursor_row);
        let start = phys * self.cols + self.cursor_col;
        let end = (start + n).min(phys * self.cols + self.cols);
        self.cells[start..end].fill(Cell::BLANK);
    }

    pub fn insert_lines(&mut self, n: usize) {
        for _ in 0..n {
            self.scroll_down();
        }
    }

    pub fn delete_lines(&mut self, n: usize) {
        for _ in 0..n {
            self.scroll_up();
        }
    }

    pub fn insert_chars(&mut self, n: usize) {
        if self.cursor_row >= self.rows {
            return;
        }
        let phys = self.physical_row(self.cursor_row);
        let row_start = phys * self.cols;
        let col = self.cursor_col;
        let n = n.min(self.cols - col);
        let src = row_start + col;
        let dest = row_start + col + n;
        let move_count = self.cols - col - n;
        if move_count > 0 {
            self.cells.copy_within(src..src + move_count, dest);
        }
        self.cells[src..src + n].fill(Cell::BLANK);
    }

    pub fn delete_chars(&mut self, n: usize) {
        if self.cursor_row >= self.rows {
            return;
        }
        let phys = self.physical_row(self.cursor_row);
        let row_start = phys * self.cols;
        let col = self.cursor_col;
        let n = n.min(self.cols - col);
        let src = row_start + col + n;
        let dest = row_start + col;
        let move_count = self.cols - col - n;
        if move_count > 0 {
            self.cells.copy_within(src..src + move_count, dest);
        }
        let blank_start = row_start + self.cols - n;
        self.cells[blank_start..row_start + self.cols].fill(Cell::BLANK);
    }

    pub fn set_scroll_region(&mut self, top: usize, bottom: usize) {
        self.scroll_top = top.min(self.rows.saturating_sub(1));
        self.scroll_bottom = bottom.min(self.rows).max(self.scroll_top + 1);
        self.cursor_row = 0;
        self.cursor_col = 0;
    }

    pub fn scroll_up_n(&mut self, n: usize) {
        for _ in 0..n {
            self.scroll_up();
        }
    }

    pub fn scroll_down_n(&mut self, n: usize) {
        for _ in 0..n {
            self.scroll_down();
        }
    }

    fn scroll_up(&mut self) {
        if self.rows == 0 || self.cols == 0 {
            return;
        }

        if self.scroll_top == 0 && self.scroll_bottom == self.rows {
            let old_top = self.physical_row(0);
            let blank_start = old_top * self.cols;
            let blank_end = blank_start + self.cols;

            let row = self.cells[blank_start..blank_end].to_vec();
            self.scrollback.push(row);
            if self.scrollback.len() > self.scrollback_max {
                self.scrollback.remove(0);
            }

            self.cells[blank_start..blank_end].fill(Cell::BLANK);
            self.top_row = (self.top_row + 1) % self.rows;
        } else {
            for r in self.scroll_top..self.scroll_bottom.saturating_sub(1) {
                let src = self.physical_row(r + 1) * self.cols;
                let dst = self.physical_row(r) * self.cols;
                for c in 0..self.cols {
                    self.cells[dst + c] = self.cells[src + c];
                }
            }
            let last = self.physical_row(self.scroll_bottom.saturating_sub(1));
            let start = last * self.cols;
            self.cells[start..start + self.cols].fill(Cell::BLANK);
        }
    }

    fn scroll_down(&mut self) {
        if self.rows == 0 || self.cols == 0 {
            return;
        }

        for r in (self.scroll_top + 1..self.scroll_bottom).rev() {
            let src = self.physical_row(r - 1) * self.cols;
            let dst = self.physical_row(r) * self.cols;
            for c in 0..self.cols {
                self.cells[dst + c] = self.cells[src + c];
            }
        }
        let first = self.physical_row(self.scroll_top);
        let start = first * self.cols;
        self.cells[start..start + self.cols].fill(Cell::BLANK);
    }

    pub fn reset_attrs(&mut self) {
        self.current_fg = COLOR_DEFAULT;
        self.current_bg = COLOR_DEFAULT;
        self.current_attrs = 0;
    }

    pub fn set_bold(&mut self, on: bool) {
        if on {
            self.current_attrs |= ATTR_BOLD;
        } else {
            self.current_attrs &= !ATTR_BOLD;
        }
    }

    pub fn set_dim(&mut self, on: bool) {
        if on {
            self.current_attrs |= ATTR_DIM;
        } else {
            self.current_attrs &= !ATTR_DIM;
        }
    }

    pub fn set_italic(&mut self, on: bool) {
        if on {
            self.current_attrs |= ATTR_ITALIC;
        } else {
            self.current_attrs &= !ATTR_ITALIC;
        }
    }

    pub fn set_underline(&mut self, on: bool) {
        if on {
            self.current_attrs |= ATTR_UNDERLINE;
        } else {
            self.current_attrs &= !ATTR_UNDERLINE;
        }
    }

    pub fn set_inverse(&mut self, on: bool) {
        if on {
            self.current_attrs |= ATTR_INVERSE;
        } else {
            self.current_attrs &= !ATTR_INVERSE;
        }
    }

    pub fn set_strikethrough(&mut self, on: bool) {
        if on {
            self.current_attrs |= ATTR_STRIKETHROUGH;
        } else {
            self.current_attrs &= !ATTR_STRIKETHROUGH;
        }
    }

    pub fn set_fg(&mut self, color: u32) {
        self.current_fg = color;
    }

    pub fn set_bg(&mut self, color: u32) {
        self.current_bg = color;
    }

    pub fn set_fg_rgb(&mut self, r: u8, g: u8, b: u8) {
        self.current_fg = COLOR_FLAG_RGB | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
    }

    pub fn set_bg_rgb(&mut self, r: u8, g: u8, b: u8) {
        self.current_bg = COLOR_FLAG_RGB | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
    }
}

#[cfg(test)]
mod tests {
    use super::Grid;

    #[test]
    fn writes_simple_text() {
        let mut g = Grid::new(8, 2, [1, 2, 3], [0, 0, 0]);
        g.write_bytes(b"abc");
        assert_eq!(g.cell_char(0, 0), 'a');
        assert_eq!(g.cell_char(0, 1), 'b');
        assert_eq!(g.cell_char(0, 2), 'c');
    }

    #[test]
    fn wraps_and_scrolls() {
        let mut g = Grid::new(4, 2, [1, 2, 3], [0, 0, 0]);
        g.write_bytes(b"abcdefghij");
        assert_eq!(g.cell_char(0, 0), 'e');
        assert_eq!(g.cell_char(1, 0), 'i');
    }

    #[test]
    fn cursor_movement() {
        let mut g = Grid::new(10, 5, [0, 0, 0], [0, 0, 0]);
        g.set_cursor(2, 3);
        g.put_char(b'X' as u32);
        assert_eq!(g.cell_char(2, 3), 'X');
    }

    #[test]
    fn erase_line() {
        let mut g = Grid::new(10, 2, [0, 0, 0], [0, 0, 0]);
        g.write_bytes(b"helloworld");
        g.set_cursor(0, 3);
        g.erase_line_right();
        assert_eq!(g.cell_char(0, 0), 'h');
        assert_eq!(g.cell_char(0, 2), 'l');
        assert_eq!(g.cell_char(0, 3), ' ');
        assert_eq!(g.cell_char(0, 9), ' ');
    }

    #[test]
    fn cell_is_16_bytes() {
        assert_eq!(std::mem::size_of::<super::Cell>(), 16);
    }

    #[test]
    fn writes_utf8_codepoints() {
        let mut g = super::Grid::new(80, 24, [0xff; 3], [0; 3]);
        g.write_bytes("héllo".as_bytes());
        assert_eq!(g.cell_char(0, 0), 'h');
        assert_eq!(g.cell_at(0, 1).ch, 0xe9);
        assert_eq!(g.cell_char(0, 2), 'l');
        assert_eq!(g.cell_char(0, 3), 'l');
        assert_eq!(g.cell_char(0, 4), 'o');
    }

    #[test]
    fn writes_3byte_utf8() {
        let mut g = super::Grid::new(80, 24, [0xff; 3], [0; 3]);
        let input = b"A\xe2\x80\x93B";
        g.write_bytes(input);
        assert_eq!(g.cell_char(0, 0), 'A');
        assert_eq!(g.cell_at(0, 1).ch, 0x2013);
        assert_eq!(g.cell_char(0, 2), 'B');
    }

    #[test]
    fn writes_4byte_utf8_emoji() {
        let mut g = super::Grid::new(80, 24, [0xff; 3], [0; 3]);
        g.write_bytes("😀".as_bytes());
        assert_eq!(g.cell_at(0, 0).ch, 0x1F600);
    }
}
