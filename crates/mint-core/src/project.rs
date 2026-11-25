use std::collections::BTreeMap;
use std::sync::Arc;

use glam::UVec2;

use crate::{
    Material, MaterialId, Model, Palette, PaletteFull, PaletteIndex, Pose, Rgba8, Texture,
    TextureId, Variant,
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
/// Every palette index in it, in raster layers and shapes, refers to an entry
/// of its palette, and a project with painted textures has at least one
/// entry, which pixels no visible layer covers take; the editing methods keep
/// it so.
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
    #[error(
        "palette entry 0 colors the pixels no visible layer covers in texture {texture:?} variant {variant:?}; cover them first, or merge another entry into entry 0"
    )]
    UncoveredPixels { texture: String, variant: String },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EditError {
    #[error("palette entry {index} does not exist; the palette has {len}")]
    MissingColor { index: u8, len: usize },
    #[error("an image of {actual} can't go in a texture of {expected}")]
    SizeMismatch { expected: UVec2, actual: UVec2 },
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
    #[error("the palette is empty, but painted textures need at least one entry")]
    EmptyPalette,
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

/// The first of `colors` that `palette` has no entry for.
fn missing_color(
    palette: &Palette,
    mut colors: impl Iterator<Item = PaletteIndex>,
) -> Option<PaletteIndex> {
    colors.find(|&c| !palette.contains(c))
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
        if palette.is_empty() && textures.iter().any(|t| !t.variants().is_empty()) {
            return Err(ProjectError::EmptyPalette);
        }
        for t in &textures {
            for v in t.variants() {
                if let Some(PaletteIndex(index)) = missing_color(&palette, v.colors()) {
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

    /// Edits a variant's name, layers, shapes, and pixels with `f`. If the
    /// edit leaves a raster layer of another size than the texture's, or a
    /// shape or pixel using a color the palette lacks, it is discarded and
    /// reported. Panics if `r` is not a painted variant of this project.
    pub fn edit_variant<T>(
        &mut self,
        r: VariantRef,
        f: impl FnOnce(&mut Variant) -> T,
    ) -> Result<T, EditError> {
        let variant = &mut self.textures[r.texture.index()].variants_mut()[r.variant];
        // Edit a copy, so a failed check or a panic in `f` leaves the
        // project as it was.
        let mut draft = variant.clone();
        let out = f(&mut draft);
        let expected = variant.size();
        if let Some(actual) = draft.size_mismatch(expected) {
            return Err(EditError::SizeMismatch { expected, actual });
        }
        if let Some(PaletteIndex(index)) = missing_color(&self.palette, draft.colors()) {
            return Err(EditError::MissingColor {
                index,
                len: self.palette.len(),
            });
        }
        *variant = draft;
        Ok(out)
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

    pub fn set_palette_color(
        &mut self,
        index: PaletteIndex,
        color: Rgba8,
    ) -> Result<(), PaletteError> {
        self.check_color(index)?;
        self.palette.set(index, color);
        Ok(())
    }

    /// Appends a palette entry.
    pub fn add_palette_color(&mut self, color: Rgba8) -> Result<PaletteIndex, PaletteError> {
        Ok(self.palette.push(color)?)
    }

    /// Removes palette entry `from`, recoloring every pixel and shape that
    /// used it with `into`. Later entries move down by one. Entry 0 can't be
    /// removed while it colors pixels that no visible layer covers, which
    /// would take the next entry's color instead of `into`'s.
    pub fn merge_palette_color(
        &mut self,
        from: PaletteIndex,
        into: PaletteIndex,
    ) -> Result<(), PaletteError> {
        self.check_color(from)?;
        self.check_color(into)?;
        if from == into {
            return Err(PaletteError::SameEntry(from.0));
        }
        if from == PaletteIndex(0) {
            for t in &self.textures {
                if let Some(v) = t.variants().iter().find(|v| v.has_uncovered()) {
                    return Err(PaletteError::UncoveredPixels {
                        texture: t.name.clone(),
                        variant: v.name.clone(),
                    });
                }
            }
        }
        let remap = |i: PaletteIndex| {
            let i = if i == from { into } else { i };
            if i > from { PaletteIndex(i.0 - 1) } else { i }
        };
        for t in &mut self.textures {
            for v in t.variants_mut() {
                for l in v.layers_mut() {
                    l.remap(remap);
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
    use crate::{IndexedImage, Layer, Raster, Shape, Slot, Style, Transform, shape};

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
            [Variant::imported("1", img)],
        )
        .unwrap();
        // Under the base layer, which is at 1.
        t.variants_mut()[0].insert_layer(0, Layer::new("paint"));
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
    fn edits_that_break_the_project_are_refused() {
        let mut p = project(2, [0; 4]);
        let missing = Err(EditError::MissingColor { index: 2, len: 2 });
        let add = |c: u8| move |v: &mut Variant| v.layers_mut()[0].insert_shape(0, dot(c));
        assert_eq!(p.edit_variant(V, add(1)), Ok(()));
        let before = p.clone();
        assert_eq!(p.edit_variant(V, add(2)), missing);
        assert_eq!(
            p.edit_variant(V, |v| v.layers_mut()[0].shapes_mut()[0] = dot(2)),
            missing
        );
        assert_eq!(p, before);

        // A panic part-way through an edit leaves the project as it was.
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            p.edit_variant(V, |v| {
                v.layers_mut()[0].insert_shape(0, dot(1));
                panic!("interrupted edit");
            })
        }));
        assert!(panicked.is_err());
        assert_eq!(p, before);

        // Pixels are checked like shapes.
        assert_eq!(
            p.edit_variant(V, |v| {
                let base = v.layers_mut()[1].raster_pixels_mut().unwrap();
                base.fill(Vec2::splat(0.5), Some(PaletteIndex(2)));
            }),
            missing
        );
        // So are raster layer sizes.
        assert_eq!(
            p.edit_variant(V, |v| {
                v.insert_layer(0, Layer::raster("small", Raster::clear(UVec2::ONE)));
            }),
            Err(EditError::SizeMismatch {
                expected: UVec2::new(2, 2),
                actual: UVec2::ONE,
            })
        );
        assert_eq!(p, before);

        // Replacing the whole variant is checked too.
        let other = project(4, [3, 0, 0, 0]);
        let foreign = other.variant(V).clone();
        assert_eq!(
            p.edit_variant(V, |v| *v = foreign),
            Err(EditError::MissingColor { index: 3, len: 2 })
        );
        assert_eq!(p, before);
        let copy = project(2, [0; 4]).variant(V).clone();
        assert_eq!(p.edit_variant(V, |v| *v = copy), Ok(()));
    }

    #[test]
    fn project_rejects_shapes_with_missing_colors() {
        let p = project(2, [0; 4]);
        let mut textures = p.textures().to_vec();
        textures[0].variants_mut()[0].layers_mut()[0].insert_shape(0, dot(5));
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
    fn painted_textures_need_a_palette_entry() {
        let p = project(1, [0; 4]);
        let mut v = Variant::new("1", UVec2::new(2, 2));
        v.insert_layer(0, Layer::new("empty"));
        let t = Texture::painted("t", Slot("body".into()), UVec2::new(2, 2), [v]).unwrap();
        let err = Project::new(
            p.target().clone(),
            Palette::default(),
            p.model().clone(),
            vec![t],
            Vec::new(),
            Pose::default(),
        )
        .unwrap_err();
        assert_eq!(err, ProjectError::EmptyPalette);
    }

    #[test]
    fn merge_recolors_and_renumbers() {
        let mut p = project(4, [0, 1, 2, 3]);
        p.edit_variant(V, |v| {
            for c in [3, 1, 2] {
                v.layers_mut()[0].insert_shape(0, dot(c));
            }
        })
        .unwrap();
        p.merge_palette_color(PaletteIndex(1), PaletteIndex(3))
            .unwrap();
        // 1 becomes 3; entries above 1 move down.
        let px: Vec<u8> = p.variant(V).layers()[1]
            .raster_pixels()
            .unwrap()
            .pixels()
            .iter()
            .map(|i| i.unwrap().0)
            .collect();
        assert_eq!(px, [0, 2, 1, 2]);
        assert_eq!(shape_colors(&p), [1, 2, 2]);
        assert_eq!(
            p.palette().colors(),
            &[0, 2, 3].map(|r| Rgba8::new(r, 0, 0, 255))
        );
        assert_eq!(
            p.merge_palette_color(PaletteIndex(0), PaletteIndex(0)),
            Err(PaletteError::SameEntry(0))
        );
        assert_eq!(
            p.merge_palette_color(PaletteIndex(3), PaletteIndex(0)),
            Err(PaletteError::Missing { index: 3, len: 3 })
        );
    }

    #[test]
    fn entry_0_stays_while_it_colors_uncovered_pixels() {
        let mut p = project(3, [1, 1, 1, 1]);
        // Hiding the base layer leaves every pixel to entry 0.
        p.edit_variant(V, |v| v.layers_mut()[1].visible = false)
            .unwrap();
        let before = p.clone();
        assert!(matches!(
            p.merge_palette_color(PaletteIndex(0), PaletteIndex(2)),
            Err(PaletteError::UncoveredPixels { .. })
        ));
        assert_eq!(p, before);
        // Merging into entry 0 keeps it, so it is allowed.
        assert_eq!(
            p.merge_palette_color(PaletteIndex(2), PaletteIndex(0)),
            Ok(())
        );
        // Once every pixel is covered, entry 0 can go.
        p.edit_variant(V, |v| v.layers_mut()[1].visible = true)
            .unwrap();
        assert_eq!(
            p.merge_palette_color(PaletteIndex(0), PaletteIndex(1)),
            Ok(())
        );
    }

    #[test]
    fn palette_is_capped() {
        let mut p = project(Palette::MAX_LEN, [0; 4]);
        assert_eq!(
            p.add_palette_color(Rgba8::new(0, 0, 0, 0)),
            Err(PaletteError::Full(PaletteFull))
        );
        assert_eq!(
            p.set_palette_color(PaletteIndex(255), Rgba8::new(1, 2, 3, 4)),
            Ok(())
        );
        assert_eq!(p.palette().colors()[255], Rgba8::new(1, 2, 3, 4));
    }
}
