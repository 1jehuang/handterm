#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct Cell {
    pub ch: u32,
    pub fg: u8,
    pub bg: u8,
    pub attrs: u8,
    pub flags: u8,
}

impl Cell {
    pub const BLANK: Self = Self {
        ch: b' ' as u32,
        fg: 0,
        bg: 0,
        attrs: 0,
        flags: 0,
    };

    #[cfg(test)]
    fn char_display(&self) -> char {
        char::from_u32(self.ch).unwrap_or(' ')
    }
}

pub struct Grid {
    cols: usize,
    rows: usize,
    cursor_col: usize,
    cursor_row: usize,
    cells: Vec<Cell>,
    top_row: usize,
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
        }
    }

    #[allow(dead_code)]
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols.max(1) as usize;
        self.rows = rows.max(1) as usize;
        self.cursor_col = 0;
        self.cursor_row = 0;
        self.top_row = 0;
        self.cells = vec![Cell::BLANK; self.cols * self.rows];
    }

    #[cfg(test)]
    pub fn cursor(&self) -> (usize, usize) {
        (self.cursor_col, self.cursor_row)
    }

    #[cfg(test)]
    pub fn clear(&mut self) {
        self.cells.fill(Cell::BLANK);
        self.cursor_col = 0;
        self.cursor_row = 0;
        self.top_row = 0;
    }

    #[inline(always)]
    fn physical_row(&self, logical_row: usize) -> usize {
        (self.top_row + logical_row) % self.rows
    }

    #[cfg(test)]
    fn cell_index(&self, logical_row: usize, col: usize) -> usize {
        self.physical_row(logical_row) * self.cols + col
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
            } else {
                match b {
                    b'\n' => self.new_line(),
                    b'\r' => self.cursor_col = 0,
                    b'\t' => {
                        let next = ((self.cursor_col / 8) + 1) * 8;
                        self.cursor_col = next.min(self.cols.saturating_sub(1));
                    }
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
            let dest = &mut self.cells[dest_start..dest_start + chunk_len];
            let src = &run[ri..ri + chunk_len];

            for (cell, &byte) in dest.iter_mut().zip(src.iter()) {
                cell.ch = byte as u32;
            }

            ri += chunk_len;
            self.cursor_col += chunk_len;

            if self.cursor_col >= self.cols {
                self.new_line();
            }
        }
    }

    #[cfg(test)]
    pub fn write_str(&mut self, text: &str) {
        self.write_bytes(text.as_bytes());
    }

    #[cfg(test)]
    pub fn get(&self, row: usize, col: usize) -> Option<&Cell> {
        if row >= self.rows || col >= self.cols {
            return None;
        }
        let idx = self.cell_index(row, col);
        self.cells.get(idx)
    }

    #[cfg(test)]
    pub fn rows_as_strings(&self) -> Vec<String> {
        let mut out = Vec::with_capacity(self.rows);
        for row in 0..self.rows {
            let phys = self.physical_row(row);
            let start = phys * self.cols;
            let end = start + self.cols;
            let mut s = String::with_capacity(self.cols);
            for cell in &self.cells[start..end] {
                s.push(cell.char_display());
            }
            out.push(s);
        }
        out
    }

    #[inline(always)]
    fn new_line(&mut self) {
        self.cursor_col = 0;
        if self.cursor_row + 1 >= self.rows {
            self.scroll_up();
        } else {
            self.cursor_row += 1;
        }
    }

    #[inline(always)]
    fn scroll_up(&mut self) {
        if self.rows == 0 || self.cols == 0 {
            return;
        }

        let old_top = self.physical_row(0);
        let blank_start = old_top * self.cols;
        let blank_end = blank_start + self.cols;
        self.cells[blank_start..blank_end].fill(Cell::BLANK);

        self.top_row = (self.top_row + 1) % self.rows;
    }
}

#[cfg(test)]
mod tests {
    use super::Grid;

    #[test]
    fn writes_simple_text() {
        let mut g = Grid::new(8, 2, [1, 2, 3], [0, 0, 0]);
        g.write_str("abc");
        assert_eq!(g.get(0, 0).expect("cell").ch, b'a' as u32);
        assert_eq!(g.get(0, 1).expect("cell").ch, b'b' as u32);
        assert_eq!(g.get(0, 2).expect("cell").ch, b'c' as u32);
    }

    #[test]
    fn wraps_and_scrolls() {
        let mut g = Grid::new(4, 2, [1, 2, 3], [0, 0, 0]);
        g.write_str("abcdefghij");
        let rows = g.rows_as_strings();
        assert_eq!(rows[0], "efgh");
        assert_eq!(rows[1], "ij  ");
    }

    #[test]
    fn clears_grid() {
        let mut g = Grid::new(4, 2, [1, 2, 3], [0, 0, 0]);
        g.write_str("hi");
        g.clear();
        let rows = g.rows_as_strings();
        assert_eq!(rows[0], "    ");
        assert_eq!(rows[1], "    ");
        assert_eq!(g.cursor(), (0, 0));
    }

    #[test]
    fn cell_is_8_bytes() {
        assert_eq!(std::mem::size_of::<super::Cell>(), 8);
    }
}
