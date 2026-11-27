//! The model file: a copy of a project's model in glTF binary (`.glb`) for
//! editing in another program, such as Blender, and reading it back.
//!
//! The file holds one mesh skinned to a skeleton of the model's nodes, posed
//! in the project's display pose, which becomes the rest pose in the other
//! program. Each vertex is weighted fully to the node it follows. Vertices
//! that share their node, position, normal, and UV are one vertex, and
//! Blender's Merge Vertices joins those that differ only in UV, so a seam
//! whose sides share a normal stays closed while editing. Materials are
//! named after the textures they draw with and show PNG images of those
//! textures, written beside the file.
//!
//! Reading takes the meshes back into the project's own node tree, which a
//! file can't change: each triangle belongs to the deepest node among its
//! corners', and corners that follow another node become seam vertices. Only
//! the file's default scene is read, and of it only meshes; images and
//! animations are ignored. The file is untrusted: anything in it that can't
//! be read safely is refused, and errors about one object name it.
//!
//! The path of a model file comes from a project file, which may be shared,
//! so writing replaces only a `.glb` model file and PNGs at its image paths,
//! never a file of another kind.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use glam::{Mat3, Mat4, Vec2, Vec3};
use gltf::accessor::{DataType, Dimensions};
use serde_json::{Value, json};

use crate::file::write_atomic;
use crate::{
    AlphaMode, Corner, MaterialId, Mesh, Model, NodeId, Project, Rgba8, TextureContent, TextureId,
    Triangle, VariantRef, Vertex, Wrap,
};

/// The extension of a model file.
pub const EXTENSION: &str = "glb";

/// The absolute path of a model file: valid UTF-8, so a project file can
/// store it, and ending in `.glb`. Where its directory exists, the
/// directory's links are resolved, so it compares with a project
/// directory's resolved path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelFilePath(PathBuf);

impl ModelFilePath {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, ModelFileError> {
        let path: PathBuf = path.into();
        let bad = |problem| ModelFileError::BadPath {
            path: path.clone(),
            problem,
        };
        if !path.is_absolute() {
            return Err(bad("it isn't absolute"));
        }
        if path.to_str().is_none() {
            return Err(bad("it isn't valid UTF-8"));
        }
        if path.extension().and_then(|e| e.to_str()) != Some(EXTENSION) {
            return Err(bad("it doesn't end in .glb"));
        }
        let name = path.file_name().ok_or_else(|| bad("it has no file name"))?;
        let resolved = match path.parent().map(Path::canonicalize) {
            Some(Ok(dir)) => dir.join(name),
            _ => path.clone(),
        };
        if resolved.to_str().is_none() {
            return Err(bad("its directory isn't valid UTF-8"));
        }
        Ok(Self(resolved))
    }

    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

/// How an export target's model units map to a model file's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Units {
    /// Size in meters, the file's unit, of one model unit.
    pub meters_per_unit: f32,
    /// Length of the target's normals in the model; the file's are unit
    /// length.
    pub normal_length: f32,
}

#[derive(Debug, thiserror::Error)]
pub enum ModelFileError {
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("{0}: {1}")]
    Png(PathBuf, png::EncodingError),
    #[error("model file path {path}: {problem}")]
    BadPath {
        path: PathBuf,
        problem: &'static str,
    },
    #[error("{0} exists and isn't a glTF binary file; choose another path")]
    NotModelFile(PathBuf),
    #[error(
        "{0} isn't a model file, so its images aren't written beside it; send the model to a model file again"
    )]
    NoModelFile(PathBuf),
    #[error("the model file is over the {MAX_FILE_BYTES}-byte limit")]
    TooLarge,
    #[error("{0} exists and isn't a PNG image; move it, or choose another model file path")]
    NotPng(PathBuf),
    #[error("two nodes are named {0:?}; a model file names each bone after its node")]
    DuplicateNode(String),
    #[error(
        "can't read the model file as glTF: {0}; export it as glTF Binary (.glb) without compression"
    )]
    Gltf(#[from] gltf::Error),
    #[error("the model file is malformed glTF; export it again as glTF Binary (.glb)")]
    Unreadable,
    #[error("the model file's default scene has no meshes, or none with faces")]
    NoMesh,
    #[error(
        "mint-rabbit made a model file it can't read back, which is a bug, so nothing was written: {0}"
    )]
    NotReadable(Box<ModelFileError>),
    #[error("node {node:?} can't go in a model file: {problem}")]
    Unwritable { node: String, problem: &'static str },
    #[error("the project has {0} nodes; a model file holds at most 65536")]
    TooManyNodes(usize),
    #[error("the model file's nodes form a loop rather than a tree")]
    Cycle,
    #[error("the model file keeps its data in a separate file; export it as glTF Binary (.glb)")]
    ExternalData,
    #[error(
        "object {0:?} isn't skinned; give it an Armature modifier and put each vertex in one bone's vertex group, or delete it or leave it out of the export"
    )]
    Unskinned(String),
    #[error("object {object:?} uses bone {bone:?}, which the project has no node for")]
    UnknownBone { object: String, bone: String },
    #[error("object {0:?} has faces that aren't triangles, such as lines or points")]
    NotTriangles(String),
    #[error("object {object:?} has no {attribute}")]
    MissingAttribute {
        object: String,
        attribute: &'static str,
    },
    #[error("object {object:?} has a malformed {what}")]
    Malformed { object: String, what: String },
    #[error("object {0:?} has faces with no material")]
    NoMaterial(String),
    #[error(
        "object {object:?} uses material {material:?}, which isn't one of the project's: {known}"
    )]
    UnknownMaterial {
        object: String,
        material: String,
        known: String,
    },
    #[error(
        "object {object:?} has {count} vertices weighted to no bone or to more than one, such as one at ({x:.3}, {y:.3}, {z:.3}) in glTF coordinates (Y up); weight each vertex to exactly one bone",
        x = .at.x, y = .at.y, z = .at.z
    )]
    VertexBones {
        object: String,
        count: usize,
        /// In the file's coordinates.
        at: Vec3,
    },
}

/// The name of each of `project`'s materials in a model file: the name of
/// its texture, made unique where two meet with a number counting up from
/// the material's index.
fn material_names(project: &Project) -> Vec<String> {
    let mut taken = HashSet::new();
    project
        .materials()
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let base = &project.texture(m.texture).name;
            let mut name = base.clone();
            let mut n = i;
            while taken.contains(&name) {
                name = format!("{base} #{n}");
                n += 1;
            }
            taken.insert(name.clone());
            name
        })
        .collect()
}

