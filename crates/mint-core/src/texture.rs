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
        if pixels.len() as u64 != u64::from(size.x) * u64::from(size.y) {
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

/// Calls `f` with the row-major index of each pixel of an image of `size`
/// whose center `shape` covers.
fn covered_pixels(shape: &Shape, size: UVec2, mut f: impl FnMut(usize)) {
    let outline = shape.outline();
    let (min, max) = outline.bounds();
    // Pixels whose centers (x + 0.5) fall within the bounds.
    let lo = |v: f32| (v - 0.5).ceil().max(0.0) as u32;
    let hi = |v: f32, side: u32| ((v - 0.5).floor() + 1.0).clamp(0.0, side as f32) as u32;
    for y in lo(min.y)..hi(max.y, size.y) {
        for x in lo(min.x)..hi(max.x, size.x) {
            if outline.covers(Vec2::new(x as f32 + 0.5, y as f32 + 0.5)) {
                f((y * size.x + x) as usize);
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

/// The pixels of a raster layer: a palette entry or nothing for each,
/// row-major from the top left.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Raster {
    size: UVec2,
    pixels: Vec<Option<PaletteIndex>>,
}

impl Raster {
    pub fn new(size: UVec2, pixels: Vec<Option<PaletteIndex>>) -> Result<Self, PixelCountMismatch> {
        if pixels.len() as u64 != u64::from(size.x) * u64::from(size.y) {
            return Err(PixelCountMismatch {
                width: size.x,
                height: size.y,
                len: pixels.len(),
            });
        }
        Ok(Self { size, pixels })
    }

    /// A raster of `size` with no pixels set.
    #[must_use]
    pub fn clear(size: UVec2) -> Self {
        Self {
            size,
            pixels: vec![None; (u64::from(size.x) * u64::from(size.y)) as usize],
        }
    }

    #[must_use]
    pub fn size(&self) -> UVec2 {
        self.size
    }

    #[must_use]
    pub fn pixels(&self) -> &[Option<PaletteIndex>] {
        &self.pixels
    }

    /// The palette entry of the pixel under `p`, or `None` where it is
    /// unset or outside the raster.
    #[must_use]
    pub fn at(&self, p: Vec2) -> Option<PaletteIndex> {
        let i = self.index(p.x.floor() as i64, p.y.floor() as i64)?;
        self.pixels[i]
    }

    fn index(&self, x: i64, y: i64) -> Option<usize> {
        let inside =
            (0..i64::from(self.size.x)).contains(&x) && (0..i64::from(self.size.y)).contains(&y);
        inside.then(|| (y * i64::from(self.size.x) + x) as usize)
    }

    /// Sets each pixel whose center `shape` covers to its color.
    pub fn paint_shape(&mut self, shape: &Shape) {
        covered_pixels(shape, self.size, |i| self.pixels[i] = Some(shape.color()));
    }

    /// Sets the pixels a pencil `width` texels wide covers from `from` to
    /// `to` to `value`: its tip, as [`pencil_tip`] places it, stepped one
    /// pixel at a time so that no gap opens.
    pub fn draw_line(&mut self, from: Vec2, to: Vec2, width: f32, value: Option<PaletteIndex>) {
        let (x0, y0) = tip_origin(from, width);
        let (x1, y1) = tip_origin(to, width);
        // Offsets from a tip's origin, which is (0, 0) at the origin.
        let tip = pencil_tip(Vec2::ZERO, width);
        // Bresenham between the two tip origins.
        let (mut x, mut y) = (x0, y0);
        let (dx, dy) = ((x1 - x).abs(), -(y1 - y).abs());
        let (sx, sy) = ((x1 - x).signum(), (y1 - y).signum());
        let mut err = dx + dy;
        loop {
            for &(ox, oy) in &tip {
                if let Some(i) = self.index(x + ox, y + oy) {
                    self.pixels[i] = value;
                }
            }
            if (x, y) == (x1, y1) {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    /// Sets the pixel under `at` and every pixel joined to it through
    /// edge-neighbors of the same value to `value`. Nothing happens outside
    /// the raster.
    pub fn fill(&mut self, at: Vec2, value: Option<PaletteIndex>) {
        let (x, y) = (at.x.floor() as i64, at.y.floor() as i64);
        let Some(start) = self.index(x, y) else {
            return;
        };
        let old = self.pixels[start];
        if old == value {
            return;
        }
        let mut stack = vec![(x, y)];
        while let Some((x, y)) = stack.pop() {
            let Some(i) = self.index(x, y).filter(|&i| self.pixels[i] == old) else {
                continue;
            };
            self.pixels[i] = value;
            stack.extend([(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)]);
        }
    }

    pub(crate) fn remap(&mut self, f: impl Fn(PaletteIndex) -> PaletteIndex) {
        for p in self.pixels.iter_mut().flatten() {
            *p = f(*p);
        }
    }
}

/// The whole pixels a pencil tip `width` texels wide (rounded, at least 1)
/// covers at `at`: those whose centers lie within half the width of the
/// tip's center, which is the center of the pixel under `at` for odd widths
/// and the nearest pixel corner for even ones. At width 1 it is the pixel
/// under `at`.
#[must_use]
pub fn pencil_tip(at: Vec2, width: f32) -> Vec<(i64, i64)> {
    let w = tip_width(width);
    let (x, y) = tip_origin(at, width);
    // The tip's center relative to its origin pixel's top-left corner.
    let center = if w % 2 == 1 { 0.5 } else { 0.0 };
    let half = w as f32 / 2.0;
    let reach = w / 2 + 1;
    (-reach..=reach)
        .flat_map(|dy| (-reach..=reach).map(move |dx| (dx, dy)))
        .filter(|&(dx, dy)| {
            let d = Vec2::new(dx as f32 + 0.5 - center, dy as f32 + 0.5 - center);
            d.length_squared() <= half * half
        })
        .map(|(dx, dy)| (x + dx, y + dy))
        .collect()
}

fn tip_width(width: f32) -> i64 {
    width.round().max(1.0) as i64
}

/// The pixel a tip is placed from: the one under `at` for odd widths, the
/// one right and below the nearest corner for even ones.
fn tip_origin(at: Vec2, width: f32) -> (i64, i64) {
    let p = if tip_width(width) % 2 == 1 {
        at.floor()
    } else {
        at.round()
    };
    (p.x as i64, p.y as i64)
}

impl From<IndexedImage> for Raster {
    fn from(image: IndexedImage) -> Self {
        Self {
            size: image.size,
            pixels: image.pixels.into_iter().map(Some).collect(),
        }
    }
}

/// Which kind of layer a layer is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LayerKind {
    Vector,
    Raster,
}

/// What a layer holds.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum LayerContent {
    /// Shapes, later ones drawing over earlier ones.
    Vector(Vec<Shape>),
    Raster(Raster),
}

/// One level of a variant's stack: shapes, or pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub name: String,
    /// Hidden layers neither show nor export.
    pub visible: bool,
    /// Locked layers are not edited; the app enforces it.
    pub locked: bool,
    /// Opacity in the views only, from 0 to 1; export ignores it.
    pub opacity: f32,
    pub(crate) content: LayerContent,
}

impl Layer {
    /// An empty, visible, unlocked, opaque vector layer.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self::with_content(name, LayerContent::Vector(Vec::new()))
    }

    /// A visible, unlocked, opaque raster layer of `raster`.
    #[must_use]
    pub fn raster(name: impl Into<String>, raster: Raster) -> Self {
        Self::with_content(name, LayerContent::Raster(raster))
    }

    fn with_content(name: impl Into<String>, content: LayerContent) -> Self {
        Self {
            name: name.into(),
            visible: true,
            locked: false,
            opacity: 1.0,
            content,
        }
    }

    #[must_use]
    pub fn kind(&self) -> LayerKind {
        match self.content {
            LayerContent::Vector(_) => LayerKind::Vector,
            LayerContent::Raster(_) => LayerKind::Raster,
        }
    }

    /// The shapes of a vector layer; a raster layer has none.
    #[must_use]
    pub fn shapes(&self) -> &[Shape] {
        match &self.content {
            LayerContent::Vector(shapes) => shapes,
            LayerContent::Raster(_) => &[],
        }
    }

    /// The shapes of a vector layer; a raster layer has none.
    pub fn shapes_mut(&mut self) -> &mut [Shape] {
        match &mut self.content {
            LayerContent::Vector(shapes) => shapes,
            LayerContent::Raster(_) => &mut [],
        }
    }

    /// The pixels of a raster layer.
    #[must_use]
    pub fn raster_pixels(&self) -> Option<&Raster> {
        match &self.content {
            LayerContent::Raster(r) => Some(r),
            LayerContent::Vector(_) => None,
        }
    }

    /// The pixels of a raster layer.
    pub fn raster_pixels_mut(&mut self) -> Option<&mut Raster> {
        match &mut self.content {
            LayerContent::Raster(r) => Some(r),
            LayerContent::Vector(_) => None,
        }
    }

    /// The shape list of a vector layer. Panics on a raster layer, which
    /// has no shapes to change.
    fn vector_mut(&mut self) -> &mut Vec<Shape> {
        match &mut self.content {
            LayerContent::Vector(shapes) => shapes,
            LayerContent::Raster(_) => panic!("layer {:?} is a raster layer", self.name),
        }
    }

    /// Panics on a raster layer.
    pub fn remove_shape(&mut self, index: usize) -> Shape {
        self.vector_mut().remove(index)
    }

    /// Moves the shape at `from` so that it ends up at `to`. Panics on a
    /// raster layer.
    pub fn move_shape(&mut self, from: usize, to: usize) {
        let shapes = self.vector_mut();
        let shape = shapes.remove(from);
        shapes.insert(to, shape);
    }

    /// Panics on a raster layer.
    pub fn insert_shape(&mut self, index: usize, shape: Shape) {
        self.vector_mut().insert(index, shape);
    }

    /// The palette entry each pixel of an image of `size` takes from this
    /// layer, row-major: for a vector layer, the topmost shape covering its
    /// center, or `None` where none does. Visibility and opacity are not
    /// applied. A raster layer must be of `size`.
    #[must_use]
    pub fn coverage(&self, size: UVec2) -> Vec<Option<PaletteIndex>> {
        match &self.content {
            LayerContent::Vector(shapes) => {
                let mut out = vec![None; (u64::from(size.x) * u64::from(size.y)) as usize];
                for shape in shapes {
                    covered_pixels(shape, size, |i| out[i] = Some(shape.color()));
                }
                out
            }
            LayerContent::Raster(r) => {
                debug_assert_eq!(r.size, size);
                r.pixels.clone()
            }
        }
    }

    /// Turns a vector layer into a raster layer of `size` holding the
    /// pixels its shapes cover. A raster layer stays as it is.
    fn rasterize(&mut self, size: UVec2) {
        if self.kind() == LayerKind::Vector {
            let pixels = self.coverage(size);
            self.content = LayerContent::Raster(Raster { size, pixels });
        }
    }

    /// The palette entries the layer uses.
    pub(crate) fn colors(&self) -> Box<dyn Iterator<Item = PaletteIndex> + '_> {
        match &self.content {
            LayerContent::Vector(shapes) => Box::new(shapes.iter().map(Shape::color)),
            LayerContent::Raster(r) => Box::new(r.pixels.iter().flatten().copied()),
        }
    }

    pub(crate) fn remap(&mut self, f: impl Fn(PaletteIndex) -> PaletteIndex) {
        match &mut self.content {
            LayerContent::Vector(shapes) => {
                for s in shapes {
                    s.set_color(f(s.color()));
                }
            }
            LayerContent::Raster(r) => r.remap(f),
        }
    }
}

/// The name of the raster layer an imported image arrives in.
const BASE_LAYER: &str = "Base";

#[derive(Clone, Debug, PartialEq)]
pub struct Variant {
    pub name: String,
    /// The texture's size, which every raster layer has.
    size: UVec2,
    /// Bottom to top.
    layers: Vec<Layer>,
}

impl Variant {
    /// A variant of a texture of `size` with no layers.
    #[must_use]
    pub fn new(name: impl Into<String>, size: UVec2) -> Self {
        Self {
            name: name.into(),
            size,
            layers: Vec::new(),
        }
    }

    /// A variant holding `image` as a locked raster layer, as imported.
    #[must_use]
    pub fn imported(name: impl Into<String>, image: IndexedImage) -> Self {
        let mut v = Self::new(name, image.size);
        let mut base = Layer::raster(BASE_LAYER, image.into());
        base.locked = true;
        v.layers.push(base);
        v
    }

    #[must_use]
    pub fn size(&self) -> UVec2 {
        self.size
    }

    /// The size of the variant, or else of one of its raster layers, that
    /// isn't `expected`.
    pub(crate) fn size_mismatch(&self, expected: UVec2) -> Option<UVec2> {
        std::iter::once(self.size)
            .chain(
                self.layers
                    .iter()
                    .filter_map(Layer::raster_pixels)
                    .map(Raster::size),
            )
            .find(|&s| s != expected)
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

    /// Turns layer `index`, if a vector layer, into a raster layer holding
    /// the pixels its shapes cover.
    pub fn rasterize_layer(&mut self, index: usize) {
        let size = self.size;
        self.layers[index].rasterize(size);
    }

    /// The palette entries the variant's layers use.
    pub(crate) fn colors(&self) -> impl Iterator<Item = PaletteIndex> + '_ {
        self.layers.iter().flat_map(Layer::colors)
    }

    /// The image the variant exports: each pixel takes the topmost visible
    /// layer that covers it, or palette entry 0 where none does.
    #[must_use]
    pub fn rasterize(&self) -> IndexedImage {
        let mut pixels =
            vec![PaletteIndex(0); (u64::from(self.size.x) * u64::from(self.size.y)) as usize];
        for layer in self.layers.iter().filter(|l| l.visible) {
            for (px, c) in pixels.iter_mut().zip(layer.coverage(self.size)) {
                if let Some(c) = c {
                    *px = c;
                }
            }
        }
        IndexedImage {
            size: self.size,
            pixels,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TextureError {
    #[error("texture {texture:?} needs at least one variant")]
    NoVariants { texture: String },
    #[error(
        "variant {variant:?} of texture {texture:?} has an image of {actual}, but the texture is {expected}"
    )]
    SizeMismatch {
        texture: String,
        variant: String,
        expected: UVec2,
        actual: UVec2,
    },
}

impl Texture {
    /// A painted texture of `size`, whose variants and their raster layers
    /// must all be of that size.
    pub fn painted(
        name: impl Into<String>,
        slot: Slot,
        size: UVec2,
        variants: impl IntoIterator<Item = Variant>,
    ) -> Result<Self, TextureError> {
        let name = name.into();
        let variants: Vec<Variant> = variants.into_iter().collect();
        for v in &variants {
            if let Some(actual) = v.size_mismatch(size) {
                return Err(TextureError::SizeMismatch {
                    texture: name,
                    variant: v.name.clone(),
                    expected: size,
                    actual,
                });
            }
        }
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

    fn image(size: UVec2, pixels: &[u8]) -> IndexedImage {
        IndexedImage::new(size, pixels.iter().map(|&i| PaletteIndex(i)).collect()).unwrap()
    }

    fn px(r: &Raster) -> Vec<Option<u8>> {
        r.pixels().iter().map(|p| p.map(|i| i.0)).collect()
    }

    #[test]
    fn painted_texture_rejects_mismatched_sizes() {
        let small = Variant::imported("1", image(UVec2::new(2, 1), &[0, 0]));
        let err =
            Texture::painted("eye", Slot("eyes".into()), UVec2::new(4, 4), [small]).unwrap_err();
        assert!(matches!(err, TextureError::SizeMismatch { .. }));

        // A raster layer of another size in a variant of the right size.
        let mut v = Variant::new("1", UVec2::new(4, 4));
        v.insert_layer(0, Layer::raster("l", Raster::clear(UVec2::new(2, 2))));
        let err = Texture::painted("eye", Slot("eyes".into()), UVec2::new(4, 4), [v]).unwrap_err();
        assert!(
            matches!(err, TextureError::SizeMismatch { actual, .. } if actual == UVec2::new(2, 2))
        );
    }

    #[test]
    fn imported_variants_start_with_a_locked_base_layer() {
        let v = Variant::imported("1", image(UVec2::new(2, 1), &[3, 4]));
        let [base] = v.layers() else {
            panic!("one layer");
        };
        assert_eq!(base.name, BASE_LAYER);
        assert!(base.locked && base.visible);
        assert_eq!(px(base.raster_pixels().unwrap()), [Some(3), Some(4)]);
    }

    #[test]
    fn rasterize_paints_visible_layers_in_order() {
        use crate::{Style, shape};
        let mut v = Variant::imported("1", image(UVec2::new(4, 2), &[5; 8]));
        let rect = |a: Vec2, b: Vec2, c: u8| {
            Shape::new(shape::rectangle(a, b), Style::Fill, PaletteIndex(c)).unwrap()
        };
        v.insert_layer(1, Layer::new("low"));
        v.insert_layer(2, Layer::new("high"));
        v.layers_mut()[1].insert_shape(0, rect(Vec2::ZERO, Vec2::new(3.0, 2.0), 1));
        // Covers the centers of pixels 2 and 3 on the top row only.
        v.layers_mut()[2].insert_shape(0, rect(Vec2::new(1.6, -1.0), Vec2::new(9.0, 0.9), 2));
        let px = |v: &Variant| -> Vec<u8> { v.rasterize().pixels().iter().map(|i| i.0).collect() };
        assert_eq!(px(&v), [1, 1, 2, 2, 1, 1, 1, 5]);
        v.layers_mut()[2].visible = false;
        assert_eq!(px(&v), [1, 1, 1, 5, 1, 1, 1, 5]);
        // Where no visible layer covers a pixel, it takes entry 0.
        v.layers_mut()[0].visible = false;
        assert_eq!(px(&v), [1, 1, 1, 0, 1, 1, 1, 0]);
    }

    #[test]
    fn rasterizing_a_layer_keeps_what_it_covers() {
        use crate::{Style, shape};
        let mut l = Layer::new("l");
        l.locked = true;
        l.opacity = 0.5;
        let square = shape::rectangle(Vec2::ZERO, Vec2::new(1.0, 2.0));
        l.insert_shape(0, Shape::new(square, Style::Fill, PaletteIndex(7)).unwrap());
        let before = l.coverage(UVec2::new(2, 2));
        let mut v = Variant::new("1", UVec2::new(2, 2));
        v.insert_layer(0, l);
        v.rasterize_layer(0);
        let l = &v.layers()[0];
        assert_eq!(l.kind(), LayerKind::Raster);
        assert!(l.locked && (l.opacity - 0.5).abs() < f32::EPSILON);
        assert_eq!(l.coverage(UVec2::new(2, 2)), before);
        assert_eq!(
            px(l.raster_pixels().unwrap()),
            [Some(7), None, Some(7), None]
        );
    }

    #[test]
    fn a_thin_line_leaves_no_gaps() {
        let mut r = Raster::clear(UVec2::new(5, 3));
        r.draw_line(
            Vec2::new(0.5, 0.5),
            Vec2::new(4.9, 2.1),
            1.0,
            Some(PaletteIndex(1)),
        );
        // One pixel per column, from the pixel under the start to the one
        // under the end.
        let rows: Vec<Vec<Option<u8>>> = px(&r).chunks(5).map(<[_]>::to_vec).collect();
        let one = Some(1);
        assert_eq!(
            rows,
            [
                [one, None, None, None, None],
                [None, one, one, None, None],
                [None, None, None, one, one],
            ]
        );

        // A dot of width 1 is the pixel under the point; erasing clears it.
        let mut r = Raster::clear(UVec2::new(3, 3));
        r.draw_line(
            Vec2::new(1.9, 1.1),
            Vec2::new(1.9, 1.1),
            1.0,
            Some(PaletteIndex(2)),
        );
        assert_eq!(px(&r).iter().flatten().count(), 1);
        assert_eq!(r.pixels()[4], Some(PaletteIndex(2)));
        r.draw_line(Vec2::new(1.5, 1.5), Vec2::new(1.5, 1.5), 1.0, None);
        assert_eq!(r, Raster::clear(UVec2::new(3, 3)));
    }

    #[test]
    fn wide_tips_are_round_and_whole() {
        let count = |width: f32, at: Vec2| {
            let mut r = Raster::clear(UVec2::new(9, 9));
            r.draw_line(at, at, width, Some(PaletteIndex(0)));
            px(&r).iter().flatten().count()
        };
        // Even tips sit on the nearest pixel corner, odd ones on a pixel.
        assert_eq!(count(2.0, Vec2::new(4.2, 3.9)), 4);
        assert_eq!(count(3.0, Vec2::new(4.2, 3.9)), 9);
        // Four by four, without its corners.
        assert_eq!(count(4.0, Vec2::new(4.2, 3.9)), 12);
        // Tips are clipped at the edge.
        assert_eq!(count(3.0, Vec2::new(0.1, 0.1)), 4);
        // The tip the views outline is the one drawn.
        let mut tip = pencil_tip(Vec2::new(4.2, 3.9), 2.0);
        tip.sort_unstable();
        assert_eq!(tip, [(3, 3), (3, 4), (4, 3), (4, 4)]);
    }

    #[test]
    fn fill_spreads_through_edge_neighbors_of_one_value() {
        let one = Some(PaletteIndex(1));
        // A diagonal wall: the fill can't cross it at its corners.
        let mut r = Raster::new(
            UVec2::new(3, 3),
            vec![None, None, one, None, one, None, one, None, None],
        )
        .unwrap();
        r.fill(Vec2::new(0.5, 0.5), Some(PaletteIndex(2)));
        assert_eq!(
            px(&r),
            [
                Some(2),
                Some(2),
                Some(1),
                Some(2),
                Some(1),
                None,
                Some(1),
                None,
                None
            ]
        );
        let before = r.clone();
        r.fill(Vec2::new(-1.0, 0.5), None);
        r.fill(Vec2::new(0.5, 0.5), Some(PaletteIndex(2)));
        assert_eq!(r, before);
    }

    #[test]
    fn painted_texture_needs_a_variant() {
        let err = Texture::painted("eye", Slot("eyes".into()), UVec2::ONE, []).unwrap_err();
        assert!(matches!(err, TextureError::NoVariants { .. }));
    }
}
