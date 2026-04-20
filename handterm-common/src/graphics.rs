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

#[derive(Debug, Clone, Copy)]
pub struct KittyImageFinalize {
    pub id: u32,
    pub compression: Option<u8>,
    pub format: u32,
    pub width: u32,
    pub height: u32,
    pub action: u8,
    pub cols: u32,
    pub rows_param: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct KittyGraphicsCommand {
    pub image_id: u32,
    pub delete: Option<u8>,
    pub quiet: u8,
}