/// `name` with every character but ASCII letters, digits, `-`, `_`, and
/// spaces replaced, so it is safe in a file name and holds no dot.
fn file_safe(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Where the image of each of `project`'s textures goes beside the model
/// file at `path`: `<stem>.<index>-<texture>.png`. The last part before
/// `.png` holds no dot, so no two textures or stems share an image.
fn image_paths(project: &Project, path: &ModelFilePath) -> Vec<PathBuf> {
    let path = path.as_path();
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    project
        .textures()
        .iter()
        .enumerate()
        .map(|(i, t)| path.with_file_name(format!("{stem}.{i}-{}.png", file_safe(&t.name))))
        .collect()
}

/// `name` as a relative URI reference: every byte but unreserved characters
/// percent-encoded.
fn uri(name: &str) -> String {
    name.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// Whether `path` is a regular file that starts with `magic`, or `None`
/// when nothing is there or the file is empty, which replacing loses
/// nothing of.
fn starts_with(path: &Path, magic: &[u8]) -> Result<Option<bool>, ModelFileError> {
    use std::io::Read as _;
    let io = |e| ModelFileError::Io(path.to_owned(), e);
    match std::fs::metadata(path) {
        Ok(m) if m.is_file() && m.len() == 0 => return Ok(None),
        Ok(m) if m.is_file() => {}
        Ok(_) => return Ok(Some(false)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io(e)),
    }
    let mut head = vec![0; magic.len()];
    let mut file = std::fs::File::open(path).map_err(io)?;
    match file.read_exact(&mut head) {
        Ok(()) => Ok(Some(head == magic)),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(Some(false)),
        Err(e) => Err(io(e)),
    }
}

/// Fails unless `path` is missing, empty, or starts with `magic`, so writing never
/// replaces a file of another kind, such as one a shared project names.
fn check_replaceable(
    path: &Path,
    magic: &[u8],
    error: fn(PathBuf) -> ModelFileError,
) -> Result<(), ModelFileError> {
    match starts_with(path, magic)? {
        None | Some(true) => Ok(()),
        Some(false) => Err(error(path.to_owned())),
    }
}

const GLB_MAGIC: &[u8] = b"glTF";
/// How far, in model units, a vertex may move through a model file, which
/// holds model-space positions as f32.
const POSITION_TOLERANCE: f32 = 0.01;
/// Largest model file read, far above any villager's.
const MAX_FILE_BYTES: u64 = 256 << 20;
const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

/// Writes `project`'s model to the model file at `path` and the images its
/// materials show beside it, each texture in the variant `variants` gives
/// for it. Refuses, before writing anything, to replace a file that isn't
/// a model file or a PNG, or to write a model file it can't read back;
/// writes the model file last.
pub fn write(
    project: &Project,
    path: &ModelFilePath,
    variants: &[usize],
    units: Units,
) -> Result<(), ModelFileError> {
    let file = path.as_path();
    check_replaceable(file, GLB_MAGIC, ModelFileError::NotModelFile)?;
    let images = image_paths(project, path);
    let uris: Vec<String> = images
        .iter()
        .map(|p| uri(&p.file_name().unwrap_or_default().to_string_lossy()))
        .collect();
    let bytes = encode(project, &uris, units)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(ModelFileError::TooLarge);
    }
    // So a model file is never replaced by one this can't read back.
    decode(project, &bytes, units).map_err(|e| ModelFileError::NotReadable(Box::new(e)))?;
    write_pngs(project, &images, variants)?;
    write_atomic(file, &bytes).map_err(|e| ModelFileError::Io(file.to_owned(), e))
}

/// Writes the images the model file at `path`, which must exist, shows for
/// `project`'s textures, each in the variant `variants` gives for it.
/// Refuses, before writing anything, to replace a file that isn't a PNG.
///
/// Panics unless `variants` has one entry per texture, each a variant of a
/// painted texture.
pub fn write_images(
    project: &Project,
    path: &ModelFilePath,
    variants: &[usize],
) -> Result<(), ModelFileError> {
    let file = path.as_path();
    if starts_with(file, GLB_MAGIC)? != Some(true) {
        return Err(ModelFileError::NoModelFile(file.to_owned()));
    }
    write_pngs(project, &image_paths(project, path), variants)
}

/// Writes each texture's image to its path in `images`, in the variant
/// `variants` gives for it; external textures show their preview, whatever
/// `variants` gives. Refuses, before writing anything, to replace a file
/// that isn't a PNG.
fn write_pngs(
    project: &Project,
    images: &[PathBuf],
    variants: &[usize],
) -> Result<(), ModelFileError> {
    assert_eq!(
        variants.len(),
        project.textures().len(),
        "one variant per texture"
    );
    for image in images {
        check_replaceable(image, PNG_MAGIC, ModelFileError::NotPng)?;
    }
    for ((i, t), image) in project.textures().iter().enumerate().zip(images) {
        let (size, pixels): (_, Vec<Rgba8>) = match t.content() {
            TextureContent::Painted(_) => {
                let v = project.variant(VariantRef {
                    texture: TextureId(i as u32),
                    variant: variants[i],
                });
                let colors = project.palette().colors();
                let img = v.rasterize();
                (
                    img.size(),
                    img.pixels().iter().map(|p| colors[p.0 as usize]).collect(),
                )
            }
            TextureContent::External { preview } => (preview.size, preview.pixels.clone()),
        };
        let png = encode_png(size, &pixels).map_err(|e| ModelFileError::Png(image.clone(), e))?;
        write_atomic(image, &png).map_err(|e| ModelFileError::Io(image.clone(), e))?;
    }
    Ok(())
}

fn encode_png(size: glam::UVec2, pixels: &[Rgba8]) -> Result<Vec<u8>, png::EncodingError> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, size.x, size.y);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let bytes: Vec<u8> = pixels.iter().flat_map(|c| c.to_array()).collect();
    enc.write_header()?.write_image_data(&bytes)?;
    Ok(out)
}

/// Each of `model`'s nodes by name, refusing two with one name.
fn nodes_by_name(model: &Model) -> Result<HashMap<&str, NodeId>, ModelFileError> {
    let mut out = HashMap::new();
    for id in model.ids() {
        let name = model.node(id).name.as_str();
        if out.insert(name, id).is_some() {
            return Err(ModelFileError::DuplicateNode(name.to_owned()));
        }
    }
    Ok(out)
}

/// The glTF binary buffer under construction, with its views and accessors.
#[derive(Default)]
struct Bin {
    data: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
}

const FLOAT: u32 = 5126;
const UNSIGNED_SHORT: u32 = 5123;
const UNSIGNED_INT: u32 = 5125;
const NEAREST: u32 = 9728;
const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;

impl Bin {
    /// Adds an accessor over `bytes`, which hold `count` elements of `kind`,
    /// with `extra` fields, such as bounds.
    fn accessor(
        &mut self,
        bytes: &[u8],
        component: u32,
        kind: &str,
        count: usize,
        target: Option<u32>,
        extra: Value,
    ) -> usize {
        while !self.data.len().is_multiple_of(4) {
            self.data.push(0);
        }
        let mut view = json!({
            "buffer": 0,
            "byteOffset": self.data.len(),
            "byteLength": bytes.len(),
        });
        if let Some(t) = target {
            view["target"] = json!(t);
        }
        self.data.extend_from_slice(bytes);
        self.views.push(view);
        let mut accessor = json!({
            "bufferView": self.views.len() - 1,
            "componentType": component,
            "count": count,
            "type": kind,
        });
        if let (Value::Object(a), Value::Object(e)) = (&mut accessor, extra) {
            a.extend(e);
        }
        self.accessors.push(accessor);
        self.accessors.len() - 1
    }

    fn floats<const N: usize>(
        &mut self,
        values: &[[f32; N]],
        kind: &str,
        target: Option<u32>,
        extra: Value,
    ) -> usize {
        let bytes: Vec<u8> = values
            .iter()
            .flatten()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        self.accessor(&bytes, FLOAT, kind, values.len(), target, extra)
    }
}

/// The vertex attributes of the file's mesh, with the index that welds
/// identical vertices.
#[derive(Default)]
struct Vertices {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    joints: Vec<u16>,
    /// Each vertex's index by its node and attribute bits.
    index: HashMap<(u32, [u32; 8]), u32>,
}

impl Vertices {
    fn add(&mut self, node: NodeId, position: Vec3, normal: Vec3, uv: Vec2) -> u32 {
        // Adding zero turns -0.0 into 0.0, so the two weld.
        let bits = [
            position.x, position.y, position.z, normal.x, normal.y, normal.z, uv.x, uv.y,
        ]
        .map(|f| (f + 0.0).to_bits());
        *self.index.entry((node.0, bits)).or_insert_with(|| {
            self.positions.push(position.to_array());
            self.normals.push(normal.to_array());
            self.uvs.push(uv.to_array());
            self.joints
                .push(u16::try_from(node.0).expect("encode refuses more nodes"));
            self.positions.len() as u32 - 1
        })
    }
}

/// The matrix that carries normals the way `m` carries points.
fn normal_matrix(m: Mat4) -> Mat3 {
    Mat3::from_mat4(m).inverse().transpose()
}

