//! Modifiers: steps of a layer's stack that change what the layer shows and
//! exports without changing the shapes or pixels it holds.
//!
//! Mirror and Radial add copies of the layer's content. Copies draw beneath
//! the content they copy, and a later copy over an earlier one, so a pixel
//! the content covers keeps its value. Their lines and centers lie on the
//! half-texel grid, where reflections and turns carry each pixel exactly
//! onto another, so a modifier gives the same pixels whether its layer holds
//! shapes or the pixels those shapes cover.

use std::collections::{HashSet, VecDeque};

use glam::{Affine2, Mat2, UVec2, Vec2};

use crate::{PaletteIndex, Shape, texel_index};

/// A point of texture space's half-texel grid, which holds pixel corners,
/// pixel centers, and the midpoints of pixel edges. Coordinates count half
/// texels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HalfPoint {
    pub x: i32,
    pub y: i32,
}

impl HalfPoint {
    /// The center of a texture of `size`.
    #[must_use]
    pub fn center(size: UVec2) -> Self {
        Self {
            x: size.x as i32,
            y: size.y as i32,
        }
    }

    /// The grid point nearest `p`, in texels.
    #[must_use]
    pub fn nearest(p: Vec2) -> Self {
        Self {
            x: (p.x * 2.0).round() as i32,
            y: (p.y * 2.0).round() as i32,
        }
    }

    /// The point in texels.
    #[must_use]
    pub fn to_vec2(self) -> Vec2 {
        Vec2::new(self.x as f32, self.y as f32) / 2.0
    }
}

/// Which lines a mirror reflects across: X across the vertical line (left
/// to right), Y across the horizontal line (top to bottom).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Axes {
    X,
    Y,
    Both,
}

impl Axes {
    /// The axes with `x` and `y` set, or `None` for neither.
    #[must_use]
    pub fn from_flags(x: bool, y: bool) -> Option<Self> {
        match (x, y) {
            (true, false) => Some(Self::X),
            (false, true) => Some(Self::Y),
            (true, true) => Some(Self::Both),
            (false, false) => None,
        }
    }

    #[must_use]
    pub fn x(self) -> bool {
        self != Self::Y
    }

    #[must_use]
    pub fn y(self) -> bool {
        self != Self::X
    }
}

/// Copies of a layer reflected across a vertical line, a horizontal line, or
/// both, which cross at `at`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Mirror {
    pub axes: Axes,
    pub at: HalfPoint,
}

/// How far apart a radial modifier's copies are turned.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Turns {
    /// One copy, half a turn around.
    Half,
    /// Three copies, a quarter turn apart.
    Quarter,
}

/// Copies of a layer turned about a center.
///
/// A quarter turn carries pixels onto pixels only about a pixel corner or a
/// pixel center, where both coordinates are whole or both are halves, so a
/// radial modifier with quarter turns always has such a center.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Radial {
    turns: Turns,
    center: HalfPoint,
}

impl Radial {
    /// A radial modifier about the center nearest `p` that `turns` allows.
    #[must_use]
    pub fn nearest(turns: Turns, p: Vec2) -> Self {
        let mut center = HalfPoint::nearest(p);
        if !Self::allows(turns, center) {
            // The nearest point whose coordinates are both whole or both
            // halves is one step along the axis rounded farthest.
            let off = p * 2.0 - center.to_vec2() * 2.0;
            if off.x.abs() >= off.y.abs() {
                center.x += if off.x >= 0.0 { 1 } else { -1 };
            } else {
                center.y += if off.y >= 0.0 { 1 } else { -1 };
            }
        }
        Self { turns, center }
    }

    fn allows(turns: Turns, c: HalfPoint) -> bool {
        turns == Turns::Half || (c.x + c.y).rem_euclid(2) == 0
    }

    #[must_use]
    pub fn turns(self) -> Turns {
        self.turns
    }

