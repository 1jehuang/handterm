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

pub fn decode_kitty_image_payload(
    format: u32,
    compression: Option<u8>,
    payload: &[u8],
    width: u32,
    height: u32,
) -> Result<(u32, u32, Vec<u8>), ()> {
    let decoded = base64_decode_kitty(payload)?;
    let decoded = decompress_kitty_payload(decoded, compression)?;
    match format {
        24 => {
            if width == 0 || height == 0 {
                return Err(());
            }
            let expected = (width as usize)
                .saturating_mul(height as usize)
                .saturating_mul(3);
            if decoded.len() != expected {
                return Err(());
            }
            let mut rgba = Vec::with_capacity((width as usize) * (height as usize) * 4);
            for chunk in decoded.chunks_exact(3) {
                rgba.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 0xff]);
            }
            Ok((width, height, rgba))
        }
        32 => {
            if width == 0 || height == 0 {
                return Err(());
            }
            let expected = (width as usize)
                .saturating_mul(height as usize)
                .saturating_mul(4);
            if decoded.len() != expected {
                return Err(());
            }
            Ok((width, height, decoded))
        }
        100 => decode_png_kitty(&decoded),
        _ => Err(()),
    }
}

fn base64_decode_kitty(input: &[u8]) -> Result<Vec<u8>, ()> {
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
            return Err(());
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

fn decode_png_kitty(encoded_png: &[u8]) -> Result<(u32, u32, Vec<u8>), ()> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(encoded_png));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|_| ())?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|_| ())?;
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
        _ => return Err(()),
    };

    Ok((info.width, info.height, rgba))
}

fn decompress_kitty_payload(decoded: Vec<u8>, compression: Option<u8>) -> Result<Vec<u8>, ()> {
    match compression {
        None => Ok(decoded),
        Some(b'z') => {
            let mut decoder = flate2::read::ZlibDecoder::new(decoded.as_slice());
            let mut out = Vec::new();
            std::io::Read::read_to_end(&mut decoder, &mut out).map_err(|_| ())?;
            Ok(out)
        }
        Some(_) => Err(()),
    }
}
