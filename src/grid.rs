#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: [u8; 3],
    pub bg: [u8; 3],
}

impl Cell {
    pub const fn blank(fg: [u8; 3], bg: [u8; 3]) -> Self {
        Self { ch: ' ', fg, bg }
    }
}

pub struct Grid {
    cols: u16,
    rows: u16,
    cursor_col: u16,
    cursor_row: u16,
    cells: Vec<Cell>,
    default_fg: [u8; 3],
    default_bg: [u8; 3],
}

impl Grid {
    pub fn new(cols: u16, rows: u16, default_fg: [u8; 3], default_bg: [u8; 3]) -> Self {
        let blank = Cell::blank(default_fg, default_bg);
        let len = usize::from(cols) * usize::from(rows);
        Self {
            cols,
            rows,
            cursor_col: 0,
            cursor_row: 0,
            cells: vec![blank; len],
            default_fg,
            default_bg,
        }
    }

    #[allow(dead_code)]
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols.max(1);
        self.rows = rows.max(1);
        self.cursor_col = 0;
        self.cursor_row = 0;
        let blank = Cell::blank(self.default_fg, self.default_bg);
        self.cells = vec![blank; usize::from(self.cols) * usize::from(self.rows)];
    }

    #[cfg(test)]
    pub fn cursor(&self) -> (u16, u16) {
        (self.cursor_col, self.cursor_row)
    }

    #[cfg(test)]
    pub fn clear(&mut self) {
        let blank = Cell::blank(self.default_fg, self.default_bg);
        self.cells.fill(blank);
        self.cursor_col = 0;
        self.cursor_row = 0;
    }

    pub fn write_bytes(&mut self, bytes: &[u8]) {
        for b in bytes {
            match b {
                b'\n' => self.new_line(),
                b'\r' => self.cursor_col = 0,
                b'\t' => {
                    // Fast tab approximation to avoid parser complexity this early.
                    let next = ((self.cursor_col / 8) + 1) * 8;
                    self.cursor_col = next.min(self.cols.saturating_sub(1));
                }
                0x20..=0x7e => self.put_char(char::from(*b)),
                _ => {}
            }
        }
    }

    #[cfg(test)]
    pub fn write_str(&mut self, text: &str) {
        self.write_bytes(text.as_bytes());
    }

    #[cfg(test)]
    pub fn get(&self, row: u16, col: u16) -> Option<&Cell> {
        if row >= self.rows || col >= self.cols {
            return None;
        }
        let idx = usize::from(row) * usize::from(self.cols) + usize::from(col);
        self.cells.get(idx)
    }

    #[cfg(test)]
    pub fn rows_as_strings(&self) -> Vec<String> {
        let mut out = Vec::with_capacity(self.rows as usize);
        for row in 0..self.rows {
            let start = usize::from(row) * usize::from(self.cols);
            let end = start + usize::from(self.cols);
            let mut s = String::with_capacity(self.cols as usize);
            for cell in &self.cells[start..end] {
                s.push(cell.ch);
            }
            out.push(s);
        }
        out
    }

    fn put_char(&mut self, ch: char) {
        if self.cursor_row >= self.rows || self.cursor_col >= self.cols {
            return;
        }

        let idx =
            usize::from(self.cursor_row) * usize::from(self.cols) + usize::from(self.cursor_col);
        if let Some(cell) = self.cells.get_mut(idx) {
            cell.ch = ch;
            cell.fg = self.default_fg;
            cell.bg = self.default_bg;
        }

        self.cursor_col += 1;
        if self.cursor_col >= self.cols {
            self.new_line();
        }
    }

    fn new_line(&mut self) {
        self.cursor_col = 0;
        if self.cursor_row + 1 >= self.rows {
            self.scroll_up();
        } else {
            self.cursor_row += 1;
        }
    }

    fn scroll_up(&mut self) {
        let row_width = usize::from(self.cols);
        if self.rows == 0 || self.cols == 0 {
            return;
        }

        let total = self.cells.len();
        self.cells.copy_within(row_width..total, 0);

        let blank = Cell::blank(self.default_fg, self.default_bg);
        let tail_start = total - row_width;
        self.cells[tail_start..].fill(blank);
    }
}

#[cfg(test)]
mod tests {
    use super::Grid;

    #[test]
    fn writes_simple_text() {
        let mut g = Grid::new(8, 2, [1, 2, 3], [0, 0, 0]);
        g.write_str("abc");
        assert_eq!(g.get(0, 0).expect("cell").ch, 'a');
        assert_eq!(g.get(0, 1).expect("cell").ch, 'b');
        assert_eq!(g.get(0, 2).expect("cell").ch, 'c');
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
}
