use std::collections::BTreeMap;
use std::sync::Arc;

use crate::{
    Layer, Material, MaterialId, Model, Palette, PaletteFull, PaletteIndex, Pose, Rgba8, Shape,
    Texture, TextureId, Variant,
};

/// The export target a project is built for: its id, such as `ac-villager`,
/// and settings that target's adapter defines and reads, such as the name of
/// the game model the project is drawn on.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ExportTarget {
    pub id: String,
    pub options: BTreeMap<String, String>,
}

impl ExportTarget {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            options: BTreeMap::new(),
        }
    }
}

/// A painted variant of one of a project's textures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VariantRef {
    pub texture: TextureId,
    pub variant: usize,
}

/// The editable document for one model.
///
/// Every palette index in it, in underlays and shapes, refers to an entry of
/// its palette; the editing methods keep it so.
#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    target: ExportTarget,
    palette: Palette,
    /// Shared between clones, so snapshots of a project for undo are cheap.
    model: Arc<Model>,
    textures: Vec<Texture>,
    materials: Vec<Material>,
    pose: Pose,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PaletteError {
    #[error("palette entry {index} does not exist; the palette has {len}")]
    Missing { index: u8, len: usize },
    #[error(transparent)]
    Full(#[from] PaletteFull),
    #[error("palette entry {0} cannot merge into itself")]
    SameEntry(u8),
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
        target: ExportTarget,
        palette: Palette,
        model: Model,
        textures: Vec<Texture>,
        materials: Vec<Material>,
        pose: Pose,
    ) -> Result<Self, ProjectError> {
        for t in &textures {
            for v in t.variants() {
                let shapes = v.layers().iter().flat_map(Layer::shapes).map(Shape::color);
                if let Some(PaletteIndex(index)) = v
                    .underlay()
                    .pixels()
                    .iter()
                    .copied()
                    .chain(shapes)
                    .find(|&i| !palette.contains(i))
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
            model: Arc::new(model),
            textures,
            materials,
            pose,
        })
    }

    #[must_use]
    pub fn target(&self) -> &ExportTarget {
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

    /// Panics if `r` is not a painted variant of this project.
    #[must_use]
    pub fn variant(&self, r: VariantRef) -> &Variant {
        &self.textures[r.texture.index()].variants()[r.variant]
    }

    /// The variant, to edit its layers. Panics if `r` is not a painted
    /// variant of this project.
    pub fn variant_mut(&mut self, r: VariantRef) -> &mut Variant {
        &mut self.textures[r.texture.index()].variants_mut()[r.variant]
    }

    fn check_color(&self, index: PaletteIndex) -> Result<(), PaletteError> {
        if self.palette.contains(index) {
            Ok(())
        } else {
            Err(PaletteError::Missing {
                index: index.0,
                len: self.palette.len(),
            })
        }
    }

    /// Inserts `shape` into a layer of `r` before position `index`.
    pub fn insert_shape(
        &mut self,
        r: VariantRef,
        layer: usize,
        index: usize,
        shape: Shape,
    ) -> Result<(), PaletteError> {
        self.check_color(shape.color())?;
        self.variant_mut(r)
            .layer_mut(layer)
            .insert_shape(index, shape);
        Ok(())
    }

    pub fn set_shape_color(
        &mut self,
        r: VariantRef,
        layer: usize,
        shape: usize,
        color: PaletteIndex,
    ) -> Result<(), PaletteError> {
        self.check_color(color)?;
        self.variant_mut(r).layer_mut(layer).shapes_mut()[shape].set_color(color);
        Ok(())
    }

    /// Inserts `layer`, shapes and all, into `r` at position `index`.
    pub fn insert_layer(
        &mut self,
        r: VariantRef,
        index: usize,
        layer: Layer,
    ) -> Result<(), PaletteError> {
        for s in layer.shapes() {
            self.check_color(s.color())?;
        }
        self.variant_mut(r).insert_layer(index, layer);
        Ok(())
    }

    pub fn set_color(&mut self, index: PaletteIndex, color: Rgba8) -> Result<(), PaletteError> {
        self.check_color(index)?;
        self.palette.set(index, color);
        Ok(())
    }

    /// Appends a palette entry.
    pub fn add_color(&mut self, color: Rgba8) -> Result<PaletteIndex, PaletteError> {
        Ok(self.palette.push(color)?)
    }

    /// Removes palette entry `from`, recoloring every pixel and shape that
    /// used it with `into`. Later entries move down by one.
    pub fn merge_color(
        &mut self,
        from: PaletteIndex,
        into: PaletteIndex,
    ) -> Result<(), PaletteError> {
        self.check_color(from)?;
        self.check_color(into)?;
        if from == into {
            return Err(PaletteError::SameEntry(from.0));
        }
        let remap = |i: PaletteIndex| {
            let i = if i == from { into } else { i };
            if i > from { PaletteIndex(i.0 - 1) } else { i }
        };
        for t in &mut self.textures {
            for v in t.variants_mut() {
                v.underlay_mut().remap(remap);
                for l in v.layers_mut() {
                    for s in l.shapes_mut() {
                        s.set_color(remap(s.color()));
                    }
                }
            }
        }
        self.palette.remove(from);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use glam::{UVec2, Vec2};

    use super::*;
    use crate::{IndexedImage, Slot, Style, Transform, shape};

    const V: VariantRef = VariantRef {
        texture: TextureId(0),
        variant: 0,
    };

    fn project(colors: usize, pixels: [u8; 4]) -> Project {
        let palette =
            Palette::from_colors((0..colors).map(|i| Rgba8::new(i as u8, 0, 0, 255))).unwrap();
        let img = IndexedImage::new(UVec2::new(2, 2), pixels.map(PaletteIndex).to_vec()).unwrap();
        let mut t = Texture::painted(
            "body",
            Slot("body".into()),
            UVec2::new(2, 2),
            [("1".to_owned(), img)],
        )
        .unwrap();
        t.variants_mut()[0].add_layer(0, "paint");
        Project::new(
            ExportTarget::new("test"),
            palette,
            Model::new("root", Transform::default()),
            vec![t],
            Vec::new(),
            Pose::default(),
        )
        .unwrap()
    }

    fn dot(color: u8) -> Shape {
        Shape::new(
            shape::rectangle(Vec2::ZERO, Vec2::ONE),
            Style::Fill,
            PaletteIndex(color),
        )
        .unwrap()
    }

    fn shape_colors(p: &Project) -> Vec<u8> {
        p.variant(V).layers()[0]
            .shapes()
            .iter()
            .map(|s| s.color().0)
            .collect()
    }

    #[test]
    fn shapes_and_layers_must_use_palette_entries() {
        let mut p = project(2, [0; 4]);
        let missing = Err(PaletteError::Missing { index: 2, len: 2 });
        assert_eq!(p.insert_shape(V, 0, 0, dot(2)), missing);
        assert_eq!(p.insert_shape(V, 0, 0, dot(1)), Ok(()));
        assert_eq!(p.set_shape_color(V, 0, 0, PaletteIndex(2)), missing);
        let mut layer = Layer::new("copied");
        layer.insert_shape(0, dot(2));
        assert_eq!(p.insert_layer(V, 0, layer.clone()), missing);
        p.add_color(Rgba8::new(9, 9, 9, 255)).unwrap();
        assert_eq!(p.insert_layer(V, 0, layer), Ok(()));
        assert_eq!(p.variant(V).layers().len(), 2);
    }

    #[test]
    fn project_rejects_shapes_with_missing_colors() {
        let p = project(2, [0; 4]);
        let mut textures = p.textures().to_vec();
        textures[0].variants_mut()[0]
            .layer_mut(0)
            .insert_shape(0, dot(5));
        let err = Project::new(
            p.target().clone(),
            p.palette().clone(),
            p.model().clone(),
            textures,
            Vec::new(),
            Pose::default(),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ProjectError::MissingPaletteEntry { index: 5, .. }
        ));
    }

    #[test]
    fn merge_recolors_and_renumbers() {
        let mut p = project(4, [0, 1, 2, 3]);
        for c in [3, 1, 2] {
            p.insert_shape(V, 0, 0, dot(c)).unwrap();
        }
        p.merge_color(PaletteIndex(1), PaletteIndex(3)).unwrap();
        // 1 becomes 3; entries above 1 move down.
        let px: Vec<u8> = p
            .variant(V)
            .underlay()
            .pixels()
            .iter()
            .map(|i| i.0)
            .collect();
        assert_eq!(px, [0, 2, 1, 2]);
        assert_eq!(shape_colors(&p), [1, 2, 2]);
        assert_eq!(
            p.palette().colors(),
            &[0, 2, 3].map(|r| Rgba8::new(r, 0, 0, 255))
        );
        assert_eq!(
            p.merge_color(PaletteIndex(0), PaletteIndex(0)),
            Err(PaletteError::SameEntry(0))
        );
        assert_eq!(
            p.merge_color(PaletteIndex(3), PaletteIndex(0)),
            Err(PaletteError::Missing { index: 3, len: 3 })
        );
    }

    #[test]
    fn palette_is_capped() {
        let mut p = project(Palette::MAX_LEN, [0; 4]);
        assert_eq!(
            p.add_color(Rgba8::new(0, 0, 0, 0)),
            Err(PaletteError::Full(PaletteFull))
        );
        assert_eq!(
            p.set_color(PaletteIndex(255), Rgba8::new(1, 2, 3, 4)),
            Ok(())
        );
        assert_eq!(p.palette().colors()[255], Rgba8::new(1, 2, 3, 4));
    }
}
