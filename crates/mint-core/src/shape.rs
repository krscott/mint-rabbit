//! Shapes: Bézier paths in texture space, filled or stroked in one palette
//! color.
//!
//! Texture space is a texture's UV space scaled by its pixel size: one unit is
//! one texel, the origin is the top-left corner, and pixel (x, y) has its
//! center at (x + 0.5, y + 0.5).

use glam::{Affine2, Vec2};

use crate::PaletteIndex;

/// Flattening tolerance in texels: curves are approximated by lines that
/// stray at most this far, well under the half-texel that decides coverage.
const TOLERANCE: f32 = 0.05;

/// Handle length, as a fraction of the radius, of a cubic Bézier quarter
/// circle.
const KAPPA: f32 = 0.552_284_8;

/// A point on a path with the handles that shape the curve on either side of
/// it, as offsets from the point. Zero handles make a sharp corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    pub point: Vec2,
    /// Handle of the segment arriving at the point.
    pub handle_in: Vec2,
    /// Handle of the segment leaving the point.
    pub handle_out: Vec2,
}

impl Anchor {
    #[must_use]
    pub const fn corner(point: Vec2) -> Self {
        Self {
            point,
            handle_in: Vec2::ZERO,
            handle_out: Vec2::ZERO,
        }
    }

    /// A smooth anchor whose handles mirror each other.
    #[must_use]
    pub fn smooth(point: Vec2, handle_out: Vec2) -> Self {
        Self {
            point,
            handle_in: -handle_out,
            handle_out,
        }
    }
}

/// How a shape covers pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Style {
    /// The path closes back to its first anchor and covers its inside, by the
    /// nonzero winding rule.
    Fill,
    /// Covers pixels within half of `width` texels of the path, with round
    /// ends and joins.
    Stroke { width: f32, closed: bool },
}

impl Style {
    fn is_valid(self) -> bool {
        match self {
            Self::Fill => true,
            Self::Stroke { width, .. } => width.is_finite() && width > 0.0,
        }
    }

