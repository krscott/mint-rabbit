//! The Model View's camera and modes, and where the pointer meets the model.

use eframe::egui::{Pos2, Rect, vec2};
use glam::Vec2;
use mint_core::TextureId;

use crate::camera::Orbit;
use crate::surface::{Hit, Surface};
use crate::texture_view::Sample;

/// Halvings of the pointer's path that find where it leaves or enters a UV
/// island.
const STEPS: u32 = 12;
/// The longest step, in screen points, the pointer's path is followed in,
/// so that it doesn't skip an island it crosses within a frame.
const STEP: f32 = 4.0;

/// What dragging with the primary button does in the Model View.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// It turns the camera around the model.
    #[default]
    Orbit,
    /// The picked tool draws on the model.
    Paint,
}

pub struct ModelView {
    pub camera: Orbit,
    pub mode: Mode,
    /// Whether the pointer is over the view this frame.
    pub hovered: bool,
    /// Where the pointer was at the last frame of a drag, and what it hit
    /// there.
    last: Option<(Pos2, Option<Hit>)>,
}

impl ModelView {
    pub fn new(camera: Orbit) -> Self {
        Self {
            camera,
            mode: Mode::default(),
            hovered: false,
            last: None,
        }
    }

    /// What the pointer at `pos` meets on the model shown in `rect`;
    /// nothing outside it, where the model isn't shown.
    pub fn hit(&self, surface: &Surface, rect: Rect, pos: Pos2) -> Option<Hit> {
        if !rect.contains(pos) {
            return None;
        }
        let ndc = Vec2::new(
            (pos.x - rect.left()) / rect.width() * 2.0 - 1.0,
            1.0 - (pos.y - rect.top()) / rect.height() * 2.0,
        );
        let (origin, dir, length) = self.camera.ray(rect.width() / rect.height().max(1.0), ndc);
        surface.hit(origin, dir, length)
    }

    /// Texels per screen point where the pointer at `pos` meets the model at
    /// `hit`, measured to a neighboring point that joins it; 1 if none does
    /// at a distance.
    pub fn texels_per_point(&self, surface: &Surface, rect: Rect, pos: Pos2, hit: &Hit) -> f32 {
        [
            vec2(1.0, 0.0),
            vec2(-1.0, 0.0),
            vec2(0.0, 1.0),
            vec2(0.0, -1.0),
        ]
        .into_iter()
        .filter_map(|d| self.hit(surface, rect, pos + d))
        .filter(|h| h.joins(hit))
        .map(|h| h.point.distance(hit.point))
        .find(|&d| d > 0.0)
        .unwrap_or(1.0)
    }

    /// Starts following a drag pressed at `pos`, and returns what the press
    /// hit.
    pub fn press(&mut self, surface: &Surface, rect: Rect, pos: Pos2) -> Option<Hit> {
        let hit = self.hit(surface, rect, pos);
        self.last = Some((pos, hit));
        hit
    }

    /// The pointer's path across `texture` from where it was at the last
    /// frame of the drag to `pos`. Where the path leaves a UV island, it
    /// reaches the island's edge; where it enters one, it jumps to the edge
    /// it enters by.
    pub fn trail(
        &mut self,
        surface: &Surface,
        rect: Rect,
        pos: Pos2,
        texture: TextureId,
    ) -> Vec<Sample> {
        let Some((from, _)) = self.last else {
            return Vec::new();
        };
        let steps = (from.distance(pos) / STEP).ceil() as usize;
        let mut path = Vec::new();
        for i in 1..=steps {
            let to = from.lerp(pos, i as f32 / steps as f32);
            self.step(surface, rect, to, texture, &mut path);
        }
        path
    }

