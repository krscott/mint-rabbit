//! CPU-side preparation of a project for display: posed triangles in drawing
//! order, batched in runs of one material, and textures resolved to the
//! colors the target shows.

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
    match texture.content() {
        TextureContent::External { preview } => preview.pixels.clone(),
        TextureContent::Painted(variants) => {
            let colors = project.palette().colors();
            variants[variant]
                .underlay()
                .pixels()
                .iter()
                // Projects guarantee underlay pixels name palette entries.
                .map(|&i| display_color(project, colors[usize::from(i.0)]))
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
#[derive(Clone, Copy, Debug)]
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

/// Poses every mesh and batches its triangles in drawing order.
pub fn posed_mesh(project: &Project) -> PosedMesh {
    let model = project.model();
    let world = model.world_matrices(project.pose());
    // Draw in the game's order, depth-first by node and then by triangle,
    // batching runs of one material: the first of two coincident triangles
    // is the one that shows.
    let mut vertices = Vec::new();
    let mut batches: Vec<Batch> = Vec::new();
    for id in model.preorder() {
        let Some(mesh) = &model.node(id).mesh else {
            continue;
        };
        for t in &mesh.triangles {
            match batches.last_mut() {
                Some(b) if b.material == t.material => b.count += 3,
                _ => batches.push(Batch {
                    material: t.material,
                    first: vertices.len() as u32,
                    count: 3,
                }),
            }
            for c in t.corners {
                let v = mesh.vertices[c.vertex as usize];
                let m = world[v.followed(id).index()];
                let normal = (Mat3::from_mat4(m) * v.normal).normalize_or_zero();
                vertices.push(GpuVertex {
                    position: m.transform_point3(v.position).to_array(),
                    normal: normal.to_array(),
                    uv: c.uv.to_array(),
                });
            }
        }
    }
    // An empty model frames the origin.
    let bounds = |pick: fn(Vec3, Vec3) -> Vec3| {
        vertices
            .iter()
            .map(|v| Vec3::from(v.position))
            .reduce(pick)
            .unwrap_or(Vec3::ZERO)
    };
    let (bounds_min, bounds_max) = (bounds(Vec3::min), bounds(Vec3::max));
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

    /// A root and an arm; the arm has one triangle per entry of `materials`,
    /// each drawing with that material (0 or 1).
    fn project_with(materials: &[u32]) -> Project {
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
            triangles: materials
                .iter()
                .map(|&m| Triangle {
                    corners: [corner(0), corner(1), corner(2)],
                    material: MaterialId(m),
                })
                .collect(),
        });
        let mut pose = Pose::default();
        pose.set_rotation(child, Quat::from_rotation_z(std::f32::consts::FRAC_PI_2));
        let img = IndexedImage::new(UVec2::new(1, 1), vec![PaletteIndex(0)]).unwrap();
        let texture =
            Texture::painted("t", Slot("body".into()), UVec2::ONE, [("1".into(), img)]).unwrap();
        Project::new(
            TargetId(mint_ac::TARGET.into()),
            Palette::from_colors([Rgba8::new(255, 1, 2, 255)]).unwrap(),
            model,
            vec![texture],
            [AlphaMode::Opaque, AlphaMode::Cutout]
                .map(|alpha| Material {
                    texture: TextureId(0),
                    wrap: [Wrap::Clamp; 2],
                    alpha,
                    double_sided: false,
                    tint: Rgba8::new(255, 255, 255, 255),
                })
                .to_vec(),
            pose,
        )
        .unwrap()
    }

    fn project() -> Project {
        project_with(&[0])
    }

    #[test]
    fn batches_keep_drawing_order() {
        let posed = posed_mesh(&project_with(&[0, 0, 1, 0]));
        let batches: Vec<_> = posed
            .batches
            .iter()
            .map(|b| (b.material.0, b.first, b.count))
            .collect();
        assert_eq!(batches, vec![(0, 0, 6), (1, 6, 3), (0, 9, 3)]);
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
