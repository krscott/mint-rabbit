//! Stamps: groups of shapes saved with their own colors, which placing
//! copies into a layer.

use glam::{Affine2, UVec2, Vec2};

use crate::{Palette, PaletteIndex, Raster, Rgba8, Shape, Style};

/// Farthest a stamp's points, curve controls, and stroke widths may reach
/// from its origin, in texels: far past any texture's size, and near enough
/// that drawing and placing never overflow.
const MAX_REACH: f32 = 65536.0;

/// A group of shapes with its own colors: each shape's color is an index
/// into the stamp's colors, not into a project's palette. Placing moves the
/// shapes by whole texels, so they cover the same pixels they did.
#[derive(Clone, Debug, PartialEq)]
pub struct Stamp {
    /// Each used by a shape.
    colors: Vec<Rgba8>,
    /// Never empty; each color indexes `colors`.
    shapes: Vec<Shape>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum StampError {
    #[error("a stamp needs at least one shape")]
    NoShapes,
    #[error("a stamp holds at most {} colors", Palette::MAX_LEN)]
    TooManyColors,
    #[error("a stamp shape uses color {index}, but the stamp has {len}")]
    MissingColor { index: u8, len: usize },
    #[error("stamp color {0} isn't used by any of its shapes")]
    UnusedColor(usize),
    #[error("a stamp's shapes reach at most {MAX_REACH} texels from its origin")]
    TooFar,
}

impl Stamp {
    /// A stamp of `shapes` as they are, coloring them from `colors`.
    pub fn new(colors: Vec<Rgba8>, shapes: Vec<Shape>) -> Result<Self, StampError> {
        if shapes.is_empty() {
            return Err(StampError::NoShapes);
        }
        if colors.len() > Palette::MAX_LEN {
            return Err(StampError::TooManyColors);
        }
        let mut used = vec![false; colors.len()];
        for s in &shapes {
            let i = usize::from(s.color().0);
            match used.get_mut(i) {
                Some(u) => *u = true,
                None => {
                    return Err(StampError::MissingColor {
                        index: s.color().0,
                        len: colors.len(),
                    });
                }
            }
        }
        if let Some(i) = used.iter().position(|u| !u) {
            return Err(StampError::UnusedColor(i));
        }
        if !shapes.iter().all(within_reach) {
            return Err(StampError::TooFar);
        }
        Ok(Self { colors, shapes })
    }

    /// A stamp of `shapes`, colored from `palette`, holding only the colors
    /// they use, in palette order. They move by whole texels so that the
    /// pixel corner nearest the middle of their bounds is the origin, which
    /// keeps the numbers in its file small. Panics if a shape's color isn't
    /// in `palette`.
    pub fn from_shapes(shapes: Vec<Shape>, palette: &Palette) -> Result<Self, StampError> {
        let mut used = vec![false; palette.len()];
        for s in &shapes {
            used[usize::from(s.color().0)] = true;
        }
        let mut colors = Vec::new();
        let mut index = vec![PaletteIndex(0); palette.len()];
        for (i, &c) in palette.colors().iter().enumerate() {
            if used[i] {
                index[i] = PaletteIndex(colors.len() as u8);
                colors.push(c);
            }
        }
        let Some((min, max)) = bounds(&shapes) else {
            return Err(StampError::NoShapes);
        };
        let origin = ((min + max) / 2.0).round();
        let shapes = shapes
            .into_iter()
            .map(|mut s| {
                s.transform(Affine2::from_translation(-origin));
                s.set_color(index[usize::from(s.color().0)]);
                s
            })
            .collect();
        Self::new(colors, shapes)
    }

    #[must_use]
    pub fn colors(&self) -> &[Rgba8] {
        &self.colors
    }

    #[must_use]
    pub fn shapes(&self) -> &[Shape] {
        &self.shapes
    }

    /// The area the shapes cover about the origin, as min and max corners.
    #[must_use]
    pub fn bounds(&self) -> (Vec2, Vec2) {
        bounds(&self.shapes).expect("a stamp has shapes")
    }

    /// The whole-texel offset the stamp moves by to be placed at `at`: the
    /// one that brings the middle of its bounds nearest `at`.
    #[must_use]
    pub fn offset(&self, at: Vec2) -> Vec2 {
        let (min, max) = self.bounds();
        (at - (min + max) / 2.0).round()
    }

