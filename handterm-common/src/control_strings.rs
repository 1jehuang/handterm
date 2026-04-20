#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SixelEvent {
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DcsEvent {
    Generic(Vec<u8>),
    Sixel(SixelEvent),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApcEvent {
    Generic(Vec<u8>),
    KittyGraphics(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OscEvent {
    Raw(Vec<u8>),
    Title { raw: Vec<u8>, title: String },
    Clipboard { raw: Vec<u8>, data: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlStringEvent {
    Osc(OscEvent),
    Dcs(DcsEvent),
    Apc(ApcEvent),
}
