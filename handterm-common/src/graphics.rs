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

#[derive(Debug, Clone, Default)]
pub struct KittyUploadState {
    pub payload_buf: Vec<u8>,
    pub pending_id: u32,
    pub pending_fmt: u32,
    pub pending_width: u32,
    pub pending_height: u32,
    pub pending_compression: Option<u8>,
    pub more_chunks: bool,
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

/// Why a kitty graphics payload could not be decoded into RGBA pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KittyImageDecodeError {
    /// The transmitted payload was not valid base64.
    InvalidBase64,
    /// Zlib decompression of the payload failed.
    Decompression,
    /// An unknown compression flag was supplied.
    UnsupportedCompression,
    /// The `f=` format parameter is not one we support (24, 32, or 100).
    UnsupportedFormat,
    /// Width/height were zero or did not match the payload length.
    InvalidDimensions,
    /// The embedded PNG failed to parse or used an unsupported color type.
    InvalidPng,
}

impl std::fmt::Display for KittyImageDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            Self::InvalidBase64 => "invalid base64 payload",
            Self::Decompression => "zlib decompression failed",
            Self::UnsupportedCompression => "unsupported compression flag",
            Self::UnsupportedFormat => "unsupported image format",
            Self::InvalidDimensions => "invalid or mismatched image dimensions",
            Self::InvalidPng => "invalid or unsupported PNG data",
        };
        f.write_str(msg)
    }
}

impl std::error::Error for KittyImageDecodeError {}

pub fn decode_kitty_image_payload(
    format: u32,
    compression: Option<u8>,
    payload: &[u8],
    width: u32,
    height: u32,
) -> Result<(u32, u32, Vec<u8>), KittyImageDecodeError> {
    let decoded = base64_decode_kitty(payload)?;
    let decoded = decompress_kitty_payload(decoded, compression)?;
    match format {
        24 => {
            if width == 0 || height == 0 {
                return Err(KittyImageDecodeError::InvalidDimensions);
            }
            let expected = (width as usize)
                .saturating_mul(height as usize)
                .saturating_mul(3);
            if decoded.len() != expected {
                return Err(KittyImageDecodeError::InvalidDimensions);
            }
            let mut rgba = Vec::with_capacity((width as usize) * (height as usize) * 4);
            for chunk in decoded.chunks_exact(3) {
                rgba.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 0xff]);
            }
            Ok((width, height, rgba))
        }
        32 => {
            if width == 0 || height == 0 {
                return Err(KittyImageDecodeError::InvalidDimensions);
            }
            let expected = (width as usize)
                .saturating_mul(height as usize)
                .saturating_mul(4);
            if decoded.len() != expected {
                return Err(KittyImageDecodeError::InvalidDimensions);
            }
            Ok((width, height, decoded))
        }
        100 => decode_png_kitty(&decoded),
        _ => Err(KittyImageDecodeError::UnsupportedFormat),
    }
}

fn base64_decode_kitty(input: &[u8]) -> Result<Vec<u8>, KittyImageDecodeError> {
    const TABLE: [u8; 256] = {
        let mut t = [0xffu8; 256];
        let mut i = 0u8;
        while i < 26 {
            t[(b'A' + i) as usize] = i;
            t[(b'a' + i) as usize] = i + 26;
            i += 1;
        }
        let mut d = 0u8;
        while d < 10 {
            t[(b'0' + d) as usize] = d + 52;
            d += 1;
        }
        t[b'+' as usize] = 62;
        t[b'/' as usize] = 63;
        t
    };
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0u32;
    for &b in input {
        if b == b'=' || b == b'\n' || b == b'\r' {
            continue;
        }
        let val = TABLE[b as usize];
        if val == 0xff {
            return Err(KittyImageDecodeError::InvalidBase64);
        }
        buf = (buf << 6) | val as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

fn decode_png_kitty(encoded_png: &[u8]) -> Result<(u32, u32, Vec<u8>), KittyImageDecodeError> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(encoded_png));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder
        .read_info()
        .map_err(|_| KittyImageDecodeError::InvalidPng)?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|_| KittyImageDecodeError::InvalidPng)?;
    let bytes = &buf[..info.buffer_size()];

    let rgba = match info.color_type {
        png::ColorType::Rgba => bytes.to_vec(),
        png::ColorType::Rgb => {
            let mut rgba = Vec::with_capacity((info.width as usize) * (info.height as usize) * 4);
            for chunk in bytes.chunks_exact(3) {
                rgba.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 0xff]);
            }
            rgba
        }
        png::ColorType::Grayscale => {
            let mut rgba = Vec::with_capacity((info.width as usize) * (info.height as usize) * 4);
            for &value in bytes {
                rgba.extend_from_slice(&[value, value, value, 0xff]);
            }
            rgba
        }
        png::ColorType::GrayscaleAlpha => {
            let mut rgba = Vec::with_capacity((info.width as usize) * (info.height as usize) * 4);
            for chunk in bytes.chunks_exact(2) {
                rgba.extend_from_slice(&[chunk[0], chunk[0], chunk[0], chunk[1]]);
            }
            rgba
        }
        _ => return Err(KittyImageDecodeError::InvalidPng),
    };

    Ok((info.width, info.height, rgba))
}

