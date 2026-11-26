//! The posed model's surface as rays meet it: which texture point lies under
//! the cursor in the Model View, and which UV island it is on.

use std::collections::HashMap;
use std::sync::Arc;

use glam::{IVec2, Vec2, Vec3};
use mint_core::{AlphaMode, TextureId, Wrap};

use crate::render::SceneData;

/// Rays closer to parallel with a triangle than this miss it.
const PARALLEL: f32 = 1e-9;
/// How close to the edge between two repeats of a texture, as a fraction of
/// it, a UV coordinate counts as on it.
const EDGE: f32 = 1e-4;

/// A point on the surface where a ray first meets it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub texture: TextureId,
    /// Where on the texture, in texels, after the material's wrap.
    pub point: Vec2,
    /// Which repeat of the texture the UV coordinate is in, before wrapping.
    cell: IVec2,
    /// The UV island the point is on.
    island: usize,
}

impl Hit {
    /// Whether a stroke can run straight in texture space from `self` to
    /// `other`: both are on one UV island, in one repeat of the texture.
    pub fn joins(&self, other: &Self) -> bool {
        (self.texture, self.island, self.cell) == (other.texture, other.island, other.cell)
    }
}

/// The posed triangles of a scene, ready for rays.
pub struct Surface {
    scene: Arc<SceneData>,
    /// Whether the view samples textures smoothly, which moves cutout edges.
    smooth: bool,
    /// Per triangle, its material and UV island.
    triangles: Arc<[(usize, usize)]>,
}

impl Surface {
    pub fn new(scene: Arc<SceneData>, smooth: bool) -> Self {
        let materials: Vec<usize> = scene
            .mesh
            .batches
            .iter()
            .flat_map(|b| std::iter::repeat_n(b.material.index(), b.count as usize / 3))
            .collect();
        let islands = islands(&scene, &materials);
        Self {
            triangles: materials.into_iter().zip(islands).collect(),
            scene,
            smooth,
        }
    }

    /// The surface of `scene`, which poses the same triangles as this one's
    /// with other texture pixels, keeping its islands.
    pub fn with_scene(&self, scene: Arc<SceneData>, smooth: bool) -> Self {
        debug_assert_eq!(scene.mesh.vertices.len(), self.triangles.len() * 3);
        Self {
            scene,
            smooth,
            triangles: self.triangles.clone(),
        }
    }

    /// The scene this is the surface of.
    pub fn scene(&self) -> &Arc<SceneData> {
        &self.scene
    }

    /// Whether cutout edges are where smooth sampling puts them.
    pub fn smooth(&self) -> bool {
        self.smooth
    }

    /// Where the ray from `origin` along `dir` first meets a face that
    /// shows: faces its material culls and texels its cutout drops let it
    /// through.
    pub fn hit(&self, origin: Vec3, dir: Vec3) -> Option<Hit> {
        let vertices = &self.scene.mesh.vertices;
        let mut best: Option<(f32, Hit)> = None;
        for (i, &(material, island)) in self.triangles.iter().enumerate() {
            let m = &self.scene.materials[material];
            let corners = &vertices[i * 3..i * 3 + 3];
            let [a, b, c] = std::array::from_fn(|k| Vec3::from(corners[k].position));
            let Some((t, u, v)) = intersect(origin, dir, [a, b, c], !m.double_sided) else {
                continue;
            };
            if best.is_some_and(|(d, _)| d <= t) {
                continue;
            }
            let [ua, ub, uc] = std::array::from_fn(|k| Vec2::from(corners[k].uv));
            let uv = ua * (1.0 - u - v) + ub * u + uc * v;
            let center = (ua + ub + uc) / 3.0;
            let [(x, cx), (y, cy)] =
                [0, 1].map(|k| texel(uv[k], center[k], m.wrap[k], m.size[k] as f32));
            let point = Vec2::new(x, y);
            if m.alpha == AlphaMode::Cutout && alpha(m, point, self.smooth) < 0.5 {
                continue;
            }
            let hit = Hit {
                texture: m.texture,
                point,
                cell: IVec2::new(cx, cy),
                island,
            };
            best = Some((t, hit));
        }
        best.map(|(_, h)| h)
    }
}

