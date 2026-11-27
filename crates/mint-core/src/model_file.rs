//! The model file: a copy of a project's model in glTF binary (`.glb`) for
//! editing in another program, such as Blender, and reading it back.
//!
//! The file holds one mesh skinned to a skeleton of the model's nodes, posed
//! in the project's display pose, which becomes the rest pose in the other
//! program. Each vertex is weighted fully to the node it follows, and
//! vertices that meet across a seam are one vertex, so editing can't open the
//! seam. Its materials are named after the textures they draw with and show
//! PNG images of those textures, written beside the file.
//!
//! Reading takes the meshes back into the project's own node tree, which a
//! file can't change: each triangle belongs to the deepest node among its
//! corners', and corners that follow another node become seam vertices.
//! Whatever else the file holds, such as images and animations, is ignored.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use glam::{Mat3, Mat4, Vec2, Vec3};
use serde_json::{Value, json};

use crate::file::write_atomic;
use crate::{
    AlphaMode, Corner, Mesh, Model, NodeId, Project, Rgba8, TextureContent, Triangle, Vertex, Wrap,
};

/// The extension of a model file.
pub const EXTENSION: &str = "glb";

#[derive(Debug, thiserror::Error)]
pub enum ModelFileError {
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("{0}: {1}")]
    Png(PathBuf, png::EncodingError),
    #[error("two nodes are named {0:?}; a model file names each bone after its node")]
    DuplicateNode(String),
    #[error("not a glTF binary file: {0}")]
    Gltf(#[from] gltf::Error),
    #[error("the model file has no meshes")]
    NoMesh,
    #[error("the model file's nodes form a loop rather than a tree")]
    Cycle,
    #[error("the model file keeps its data in a separate file; export it as glTF Binary (.glb)")]
    ExternalData,
    #[error(
        "object {0:?} isn't skinned; give it an Armature modifier and put each vertex in one bone's vertex group"
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
        "object {object:?} has {count} vertices weighted to no bone or to more than one, such as one at ({x:.3}, {y:.3}, {z:.3}); weight each vertex to exactly one bone",
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
/// its texture, made unique with the material's index where two share it.
#[must_use]
pub fn material_names(project: &Project) -> Vec<String> {
    let mut taken = HashSet::new();
    project
        .materials()
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let name = &project.texture(m.texture).name;
            let name = if taken.contains(name) {
                format!("{name} #{i}")
            } else {
                name.clone()
            };
            taken.insert(name.clone());
            name
        })
        .collect()
}

/// `name` with every character but ASCII letters, digits, `-`, and `_`
/// replaced, so it is safe in a file name and a URI.
fn file_safe(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Where the image of each of `project`'s textures goes beside the model
/// file at `path`: `<stem>.<texture>.png`, with the texture's index added
/// where two names meet.
#[must_use]
pub fn image_paths(project: &Project, path: &Path) -> Vec<PathBuf> {
    let stem = file_safe(&path.file_stem().unwrap_or_default().to_string_lossy());
    let mut taken = HashSet::new();
    project
        .textures()
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let base = format!("{stem}.{}", file_safe(&t.name));
            let name = if taken.contains(&base) {
                format!("{base}.{i}.png")
            } else {
                format!("{base}.png")
            };
            taken.insert(base);
            path.with_file_name(name)
        })
        .collect()
}

