//! `GameCube` texture and palette encodings used by villager models: CI4
//! textures tiled in 8x8 blocks, and RGB5A3 palette entries.

use mint_core::{PaletteIndex, Rgba8};

/// Width and height of a CI4 tile in texels.
pub const TILE: u32 = 8;
/// Bytes per CI4 tile: 8x8 texels at 4 bits each.
const TILE_BYTES: usize = 32;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum GxError {
    #[error("texture size {width}x{height} is not a multiple of 8 within 8..=1024 on both sides")]
    BadSize { width: u32, height: u32 },
    #[error("CI4 texture {width}x{height} needs {expected} bytes, found {actual}")]
    WrongLength {
        width: u32,
        height: u32,
        expected: usize,
        actual: usize,
    },
    #[error("CI4 pixel uses palette entry {0}; only entries 0-15 fit in 4 bits")]
    IndexTooLarge(u8),
}

/// Bytes a CI4 texture of this size takes.
#[must_use]
pub fn ci4_len(width: u32, height: u32) -> usize {
    (width * height / 2) as usize
}

/// Checks a texture size the game can load: whole 8x8 tiles, and at most
/// 1024 texels a side to fit the load command's 10-bit size fields.
pub fn check_size(width: u32, height: u32) -> Result<(), GxError> {
    let ok = |v: u32| (TILE..=1024).contains(&v) && v.is_multiple_of(TILE);
    if ok(width) && ok(height) {
        Ok(())
    } else {
        Err(GxError::BadSize { width, height })
    }
}

/// The byte offset and nibble shift holding texel (x, y).
fn texel_location(width: u32, x: u32, y: u32) -> (usize, u32) {
    let tiles_per_row = width / TILE;
    let tile = (y / TILE) * tiles_per_row + x / TILE;
    let within = (y % TILE) * TILE + x % TILE;
    let byte = tile as usize * TILE_BYTES + (within / 2) as usize;
    let shift = if within.is_multiple_of(2) { 4 } else { 0 };
    (byte, shift)
}

/// Decodes a tiled CI4 texture into row-major palette indices.
pub fn decode_ci4(data: &[u8], width: u32, height: u32) -> Result<Vec<PaletteIndex>, GxError> {
    check_size(width, height)?;
    let expected = ci4_len(width, height);
    if data.len() != expected {
        return Err(GxError::WrongLength {
            width,
            height,
            expected,
            actual: data.len(),
        });
    }
    let mut out = Vec::with_capacity((width * height) as usize);
    for y in 0..height {
        for x in 0..width {
            let (byte, shift) = texel_location(width, x, y);
            out.push(PaletteIndex((data[byte] >> shift) & 0xF));
        }
    }
    Ok(out)
}

/// Encodes row-major palette indices as a tiled CI4 texture.
pub fn encode_ci4(pixels: &[PaletteIndex], width: u32, height: u32) -> Result<Vec<u8>, GxError> {
    check_size(width, height)?;
    assert_eq!(
        pixels.len(),
        (width * height) as usize,
        "pixel count must match size"
    );
    let mut out = vec![0u8; ci4_len(width, height)];
    for y in 0..height {
        for x in 0..width {
            let PaletteIndex(i) = pixels[(y * width + x) as usize];
            if i > 0xF {
                return Err(GxError::IndexTooLarge(i));
            }
            let (byte, shift) = texel_location(width, x, y);
            out[byte] |= i << shift;
        }
    }
    Ok(out)
}

fn expand5(v: u16) -> u8 {
    let v = (v & 0x1F) as u8;
    (v << 3) | (v >> 2)
}

fn expand4(v: u16) -> u8 {
    (v & 0xF) as u8 * 17
}

fn expand3(v: u16) -> u8 {
    let v = (v & 0x7) as u8;
    (v << 5) | (v << 2) | (v >> 1)
}

/// Decodes one RGB5A3 palette entry.
#[must_use]
pub fn decode_rgb5a3(v: u16) -> Rgba8 {
    if v & 0x8000 != 0 {
        Rgba8::new(expand5(v >> 10), expand5(v >> 5), expand5(v), 255)
    } else {
        Rgba8::new(
            expand4(v >> 8),
            expand4(v >> 4),
            expand4(v),
            expand3(v >> 12),
        )
    }
}

/// Rounds an 8-bit channel to the nearest level of a `bits`-bit one.
fn round_to(v: u8, bits: u32) -> u16 {
    let max = (1u32 << bits) - 1;
    ((u32::from(v) * max + 127) / 255) as u16
}

/// Encodes a color as the nearest RGB5A3 value. Of the two forms (opaque
/// RGB555, or 3-bit alpha with RGB444) it picks the one that decodes closer
/// to `c`, preferring RGB555 on a tie, so decoded game palettes encode back
/// to colors that decode identically.
#[must_use]
pub fn encode_rgb5a3(c: Rgba8) -> u16 {
    let rgb555 = 0x8000 | (round_to(c.r, 5) << 10) | (round_to(c.g, 5) << 5) | round_to(c.b, 5);
    let a3rgb4 = (round_to(c.a, 3) << 12)
        | (round_to(c.r, 4) << 8)
        | (round_to(c.g, 4) << 4)
        | round_to(c.b, 4);
    let error = |v: u16| {
        let d = decode_rgb5a3(v).to_array();
        c.to_array()
            .iter()
            .zip(d)
            .map(|(&a, b)| (i32::from(a) - i32::from(b)).pow(2))
            .sum::<i32>()
    };
    if error(rgb555) <= error(a3rgb4) {
        rgb555
    } else {
        a3rgb4
    }
}

/// The color the game shows for `c` after RGB5A3 quantization.
#[must_use]
pub fn quantize_rgb5a3(c: Rgba8) -> Rgba8 {
    decode_rgb5a3(encode_rgb5a3(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_rgb5a3_value_round_trips_or_decodes_identically() {
        for v in 0..=u16::MAX {
            let c = decode_rgb5a3(v);
            let back = encode_rgb5a3(c);
            assert_eq!(decode_rgb5a3(back), c, "{v:#06x}");
        }
    }

    #[test]
    fn encoding_rounds_to_nearest() {
        // 7 is closer to the 5-bit level 8 than to 0.
        assert_eq!(
            quantize_rgb5a3(Rgba8::new(7, 7, 7, 255)),
            Rgba8::new(8, 8, 8, 255)
        );
        // Nearly opaque stays opaque rather than dropping to 4-bit color.
        assert_eq!(encode_rgb5a3(Rgba8::new(200, 0, 0, 254)) & 0x8000, 0x8000);
        // Mostly transparent uses the alpha form.
        assert_eq!(quantize_rgb5a3(Rgba8::new(255, 255, 255, 40)).a, 36);
    }

    #[test]
    fn ci4_round_trips_and_tiles() {
        let (w, h) = (16, 8);
        let pixels: Vec<_> = (0..w * h).map(|i| PaletteIndex((i % 16) as u8)).collect();
        let bytes = encode_ci4(&pixels, w, h).unwrap();
        assert_eq!(bytes.len(), 64);
        // Texel (8, 0) starts the second tile.
        assert_eq!(bytes[32] >> 4, 8);
        assert_eq!(decode_ci4(&bytes, w, h).unwrap(), pixels);
    }

    #[test]
    fn ci4_rejects_unaligned_size() {
        assert_eq!(
            decode_ci4(&[0; 6], 4, 3),
            Err(GxError::BadSize {
                width: 4,
                height: 3
            })
        );
    }
}