/// Möller–Trumbore: the distance along the ray and the barycentric weights
/// of the triangle's second and third corners where the ray meets it, if it
/// does in front of `origin`. With `cull`, a triangle seen from the back,
/// whose corners run clockwise, is missed.
fn intersect(origin: Vec3, dir: Vec3, [a, b, c]: [Vec3; 3], cull: bool) -> Option<(f32, f32, f32)> {
    let (e1, e2) = (b - a, c - a);
    let p = dir.cross(e2);
    let det = e1.dot(p);
    if (cull && det < PARALLEL) || det.abs() < PARALLEL {
        return None;
    }
    let s = origin - a;
    let u = s.dot(p) / det;
    let q = s.cross(e1);
    let v = dir.dot(q) / det;
    let t = e2.dot(q) / det;
    let inside = (0.0..=1.0).contains(&u) && v >= 0.0 && u + v <= 1.0;
    (inside && t > 0.0).then_some((t, u, v))
}

/// A UV coordinate on one axis as a texel coordinate on a texture `size`
/// texels across, and the repeat of the texture it is in, for a point of a
/// triangle whose corners average `center`. A point within rounding of the
/// edge between two repeats belongs to the one its triangle is in, so a
/// triangle ending on the texture's edge stays on it, just inside its last
/// texel. Clamped coordinates
/// on or past the texture's far edge, or before its near one, land on the
/// edge texel, which the view shows there.
fn texel(uv: f32, center: f32, mode: Wrap, size: f32) -> (f32, i32) {
    if mode == Wrap::Clamp {
        let half = 0.5 / size;
        let t = if (0.0..1.0).contains(&uv) {
            uv
        } else {
            uv.clamp(half, 1.0 - half)
        };
        return (t * size, 0);
    }
    let mut cell = uv.floor();
    if uv - cell < EDGE && center < cell {
        cell -= 1.0;
    } else if cell + 1.0 - uv < EDGE && center > cell + 1.0 {
        cell += 1.0;
    }
    let f = uv - cell;
    let mirrored = mode == Wrap::Mirror && cell.rem_euclid(2.0) >= 1.0;
    let t = if mirrored { 1.0 - f } else { f };
    // The far edge itself is in the last texel, which the view shows there.
    ((t * size).min(size - size * f32::EPSILON), cell as i32)
}

/// The alpha, from 0 to 1, that material `m` shows at texel point `point`:
/// of the texel under it, or with `smooth`, blended from the four nearest
/// as the view's sampler does.
fn alpha(m: &crate::render::MaterialData, point: Vec2, smooth: bool) -> f32 {
    let [w, h] = m.size.map(i64::from);
    let at = |x: i64, y: i64| {
        let x = wrap_index(x, w, m.wrap[0]);
        let y = wrap_index(y, h, m.wrap[1]);
        f32::from(m.pixels[((y * w + x) * 4 + 3) as usize]) / 255.0
    };
    if !smooth {
        return at(point.x.floor() as i64, point.y.floor() as i64);
    }
    let p = point - Vec2::splat(0.5);
    let (x, y) = (p.x.floor(), p.y.floor());
    let (fx, fy) = (p.x - x, p.y - y);
    let (x, y) = (x as i64, y as i64);
    let top = at(x, y) * (1.0 - fx) + at(x + 1, y) * fx;
    let bottom = at(x, y + 1) * (1.0 - fx) + at(x + 1, y + 1) * fx;
    top * (1.0 - fy) + bottom * fy
}

/// Texel `i` of a row or column `n` long, wrapped as the sampler wraps it.
fn wrap_index(i: i64, n: i64, mode: Wrap) -> i64 {
    match mode {
        Wrap::Clamp => i.clamp(0, n - 1),
        Wrap::Repeat => i.rem_euclid(n),
        Wrap::Mirror => {
            let m = i.rem_euclid(2 * n);
            if m < n { m } else { 2 * n - 1 - m }
        }
    }
}

