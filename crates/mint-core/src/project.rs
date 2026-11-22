use crate::{Material, MaterialId, Model, Palette, PaletteIndex, Pose, Texture, TextureId};

/// Names the export target a project is built for, such as `ac-villager`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TargetId(pub String);

/// The editable document for one model.
#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    target: TargetId,
    palette: Palette,
    model: Model,
    textures: Vec<Texture>,
    materials: Vec<Material>,
    pose: Pose,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProjectError {
    #[error("material {material} uses missing texture {texture}")]
    MissingTexture { material: u32, texture: u32 },
    #[error("node {node:?} has a triangle using missing material {material}")]
    MissingMaterial { node: String, material: u32 },
    #[error("node {node:?} has a triangle using missing vertex {vertex}")]
    MissingVertex { node: String, vertex: u32 },
    #[error("node {node:?} has a vertex following missing node {follows}")]
    MissingFollowedNode { node: String, follows: u32 },
    #[error(
        "texture {texture:?} variant {variant:?} uses palette entry {index}, but the palette has {len}"
    )]
    MissingPaletteEntry {
        texture: String,
        variant: String,
        index: u8,
        len: usize,
    },
}

impl Project {
    /// Builds a project, checking that every reference between its parts
    /// resolves.
    pub fn new(
        target: TargetId,
        palette: Palette,
        model: Model,
        textures: Vec<Texture>,
        materials: Vec<Material>,
        pose: Pose,
    ) -> Result<Self, ProjectError> {
        for t in &textures {
            for v in t.variants() {
                if let Some(&PaletteIndex(index)) = v
                    .underlay()
                    .pixels()
                    .iter()
                    .find(|i| usize::from(i.0) >= palette.len())
                {
                    return Err(ProjectError::MissingPaletteEntry {
                        texture: t.name.clone(),
                        variant: v.name.clone(),
                        index,
                        len: palette.len(),
                    });
                }
            }
        }
        for (i, m) in materials.iter().enumerate() {
            if m.texture.index() >= textures.len() {
                return Err(ProjectError::MissingTexture {
                    material: i as u32,
                    texture: m.texture.0,
                });
            }
        }
        for node in model.nodes() {
            let Some(mesh) = &node.mesh else { continue };
            for v in &mesh.vertices {
                if let Some(f) = v.follows
                    && !model.contains(f)
                {
                    return Err(ProjectError::MissingFollowedNode {
                        node: node.name.clone(),
                        follows: f.0,
                    });
                }
            }
            for t in &mesh.triangles {
                if t.material.index() >= materials.len() {
                    return Err(ProjectError::MissingMaterial {
                        node: node.name.clone(),
                        material: t.material.0,
                    });
                }
                if let Some(c) = t
                    .corners
                    .iter()
                    .find(|c| c.vertex as usize >= mesh.vertices.len())
                {
                    return Err(ProjectError::MissingVertex {
                        node: node.name.clone(),
                        vertex: c.vertex,
                    });
                }
            }
        }
        Ok(Self {
            target,
            palette,
            model,
            textures,
            materials,
            pose,
        })
    }

    #[must_use]
    pub fn target(&self) -> &TargetId {
        &self.target
    }

    #[must_use]
    pub fn palette(&self) -> &Palette {
        &self.palette
    }

    #[must_use]
    pub fn model(&self) -> &Model {
        &self.model
    }

    #[must_use]
    pub fn textures(&self) -> &[Texture] {
        &self.textures
    }

    #[must_use]
    pub fn texture(&self, id: TextureId) -> &Texture {
        &self.textures[id.index()]
    }

    #[must_use]
    pub fn materials(&self) -> &[Material] {
        &self.materials
    }

    #[must_use]
    pub fn material(&self, id: MaterialId) -> &Material {
        &self.materials[id.index()]
    }

    #[must_use]
    pub fn pose(&self) -> &Pose {
        &self.pose
    }
}