    #[must_use]
    pub fn closed(self) -> bool {
        match self {
            Self::Fill => true,
            Self::Stroke { closed, .. } => closed,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ShapeError {
    #[error("a shape needs at least one anchor")]
    NoAnchors,
    #[error("stroke width must be a positive number of texels")]
    BadWidth,
}

/// A single vector element of a layer: a path of cubic Bézier segments
/// between anchors, with a style and a palette color.
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    /// Never empty.
    anchors: Vec<Anchor>,
    style: Style,
    color: PaletteIndex,
}

impl Shape {
    pub fn new(
        anchors: Vec<Anchor>,
        style: Style,
        color: PaletteIndex,
    ) -> Result<Self, ShapeError> {
        if anchors.is_empty() {
            return Err(ShapeError::NoAnchors);
        }
        if !style.is_valid() {
            return Err(ShapeError::BadWidth);
        }
        Ok(Self {
            anchors,
            style,
            color,
        })
    }

    #[must_use]
    pub fn anchors(&self) -> &[Anchor] {
        &self.anchors
    }

    pub fn anchors_mut(&mut self) -> &mut [Anchor] {
        &mut self.anchors
    }

    /// Inserts an anchor before position `index`.
    pub fn insert_anchor(&mut self, index: usize, anchor: Anchor) {
        self.anchors.insert(index, anchor);
    }

    /// Removes an anchor, unless it is the last one left.
    pub fn remove_anchor(&mut self, index: usize) -> Result<Anchor, ShapeError> {
        if self.anchors.len() == 1 {
            return Err(ShapeError::NoAnchors);
        }
        Ok(self.anchors.remove(index))
    }

    #[must_use]
    pub fn style(&self) -> Style {
        self.style
    }

    pub fn set_style(&mut self, style: Style) -> Result<(), ShapeError> {
        if !style.is_valid() {
            return Err(ShapeError::BadWidth);
        }
        self.style = style;
        Ok(())
    }

    #[must_use]
    pub fn color(&self) -> PaletteIndex {
        self.color
    }

    pub fn set_color(&mut self, color: PaletteIndex) {
        self.color = color;
    }

    /// Applies `transform` to every anchor and handle.
    pub fn transform(&mut self, transform: Affine2) {
        for a in &mut self.anchors {
            a.point = transform.transform_point2(a.point);
            a.handle_in = transform.transform_vector2(a.handle_in);
            a.handle_out = transform.transform_vector2(a.handle_out);
        }
    }

    /// The cubic segments as `[start, control, control, end]`, including the
    /// closing segment of a closed path.
    pub fn segments(&self) -> impl Iterator<Item = [Vec2; 4]> + '_ {
        let n = self.anchors.len();
        let count = if self.style.closed() { n } else { n - 1 };
        (0..count).map(move |i| {
            let a = self.anchors[i];
            let b = self.anchors[(i + 1) % n];
            [
                a.point,
                a.point + a.handle_out,
                b.point + b.handle_in,
                b.point,
            ]
        })
    }

    /// The path as a polyline within [`TOLERANCE`] of the curves, with the
    /// style that decides which pixels it covers.
    #[must_use]
    pub fn outline(&self) -> Outline {
        let mut points = vec![self.anchors[0].point];
        for seg in self.segments() {
            flatten_cubic(seg, &mut points);
        }
        if self.style.closed() && points.len() > 1 {
            // The closing segment ends on the first point again.
            points.pop();
        }
        Outline {
            points,
            style: self.style,
        }
    }
}

/// Appends points along a cubic, excluding its start, enough of them that
/// the lines between stay within [`TOLERANCE`] (Wang's formula). A straight
/// segment is appended exactly, as its end alone.
fn flatten_cubic([p0, p1, p2, p3]: [Vec2; 4], out: &mut Vec<Vec2>) {
    if is_straight([p0, p1, p2, p3]) {
        out.push(p3);
        return;
    }
    let d = (p0 - 2.0 * p1 + p2)
        .length()
        .max((p1 - 2.0 * p2 + p3).length());
    let n = ((0.75 * d / TOLERANCE).sqrt().ceil() as usize).clamp(1, 1024);
    for i in 1..=n {
        let t = i as f32 / n as f32;
        let u = 1.0 - t;
        out.push(u * u * u * p0 + 3.0 * u * u * t * p1 + 3.0 * u * t * t * p2 + t * t * t * p3);
    }
}

/// Whether a cubic is a straight line from its start to its end: both
/// controls lie within [`TOLERANCE`] of that segment and between its ends,
/// so the curve strays no further from it than flattening allows. Controls
/// exactly on the ends, as at corners, always count, at any angle.
fn is_straight([p0, p1, p2, p3]: [Vec2; 4]) -> bool {
    let line = p3 - p0;
    let len2 = line.length_squared();
    let on_segment = |c: Vec2| {
        let d = c - p0;
        if !(len2 > 0.0 && len2.is_finite()) {
            return d == Vec2::ZERO;
        }
        // Unnormalized, so a control on an end compares exactly.
        line.perp_dot(d).abs() <= TOLERANCE * len2.sqrt() && (0.0..=len2).contains(&line.dot(d))
    };
    on_segment(p1) && on_segment(p2)
}

/// A shape flattened to lines, ready for coverage tests.
#[derive(Clone, Debug, PartialEq)]
pub struct Outline {
    /// Never empty. A closed outline returns from the last point to the
    /// first.
    points: Vec<Vec2>,
    style: Style,
}

impl Outline {
    #[must_use]
    pub fn points(&self) -> &[Vec2] {
        &self.points
    }

    #[must_use]
    pub fn closed(&self) -> bool {
        self.style.closed()
    }

    fn edges(&self) -> impl Iterator<Item = (Vec2, Vec2)> + '_ {
        let n = self.points.len();
        let count = if self.closed() { n } else { n - 1 };
        (0..count).map(move |i| (self.points[i], self.points[(i + 1) % n]))
    }

    /// The smallest box holding every covered point, as (min, max).
    #[must_use]
    pub fn bounds(&self) -> (Vec2, Vec2) {
        let (min, max) = self
            .points
            .iter()
            .fold((Vec2::INFINITY, Vec2::NEG_INFINITY), |(lo, hi), &p| {
                (lo.min(p), hi.max(p))
            });
        let pad = match self.style {
            Style::Fill => 0.0,
            Style::Stroke { width, .. } => width / 2.0,
        };
        (min - pad, max + pad)
    }