/// The UV island of each triangle: triangles of one texture whose UVs run
/// the same way around (so not folded over each other), sharing an edge
/// whose ends have the same position and UV coordinate, are on one island.
fn islands(scene: &SceneData, materials: &[usize]) -> Vec<usize> {
    let vertices = &scene.mesh.vertices;
    // Positions are matched on a grid a millionth of the model's size
    // across, so copies of a vertex posed with tiny differences usually
    // match, though two on either side of a grid line don't.
    let span = (scene.mesh.bounds_max - scene.mesh.bounds_min).max_element();
    let step = (span * 1e-6).max(f32::MIN_POSITIVE);
    let corner = |i: usize| {
        let v = &vertices[i];
        let p = v.position.map(|c| (c / step).round() as i64);
        (p, v.uv.map(f32::to_bits))
    };
    let winding = |t: usize| {
        let [a, b, c] = [0, 1, 2].map(|k| Vec2::from(vertices[t * 3 + k].uv));
        (b - a).perp_dot(c - a) >= 0.0
    };
    let mut parent: Vec<usize> = (0..materials.len()).collect();
    let mut edges = HashMap::new();
    for (t, &m) in materials.iter().enumerate() {
        for k in 0..3 {
            let (a, b) = (corner(t * 3 + k), corner(t * 3 + (k + 1) % 3));
            let key = (scene.materials[m].texture, winding(t), a.min(b), a.max(b));
            match edges.entry(key) {
                std::collections::hash_map::Entry::Occupied(e) => {
                    union(&mut parent, *e.get(), t);
                }
                std::collections::hash_map::Entry::Vacant(e) => {
                    e.insert(t);
                }
            }
        }
    }
    (0..materials.len()).map(|t| root(&mut parent, t)).collect()
}

