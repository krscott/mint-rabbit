//! CPU-side preparation of a project for display: posed triangles grouped
//! by material, and textures resolved to the colors the target shows.

use glam::{Mat3, Vec2, Vec3};
use mint_core::{MaterialId, Project, Rgba8, Texture, TextureContent, TextureId};

/// Converts a palette color to what the export target displays.
pub fn display_color(project: &Project, c: Rgba8) -> Rgba8 {
    if project.target().0 == mint_ac::TARGET {
        mint_ac::quantize_color(c)
    } else {
        c
    }
}

/// The RGBA pixels of one variant (or the preview of an external texture).
pub fn texture_pixels(project: &Project, texture: &Texture, variant: usize) -> Vec<Rgba8> {
    let size = texture.size();
    let blank = vec![Rgba8::new(0, 0, 0, 0); (size.x * size.y) as usize];
    match texture.content() {
        TextureContent::External { preview } => preview.pixels.clone(),
        TextureContent::Painted(variants) => {
            let Some(underlay) = variants.get(variant).and_then(|v| v.underlay()) else {
                return blank;
            };
            underlay
                .pixels()
                .iter()
                .map(|&i| {
                    project
                        .palette()
                        .get(i)
                        .map_or(Rgba8::new(255, 0, 255, 255), |c| display_color(project, c))
                })
                .collect()
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

/// Triangle corners of one material, ready to draw.
pub struct Batch {
    pub material: MaterialId,
    pub first: u32,
    pub count: u32,
}

/// Posed geometry of a whole project.
pub struct PosedMesh {
    pub vertices: Vec<GpuVertex>,
    pub batches: Vec<Batch>,
    pub bounds_min: Vec3,
    pub bounds_max: Vec3,
}

/// Poses every mesh and groups the triangle corners by material.
pub fn posed_mesh(project: &Project) -> PosedMesh {
    let model = project.model();
    let world = model.world_matrices(project.pose());
    let mut per_material: Vec<Vec<GpuVertex>> = vec![Vec::new(); project.materials().len()];
    for id in model.ids() {
        let Some(mesh) = &model.node(id).mesh else {
            continue;
        };
        for t in &mesh.triangles {
            for c in t.corners {
                let v = mesh.vertices[c.vertex as usize];
                let m = world[v.followed(id).index()];
                let normal = (Mat3::from_mat4(m) * v.normal).normalize_or_zero();
                per_material[t.material.index()].push(GpuVertex {
                    position: m.transform_point3(v.position).to_array(),
                    normal: normal.to_array(),
                    uv: c.uv.to_array(),
                });
            }
        }
    }
    let mut vertices = Vec::new();
    let mut batches = Vec::new();
    for (i, vs) in per_material.into_iter().enumerate() {
        if vs.is_empty() {
            continue;
        }
        batches.push(Batch {
            material: MaterialId(i as u32),
            first: vertices.len() as u32,
            count: vs.len() as u32,
        });
        vertices.extend(vs);
    }
    let (bounds_min, bounds_max) = vertices.iter().fold(
        (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
        |(lo, hi), v| {
            let p = Vec3::from(v.position);
            (lo.min(p), hi.max(p))
        },
    );
    PosedMesh {
        vertices,
        batches,
        bounds_min,
        bounds_max,
    }
}

/// UV-space triangles that draw with `texture`, for the Texture View
/// wireframe.
pub fn uv_triangles(project: &Project, texture: TextureId) -> Vec<[Vec2; 3]> {
    let model = project.model();
    model
        .ids()
        .filter_map(|id| model.node(id).mesh.as_ref())
        .flat_map(|m| m.triangles.iter())
        .filter(|t| project.material(t.material).texture == texture)
        .map(|t| t.corners.map(|c| c.uv))
        .collect()
}

#[cfg(test)]
mod tests {
    use glam::{Quat, UVec2};
    use mint_core::{
        AlphaMode, Corner, IndexedImage, Material, Mesh, Model, NodeId, Palette, PaletteIndex,
        Pose, Slot, TargetId, Transform, Triangle, Vertex, Wrap,
    };

    use super::*;

    fn project() -> Project {
        let mut model = Model::new("root", Transform::default());
        let child = model.add_child(
            NodeId::ROOT,
            "arm",
            Transform::from_translation(Vec3::X * 10.0),
        );
        let vertex = |p: Vec3, follows| Vertex {
            position: p,
            normal: Vec3::Y,
            follows,
        };
        let corner = |vertex| Corner {
            vertex,
            uv: Vec2::ZERO,
        };
        model.node_mut(child).mesh = Some(Mesh {
            vertices: vec![
                vertex(Vec3::ZERO, Some(NodeId::ROOT)),
                vertex(Vec3::X, None),
                vertex(Vec3::Y, None),
            ],
            triangles: vec![Triangle {
                corners: [corner(0), corner(1), corner(2)],
                material: MaterialId(0),
            }],
        });
        let mut pose = Pose::default();
        pose.set_rotation(child, Quat::from_rotation_z(std::f32::consts::FRAC_PI_2));
        let img = IndexedImage::new(UVec2::new(1, 1), vec![PaletteIndex(0)]).unwrap();
        let texture = Texture::painted(
            "t",
            Slot("body".into()),
            UVec2::ONE,
            [("1".into(), Some(img))],
        )
        .unwrap();
        Project::new(
            TargetId(mint_ac::TARGET.into()),
            Palette::from_colors([Rgba8::new(255, 1, 2, 255)]).unwrap(),
            model,
            vec![texture],
            vec![Material {
                texture: TextureId(0),
                wrap: [Wrap::Clamp; 2],
                alpha: AlphaMode::Opaque,
                double_sided: false,
                tint: Rgba8::new(255, 255, 255, 255),
            }],
            pose,
        )
        .unwrap()
    }

    #[test]
    fn seam_vertices_follow_their_node() {
        let posed = posed_mesh(&project());
        let p: Vec<Vec3> = posed
            .vertices
            .iter()
            .map(|v| Vec3::from(v.position))
            .collect();
        // The seam vertex stays at the root; the arm's own vertices are
        // offset by 10 and rotated a quarter turn.
        assert!(p[0].distance(Vec3::ZERO) < 1e-5, "{p:?}");
        assert!(p[1].distance(Vec3::new(10.0, 1.0, 0.0)) < 1e-5, "{p:?}");
        assert!(p[2].distance(Vec3::new(9.0, 0.0, 0.0)) < 1e-5, "{p:?}");
    }

    #[test]
    fn ac_colors_show_quantized() {
        let p = project();
        let px = texture_pixels(&p, &p.textures()[0], 0);
        // RGB5A3 cannot hold 1 and 2 exactly.
        assert_eq!(px, vec![Rgba8::new(255, 0, 0, 255)]);
    }
}