    /// The shapes placed at `at`, moved by [`Self::offset`], each stamp
    /// color `i` becoming palette entry `entries[i]`. Panics unless there is
    /// an entry for each stamp color.
    #[must_use]
    pub fn placed(&self, at: Vec2, entries: &[PaletteIndex]) -> Vec<Shape> {
        assert_eq!(entries.len(), self.colors.len(), "an entry per stamp color");
        let move_by = Affine2::from_translation(self.offset(at));
        self.shapes
            .iter()
            .map(|s| {
                let mut s = s.clone();
                s.transform(move_by);
                s.set_color(entries[usize::from(s.color().0)]);
                s
            })
            .collect()
    }

    /// The stamp's pixels in its own colors, from the pixel corner at or
    /// above and left of its bounds, and its shapes and stroke widths scaled
    /// down if needed so that neither side is over `max_side` pixels. Scaled
    /// down, strokes are wide enough to cover a pixel wherever they lie.
    #[must_use]
    pub fn raster(&self, max_side: u32) -> Raster {
        let (min, max) = self.bounds();
        let (min, max) = (min.floor(), max.ceil());
        let longest = (max - min).max_element().max(1.0);
        let scale = (max_side.max(1) as f32 / longest).min(1.0);
        let size = ((max - min) * scale).ceil().max(Vec2::ONE).as_uvec2();
        let fit = Affine2::from_scale(Vec2::splat(scale)) * Affine2::from_translation(-min);
        let mut raster = Raster::clear(size.min(UVec2::splat(max_side.max(1))));
        for s in &self.shapes {
            let mut s = s.clone();
            s.transform(fit);
            // A dot this wide holds a pixel center wherever it lies.
            if let Style::Stroke { width, closed } = s.style()
                && scale < 1.0
            {
                let width = (width * scale).max(std::f32::consts::SQRT_2);
                s.set_style(Style::Stroke { width, closed })
                    .expect("a positive width");
            }
            raster.paint_shape(&s);
        }
        raster
    }
}

/// Whether `shape`'s points, curve controls, and stroke width are within
/// [`MAX_REACH`] of the origin; never for a value that isn't a number.
fn within_reach(shape: &Shape) -> bool {
    let near = |v: f32| v.abs() <= MAX_REACH;
    let width = match shape.style() {
        Style::Fill => 0.0,
        Style::Stroke { width, .. } => width,
    };
    near(width)
        && shape.anchors().iter().all(|a| {
            [a.point, a.point + a.handle_in, a.point + a.handle_out]
                .iter()
                .all(|p| near(p.x) && near(p.y))
        })
}

fn bounds(shapes: &[Shape]) -> Option<(Vec2, Vec2)> {
    shapes
        .iter()
        .map(|s| s.outline().bounds())
        .reduce(|(a0, a1), (b0, b1)| (a0.min(b0), a1.max(b1)))
}

#[cfg(test)]
mod tests {
    use crate::Anchor;

    use super::*;

    fn square(min: Vec2, side: f32, color: u8) -> Shape {
        let anchors = [
            min,
            min + Vec2::new(side, 0.0),
            min + Vec2::splat(side),
            min + Vec2::new(0.0, side),
        ]
        .map(Anchor::corner)
        .to_vec();
        Shape::new(anchors, Style::Fill, PaletteIndex(color)).unwrap()
    }

    fn palette() -> Palette {
        Palette::from_colors([
            Rgba8::new(0, 0, 0, 255),
            Rgba8::new(255, 0, 0, 255),
            Rgba8::new(0, 255, 0, 255),
            Rgba8::new(0, 0, 255, 255),
        ])
        .unwrap()
    }

    #[test]
    fn a_stamp_keeps_only_the_colors_it_uses() {
        let shapes = vec![square(Vec2::ZERO, 2.0, 3), square(Vec2::ONE, 2.0, 1)];
        let stamp = Stamp::from_shapes(shapes, &palette()).unwrap();
        assert_eq!(
            stamp.colors(),
            [Rgba8::new(255, 0, 0, 255), Rgba8::new(0, 0, 255, 255)]
        );
        let colors: Vec<_> = stamp.shapes().iter().map(Shape::color).collect();
        assert_eq!(colors, [PaletteIndex(1), PaletteIndex(0)]);
    }

    #[test]
    fn placing_where_it_was_saved_gives_the_same_shapes() {
        // Bounds 3..7.5 by 1..4.5, the stroke's half width included, so the
        // origin is the corner (5, 3) nearest their middle (5.25, 2.75).
        let shapes = vec![square(Vec2::new(3.0, 1.0), 3.0, 2), {
            let mut s = square(Vec2::new(5.0, 2.0), 2.0, 2);
            s.set_style(Style::Stroke {
                width: 1.0,
                closed: false,
            })
            .unwrap();
            s
        }];
        let stamp = Stamp::from_shapes(shapes.clone(), &palette()).unwrap();
        // Placed with their middle near where it was.
        let placed = stamp.placed(Vec2::new(5.3, 2.8), &[PaletteIndex(2)]);
        assert_eq!(placed, shapes);
    }

