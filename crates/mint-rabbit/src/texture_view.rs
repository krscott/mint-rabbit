//! The Texture View: one texture variant, pixel-exact, with the UV layout of
//! the triangles that use it drawn on top.

use eframe::egui::{
    self, Color32, ColorImage, Rect, Sense, Stroke, TextureHandle, TextureOptions, pos2, vec2,
};
use mint_core::{Project, TextureId};

use crate::scene;

/// Identifies the image shown, so the uploaded texture is reused until it
/// changes.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    generation: u64,
    texture: TextureId,
    variant: usize,
}

#[derive(Default)]
pub struct TextureView {
    cached: Option<(Key, TextureHandle)>,
}

impl TextureView {
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        project: &Project,
        generation: u64,
        texture: TextureId,
        variant: usize,
    ) {
        let key = Key {
            generation,
            texture,
            variant,
        };
        let tex = project.texture(texture);
        let handle = match &self.cached {
            Some((k, h)) if *k == key => h.clone(),
            _ => {
                let size = tex.size();
                let pixels: Vec<Color32> = scene::texture_pixels(project, tex, variant)
                    .iter()
                    .map(|c| Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a))
                    .collect();
                let image = ColorImage::new([size.x as usize, size.y as usize], pixels);
                let h = ui
                    .ctx()
                    .load_texture("texture view", image, TextureOptions::NEAREST);
                self.cached = Some((key, h.clone()));
                h
            }
        };

        let (rect, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
        let size = tex.size().as_vec2();
        let scale = (rect.width() / size.x).min(rect.height() / size.y).max(1.0);
        let image_rect = Rect::from_center_size(rect.center(), vec2(size.x, size.y) * scale);
        let painter = ui.painter_at(rect);

        // Checkerboard behind transparent pixels.
        let cell = (scale * 2.0).max(8.0);
        let (cols, rows) = (
            (image_rect.width() / cell).ceil() as i32,
            (image_rect.height() / cell).ceil() as i32,
        );
        for y in 0..rows {
            for x in 0..cols {
                let shade = if (x + y) % 2 == 0 { 70 } else { 90 };
                let min = image_rect.min + vec2(x as f32, y as f32) * cell;
                let r = Rect::from_min_max(min, (min + vec2(cell, cell)).min(image_rect.max));
                painter.rect_filled(r, 0.0, Color32::from_gray(shade));
            }
        }
        painter.image(
            handle.id(),
            image_rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );

        let to_screen = |uv: glam::Vec2| {
            image_rect.min + vec2(uv.x * image_rect.width(), uv.y * image_rect.height())
        };
        let stroke = Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 0, 160));
        for tri in scene::uv_triangles(project, texture) {
            let p = tri.map(to_screen);
            painter.line_segment([p[0], p[1]], stroke);
            painter.line_segment([p[1], p[2]], stroke);
            painter.line_segment([p[2], p[0]], stroke);
        }
        painter.rect_stroke(
            image_rect,
            0.0,
            Stroke::new(1.0, Color32::GRAY),
            egui::StrokeKind::Outside,
        );
    }
}
