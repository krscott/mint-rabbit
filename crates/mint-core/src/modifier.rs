//! Modifiers: steps of a layer's stack that change what the layer shows and
//! exports without changing the shapes or pixels it holds.
//!
//! Mirror and Radial add copies of the pixels a layer covers, whether it
//! holds shapes or pixels. Copies draw beneath the content they copy, and a
//! later copy over an earlier one, so a pixel the content covers keeps its
//! value; a copy that leaves the image is gone, also for later modifiers.
//! Their lines and centers lie on the half-texel grid within the texture,
//! where reflections and turns carry each pixel exactly onto another.

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

    /// The grid point nearest `p`, in texels, on a texture of `size`.
    #[must_use]
    pub fn nearest(p: Vec2, size: UVec2) -> Self {
        Self {
            x: snap(p.x * 2.0, 2 * size.x as i32, None),
            y: snap(p.y * 2.0, 2 * size.y as i32, None),
        }
    }

    /// The point with its coordinate `axis` (0 across, 1 down) moved to the
    /// nearest value to `v`, in texels, on a texture of `size`, and of
    /// `parity` in half texels if given.
    fn with_coordinate(self, axis: usize, v: f32, size: UVec2, parity: Option<i32>) -> Self {
        let mut c = [self.x, self.y];
        c[axis] = snap(v * 2.0, 2 * size[axis] as i32, parity);
        Self { x: c[0], y: c[1] }
    }

    /// The point in texels.
    #[must_use]
    pub fn to_vec2(self) -> Vec2 {
        Vec2::new(self.x as f32, self.y as f32) / 2.0
    }
}

