//! Saving and loading projects as a directory: `project.ron` holds
//! everything except images, which sit beside it as PNG files. A stamp is
//! one RON file whose shapes are written as a project's are.
//!
//! Raster layers are 8-bit grayscale-and-alpha PNGs: a pixel's gray value is
//! its palette index, and alpha 0 marks a pixel the layer doesn't set.
//! External texture previews are RGBA PNGs. Loading goes through the
//! same constructors as building a project in code, so a file can't produce
//! a project that breaks their rules.

use std::collections::BTreeMap;
use std::io::BufReader;
use std::path::{Component, Path, PathBuf};

use glam::{Quat, UVec2, Vec2, Vec3};
use serde::{Deserialize, Serialize};

use crate::texture::LayerContent;
use crate::{
    AlphaMode, Anchor, Axes, Corner, ExportTarget, HalfPoint, Layer, Material, Mesh, Mirror, Model,
    Modifier, ModifierKind, NodeId, Palette, PaletteIndex, Pose, Project, Radial, Raster, Rgba8,
    RgbaImage, Shape, ShapeError, Slot, Stamp, StampError, Style, Texture, TextureContent,
    TextureId, Transform, Triangle, Turns, Variant, Vertex, Wrap,
};

/// The project file inside a project directory.
pub const PROJECT_FILE: &str = "project.ron";
const IMAGE_DIR: &str = "textures";
/// Largest texture side a project file may give, the usual GPU limit.
const MAX_TEXTURE_SIDE: u32 = 8192;
/// Version of the file layout this build reads and writes.
const FORMAT: u32 = 1;
/// The extension of a stamp file.
pub const STAMP_EXTENSION: &str = "ron";
/// Version of the stamp file layout this build reads and writes.
const STAMP_FORMAT: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    #[error("texture {texture:?} is {size}; textures are at most {MAX_TEXTURE_SIDE} texels a side")]
    TextureTooLarge { texture: String, size: UVec2 },
    #[error("{0} is a link; images are saved only into a directory inside the project")]
    ImageDirLink(PathBuf),
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("{0}: {1}")]
    Png(PathBuf, png::EncodingError),
    #[error("could not write the project file: {0}")]
    Ron(#[from] ron::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("{0}: {1}")]
    Ron(PathBuf, Box<ron::error::SpannedError>),
    #[error("{0}: {1}")]
    Png(PathBuf, png::DecodingError),
    #[error("project format {0} is not supported; this build reads format {FORMAT}")]
    Format(u32),
    #[error("{0}: {1}")]
    Image(PathBuf, String),
    #[error("image path {0:?} must be a relative path inside the project directory")]
    ImagePath(String),
    #[error("texture {texture:?} is {size}; textures are at most {MAX_TEXTURE_SIDE} texels a side")]
    TextureTooLarge { texture: String, size: UVec2 },
    #[error("the project has no nodes; the first node is the model's root")]
    NoRoot,
    #[error("node {node} names parent {parent}, which is not an earlier node")]
    Parent { node: usize, parent: u32 },
    #[error("node {0} has no parent; only the first node is the root")]
    SecondRoot(usize),
    #[error(transparent)]
    Palette(crate::PaletteFull),
    #[error("texture {texture:?} variant {variant:?}: {source}")]
    Shape {
        texture: String,
        variant: String,
        source: crate::ShapeError,
    },
    #[error(transparent)]
    Texture(#[from] crate::TextureError),
    #[error(transparent)]
    Project(#[from] crate::ProjectError),
}

#[derive(Debug, thiserror::Error)]
pub enum StampFileError {
    #[error("{0} already exists")]
    Exists(PathBuf),
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("{0}: {1}")]
    Ron(PathBuf, Box<ron::error::SpannedError>),
    #[error("could not write the stamp file: {0}")]
    Encode(#[from] ron::Error),
    #[error("{0}: stamp format {1} is not supported; this build reads format {STAMP_FORMAT}")]
    Format(PathBuf, u32),
    #[error("{0}: {1}")]
    Shape(PathBuf, ShapeError),
    #[error("{0}: {1}")]
    Stamp(PathBuf, StampError),
}

#[derive(Deserialize)]
struct Version {
    format: u32,
}

#[derive(Serialize, Deserialize)]
struct ProjectFile {
    format: u32,
    target: TargetFile,
    palette: Vec<[u8; 4]>,
    /// Parents before children; the first node is the root.
    nodes: Vec<NodeFile>,
    materials: Vec<MaterialFile>,
    textures: Vec<TextureFile>,
    /// Display rotations as quaternions (x, y, z, w), by node.
    pose: BTreeMap<u32, [f32; 4]>,
}

#[derive(Serialize, Deserialize)]
struct TargetFile {
    id: String,
    options: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize)]
struct NodeFile {
    name: String,
    parent: Option<u32>,
    translation: [f32; 3],
    rotation: [f32; 4],
    scale: [f32; 3],
    mesh: Option<MeshFile>,
}

#[derive(Serialize, Deserialize)]
struct MeshFile {
    vertices: Vec<VertexFile>,
    triangles: Vec<TriangleFile>,
}

#[derive(Serialize, Deserialize)]
struct VertexFile {
    position: [f32; 3],
    normal: [f32; 3],
    follows: Option<u32>,
}

#[derive(Serialize, Deserialize)]
struct TriangleFile {
    material: u32,
    /// Vertex index and UV of each corner.
    corners: [(u32, [f32; 2]); 3],
}

#[derive(Serialize, Deserialize)]
struct MaterialFile {
    texture: u32,
    wrap: [WrapFile; 2],
    alpha: AlphaFile,
    double_sided: bool,
    tint: [u8; 4],
}

#[derive(Serialize, Deserialize)]
enum WrapFile {
    Clamp,
    Repeat,
    Mirror,
}

#[derive(Serialize, Deserialize)]
enum AlphaFile {
    Opaque,
    Cutout,
}

#[derive(Serialize, Deserialize)]
struct TextureFile {
    name: String,
    slot: String,
    size: [u32; 2],
    content: ContentFile,
}

#[derive(Serialize, Deserialize)]
enum ContentFile {
    Painted(Vec<VariantFile>),
    /// An RGBA PNG.
    External {
        preview: String,
    },
}

#[derive(Serialize, Deserialize)]
struct VariantFile {
    name: String,
    /// Bottom to top.
    layers: Vec<LayerFile>,
}

#[derive(Serialize, Deserialize)]
struct LayerFile {
    name: String,
    visible: bool,
    locked: bool,
    opacity: f32,
    /// Absent in projects saved before layers had modifiers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    modifiers: Vec<ModifierFile>,
    content: LayerContentFile,
}

#[derive(Serialize, Deserialize)]
struct ModifierFile {
    enabled: bool,
    kind: ModifierKindFile,
}

/// Lines and centers in texels, on the half-texel grid.
#[derive(Serialize, Deserialize)]
enum ModifierKindFile {
    Mirror { axes: AxesFile, at: [f32; 2] },
    Radial { turns: TurnsFile, center: [f32; 2] },
}

#[derive(Serialize, Deserialize)]
enum AxesFile {
    X,
    Y,
    Both,
}

#[derive(Serialize, Deserialize)]
enum TurnsFile {
    Half,
    Quarter,
}

#[derive(Serialize, Deserialize)]
enum LayerContentFile {
    Vector(Vec<ShapeFile>),
    /// An 8-bit grayscale-and-alpha PNG.
    Raster(String),
}

#[derive(Serialize, Deserialize)]
struct ShapeFile {
    color: u8,
    style: StyleFile,
    /// Point, incoming handle, and outgoing handle of each anchor.
    anchors: Vec<[[f32; 2]; 3]>,
}

#[derive(Serialize, Deserialize)]
struct StampFile {
    format: u32,
    colors: Vec<[u8; 4]>,
    /// Colors index `colors`.
    shapes: Vec<ShapeFile>,
}

#[derive(Serialize, Deserialize)]
enum StyleFile {
    Fill,
    Stroke { width: f32, closed: bool },
}

/// Writes `project` into directory `dir`, creating it if needed.
///
/// Image files are named after their contents, so a save only ever
/// rewrites an image the previous project file refers to with the same
/// bytes. The project file is written beside its destination and renamed
/// into place, and only then are the previous project file's images that the
/// new one doesn't use removed; an interrupted save leaves the previous
/// project intact, and the images it wrote stay behind. Saves to one
/// directory must not overlap.
pub fn save(project: &Project, dir: &Path) -> Result<(), SaveError> {
    stage(project, dir)?.commit(dir)
}

/// A save whose images are written but whose project file is not.
struct Staged {
    text: String,
    /// Names in the image directory of the images written.
    images: Vec<String>,
    /// Names in the image directory of the images the previous project file
    /// referred to.
    previous: Vec<String>,
}

/// Writes the images of `project` into `dir` and prepares its project file.
fn stage(project: &Project, dir: &Path) -> Result<Staged, SaveError> {
    // Load would refuse it; checked before anything is written.
    if let Some(t) = project
        .textures()
        .iter()
        .find(|t| t.size().max_element() > MAX_TEXTURE_SIDE)
    {
        return Err(SaveError::TextureTooLarge {
            texture: t.name.clone(),
            size: t.size(),
        });
    }
    let previous = referenced_images(dir);
    let images = dir.join(IMAGE_DIR);
    std::fs::create_dir_all(&images).map_err(|e| SaveError::Io(images.clone(), e))?;
    let linked =
        std::fs::symlink_metadata(&images).map_err(|e| SaveError::Io(images.clone(), e))?;
    if !linked.is_dir() {
        return Err(SaveError::ImageDirLink(images));
    }
    let mut written = Vec::new();
    let mut image = |stem: String, png: Vec<u8>| -> Result<String, SaveError> {
        let name = format!("{stem}-{:016x}.png", fnv1a(&png));
        let path = images.join(&name);
        // Written even when present, in case the file was changed since.
        write_atomic(&path, &png).map_err(|e| SaveError::Io(path.clone(), e))?;
        let rel = format!("{IMAGE_DIR}/{name}");
        written.push(name);
        Ok(rel)
    };
    let mut textures = Vec::new();
    for (t, texture) in project.textures().iter().enumerate() {
        let content = match texture.content() {
            TextureContent::Painted(variants) => {
                let mut out = Vec::new();
                for (v, variant) in variants.iter().enumerate() {
                    let mut layers = Vec::new();
                    for (l, layer) in variant.layers().iter().enumerate() {
                        let content = match &layer.content {
                            LayerContent::Vector(shapes) => {
                                LayerContentFile::Vector(shapes.iter().map(shape_file).collect())
                            }
                            LayerContent::Raster(raster) => {
                                let png = encode_raster(raster)
                                    .map_err(|e| SaveError::Png(dir.join(IMAGE_DIR), e))?;
                                LayerContentFile::Raster(image(format!("{t}-{v}-{l}"), png)?)
                            }
                        };
                        layers.push(LayerFile {
                            name: layer.name.clone(),
                            visible: layer.visible,
                            locked: layer.locked,
                            opacity: layer.opacity,
                            modifiers: layer.modifiers.iter().map(modifier_file).collect(),
                            content,
                        });
                    }
                    out.push(VariantFile {
                        name: variant.name.clone(),
                        layers,
                    });
                }
                ContentFile::Painted(out)
            }
            TextureContent::External { preview } => {
                let png =
                    encode_rgba(preview).map_err(|e| SaveError::Png(dir.join(IMAGE_DIR), e))?;
                let rel = image(format!("{t}-preview"), png)?;
                ContentFile::External { preview: rel }
            }
        };
        textures.push(TextureFile {
            name: texture.name.clone(),
            slot: texture.slot.0.clone(),
            size: texture.size().to_array(),
            content,
        });
    }
    let model = project.model();
    let file = ProjectFile {
        format: FORMAT,
        target: TargetFile {
            id: project.target().id.clone(),
            options: project.target().options.clone(),
        },
        palette: project
            .palette()
            .colors()
            .iter()
            .map(|c| c.to_array())
            .collect(),
        nodes: model.ids().map(|id| node_file(model, id)).collect(),
        materials: project.materials().iter().map(material_file).collect(),
        textures,
        pose: project
            .pose()
            .rotations()
            .map(|(id, q)| (id.0, q.to_array()))
            .collect(),
    };
    let text = ron::ser::to_string_pretty(&file, ron::ser::PrettyConfig::new().depth_limit(4))?;
    Ok(Staged {
        text,
        images: written,
        previous,
    })
}

/// Whether `name` has the form save gives images, such as
/// `3-0-2-0123456789abcdef.png` or `4-preview-0123456789abcdef.png`.
fn is_saved_image_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".png") else {
        return false;
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let hash = |s: &str| s.len() == 16 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    match stem.split('-').collect::<Vec<_>>()[..] {
        [t, "preview", h] => digits(t) && hash(h),
        [t, v, l, h] => digits(t) && digits(v) && digits(l) && hash(h),
        _ => false,
    }
}

