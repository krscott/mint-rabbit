//! Animal Crossing (`GameCube`) import and export for mint-rabbit, reading and
//! writing a built `ac-decomp` checkout.

mod anim;
mod c;
mod export;
mod gbi;
mod gx;
mod import;

pub use export::{
    Export, ExportError, ImportedFrom, TextureExport, VILLAGER_JOINT_CHILDREN, export,
    export_textures, imported_from,
};
/// The color the game shows for a palette entry.
pub use gx::quantize_rgb5a3 as quantize_color;
pub use import::{Decomp, ImportError, VillagerRef, WriteError};

#[cfg(test)]
mod roundtrip;

/// The export target id recorded in AC projects.
pub const TARGET: &str = "ac-villager";

/// Export target option of AC projects: the model the villager is drawn on,
/// such as `cat_1`.
pub const OPTION_MODEL: &str = "model";
/// Export target option of AC projects: the texture set the villager was
/// imported from, such as `cat_2`.
pub const OPTION_TEXTURE_SET: &str = "texture_set";

/// Texture slots of the AC villager target.
pub const SLOT_BODY: &str = "body";
pub const SLOT_EYES: &str = "eyes";
pub const SLOT_MOUTH: &str = "mouth";
pub const SLOT_CLOTHING: &str = "clothing";
