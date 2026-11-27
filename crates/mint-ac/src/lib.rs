//! Animal Crossing (`GameCube`) import and export for mint-rabbit, reading and
//! writing a built `ac-decomp` checkout.

mod anim;
mod c;
mod disc;
mod export;
mod gbi;
mod gx;
mod import;

pub use disc::NamesError;
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

/// How AC model units map to a model file: a villager stands about 3 m tall
/// there rather than 3 km, and normals edited in the file come back about as
/// long as AC stores them, 120, which the import reads as 1/127 of that.
pub const MODEL_FILE_UNITS: mint_core::model_file::Units = mint_core::model_file::Units {
    meters_per_unit: 0.001,
    normal_length: 120.0 / 127.0,
};

/// Texture slots of the AC villager target.
pub const SLOT_BODY: &str = "body";
pub const SLOT_EYES: &str = "eyes";
pub const SLOT_MOUTH: &str = "mouth";
pub const SLOT_CLOTHING: &str = "clothing";
