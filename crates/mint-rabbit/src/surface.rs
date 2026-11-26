//! The posed model's surface as rays meet it: which texture point lies under
//! the cursor in the Model View, and which UV island it is on.

use std::collections::HashMap;
use std::sync::Arc;

use glam::{Vec2, Vec3};
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
    /// The UV coordinate before wrapping, in texels.
    unwrapped: Vec2,
    /// The UV island the point is on: triangles of one texture joined by
    /// edges whose ends share a position and a UV coordinate.
    island: usize,
}

impl Hit {
    /// Whether a stroke can run straight in texture space from `self` to
    /// `other`: both are on one island, and no wrap of the texture lies
    /// between them.
    pub fn joins(&self, other: &Self) -> bool {
        self.texture == other.texture
            && self.island == other.island
            && self.point.distance(other.point) <= self.unwrapped.distance(other.unwrapped) + 0.5
    }
}

/// The posed triangles of a scene, ready for rays.
pub struct Surface {
    scene: Arc<SceneData>,
    /// Per triangle, its material and island.
    triangles: Vec<(usize, usize)>,
}

impl Surface {
    pub fn new(scene: Arc<SceneData>) -> Self {
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
        }
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
            let size = Vec2::new(m.size[0] as f32, m.size[1] as f32);
            let center = (ua + ub + uc) / 3.0;
            let point = Vec2::new(
                wrap(uv.x, center.x, m.wrap[0]),
                wrap(uv.y, center.y, m.wrap[1]),
            ) * size;
            if m.alpha == AlphaMode::Cutout && alpha(&m.pixels, m.size, point) < 128 {
                continue;
            }
            let hit = Hit {
                texture: m.texture,
                point,
                unwrapped: uv * size,
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

/// A UV coordinate on one axis as a fraction of the texture, for a point of
/// a triangle whose corners average `center`. A point within rounding of
/// the edge between two repeats of the texture belongs to the one its
/// triangle is in, so a triangle ending on the texture's edge stays on it.
fn wrap(uv: f32, center: f32, mode: Wrap) -> f32 {
    if mode == Wrap::Clamp {
        return uv.clamp(0.0, 1.0);
    }
    let mut cell = uv.floor();
    if uv - cell < EDGE && center < cell {
        cell -= 1.0;
    } else if cell + 1.0 - uv < EDGE && center > cell + 1.0 {
        cell += 1.0;
    }
    let f = uv - cell;
    if mode == Wrap::Mirror && cell.rem_euclid(2.0) >= 1.0 {
        1.0 - f
    } else {
        f
    }
}

/// The alpha of the texel under `point`.
fn alpha(pixels: &[u8], [w, h]: [u32; 2], point: Vec2) -> u8 {
    let x = (point.x.floor().max(0.0) as u32).min(w.saturating_sub(1));
    let y = (point.y.floor().max(0.0) as u32).min(h.saturating_sub(1));
    pixels[((y * w + x) * 4 + 3) as usize]
}

/// The island of each triangle: triangles of one texture sharing an edge
/// whose ends have the same position and UV coordinate are on one island.
fn islands(scene: &SceneData, materials: &[usize]) -> Vec<usize> {
    let vertices = &scene.mesh.vertices;
    // Positions match to a millionth of the model's size, which posing
    // two meshes' copies of a seam vertex may disturb.
    let span = (scene.mesh.bounds_max - scene.mesh.bounds_min).max_element();
    let step = (span * 1e-6).max(f32::MIN_POSITIVE);
    let corner = |i: usize| {
        let v = &vertices[i];
        let p = v.position.map(|c| (c / step).round() as i64);
        (p, v.uv.map(f32::to_bits))
    };
    let mut parent: Vec<usize> = (0..materials.len()).collect();
    let mut edges = HashMap::new();
    for (t, &m) in materials.iter().enumerate() {
        for k in 0..3 {
            let (a, b) = (corner(t * 3 + k), corner(t * 3 + (k + 1) % 3));
            let key = (scene.materials[m].texture, a.min(b), a.max(b));
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
        Surface::new(scene)
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
        // Clamped UVs past the edge land on it.
        let s = surface(&[OPAQUE], &square(0, 0.0, 0.0, [0.5, 0.0], [1.5, 1.0]));
        let h = down(&s, 0.9, 0.5).unwrap();
        assert!(near(h.point, Vec2::new(8.0, 2.0)), "{h:?}");
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
    }
}
