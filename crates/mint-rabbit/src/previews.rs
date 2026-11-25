//! Images of a project's variants and layers as egui textures, kept current
//! with the project, and the thumbnails that show them.

use std::collections::HashMap;

use eframe::egui::{
    self, Color32, ColorImage, Rect, Sense, Stroke, TextureHandle, TextureOptions, pos2, vec2,
};
use glam::UVec2;
use mint_core::{Project, Rgba8, VariantRef};

use crate::scene;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    Variant(VariantRef),
    Layer(VariantRef, usize),
}

/// The images of one open project. An image is drawn again when it is asked
/// for after any change to the project.
#[derive(Default)]
pub struct Previews {
    /// Each image with the editor generation it was drawn at.
    images: HashMap<Key, (u64, TextureHandle)>,
}

impl Previews {
    /// A variant as the views show it. For an external texture, its preview,
    /// whichever variant is named.
    pub fn variant(
        &mut self,
        ctx: &egui::Context,
        project: &Project,
        generation: u64,
        r: VariantRef,
    ) -> TextureHandle {
        let texture = project.texture(r.texture);
        self.image(ctx, Key::Variant(r), generation, texture.size(), || {
            scene::texture_pixels(project, texture, r.variant)
        })
    }

    /// One layer of a painted variant alone, at full opacity whether or not
    /// it is hidden.
    pub fn layer(
        &mut self,
        ctx: &egui::Context,
        project: &Project,
        generation: u64,
        r: VariantRef,
        layer: usize,
    ) -> TextureHandle {
        let size = project.texture(r.texture).size();
        self.image(ctx, Key::Layer(r, layer), generation, size, || {
            scene::layer_pixels(project, &project.variant(r).layers()[layer], size)
        })
    }

    fn image(
        &mut self,
        ctx: &egui::Context,
        key: Key,
        generation: u64,
        size: UVec2,
        pixels: impl FnOnce() -> Vec<Rgba8>,
    ) -> TextureHandle {
        if let Some((drawn, handle)) = self.images.get(&key)
            && *drawn == generation
        {
            return handle.clone();
        }
        let pixels: Vec<Color32> = pixels()
            .iter()
            .map(|c| Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a))
            .collect();
        let image = ColorImage::new([size.x as usize, size.y as usize], pixels);
        if let Some((drawn, handle)) = self.images.get_mut(&key) {
            handle.set(image, TextureOptions::NEAREST);
            *drawn = generation;
            return handle.clone();
        }
        let handle = ctx.load_texture("preview", image, TextureOptions::NEAREST);
        self.images.insert(key, (generation, handle.clone()));
        handle
    }
}

/// Fills `rect` with the checkerboard that shows through transparent pixels,
/// in squares `cell` points wide.
pub fn checkerboard(painter: &egui::Painter, rect: Rect, cell: f32) {
    let (cols, rows) = (
        (rect.width() / cell).ceil() as i32,
        (rect.height() / cell).ceil() as i32,
    );
    for y in 0..rows {
        for x in 0..cols {
            let shade = if (x + y) % 2 == 0 { 70 } else { 90 };
            let min = rect.min + vec2(x as f32, y as f32) * cell;
            let r = Rect::from_min_max(min, (min + vec2(cell, cell)).min(rect.max));
            painter.rect_filled(r, 0.0, Color32::from_gray(shade));
        }
    }
}

/// A clickable image fitted into `size`, outlined when `picked`. `image` is
/// asked for only when the thumbnail is in view, as a row scrolled out of a
/// list is not.
pub fn thumbnail(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    picked: bool,
    image: impl FnOnce(&egui::Context) -> TextureHandle,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let image = image(ui.ctx());
    let [w, h] = image.size().map(|n| n.max(1) as f32);
    let scale = (rect.width() / w).min(rect.height() / h);
    let fitted = Rect::from_center_size(rect.center(), vec2(w, h) * scale);
    let visuals = ui.visuals();
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, visuals.extreme_bg_color);
    checkerboard(&painter.with_clip_rect(fitted), fitted, 4.0);
    painter.image(
        image.id(),
        fitted,
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
        Color32::WHITE,
    );
    let stroke = if picked {
        Stroke::new(2.0, visuals.selection.bg_fill)
    } else if response.hovered() {
        visuals.widgets.hovered.bg_stroke
    } else {
        visuals.widgets.noninteractive.bg_stroke
    };
    painter.rect_stroke(rect, 2.0, stroke, egui::StrokeKind::Outside);
    response
}
