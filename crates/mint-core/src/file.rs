//! Saving and loading projects as a directory: `project.ron` holds
//! everything except images, which sit beside it as PNG files.
//!
//! Underlays are 8-bit indexed PNGs whose pixel values are palette indices;
//! their PNG palette is a copy of the project's for viewing and is ignored on
//! load. External texture previews are RGBA PNGs. Loading goes through the
//! same constructors as building a project in code, so a file can't produce
//! a project that breaks their rules.

use std::collections::BTreeMap;
use std::io::BufReader;
use std::path::{Component, Path, PathBuf};

use glam::{Quat, UVec2, Vec2, Vec3};
use serde::{Deserialize, Serialize};

use crate::{
    AlphaMode, Anchor, Corner, ExportTarget, IndexedImage, Layer, Material, Mesh, Model, NodeId,
    Palette, PaletteIndex, Pose, Project, Rgba8, RgbaImage, Shape, Slot, Style, Texture,
    TextureContent, TextureId, Transform, Triangle, Vertex, Wrap,
};

/// The project file inside a project directory.
pub const PROJECT_FILE: &str = "project.ron";
const IMAGE_DIR: &str = "textures";
/// Largest texture side a project file may give, the usual GPU limit.
const MAX_TEXTURE_SIDE: u32 = 8192;
/// Version of the file layout this build reads and writes.
const FORMAT: u32 = 1;

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
    /// An 8-bit indexed PNG.
    underlay: String,
    layers: Vec<LayerFile>,
}

#[derive(Serialize, Deserialize)]
struct LayerFile {
    name: String,
    visible: bool,
    locked: bool,
    opacity: f32,
    shapes: Vec<ShapeFile>,
}

#[derive(Serialize, Deserialize)]
struct ShapeFile {
    color: u8,
    style: StyleFile,
    /// Point, incoming handle, and outgoing handle of each anchor.
    anchors: Vec<[[f32; 2]; 3]>,
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
                    let png = encode_indexed(variant.underlay(), project.palette())
                        .map_err(|e| SaveError::Png(dir.join(IMAGE_DIR), e))?;
                    let rel = image(format!("{t}-{v}"), png)?;
                    out.push(VariantFile {
                        name: variant.name.clone(),
                        underlay: rel,
                        layers: variant.layers().iter().map(layer_file).collect(),
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
/// `3-0-0123456789abcdef.png` or `4-preview-0123456789abcdef.png`.
fn is_saved_image_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".png") else {
        return false;
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    match stem.split('-').collect::<Vec<_>>()[..] {
        [t, v, hash] => {
            digits(t)
                && (digits(v) || v == "preview")
                && hash.len() == 16
                && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        }
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
            ContentFile::Painted(variants) => variants.iter().map(|v| v.underlay.clone()).collect(),
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
            ContentFile::Painted(variants) => {
                let underlays = variants
                    .iter()
                    .map(|v| {
                        Ok((
                            v.name.clone(),
                            read_indexed(&image_path(dir, &v.underlay)?, size)?,
                        ))
                    })
                    .collect::<Result<Vec<_>, LoadError>>()?;
                let mut texture = Texture::painted(t.name.clone(), slot, size, underlays)?;
                for (variant, file) in texture.variants_mut().iter_mut().zip(variants) {
                    for (i, l) in file.layers.into_iter().enumerate() {
                        let layer = layer(l).map_err(|source| LoadError::Shape {
                            texture: t.name.clone(),
                            variant: file.name.clone(),
                            source,
                        })?;
                        variant.insert_layer(i, layer);
                    }
                }
                texture
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

fn layer_file(l: &Layer) -> LayerFile {
    LayerFile {
        name: l.name.clone(),
        visible: l.visible,
        locked: l.locked,
        opacity: l.opacity,
        shapes: l
            .shapes()
            .iter()
            .map(|s| ShapeFile {
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
            })
            .collect(),
    }
}

fn layer(l: LayerFile) -> Result<Layer, crate::ShapeError> {
    let mut layer = Layer::new(l.name);
    layer.visible = l.visible;
    layer.locked = l.locked;
    layer.opacity = l.opacity;
    for (i, s) in l.shapes.into_iter().enumerate() {
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
        layer.insert_shape(i, Shape::new(anchors, style, PaletteIndex(s.color))?);
    }
    Ok(layer)
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

/// Replaces the file at `path` with `bytes` all at once: they are written
/// to a new, uniquely named file beside it (never one already there, such as
/// a planted link), synced, and renamed over it, and the directory is
/// synced where possible so the rename survives a power loss. The file keeps
/// the permissions of the one it replaces, and a new file gets the usual
/// ones for the umask. A failed write removes the new file and leaves the
/// old one.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
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
    tmp.persist(path).map_err(|e| e.error)?;
    // The file is in place; a directory that can't be synced only loses
    // durability, so it isn't an error.
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

fn encode_indexed(image: &IndexedImage, palette: &Palette) -> Result<Vec<u8>, png::EncodingError> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, image.size().x, image.size().y);
    enc.set_color(png::ColorType::Indexed);
    enc.set_depth(png::BitDepth::Eight);
    let mut colors = palette.colors().to_vec();
    if colors.is_empty() {
        // PNG needs a palette entry; an image over an empty palette has no
        // pixels to use it.
        colors.push(Rgba8::new(0, 0, 0, 255));
    }
    enc.set_palette(
        colors
            .iter()
            .flat_map(|c| [c.r, c.g, c.b])
            .collect::<Vec<_>>(),
    );
    enc.set_trns(colors.iter().map(|c| c.a).collect::<Vec<_>>());
    let pixels: Vec<u8> = image.pixels().iter().map(|i| i.0).collect();
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

fn read_indexed(path: &Path, size: UVec2) -> Result<IndexedImage, LoadError> {
    let (color, depth, pixels) = read_png(path, size)?;
    if (color, depth) != (png::ColorType::Indexed, png::BitDepth::Eight) {
        return Err(LoadError::Image(
            path.to_owned(),
            "must be an 8-bit indexed PNG".into(),
        ));
    }
    IndexedImage::new(size, pixels.into_iter().map(PaletteIndex).collect())
        .map_err(|e| LoadError::Image(path.to_owned(), e.to_string()))
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
    use crate::{VariantRef, shape};

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
                ("1".to_owned(), underlay.clone()),
                ("2".to_owned(), underlay),
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
            v.insert_layer(0, low);
            v.insert_layer(1, high);
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
        assert_eq!(first.len(), 3);

        // Merging colors changes every underlay. Until the project file is
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
        assert_eq!(second.len(), 3);
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
        let name = "0-0-0123456789abcdef.png";
        std::os::unix::fs::symlink("../project.ron", dir.path().join(IMAGE_DIR).join(name))
            .unwrap();
        let manifest = dir.path().join(PROJECT_FILE);
        let text = std::fs::read_to_string(&manifest).unwrap();
        let first = images(dir.path())
            .into_iter()
            .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
            .find(|f| f.starts_with("0-0-") && f != name)
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
        let texture =
            Texture::painted("wide", Slot("body".into()), size, [("1".to_owned(), wide)]).unwrap();
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
        assert!(is_saved_image_name("3-0-0123456789abcdef.png"));
        assert!(is_saved_image_name("12-preview-0123456789abcdef.png"));
        for name in [
            "reference.png",
            "3-0.png",
            "3-0-0123456789ABCDEF.png",
            "3-x-0123456789abcdef.png",
            "3-0-0123456789abcdef.ron",
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

        // An underlay pixel past the palette's end fails as a project would.
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
}