    #[must_use]
    pub fn center(self) -> HalfPoint {
        self.center
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModifierKind {
    Mirror(Mirror),
    Radial(Radial),
}

/// One step of a layer's modifier stack. A modifier that is off neither
/// shows nor exports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Modifier {
    pub enabled: bool,
    pub kind: ModifierKind,
}

impl Modifier {
    /// An enabled modifier.
    #[must_use]
    pub fn new(kind: ModifierKind) -> Self {
        Self {
            enabled: true,
            kind,
        }
    }

    /// The motions that place the modifier's copies, in drawing order.
    fn copies(self) -> Vec<Motion> {
        const FLIP_X: [i32; 4] = [-1, 0, 0, 1];
        const FLIP_Y: [i32; 4] = [1, 0, 0, -1];
        const HALF: [i32; 4] = [-1, 0, 0, -1];
        // Clockwise on screen, where y points down.
        const QUARTER: [i32; 4] = [0, -1, 1, 0];
        const THREE_QUARTERS: [i32; 4] = [0, 1, -1, 0];
        let (center, ms): (_, &[[i32; 4]]) = match self.kind {
            ModifierKind::Mirror(m) => (
                m.at,
                match m.axes {
                    Axes::X => &[FLIP_X],
                    Axes::Y => &[FLIP_Y],
                    Axes::Both => &[FLIP_X, FLIP_Y, HALF],
                },
            ),
            ModifierKind::Radial(r) => (
                r.center,
                match r.turns {
                    Turns::Half => &[HALF],
                    Turns::Quarter => &[QUARTER, HALF, THREE_QUARTERS],
                },
            ),
        };
        ms.iter().map(|&m| Motion { m, center }).collect()
    }

    /// `pixels`, of an image of `size`, with the modifier's copies beneath
    /// them.
    pub(crate) fn apply_pixels(
        self,
        size: UVec2,
        pixels: &[Option<PaletteIndex>],
    ) -> Vec<Option<PaletteIndex>> {
        let copies = self.copies();
        let mut out = pixels.to_vec();
        for (i, px) in out.iter_mut().enumerate() {
            if px.is_some() {
                continue;
            }
            let (x, y) = pixel_xy(size, i);
            // The last copy drawn over this pixel wins.
            *px = copies
                .iter()
                .rev()
                .filter_map(|c| c.inverse().pixel(size, x, y))
                .find_map(|j| pixels[j]);
        }
        out
    }

    /// `shapes` with the modifier's copies beneath them, as shapes.
    pub(crate) fn apply_shapes(self, shapes: &[Shape]) -> Vec<Shape> {
        let mut out = Vec::new();
        for c in self.copies() {
            let t = c.affine();
            out.extend(shapes.iter().map(|s| {
                let mut s = s.clone();
                s.transform(t);
                s
            }));
        }
        out.extend_from_slice(shapes);
        out
    }
}

/// The row-major pixel indices of an image of `size` that `modifiers` show
/// as copies of pixel `i`, or that show copies of it, and so on: every pixel
/// an edit at `i` must also make for the layer to look edited there. Holds
/// `i` itself.
pub(crate) fn orbit(modifiers: &[Modifier], size: UVec2, i: usize) -> Vec<usize> {
    let motions: Vec<Motion> = modifiers
        .iter()
        .filter(|m| m.enabled)
        .flat_map(|m| m.copies())
        .flat_map(|c| [c, c.inverse()])
        .collect();
    let mut seen = HashSet::from([i]);
    let mut queue = VecDeque::from([i]);
    while let Some(j) = queue.pop_front() {
        let (x, y) = pixel_xy(size, j);
        for k in motions.iter().filter_map(|c| c.pixel(size, x, y)) {
            if seen.insert(k) {
                queue.push_back(k);
            }
        }
    }
    let mut out: Vec<usize> = seen.into_iter().collect();
    out.sort_unstable();
    out
}

fn pixel_xy(size: UVec2, i: usize) -> (i64, i64) {
    let w = size.x as usize;
    ((i % w) as i64, (i / w) as i64)
}

/// A reflection or turn about `center`: a point `p` goes to
/// `center + m (p - center)`, with `m` row-major.
#[derive(Clone, Copy, Debug)]
struct Motion {
    m: [i32; 4],
    center: HalfPoint,
}

impl Motion {
    fn inverse(self) -> Self {
        // Reflections and turns are orthogonal: the inverse is the
        // transpose.
        let [a, b, c, d] = self.m;
        Self {
            m: [a, c, b, d],
            center: self.center,
        }
    }

