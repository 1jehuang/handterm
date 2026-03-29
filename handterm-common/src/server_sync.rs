use crate::grid::{Cell, CellSnapshot, Grid, UnderlineStyle};
use crate::protocol::{CursorState, DirtyCell, KittyImageData, KittyImagePlacement};
use crate::terminal::{CursorStyle, KittyImage, KittyPlacement};

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

    let grapheme = dirty.grapheme.clone().map(Into::into);
    grid.set_cell_with_grapheme(
        dirty.row as usize,
        dirty.col as usize,
        Cell::from_snapshot(CellSnapshot {
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