    /// Whether the shape covers point `p`.
    #[must_use]
    pub fn covers(&self, p: Vec2) -> bool {
        match self.style {
            Style::Fill => self.winding(p) != 0,
            Style::Stroke { width, .. } => self.distance(p) <= width / 2.0,
        }
    }

    /// Distance from `p` to the nearest point of the path itself.
    #[must_use]
    pub fn distance(&self, p: Vec2) -> f32 {
        if self.points.len() == 1 {
            return p.distance(self.points[0]);
        }
        self.edges()
            .map(|(a, b)| {
                let ab = b - a;
                let t =
                    ((p - a).dot(ab) / ab.length_squared().max(f32::MIN_POSITIVE)).clamp(0.0, 1.0);
                p.distance(a + ab * t)
            })
            .fold(f32::INFINITY, f32::min)
    }

    /// Winding number of the closed outline around `p`. Edges include their
    /// lower end and exclude their upper one, and the side of an edge is
    /// always computed from its lower end, so two outlines sharing an edge in
    /// opposite directions agree on every point and one of them covers it.
    fn winding(&self, p: Vec2) -> i32 {
        let mut w = 0;
        let n = self.points.len();
        for i in 0..n {
            let (a, b) = (self.points[i], self.points[(i + 1) % n]);
            let (lo, hi, dir) = if a.y <= b.y { (a, b, 1) } else { (b, a, -1) };
            if lo.y <= p.y && p.y < hi.y && (hi - lo).perp_dot(p - lo) > 0.0 {
                w += dir;
            }
        }
        w
    }
}

/// Anchors of an axis-aligned ellipse, clockwise on screen from the right.
#[must_use]
pub fn ellipse(center: Vec2, radii: Vec2) -> Vec<Anchor> {
    let (x, y) = (Vec2::new(radii.x, 0.0), Vec2::new(0.0, radii.y));
    vec![
        Anchor::smooth(center + x, y * KAPPA),
        Anchor::smooth(center + y, -x * KAPPA),
        Anchor::smooth(center - x, -y * KAPPA),
        Anchor::smooth(center - y, x * KAPPA),
    ]
}

/// Anchors of an axis-aligned rectangle with corners `a` and `b`.
#[must_use]
pub fn rectangle(a: Vec2, b: Vec2) -> Vec<Anchor> {
    let (min, max) = (a.min(b), a.max(b));
    [min, Vec2::new(max.x, min.y), max, Vec2::new(min.x, max.y)]
        .map(Anchor::corner)
        .to_vec()
}

/// Smooth anchors through a freehand stroke: the points are simplified to
/// those within `tolerance` texels of the stroke, then joined by
/// Catmull-Rom curves. Ends and turns sharper than a right angle stay
/// corners. Each handle keeps its Catmull-Rom direction but is at most a
/// third of the segment it shapes and strays at most 4/3 `tolerance` to the
/// side of it, so each curve stays within `tolerance` of the simplified line
/// and within twice `tolerance` of the stroke. Returns no anchors for no
/// points.
#[must_use]
pub fn freehand(points: &[Vec2], tolerance: f32) -> Vec<Anchor> {
    let mut kept = Vec::new();
    if let Some(&first) = points.first() {
        kept.push(first);
        simplify(points, tolerance, &mut kept);
    }
    let n = kept.len();
    (0..n)
        .map(|i| {
            if i == 0 || i == n - 1 || (kept[i] - kept[i - 1]).dot(kept[i + 1] - kept[i]) < 0.0 {
                return Anchor::corner(kept[i]);
            }
            let h = (kept[i + 1] - kept[i - 1]) / 6.0;
            // A cubic strays at most 3/4 of its controls' offset from its
            // chord.
            let bound = |h: Vec2, segment: Vec2| {
                let side = segment.normalize_or_zero().perp_dot(h).abs();
                let max_side = tolerance.max(0.0) * 4.0 / 3.0;
                let h = if side > max_side {
                    h * (max_side / side)
                } else {
                    h
                };
                h.clamp_length_max(segment.length() / 3.0)
            };
            Anchor {
                point: kept[i],
                handle_in: bound(-h, kept[i - 1] - kept[i]),
                handle_out: bound(h, kept[i + 1] - kept[i]),
            }
        })
        .collect()
}

