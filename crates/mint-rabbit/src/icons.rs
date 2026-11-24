//! Lucide icons, drawn as text in a font family of their own.

use eframe::egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
use eframe::egui::{self, FontData, FontFamily, FontId, RichText};
pub use lucide_icons::Icon;

const FAMILY: &str = "icons";

/// Icon height in buttons beside text.
pub const SMALL: f32 = 14.0;
/// Icon height in buttons that hold only an icon.
pub const LARGE: f32 = 18.0;

/// Adds the icon font, which [`text`] needs.
pub fn install(ctx: &egui::Context) {
    ctx.add_font(FontInsert::new(
        FAMILY,
        FontData::from_static(lucide_icons::LUCIDE_FONT_BYTES),
        vec![InsertFontFamily {
            family: FontFamily::Name(FAMILY.into()),
            priority: FontPriority::Highest,
        }],
    ));
}

/// `icon` as text `size` points high.
pub fn text(icon: Icon, size: f32) -> RichText {
    RichText::new(icon.unicode()).font(FontId::new(size, FontFamily::Name(FAMILY.into())))
}