    fn affine(self) -> Affine2 {
        let [a, b, c, d] = self.m.map(|v| v as f32);
        let center = self.center.to_vec2();
        let m = Mat2::from_cols(Vec2::new(a, c), Vec2::new(b, d));
        Affine2::from_mat2_translation(m, center - m * center)
    }

    /// The row-major index of the pixel that pixel `(x, y)` goes to in an
    /// image of `size`, or `None` outside it.
    fn pixel(self, size: UVec2, x: i64, y: i64) -> Option<usize> {
        let [a, b, c, d] = self.m.map(i64::from);
        let (cx, cy) = (i64::from(self.center.x), i64::from(self.center.y));
        // Pixel centers in half texels, which are odd.
        let (px, py) = (2 * x + 1 - cx, 2 * y + 1 - cy);
        let (qx, qy) = (a * px + b * py + cx, c * px + d * py + cy);
        debug_assert!(qx.rem_euclid(2) == 1 && qy.rem_euclid(2) == 1);
        texel_index(size, (qx - 1).div_euclid(2), (qy - 1).div_euclid(2))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Anchor, Style};

    fn mirror(axes: Axes, x: i32, y: i32) -> Modifier {
        Modifier::new(ModifierKind::Mirror(Mirror {
            axes,
            at: HalfPoint { x, y },
        }))
    }

    fn radial(turns: Turns, p: Vec2) -> Modifier {
        Modifier::new(ModifierKind::Radial(Radial::nearest(turns, p)))
    }

    /// An image of `size` from rows of `.` (unset) and digits.
    fn image(rows: &[&str]) -> (UVec2, Vec<Option<PaletteIndex>>) {
        let size = UVec2::new(rows[0].len() as u32, rows.len() as u32);
        let px = rows
            .iter()
            .flat_map(|r| r.chars())
            .map(|c| c.to_digit(10).map(|d| PaletteIndex(d as u8)))
            .collect();
        (size, px)
    }