/// The unit direction of finite `n`, or zero for zero. Scaled to its
/// largest part first, so a very long or short vector doesn't round to zero.
fn direction(n: Vec3) -> Vec3 {
    (n / n.abs().max_element().max(f32::MIN_POSITIVE)).normalize_or_zero()
}

/// Encodes `project`'s model as a glTF binary whose materials show the
/// images at `uris`, one per texture, relative to the file.
fn encode(project: &Project, uris: &[String], units: Units) -> Result<Vec<u8>, ModelFileError> {
    let model = project.model();
    nodes_by_name(model)?;
    if model.nodes().len() > 1 << 16 {
        return Err(ModelFileError::TooManyNodes(model.nodes().len()));
    }
    let s = units.meters_per_unit;
    let locals = model.local_matrices(project.pose());
    let world = model.compose(&locals);
    let normals: Vec<Mat3> = world.iter().copied().map(normal_matrix).collect();
    let to_local: Vec<Mat4> = world.iter().map(Mat4::inverse).collect();

    // The skeleton, posed, with translations in the file's units. The
    // inverse bind matrices come from the transforms as written, so they
    // cancel them exactly even where a node's matrix doesn't decompose.
    let written: Vec<(Vec3, glam::Quat, Vec3)> = locals
        .iter()
        .map(|m| {
            let (scale, rotation, translation) = m.to_scale_rotation_translation();
            // A sheared matrix gives a rotation that isn't unit length.
            (scale, rotation.normalize(), translation * s)
        })
        .collect();
    let written_world = model.compose(
        &written
            .iter()
            .map(|&(sc, r, t)| Mat4::from_scale_rotation_translation(sc, r, t))
            .collect::<Vec<_>>(),
    );
    let ibms: Vec<Mat4> = written_world.iter().map(Mat4::inverse).collect();
    for id in model.ids() {
        let i = id.index();
        let (sc, r, t) = written[i];
        let det = written_world[i].determinant();
        let finite = sc.is_finite() && r.is_finite() && t.is_finite();
        if !finite
            || !det.is_finite()
            || det == 0.0
            || !ibms[i].is_finite()
            || !normals[i].is_finite()
            || !to_local[i].is_finite()
        {
            return Err(ModelFileError::Unwritable {
                node: model.node(id).name.clone(),
                problem: "its transform is too small, too large, or isn't finite",
            });
        }
    }

    // Triangles in drawing order: one primitive per run of a node's
    // triangles that share a material, so a file read back unchanged keeps
    // the order. Blender groups each object's faces by material instead.
    let mut verts = Vertices::default();
    let mut runs: Vec<(u32, Vec<u32>)> = Vec::new();
    for id in model.ids() {
        let Some(mesh) = &model.node(id).mesh else {
            continue;
        };
        let mut last = None;
        for t in &mesh.triangles {
            if last != Some(t.material) {
                runs.push((t.material.0, Vec::new()));
                last = Some(t.material);
            }
            let run = &mut runs.last_mut().expect("pushed above").1;
            for c in t.corners {
                let v = mesh.vertices[c.vertex as usize];
                let f = v.followed(id);
                let p = world[f.index()].transform_point3(v.position) * s;
                // Checked before normalizing, which turns NaN into zero. A
                // zero normal, which a few AC vertices have, stays zero
                // rather than taking a made-up direction.
                let n = normals[f.index()] * v.normal;
                if !(p.is_finite() && n.is_finite() && c.uv.is_finite()) {
                    return Err(ModelFileError::Unwritable {
                        node: model.node(id).name.clone(),
                        problem: "a vertex's position, normal, or UV isn't a finite number",
                    });
                }
                // Mapped back as reading does, so a vertex too far out or
                // under too small a scale for f32 is refused, not moved.
                let back = to_local[f.index()].transform_point3(p / s);
                if (back - v.position).abs().max_element() > POSITION_TOLERANCE {
                    return Err(ModelFileError::Unwritable {
                        node: model.node(id).name.clone(),
                        problem: "a vertex wouldn't keep its position, as it's too far out or under too small a scale",
                    });
                }
                run.push(verts.add(f, p, direction(n), c.uv));
            }
        }
    }
    if verts.positions.is_empty() {
        return Err(ModelFileError::NoMesh);
    }

    let mut bin = Bin::default();
    let (min, max) = verts.positions.iter().fold(
        (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
        |(lo, hi), &p| (lo.min(p.into()), hi.max(p.into())),
    );
    let position = bin.floats(
        &verts.positions,
        "VEC3",
        Some(ARRAY_BUFFER),
        json!({"min": min.to_array(), "max": max.to_array()}),
    );
    let normal = bin.floats(&verts.normals, "VEC3", Some(ARRAY_BUFFER), json!({}));
    let uv = bin.floats(&verts.uvs, "VEC2", Some(ARRAY_BUFFER), json!({}));
    let joints = {
        let bytes: Vec<u8> = verts
            .joints
            .iter()
            .flat_map(|&j| [j, 0, 0, 0])
            .flat_map(u16::to_le_bytes)
            .collect();
        bin.accessor(
            &bytes,
            UNSIGNED_SHORT,
            "VEC4",
            verts.joints.len(),
            Some(ARRAY_BUFFER),
            json!({}),
        )
    };
    let weights = bin.floats(
        &vec![[1.0f32, 0.0, 0.0, 0.0]; verts.joints.len()],
        "VEC4",
        Some(ARRAY_BUFFER),
        json!({}),
    );
    let primitives: Vec<Value> = runs
        .iter()
        .map(|(material, indices)| {
            let bytes: Vec<u8> = indices.iter().flat_map(|i| i.to_le_bytes()).collect();
            let a = bin.accessor(
                &bytes,
                UNSIGNED_INT,
                "SCALAR",
                indices.len(),
                Some(ELEMENT_ARRAY_BUFFER),
                json!({}),
            );
            json!({
                "attributes": {
                    "POSITION": position,
                    "NORMAL": normal,
                    "TEXCOORD_0": uv,
                    "JOINTS_0": joints,
                    "WEIGHTS_0": weights,
                },
                "indices": a,
                "material": material,
            })
        })
        .collect();

    let mut nodes: Vec<Value> = model
        .ids()
        .zip(&written)
        .map(|(id, (scale, rotation, translation))| {
            let children: Vec<u32> = model.children(id).map(|c| c.0).collect();
            let mut node = json!({
                "name": model.node(id).name,
                "translation": translation.to_array(),
                "rotation": rotation.to_array(),
                "scale": scale.to_array(),
            });
            if !children.is_empty() {
                node["children"] = json!(children);
            }
            node
        })
        .collect();
    let ibms: Vec<[f32; 16]> = ibms.iter().map(Mat4::to_cols_array).collect();
    let ibm = bin.floats(&ibms, "MAT4", None, json!({}));
    nodes.push(json!({"name": "Model", "mesh": 0, "skin": 0}));

    let material_names = material_names(project);
    let materials: Vec<Value> = project
        .materials()
        .iter()
        .zip(&material_names)
        .enumerate()
        .map(|(i, (m, name))| {
            let tint = m.tint.to_array();
            let linear = |c: u8| {
                let c = f32::from(c) / 255.0;
                if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
            };
            let mut material = json!({
                "name": name,
                "pbrMetallicRoughness": {
                    "baseColorTexture": {"index": i},
                    "baseColorFactor": [linear(tint[0]), linear(tint[1]), linear(tint[2]), f32::from(tint[3]) / 255.0],
                    "metallicFactor": 0.0,
                    "roughnessFactor": 1.0,
                },
                "doubleSided": m.double_sided,
            });
            if m.alpha == AlphaMode::Cutout {
                material["alphaMode"] = json!("MASK");
                material["alphaCutoff"] = json!(0.5);
            }
            material
        })
        .collect();
    let wrap = |w: Wrap| match w {
        Wrap::Clamp => 33071,
        Wrap::Repeat => 10497,
        Wrap::Mirror => 33648,
    };
    let samplers: Vec<Value> = project
        .materials()
        .iter()
        .map(|m| json!({"magFilter": NEAREST, "minFilter": NEAREST, "wrapS": wrap(m.wrap[0]), "wrapT": wrap(m.wrap[1])}))
        .collect();
    let textures: Vec<Value> = project
        .materials()
        .iter()
        .enumerate()
        .map(|(i, m)| json!({"sampler": i, "source": m.texture.0}))
        .collect();
    let images: Vec<Value> = uris.iter().map(|u| json!({"uri": u})).collect();

    let root = model.ids().count();
    let doc = json!({
        "asset": {"version": "2.0", "generator": "mint-rabbit"},
        "scene": 0,
        "scenes": [{"nodes": [0, root]}],
        "nodes": nodes,
        "meshes": [{"name": "Model", "primitives": primitives}],
        "skins": [{
            "joints": (0..root).collect::<Vec<_>>(),
            "inverseBindMatrices": ibm,
            "skeleton": 0,
        }],
        "materials": materials,
        "samplers": samplers,
        "textures": textures,
        "images": images,
        "buffers": [{"byteLength": bin.data.len()}],
        "bufferViews": bin.views,
        "accessors": bin.accessors,
    });
    let json = serde_json::to_vec(&doc).expect("a JSON value serializes");
    Ok(glb(json, bin.data))
}

/// Packs a glTF document and its buffer into the binary container.
fn glb(mut json: Vec<u8>, mut bin: Vec<u8>) -> Vec<u8> {
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let total = 12 + 8 + json.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(GLB_MAGIC);
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);
    out
}

