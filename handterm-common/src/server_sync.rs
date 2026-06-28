use crate::graphics::{KittyImage, KittyPlacement};
use crate::grid::{Cell, CellSnapshot, Grid, UnderlineStyle};
use crate::protocol::{CursorState, DirtyCell, KittyImageData, KittyImagePlacement};
use crate::terminal::CursorStyle;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AppliedServerEffects {
    pub title: Option<String>,
    pub clipboard: Option<Vec<u8>>,
    pub bell: bool,
    pub closed: Option<Option<i32>>,
}

pub fn apply_dirty_cell(grid: &mut Grid, dirty: &DirtyCell) {
    let underline_style = match dirty.underline_style {
        1 => UnderlineStyle::Single,
        2 => UnderlineStyle::Double,
        3 => UnderlineStyle::Curly,
        4 => UnderlineStyle::Dotted,
        5 => UnderlineStyle::Dashed,
        _ => UnderlineStyle::None,
    };

    // `Cell::from_snapshot` discards the snapshot's grapheme field, so we keep it
    // out of the snapshot entirely and hand the single owned copy straight to the
    // grid. This avoids the previous double clone of the grapheme string (one for
    // the snapshot, one for the grid) on every grapheme-bearing cell.
    let grapheme = dirty.grapheme.as_deref().map(Box::<str>::from);
    grid.set_cell_with_grapheme(
        dirty.row as usize,
        dirty.col as usize,
        Cell::from_snapshot(CellSnapshot {
            ch: dirty.ch,
            grapheme: None,
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

pub fn apply_cursor_state(
    grid: &mut Grid,
    cursor_visible: &mut bool,
    cursor_style: &mut CursorStyle,
    cursor: Option<&CursorState>,
) {
    match cursor {
        Some(cursor) => {
            grid.set_cursor(cursor.row as usize, cursor.col as usize);
            *cursor_visible = cursor.visible;
            *cursor_style = match cursor.style {
                1 => CursorStyle::Underline,
                2 => CursorStyle::Bar,
                _ => CursorStyle::Block,
            };
        }
        None => {
            *cursor_visible = false;
        }
    }
}

pub fn kitty_images_from_wire(images: &[KittyImageData]) -> Vec<KittyImage> {
    images
        .iter()
        .map(|image| KittyImage {
            id: image.id,
            width: image.width,
            height: image.height,
            data: image.data.clone(),
        })
        .collect()
}

pub fn kitty_placements_from_wire(placements: &[KittyImagePlacement]) -> Vec<KittyPlacement> {
    placements
        .iter()
        .map(|placement| KittyPlacement {
            image_id: placement.image_id,
            col: placement.col as usize,
            row: placement.row as usize,
            cols: placement.cols as usize,
            rows: placement.rows as usize,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirty(row: u16, col: u16, ch: u32, grapheme: Option<&str>) -> DirtyCell {
        DirtyCell {
            row,
            col,
            ch,
            grapheme: grapheme.map(str::to_string),
            fg: 0x0011_2233,
            bg: 0x0044_5566,
            underline_color: 0x0077_8899,
            hyperlink_id: 5,
            attrs: 0x3,
            flags: 0x1,
            underline_style: 2,
        }
    }

    #[test]
    fn apply_dirty_cell_writes_plain_cell_fields() {
        let mut grid = Grid::new(10, 4, [0xff; 3], [0; 3]);
        apply_dirty_cell(&mut grid, &dirty(1, 2, 'h' as u32, None));

        let cell = grid.cell_at(1, 2);
        assert_eq!(cell.ch, 'h' as u32);
        assert_eq!(cell.fg, 0x0011_2233);
        assert_eq!(cell.bg, 0x0044_5566);
        assert_eq!(cell.underline_color, 0x0077_8899);
        assert_eq!(cell.hyperlink_id, 5);
        assert_eq!(cell.attrs, 0x3);
        assert_eq!(cell.flags, 0x1);
        assert_eq!(cell.underline_style, UnderlineStyle::Double);
        assert_eq!(grid.cell_grapheme_at(1, 2), None);
    }

    #[test]
    fn apply_dirty_cell_stores_grapheme_cluster() {
        let mut grid = Grid::new(10, 4, [0xff; 3], [0; 3]);
        apply_dirty_cell(&mut grid, &dirty(0, 0, '❤' as u32, Some("❤️")));
        assert_eq!(grid.cell_at(0, 0).ch, '❤' as u32);
        assert_eq!(grid.cell_grapheme_at(0, 0), Some("❤️"));
    }

    #[test]
    fn apply_dirty_cell_clears_grapheme_when_absent() {
        let mut grid = Grid::new(10, 4, [0xff; 3], [0; 3]);
        apply_dirty_cell(&mut grid, &dirty(0, 0, '❤' as u32, Some("❤️")));
        assert_eq!(grid.cell_grapheme_at(0, 0), Some("❤️"));
        // A subsequent plain write to the same cell must drop the grapheme.
        apply_dirty_cell(&mut grid, &dirty(0, 0, 'x' as u32, None));
        assert_eq!(grid.cell_grapheme_at(0, 0), None);
    }

    #[test]
    fn apply_cursor_state_sets_position_visibility_and_style() {
        let mut grid = Grid::new(10, 4, [0xff; 3], [0; 3]);
        let mut visible = false;
        let mut style = CursorStyle::Block;
        let cursor = CursorState {
            row: 2,
            col: 3,
            style: 2,
            visible: true,
        };
        apply_cursor_state(&mut grid, &mut visible, &mut style, Some(&cursor));
        assert_eq!(grid.cursor_pos(), (3, 2));
        assert!(visible);
        assert_eq!(style, CursorStyle::Bar);

        apply_cursor_state(&mut grid, &mut visible, &mut style, None);
        assert!(!visible);
    }

    #[test]
    fn kitty_state_converts_from_wire() {
        let images = vec![KittyImageData {
            id: 7,
            width: 2,
            height: 1,
            data: vec![1, 2, 3, 4, 5, 6, 7, 8],
        }];
        let placements = vec![KittyImagePlacement {
            image_id: 7,
            col: 1,
            row: 2,
            cols: 3,
            rows: 4,
        }];

        let out_images = kitty_images_from_wire(&images);
        assert_eq!(out_images.len(), 1);
        assert_eq!(out_images[0].id, 7);
        assert_eq!(out_images[0].width, 2);
        assert_eq!(out_images[0].data, images[0].data);

        let out_placements = kitty_placements_from_wire(&placements);
        assert_eq!(out_placements.len(), 1);
        assert_eq!(out_placements[0].image_id, 7);
        assert_eq!(out_placements[0].col, 1);
        assert_eq!(out_placements[0].row, 2);
        assert_eq!(out_placements[0].cols, 3);
        assert_eq!(out_placements[0].rows, 4);
    }
}