/// The whole number nearest `v` from 0 to `max` (at least 2), and with
/// `parity` of that parity.
fn snap(v: f32, max: i32, parity: Option<i32>) -> i32 {
    let v = if v.is_nan() {
        0.0
    } else {
        v.clamp(0.0, max as f32)
    };
    let r = v.round() as i32;
    match parity {
        Some(p) if (r - p).rem_euclid(2) != 0 => {
            // One step toward `v`, or the other way at the ends.
            let toward = if v >= r as f32 { r + 1 } else { r - 1 };
            if (0..=max).contains(&toward) {
                toward
            } else {
                2 * r - toward
            }
        }
        _ => r,
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

/// Copies of a layer turned about a center within its texture.
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
    /// A radial modifier about the center nearest `p`, in texels, that
    /// `turns` allows on a texture of `size`.
    #[must_use]
    pub fn nearest(turns: Turns, p: Vec2, size: UVec2) -> Self {
        let (vx, vy) = (p.x * 2.0, p.y * 2.0);
        let (mx, my) = (2 * size.x as i32, 2 * size.y as i32);
        let center = match turns {
            Turns::Half => HalfPoint {
                x: snap(vx, mx, None),
                y: snap(vy, my, None),
            },
            Turns::Quarter => {
                // Round one coordinate freely and the other to match it,
                // whichever lands nearer.
                let x = snap(vx, mx, None);
                let a = HalfPoint {
                    x,
                    y: snap(vy, my, Some(x)),
                };
                let y = snap(vy, my, None);
                let b = HalfPoint {
                    x: snap(vx, mx, Some(y)),
                    y,
                };
                let target = Vec2::new(vx, vy).clamp(Vec2::ZERO, Vec2::new(mx as f32, my as f32));
                let d = |c: HalfPoint| Vec2::new(c.x as f32, c.y as f32).distance_squared(target);
                if d(b) < d(a) { b } else { a }
            }
        };
        Self { turns, center }
    }

    /// The modifier with its center's coordinate `axis` (0 across, 1 down)
    /// moved to the nearest value to `v`, in texels, on a texture of `size`.
    /// The other coordinate stays, so with quarter turns this one moves a
    /// whole texel at a time.
    #[must_use]
    pub fn with_coordinate(self, axis: usize, v: f32, size: UVec2) -> Self {
        let other = [self.center.x, self.center.y][1 - axis];
        let parity = (self.turns == Turns::Quarter).then_some(other);
        Self {
            turns: self.turns,
            center: self.center.with_coordinate(axis, v, size, parity),
        }
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

impl ModifierKind {
    /// Where the modifier's guide lines cross, and whether it has the
    /// vertical line (`[0]`) and the horizontal one (`[1]`). A radial
    /// center is where its two lines cross.
    #[must_use]
    pub fn guides(self) -> (HalfPoint, [bool; 2]) {
        match self {
            Self::Mirror(m) => (m.at, [m.axes.x(), m.axes.y()]),
            Self::Radial(r) => (r.center, [true, true]),
        }
    }

    /// The modifier with guide line `axis` (0 the vertical line, 1 the
    /// horizontal one) moved to the nearest place to `v`, in texels, it can
    /// be on a texture of `size`; see [`Self::step`].
    #[must_use]
    pub fn with_coordinate(self, axis: usize, v: f32, size: UVec2) -> Self {
        match self {
            Self::Mirror(m) => Self::Mirror(Mirror {
                at: m.at.with_coordinate(axis, v, size, None),
                ..m
            }),
            Self::Radial(r) => Self::Radial(r.with_coordinate(axis, v, size)),
        }
    }

    /// How far, in texels, a guide line moves at a time: half a texel, or a
    /// whole one for quarter turns.
    #[must_use]
    pub fn step(self) -> f32 {
        match self {
            Self::Radial(r) if r.turns == Turns::Quarter => 1.0,
            _ => 0.5,
        }
    }

    /// The modifier with its guides through the center of a texture of
    /// `size`, or as near as it allows.
    #[must_use]
    pub fn centered(self, size: UVec2) -> Self {
        let c = HalfPoint::center(size);
        match self {
            Self::Mirror(m) => Self::Mirror(Mirror { at: c, ..m }),
            Self::Radial(r) => Self::Radial(Radial::nearest(r.turns, c.to_vec2(), size)),
        }
    }
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
        const FLIP_X: [i64; 4] = [-1, 0, 0, 1];
        const FLIP_Y: [i64; 4] = [1, 0, 0, -1];
        const HALF: [i64; 4] = [-1, 0, 0, -1];
        // Clockwise on screen, where y points down.
        const QUARTER: [i64; 4] = [0, -1, 1, 0];
        const THREE_QUARTERS: [i64; 4] = [0, 1, -1, 0];
        let (center, ms): (_, &[[i64; 4]]) = match self.kind {
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
        ms.iter().map(|&m| Motion::about(m, center)).collect()
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
            // The last copy drawn over this pixel wins.
            *px = copies
                .iter()
                .rev()
                .filter_map(|c| c.inverse().pixel(size, i))
                .find_map(|j| pixels[j]);
        }
        out
    }

    /// `shapes` with the modifier's copies beneath them, as shapes. Unlike
    /// [`Self::apply_pixels`] on the pixels they cover, these copy whole
    /// shapes, so they can bring parts from past the texture's edge into
    /// view, and a pixel center lying exactly on a copied shape's edge can
    /// fall the other way.
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

/// The copies of each enabled modifier of `modifiers`, in stack order.
pub(crate) fn stack(modifiers: &[Modifier]) -> Vec<Vec<Motion>> {
    modifiers
        .iter()
        .filter(|m| m.enabled)
        .map(|m| m.copies())
        .collect()
}

/// The pixels of an image of `size` that `stack` shows as the same as
/// pixels in `from`: those it shows copies of there, and every pixel showing
/// a copy of one of those, through copies on the image only, as the stack
/// draws them. Holds `from` too.
pub(crate) fn matching(stack: &[Vec<Motion>], size: UVec2, from: &[usize]) -> Vec<usize> {
    let inverses = stack
        .iter()
        .rev()
        .map(|copies| copies.iter().map(|c| c.inverse()).collect());
    let sources = reach(inverses, size, from);
    let sources: Vec<usize> = (0..sources.len()).filter(|&j| sources[j]).collect();
    let shown = reach(stack.iter().cloned(), size, &sources);
    (0..shown.len()).filter(|&j| shown[j]).collect()
}

/// Which pixels of an image of `size` the pixels in `from` reach
/// when each step of `steps` adds its motions' images of the pixels reached
/// so far that land on the image.
fn reach(steps: impl Iterator<Item = Vec<Motion>>, size: UVec2, from: &[usize]) -> Vec<bool> {
    let mut seen = vec![false; (u64::from(size.x) * u64::from(size.y)) as usize];
    for &i in from {
        seen[i] = true;
    }
    let mut reached = from.to_vec();
    for motions in steps {
        for k in 0..reached.len() {
            for m in &motions {
                if let Some(j) = m.pixel(size, reached[k])
                    && !std::mem::replace(&mut seen[j], true)
                {
                    reached.push(j);
                }
            }
        }
    }
    seen
}

/// A rigid motion of texture space that carries pixels onto pixels: a point
/// `p`, in half texels, goes to `m p + t`, with `m` a row-major reflection
/// or turn and `t` even.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Motion {
    m: [i64; 4],
    t: [i64; 2],
}

impl Motion {
    /// `m` about `center`, which fixes it.
    fn about(m: [i64; 4], center: HalfPoint) -> Self {
        let c = [i64::from(center.x), i64::from(center.y)];
        let mc = mul(m, c);
        Self {
            m,
            t: [c[0] - mc[0], c[1] - mc[1]],
        }
    }

    fn inverse(self) -> Self {
        // Reflections and turns are orthogonal: the inverse is the
        // transpose.
        let [a, b, c, d] = self.m;
        let m = [a, c, b, d];
        let t = mul(m, self.t);
        Self {
            m,
            t: [-t[0], -t[1]],
        }
    }

    fn affine(self) -> Affine2 {
        let [a, b, c, d] = self.m.map(|v| v as f32);
        let m = Mat2::from_cols(Vec2::new(a, c), Vec2::new(b, d));
        Affine2::from_mat2_translation(m, Vec2::new(self.t[0] as f32, self.t[1] as f32) / 2.0)
    }

    /// The row-major index of the pixel that pixel `i` goes to in an image
    /// of `size`, or `None` outside it.
    pub(crate) fn pixel(self, size: UVec2, i: usize) -> Option<usize> {
        let w = size.x as usize;
        // The pixel's center in half texels, which is odd.
        let p = [2 * (i % w) as i64 + 1, 2 * (i / w) as i64 + 1];
        let q = mul(self.m, p);
        let (qx, qy) = (q[0] + self.t[0], q[1] + self.t[1]);
        debug_assert!(qx.rem_euclid(2) == 1 && qy.rem_euclid(2) == 1);
        texel_index(size, (qx - 1).div_euclid(2), (qy - 1).div_euclid(2))
    }
}

fn mul([a, b, c, d]: [i64; 4], [x, y]: [i64; 2]) -> [i64; 2] {
    [a * x + b * y, c * x + d * y]
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
        Modifier::new(ModifierKind::Radial(Radial::nearest(
            turns,
            p,
            UVec2::splat(64),
        )))
    }

    fn pixel_xy(size: UVec2, i: usize) -> (i64, i64) {
        let w = size.x as usize;
        ((i % w) as i64, (i / w) as i64)
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
        let big = UVec2::splat(64);
        let at = |turns, x, y| Radial::nearest(turns, Vec2::new(x, y), big).center();
        assert_eq!(at(Turns::Quarter, 2.4, 2.0), HalfPoint { x: 4, y: 4 });
        assert_eq!(at(Turns::Quarter, 2.6, 2.2), HalfPoint { x: 5, y: 5 });
        assert_eq!(at(Turns::Quarter, 2.5, 2.5), HalfPoint { x: 5, y: 5 });
        assert_eq!(at(Turns::Half, 2.5, 2.0), HalfPoint { x: 5, y: 4 });
        // Centers stay on the texture, also when snapping pushes them.
        let small = UVec2::splat(4);
        let r = Radial::nearest(Turns::Quarter, Vec2::new(4.0, 0.5), small);
        assert_eq!(r.center(), HalfPoint { x: 8, y: 2 });
        let r = Radial::nearest(Turns::Half, Vec2::new(1e30, f32::NAN), small);
        assert_eq!(r.center(), HalfPoint { x: 8, y: 0 });
        // Moving one coordinate keeps the other, a whole texel at a time.
        let r = r.with_coordinate(1, 2.6, small);
        assert_eq!(r.center(), HalfPoint { x: 8, y: 5 });
        let q = Radial::nearest(Turns::Quarter, Vec2::new(2.0, 2.0), small);
        assert_eq!(
            q.with_coordinate(0, 2.6, small).center(),
            HalfPoint { x: 6, y: 4 }
        );
        assert_eq!(
            q.with_coordinate(1, 4.0, small).center(),
            HalfPoint { x: 4, y: 8 }
        );
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

    fn matches(ms: &[Modifier], size: UVec2, i: usize) -> Vec<usize> {
        matching(&stack(ms), size, &[i])
    }

    #[test]
    fn matching_pixels_follow_the_stack() {
        let size = UVec2::new(4, 4);
        let ms = [mirror(Axes::X, 4, 0), mirror(Axes::Y, 0, 4)];
        assert_eq!(matches(&ms, size, 0), [0, 3, 12, 15]);
        let mut off = ms;
        off[1].enabled = false;
        assert_eq!(matches(&off, size, 0), [0, 3]);
        // Copies off the image don't count.
        assert_eq!(matches(&[mirror(Axes::X, 2, 0)], size, 2), [2]);
        // Mirrors on different lines reach only what one pass through the
        // stack shows, and the way back.
        let size = UVec2::new(8, 1);
        let ms = [mirror(Axes::X, 4, 0), mirror(Axes::X, 6, 0)];
        assert_eq!(matches(&ms, size, 0), [0, 2, 3, 5]);
        // Pixels showing copies of the same pixel match each other too.
        assert_eq!(matches(&ms, size, 5), [0, 2, 3, 5]);
        // Several pixels match what each matches.
        assert_eq!(matching(&stack(&ms), size, &[0, 7]), [0, 2, 3, 5, 7]);
        // A copy cut off at the edge carries nothing on to later modifiers.
        let ms = [mirror(Axes::X, 2, 0), mirror(Axes::X, 6, 0)];
        assert_eq!(matches(&ms, size, 2), [2, 3]);
        // Repeated modifiers add nothing new.
        let ms = [mirror(Axes::Both, 4, 4); 13];
        assert_eq!(matches(&ms, UVec2::new(4, 4), 0), [0, 3, 12, 15]);
    }
}
