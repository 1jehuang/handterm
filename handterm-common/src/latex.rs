use std::fmt;

/// Prefix for Handterm's private LaTeX APC payload.
///
/// The complete wire form is `ESC _ L ; <UTF-8 LaTeX> ESC \\`.
pub const LATEX_APC_PREFIX: &[u8] = b"L;";

/// A terminal-cell layout produced from one LaTeX math body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LatexLayout {
    lines: Vec<String>,
    baseline: usize,
    width: usize,
}

impl LatexLayout {
    /// Rendered lines, padded to a consistent terminal-cell width.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// The line containing the mathematical baseline.
    pub const fn baseline(&self) -> usize {
        self.baseline
    }

    /// Width of every rendered line in terminal display cells.
    pub const fn width(&self) -> usize {
        self.width
    }

    /// Materialize the layout as newline-separated terminal text.
    pub fn as_text(&self) -> String {
        self.lines.join("\n")
    }
}

/// Failure to encode or render a Handterm LaTeX payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LatexError {
    InvalidUtf8,
    ControlByte(u8),
    Unsupported(String),
}

impl fmt::Display for LatexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUtf8 => f.write_str("LaTeX payload is not valid UTF-8"),
            Self::ControlByte(byte) => {
                write!(
                    f,
                    "LaTeX source contains reserved control byte 0x{byte:02x}"
                )
            }
            Self::Unsupported(message) => write!(f, "LaTeX could not be rendered: {message}"),
        }
    }
}

impl std::error::Error for LatexError {}

/// Render a UTF-8 LaTeX math body as terminal-friendly Unicode cells.
pub fn render_latex(source: &[u8]) -> Result<LatexLayout, LatexError> {
    let source = std::str::from_utf8(source).map_err(|_| LatexError::InvalidUtf8)?;
    render_latex_str(source)
}

/// Render a LaTeX math body as terminal-friendly Unicode cells.
pub fn render_latex_str(source: &str) -> Result<LatexLayout, LatexError> {
    let rendered = mdwright_latex::render_unicode_math(source)
        .map_err(|error| LatexError::Unsupported(error.to_string()))?;
    Ok(LatexLayout {
        lines: rendered.lines().to_vec(),
        baseline: rendered.baseline(),
        width: rendered.width(),
    })
}

/// Encode source for Handterm's private LaTeX APC protocol.
///
/// ESC and BEL are rejected because they terminate APC control strings.
pub fn encode_latex_apc(source: &str) -> Result<Vec<u8>, LatexError> {
    if let Some(byte) = source.bytes().find(|byte| matches!(byte, 0x07 | 0x1b)) {
        return Err(LatexError::ControlByte(byte));
    }

    let mut encoded = Vec::with_capacity(source.len() + LATEX_APC_PREFIX.len() + 4);
    encoded.extend_from_slice(b"\x1b_");
    encoded.extend_from_slice(LATEX_APC_PREFIX);
    encoded.extend_from_slice(source.as_bytes());
    encoded.extend_from_slice(b"\x1b\\");
    Ok(encoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_fraction_as_terminal_grid() {
        let layout = render_latex_str(r"\frac{a}{b}").expect("fraction should render");
        assert_eq!(layout.lines(), &["a", "─", "b"]);
        assert_eq!(layout.baseline(), 1);
        assert_eq!(layout.width(), 1);
    }

    #[test]
    fn renders_symbols_scripts_and_matrices() {
        assert_eq!(
            render_latex_str(r"\alpha_i^2")
                .expect("scripts should render")
                .lines(),
            &["α²ᵢ"]
        );
        assert_eq!(
            render_latex_str(r"\begin{pmatrix}a & bb \\ c & d\end{pmatrix}")
                .expect("matrix should render")
                .lines(),
            &["(a  bb)", "(c  d )"]
        );
    }

    #[test]
    fn apc_encoding_roundtrips_source_bytes() {
        let encoded = encode_latex_apc(r"\sqrt{x^2+y^2}").expect("source should encode");
        assert_eq!(encoded, b"\x1b_L;\\sqrt{x^2+y^2}\x1b\\");
    }

    #[test]
    fn apc_encoding_rejects_terminating_control_bytes() {
        assert_eq!(
            encode_latex_apc("x\x1by"),
            Err(LatexError::ControlByte(0x1b))
        );
        assert_eq!(
            encode_latex_apc("x\x07y"),
            Err(LatexError::ControlByte(0x07))
        );
    }

    #[test]
    fn malformed_or_unsupported_source_returns_an_error() {
        assert!(render_latex_str(r"\frac{a}").is_err());
        assert!(render_latex_str(r"\color{red}{x}").is_err());
        assert_eq!(render_latex(b"\xff"), Err(LatexError::InvalidUtf8));
    }
}
