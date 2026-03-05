use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct HexColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl HexColor {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub const fn as_u32_rgb(self) -> u32 {
        ((self.r as u32) << 16) | ((self.g as u32) << 8) | (self.b as u32)
    }
}

impl fmt::Display for HexColor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

impl FromStr for HexColor {
    type Err = String;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let s = input.trim();
        let raw = s
            .strip_prefix('#')
            .ok_or_else(|| format!("expected #RRGGBB, got {s}"))?;

        if raw.len() != 6 {
            return Err(format!("expected 6 hex digits, got {s}"));
        }

        let r = u8::from_str_radix(&raw[0..2], 16)
            .map_err(|_| format!("invalid red component in {s}"))?;
        let g = u8::from_str_radix(&raw[2..4], 16)
            .map_err(|_| format!("invalid green component in {s}"))?;
        let b = u8::from_str_radix(&raw[4..6], 16)
            .map_err(|_| format!("invalid blue component in {s}"))?;

        Ok(Self { r, g, b })
    }
}

impl TryFrom<String> for HexColor {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<HexColor> for String {
    fn from(value: HexColor) -> Self {
        value.to_string()
    }
}

const PALETTE: [u32; 16] = [
    0x000000, 0xcc0000, 0x4e9a06, 0xc4a000, 0x3465a4, 0x75507b, 0x06989a, 0xd3d7cf,
    0x555753, 0xef2929, 0x8ae234, 0xfce94f, 0x729fcf, 0xad7fa8, 0x34e2e2, 0xeeeeec,
];

pub fn to_rgb(c: u32) -> u32 {
    use crate::grid::COLOR_FLAG_RGB;
    if c & COLOR_FLAG_RGB != 0 {
        c & 0x00FF_FFFF
    } else {
        let idx = c as u8;
        if (idx as usize) < PALETTE.len() {
            PALETTE[idx as usize]
        } else if (16..232).contains(&idx) {
            let v = idx - 16;
            let r = (v / 36) * 51;
            let g = ((v % 36) / 6) * 51;
            let b = (v % 6) * 51;
            ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
        } else if idx >= 232 {
            let v = 8 + (idx - 232) as u32 * 10;
            (v << 16) | (v << 8) | v
        } else {
            0xffffff
        }
    }
}

#[cfg(test)]
mod tests {
    use super::HexColor;

    #[test]
    fn parses_valid_hex_color() {
        let c: HexColor = "#cdd6f4".parse().expect("color should parse");
        assert_eq!(c.r, 0xcd);
        assert_eq!(c.g, 0xd6);
        assert_eq!(c.b, 0xf4);
    }

    #[test]
    fn rejects_invalid_color() {
        let err = "#zz0000"
            .parse::<HexColor>()
            .expect_err("invalid color should fail");
        assert!(err.contains("invalid red component"));
    }
}
