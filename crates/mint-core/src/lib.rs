//! Target-independent domain model for mint-rabbit. See `GLOSSARY.md` for
//! the meaning of each term.

mod model;
mod pose;
mod project;
pub mod shape;
mod texture;

pub use model::{Corner, Mesh, Model, Node, NodeId, Transform, Triangle, Vertex};
pub use pose::Pose;
pub use project::{PaletteError, Project, ProjectError, TargetId, VariantRef};
pub use shape::{Anchor, Outline, Shape, ShapeError, Style};
pub use texture::{
    AlphaMode, IndexedImage, Layer, Material, MaterialId, Palette, PaletteFull, PaletteIndex,
    PixelCountMismatch, Rgba8, RgbaImage, Slot, Texture, TextureContent, TextureError, TextureId,
    Variant, Wrap,
};
