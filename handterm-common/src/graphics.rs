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
