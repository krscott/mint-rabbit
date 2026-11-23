//! Palette, textures, variants, layers, and materials.

use glam::{UVec2, Vec2};

use crate::Shape;

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

    #[must_use]
    pub fn contains(&self, index: PaletteIndex) -> bool {
        usize::from(index.0) < self.entries.len()
    }

    pub(crate) fn set(&mut self, index: PaletteIndex, color: Rgba8) {
        self.entries[usize::from(index.0)] = color;
    }

    pub(crate) fn push(&mut self, color: Rgba8) -> Result<PaletteIndex, PaletteFull> {
        if self.entries.len() == Self::MAX_LEN {
            return Err(PaletteFull);
        }
        self.entries.push(color);
        Ok(PaletteIndex((self.entries.len() - 1) as u8))
    }

    pub(crate) fn remove(&mut self, index: PaletteIndex) {
        self.entries.remove(usize::from(index.0));
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

    /// Replaces every pixel's index with `f` of it.
    pub(crate) fn remap(&mut self, f: impl Fn(PaletteIndex) -> PaletteIndex) {
        for p in &mut self.pixels {
            *p = f(*p);
        }
    }

    /// Sets each pixel whose center `shape` covers to its color.
    fn paint(&mut self, shape: &Shape) {
        let outline = shape.outline();
        let (min, max) = outline.bounds();
        // Pixels whose centers (x + 0.5) fall within the bounds.
        let lo = |v: f32| (v - 0.5).ceil().max(0.0) as u32;
        let hi = |v: f32, size: u32| ((v - 0.5).floor() + 1.0).clamp(0.0, size as f32) as u32;
        for y in lo(min.y)..hi(max.y, self.size.y) {
            for x in lo(min.x)..hi(max.x, self.size.x) {
                if outline.covers(Vec2::new(x as f32 + 0.5, y as f32 + 0.5)) {
                    self.pixels[(y * self.size.x + x) as usize] = shape.color();
                }
            }
        }
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

/// An ordered list of shapes in one variant; later shapes draw over
/// earlier ones.
#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub name: String,
    /// Hidden layers neither show nor export.
    pub visible: bool,
    /// Locked layers are not edited; the app enforces it.
    pub locked: bool,
    /// Opacity in the views only, from 0 to 1; export ignores it.
    pub opacity: f32,
    shapes: Vec<Shape>,
}

impl Layer {
    /// An empty, visible, unlocked, opaque layer.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            visible: true,
            locked: false,
            opacity: 1.0,
            shapes: Vec::new(),
        }
    }

    #[must_use]
    pub fn shapes(&self) -> &[Shape] {
        &self.shapes
    }

    pub fn shapes_mut(&mut self) -> &mut [Shape] {
        &mut self.shapes
    }

    pub fn remove_shape(&mut self, index: usize) -> Shape {
        self.shapes.remove(index)
    }

    /// Moves the shape at `from` so that it ends up at `to`.
    pub fn move_shape(&mut self, from: usize, to: usize) {
        let shape = self.shapes.remove(from);
        self.shapes.insert(to, shape);
    }

    pub fn insert_shape(&mut self, index: usize, shape: Shape) {
        self.shapes.insert(index, shape);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Variant {
    pub name: String,
    underlay: IndexedImage,
    /// Bottom to top.
    layers: Vec<Layer>,
}

impl Variant {
    #[must_use]
    pub fn underlay(&self) -> &IndexedImage {
        &self.underlay
    }

    pub(crate) fn underlay_mut(&mut self) -> &mut IndexedImage {
        &mut self.underlay
    }

    /// The layers, bottom to top.
    #[must_use]
    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    pub fn layers_mut(&mut self) -> &mut [Layer] {
        &mut self.layers
    }

    pub fn insert_layer(&mut self, index: usize, layer: Layer) {
        self.layers.insert(index, layer);
    }

    pub fn remove_layer(&mut self, index: usize) -> Layer {
        self.layers.remove(index)
    }

    /// Moves the layer at `from` so that it ends up at `to`.
    pub fn move_layer(&mut self, from: usize, to: usize) {
        let layer = self.layers.remove(from);
        self.layers.insert(to, layer);
    }

    /// The image the variant exports: the underlay with the shapes of every
    /// visible layer on top, each pixel taking the topmost shape that covers
    /// its center.
    #[must_use]
    pub fn rasterize(&self) -> IndexedImage {
        let mut image = self.underlay.clone();
        for layer in self.layers.iter().filter(|l| l.visible) {
            for shape in &layer.shapes {
                image.paint(shape);
            }
        }
        image
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
                        layers: Vec::new(),
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

    /// The variants, for the crate's checked edits; see
    /// [`crate::Project::edit_variant`].
    pub(crate) fn variants_mut(&mut self) -> &mut [Variant] {
        match &mut self.content {
            TextureContent::Painted(v) => v,
            TextureContent::External { .. } => &mut [],
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
    fn rasterize_paints_visible_layers_in_order() {
        use crate::{Style, shape};
        let underlay = IndexedImage::new(UVec2::new(4, 2), vec![PaletteIndex(0); 8]).unwrap();
        let mut t = Texture::painted(
            "body",
            Slot("body".into()),
            UVec2::new(4, 2),
            [("1".to_owned(), underlay)],
        )
        .unwrap();
        let v = &mut t.variants_mut()[0];
        let rect = |a: Vec2, b: Vec2, c: u8| {
            Shape::new(shape::rectangle(a, b), Style::Fill, PaletteIndex(c)).unwrap()
        };
        v.insert_layer(0, Layer::new("low"));
        v.insert_layer(1, Layer::new("high"));
        v.layers_mut()[0].insert_shape(0, rect(Vec2::ZERO, Vec2::new(3.0, 2.0), 1));
        // Covers the centers of pixels 2 and 3 on the top row only.
        v.layers_mut()[1].insert_shape(0, rect(Vec2::new(1.6, -1.0), Vec2::new(9.0, 0.9), 2));
        let px = |v: &Variant| -> Vec<u8> { v.rasterize().pixels().iter().map(|i| i.0).collect() };
        assert_eq!(px(v), [1, 1, 2, 2, 1, 1, 1, 0]);
        v.layers_mut()[1].visible = false;
        assert_eq!(px(v), [1, 1, 1, 0, 1, 1, 1, 0]);
        assert_eq!(v.underlay().pixels(), &[PaletteIndex(0); 8]);
    }

    #[test]
    fn painted_texture_needs_a_variant() {
        let err = Texture::painted("eye", Slot("eyes".into()), UVec2::ONE, []).unwrap_err();
        assert!(matches!(err, TextureError::NoVariants { .. }));
    }
}
