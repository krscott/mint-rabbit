//! Target-independent domain model for mint-rabbit. See `GLOSSARY.md` for
//! the meaning of each term.

mod model;
mod pose;
mod project;
mod texture;

pub use model::{Corner, Mesh, Model, Node, NodeId, Transform, Triangle, Vertex};
pub use pose::Pose;
pub use project::{Project, ProjectError, TargetId};
pub use texture::{
    AlphaMode, IndexedImage, Material, MaterialId, Palette, PaletteFull, PaletteIndex,
    PixelCountMismatch, Rgba8, RgbaImage, Slot, Texture, TextureContent, TextureError, TextureId,
    Variant, Wrap,
};
