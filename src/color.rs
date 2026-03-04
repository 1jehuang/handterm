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