/// Names in the image directory of the images the project file in `dir`
/// refers to that a save may later remove, or none if there is no readable
/// project file.
fn referenced_images(dir: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(dir.join(PROJECT_FILE)) else {
        return Vec::new();
    };
    let Ok(file) = ron::from_str::<ProjectFile>(&text) else {
        return Vec::new();
    };
    file.textures
        .iter()
        .flat_map(|t| match &t.content {
            ContentFile::Painted(variants) => variants
                .iter()
                .flat_map(|v| &v.layers)
                .filter_map(|l| match &l.content {
                    LayerContentFile::Raster(rel) => Some(rel.clone()),
                    LayerContentFile::Vector(_) => None,
                })
                .collect(),
            ContentFile::External { preview } => vec![preview.clone()],
        })
        // Only images saves write: their names, directly in the image
        // directory. Such a name can't hold a path separator.
        .filter_map(|rel| {
            let name = rel.strip_prefix(IMAGE_DIR)?.strip_prefix('/')?;
            is_saved_image_name(name).then(|| name.to_owned())
        })
        .collect()
}

impl Staged {
    /// Replaces the project file, then removes the images only the previous
    /// project file used. Other files are left alone, and failing to remove
    /// one doesn't fail the save.
    fn commit(self, dir: &Path) -> Result<(), SaveError> {
        let manifest = dir.join(PROJECT_FILE);
        write_atomic(&manifest, self.text.as_bytes()).map_err(|e| SaveError::Io(manifest, e))?;
        // Old images go only once the new project file is known to be on
        // disk; otherwise a power loss could bring back the old project
        // file without them.
        if std::fs::File::open(dir).and_then(|d| d.sync_all()).is_err() {
            return Ok(());
        }
        // Staging checked that the image directory is a real directory, and
        // removing a link removes the link itself, never its target.
        let images = dir.join(IMAGE_DIR);
        for old in self.previous.iter().filter(|n| !self.images.contains(n)) {
            let _ = std::fs::remove_file(images.join(old));
        }
        Ok(())
    }
}

