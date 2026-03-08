use crate::grid::{
    ATTR_BOLD, ATTR_DIM, ATTR_HAS_UCOLOR, ATTR_INVERSE, COLOR_DEFAULT, COLOR_FLAG_RGB, Cell,
    Selection,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellColors {
    pub fg: u32,
    pub bg: u32,
}

#[inline]
pub fn resolve_cell_colors(
    cell: &Cell,
    base_fg: u32,
    base_bg: u32,
    is_cursor_block: bool,
    is_selected: bool,
) -> CellColors {
    let mut fg = if cell.fg == COLOR_DEFAULT {
        base_fg
    } else {
        crate::color::to_rgb(cell.fg)
    };
    let mut bg = if cell.bg == COLOR_DEFAULT {
        base_bg
    } else {
        crate::color::to_rgb(cell.bg)
    };

    if cell.attrs & ATTR_BOLD != 0
        && cell.fg != COLOR_DEFAULT
        && (cell.fg & COLOR_FLAG_RGB == 0)
        && (cell.fg as u8) < 8
    {
        fg = crate::color::to_rgb(cell.fg + 8);
    }
    if cell.attrs & ATTR_DIM != 0 {
        let r = ((fg >> 16) & 0xff) * 2 / 3;
        let g = ((fg >> 8) & 0xff) * 2 / 3;
        let b = (fg & 0xff) * 2 / 3;
        fg = (r << 16) | (g << 8) | b;
    }
    if cell.attrs & ATTR_INVERSE != 0 {
        std::mem::swap(&mut fg, &mut bg);
    }
    if is_cursor_block || is_selected {
        std::mem::swap(&mut fg, &mut bg);
    }

    CellColors { fg, bg }
}

#[inline]
pub fn resolve_underline_color(cell: &Cell, actual_fg: u32) -> u32 {
    if cell.attrs & ATTR_HAS_UCOLOR != 0 {
        crate::color::to_rgb(cell.underline_color)
    } else {
        actual_fg
    }
}

#[inline]
pub fn is_in_selection(selection: Option<Selection>, row: usize, col: usize) -> bool {
    let Some(sel) = selection else {
        return false;
    };
    let (sr, sc, er, ec) = if sel.start_row < sel.end_row
        || (sel.start_row == sel.end_row && sel.start_col <= sel.end_col)
    {
        (sel.start_row, sel.start_col, sel.end_row, sel.end_col)
    } else {
        (sel.end_row, sel.end_col, sel.start_row, sel.start_col)
    };
    if row < sr || row > er {
        false
    } else if row == sr && row == er {
        col >= sc && col <= ec
    } else if row == sr {
        col >= sc
    } else if row == er {
        col <= ec
    } else {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{ATTR_UNDERLINE, UnderlineStyle};

    #[test]
    fn selection_swaps_resolved_colors() {
        let mut cell = Cell::BLANK;
        cell.ch = 'x' as u32;
        cell.fg = 2;
        cell.bg = 4;

        let normal = resolve_cell_colors(&cell, 0xffffff, 0x000000, false, false);
        let selected = resolve_cell_colors(&cell, 0xffffff, 0x000000, false, true);

        assert_eq!(selected.fg, normal.bg);
        assert_eq!(selected.bg, normal.fg);
    }

    #[test]
    fn bold_and_dim_follow_terminal_rules() {
        let mut cell = Cell::BLANK;
        cell.ch = 'x' as u32;
        cell.fg = 1;
        cell.attrs = ATTR_BOLD | ATTR_DIM;

        let colors = resolve_cell_colors(&cell, 0xffffff, 0x000000, false, false);
        assert_ne!(colors.fg, crate::color::to_rgb(1));

        cell.attrs = ATTR_INVERSE;
        let inverse = resolve_cell_colors(&cell, 0xffffff, 0x123456, false, false);
        assert_eq!(inverse.bg, crate::color::to_rgb(1));
        assert_eq!(inverse.fg, 0x123456);
    }

    #[test]
    fn custom_underline_color_overrides_foreground() {
        let mut cell = Cell::BLANK;
        cell.ch = 'x' as u32;
        cell.underline_color = 0x8000_00ff;
        cell.attrs = ATTR_UNDERLINE | ATTR_HAS_UCOLOR;
        cell.underline_style = UnderlineStyle::Single;

        assert_eq!(resolve_underline_color(&cell, 0x112233), 0x0000ff);
    }

    #[test]
    fn selection_range_handles_reverse_drag() {
        let selection = Selection {
            start_col: 5,
            start_row: 3,
            end_col: 2,
            end_row: 1,
        };

        assert!(is_in_selection(Some(selection), 1, 2));
        assert!(is_in_selection(Some(selection), 2, 4));
        assert!(is_in_selection(Some(selection), 3, 5));
        assert!(!is_in_selection(Some(selection), 0, 0));
        assert!(!is_in_selection(Some(selection), 3, 6));
    }
}
