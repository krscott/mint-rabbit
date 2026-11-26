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

    /// What the pointer at `pos` meets on the model shown in `rect`.
    pub fn hit(&self, surface: &Surface, rect: Rect, pos: Pos2) -> Option<Hit> {
        let ndc = Vec2::new(
            (pos.x - rect.left()) / rect.width() * 2.0 - 1.0,
            1.0 - (pos.y - rect.top()) / rect.height() * 2.0,
        );
        let (origin, dir) = self.camera.ray(rect.width() / rect.height().max(1.0), ndc);
        surface.hit(origin, dir)
    }

    /// Texels per screen point where the pointer at `pos` meets the model at
    /// `hit`, measured to a neighboring point on the same island; 1 if
    /// there is none.
    pub fn texels_per_point(&self, surface: &Surface, rect: Rect, pos: Pos2, hit: &Hit) -> f32 {
        [
            vec2(1.0, 0.0),
            vec2(-1.0, 0.0),
            vec2(0.0, 1.0),
            vec2(0.0, -1.0),
        ]
        .into_iter()
        .filter_map(|d| self.hit(surface, rect, pos + d))
        .find(|h| h.joins(hit))
        .map_or(1.0, |h| h.point.distance(hit.point))
    }

    /// Starts following a drag pressed at `pos`.
    pub fn press(&mut self, surface: &Surface, rect: Rect, pos: Pos2) {
        self.last = Some((pos, self.hit(surface, rect, pos)));
    }

    /// The pointer's path across `texture` from where it was at the last
    /// frame of the drag to `pos`. Where the path leaves an island, it
    /// reaches the island's edge; where it enters one, it jumps to the
    /// edge it enters by.
    pub fn trail(
        &mut self,
        surface: &Surface,
        rect: Rect,
        pos: Pos2,
        texture: TextureId,
    ) -> Vec<Sample> {
        let hit = self.hit(surface, rect, pos);
        let Some((from, was)) = self.last.replace((pos, hit)) else {
            return Vec::new();
        };
        if from == pos {
            return Vec::new();
        }
        let on = |h: Option<Hit>| h.filter(|h| h.texture == texture);
        let (a, b) = (on(was), on(hit));
        let sample = |h: Hit, jump| Sample {
            point: h.point,
            jump,
        };
        if let (Some(a), Some(b)) = (a, b)
            && a.joins(&b)
        {
            return vec![sample(b, false)];
        }
        let mut path = Vec::new();
        if let Some(a) = a {
            path.push(sample(self.edge(surface, rect, pos, from, &a), false));
        }
        if let Some(b) = b {
            path.push(sample(self.edge(surface, rect, from, pos, &b), true));
            path.push(sample(b, false));
        }
        path
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
        let points: Vec<(Vec2, bool)> = path.iter().map(|s| (s.point, s.jump)).collect();
        let expected = [
            (Vec2::new(2.0, 2.0), false),
            (Vec2::new(4.0, 2.0), true),
            (Vec2::new(5.0, 2.0), false),
        ];
        assert_eq!(points.len(), 3, "{points:?}");
        for ((p, jump), (q, expected_jump)) in points.iter().zip(expected) {
            assert!(p.distance(q) < 0.05 && *jump == expected_jump, "{points:?}");
        }
        // Within one island, the path is just where the pointer went.
        let path = view.trail(&s, rect, screen(1.75), TextureId(0));
        assert_eq!(path.len(), 1);
        assert!(!path[0].jump);
        // Off the model, it ends at the island's edge.
        let path = view.trail(&s, rect, screen(2.5), TextureId(0));
        assert_eq!(path.len(), 1);
        assert!(
            path[0].point.distance(Vec2::new(6.0, 2.0)) < 0.05,
            "{path:?}"
        );
    }
}