/// 64-bit FNV-1a, a stable hash for naming files by their contents.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// Reads the project in directory `dir`.
pub fn load(dir: &Path) -> Result<Project, LoadError> {
    let path = dir.join(PROJECT_FILE);
    let text = std::fs::read_to_string(&path).map_err(|e| LoadError::Io(path.clone(), e))?;
    // Read the version alone first, so a newer file reports its version
    // rather than whatever field changed.
    let version: Version =
        ron::from_str(&text).map_err(|e| LoadError::Ron(path.clone(), Box::new(e)))?;
    if version.format != FORMAT {
        return Err(LoadError::Format(version.format));
    }
    let file: ProjectFile =
        ron::from_str(&text).map_err(|e| LoadError::Ron(path.clone(), Box::new(e)))?;

    let mut nodes = file.nodes.into_iter().enumerate();
    let (_, root) = nodes.next().ok_or(LoadError::NoRoot)?;
    if let Some(parent) = root.parent {
        return Err(LoadError::Parent { node: 0, parent });
    }
    let mut model = Model::new(root.name.clone(), transform(&root));
    model.node_mut(NodeId::ROOT).mesh = root.mesh.map(mesh);
    for (i, n) in nodes {
        let parent = match n.parent {
            Some(p) if (p as usize) < i => NodeId(p),
            Some(parent) => return Err(LoadError::Parent { node: i, parent }),
            None => return Err(LoadError::SecondRoot(i)),
        };
        let id = model.add_child(parent, n.name.clone(), transform(&n));
        model.node_mut(id).mesh = n.mesh.map(mesh);
    }

    let palette = Palette::from_colors(
        file.palette
            .iter()
            .map(|&[r, g, b, a]| Rgba8::new(r, g, b, a)),
    )
    .map_err(LoadError::Palette)?;
    let mut textures = Vec::new();
    for t in file.textures {
        let size = UVec2::from_array(t.size);
        if size.max_element() > MAX_TEXTURE_SIDE {
            return Err(LoadError::TextureTooLarge {
                texture: t.name,
                size,
            });
        }
        let slot = Slot(t.slot);
        let texture = match t.content {
            ContentFile::Painted(files) => {
                let mut variants = Vec::new();
                for file in files {
                    let mut variant = Variant::new(file.name.clone(), size);
                    for (i, l) in file.layers.into_iter().enumerate() {
                        let mut layer = match l.content {
                            LayerContentFile::Vector(shapes) => {
                                let mut layer = Layer::new(l.name);
                                for (k, s) in shapes.into_iter().enumerate() {
                                    let s = shape(s).map_err(|source| LoadError::Shape {
                                        texture: t.name.clone(),
                                        variant: file.name.clone(),
                                        source,
                                    })?;
                                    layer.insert_shape(k, s);
                                }
                                layer
                            }
                            LayerContentFile::Raster(rel) => {
                                Layer::raster(l.name, read_raster(&image_path(dir, &rel)?, size)?)
                            }
                        };
                        layer.visible = l.visible;
                        layer.locked = l.locked;
                        layer.opacity = l.opacity;
                        layer.modifiers =
                            l.modifiers.into_iter().map(|m| modifier(m, size)).collect();
                        variant.insert_layer(i, layer);
                    }
                    variants.push(variant);
                }
                Texture::painted(t.name.clone(), slot, size, variants)?
            }
            ContentFile::External { preview } => {
                let image = read_rgba(&image_path(dir, &preview)?, size)?;
                Texture::external(t.name, slot, image)
            }
        };
        textures.push(texture);
    }
    let mut pose = Pose::default();
    for (node, q) in file.pose {
        pose.set_rotation(NodeId(node), Quat::from_array(q));
    }
    Ok(Project::new(
        ExportTarget {
            id: file.target.id,
            options: file.target.options,
        },
        palette,
        model,
        textures,
        file.materials.into_iter().map(material).collect(),
        pose,
    )?)
}