fn decompress_kitty_payload(
    decoded: Vec<u8>,
    compression: Option<u8>,
) -> Result<Vec<u8>, KittyImageDecodeError> {
    match compression {
        None => Ok(decoded),
        Some(b'z') => {
            let mut decoder = flate2::read::ZlibDecoder::new(decoded.as_slice());
            let mut out = Vec::new();
            std::io::Read::read_to_end(&mut decoder, &mut out)
                .map_err(|_| KittyImageDecodeError::Decompression)?;
            Ok(out)
        }
        Some(_) => Err(KittyImageDecodeError::UnsupportedCompression),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Standard base64 encoder (padding included) so tests exercise the
    /// module's hand-rolled decoder against independently produced input.
    fn base64_encode(data: &[u8]) -> Vec<u8> {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = Vec::with_capacity(data.len().div_ceil(3) * 4);
        for chunk in data.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
            let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
            let triple = (b0 << 16) | (b1 << 8) | b2;
            out.push(ALPHABET[(triple >> 18) as usize & 0x3f]);
            out.push(ALPHABET[(triple >> 12) as usize & 0x3f]);
            out.push(if chunk.len() > 1 {
                ALPHABET[(triple >> 6) as usize & 0x3f]
            } else {
                b'='
            });
            out.push(if chunk.len() > 2 {
                ALPHABET[triple as usize & 0x3f]
            } else {
                b'='
            });
        }
        out
    }

    fn zlib_compress(data: &[u8]) -> Vec<u8> {
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(data).expect("zlib write should succeed");
        encoder.finish().expect("zlib finish should succeed")
    }

    /// Encode `pixels` as a PNG of the given color type using the png crate.
    fn encode_png(width: u32, height: u32, color: png::ColorType, pixels: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, width, height);
            encoder.set_color(color);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("png header should write");
            writer
                .write_image_data(pixels)
                .expect("png data should write");
        }
        out
    }

    #[test]
    fn decodes_rgb_payload_and_appends_opaque_alpha() {
        // 2x1 image: red pixel then green pixel.
        let rgb = [0xff, 0x00, 0x00, 0x00, 0xff, 0x00];
        let payload = base64_encode(&rgb);

        let (w, h, rgba) =
            decode_kitty_image_payload(24, None, &payload, 2, 1).expect("rgb should decode");

        assert_eq!((w, h), (2, 1));
        assert_eq!(
            rgba,
            vec![0xff, 0x00, 0x00, 0xff, 0x00, 0xff, 0x00, 0xff],
            "each RGB pixel should gain an opaque alpha byte"
        );
    }

    #[test]
    fn decodes_rgba_payload_verbatim() {
        // 1x2 image with distinct alpha values that must survive untouched.
        let rgba = [0x10, 0x20, 0x30, 0x80, 0x40, 0x50, 0x60, 0x00];
        let payload = base64_encode(&rgba);

        let (w, h, out) =
            decode_kitty_image_payload(32, None, &payload, 1, 2).expect("rgba should decode");

        assert_eq!((w, h), (1, 2));
        assert_eq!(out, rgba.to_vec());
    }

    #[test]
    fn decodes_png_payload_of_each_supported_color_type() {
        // For every supported PNG color type the decoder must normalize to RGBA
        // and take dimensions from the PNG itself (the passed w/h are ignored
        // for f=100).
        let cases: [(png::ColorType, Vec<u8>, Vec<u8>); 4] = [
            (
                png::ColorType::Rgba,
                vec![1, 2, 3, 4, 5, 6, 7, 8],
                vec![1, 2, 3, 4, 5, 6, 7, 8],
            ),
            (
                png::ColorType::Rgb,
                vec![1, 2, 3, 4, 5, 6],
                vec![1, 2, 3, 0xff, 4, 5, 6, 0xff],
            ),
            (
                png::ColorType::Grayscale,
                vec![9, 200],
                vec![9, 9, 9, 0xff, 200, 200, 200, 0xff],
            ),
            (
                png::ColorType::GrayscaleAlpha,
                vec![9, 100, 200, 50],
                vec![9, 9, 9, 100, 200, 200, 200, 50],
            ),
        ];

        for (color, pixels, expected_rgba) in cases {
            let png_bytes = encode_png(2, 1, color, &pixels);
            let payload = base64_encode(&png_bytes);
            let (w, h, rgba) = decode_kitty_image_payload(100, None, &payload, 0, 0)
                .unwrap_or_else(|e| panic!("{color:?} png should decode: {e}"));
            assert_eq!((w, h), (2, 1), "{color:?} dimensions come from the PNG");
            assert_eq!(rgba, expected_rgba, "{color:?} should normalize to RGBA");
        }
    }

    #[test]
    fn decodes_zlib_compressed_rgb_payload() {
        let rgb = [0xaa, 0xbb, 0xcc];
        let payload = base64_encode(&zlib_compress(&rgb));

        let (w, h, rgba) = decode_kitty_image_payload(24, Some(b'z'), &payload, 1, 1)
            .expect("compressed rgb should decode");

        assert_eq!((w, h), (1, 1));
        assert_eq!(rgba, vec![0xaa, 0xbb, 0xcc, 0xff]);
    }

    #[test]
    fn decodes_zlib_compressed_png_payload() {
        let png_bytes = encode_png(1, 1, png::ColorType::Rgb, &[7, 8, 9]);
        let payload = base64_encode(&zlib_compress(&png_bytes));

        let (w, h, rgba) = decode_kitty_image_payload(100, Some(b'z'), &payload, 0, 0)
            .expect("compressed png should decode");

        assert_eq!((w, h), (1, 1));
        assert_eq!(rgba, vec![7, 8, 9, 0xff]);
    }

    #[test]
    fn base64_decoder_skips_padding_and_newlines() {
        // Kitty chunked uploads can interleave newlines; padding and CR/LF must
        // be ignored rather than rejected.
        let rgb = [0x01, 0x02, 0x03];
        let mut payload = base64_encode(&rgb);
        payload.insert(2, b'\n');
        payload.insert(5, b'\r');

        let (_, _, rgba) = decode_kitty_image_payload(24, None, &payload, 1, 1)
            .expect("payload with newlines should decode");
        assert_eq!(rgba, vec![0x01, 0x02, 0x03, 0xff]);
    }

    #[test]
    fn rejects_malformed_base64() {
        for payload in [&b"ab!d"[..], b"a b", b"\x00\x01", "é".as_bytes()] {
            assert_eq!(
                decode_kitty_image_payload(24, None, payload, 1, 1),
                Err(KittyImageDecodeError::InvalidBase64),
                "payload {payload:?} should be rejected as base64"
            );
        }
    }

    #[test]
    fn rejects_truncated_pixel_data() {
        // Declared 2x2 RGB needs 12 bytes; provide only one pixel.
        let payload = base64_encode(&[1, 2, 3]);
        assert_eq!(
            decode_kitty_image_payload(24, None, &payload, 2, 2),
            Err(KittyImageDecodeError::InvalidDimensions)
        );

        // RGBA path checks length too.
        let payload = base64_encode(&[1, 2, 3, 4]);
        assert_eq!(
            decode_kitty_image_payload(32, None, &payload, 2, 2),
            Err(KittyImageDecodeError::InvalidDimensions)
        );
    }

    #[test]
    fn rejects_excess_pixel_data() {
        // Payload longer than width*height*bpp must not be silently truncated.
        let payload = base64_encode(&[0u8; 8]);
        assert_eq!(
            decode_kitty_image_payload(24, None, &payload, 1, 1),
            Err(KittyImageDecodeError::InvalidDimensions)
        );
    }

    #[test]
    fn rejects_zero_dimensions() {
        let payload = base64_encode(&[]);
        for (w, h) in [(0, 1), (1, 0), (0, 0)] {
            for format in [24, 32] {
                assert_eq!(
                    decode_kitty_image_payload(format, None, &payload, w, h),
                    Err(KittyImageDecodeError::InvalidDimensions),
                    "f={format} w={w} h={h} should be rejected"
                );
            }
        }
    }

    #[test]
    fn oversized_declared_dimensions_do_not_overflow_or_allocate() {
        // width*height*bpp would overflow usize arithmetic without saturation;
        // the tiny payload must simply mismatch, not panic or OOM.
        let payload = base64_encode(&[1, 2, 3]);
        assert_eq!(
            decode_kitty_image_payload(24, None, &payload, u32::MAX, u32::MAX),
            Err(KittyImageDecodeError::InvalidDimensions)
        );
        assert_eq!(
            decode_kitty_image_payload(32, None, &payload, u32::MAX, u32::MAX),
            Err(KittyImageDecodeError::InvalidDimensions)
        );
    }

    #[test]
    fn rejects_unsupported_format() {
        let payload = base64_encode(&[1, 2, 3]);
        for format in [0, 1, 23, 33, 99, 101, u32::MAX] {
            assert_eq!(
                decode_kitty_image_payload(format, None, &payload, 1, 1),
                Err(KittyImageDecodeError::UnsupportedFormat),
                "f={format} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_unknown_compression_flag() {
        let payload = base64_encode(&[1, 2, 3]);
        assert_eq!(
            decode_kitty_image_payload(24, Some(b'q'), &payload, 1, 1),
            Err(KittyImageDecodeError::UnsupportedCompression)
        );
    }

    #[test]
    fn rejects_corrupt_zlib_stream() {
        // Valid base64 of bytes that are not a zlib stream.
        let payload = base64_encode(b"definitely not zlib");
        assert_eq!(
            decode_kitty_image_payload(24, Some(b'z'), &payload, 1, 1),
            Err(KittyImageDecodeError::Decompression)
        );
    }

    #[test]
    fn rejects_invalid_png_bytes() {
        // Garbage that is not a PNG at all.
        let payload = base64_encode(b"not a png");
        assert_eq!(
            decode_kitty_image_payload(100, None, &payload, 0, 0),
            Err(KittyImageDecodeError::InvalidPng)
        );

        // A real PNG truncated mid-stream must also fail cleanly.
        let png_bytes = encode_png(2, 2, png::ColorType::Rgba, &[0u8; 16]);
        let truncated = &png_bytes[..png_bytes.len() / 2];
        let payload = base64_encode(truncated);
        assert_eq!(
            decode_kitty_image_payload(100, None, &payload, 0, 0),
            Err(KittyImageDecodeError::InvalidPng)
        );
    }

    #[test]
    fn empty_payload_decodes_only_when_expected_empty() {
        // An empty payload is valid base64 (zero bytes) but can never satisfy
        // a nonzero pixel format.
        assert_eq!(
            decode_kitty_image_payload(24, None, b"", 1, 1),
            Err(KittyImageDecodeError::InvalidDimensions)
        );
        assert_eq!(
            decode_kitty_image_payload(100, None, b"", 0, 0),
            Err(KittyImageDecodeError::InvalidPng)
        );
    }

    #[test]
    fn decode_error_display_is_human_readable() {
        // The Display impl feeds user-facing logs; keep each variant nonempty
        // and distinct.
        let variants = [
            KittyImageDecodeError::InvalidBase64,
            KittyImageDecodeError::Decompression,
            KittyImageDecodeError::UnsupportedCompression,
            KittyImageDecodeError::UnsupportedFormat,
            KittyImageDecodeError::InvalidDimensions,
            KittyImageDecodeError::InvalidPng,
        ];
        let mut messages: Vec<String> = variants.iter().map(|v| v.to_string()).collect();
        assert!(messages.iter().all(|m| !m.is_empty()));
        messages.sort();
        messages.dedup();
        assert_eq!(messages.len(), variants.len(), "messages must be distinct");
    }
}