    /// One step of [`Self::trail`], to `pos`, added to `path`.
    fn step(
        &mut self,
        surface: &Surface,
        rect: Rect,
        pos: Pos2,
        texture: TextureId,
        path: &mut Vec<Sample>,
    ) {
        let hit = self.hit(surface, rect, pos);
        let Some((from, was)) = self.last.replace((pos, hit)) else {
            return;
        };
        let on = |h: Option<Hit>| h.filter(|h| h.texture == texture);
        let (a, b) = (on(was), on(hit));
        let sample = |h: Hit, jump| Sample {
            point: h.point,
            jump,
        };
        if let (Some(a), Some(b)) = (a, b)
            && a.joins(&b)
        {
            path.push(sample(b, false));
            return;
        }
        if let Some(a) = a {
            path.push(sample(self.edge(surface, rect, pos, from, &a), false));
        }
        if let Some(b) = b {
            path.push(sample(self.edge(surface, rect, from, pos, &b), true));
            path.push(sample(b, false));
        }
    }

    /// Of the points from `outside` to `inside`, whose hit `to` is, the
    /// hit nearest `outside` that joins `to`.
    fn edge(&self, surface: &Surface, rect: Rect, outside: Pos2, inside: Pos2, to: &Hit) -> Hit {
        let (mut outside, mut inside, mut best) = (outside, inside, *to);
        for _ in 0..STEPS {
            let mid = outside.lerp(inside, 0.5);
            match self.hit(surface, rect, mid) {
                Some(h) if h.joins(to) => {
                    inside = mid;
                    best = h;
                }
                _ => outside = mid,
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use eframe::egui::pos2;
    use glam::Vec3;

    use super::*;
    use crate::surface::tests::{OPAQUE, square, surface};

    #[test]
    fn a_path_across_islands_breaks_at_their_edges() {
        // Two squares side by side on an 8x4 texture, sharing an edge in
        // space but not in UV: u runs 0 to 0.25 on the left and 0.5 to 0.75
        // on the right, so their edge is texel x 2 on one and 4 on the
        // other.
        let mut tris = square(0, 0.0, 0.0, [0.0, 0.0], [0.25, 1.0]).to_vec();
        tris.extend(square(0, 1.0, 0.0, [0.5, 0.0], [0.75, 1.0]));
        let s = surface(&[OPAQUE], &tris);
        // A camera looking down -Z at the squares.
        let camera = Orbit {
            yaw: 0.0,
            pitch: 0.0,
            distance: 4.0,
            target: Vec3::new(1.0, 0.5, 0.0),
        };
        let mut view = ModelView::new(camera);
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 200.0));
        let view_proj = view.camera.view_proj(1.0);
        let screen = |x: f32| {
            let ndc = view_proj.project_point3(Vec3::new(x, 0.5, 0.0));
            pos2((ndc.x + 1.0) * 100.0, (1.0 - ndc.y) * 100.0)
        };
        let (from, to) = (screen(0.5), screen(1.5));
        view.press(&s, rect, from);
        let path = view.trail(&s, rect, to, TextureId(0));
        let near = |s: &Sample, x: f32| s.point.distance(Vec2::new(x, 2.0)) < 0.05;
        // One jump, from the left square's edge to the right one's.
        let jumps: Vec<usize> = (0..path.len()).filter(|&i| path[i].jump).collect();
        assert_eq!(jumps.len(), 1, "{path:?}");
        let j = jumps[0];
        assert!(near(&path[j - 1], 2.0) && near(&path[j], 4.0), "{path:?}");
        assert!(near(path.last().unwrap(), 5.0), "{path:?}");
        // Within one island, the path is just where the pointer went.
        let path = view.trail(&s, rect, screen(1.75), TextureId(0));
        assert!(path.iter().all(|s| !s.jump), "{path:?}");
        assert!(near(path.last().unwrap(), 5.5), "{path:?}");
        // Off the model, it ends at the island's edge.
        let path = view.trail(&s, rect, screen(2.5), TextureId(0));
        assert!(path.iter().all(|s| !s.jump), "{path:?}");
        assert!(near(path.last().unwrap(), 6.0), "{path:?}");
    }
}