fn node_file(model: &Model, id: NodeId) -> NodeFile {
    let n = model.node(id);
    NodeFile {
        name: n.name.clone(),
        parent: n.parent().map(|p| p.0),
        translation: n.transform.translation.to_array(),
        rotation: n.transform.rotation.to_array(),
        scale: n.transform.scale.to_array(),
        mesh: n.mesh.as_ref().map(|m| MeshFile {
            vertices: m
                .vertices
                .iter()
                .map(|v| VertexFile {
                    position: v.position.to_array(),
                    normal: v.normal.to_array(),
                    follows: v.follows.map(|f| f.0),
                })
                .collect(),
            triangles: m
                .triangles
                .iter()
                .map(|t| TriangleFile {
                    material: t.material.0,
                    corners: t.corners.map(|c| (c.vertex, c.uv.to_array())),
                })
                .collect(),
        }),
    }
}

fn transform(n: &NodeFile) -> Transform {
    Transform {
        translation: Vec3::from_array(n.translation),
        rotation: Quat::from_array(n.rotation),
        scale: Vec3::from_array(n.scale),
    }
}

fn mesh(m: MeshFile) -> Mesh {
    Mesh {
        vertices: m
            .vertices
            .into_iter()
            .map(|v| Vertex {
                position: Vec3::from_array(v.position),
                normal: Vec3::from_array(v.normal),
                follows: v.follows.map(NodeId),
            })
            .collect(),
        triangles: m
            .triangles
            .into_iter()
            .map(|t| Triangle {
                corners: t.corners.map(|(vertex, uv)| Corner {
                    vertex,
                    uv: Vec2::from_array(uv),
                }),
                material: crate::MaterialId(t.material),
            })
            .collect(),
    }
}

fn modifier_file(m: &Modifier) -> ModifierFile {
    ModifierFile {
        enabled: m.enabled,
        kind: match m.kind {
            ModifierKind::Mirror(m) => ModifierKindFile::Mirror {
                axes: match m.axes {
                    Axes::X => AxesFile::X,
                    Axes::Y => AxesFile::Y,
                    Axes::Both => AxesFile::Both,
                },
                at: m.at.to_vec2().to_array(),
            },
            ModifierKind::Radial(r) => ModifierKindFile::Radial {
                turns: match r.turns() {
                    Turns::Half => TurnsFile::Half,
                    Turns::Quarter => TurnsFile::Quarter,
                },
                center: r.center().to_vec2().to_array(),
            },
        },
    }
}

/// A modifier from its file form, with lines and centers moved to the
/// nearest place they can be on a texture of `size`.
fn modifier(m: ModifierFile, size: UVec2) -> Modifier {
    let kind = match m.kind {
        ModifierKindFile::Mirror { axes, at } => ModifierKind::Mirror(Mirror {
            axes: match axes {
                AxesFile::X => Axes::X,
                AxesFile::Y => Axes::Y,
                AxesFile::Both => Axes::Both,
            },
            at: HalfPoint::nearest(Vec2::from_array(at), size),
        }),
        ModifierKindFile::Radial { turns, center } => {
            let turns = match turns {
                TurnsFile::Half => Turns::Half,
                TurnsFile::Quarter => Turns::Quarter,
            };
            ModifierKind::Radial(Radial::nearest(turns, Vec2::from_array(center), size))
        }
    };
    Modifier {
        enabled: m.enabled,
        kind,
    }
}

fn material_file(m: &Material) -> MaterialFile {
    MaterialFile {
        texture: m.texture.0,
        wrap: m.wrap.map(|w| match w {
            Wrap::Clamp => WrapFile::Clamp,
            Wrap::Repeat => WrapFile::Repeat,
            Wrap::Mirror => WrapFile::Mirror,
        }),
        alpha: match m.alpha {
            AlphaMode::Opaque => AlphaFile::Opaque,
            AlphaMode::Cutout => AlphaFile::Cutout,
        },
        double_sided: m.double_sided,
        tint: m.tint.to_array(),
    }
}