/// Ramer-Douglas-Peucker: appends the points of `points[1..]` that are
/// needed to stay within `tolerance`, ending with the last point. Pending
/// ranges sit on an explicit stack, as a long stroke can split many times.
fn simplify(points: &[Vec2], tolerance: f32, out: &mut Vec<Vec2>) {
    if points.len() < 2 {
        return;
    }
    // Ranges of `points` still to simplify, by first and last index; the
    // leftmost is on top.
    let mut pending = vec![(0, points.len() - 1)];
    while let Some((start, end)) = pending.pop() {
        let (first, last) = (points[start], points[end]);
        let line = last - first;
        let far = (start + 1..end)
            .map(|i| {
                // Distance to the segment, not the line through it, so a
                // stroke that doubles back keeps its turning point.
                let p = points[i];
                let t = ((p - first).dot(line) / line.length_squared().max(f32::MIN_POSITIVE))
                    .clamp(0.0, 1.0);
                (i, p.distance(first + line * t))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1));
        match far {
            Some((i, d)) if d > tolerance => {
                pending.push((i, end));
                pending.push((start, i));
            }
            _ => out.push(last),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fill(anchors: Vec<Anchor>) -> Shape {
        Shape::new(anchors, Style::Fill, PaletteIndex(1)).unwrap()
    }

    #[test]
    fn shapes_need_anchors_and_a_positive_width() {
        assert_eq!(
            Shape::new(vec![], Style::Fill, PaletteIndex(0)),
            Err(ShapeError::NoAnchors)
        );
        let bad = Style::Stroke {
            width: 0.0,
            closed: false,
        };
        let dot = vec![Anchor::corner(Vec2::ZERO)];
        assert_eq!(
            Shape::new(dot.clone(), bad, PaletteIndex(0)),
            Err(ShapeError::BadWidth)
        );
        let mut s = fill(dot);
        assert_eq!(s.remove_anchor(0), Err(ShapeError::NoAnchors));
        assert_eq!(s.set_style(bad), Err(ShapeError::BadWidth));
    }

    #[test]
    fn rectangle_fill_covers_its_inside() {
        let o = fill(rectangle(Vec2::new(1.0, 1.0), Vec2::new(3.0, 2.0))).outline();
        assert!(o.covers(Vec2::new(1.5, 1.5)));
        assert!(o.covers(Vec2::new(2.5, 1.5)));
        assert!(!o.covers(Vec2::new(0.5, 1.5)));
        assert!(!o.covers(Vec2::new(1.5, 2.5)));
    }

    #[test]
    fn ellipse_stays_close_to_a_circle() {
        let o = fill(ellipse(Vec2::splat(8.0), Vec2::splat(4.0))).outline();
        for &p in o.points() {
            assert!((p.distance(Vec2::splat(8.0)) - 4.0).abs() < 0.01, "{p}");
        }
        assert!(o.covers(Vec2::new(11.5, 8.0)));
        assert!(!o.covers(Vec2::new(11.0, 11.0)));
    }

    #[test]
    fn nonzero_rule_covers_doubly_wound_regions() {
        // A pentagram winds twice around its center.
        let star: Vec<Anchor> = (0..5)
            .map(|i| {
                let a = i as f32 * 4.0 * std::f32::consts::PI / 5.0;
                Anchor::corner(Vec2::splat(8.0) + 6.0 * Vec2::new(a.sin(), -a.cos()))
            })
            .collect();
        let o = fill(star).outline();
        assert!(o.covers(Vec2::splat(8.0)));
        assert!(!o.covers(Vec2::new(1.0, 1.0)));
    }

    #[test]
    fn strokes_cover_within_half_their_width() {
        let line = vec![
            Anchor::corner(Vec2::new(0.0, 0.0)),
            Anchor::corner(Vec2::new(10.0, 0.0)),
        ];
        let style = Style::Stroke {
            width: 2.0,
            closed: false,
        };
        let o = Shape::new(line, style, PaletteIndex(0)).unwrap().outline();
        assert!(o.covers(Vec2::new(5.0, 1.0)));
        assert!(!o.covers(Vec2::new(5.0, 1.1)));
        // Round end cap.
        assert!(o.covers(Vec2::new(-0.7, 0.7)));
        assert!(!o.covers(Vec2::new(-1.0, 1.0)));
        assert_eq!(o.bounds(), (Vec2::new(-1.0, -1.0), Vec2::new(11.0, 1.0)));
    }

    #[test]
    fn transform_moves_points_and_turns_handles() {
        let mut s = fill(ellipse(Vec2::ZERO, Vec2::new(2.0, 1.0)));
        s.transform(Affine2::from_angle_translation(
            std::f32::consts::FRAC_PI_2,
            Vec2::new(5.0, 0.0),
        ));
        let a = s.anchors()[0];
        assert!(a.point.abs_diff_eq(Vec2::new(5.0, 2.0), 1e-5));
        assert!(a.handle_out.abs_diff_eq(Vec2::new(-KAPPA, 0.0), 1e-5));
    }

    #[test]
    fn straight_edges_on_pixel_centers_cover_consistently() {
        // The top edge runs through the centers of row 0, which belong to
        // the fill along the whole edge.
        let o = fill(rectangle(Vec2::splat(0.5), Vec2::new(8.5, 4.5))).outline();
        assert_eq!(o.points().len(), 4);
        for x in 1..8 {
            assert!(o.covers(Vec2::new(x as f32 + 0.5, 0.5)), "{x}");
        }
        // Handles along a straight edge, or within the tolerance of it,
        // leave it straight.
        let mut anchors = rectangle(Vec2::splat(0.5), Vec2::new(8.5, 4.5));
        anchors[0].handle_out = Vec2::new(0.25, 0.01);
        anchors[1].handle_in = Vec2::new(-0.25, 0.0);
        let o = fill(anchors).outline();
        assert_eq!(o.points().len(), 4);

        // Two triangles sharing a diagonal with fractional ends cover each
        // pixel center along it exactly once.
        let (a, b) = (Vec2::new(0.0, 0.1), Vec2::new(2.5, 2.1));
        let halves = [
            fill([a, Vec2::new(2.5, 0.1), b].map(Anchor::corner).to_vec()),
            fill([a, b, Vec2::new(0.0, 2.1)].map(Anchor::corner).to_vec()),
        ]
        .map(|s| s.outline());
        for p in [Vec2::splat(0.5), Vec2::new(1.5, 1.5), Vec2::new(2.0, 1.7)] {
            assert_eq!(halves.iter().filter(|o| o.covers(p)).count(), 1, "{p}");
        }

        // Corner-only edges stay exact at any angle, so two triangles that
        // share a diagonal through pixel centers cover each center once.
        let mut rotated = fill(rectangle(Vec2::ZERO, Vec2::new(8.0, 5.0)));
        rotated.transform(Affine2::from_angle(0.3));
        assert_eq!(rotated.outline().points().len(), 4);
        let (a, b) = (Vec2::new(1.5, 0.5), Vec2::new(10.5, 27.5));
        let halves = [
            fill(
                vec![a, b, Vec2::new(10.5, 0.5)]
                    .into_iter()
                    .map(Anchor::corner)
                    .collect(),
            ),
            fill(
                vec![a, Vec2::new(1.5, 27.5), b]
                    .into_iter()
                    .map(Anchor::corner)
                    .collect(),
            ),
        ]
        .map(|s| s.outline());
        for y in 0..28 {
            for x in 0..11 {
                let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                let inside = (1.5..10.5).contains(&p.x) && (0.5..27.5).contains(&p.y);
                let count = halves.iter().filter(|o| o.covers(p)).count();
                assert_eq!(count, usize::from(inside), "{p}");
            }
        }
    }

    #[test]
    fn closed_strokes_cover_their_outline_only() {
        let style = Style::Stroke {
            width: 1.0,
            closed: true,
        };
        let ring = Shape::new(
            rectangle(Vec2::ZERO, Vec2::splat(8.0)),
            style,
            PaletteIndex(0),
        );
        let o = ring.unwrap().outline();
        // The closing edge, from the last corner back to the first.
        assert!(o.covers(Vec2::new(0.0, 4.0)));
        assert!(o.covers(Vec2::new(4.0, 0.4)));
        assert!(!o.covers(Vec2::splat(4.0)));

        // One anchor whose handles loop back to it encloses an area.
        let loop_ = Anchor {
            point: Vec2::splat(4.0),
            handle_in: Vec2::new(0.0, 8.0),
            handle_out: Vec2::new(8.0, 0.0),
        };
        assert!(fill(vec![loop_]).outline().covers(Vec2::splat(6.5)));
    }

    #[test]
    fn freehand_simplifies_and_smooths() {
        let points: Vec<Vec2> = (0..=20).map(|i| Vec2::new(i as f32, 0.0)).collect();
        let anchors = freehand(&points, 0.5);
        assert_eq!(
            anchors,
            vec![
                Anchor::corner(Vec2::ZERO),
                Anchor::corner(Vec2::new(20.0, 0.0))
            ]
        );
        let bend = [Vec2::ZERO, Vec2::new(5.0, 5.0), Vec2::new(10.0, 0.0)];
        let anchors = freehand(&bend, 0.5);
        assert_eq!(anchors.len(), 3);
        // Along the Catmull-Rom direction, shortened to stay within the
        // tolerance of each segment.
        let out = anchors[1].handle_out;
        assert!(out.y == 0.0 && out.x > 0.0 && out.x < 10.0 / 6.0, "{out}");
        assert_eq!(anchors[1].handle_in, -out);
        // A right-angle stroke stays within twice the tolerance of itself.
        let mut ell: Vec<Vec2> = (0..=30).map(|x| Vec2::new(x as f32, 0.0)).collect();
        ell.extend((1..=30).map(|y| Vec2::new(30.0, y as f32)));
        let stroke = Style::Stroke {
            width: 0.1,
            closed: false,
        };
        let o = Shape::new(freehand(&ell, 0.5), stroke, PaletteIndex(0))
            .unwrap()
            .outline();
        let drawn = Shape::new(
            [Vec2::ZERO, Vec2::new(30.0, 0.0), Vec2::splat(30.0)]
                .map(Anchor::corner)
                .to_vec(),
            stroke,
            PaletteIndex(0),
        )
        .unwrap()
        .outline();
        for &p in o.points() {
            assert!(drawn.distance(p) <= 1.0 + 1e-4, "{p}");
        }
        // A stroke that doubles back keeps its far end, as a corner.
        let back = [Vec2::ONE, Vec2::new(10.0, 1.0), Vec2::new(5.0, 1.0)];
        assert_eq!(freehand(&back, 0.25), back.map(Anchor::corner).to_vec());
        // A long line ending in a short flick stops where the flick does.
        let mut hook: Vec<Vec2> = (0..=100).map(|x| Vec2::new(x as f32, 0.0)).collect();
        hook.extend([Vec2::new(101.0, 1.0), Vec2::new(101.5, 2.0)]);
        let style = Style::Stroke {
            width: 0.1,
            closed: false,
        };
        let o = Shape::new(freehand(&hook, 0.5), style, PaletteIndex(0))
            .unwrap()
            .outline();
        assert!(
            o.points().iter().all(|p| p.x <= 101.5 + 1e-4),
            "{:?}",
            o.points()
        );
        // A long zigzag simplifies without deep recursion.
        let zigzag: Vec<Vec2> = (0..12_000)
            .map(|i| [Vec2::ZERO, Vec2::new(8.0, 0.0), Vec2::new(0.0, 8.0)][i % 3])
            .collect();
        assert!(freehand(&zigzag, 0.5).len() > 3);
        assert!(freehand(&[], 0.5).is_empty());
        assert_eq!(freehand(&[Vec2::ONE], 0.5).len(), 1);
    }
}