/// Writes `project`'s model to the model file at `path` and the images its
/// materials show beside it. `variants` gives the variant shown for each
/// texture, and `meters_per_unit` the size of one model unit in the file,
/// whose units are meters.
pub fn write(
    project: &Project,
    path: &Path,
    variants: &[usize],
    meters_per_unit: f32,
) -> Result<(), ModelFileError> {
    let images = image_paths(project, path);
    let uris: Vec<String> = images
        .iter()
        .map(|p| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    let bytes = encode(project, &uris, meters_per_unit)?;
    write_images(project, path, variants)?;
    write_atomic(path, &bytes).map_err(|e| ModelFileError::Io(path.to_owned(), e))
}

/// Writes the images the model file at `path` shows for `project`'s
/// textures, each in its variant given by `variants`.
pub fn write_images(
    project: &Project,
    path: &Path,
    variants: &[usize],
) -> Result<(), ModelFileError> {
    for ((i, t), image) in project
        .textures()
        .iter()
        .enumerate()
        .zip(image_paths(project, path))
    {
        let (size, pixels): (_, Vec<Rgba8>) = match t.content() {
            TextureContent::Painted(v) => {
                let v = &v[variants.get(i).copied().unwrap_or(0).min(v.len() - 1)];
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
        write_atomic(&image, &png).map_err(|e| ModelFileError::Io(image.clone(), e))?;
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

/// Each node's model-space matrix in the display pose.
fn posed(project: &Project) -> Vec<Mat4> {
    project.model().world_matrices(project.pose())
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
    /// Adds an accessor over `bytes`, which hold `count` elements of `kind`.
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
    ) -> usize {
        let bytes: Vec<u8> = values
            .iter()
            .flatten()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        self.accessor(&bytes, FLOAT, kind, values.len(), target, json!({}))
    }
}

/// A vertex of the file's mesh.
#[derive(Default)]
struct Vertices {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    joints: Vec<u16>,
    /// Index of each vertex by its node and attribute bits, which welds
    /// vertices that meet across a seam.
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
            self.joints.push(node.0 as u16);
            self.positions.len() as u32 - 1
        })
    }
}

/// The matrix that carries normals the way `m` carries points.
fn normal_matrix(m: Mat4) -> Mat3 {
    Mat3::from_mat4(m).inverse().transpose()
}

/// `n` turned by normal matrix `m`, keeping its length, which a scale in
/// `m` would change.
fn carry_normal(m: Mat3, n: Vec3) -> Vec3 {
    let t = m * n;
    let len = t.length();
    if len > 0.0 { t * (n.length() / len) } else { t }
}

/// Encodes `project`'s model as a glTF binary whose materials show the
/// images at `uris`, one per texture, relative to the file.
fn encode(
    project: &Project,
    uris: &[String],
    meters_per_unit: f32,
) -> Result<Vec<u8>, ModelFileError> {
    let model = project.model();
    let mut names = HashSet::new();
    for n in model.nodes() {
        if !names.insert(&n.name) {
            return Err(ModelFileError::DuplicateNode(n.name.clone()));
        }
    }
    let s = meters_per_unit;
    let world = posed(project);
    let normals: Vec<Mat3> = world.iter().copied().map(normal_matrix).collect();

    // Triangles in drawing order: one primitive per run of a node's
    // triangles that share a material, so reading keeps the order.
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
                // Kept at their own length rather than normalized, so the
                // file reads back exactly; some targets store shorter
                // normals, such as AC's at 120/127.
                let n = carry_normal(normals[f.index()], v.normal);
                run.push(verts.add(f, p, n, c.uv));
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
    let position = {
        let bytes: Vec<u8> = verts
            .positions
            .iter()
            .flatten()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        bin.accessor(
            &bytes,
            FLOAT,
            "VEC3",
            verts.positions.len(),
            Some(ARRAY_BUFFER),
            json!({"min": min.to_array(), "max": max.to_array()}),
        )
    };
    let normal = bin.floats(&verts.normals, "VEC3", Some(ARRAY_BUFFER));
    let uv = bin.floats(&verts.uvs, "VEC2", Some(ARRAY_BUFFER));
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

    // The skeleton, posed, with translations in the file's units.
    let mut nodes: Vec<Value> = model
        .ids()
        .map(|id| {
            let n = model.node(id);
            let local = n.transform.matrix() * Mat4::from_quat(project.pose().rotation(id));
            let (scale, rotation, translation) = local.to_scale_rotation_translation();
            let children: Vec<u32> = model.children(id).map(|c| c.0).collect();
            let mut node = json!({
                "name": n.name,
                "translation": (translation * s).to_array(),
                "rotation": rotation.to_array(),
                "scale": scale.to_array(),
            });
            if !children.is_empty() {
                node["children"] = json!(children);
            }
            node
        })
        .collect();
    let ibms: Vec<[f32; 16]> = world
        .iter()
        .map(|m| {
            let mut m = *m;
            m.w_axis = (m.w_axis.truncate() * s).extend(1.0);
            m.inverse().to_cols_array()
        })
        .collect();
    let ibm = bin.floats(&ibms, "MAT4", None);
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
    out.extend_from_slice(b"glTF");
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

/// Reads the model file at `path` as a new model for `project`: its node
/// tree with the file's meshes. `meters_per_unit` must be what the file was
/// written with.
pub fn read(project: &Project, path: &Path, meters_per_unit: f32) -> Result<Model, ModelFileError> {
    let bytes = std::fs::read(path).map_err(|e| ModelFileError::Io(path.to_owned(), e))?;
    decode(project, &bytes, meters_per_unit)
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

/// Decodes a glTF binary as a new model for `project`.
fn decode(project: &Project, bytes: &[u8], meters_per_unit: f32) -> Result<Model, ModelFileError> {
    let gltf = gltf::Gltf::from_slice(bytes)?;
    let blob = gltf.blob.as_deref();
    let buffer = |b: gltf::Buffer<'_>| match b.source() {
        gltf::buffer::Source::Bin => blob,
        gltf::buffer::Source::Uri(_) => None,
    };
    if gltf
        .buffers()
        .any(|b| matches!(b.source(), gltf::buffer::Source::Uri(_)))
    {
        return Err(ModelFileError::ExternalData);
    }

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

    let model = project.model();
    let by_name: HashMap<&str, NodeId> = model
        .ids()
        .map(|id| (model.node(id).name.as_str(), id))
        .collect();
    let mut depth = vec![0usize; model.nodes().len()];
    for id in model.ids() {
        if let Some(p) = model.node(id).parent() {
            depth[id.index()] = depth[p.index()] + 1;
        }
    }
    let materials: HashMap<String, u32> = material_names(project)
        .into_iter()
        .enumerate()
        .map(|(i, n)| (n, i as u32))
        .collect();
    let world = posed(project);
    let to_local: Vec<Mat4> = world.iter().map(|m| m.inverse()).collect();
    // Normals carry back by the inverse of the normal matrix, the
    // transpose of the point matrix.
    let to_local_normal: Vec<Mat3> = world
        .iter()
        .map(|m| Mat3::from_mat4(*m).transpose())
        .collect();

    let mut meshes: Vec<Option<Mesh>> = vec![None; model.nodes().len()];
    let mut welds: Vec<HashMap<(u32, [u32; 6]), u32>> = vec![HashMap::new(); model.nodes().len()];
    let mut any = false;
    for node in gltf.nodes() {
        let Some(mesh) = node.mesh() else { continue };
        any = true;
        let object = node
            .name()
            .or(mesh.name())
            .map_or_else(|| format!("node {}", node.index()), str::to_owned);
        let skin = node
            .skin()
            .ok_or_else(|| ModelFileError::Unskinned(object.clone()))?;
        let ibms: Vec<Mat4> = skin
            .reader(buffer)
            .read_inverse_bind_matrices()
            .map_or_else(Vec::new, |r| {
                r.map(|m| Mat4::from_cols_array_2d(&m)).collect()
            });
        // Each joint's bind-space-to-file matrix and project node, or the
        // bone's name where the project has no such node.
        let joints: Vec<Result<(Mat4, NodeId), String>> = skin
            .joints()
            .enumerate()
            .map(|(j, jn)| {
                let name = jn.name().unwrap_or_default();
                let id = by_name.get(name).copied().ok_or_else(|| name.to_owned())?;
                let ibm = ibms.get(j).copied().unwrap_or(Mat4::IDENTITY);
                Ok((global[jn.index()] * ibm, id))
            })
            .collect();

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
            let reader = prim.reader(buffer);
            let missing = |attribute| ModelFileError::MissingAttribute {
                object: object.clone(),
                attribute,
            };
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
                .read_tex_coords(0)
                .ok_or_else(|| missing("UV map"))?
                .into_f32()
                .map(Vec2::from)
                .collect();
            if uvs.len() != positions.len() || normals.len() != positions.len() {
                return Err(missing("UV and normal for every vertex"));
            }
            // The one joint each vertex follows, or `None` for none or many.
            let mut bone: Vec<Option<usize>> = vec![None; positions.len()];
            let mut weighted = vec![0u32; positions.len()];
            for set in 0.. {
                let (Some(j), Some(w)) = (reader.read_joints(set), reader.read_weights(set)) else {
                    break;
                };
                for (v, (j, w)) in j
                    .into_u16()
                    .zip(w.into_f32())
                    .enumerate()
                    .take(positions.len())
                {
                    for (j, w) in j.into_iter().zip(w) {
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

            for tri in indices.chunks_exact(3) {
                let mut corners = [(NodeId::ROOT, Vec3::ZERO, Vec3::ZERO, Vec2::ZERO); 3];
                for (c, &v) in corners.iter_mut().zip(tri) {
                    let v = v as usize;
                    let j = bone[v].expect("checked above");
                    let (m, id) = joints
                        .get(j)
                        .cloned()
                        .unwrap_or_else(|| Err(format!("joint {j}")))
                        .map_err(|bone| ModelFileError::UnknownBone {
                            object: object.clone(),
                            bone,
                        })?;
                    let p = m.transform_point3(positions[v]) / meters_per_unit;
                    let n = carry_normal(normal_matrix(m), normals[v]);
                    *c = (
                        id,
                        to_local[id.index()].transform_point3(p),
                        carry_normal(to_local_normal[id.index()], n),
                        uvs[v],
                    );
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
                mesh.triangles.push(Triangle {
                    corners,
                    material: crate::MaterialId(material),
                });
            }
        }
    }
    if !any {
        return Err(ModelFileError::NoMesh);
    }
    let mut out = model.clone();
    for (id, mesh) in model.ids().zip(meshes) {
        out.node_mut(id).mesh = mesh;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use glam::{Quat, UVec2};

    use super::*;
    use crate::{
        ExportTarget, IndexedImage, Material, MaterialId, Palette, PaletteIndex, Pose, Slot,
        Texture, TextureId, Transform, Variant,
    };

    /// A root with a child turned by the pose, each with a triangle, the
    /// child's sharing two corners with the root's as seam vertices.
    fn sample() -> Project {
        let mut model = Model::new(
            "root",
            Transform::from_translation(Vec3::new(0.0, 100.0, 0.0)),
        );
        let child = model.add_child(
            NodeId::ROOT,
            "child",
            Transform::from_translation(Vec3::new(50.0, 0.0, 0.0)),
        );
        let corner = |vertex, u, v| Corner {
            vertex,
            uv: Vec2::new(u, v),
        };
        model.node_mut(NodeId::ROOT).mesh = Some(Mesh {
            vertices: vec![
                Vertex {
                    position: Vec3::ZERO,
                    normal: Vec3::Z,
                    follows: None,
                },
                Vertex {
                    position: Vec3::new(50.0, 0.0, 0.0),
                    normal: Vec3::Z,
                    follows: None,
                },
                Vertex {
                    position: Vec3::new(50.0, 10.0, 0.0),
                    normal: Vec3::Z,
                    follows: None,
                },
            ],
            triangles: vec![Triangle {
                corners: [
                    corner(0, 0.0, 0.0),
                    corner(1, 0.5, 0.0),
                    corner(2, 0.5, 0.5),
                ],
                material: MaterialId(0),
            }],
        });
        model.node_mut(child).mesh = Some(Mesh {
            vertices: vec![
                Vertex {
                    position: Vec3::new(50.0, 0.0, 0.0),
                    normal: Vec3::Z,
                    follows: Some(NodeId::ROOT),
                },
                Vertex {
                    position: Vec3::new(50.0, 10.0, 0.0),
                    normal: Vec3::Z,
                    follows: Some(NodeId::ROOT),
                },
                Vertex {
                    position: Vec3::new(20.0, 5.0, 0.0),
                    normal: Vec3::Y,
                    follows: None,
                },
            ],
            triangles: vec![
                Triangle {
                    corners: [
                        corner(0, 0.5, 0.0),
                        corner(1, 0.5, 0.5),
                        corner(2, 1.0, 0.25),
                    ],
                    material: MaterialId(0),
                },
                Triangle {
                    corners: [
                        corner(2, 1.0, 0.25),
                        corner(1, 0.5, 0.5),
                        corner(0, 0.5, 0.0),
                    ],
                    material: MaterialId(1),
                },
            ],
        });
        let mut pose = Pose::default();
        pose.set_rotation(child, Quat::from_rotation_z(0.5));
        let palette = Palette::from_colors([Rgba8::new(10, 20, 30, 255)]).unwrap();
        let image = IndexedImage::new(UVec2::new(2, 2), vec![PaletteIndex(0); 4]).unwrap();
        let texture = Texture::painted(
            "body",
            Slot("body".into()),
            UVec2::new(2, 2),
            [Variant::imported("1", image)],
        )
        .unwrap();
        let material = Material {
            texture: TextureId(0),
            wrap: [Wrap::Clamp, Wrap::Repeat],
            alpha: AlphaMode::Opaque,
            double_sided: false,
            tint: Rgba8::new(255, 255, 255, 255),
        };
        let cutout = Material {
            alpha: AlphaMode::Cutout,
            ..material
        };
        Project::new(
            ExportTarget::new("test"),
            palette,
            model,
            vec![texture],
            vec![material, cutout],
            pose,
        )
        .unwrap()
    }

    /// Positions rounded to whole units and normals to hundredths, by node.
    fn summary(model: &Model) -> Vec<Vec<String>> {
        model
            .ids()
            .map(|id| {
                let Some(m) = &model.node(id).mesh else {
                    return Vec::new();
                };
                m.triangles
                    .iter()
                    .map(|t| {
                        let mut s = format!("m{}", t.material.0);
                        for c in t.corners {
                            let v = m.vertices[c.vertex as usize];
                            let _ = write!(
                                s,
                                " p{:?} n{:?} f{} uv{:?}",
                                v.position.round().to_array(),
                                (v.normal * 100.0).round().to_array(),
                                v.followed(id).0,
                                c.uv.to_array()
                            );
                        }
                        s
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn round_trip_keeps_triangles_seams_and_order() {
        let p = sample();
        let bytes = encode(&p, &["body.png".into()], 0.01).unwrap();
        let model = decode(&p, &bytes, 0.01).unwrap();
        assert_eq!(summary(&model), summary(p.model()));
        // The root keeps its own vertices; the child's two seam vertices
        // follow the root.
        let child = model.node(NodeId(1)).mesh.as_ref().unwrap();
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
    fn seam_vertices_weld_into_one() {
        let p = sample();
        let bytes = encode(&p, &["body.png".into()], 1.0).unwrap();
        let gltf = gltf::Gltf::from_slice(&bytes).unwrap();
        let count = gltf.accessors().next().unwrap().count();
        // The root's three vertices, which the child's seam vertices share,
        // and the child's own vertex.
        assert_eq!(count, 4);
    }

    #[test]
    fn reading_refuses_unknown_bones_and_materials() {
        let p = sample();
        let bytes = encode(&p, &["body.png".into()], 1.0).unwrap();

        let mut renamed = p.model().clone();
        renamed.node_mut(NodeId(1)).name = "arm".into();
        let mut other = p.clone();
        other.set_model(renamed).unwrap();
        assert!(matches!(
            decode(&other, &bytes, 1.0),
            Err(ModelFileError::UnknownBone { bone, .. }) if bone == "child"
        ));

        let texture = Texture::painted(
            "skin",
            Slot("body".into()),
            UVec2::new(2, 2),
            p.textures()[0].variants().to_vec(),
        )
        .unwrap();
        let other = Project::new(
            p.target().clone(),
            p.palette().clone(),
            p.model().clone(),
            vec![texture],
            p.materials().to_vec(),
            p.pose().clone(),
        )
        .unwrap();
        assert!(matches!(
            decode(&other, &bytes, 1.0),
            Err(ModelFileError::UnknownMaterial { material, known, .. })
                if material == "body" && known == "skin, skin #1"
        ));
    }

    #[test]
    fn materials_are_named_after_textures() {
        assert_eq!(material_names(&sample()), ["body", "body #1"]);
    }

    #[test]
    fn blender_suffixes_match_their_material() {
        assert_eq!(strip_blender_suffix("body #1.002"), "body #1");
        assert_eq!(strip_blender_suffix("body.v2"), "body.v2");
    }

    #[test]
    fn image_paths_sit_beside_the_file() {
        let p = sample();
        assert_eq!(
            image_paths(&p, Path::new("/x/my villager.glb")),
            [PathBuf::from("/x/my_villager.body.png")]
        );
    }

    #[test]
    fn duplicate_node_names_are_refused() {
        let p = sample();
        let mut model = p.model().clone();
        model.node_mut(NodeId(1)).name = "root".into();
        let mut p2 = p.clone();
        p2.set_model(model).unwrap();
        assert!(
            matches!(encode(&p2, &["a".into()], 1.0), Err(ModelFileError::DuplicateNode(n)) if n == "root")
        );
    }

    #[test]
    fn writes_the_file_and_its_images() {
        let p = sample();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.glb");
        write(&p, &path, &[0], 0.01).unwrap();
        assert!(dir.path().join("m.body.png").is_file());
        assert_eq!(summary(&read(&p, &path, 0.01).unwrap()), summary(p.model()));
    }
}
