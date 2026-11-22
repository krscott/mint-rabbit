//! Palette, textures, variants, and materials.

use glam::UVec2;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgba8 {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba8 {
    #[must_use]
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    #[must_use]
    pub const fn to_array(self) -> [u8; 4] {
        [self.r, self.g, self.b, self.a]
    }
}

/// Index of a palette entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PaletteIndex(pub u8);

/// The project's ordered list of colors, at most 256 entries.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Palette {
    entries: Vec<Rgba8>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("a palette holds at most 256 colors")]
pub struct PaletteFull;

impl Palette {
    pub const MAX_LEN: usize = 256;

    pub fn from_colors(colors: impl IntoIterator<Item = Rgba8>) -> Result<Self, PaletteFull> {
        let entries: Vec<_> = colors.into_iter().collect();
        if entries.len() > Self::MAX_LEN {
            return Err(PaletteFull);
        }
        Ok(Self { entries })
    }

    #[must_use]
    pub fn colors(&self) -> &[Rgba8] {
        &self.entries
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// A raster image whose pixels are palette indices, row-major from the top
/// left.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexedImage {
    size: UVec2,
    pixels: Vec<PaletteIndex>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("image is {width}x{height} but has {len} pixels")]
pub struct PixelCountMismatch {
    pub width: u32,
    pub height: u32,
    pub len: usize,
}

impl IndexedImage {
    pub fn new(size: UVec2, pixels: Vec<PaletteIndex>) -> Result<Self, PixelCountMismatch> {
        if pixels.len() != (size.x * size.y) as usize {
            return Err(PixelCountMismatch {
                width: size.x,
                height: size.y,
                len: pixels.len(),
            });
        }
        Ok(Self { size, pixels })
    }

    #[must_use]
    pub fn size(&self) -> UVec2 {
        self.size
    }

    #[must_use]
    pub fn pixels(&self) -> &[PaletteIndex] {
        &self.pixels
    }

    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> PaletteIndex {
        self.pixels[(y * self.size.x + x) as usize]
    }
}

/// A raster image in direct color, used only for previews the project never
/// exports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RgbaImage {
    pub size: UVec2,
    pub pixels: Vec<Rgba8>,
}

impl RgbaImage {
    #[must_use]
    pub fn filled(size: UVec2, color: Rgba8) -> Self {
        Self {
            size,
            pixels: vec![color; (size.x * size.y) as usize],
        }
    }
}

/// The place in an export target's format that a texture fills, named by
/// that target.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Slot(pub String);

/// Index of a texture within its project.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TextureId(pub u32);

impl TextureId {
    #[must_use]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// A named image of fixed pixel size: painted variants, or the preview of an
/// image the export target supplies.
#[derive(Clone, Debug, PartialEq)]
pub struct Texture {
    pub name: String,
    pub slot: Slot,
    size: UVec2,
    content: TextureContent,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TextureContent {
    /// Painted in the project; always holds at least one variant.
    Painted(Vec<Variant>),
    /// Supplied by the export target at runtime; only previewed.
    External { preview: RgbaImage },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Variant {
    pub name: String,
    underlay: IndexedImage,
}

impl Variant {
    #[must_use]
    pub fn underlay(&self) -> &IndexedImage {
        &self.underlay
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TextureError {
    #[error("texture {texture:?} needs at least one variant")]
    NoVariants { texture: String },
    #[error("variant {variant:?} of texture {texture:?} is {actual} but the texture is {expected}")]
    SizeMismatch {
        texture: String,
        variant: String,
        expected: UVec2,
        actual: UVec2,
    },
}

impl Texture {
    /// A painted texture whose variants each carry an underlay of `size`.
    pub fn painted(
        name: impl Into<String>,
        slot: Slot,
        size: UVec2,
        variants: impl IntoIterator<Item = (String, IndexedImage)>,
    ) -> Result<Self, TextureError> {
        let name = name.into();
        let variants: Vec<Variant> = variants
            .into_iter()
            .map(|(variant, underlay)| {
                if underlay.size() == size {
                    Ok(Variant {
                        name: variant,
                        underlay,
                    })
                } else {
                    Err(TextureError::SizeMismatch {
                        texture: name.clone(),
                        variant,
                        expected: size,
                        actual: underlay.size(),
                    })
                }
            })
            .collect::<Result<_, _>>()?;
        if variants.is_empty() {
            return Err(TextureError::NoVariants { texture: name });
        }
        Ok(Self {
            name,
            slot,
            size,
            content: TextureContent::Painted(variants),
        })
    }

    #[must_use]
    pub fn external(name: impl Into<String>, slot: Slot, preview: RgbaImage) -> Self {
        Self {
            name: name.into(),
            slot,
            size: preview.size,
            content: TextureContent::External { preview },
        }
    }

    #[must_use]
    pub fn size(&self) -> UVec2 {
        self.size
    }

    #[must_use]
    pub fn content(&self) -> &TextureContent {
        &self.content
    }

    /// The painted variants, or an empty slice for an external texture.
    #[must_use]
    pub fn variants(&self) -> &[Variant] {
        match &self.content {
            TextureContent::Painted(v) => v,
            TextureContent::External { .. } => &[],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Wrap {
    Clamp,
    Repeat,
    Mirror,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AlphaMode {
    Opaque,
    /// Pixels below half alpha are not drawn.
    Cutout,
}

/// Index of a material within its project.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MaterialId(pub u32);

impl MaterialId {
    #[must_use]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// How triangles draw with a texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Material {
    pub texture: TextureId,
    /// Wrap mode along U, then V.
    pub wrap: [Wrap; 2],
    pub alpha: AlphaMode,
    /// Draw back faces too.
    pub double_sided: bool,
    /// Color multiplied into every texel.
    pub tint: Rgba8,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn painted_texture_rejects_mismatched_underlay() {
        let img = IndexedImage::new(UVec2::new(2, 1), vec![PaletteIndex(0); 2]).unwrap();
        let err = Texture::painted(
            "eye",
            Slot("eyes".into()),
            UVec2::new(4, 4),
            [("1".to_owned(), img)],
        )
        .unwrap_err();
        assert!(matches!(err, TextureError::SizeMismatch { .. }));
    }

    #[test]
    fn painted_texture_needs_a_variant() {
        let err = Texture::painted("eye", Slot("eyes".into()), UVec2::ONE, []).unwrap_err();
        assert!(matches!(err, TextureError::NoVariants { .. }));
    }
}