fn material(m: MaterialFile) -> Material {
    let [r, g, b, a] = m.tint;
    Material {
        texture: TextureId(m.texture),
        wrap: m.wrap.map(|w| match w {
            WrapFile::Clamp => Wrap::Clamp,
            WrapFile::Repeat => Wrap::Repeat,
            WrapFile::Mirror => Wrap::Mirror,
        }),
        alpha: match m.alpha {
            AlphaFile::Opaque => AlphaMode::Opaque,
            AlphaFile::Cutout => AlphaMode::Cutout,
        },
        double_sided: m.double_sided,
        tint: Rgba8::new(r, g, b, a),
    }
}

fn shape_file(s: &Shape) -> ShapeFile {
    ShapeFile {
        color: s.color().0,
        style: match s.style() {
            Style::Fill => StyleFile::Fill,
            Style::Stroke { width, closed } => StyleFile::Stroke { width, closed },
        },
        anchors: s
            .anchors()
            .iter()
            .map(|a| {
                [
                    a.point.to_array(),
                    a.handle_in.to_array(),
                    a.handle_out.to_array(),
                ]
            })
            .collect(),
    }
}

fn shape(s: ShapeFile) -> Result<Shape, crate::ShapeError> {
    let style = match s.style {
        StyleFile::Fill => Style::Fill,
        StyleFile::Stroke { width, closed } => Style::Stroke { width, closed },
    };
    let anchors = s
        .anchors
        .into_iter()
        .map(|[p, i, o]| Anchor {
            point: Vec2::from_array(p),
            handle_in: Vec2::from_array(i),
            handle_out: Vec2::from_array(o),
        })
        .collect();
    Shape::new(anchors, style, PaletteIndex(s.color))
}

/// Whether `rel` is a relative path that names no parent directory.
fn relative_inside(rel: &str) -> bool {
    Path::new(rel)
        .components()
        .all(|c| matches!(c, Component::Normal(_)))
}

/// Resolves an image path from the project file, which must stay inside
/// the project directory, links included; returns its resolved path.
fn image_path(dir: &Path, rel: &str) -> Result<PathBuf, LoadError> {
    if !relative_inside(rel) {
        return Err(LoadError::ImagePath(rel.to_owned()));
    }
    let resolve = |p: &Path| p.canonicalize().map_err(|e| LoadError::Io(p.to_owned(), e));
    let path = resolve(&dir.join(rel))?;
    if path.starts_with(resolve(dir)?) {
        // The resolved path, so the file checked is the file opened.
        Ok(path)
    } else {
        Err(LoadError::ImagePath(rel.to_owned()))
    }
}

/// Writes `stamp` to a new file at `path`, all at once, never replacing a
/// file already there.
pub fn save_stamp(stamp: &Stamp, path: &Path) -> Result<(), StampFileError> {
    let file = StampFile {
        format: STAMP_FORMAT,
        colors: stamp.colors().iter().map(|c| c.to_array()).collect(),
        shapes: stamp.shapes().iter().map(shape_file).collect(),
    };
    let text = ron::ser::to_string_pretty(&file, ron::ser::PrettyConfig::new().depth_limit(2))?;
    write_file(path, text.as_bytes(), false).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => StampFileError::Exists(path.to_owned()),
        _ => StampFileError::Io(path.to_owned(), e),
    })
}

/// Reads the stamp file at `path`.
pub fn load_stamp(path: &Path) -> Result<Stamp, StampFileError> {
    let text = std::fs::read_to_string(path).map_err(|e| StampFileError::Io(path.to_owned(), e))?;
    let ron_error = |e| StampFileError::Ron(path.to_owned(), Box::new(e));
    let version: Version = ron::from_str(&text).map_err(ron_error)?;
    if version.format != STAMP_FORMAT {
        return Err(StampFileError::Format(path.to_owned(), version.format));
    }
    let file: StampFile = ron::from_str(&text).map_err(ron_error)?;
    let shapes = file
        .shapes
        .into_iter()
        .map(shape)
        .collect::<Result<_, _>>()
        .map_err(|e| StampFileError::Shape(path.to_owned(), e))?;
    let colors = file
        .colors
        .into_iter()
        .map(|[r, g, b, a]| Rgba8::new(r, g, b, a))
        .collect();
    Stamp::new(colors, shapes).map_err(|e| StampFileError::Stamp(path.to_owned(), e))
}

/// Replaces the file at `path` with `bytes` all at once: they are written
/// to a new, uniquely named file beside it (never one already there, such as
/// a planted link), synced, and renamed over it, and the directory is
/// synced where possible so the rename survives a power loss. The file keeps
/// the permissions of the one it replaces, and a new file gets the usual
/// ones for the umask. A failed write removes the new file and leaves the
/// old one.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_file(path, bytes, true)
}

/// [`write_atomic`], or with `replace` false, the same into a new file
/// only, failing with [`std::io::ErrorKind::AlreadyExists`] if `path`
/// exists.
fn write_file(path: &Path, bytes: &[u8], replace: bool) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let mut tmp = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o666))
        .tempfile_in(dir)?;
    // Set explicitly, as creating a file applies the umask.
    if let Ok(old) = std::fs::metadata(path) {
        tmp.as_file().set_permissions(old.permissions())?;
    }
    std::io::Write::write_all(&mut tmp, bytes)?;
    tmp.as_file().sync_all()?;
    if replace {
        tmp.persist(path).map_err(|e| e.error)?;
    } else {
        tmp.persist_noclobber(path).map_err(|e| e.error)?;
    }
    // The file is in place; a directory that can't be synced only loses
    // durability, so it isn't an error.
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

