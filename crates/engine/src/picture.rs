//! Received images, viewed on a tap (ADR-044): decoded here and nowhere
//! else, in Rust, within fixed limits, and scaled down for the screen. The
//! apps never hand an image to a system decoder.

use std::io::Cursor;

use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits};
use zeroize::Zeroizing;

/// Input: the document limit (ADR-043).
const MAX_INPUT: usize = termchat_proto::MAX_LARGE_FILE_BYTES as usize;
const MAX_SIDE: u32 = 8_192;
const MAX_PIXELS: u64 = 40_000_000;
const MAX_ALLOC: u64 = 256 << 20;
/// The longest side the apps get: enough for any screen.
pub const MAX_SHOWN: u32 = 2_560;

/// What the viewer draws: RGBA, 4 bytes a pixel, not premultiplied. Wiped
/// when dropped; no `Debug`, so it never lands in a log.
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Zeroizing<Vec<u8>>,
}

/// A PNG, JPEG, GIF or WebP signature at the start: the only formats
/// decoded. Reads a few bytes, decodes nothing.
pub fn is_image(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes.starts_with(b"\xff\xd8\xff")
        || bytes.starts_with(b"GIF87a")
        || bytes.starts_with(b"GIF89a")
        || (bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"))
}

fn format(bytes: &[u8]) -> Option<ImageFormat> {
    match bytes {
        [0x89, b'P', ..] => Some(ImageFormat::Png),
        [0xff, 0xd8, ..] => Some(ImageFormat::Jpeg),
        [b'G', b'I', b'F', ..] => Some(ImageFormat::Gif),
        [b'R', b'I', b'F', b'F', ..] => Some(ImageFormat::WebP),
        _ => None,
    }
}

/// Decodes a received image within the limits, upright, its first frame,
/// at most `MAX_SHOWN` on the longest side.
pub fn decode(bytes: &[u8]) -> Result<Picture, String> {
    const CANNOT: &str = "this image cannot be shown";
    if bytes.len() > MAX_INPUT || !is_image(bytes) {
        return Err(CANNOT.into());
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_ALLOC);
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format(bytes).ok_or(CANNOT)?);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|_| CANNOT)?;
    let (w, h) = decoder.dimensions();
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(CANNOT.into());
    }
    let orientation = decoder.orientation().ok();
    let mut img = DynamicImage::from_decoder(decoder).map_err(|_| CANNOT)?;
    if let Some(o) = orientation {
        img.apply_orientation(o);
    }
    if img.width() > MAX_SHOWN || img.height() > MAX_SHOWN {
        img = img.resize(MAX_SHOWN, MAX_SHOWN, image::imageops::FilterType::Triangle);
    }
    let rgba = img.into_rgba8();
    Ok(Picture {
        width: rgba.width(),
        height: rgba.height(),
        rgba: Zeroizing::new(rgba.into_raw()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageEncoder, codecs::png::PngEncoder};

    fn png(w: u32, h: u32) -> Vec<u8> {
        let pixels: Vec<u8> = (0..w * h * 4).map(|n| (n % 251) as u8).collect();
        let mut out = Vec::new();
        PngEncoder::new(&mut out)
            .write_image(&pixels, w, h, image::ExtendedColorType::Rgba8)
            .unwrap();
        out
    }

    /// A PNG header claiming `w` × `h`, with no pixel data behind it.
    fn png_header(w: u32, h: u32) -> Vec<u8> {
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(b"IHDR");
        ihdr.extend_from_slice(&w.to_be_bytes());
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        // CRC-32 (IEEE), bit by bit: PNG checks it before reading the size.
        let mut crc = !0u32;
        for &byte in &ihdr {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        let crc = !crc;
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        out.extend_from_slice(&13u32.to_be_bytes());
        out.extend_from_slice(&ihdr);
        out.extend_from_slice(&crc.to_be_bytes());
        out
    }

    #[test]
    fn signatures_only() {
        assert!(is_image(&png(1, 1)));
        assert!(is_image(b"\xff\xd8\xff\xe0rest"));
        assert!(is_image(b"GIF89a..."));
        assert!(is_image(b"RIFF\0\0\0\0WEBPVP8 "));
        for not in [
            &b""[..],
            b"%PDF-1.7",
            b"RIFF\0\0\0\0WAVE",
            b"GIF8",
            b"\x89PNX",
        ] {
            assert!(!is_image(not));
        }
    }

    #[test]
    fn decodes_and_fits_the_screen() {
        let p = decode(&png(3, 2)).unwrap();
        assert_eq!((p.width, p.height, p.rgba.len()), (3, 2, 24));
        assert_eq!(&p.rgba[..4], &[0, 1, 2, 3]);
        let big = decode(&png(MAX_SHOWN + 440, 300)).unwrap();
        assert_eq!(big.width, MAX_SHOWN);
        assert!(big.height < 300);
    }

    #[test]
    fn refuses_what_is_not_an_image_or_too_big() {
        let cannot = Err("this image cannot be shown".to_string());
        let as_err = |r: Result<Picture, String>| r.map(|_| ());
        assert_eq!(as_err(decode(b"%PDF-1.7")), cannot.clone());
        // A signature, then garbage, or cut short.
        assert_eq!(as_err(decode(b"\x89PNG\r\n\x1a\ngarbage")), cannot.clone());
        let whole = png(4, 4);
        assert_eq!(as_err(decode(&whole[..whole.len() - 20])), cannot.clone());
        // Headers that ask for too much never get their pixels allocated.
        assert_eq!(as_err(decode(&png_header(MAX_SIDE + 1, 1))), cannot.clone());
        assert_eq!(as_err(decode(&png_header(8_000, 8_000))), cannot.clone());
        assert_eq!(as_err(decode(&png_header(0, 5))), cannot);
    }
}
