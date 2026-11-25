//! Target-independent domain model for mint-rabbit. See `GLOSSARY.md` for
//! the meaning of each term.

pub mod file;
mod model;
mod pose;
mod project;
pub mod shape;
mod texture;

pub use model::{Corner, Mesh, Model, Node, NodeId, Transform, Triangle, Vertex};
pub use pose::Pose;
pub use project::{EditError, ExportTarget, PaletteError, Project, ProjectError, VariantRef};
pub use shape::{Anchor, Outline, Shape, ShapeError, Style};
pub use texture::{
    AlphaMode, BASE_LAYER, IndexedImage, Layer, LayerContent, LayerKind, Material, MaterialId,
    Palette, PaletteFull, PaletteIndex, PixelCountMismatch, Raster, Rgba8, RgbaImage, Slot,
    Texture, TextureContent, TextureError, TextureId, Variant, Wrap,
};