fn encode_raster(raster: &Raster) -> Result<Vec<u8>, png::EncodingError> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, raster.size().x, raster.size().y);
    enc.set_color(png::ColorType::GrayscaleAlpha);
    enc.set_depth(png::BitDepth::Eight);
    let pixels: Vec<u8> = raster
        .pixels()
        .iter()
        .flat_map(|p| p.map_or([0, 0], |i| [i.0, 255]))
        .collect();
    enc.write_header()?.write_image_data(&pixels)?;
    Ok(out)
}

fn encode_rgba(image: &RgbaImage) -> Result<Vec<u8>, png::EncodingError> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, image.size.x, image.size.y);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let pixels: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
    enc.write_header()?.write_image_data(&pixels)?;
    Ok(out)
}

/// Decodes a PNG with no color conversion, returning its size, color type,
/// and pixel bytes.
fn read_png(
    path: &Path,
    size: UVec2,
) -> Result<(png::ColorType, png::BitDepth, Vec<u8>), LoadError> {
    let file = std::fs::File::open(path).map_err(|e| LoadError::Io(path.to_owned(), e))?;
    let png_err = |e| LoadError::Png(path.to_owned(), e);
    let mut dec = png::Decoder::new(BufReader::new(file));
    dec.set_transformations(png::Transformations::IDENTITY);
    let mut reader = dec.read_info().map_err(png_err)?;
    // Checked before decoding, so a PNG that doesn't match its texture can't
    // make load allocate for its size; texture sizes themselves are capped
    // at MAX_TEXTURE_SIDE.
    let actual = UVec2::new(reader.info().width, reader.info().height);
    if actual != size {
        return Err(LoadError::Image(
            path.to_owned(),
            format!("is {actual} but the texture is {size}"),
        ));
    }
    let mut buf = vec![
        0;
        reader.output_buffer_size().ok_or_else(|| LoadError::Image(
            path.to_owned(),
            "is too large".into()
        ))?
    ];
    let info = reader.next_frame(&mut buf).map_err(png_err)?;
    buf.truncate(info.buffer_size());
    Ok((info.color_type, info.bit_depth, buf))
}

fn read_raster(path: &Path, size: UVec2) -> Result<Raster, LoadError> {
    let (color, depth, pixels) = read_png(path, size)?;
    if (color, depth) != (png::ColorType::GrayscaleAlpha, png::BitDepth::Eight) {
        return Err(LoadError::Image(
            path.to_owned(),
            "must be an 8-bit grayscale-and-alpha PNG".into(),
        ));
    }
    let (pixels, _) = pixels.as_chunks::<2>();
    let pixels = pixels
        .iter()
        .map(|&[index, alpha]| match alpha {
            0 => Ok(None),
            255 => Ok(Some(PaletteIndex(index))),
            _ => Err(LoadError::Image(
                path.to_owned(),
                format!("has alpha {alpha}; raster layer pixels are set (255) or not (0)"),
            )),
        })
        .collect::<Result<_, _>>()?;
    Raster::new(size, pixels).map_err(|e| LoadError::Image(path.to_owned(), e.to_string()))
}