    #[test]
    fn placing_maps_each_color_and_moves_by_whole_texels() {
        let stamp = Stamp::from_shapes(vec![square(Vec2::ZERO, 2.0, 1)], &palette()).unwrap();
        let placed = stamp.placed(Vec2::new(10.2, 4.7), &[PaletteIndex(7)]);
        assert_eq!(placed, [square(Vec2::new(9.0, 4.0), 2.0, 7)]);
    }

    #[test]
    fn stamps_need_shapes_and_their_colors() {
        assert_eq!(
            Stamp::from_shapes(Vec::new(), &palette()),
            Err(StampError::NoShapes)
        );
        assert_eq!(
            Stamp::new(Vec::new(), vec![square(Vec2::ZERO, 1.0, 0)]),
            Err(StampError::MissingColor { index: 0, len: 0 })
        );
        let red = Rgba8::new(255, 0, 0, 255);
        assert_eq!(
            Stamp::new(vec![red, red], vec![square(Vec2::ZERO, 1.0, 1)]),
            Err(StampError::UnusedColor(0))
        );
        assert_eq!(
            Stamp::new(vec![red], vec![square(Vec2::splat(2e38), 1.0, 0)]),
            Err(StampError::TooFar)
        );
        // Handles out of reach, though the point is near.
        let far = Anchor::smooth(Vec2::ZERO, Vec2::new(1e5, 0.0));
        let shape = Shape::new(vec![far], Style::Fill, PaletteIndex(0)).unwrap();
        assert_eq!(
            Stamp::new(vec![red], vec![shape.clone()]),
            Err(StampError::TooFar)
        );
    }

    #[test]
    fn the_raster_covers_the_bounds_in_stamp_colors() {
        let shapes = vec![square(Vec2::ZERO, 2.0, 3), square(Vec2::ONE, 2.0, 1)];
        let stamp = Stamp::from_shapes(shapes, &palette()).unwrap();
        let raster = stamp.raster(64);
        assert_eq!(raster.size(), UVec2::new(3, 3));
        let (b, r) = (Some(PaletteIndex(1)), Some(PaletteIndex(0)));
        assert_eq!(raster.pixels(), [b, b, None, b, r, r, None, r, r]);
        // Scaled down to fit.
        assert_eq!(stamp.raster(2).size(), UVec2::new(2, 2));
    }

    #[test]
    fn thumbnails_scale_strokes_too() {
        // A 64-texel square outline 8 texels wide, drawn at a quarter size.
        let mut ring = square(Vec2::ZERO, 64.0, 0);
        ring.set_style(Style::Stroke {
            width: 8.0,
            closed: true,
        })
        .unwrap();
        let stamp = Stamp::from_shapes(vec![ring], &palette()).unwrap();
        let raster = stamp.raster(18);
        assert_eq!(raster.size(), UVec2::new(18, 18));
        let set = raster.pixels().iter().filter(|p| p.is_some()).count();
        // A ring 2 pixels wide around a 16-pixel square.
        assert_eq!(set, 18 * 18 - 14 * 14);

        // A dot thinner than a texel still shows, though at a quarter size
        // it falls between pixel centers: dots at the far corners set the
        // scale, 256 texels to 64 pixels, and one lands at (32.1, 1.1),
        // over half a pixel from any center.
        let dot = |x: f32, y: f32| {
            let stroke = Style::Stroke {
                width: 0.5,
                closed: false,
            };
            Shape::new(
                vec![Anchor::corner(Vec2::new(x, y))],
                stroke,
                PaletteIndex(0),
            )
            .unwrap()
        };
        let stamp = Stamp::new(
            vec![Rgba8::new(0, 0, 0, 255)],
            vec![dot(0.25, 0.25), dot(255.75, 0.25), dot(128.4, 4.4)],
        )
        .unwrap();
        let raster = stamp.raster(64);
        assert_eq!(raster.size().x, 64);
        assert!(raster.pixels()[64 + 32].is_some(), "the dot vanished");

        // Outlines thinner than a texel, 200 texels across, still show.
        for width in [1.0, 0.5] {
            let mut thin = square(Vec2::splat(0.25), 200.0, 0);
            thin.set_style(Style::Stroke {
                width,
                closed: true,
            })
            .unwrap();
            let stamp = Stamp::from_shapes(vec![thin], &palette()).unwrap();
            let set = stamp
                .raster(64)
                .pixels()
                .iter()
                .filter(|p| p.is_some())
                .count();
            assert!(set > 4 * 50, "width {width}: {set} pixels set");
        }
    }
}