/// Reads the model file at `path` as new meshes for `project`'s nodes, one
/// per node in id order, for [`Project::set_meshes`]. `units` must be what
/// the file was written with.
pub fn read(
    project: &Project,
    path: &ModelFilePath,
    units: Units,
) -> Result<Vec<Option<Mesh>>, ModelFileError> {
    use std::io::Read as _;
    let file = path.as_path();
    let io = |e| ModelFileError::Io(file.to_owned(), e);
    let meta = std::fs::metadata(file).map_err(io)?;
    if !meta.is_file() {
        return Err(ModelFileError::NotModelFile(file.to_owned()));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(file)
        .map_err(io)?
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    decode(project, &bytes, units)
}

/// The model-space matrix of each of the file's nodes, given each node's
/// parent and local matrix.
fn globals(parent: &[Option<usize>], locals: &[Mat4]) -> Result<Vec<Mat4>, ModelFileError> {
    let mut global: Vec<Option<Mat4>> = vec![None; locals.len()];
    for i in 0..locals.len() {
        // Walk up to a resolved node or a root, then back down.
        let mut chain = vec![i];
        while let Some(p) = parent[*chain.last().expect("never empty")] {
            if global[p].is_some() {
                break;
            }
            if chain.len() > locals.len() {
                return Err(ModelFileError::Cycle);
            }
            chain.push(p);
        }
        for &n in chain.iter().rev() {
            if global[n].is_none() {
                let up = parent[n].and_then(|p| global[p]).unwrap_or(Mat4::IDENTITY);
                global[n] = Some(up * locals[n]);
            }
        }
    }
    Ok(global
        .into_iter()
        .map(|m| m.expect("all resolved"))
        .collect())
}

/// Blender adds `.001` and so on to a name already taken in its file.
fn strip_blender_suffix(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((base, n)) if n.len() == 3 && n.bytes().all(|b| b.is_ascii_digit()) => base,
        _ => name,
    }
}

/// The accessor of `prim`'s attribute `semantic`, if it has one.
fn attribute<'a>(
    prim: &gltf::Primitive<'a>,
    semantic: &gltf::Semantic,
) -> Option<gltf::Accessor<'a>> {
    prim.attributes()
        .find(|(s, _)| s == semantic)
        .map(|(_, a)| a)
}

/// Fails unless `accessor` is one of `kinds` and lies wholly inside the
/// binary chunk of `blob_len` bytes, so reading it can't panic.
fn check_accessor(
    accessor: &gltf::Accessor<'_>,
    kinds: &[(DataType, Dimensions)],
    blob_len: usize,
    object: &str,
    what: &str,
) -> Result<(), ModelFileError> {
    let malformed = || ModelFileError::Malformed {
        object: object.to_owned(),
        what: what.to_owned(),
    };
    let kind = (accessor.data_type(), accessor.dimensions());
    if !kinds.contains(&kind) || accessor.sparse().is_some() || accessor.count() == 0 {
        return Err(malformed());
    }
    let view = accessor.view().ok_or_else(malformed)?;
    let size = accessor.size();
    let stride = view.stride().unwrap_or(size);
    let fits = (|| {
        let end_in_view = stride
            .checked_mul(accessor.count() - 1)?
            .checked_add(accessor.offset())?
            .checked_add(size)?;
        let view_end = view.offset().checked_add(view.length())?;
        Some(stride >= size && end_in_view <= view.length() && view_end <= blob_len)
    })();
    if fits == Some(true) {
        Ok(())
    } else {
        Err(malformed())
    }
}

const POSITION_KINDS: &[(DataType, Dimensions)] = &[(DataType::F32, Dimensions::Vec3)];
const UV_KINDS: &[(DataType, Dimensions)] = &[
    (DataType::F32, Dimensions::Vec2),
    (DataType::U8, Dimensions::Vec2),
    (DataType::U16, Dimensions::Vec2),
];
const JOINT_KINDS: &[(DataType, Dimensions)] = &[
    (DataType::U8, Dimensions::Vec4),
    (DataType::U16, Dimensions::Vec4),
];
const WEIGHT_KINDS: &[(DataType, Dimensions)] = &[
    (DataType::F32, Dimensions::Vec4),
    (DataType::U8, Dimensions::Vec4),
    (DataType::U16, Dimensions::Vec4),
];
const INDEX_KINDS: &[(DataType, Dimensions)] = &[
    (DataType::U8, Dimensions::Scalar),
    (DataType::U16, Dimensions::Scalar),
    (DataType::U32, Dimensions::Scalar),
];
const MATRIX_KINDS: &[(DataType, Dimensions)] = &[(DataType::F32, Dimensions::Mat4)];