fn read_rgba(path: &Path, size: UVec2) -> Result<RgbaImage, LoadError> {
    let (color, depth, pixels) = read_png(path, size)?;
    if (color, depth) != (png::ColorType::Rgba, png::BitDepth::Eight) {
        return Err(LoadError::Image(
            path.to_owned(),
            "must be an 8-bit RGBA PNG".into(),
        ));
    }
    let (pixels, _) = pixels.as_chunks::<4>();
    let pixels = pixels
        .iter()
        .map(|&[r, g, b, a]| Rgba8::new(r, g, b, a))
        .collect();
    Ok(RgbaImage { size, pixels })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{IndexedImage, VariantRef, shape};

    fn sample() -> Project {
        let palette =
            Palette::from_colors([Rgba8::new(1, 2, 3, 255), Rgba8::new(200, 100, 0, 128)]).unwrap();
        let mut model = Model::new("root", Transform::from_translation(Vec3::X));
        let child = model.add_child(NodeId::ROOT, "head", Transform::default());
        model.node_mut(child).mesh = Some(Mesh {
            vertices: vec![
                Vertex {
                    position: Vec3::ZERO,
                    normal: Vec3::Y,
                    follows: Some(NodeId::ROOT),
                },
                Vertex {
                    position: Vec3::X,
                    normal: Vec3::Y,
                    follows: None,
                },
                Vertex {
                    position: Vec3::Z,
                    normal: Vec3::Y,
                    follows: None,
                },
            ],
            triangles: vec![Triangle {
                corners: [0, 1, 2].map(|vertex| Corner {
                    vertex,
                    uv: Vec2::new(0.25, 0.5),
                }),
                material: crate::MaterialId(0),
            }],
        });
        let underlay = IndexedImage::new(
            UVec2::new(4, 2),
            [0, 1, 1, 0, 1, 0, 0, 1].map(PaletteIndex).to_vec(),
        )
        .unwrap();
        let painted = Texture::painted(
            "eyes",
            Slot("eyes".into()),
            UVec2::new(4, 2),
            [
                Variant::imported("1", underlay.clone()),
                Variant::imported("2", underlay),
            ],
        )
        .unwrap();
        let mut preview = RgbaImage::filled(UVec2::new(2, 2), Rgba8::new(9, 8, 7, 6));
        preview.pixels[3] = Rgba8::new(1, 1, 1, 1);
        let external = Texture::external("shirt", Slot("clothing".into()), preview);
        let mut pose = Pose::default();
        pose.set_rotation(child, Quat::from_rotation_z(0.5));
        let mut target = ExportTarget::new("test");
        target.options.insert("model".into(), "cat_1".into());
        let mut p = Project::new(
            target,
            palette,
            model,
            vec![painted, external],
            vec![Material {
                texture: TextureId(0),
                wrap: [Wrap::Mirror, Wrap::Clamp],
                alpha: AlphaMode::Cutout,
                double_sided: true,
                tint: Rgba8::new(255, 250, 240, 255),
            }],
            pose,
        )
        .unwrap();
        let r = VariantRef {
            texture: TextureId(0),
            variant: 1,
        };
        let stroke = Style::Stroke {
            width: 1.5,
            closed: true,
        };
        let ring = Shape::new(
            shape::ellipse(Vec2::splat(2.0), Vec2::ONE),
            stroke,
            PaletteIndex(1),
        );
        let square = Shape::new(
            shape::rectangle(Vec2::ZERO, Vec2::ONE),
            Style::Fill,
            PaletteIndex(0),
        );
        p.edit_variant(r, |v| {
            let mut low = Layer::new("low");
            low.insert_shape(0, square.unwrap());
            let mut high = Layer::new("high");
            high.opacity = 0.5;
            high.locked = true;
            high.insert_shape(0, ring.unwrap());
            high.modifiers = vec![
                Modifier {
                    enabled: false,
                    ..Modifier::new(ModifierKind::Mirror(Mirror {
                        axes: Axes::Both,
                        at: HalfPoint { x: 3, y: 2 },
                    }))
                },
                Modifier::new(ModifierKind::Radial(Radial::nearest(
                    Turns::Quarter,
                    Vec2::new(1.5, 0.5),
                    UVec2::new(4, 2),
                ))),
            ];
            // Set and unset pixels, over the base layer.
            let mut dots = Raster::clear(UVec2::new(4, 2));
            dots.draw_line(Vec2::ZERO, Vec2::new(3.5, 0.5), 1.0, Some(PaletteIndex(1)));
            let mut dots = Layer::raster("dots", dots);
            dots.visible = false;
            dots.modifiers = vec![Modifier::new(ModifierKind::Radial(Radial::nearest(
                Turns::Half,
                Vec2::new(2.0, 0.5),
                UVec2::new(4, 2),
            )))];
            v.insert_layer(1, low);
            v.insert_layer(2, high);
            v.insert_layer(0, dots);
        })
        .unwrap();
        p
    }

    #[test]
    fn save_then_load_gives_the_same_project() {
        let dir = tempfile::tempdir().unwrap();
        let p = sample();
        save(&p, dir.path()).unwrap();
        assert_eq!(load(dir.path()).unwrap(), p);
        // Saving again over the same directory works too.
        save(&p, dir.path()).unwrap();
        assert_eq!(load(dir.path()).unwrap(), p);
    }

    fn images(dir: &Path) -> Vec<PathBuf> {
        let mut out: Vec<_> = std::fs::read_dir(dir.join(IMAGE_DIR))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        out.sort();
        out
    }

    #[test]
    fn saving_keeps_the_previous_images_until_the_project_file_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = sample();
        save(&p, dir.path()).unwrap();
        let first = images(dir.path());
        assert_eq!(first.len(), 4);

        // Merging colors changes every raster layer. Until the project file is
        // replaced, the old one still loads with its own images.
        p.merge_palette_color(PaletteIndex(1), PaletteIndex(0))
            .unwrap();
        let staged = stage(&p, dir.path()).unwrap();
        assert_eq!(load(dir.path()).unwrap(), sample());

        // Once it is replaced, the old project's images go, but files the
        // project never referred to stay.
        let reference = dir.path().join(IMAGE_DIR).join("reference.png");
        std::fs::write(&reference, b"not ours").unwrap();
        staged.commit(dir.path()).unwrap();
        assert_eq!(load(dir.path()).unwrap(), p);
        assert!(reference.is_file());
        std::fs::remove_file(&reference).unwrap();

        // A project file naming other files as images can't get them removed.
        let manifest = dir.path().join(PROJECT_FILE);
        let text = std::fs::read_to_string(&manifest).unwrap();
        let first_image = images(dir.path())[0]
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        std::fs::write(
            &manifest,
            text.replace(&format!("textures/{first_image}"), "project.ron"),
        )
        .unwrap();
        save(&p, dir.path()).unwrap();
        assert_eq!(load(dir.path()).unwrap(), p);
        let second = images(dir.path());
        assert_eq!(second.len(), 4);
        assert!(
            second
                .iter()
                .all(|f| !first.contains(f) || f.to_string_lossy().contains("preview"))
        );
    }

    #[test]
    fn writes_keep_the_replaced_file_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shared.c");
        // Group-writable, which a typical umask (022) would remove.
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o664)).unwrap();
        write_atomic(&path, b"new").unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o664
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
    }

    #[test]
    fn saving_through_a_linked_path_keeps_its_images() {
        let base = tempfile::tempdir().unwrap();
        let real = base.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = base.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let p = sample();
        save(&p, &link).unwrap();
        save(&p, &link).unwrap();
        assert_eq!(load(&link).unwrap(), p);
    }

    #[test]
    fn cleanup_never_removes_through_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = sample();
        save(&p, dir.path()).unwrap();
        // A link with a saved image's name, pointing at the project file,
        // that the project file names.
        let name = "0-0-0-0123456789abcdef.png";
        std::os::unix::fs::symlink("../project.ron", dir.path().join(IMAGE_DIR).join(name))
            .unwrap();
        let manifest = dir.path().join(PROJECT_FILE);
        let text = std::fs::read_to_string(&manifest).unwrap();
        let first = images(dir.path())
            .into_iter()
            .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
            .find(|f| f.starts_with("0-0-0-") && f != name)
            .unwrap();
        std::fs::write(&manifest, text.replace(&first, name)).unwrap();
        p.merge_palette_color(PaletteIndex(1), PaletteIndex(0))
            .unwrap();
        save(&p, dir.path()).unwrap();
        assert_eq!(load(dir.path()).unwrap(), p);
    }

    #[test]
    fn save_refuses_textures_load_would_refuse() {
        let size = UVec2::new(MAX_TEXTURE_SIDE + 1, 1);
        let wide = IndexedImage::new(size, vec![PaletteIndex(0); size.x as usize]).unwrap();
        let texture = Texture::painted(
            "wide",
            Slot("body".into()),
            size,
            [Variant::imported("1", wide)],
        )
        .unwrap();
        let mut textures = sample().textures().to_vec();
        textures.push(texture);
        let p = Project::new(
            ExportTarget::new("test"),
            sample().palette().clone(),
            Model::new("root", Transform::default()),
            textures,
            Vec::new(),
            Pose::default(),
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            save(&p, dir.path()),
            Err(SaveError::TextureTooLarge { .. })
        ));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn save_refuses_a_linked_image_directory() {
        let dir = tempfile::tempdir().unwrap();
        let shared = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(shared.path(), dir.path().join(IMAGE_DIR)).unwrap();
        assert!(matches!(
            save(&sample(), dir.path()),
            Err(SaveError::ImageDirLink(_))
        ));
        assert_eq!(std::fs::read_dir(shared.path()).unwrap().count(), 0);
    }

    #[test]
    fn only_saved_image_names_are_cleaned_up() {
        assert!(is_saved_image_name("3-0-2-0123456789abcdef.png"));
        assert!(is_saved_image_name("12-preview-0123456789abcdef.png"));
        for name in [
            "reference.png",
            "3-0.png",
            "3-0-0123456789abcdef.png",
            "3-0-2-0123456789ABCDEF.png",
            "3-x-2-0123456789abcdef.png",
            "3-0-2-0123456789abcdef.ron",
        ] {
            assert!(!is_saved_image_name(name), "{name}");
        }
    }

    #[test]
    fn load_refuses_images_linked_from_outside() {
        let dir = tempfile::tempdir().unwrap();
        save(&sample(), dir.path()).unwrap();
        let inside = images(dir.path()).remove(0);
        let outside = tempfile::tempdir().unwrap();
        let elsewhere = outside.path().join("elsewhere.png");
        std::fs::rename(&inside, &elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &inside).unwrap();
        assert!(matches!(load(dir.path()), Err(LoadError::ImagePath(_))));
    }

    #[test]
    fn load_refuses_other_formats_and_escaping_paths() {
        let dir = tempfile::tempdir().unwrap();
        save(&sample(), dir.path()).unwrap();
        let path = dir.path().join(PROJECT_FILE);
        let text = std::fs::read_to_string(&path).unwrap();

        std::fs::write(&path, text.replace("format: 1", "format: 2")).unwrap();
        assert!(matches!(load(dir.path()), Err(LoadError::Format(2))));

        let escaping = text.replacen("\"textures/", "\"../", 1);
        assert_ne!(escaping, text);
        std::fs::write(&path, escaping).unwrap();
        assert!(matches!(load(dir.path()), Err(LoadError::ImagePath(p)) if p.starts_with("../")));

        let huge = text.replacen("size: (4, 2)", "size: (60000, 60000)", 1);
        assert_ne!(huge, text);
        std::fs::write(&path, huge).unwrap();
        assert!(matches!(
            load(dir.path()),
            Err(LoadError::TextureTooLarge { .. })
        ));

        // A raster pixel past the palette's end fails as a project would.
        let one_color = text.replace("(200, 100, 0, 128),", "");
        assert_ne!(one_color, text);
        std::fs::write(&path, one_color).unwrap();
        assert!(matches!(
            load(dir.path()),
            Err(LoadError::Project(
                crate::ProjectError::MissingPaletteEntry { index: 1, .. }
            ))
        ));
    }

    fn stamp() -> Stamp {
        let curve = Shape::new(
            vec![
                Anchor::smooth(Vec2::new(1.0, 2.0), Vec2::new(0.5, -0.25)),
                Anchor::corner(Vec2::new(4.0, 3.5)),
            ],
            Style::Stroke {
                width: 1.5,
                closed: false,
            },
            PaletteIndex(1),
        )
        .unwrap();
        let dot = Shape::new(
            vec![Anchor::corner(Vec2::new(-2.0, 0.0))],
            Style::Fill,
            PaletteIndex(0),
        )
        .unwrap();
        Stamp::new(
            vec![Rgba8::new(10, 20, 30, 255), Rgba8::new(200, 100, 0, 128)],
            vec![curve, dot],
        )
        .unwrap()
    }

    #[test]
    fn stamps_save_then_load_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Eye.ron");
        save_stamp(&stamp(), &path).unwrap();
        assert_eq!(load_stamp(&path).unwrap(), stamp());
        // Nothing but the stamp is left behind.
        let files: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn saving_a_stamp_never_replaces_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Eye.ron");
        std::fs::write(&path, "mine").unwrap();
        assert!(matches!(
            save_stamp(&stamp(), &path),
            Err(StampFileError::Exists(p)) if p == path
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "mine");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn loading_a_stamp_checks_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Eye.ron");
        save_stamp(&stamp(), &path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();

        std::fs::write(&path, text.replace("format: 1", "format: 2")).unwrap();
        assert!(matches!(
            load_stamp(&path),
            Err(StampFileError::Format(_, 2))
        ));

        let one_color = text.replace("(10, 20, 30, 255),", "");
        assert_ne!(one_color, text);
        std::fs::write(&path, one_color).unwrap();
        assert!(matches!(
            load_stamp(&path),
            Err(StampFileError::Stamp(
                _,
                StampError::MissingColor { index: 1, len: 1 }
            ))
        ));

        let no_width = text.replace("width: 1.5", "width: 0.0");
        assert_ne!(no_width, text);
        std::fs::write(&path, no_width).unwrap();
        assert!(matches!(
            load_stamp(&path),
            Err(StampFileError::Shape(_, ShapeError::BadWidth))
        ));
    }
}