    fn rows(size: UVec2, px: &[Option<PaletteIndex>]) -> Vec<String> {
        px.chunks(size.x as usize)
            .map(|r| {
                r.iter()
                    .map(|p| p.map_or('.', |i| char::from(b'0' + i.0)))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn mirror_x_copies_beneath_the_content() {
        let (size, px) = image(&["1..2.", "....."]);
        // The line through the middle of pixel 2 of 5.
        let out = mirror(Axes::X, 5, 0).apply_pixels(size, &px);
        assert_eq!(rows(size, &out), ["12.21", "....."]);
        let out = mirror(Axes::X, 5, 0).apply_pixels(size, &image(&["1...2", "....."]).1);
        // Each side keeps its own pixel where the copies land on content.
        assert_eq!(rows(size, &out), ["1...2", "....."]);
    }

    #[test]
    fn mirror_lines_off_center_clip_copies() {
        let (size, px) = image(&["12..", "...."]);
        // A vertical line between pixels 2 and 3 throws the X copies past
        // the right edge; the Y copies stay.
        let out = mirror(Axes::Both, 6, 2).apply_pixels(size, &px);
        assert_eq!(rows(size, &out), ["12..", "12.."]);
    }

    #[test]
    fn later_copies_draw_over_earlier_ones() {
        // Mirror Both draws the X copy, then the Y copy, then the half turn.
        let (size, px) = image(&["1..", "...", "..2"]);
        let out = mirror(Axes::Both, 3, 3).apply_pixels(size, &px);
        // Top right: X copy of 1 under the Y copy of 2.
        assert_eq!(rows(size, &out), ["1.2", "...", "1.2"]);
    }

    #[test]
    fn quarter_turns_carry_pixels_around() {
        let (size, px) = image(&["1...", "....", "....", "...."]);
        let out = radial(Turns::Quarter, Vec2::new(2.0, 2.0)).apply_pixels(size, &px);
        assert_eq!(rows(size, &out), ["1..1", "....", "....", "1..1"]);
        // Clockwise on screen: the top-left pixel's first copy is top right.
        let (size, px) = image(&["12..", "....", "....", "...."]);
        let out = radial(Turns::Quarter, Vec2::new(2.0, 2.0)).apply_pixels(size, &px);
        assert_eq!(rows(size, &out), ["12.1", "...2", "2...", "1.21"]);
    }

    #[test]
    fn radial_centers_snap_to_where_quarter_turns_stay_on_pixels() {
        let r = Radial::nearest(Turns::Quarter, Vec2::new(2.4, 2.0));
        assert_eq!(r.center(), HalfPoint { x: 4, y: 4 });
        let r = Radial::nearest(Turns::Quarter, Vec2::new(2.6, 2.1));
        assert_eq!(r.center(), HalfPoint { x: 6, y: 4 });
        let r = Radial::nearest(Turns::Quarter, Vec2::new(2.5, 2.5));
        assert_eq!(r.center(), HalfPoint { x: 5, y: 5 });
        let r = Radial::nearest(Turns::Half, Vec2::new(2.5, 2.0));
        assert_eq!(r.center(), HalfPoint { x: 5, y: 4 });
    }

    #[test]
    fn shapes_and_their_pixels_get_the_same_copies() {
        let size = UVec2::new(9, 7);
        let square = Shape::new(
            [(1.0, 1.0), (3.4, 1.0), (3.4, 2.6), (1.0, 2.6)]
                .map(|(x, y)| Anchor::corner(Vec2::new(x, y)))
                .to_vec(),
            Style::Fill,
            PaletteIndex(3),
        )
        .unwrap();
        let mut pixels = vec![None; 63];
        for (i, px) in pixels.iter_mut().enumerate() {
            let (x, y) = pixel_xy(size, i);
            if square
                .outline()
                .covers(Vec2::new(x as f32 + 0.5, y as f32 + 0.5))
            {
                *px = Some(PaletteIndex(3));
            }
        }
        for m in [
            mirror(Axes::Both, 9, 7),
            mirror(Axes::X, 7, 0),
            radial(Turns::Quarter, Vec2::new(4.5, 3.5)),
            radial(Turns::Half, Vec2::new(4.0, 3.5)),
        ] {
            let shapes = m.apply_shapes(std::slice::from_ref(&square));
            let mut drawn = vec![None; 63];
            for s in &shapes {
                for (i, px) in drawn.iter_mut().enumerate() {
                    let (x, y) = pixel_xy(size, i);
                    if s.outline()
                        .covers(Vec2::new(x as f32 + 0.5, y as f32 + 0.5))
                    {
                        *px = Some(s.color());
                    }
                }
            }
            assert_eq!(
                rows(size, &drawn),
                rows(size, &m.apply_pixels(size, &pixels)),
                "{m:?}"
            );
        }
    }

    #[test]
    fn an_orbit_holds_every_pixel_an_edit_must_reach() {
        let size = UVec2::new(4, 4);
        let ms = [mirror(Axes::X, 4, 0), mirror(Axes::Y, 0, 4)];
        assert_eq!(orbit(&ms, size, 0), [0, 3, 12, 15]);
        let mut off = ms;
        off[1].enabled = false;
        assert_eq!(orbit(&off, size, 0), [0, 3]);
        // Copies off the image don't count.
        assert_eq!(orbit(&[mirror(Axes::X, 2, 0)], size, 2), [2]);
    }
}