fn root(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

fn union(parent: &mut [usize], a: usize, b: usize) {
    let (a, b) = (root(parent, a), root(parent, b));
    parent[a.max(b)] = a.min(b);
}

#[cfg(test)]
pub(crate) mod tests {
    use glam::UVec2;
    use mint_core::{
        Corner, IndexedImage, Material, MaterialId, Mesh, Model, NodeId, Palette, PaletteIndex,
        Pose, Project, Rgba8, Slot, Texture, Transform, Triangle, Variant, Vertex,
    };

    use super::*;

    /// A triangle corner at `position` with UV coordinate `uv`.
    pub(crate) type C = ([f32; 3], [f32; 2]);

    /// A project with one 8x4 texture of colors 0 (clear) and 1 (opaque),
    /// clear in its left half, and one material per entry of `materials`,
    /// drawing `triangles` as `(material, corners)`.
    pub(crate) fn surface(
        materials: &[(Wrap, AlphaMode, bool)],
        triangles: &[(u32, [C; 3])],
    ) -> Surface {
        let mut model = Model::new("root", Transform::default());
        let mut vertices = Vec::new();
        let mut tris = Vec::new();
        for &(material, corners) in triangles {
            let corners = corners.map(|(p, uv)| {
                vertices.push(Vertex {
                    position: Vec3::from(p),
                    normal: Vec3::Z,
                    follows: None,
                });
                Corner {
                    vertex: vertices.len() as u32 - 1,
                    uv: Vec2::from(uv),
                }
            });
            tris.push(Triangle {
                corners,
                material: MaterialId(material),
            });
        }
        model.node_mut(NodeId::ROOT).mesh = Some(Mesh {
            vertices,
            triangles: tris,
        });
        let size = UVec2::new(8, 4);
        let pixels = (0..32)
            .map(|i| PaletteIndex(u8::from(i % 8 >= 4)))
            .collect();
        let img = IndexedImage::new(size, pixels).unwrap();
        let texture = Texture::painted(
            "t",
            Slot("body".into()),
            size,
            [Variant::imported("1", img)],
        )
        .unwrap();
        let project = Project::new(
            mint_core::ExportTarget::new("test"),
            Palette::from_colors([Rgba8::new(0, 0, 0, 0), Rgba8::new(255, 255, 255, 255)]).unwrap(),
            model,
            vec![texture],
            materials
                .iter()
                .map(|&(wrap, alpha, double_sided)| Material {
                    texture: TextureId(0),
                    wrap: [wrap; 2],
                    alpha,
                    double_sided,
                    tint: Rgba8::new(255, 255, 255, 255),
                })
                .collect(),
            Pose::default(),
        )
        .unwrap();
        let scene = Arc::new(SceneData::new(&project, &[0], 0));
        Surface::new(scene, false)
    }

    pub(crate) const OPAQUE: (Wrap, AlphaMode, bool) = (Wrap::Clamp, AlphaMode::Opaque, false);

    /// A counterclockwise unit square in the plane `z`, facing +Z, its UVs
    /// running from `uv0` to `uv1`, as two triangles of `material`.
    pub(crate) fn square(
        material: u32,
        x: f32,
        z: f32,
        uv0: [f32; 2],
        uv1: [f32; 2],
    ) -> [(u32, [C; 3]); 2] {
        let p = |dx: f32, dy: f32| [x + dx, dy, z];
        let (a, b, c, d) = (
            (p(0.0, 0.0), [uv0[0], uv1[1]]),
            (p(1.0, 0.0), [uv1[0], uv1[1]]),
            (p(1.0, 1.0), [uv1[0], uv0[1]]),
            (p(0.0, 1.0), [uv0[0], uv0[1]]),
        );
        [(material, [a, b, c]), (material, [a, c, d])]
    }

    /// Where a ray straight down -Z through `(x, y)` meets the surface.
    fn down(s: &Surface, x: f32, y: f32) -> Option<Hit> {
        s.hit(Vec3::new(x, y, 10.0), Vec3::NEG_Z)
    }

    fn near(a: Vec2, b: Vec2) -> bool {
        a.distance(b) < 1e-4
    }

    #[test]
    fn a_hit_gives_its_texture_point() {
        let s = surface(&[OPAQUE], &square(0, 0.0, 0.0, [0.0, 0.0], [1.0, 1.0]));
        // UV y runs down the texture while the square's y runs up.
        let h = down(&s, 0.25, 0.75).unwrap();
        assert!(near(h.point, Vec2::new(2.0, 1.0)), "{h:?}");
        assert!(down(&s, 1.5, 0.5).is_none());
    }

    #[test]
    fn texture_edges_stay_on_the_edge() {
        // A square covering the texture's second repeat to the right: its
        // right edge is u = 2, which repeats to the texture's right edge,
        // not its left.
        let s = surface(
            &[(Wrap::Repeat, AlphaMode::Opaque, false)],
            &square(0, 0.0, 0.0, [1.0, 0.0], [2.0, 1.0]),
        );
        let h = down(&s, 1.0 - 1e-6, 0.5).unwrap();
        assert!(near(h.point, Vec2::new(8.0, 2.0)), "{h:?}");
        let h = down(&s, 1e-6, 0.5).unwrap();
        assert!(near(h.point, Vec2::new(0.0, 2.0)), "{h:?}");
        // Clamped UVs past the edge land on the edge texel, which shows
        // there.
        let s = surface(&[OPAQUE], &square(0, 0.0, 0.0, [0.5, 0.0], [1.5, 1.0]));
        let h = down(&s, 0.9, 0.5).unwrap();
        assert!(near(h.point, Vec2::new(7.5, 2.0)), "{h:?}");
    }

    #[test]
    fn mirrored_uvs_land_on_the_same_texels() {
        // Two squares side by side: the right one's UVs run backwards, as
        // on the other half of a symmetric model; and a third uses a
        // mirror wrap past u = 1 to do the same.
        let mut tris = square(0, 0.0, 0.0, [0.0, 0.0], [1.0, 1.0]).to_vec();
        tris.extend(square(0, 1.0, 0.0, [1.0, 0.0], [0.0, 1.0]));
        tris.extend(square(1, 2.0, 0.0, [1.0, 0.0], [2.0, 1.0]));
        let mirror = (Wrap::Mirror, AlphaMode::Opaque, false);
        // Mirrored UVs turn a front face's UVs clockwise, which must not
        // cull it.
        let s = surface(&[OPAQUE, mirror], &tris);
        let left = down(&s, 0.25, 0.5).unwrap();
        let right = down(&s, 1.75, 0.5).unwrap();
        let wrapped = down(&s, 2.75, 0.5).unwrap();
        assert!(near(left.point, Vec2::new(2.0, 2.0)), "{left:?}");
        assert!(near(right.point, left.point), "{right:?}");
        assert!(near(wrapped.point, left.point), "{wrapped:?}");
    }

    #[test]
    fn rays_pass_faces_that_dont_show() {
        // A culled back face, then a cutout face clear in its left half,
        // over an opaque square that catches what passes.
        let back = square(0, 0.0, 2.0, [0.0, 0.0], [1.0, 1.0]).map(|(m, [a, b, c])| (m, [a, c, b]));
        let mut tris = back.to_vec();
        tris.extend(square(1, 0.0, 1.0, [0.0, 0.0], [1.0, 1.0]));
        tris.extend(square(0, 0.0, 0.0, [0.0, 0.0], [0.5, 0.5]));
        let cutout = (Wrap::Clamp, AlphaMode::Cutout, false);
        let s = surface(&[OPAQUE, cutout], &tris);
        // On the cutout's clear half, the ray reaches the bottom square.
        let h = down(&s, 0.25, 0.5).unwrap();
        assert!(near(h.point, Vec2::new(1.0, 1.0)), "{h:?}");
        let h = down(&s, 0.75, 0.5).unwrap();
        assert!(near(h.point, Vec2::new(6.0, 2.0)), "{h:?}");
        // A double-sided material shows its back.
        let s = surface(&[(Wrap::Clamp, AlphaMode::Opaque, true)], &back);
        assert!(down(&s, 0.25, 0.5).is_some());
    }

    #[test]
    fn strokes_join_within_an_island() {
        // Two squares sharing an edge with the same UVs are one island; a
        // third shares an edge in space but not in UV.
        let mut tris = square(0, 0.0, 0.0, [0.0, 0.0], [0.25, 1.0]).to_vec();
        tris.extend(square(0, 1.0, 0.0, [0.25, 0.0], [0.5, 1.0]));
        tris.extend(square(0, 2.0, 0.0, [0.75, 0.0], [1.0, 1.0]));
        let s = surface(&[OPAQUE], &tris);
        let [a, b, c] = [0.5, 1.5, 2.5].map(|x| down(&s, x, 0.5).unwrap());
        assert!(a.joins(&b));
        assert!(!b.joins(&c));
        // Within one island, a repeat wrap between two points breaks it.
        let repeat = (Wrap::Repeat, AlphaMode::Opaque, false);
        let s = surface(&[repeat], &square(0, 0.0, 0.0, [0.5, 0.0], [1.5, 1.0]));
        let [a, b, c] = [0.1, 0.4, 0.6].map(|x| down(&s, x, 0.5).unwrap());
        assert!(a.joins(&b));
        assert!(!b.joins(&c));
        // So does a mirror wrap's fold, though the texel is the same on
        // either side of it.
        let mirror = (Wrap::Mirror, AlphaMode::Opaque, false);
        let s = surface(&[mirror], &square(0, 0.0, 0.0, [0.5, 0.0], [1.5, 1.0]));
        let [b, c] = [0.45, 0.55].map(|x| down(&s, x, 0.5).unwrap());
        assert!(near(b.point, c.point));
        assert!(!b.joins(&c));
        // And a fold in the UVs themselves: halves sharing their middle
        // edge's UVs, but running opposite ways, are separate islands.
        let mut tris = square(0, 0.0, 0.0, [0.0, 0.0], [0.5, 1.0]).to_vec();
        tris.extend(square(0, 1.0, 0.0, [0.5, 0.0], [0.0, 1.0]));
        let s = surface(&[OPAQUE], &tris);
        let [b, c] = [0.9, 1.1].map(|x| down(&s, x, 0.5).unwrap());
        assert!(!b.joins(&c));
    }

    #[test]
    fn smooth_cutouts_match_the_sampler() {
        // One opaque texel in a clear 4x4 texture: sampled smoothly, its
        // corners are clear, as the view shows them.
        let mut pixels = vec![0; 4 * 4 * 4];
        pixels[(5 * 4) + 3] = 255;
        let m = crate::render::MaterialData {
            texture: TextureId(0),
            pixels,
            size: [4, 4],
            wrap: [Wrap::Clamp; 2],
            alpha: AlphaMode::Cutout,
            double_sided: false,
            tint: [1.0; 4],
        };
        let corner = Vec2::new(1.05, 1.05);
        assert!(alpha(&m, corner, false) > 0.5);
        assert!(alpha(&m, corner, true) < 0.5);
        let center = Vec2::new(1.5, 1.5);
        assert!(alpha(&m, center, true) > 0.5);
    }
}