/// Decodes a glTF binary as new meshes for `project`'s nodes.
fn decode(
    project: &Project,
    bytes: &[u8],
    units: Units,
) -> Result<Vec<Option<Mesh>>, ModelFileError> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(ModelFileError::TooLarge);
    }
    // The gltf crate panics on some malformed files, such as a short header
    // or a primitive naming a missing accessor, before returning an error.
    let gltf = std::panic::catch_unwind(|| gltf::Gltf::from_slice(bytes))
        .map_err(|_| ModelFileError::Unreadable)??;
    let blob = gltf.blob.as_deref().unwrap_or_default();
    if gltf
        .buffers()
        .any(|b| matches!(b.source(), gltf::buffer::Source::Uri(_)))
    {
        return Err(ModelFileError::ExternalData);
    }
    let buffer = |_: gltf::Buffer<'_>| Some(blob);

    // Model-space matrices of the file's nodes.
    let count = gltf.nodes().len();
    let mut parent = vec![None; count];
    for n in gltf.nodes() {
        for c in n.children() {
            parent[c.index()] = Some(n.index());
        }
    }
    let locals: Vec<Mat4> = gltf
        .nodes()
        .map(|n| Mat4::from_cols_array_2d(&n.transform().matrix()))
        .collect();
    let global = globals(&parent, &locals)?;
    // The default scene's nodes; with no default, the first scene's.
    let mut in_scene = vec![false; count];
    let mut stack: Vec<gltf::Node<'_>> = gltf
        .default_scene()
        .or_else(|| gltf.scenes().next())
        .map(|s| s.nodes().collect())
        .unwrap_or_default();
    while let Some(n) = stack.pop() {
        if !std::mem::replace(&mut in_scene[n.index()], true) {
            stack.extend(n.children());
        }
    }

    let model = project.model();
    let by_name = nodes_by_name(model)?;
    let depth = model.depths();
    let materials: HashMap<String, MaterialId> = material_names(project)
        .into_iter()
        .enumerate()
        .map(|(i, n)| (n, MaterialId(i as u32)))
        .collect();
    let world = model.world_matrices(project.pose());
    let to_local: Vec<Mat4> = world.iter().map(|m| m.inverse()).collect();
    // Normals carry back by the inverse of the normal matrix, the
    // transpose of the point matrix.
    let to_local_normal: Vec<Mat3> = world
        .iter()
        .map(|m| Mat3::from_mat4(*m).transpose())
        .collect();

    let mut meshes: Vec<Option<Mesh>> = vec![None; model.nodes().len()];
    let mut welds: Vec<HashMap<(u32, [u32; 6]), u32>> = vec![HashMap::new(); model.nodes().len()];
    for node in gltf.nodes().filter(|n| in_scene[n.index()]) {
        let Some(mesh) = node.mesh() else { continue };
        let object = node
            .name()
            .or(mesh.name())
            .map_or_else(|| format!("node {}", node.index()), str::to_owned);
        let malformed = |what: &str| ModelFileError::Malformed {
            object: object.clone(),
            what: what.to_owned(),
        };
        let skin = node
            .skin()
            .ok_or_else(|| ModelFileError::Unskinned(object.clone()))?;
        let joint_count = skin.joints().len();
        let ibms: Vec<Mat4> = match skin.inverse_bind_matrices() {
            // The glTF default when a skin gives none.
            None => vec![Mat4::IDENTITY; joint_count],
            Some(a) => {
                check_accessor(&a, MATRIX_KINDS, blob.len(), &object, "skin")?;
                let ibms: Vec<Mat4> = skin
                    .reader(buffer)
                    .read_inverse_bind_matrices()
                    .ok_or_else(|| malformed("skin"))?
                    .map(|m| Mat4::from_cols_array_2d(&m))
                    .collect();
                if ibms.len() < joint_count {
                    return Err(malformed("skin"));
                }
                ibms
            }
        };
        // Each joint's bind-space-to-file matrix, its normal matrix, and its
        // project node, or the bone's name where the project has no such
        // node.
        let joints: Vec<Result<(Mat4, Mat3, NodeId), String>> = skin
            .joints()
            .zip(&ibms)
            .map(|(jn, ibm)| {
                let name = jn.name().unwrap_or_default();
                let id = by_name.get(name).copied().ok_or_else(|| name.to_owned())?;
                let m = global[jn.index()] * *ibm;
                Ok((m, normal_matrix(m), id))
            })
            .collect();
        for (m, nm, id) in joints.iter().flatten() {
            let det = m.determinant();
            if !m.is_finite() || !nm.is_finite() || !det.is_finite() || det == 0.0 {
                return Err(malformed(&format!(
                    "skin: bone {:?} is scaled to nothing or isn't a finite transform",
                    model.node(*id).name
                )));
            }
        }

        for prim in mesh.primitives() {
            if prim.mode() != gltf::mesh::Mode::Triangles {
                return Err(ModelFileError::NotTriangles(object));
            }
            let material = prim
                .material()
                .name()
                .ok_or_else(|| ModelFileError::NoMaterial(object.clone()))?;
            let material = *materials
                .get(material)
                .or_else(|| materials.get(strip_blender_suffix(material)))
                .ok_or_else(|| {
                    let mut known: Vec<&str> = materials.keys().map(String::as_str).collect();
                    known.sort_unstable();
                    ModelFileError::UnknownMaterial {
                        object: object.clone(),
                        material: material.to_owned(),
                        known: known.join(", "),
                    }
                })?;
            let missing = |attribute| ModelFileError::MissingAttribute {
                object: object.clone(),
                attribute,
            };
            let checked =
                |semantic: gltf::Semantic, kinds: &[(DataType, Dimensions)], what: &'static str| {
                    let a = attribute(&prim, &semantic).ok_or_else(|| missing(what))?;
                    check_accessor(&a, kinds, blob.len(), &object, what)
                };
            checked(
                gltf::Semantic::Positions,
                POSITION_KINDS,
                "vertex positions",
            )?;
            checked(gltf::Semantic::Normals, POSITION_KINDS, "normals")?;
            // The UV map the material's texture names, by default the first.
            let uv_set = prim
                .material()
                .pbr_metallic_roughness()
                .base_color_texture()
                .map_or(0, |t| t.tex_coord());
            checked(gltf::Semantic::TexCoords(uv_set), UV_KINDS, "UV map")?;
            let mut sets = 0;
            let vertex_count = attribute(&prim, &gltf::Semantic::Positions)
                .expect("checked above")
                .count();
            while let Some(j) = attribute(&prim, &gltf::Semantic::Joints(sets)) {
                check_accessor(&j, JOINT_KINDS, blob.len(), &object, "bone list")?;
                checked(gltf::Semantic::Weights(sets), WEIGHT_KINDS, "bone weights")?;
                let w = attribute(&prim, &gltf::Semantic::Weights(sets)).expect("checked above");
                if j.count() != vertex_count || w.count() != vertex_count {
                    return Err(malformed(
                        "skin: its bone lists and vertices differ in number",
                    ));
                }
                sets += 1;
            }
            if let Some(i) = prim.indices() {
                check_accessor(&i, INDEX_KINDS, blob.len(), &object, "face list")?;
            }

            let reader = prim.reader(buffer);
            let positions: Vec<Vec3> = reader
                .read_positions()
                .ok_or_else(|| missing("vertex positions"))?
                .map(Vec3::from)
                .collect();
            let normals: Vec<Vec3> = reader
                .read_normals()
                .ok_or_else(|| missing("normals"))?
                .map(Vec3::from)
                .collect();
            let uvs: Vec<Vec2> = reader
                .read_tex_coords(uv_set)
                .ok_or_else(|| missing("UV map"))?
                .into_f32()
                .map(Vec2::from)
                .collect();
            if uvs.len() != positions.len() || normals.len() != positions.len() {
                return Err(malformed("vertex list: its attributes differ in length"));
            }
            // The one joint each vertex follows, or `None` for none or many.
            let mut bone: Vec<Option<usize>> = vec![None; positions.len()];
            let mut weighted = vec![0u32; positions.len()];
            for set in 0..sets {
                let (Some(j), Some(w)) = (reader.read_joints(set), reader.read_weights(set)) else {
                    return Err(missing("bone weights"));
                };
                for (v, (j, w)) in j.into_u16().zip(w.into_f32()).enumerate() {
                    for (j, w) in j.into_iter().zip(w) {
                        if !(w.is_finite() && w >= 0.0) {
                            return Err(malformed(
                                "skin: a bone weight is negative or not a number",
                            ));
                        }
                        if w > 0.0 {
                            weighted[v] += 1;
                            bone[v] = Some(j as usize);
                        }
                    }
                }
            }
            let bad: Vec<usize> = (0..positions.len()).filter(|&v| weighted[v] != 1).collect();
            if let Some(&first) = bad.first() {
                return Err(ModelFileError::VertexBones {
                    object: object.clone(),
                    count: bad.len(),
                    at: positions[first],
                });
            }
            let indices: Vec<u32> = match reader.read_indices() {
                Some(i) => i.into_u32().collect(),
                None => (0..positions.len() as u32).collect(),
            };
            if !indices.len().is_multiple_of(3) {
                return Err(ModelFileError::NotTriangles(object));
            }
            if indices.iter().any(|&i| i as usize >= positions.len()) {
                return Err(malformed("face list: it names vertices past its last"));
            }

            for tri in indices.chunks_exact(3) {
                let mut corners = [(NodeId::ROOT, Vec3::ZERO, Vec3::ZERO, Vec2::ZERO); 3];
                for (c, &v) in corners.iter_mut().zip(tri) {
                    let v = v as usize;
                    let j = bone[v].expect("checked above");
                    let (m, nm, id) = joints
                        .get(j)
                        .cloned()
                        .unwrap_or_else(|| Err(format!("joint {j}")))
                        .map_err(|bone| ModelFileError::UnknownBone {
                            object: object.clone(),
                            bone,
                        })?;
                    if !(positions[v].is_finite() && normals[v].is_finite()) {
                        return Err(malformed(
                            "vertex: its position or normal isn't a finite number",
                        ));
                    }
                    let p = m.transform_point3(positions[v]) / units.meters_per_unit;
                    // Checked before normalizing, which turns NaN into zero.
                    let n = to_local_normal[id.index()] * (nm * normals[v]);
                    let local = (
                        id,
                        to_local[id.index()].transform_point3(p),
                        direction(n) * units.normal_length,
                        uvs[v],
                    );
                    if !(local.1.is_finite() && n.is_finite() && local.3.is_finite()) {
                        return Err(malformed(
                            "vertex: its position, normal, or UV isn't a finite number in the model",
                        ));
                    }
                    *c = local;
                }
                let owner = corners
                    .iter()
                    .map(|c| c.0)
                    .max_by_key(|id| (depth[id.index()], std::cmp::Reverse(id.0)))
                    .expect("three corners");
                let mesh = meshes[owner.index()].get_or_insert_with(Mesh::default);
                let weld = &mut welds[owner.index()];
                let corners = corners.map(|(id, p, n, uv)| {
                    let bits = [p.x, p.y, p.z, n.x, n.y, n.z].map(|f| (f + 0.0).to_bits());
                    let vertex = *weld.entry((id.0, bits)).or_insert_with(|| {
                        mesh.vertices.push(Vertex {
                            position: p,
                            normal: n,
                            follows: (id != owner).then_some(id),
                        });
                        mesh.vertices.len() as u32 - 1
                    });
                    Corner { vertex, uv }
                });
                mesh.triangles.push(Triangle { corners, material });
            }
        }
    }
    if meshes.iter().all(Option::is_none) {
        return Err(ModelFileError::NoMesh);
    }
    Ok(meshes)
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use glam::{Quat, UVec2};

    use super::*;
    use crate::{
        ExportTarget, IndexedImage, Material, Palette, PaletteIndex, Pose, Slot, Texture,
        Transform, Variant,
    };

    const UNITS: Units = Units {
        meters_per_unit: 0.01,
        normal_length: 1.0,
    };

    fn vertex(x: f32, y: f32, normal: Vec3, follows: Option<NodeId>) -> Vertex {
        Vertex {
            position: Vec3::new(x, y, 0.0),
            normal,
            follows,
        }
    }

    fn triangle(corners: [(u32, f32, f32); 3], material: u32) -> Triangle {
        Triangle {
            corners: corners.map(|(vertex, u, v)| Corner {
                vertex,
                uv: Vec2::new(u, v),
            }),
            material: MaterialId(material),
        }
    }

    /// A project whose root and child, the child turned by the pose, have
    /// `meshes`, drawing with materials named after `textures`.
    fn project(child_scale: Vec3, meshes: [Mesh; 2], textures: &[&str]) -> Project {
        let mut model = Model::new(
            "root",
            Transform::from_translation(Vec3::new(0.0, 100.0, 0.0)),
        );
        let child = model.add_child(
            NodeId::ROOT,
            "child",
            Transform {
                scale: child_scale,
                ..Transform::from_translation(Vec3::new(50.0, 0.0, 0.0))
            },
        );
        let [root_mesh, child_mesh] = meshes;
        model.node_mut(NodeId::ROOT).mesh = Some(root_mesh);
        model.node_mut(child).mesh = Some(child_mesh);
        let mut pose = Pose::default();
        pose.set_rotation(child, Quat::from_rotation_z(0.5));
        let palette = Palette::from_colors([Rgba8::new(10, 20, 30, 255)]).unwrap();
        let image = IndexedImage::new(UVec2::new(2, 2), vec![PaletteIndex(0); 4]).unwrap();
        let textures: Vec<Texture> = textures
            .iter()
            .map(|name| {
                Texture::painted(
                    *name,
                    Slot("body".into()),
                    UVec2::new(2, 2),
                    [Variant::imported("1", image.clone())],
                )
                .unwrap()
            })
            .collect();
        let material = Material {
            texture: TextureId(0),
            wrap: [Wrap::Clamp, Wrap::Repeat],
            alpha: AlphaMode::Opaque,
            double_sided: false,
            tint: Rgba8::new(255, 255, 255, 255),
        };
        let materials = vec![
            material,
            Material {
                alpha: AlphaMode::Cutout,
                ..material
            },
        ];
        Project::new(
            ExportTarget::new("test"),
            palette,
            model,
            textures,
            materials,
            pose,
        )
        .unwrap()
    }

    /// The root's triangle and the child's two, which share two corners
    /// with the root's as seam vertices.
    fn meshes() -> [Mesh; 2] {
        [
            Mesh {
                vertices: vec![
                    vertex(0.0, 0.0, Vec3::Z, None),
                    vertex(50.0, 0.0, Vec3::Z, None),
                    vertex(50.0, 10.0, Vec3::Z, None),
                ],
                triangles: vec![triangle([(0, 0.0, 0.0), (1, 0.5, 0.0), (2, 0.5, 0.5)], 0)],
            },
            Mesh {
                vertices: vec![
                    vertex(50.0, 0.0, Vec3::Z, Some(NodeId::ROOT)),
                    vertex(50.0, 10.0, Vec3::Z, Some(NodeId::ROOT)),
                    vertex(20.0, 5.0, Vec3::Y, None),
                ],
                triangles: vec![
                    triangle([(0, 0.5, 0.0), (1, 0.5, 0.5), (2, 1.0, 0.25)], 0),
                    triangle([(2, 1.0, 0.25), (1, 0.5, 0.5), (0, 0.5, 0.0)], 1),
                ],
            },
        ]
    }

    fn sample() -> Project {
        project(Vec3::ONE, meshes(), &["body"])
    }

    /// Positions rounded to whole units and normals to hundredths, by node.
    fn summary<'a>(meshes: impl Iterator<Item = Option<&'a Mesh>>) -> Vec<Vec<String>> {
        meshes
            .enumerate()
            .map(|(i, m)| {
                let Some(m) = m else { return Vec::new() };
                m.triangles
                    .iter()
                    .map(|t| {
                        let mut s = format!("m{}", t.material.0);
                        for c in t.corners {
                            let v = m.vertices[c.vertex as usize];
                            let _ = write!(
                                s,
                                " p{:?} n{:?} f{} uv{:?}",
                                (v.position.round() + 0.0).to_array(),
                                ((v.normal * 100.0).round() + 0.0).to_array(),
                                v.followed(NodeId(i as u32)).0,
                                c.uv.to_array()
                            );
                        }
                        s
                    })
                    .collect()
            })
            .collect()
    }

    fn round_trip(p: &Project) -> Vec<Option<Mesh>> {
        let bytes = encode(p, &["body.png".into()], UNITS).unwrap();
        decode(p, &bytes, UNITS).unwrap()
    }

    #[test]
    fn round_trip_keeps_triangles_seams_and_order() {
        let p = sample();
        let meshes = round_trip(&p);
        assert_eq!(
            summary(meshes.iter().map(Option::as_ref)),
            summary(p.model().meshes())
        );
        // The child's two seam vertices still follow the root.
        let child = meshes[1].as_ref().unwrap();
        assert_eq!(
            child
                .vertices
                .iter()
                .filter(|v| v.follows == Some(NodeId::ROOT))
                .count(),
            2
        );
    }

    #[test]
    fn round_trip_survives_a_scale_the_pose_shears() {
        let p = project(Vec3::new(2.0, 1.0, 1.0), meshes(), &["body"]);
        let meshes = round_trip(&p);
        assert_eq!(
            summary(meshes.iter().map(Option::as_ref)),
            summary(p.model().meshes())
        );
    }

    #[test]
    fn normals_come_back_at_the_target_length() {
        let p = sample();
        let units = Units {
            normal_length: 0.5,
            ..UNITS
        };
        let bytes = encode(&p, &["body.png".into()], units).unwrap();
        let meshes = decode(&p, &bytes, units).unwrap();
        for v in &meshes[1].as_ref().unwrap().vertices {
            assert!((v.normal.length() - 0.5).abs() < 1e-5, "{}", v.normal);
        }
    }

    #[test]
    fn a_triangle_moves_to_the_deepest_node_its_corners_follow() {
        let [root, mut child] = meshes();
        // A third child triangle whose corners all follow the root.
        child
            .vertices
            .push(vertex(40.0, 0.0, Vec3::Z, Some(NodeId::ROOT)));
        child
            .triangles
            .push(triangle([(0, 0.5, 0.0), (3, 0.4, 0.0), (1, 0.5, 0.5)], 0));
        let p = project(Vec3::ONE, [root, child], &["body"]);
        let meshes = round_trip(&p);
        assert_eq!(meshes[0].as_ref().unwrap().triangles.len(), 2);
        assert_eq!(meshes[1].as_ref().unwrap().triangles.len(), 2);
        let moved = meshes[0].as_ref().unwrap();
        assert!(moved.vertices.iter().all(|v| v.follows.is_none()));
    }

    #[test]
    fn seam_vertices_weld_into_one() {
        let p = sample();
        let bytes = encode(&p, &["body.png".into()], UNITS).unwrap();
        let gltf = gltf::Gltf::from_slice(&bytes).unwrap();
        // The root's three vertices, which the child's seam vertices share,
        // and the child's own vertex.
        assert_eq!(gltf.accessors().next().unwrap().count(), 4);
    }

    #[test]
    fn reading_refuses_unknown_bones_and_materials() {
        let p = sample();
        let bytes = encode(&p, &["body.png".into()], UNITS).unwrap();

        let mut renamed = p.model().clone();
        renamed.node_mut(NodeId(1)).name = "arm".into();
        let other = Project::new(
            p.target().clone(),
            p.palette().clone(),
            renamed,
            p.textures().to_vec(),
            p.materials().to_vec(),
            p.pose().clone(),
        )
        .unwrap();
        assert!(matches!(
            decode(&other, &bytes, UNITS),
            Err(ModelFileError::UnknownBone { bone, .. }) if bone == "child"
        ));

        let other = project(Vec3::ONE, meshes(), &["skin"]);
        assert!(matches!(
            decode(&other, &bytes, UNITS),
            Err(ModelFileError::UnknownMaterial { material, known, .. })
                if material == "body" && known == "skin, skin #1"
        ));
    }

    /// `bytes` with the JSON chunk edited by `f`.
    fn edit_json(bytes: &[u8], f: impl FnOnce(&mut Value)) -> Vec<u8> {
        let gltf = gltf::Gltf::from_slice(bytes).unwrap();
        let mut doc: Value = serde_json::to_value(gltf.document.into_json()).unwrap();
        f(&mut doc);
        glb(serde_json::to_vec(&doc).unwrap(), gltf.blob.unwrap())
    }

    #[test]
    fn reading_refuses_malformed_faces() {
        let p = sample();
        let bytes = encode(&p, &["body.png".into()], UNITS).unwrap();
        // The root's triangle's index accessor, cut to two indices.
        let index = |doc: &Value| {
            doc["meshes"][0]["primitives"][0]["indices"]
                .as_u64()
                .unwrap() as usize
        };
        let short = edit_json(&bytes, |doc| {
            let i = index(doc);
            doc["accessors"][i]["count"] = json!(2);
        });
        assert!(matches!(
            decode(&p, &short, UNITS),
            Err(ModelFileError::NotTriangles(_))
        ));
        // Joints given as floats, which the reader can't take.
        let floats = edit_json(&bytes, |doc| {
            let j = doc["meshes"][0]["primitives"][0]["attributes"]["JOINTS_0"]
                .as_u64()
                .unwrap() as usize;
            doc["accessors"][j]["componentType"] = json!(FLOAT);
            doc["accessors"][j]["count"] = json!(1);
        });
        assert!(matches!(
            decode(&p, &floats, UNITS),
            Err(ModelFileError::Malformed { .. })
        ));
        // An accessor longer than its buffer view.
        let long = edit_json(&bytes, |doc| {
            let i = index(doc);
            doc["accessors"][i]["count"] = json!(3_000_000);
        });
        assert!(matches!(
            decode(&p, &long, UNITS),
            Err(ModelFileError::Malformed { .. })
        ));
    }

    #[test]
    fn reading_refuses_indices_past_the_vertices() {
        let p = sample();
        let bytes = encode(&p, &["body.png".into()], UNITS).unwrap();
        let gltf = gltf::Gltf::from_slice(&bytes).unwrap();
        let view = gltf
            .meshes()
            .next()
            .unwrap()
            .primitives()
            .next()
            .unwrap()
            .indices()
            .unwrap()
            .view()
            .unwrap();
        let mut blob = gltf.blob.clone().unwrap();
        blob[view.offset()..view.offset() + 4].copy_from_slice(&999u32.to_le_bytes());
        let json = serde_json::to_vec(&gltf.document.into_json()).unwrap();
        assert!(matches!(
            decode(&p, &glb(json, blob), UNITS),
            Err(ModelFileError::Malformed { .. })
        ));
    }

    #[test]
    fn only_the_default_scene_is_read() {
        let p = sample();
        let bytes = encode(&p, &["body.png".into()], UNITS).unwrap();
        // A second scene holding an unskinned copy of the mesh.
        let backup = edit_json(&bytes, |doc| {
            let n = doc["nodes"].as_array().unwrap().len();
            doc["nodes"]
                .as_array_mut()
                .unwrap()
                .push(json!({"name": "backup", "mesh": 0}));
            doc["scenes"]
                .as_array_mut()
                .unwrap()
                .push(json!({"nodes": [n]}));
        });
        assert!(decode(&p, &backup, UNITS).is_ok());
    }

    #[test]
    fn material_names_are_unique() {
        assert_eq!(material_names(&sample()), ["body", "body #1"]);
        // A texture already named like a generated name.
        let mut p = project(Vec3::ONE, meshes(), &["body", "body #1"]);
        let mut materials = p.materials().to_vec();
        materials.push(Material {
            texture: TextureId(1),
            ..materials[0]
        });
        p = Project::new(
            p.target().clone(),
            p.palette().clone(),
            p.model().clone(),
            p.textures().to_vec(),
            materials,
            p.pose().clone(),
        )
        .unwrap();
        assert_eq!(material_names(&p), ["body", "body #1", "body #1 #2"]);
    }

    #[test]
    fn blender_suffixes_match_their_material() {
        assert_eq!(strip_blender_suffix("body #1.002"), "body #1");
        assert_eq!(strip_blender_suffix("body.v2"), "body.v2");
    }

    #[test]
    fn image_paths_sit_beside_the_file() {
        let p = project(Vec3::ONE, meshes(), &["body", "body"]);
        let path = ModelFilePath::new("/nonexistent-dir/my cat.glb").unwrap();
        assert_eq!(
            image_paths(&p, &path),
            [
                PathBuf::from("/nonexistent-dir/my cat.0-body.png"),
                PathBuf::from("/nonexistent-dir/my cat.1-body.png")
            ]
        );
        assert_eq!(uri("my cat.0-body.png"), "my%20cat.0-body.png");
    }

    #[test]
    fn model_file_paths_are_absolute_utf8_glb_files() {
        assert!(ModelFilePath::new("rel/m.glb").is_err());
        assert!(ModelFilePath::new("/x/m.png").is_err());
        assert!(ModelFilePath::new("/x/m").is_err());
        // The directory resolves through links.
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().canonicalize().unwrap();
        std::os::unix::fs::symlink(&real, real.join("link")).unwrap();
        assert_eq!(
            ModelFilePath::new(real.join("link/m.glb"))
                .unwrap()
                .as_path(),
            real.join("m.glb")
        );
    }

    #[test]
    fn duplicate_node_names_are_refused() {
        let p = sample();
        let mut model = p.model().clone();
        model.node_mut(NodeId(1)).name = "root".into();
        let p = Project::new(
            p.target().clone(),
            p.palette().clone(),
            model,
            p.textures().to_vec(),
            p.materials().to_vec(),
            p.pose().clone(),
        )
        .unwrap();
        assert!(
            matches!(encode(&p, &["a".into()], UNITS), Err(ModelFileError::DuplicateNode(n)) if n == "root")
        );
    }

    #[test]
    fn writes_the_file_and_its_images_but_not_over_other_files() {
        let p = sample();
        let dir = tempfile::tempdir().unwrap();
        let path = ModelFilePath::new(dir.path().join("m.glb")).unwrap();
        let image = path.as_path().with_file_name("m.0-body.png");
        // Images go only beside an existing model file.
        assert!(matches!(
            write_images(&p, &path, &[0]),
            Err(ModelFileError::NoModelFile(_))
        ));
        write(&p, &path, &[0], UNITS).unwrap();
        assert!(image.is_file());
        let meshes = read(&p, &path, UNITS).unwrap();
        assert_eq!(
            summary(meshes.iter().map(Option::as_ref)),
            summary(p.model().meshes())
        );
        // Writing again replaces both.
        write(&p, &path, &[0], UNITS).unwrap();

        let other = ModelFilePath::new(dir.path().join("notes.glb")).unwrap();
        std::fs::write(other.as_path(), "keep me").unwrap();
        assert!(matches!(
            write(&p, &other, &[0], UNITS),
            Err(ModelFileError::NotModelFile(_))
        ));
        assert!(matches!(
            write_images(&p, &other, &[0]),
            Err(ModelFileError::NoModelFile(_))
        ));
        std::fs::write(&image, "keep me").unwrap();
        assert!(matches!(
            write_images(&p, &path, &[0]),
            Err(ModelFileError::NotPng(_))
        ));
        assert_eq!(std::fs::read_to_string(other.as_path()).unwrap(), "keep me");
        assert_eq!(std::fs::read_to_string(&image).unwrap(), "keep me");
    }

    #[test]
    fn a_refused_image_leaves_the_model_file_alone() {
        let p = sample();
        let dir = tempfile::tempdir().unwrap();
        let path = ModelFilePath::new(dir.path().join("m.glb")).unwrap();
        std::fs::create_dir(dir.path().join("m.0-body.png")).unwrap();
        assert!(matches!(
            write(&p, &path, &[0], UNITS),
            Err(ModelFileError::NotPng(_))
        ));
        assert!(!path.as_path().exists());
    }

    #[test]
    fn writing_refuses_a_node_scaled_to_nothing() {
        let p = project(Vec3::new(1.0, 0.0, 1.0), meshes(), &["body"]);
        assert!(matches!(
            encode(&p, &["body.png".into()], UNITS),
            Err(ModelFileError::Unwritable { node, .. }) if node == "child"
        ));
    }

    #[test]
    fn writing_refuses_tiny_scales_and_keeps_the_old_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = ModelFilePath::new(dir.path().join("m.glb")).unwrap();
        write(&sample(), &path, &[0], UNITS).unwrap();
        let before = std::fs::read(path.as_path()).unwrap();
        for scale in [Vec3::splat(1e-13), Vec3::new(1e20, 1.0, 1.0)] {
            let p = project(scale, meshes(), &["body"]);
            assert!(write(&p, &path, &[0], UNITS).is_err(), "{scale}");
        }
        assert_eq!(std::fs::read(path.as_path()).unwrap(), before);
    }

    #[test]
    fn a_thin_or_tiny_node_keeps_its_vertices_or_is_refused() {
        let p = project(Vec3::new(1e-2, 1.0, 1.0), meshes(), &["body"]);
        let read = round_trip(&p);
        let before = &p.model().node(NodeId(1)).mesh.as_ref().unwrap().vertices;
        let after = &read[1].as_ref().unwrap().vertices;
        // The child's own vertex, its last.
        let (a, b) = (before.last().unwrap(), after.last().unwrap());
        assert!((b.normal.length() - 1.0).abs() < 1e-5, "{}", b.normal);
        assert!(
            (b.position - a.position).abs().max_element() <= POSITION_TOLERANCE,
            "{} came back as {}",
            a.position,
            b.position
        );
        for scale in [Vec3::new(1e-5, 1.0, 1.0), Vec3::splat(1e-5)] {
            let p = project(scale, meshes(), &["body"]);
            assert!(
                matches!(
                    encode(&p, &["body.png".into()], UNITS),
                    Err(ModelFileError::Unwritable { node, .. }) if node == "child"
                ),
                "{scale}"
            );
        }
    }

    #[test]
    fn tiny_bone_scales_keep_their_normals() {
        let p = sample();
        let bytes = encode(&p, &["body.png".into()], UNITS).unwrap();
        let thin = edit_json(&bytes, |doc| {
            doc["nodes"][0]["scale"] = json!([1.0, 1.0, 1e-20]);
        });
        let meshes = decode(&p, &thin, UNITS).unwrap();
        for v in &meshes[0].as_ref().unwrap().vertices {
            assert!((v.normal.length() - 1.0).abs() < 1e-5, "{}", v.normal);
        }
    }

    #[test]
    fn an_empty_file_counts_as_missing() {
        let p = sample();
        let dir = tempfile::tempdir().unwrap();
        let path = ModelFilePath::new(dir.path().join("m.glb")).unwrap();
        std::fs::write(path.as_path(), "").unwrap();
        assert!(matches!(
            write_images(&p, &path, &[0]),
            Err(ModelFileError::NoModelFile(_))
        ));
        write(&p, &path, &[0], UNITS).unwrap();
    }

    #[test]
    fn a_file_with_no_faces_is_refused() {
        let p = sample();
        let bytes = encode(&p, &["body.png".into()], UNITS).unwrap();
        let empty = edit_json(&bytes, |doc| doc["meshes"][0]["primitives"] = json!([]));
        assert!(matches!(
            decode(&p, &empty, UNITS),
            Err(ModelFileError::NoMesh)
        ));
    }

    #[test]
    fn reading_refuses_what_isnt_a_regular_file() {
        let p = sample();
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("d.glb")).unwrap();
        let path = ModelFilePath::new(dir.path().join("d.glb")).unwrap();
        assert!(matches!(
            read(&p, &path, UNITS),
            Err(ModelFileError::NotModelFile(_))
        ));
    }

    #[test]
    fn malformed_glb_headers_are_errors_not_panics() {
        let p = sample();
        let mut short = b"glTF".to_vec();
        short.extend_from_slice(&2u32.to_le_bytes());
        short.extend_from_slice(&4u32.to_le_bytes());
        assert!(decode(&p, &short, UNITS).is_err());
    }

    #[test]
    fn bones_scaled_to_nothing_are_refused() {
        let p = sample();
        let bytes = encode(&p, &["body.png".into()], UNITS).unwrap();
        let flat = edit_json(&bytes, |doc| {
            doc["nodes"][1]["scale"] = json!([0.0, 0.0, 0.0]);
        });
        assert!(matches!(
            decode(&p, &flat, UNITS),
            Err(ModelFileError::Malformed { .. })
        ));
    }
}
