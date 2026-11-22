//! Animal Crossing (`GameCube`) import and export for mint-rabbit, reading and
//! writing a built `ac-decomp` checkout.

mod anim;
mod c;
mod export;
mod gbi;
mod gx;
mod import;

pub use export::{
    Export, ExportError, TextureExport, VILLAGER_JOINT_CHILDREN, export, export_textures,
};
/// The color the game shows for a palette entry.
pub use gx::quantize_rgb5a3 as quantize_color;
pub use import::{Decomp, ImportError, VillagerRef, WriteError};

#[cfg(test)]
mod roundtrip;

/// The export target id recorded in AC projects.
pub const TARGET: &str = "ac-villager";

/// Export target options of AC projects: the model the villager is drawn
/// on, such as `cat_1`, and the texture set it was imported from, such as
/// `cat_2`.
pub const OPTION_MODEL: &str = "model";
pub const OPTION_TEXTURE_SET: &str = "texture_set";

/// Texture slots of the AC villager target.
pub const SLOT_BODY: &str = "body";
pub const SLOT_EYES: &str = "eyes";
pub const SLOT_MOUTH: &str = "mouth";
pub const SLOT_CLOTHING: &str = "clothing";
