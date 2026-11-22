//! Builds a project from a villager in a built decomp checkout.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use glam::{EulerRot, Quat, UVec2, Vec2, Vec3};
use mint_core::{
    Corner, IndexedImage, Material, MaterialId, Mesh, Model, NodeId, Palette, Pose, Project, Rgba8,
    RgbaImage, Slot, Texture, TextureId, Transform, Triangle, Vertex,
};

use crate::anim;
use crate::c::{CError, CFile, Expr, Init};
use crate::gbi::{self, DrawState, RawVtx, TexSource};
use crate::gx;
use crate::{SLOT_BODY, SLOT_CLOTHING, SLOT_EYES, SLOT_MOUTH, TARGET};

/// The animation whose first frame poses imported models.
const POSE_ANIMATION: &str = "cKF_ba_r_npc_1_wait1";
const POSE_FILE: &str = "src/data/npc/npc_1_wait1.c";
const DRAW_DATA_FILE: &str = "src/data/npc/npc_draw_data.c";
const DRAW_DATA_TABLE: &str = "npc_draw_data_tbl";
const CLOTHING_PREVIEW: Rgba8 = Rgba8::new(200, 200, 210, 255);

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("{0} is not an ac-decomp checkout: {1} is missing")]
    NotDecomp(PathBuf, PathBuf),
    #[error(
        "{0} is missing, so the decomp has no extracted assets for {1}: check the version, or build the decomp once (`python configure.py && ninja`) with your disc image in orig/{1}"
    )]
    NotBuilt(PathBuf, String),
    #[error(transparent)]
    C(#[from] CError),
    #[error("{file}: {problem}")]
    Malformed { file: PathBuf, problem: String },
    #[error("{model}, joint {joint} ({dl}): {source}")]
    DisplayList {
        model: String,
        joint: usize,
        dl: String,
        source: gbi::DlError,
    },
    #[error("{model}: {problem}")]
    Model { model: String, problem: String },
    #[error(transparent)]
    Anim(#[from] anim::AnimError),
    #[error(transparent)]
    Project(#[from] mint_core::ProjectError),
    #[error(transparent)]
    Texture(#[from] mint_core::TextureError),
    #[error("{name}: {source}")]
    Gx { name: String, source: gx::GxError },
}

/// A built ac-decomp checkout.
#[derive(Clone, Debug)]
pub struct Decomp {
    root: PathBuf,
    version: String,
}

/// One entry of the villager table: a texture set drawn on a model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VillagerRef {
    /// Position in `npc_draw_data_tbl`.
    pub index: usize,
    /// Model name, such as `cat_1`.
    pub model: String,
    /// Texture set name, such as `cat_2`.
    pub texture_set: String,
    tmem: String,
    palette: String,
    eyes: Vec<String>,
    mouths: Vec<String>,
}

impl Decomp {
    /// Opens a checkout built for `version`, such as `GAFE01_00`.
    pub fn open(root: impl Into<PathBuf>, version: impl Into<String>) -> Result<Self, ImportError> {
        let d = Self {
            root: root.into(),
            version: version.into(),
        };
        let marker = d.root.join(DRAW_DATA_FILE);
        if !marker.is_file() {
            return Err(ImportError::NotDecomp(d.root.clone(), marker));
        }
        let assets = d.build_include().join("assets/npc");
        if !assets.is_dir() {
            return Err(ImportError::NotBuilt(assets, d.version.clone()));
        }
        Ok(d)
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn build_include(&self) -> PathBuf {
        self.root.join("build").join(&self.version).join("include")
    }

    fn include_dirs(&self) -> Vec<PathBuf> {
        vec![self.build_include(), self.root.join("include")]
    }

    fn read(&self, rel: impl AsRef<Path>) -> Result<CFile, ImportError> {
        Ok(CFile::read(&self.root.join(rel), &self.include_dirs())?)
    }

    /// Lists the villager table, in table order.
    pub fn villagers(&self) -> Result<Vec<VillagerRef>, ImportError> {
        let file = self.read(DRAW_DATA_FILE)?;
        parse_villagers(&file)
    }

    /// Imports one villager as a project.
    pub fn import(&self, v: &VillagerRef) -> Result<Project, ImportError> {
        let model_file = self.read(format!("src/data/npc/model/mdl/{}.c", v.model))?;
        let tex_file = self.read(format!("src/data/npc/model/tex/{}.c", v.texture_set))?;
        let pose_file = self.read(POSE_FILE)?;
        build_project(&model_file, &tex_file, &pose_file, v)
    }
}

fn malformed(file: &CFile, problem: impl Into<String>) -> ImportError {
    ImportError::Malformed {
        file: file.path.clone(),
        problem: problem.into(),
    }
}

fn ident_list(init: &Init) -> Option<Vec<String>> {
    init.as_list()?
        .iter()
        .map(|i| i.as_expr()?.as_ident().map(str::to_owned))
        .collect()
}

pub(crate) fn parse_villagers(file: &CFile) -> Result<Vec<VillagerRef>, ImportError> {
    let table = file
        .get(DRAW_DATA_TABLE)
        .and_then(|d| d.init.as_list())
        .ok_or_else(|| malformed(file, format!("{DRAW_DATA_TABLE} is missing")))?;
    let mut out = Vec::new();
    for (index, entry) in table.iter().enumerate() {
        let bad = |what: &str| malformed(file, format!("{DRAW_DATA_TABLE}[{index}]: {what}"));
        let fields = entry
            .as_list()
            .ok_or_else(|| bad("entry is not a brace list"))?;
        let Some(Expr::AddrOf(skel)) = fields.get(2).and_then(Init::as_expr) else {
            return Err(bad("third field is not &skeleton"));
        };
        let model = skel
            .as_ident()
            .and_then(|s| s.strip_prefix("cKF_bs_r_"))
            .ok_or_else(|| bad("skeleton name lacks cKF_bs_r_ prefix"))?
            .to_owned();
        let tex = fields
            .get(3)
            .and_then(Init::as_list)
            .ok_or_else(|| bad("texture data is not a brace list"))?;
        let name_at = |i: usize| tex.get(i).and_then(Init::as_expr).and_then(Expr::as_ident);
        let (Some(tmem), Some(palette)) = (name_at(0), name_at(1)) else {
            return Err(bad("texture or palette is not a name"));
        };
        let Some(texture_set) = tmem.strip_suffix("_tmem_txt") else {
            // Entries without a body texture (NULL) are not villagers.
            continue;
        };
        let names = |i: usize| {
            tex.get(i)
                .and_then(ident_list)
                .map(|v| v.into_iter().filter(|n| n != "NULL").collect::<Vec<_>>())
                .ok_or_else(|| bad("eye or mouth list is malformed"))
        };
        out.push(VillagerRef {
            index,
            model,
            texture_set: texture_set.to_owned(),
            tmem: tmem.to_owned(),
            palette: palette.to_owned(),
            eyes: names(2)?,
            mouths: names(3)?,
        });
    }
    Ok(out)
}

struct Joint {
    dl: Option<String>,
    children: usize,
    translation: [i64; 3],
}

fn joints(file: &CFile, model: &str) -> Result<Vec<Joint>, ImportError> {
    let skel_name = format!("cKF_bs_r_{model}");
    let skel = file
        .get(&skel_name)
        .and_then(|d| d.init.as_list())
        .ok_or_else(|| malformed(file, format!("{skel_name} is missing")))?;
    let table_name = skel
        .get(2)
        .and_then(Init::as_expr)
        .and_then(Expr::as_ident)
        .ok_or_else(|| malformed(file, format!("{skel_name} has no joint table")))?;
    let table = file
        .get(table_name)
        .and_then(|d| d.init.as_list())
        .ok_or_else(|| malformed(file, format!("{table_name} is missing")))?;
    table
        .iter()
        .enumerate()
        .map(|(i, j)| {
            let bad = || malformed(file, format!("{table_name}[{i}] is malformed"));
            let f = j.as_list().ok_or_else(bad)?;
            if f.len() != 6 {
                return Err(bad());
            }
            let dl = f[0].as_expr().and_then(Expr::as_ident).ok_or_else(bad)?;
            let int = |k: usize| f[k].as_expr().and_then(Expr::as_int).ok_or_else(bad);
            Ok(Joint {
                dl: (dl != "NULL").then(|| dl.to_owned()),
                children: int(1)? as usize,
                translation: [int(3)?, int(4)?, int(5)?],
            })
        })
        .collect()
}

/// Parents of each joint, from the depth-first child counts.
fn parents(joints: &[Joint]) -> Option<Vec<Option<usize>>> {
    let mut parents = vec![None; joints.len()];
    // Stack of (joint, children still to visit).
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for (i, j) in joints.iter().enumerate() {
        while let Some(&(_, 0)) = stack.last() {
            stack.pop();
        }
        match stack.last_mut() {
            Some((p, remaining)) => {
                parents[i] = Some(*p);
                *remaining -= 1;
            }
            None if i != 0 => return None,
            None => {}
        }
        stack.push((i, j.children));
    }
    stack.iter().all(|&(_, r)| r == 0).then_some(parents)
}

/// Names a joint after its display list, `<joint>_<species>_model`. The
/// species part is not always the model's own (`lrc_1` uses `Lfoot2_rcd_model`).
fn joint_name(dl: Option<&str>, index: usize) -> String {
    dl.and_then(|d| d.strip_suffix("_model"))
        .and_then(|d| d.rsplit_once('_'))
        .map_or_else(|| format!("joint {index}"), |(name, _)| name.to_owned())
}

fn bytes(file: &CFile, name: &str) -> Result<Vec<u8>, ImportError> {
    let ints = file
        .get(name)
        .and_then(|d| d.init.as_ints())
        .ok_or_else(|| malformed(file, format!("{name} is missing or not a byte list")))?;
    ints.into_iter()
        .map(|v| u8::try_from(v).map_err(|_| malformed(file, format!("{name} has non-byte {v}"))))
        .collect()
}

fn ci4_image(
    file: &CFile,
    name: &str,
    data: &[u8],
    size: UVec2,
) -> Result<IndexedImage, ImportError> {
    let pixels = gx::decode_ci4(data, size.x, size.y).map_err(|source| ImportError::Gx {
        name: name.to_owned(),
        source,
    })?;
    IndexedImage::new(size, pixels).map_err(|e| malformed(file, e.to_string()))
}

pub(crate) fn build_project(
    model_file: &CFile,
    tex_file: &CFile,
    pose_file: &CFile,
    v: &VillagerRef,
) -> Result<Project, ImportError> {
    let model_name = &v.model;
    let model_err = |problem: String| ImportError::Model {
        model: model_name.clone(),
        problem,
    };
    let joints = joints(model_file, model_name)?;
    let parents = parents(&joints)
        .filter(|p| !p.is_empty())
        .ok_or_else(|| model_err("joint child counts do not form a tree".into()))?;

    let vertex_array = format!("{model_name}_v");
    let raw_vertices: Vec<RawVtx> = model_file
        .get(&vertex_array)
        .and_then(|d| d.init.as_list())
        .ok_or_else(|| malformed(model_file, format!("{vertex_array} is missing")))?
        .iter()
        .enumerate()
        .map(|(i, init)| {
            RawVtx::from_init(init)
                .ok_or_else(|| malformed(model_file, format!("{vertex_array}[{i}] is not a Vtx")))
        })
        .collect::<Result<_, _>>()?;
    // Flag 0 vertices are transformed by the matrix current when they load,
    // which is what seam detection relies on.
    if let Some((i, v)) = raw_vertices.iter().enumerate().find(|(_, v)| v.flag != 0) {
        return Err(model_err(format!(
            "{vertex_array}[{i}] has flag {}; only flag 0 vertices are supported",
            v.flag
        )));
    }

    // Interpret every joint's display list.
    let mut dl_meshes = Vec::with_capacity(joints.len());
    for (i, j) in joints.iter().enumerate() {
        let Some(dl) = &j.dl else {
            dl_meshes.push(None);
            continue;
        };
        let list = model_file
            .get(dl)
            .and_then(|d| d.init.as_list())
            .ok_or_else(|| malformed(model_file, format!("display list {dl} is missing")))?;
        let mesh = gbi::interpret(list, &vertex_array, raw_vertices.len()).map_err(|source| {
            ImportError::DisplayList {
                model: model_name.clone(),
                joint: i,
                dl: dl.clone(),
                source,
            }
        })?;
        dl_meshes.push(Some(mesh));
    }
    let slots = gbi::matrix_slots(&joints.iter().map(|j| j.dl.is_some()).collect::<Vec<_>>());

    // Textures, one per distinct source.
    let mut loads: BTreeMap<TexSource, UVec2> = BTreeMap::new();
    for mesh in dl_meshes.iter().flatten() {
        for t in &mesh.triangles {
            let size = UVec2::new(t.state.tex.width, t.state.tex.height);
            if let Some(prev) = loads.insert(t.state.tex.source, size)
                && prev != size
            {
                return Err(model_err(format!(
                    "{} texture is loaded as both {prev} and {size}",
                    t.state.tex.source
                )));
            }
        }
    }
    let palette_ints = tex_file
        .get(&v.palette)
        .and_then(|d| d.init.as_ints())
        .ok_or_else(|| malformed(tex_file, format!("{} is missing", v.palette)))?;
    let palette = Palette::from_colors(palette_ints.iter().map(|&c| gx::decode_rgb5a3(c as u16)))
        .map_err(|e| malformed(tex_file, e.to_string()))?;
    let tmem = bytes(tex_file, &v.tmem)?;

    let mut textures = Vec::new();
    let mut texture_ids: HashMap<TexSource, TextureId> = HashMap::new();
    let mut body_ranges: Vec<(u32, u32)> = Vec::new();
    for (&source, &size) in &loads {
        let texture = match source {
            TexSource::Eye | TexSource::Mouth => {
                let (names, slot) = if source == TexSource::Eye {
                    (&v.eyes, SLOT_EYES)
                } else {
                    (&v.mouths, SLOT_MOUTH)
                };
                let variants = names
                    .iter()
                    .enumerate()
                    .map(|(k, name)| {
                        let img = ci4_image(tex_file, name, &bytes(tex_file, name)?, size)?;
                        Ok((format!("{}", k + 1), img))
                    })
                    .collect::<Result<Vec<_>, ImportError>>()?;
                Texture::painted(slot, Slot(slot.into()), size, variants)?
            }
            TexSource::Clothing => Texture::external(
                SLOT_CLOTHING,
                Slot(SLOT_CLOTHING.into()),
                RgbaImage::filled(size, CLOTHING_PREVIEW),
            ),
            TexSource::Body { offset } => {
                let len = gx::ci4_len(size.x, size.y) as u32;
                let end = offset.checked_add(len).ok_or_else(|| {
                    model_err(format!("body texture offset {offset:#x} is out of range"))
                })?;
                if let Some(&(o, e)) = body_ranges.iter().find(|&&(o, e)| offset < e && o < end) {
                    return Err(model_err(format!(
                        "body textures at {offset:#x}..{end:#x} and {o:#x}..{e:#x} overlap"
                    )));
                }
                body_ranges.push((offset, end));
                let data = tmem.get(offset as usize..end as usize).ok_or_else(|| {
                    malformed(
                        tex_file,
                        format!("{} is shorter than {end:#x} bytes", v.tmem),
                    )
                })?;
                let name = format!("{SLOT_BODY} {offset:#05x}");
                let img = ci4_image(tex_file, &name, data, size)?;
                Texture::painted(name, Slot(SLOT_BODY.into()), size, [("1".to_owned(), img)])?
            }
        };
        texture_ids.insert(source, TextureId(textures.len() as u32));
        textures.push(texture);
    }

    // Nodes and meshes.
    let mut model = Model::new(
        joint_name(joints[0].dl.as_deref(), 0),
        translation(&joints[0]),
    );
    for (i, j) in joints.iter().enumerate().skip(1) {
        let parent = NodeId(parents[i].expect("non-root joints have parents") as u32);
        let id = model.add_child(parent, joint_name(j.dl.as_deref(), i), translation(j));
        debug_assert_eq!(id.index(), i);
    }
    let mut materials: Vec<Material> = Vec::new();
    let mut material_ids: HashMap<DrawState, MaterialId> = HashMap::new();
    for (i, dl_mesh) in dl_meshes.iter().enumerate() {
        let Some(dl_mesh) = dl_mesh else { continue };
        let mut mesh = Mesh::default();
        let mut local: HashMap<usize, (u32, Option<u32>)> = HashMap::new();
        for t in &dl_mesh.triangles {
            let material = *material_ids.entry(t.state).or_insert_with(|| {
                materials.push(Material {
                    texture: texture_ids[&t.state.tex.source],
                    wrap: t.state.tex.wrap,
                    alpha: t.state.alpha,
                    double_sided: t.state.double_sided,
                    tint: t.state.tint,
                });
                MaterialId(materials.len() as u32 - 1)
            });
            let size = Vec2::new(t.state.tex.width as f32, t.state.tex.height as f32);
            let mut corners = [Corner {
                vertex: 0,
                uv: Vec2::ZERO,
            }; 3];
            for (c, used) in corners.iter_mut().zip(t.vertices) {
                let followed = match used.matrix {
                    None => i,
                    Some(slot) => *slots.get(&slot).ok_or_else(|| {
                        model_err(format!("joint {i} loads unknown matrix slot {slot}"))
                    })?,
                };
                let follows = (followed != i).then_some(NodeId(followed as u32));
                let (index, prev) = *local.entry(used.index).or_insert_with(|| {
                    let raw = raw_vertices[used.index];
                    mesh.vertices.push(Vertex {
                        position: Vec3::from(raw.pos.map(f32::from)),
                        normal: Vec3::from(raw.normal.map(|n| f32::from(n) / 127.0)),
                        follows,
                    });
                    (mesh.vertices.len() as u32 - 1, follows.map(|n| n.0))
                });
                if prev != follows.map(|n| n.0) {
                    return Err(model_err(format!(
                        "vertex {} is loaded under two different joint matrices",
                        used.index
                    )));
                }
                let raw = raw_vertices[used.index];
                *c = Corner {
                    vertex: index,
                    uv: Vec2::new(f32::from(raw.st[0]), f32::from(raw.st[1])) / 32.0 / size,
                };
            }
            mesh.triangles.push(Triangle { corners, material });
        }
        model.node_mut(NodeId(i as u32)).mesh = Some(mesh);
    }

    // Display pose from the shared idle animation.
    let rotations = anim::sample(pose_file, POSE_ANIMATION, joints.len(), 1.0)?;
    let mut pose = Pose::default();
    for (i, [x, y, z]) in rotations.iter().enumerate() {
        pose.set_rotation(
            NodeId(i as u32),
            Quat::from_euler(
                EulerRot::ZYX,
                z.to_radians(),
                y.to_radians(),
                x.to_radians(),
            ),
        );
    }

    Ok(Project::new(
        mint_core::TargetId(TARGET.into()),
        palette,
        model,
        textures,
        materials,
        pose,
    )?)
}

fn translation(j: &Joint) -> Transform {
    Transform::from_translation(Vec3::from(j.translation.map(|v| v as f32)))
}

/// Special NPC models the importer refuses: unsupported combiners or
/// commands (fob, gst, hem) and a custom-design eye palette slot (mka).
#[cfg(test)]
pub(crate) const SPECIAL_NPC_MODELS: [&str; 4] = ["fob_1", "gst_1", "hem_1", "mka_1"];

#[cfg(test)]
mod tests {
    use super::*;

    fn joint(children: usize) -> Joint {
        Joint {
            dl: None,
            children,
            translation: [0; 3],
        }
    }

    #[test]
    fn parents_follow_depth_first_child_counts() {
        // 0 -> {1 -> {2}, 3}
        let js = [joint(2), joint(1), joint(0), joint(0)];
        assert_eq!(parents(&js), Some(vec![None, Some(0), Some(1), Some(0)]));
    }

    #[test]
    fn parents_reject_inconsistent_counts() {
        assert_eq!(parents(&[joint(2), joint(0)]), None);
        assert_eq!(parents(&[joint(0), joint(0)]), None);
    }

    /// Parses every model source in a decomp checkout named by
    /// `MINT_AC_DECOMP`, without extracted assets, and checks that only the
    /// known special NPCs are refused.
    #[test]
    #[ignore = "needs an ac-decomp checkout in MINT_AC_DECOMP"]
    fn all_model_sources_interpret() {
        let root = PathBuf::from(std::env::var("MINT_AC_DECOMP").expect("MINT_AC_DECOMP"));
        let dir = root.join("src/data/npc/model/mdl");
        let mut refused = BTreeMap::new();
        let mut count = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let model = path.file_stem().unwrap().to_str().unwrap().to_owned();
            let text: String = std::fs::read_to_string(&path)
                .unwrap()
                .lines()
                .filter(|l| !l.trim_start().starts_with("#include"))
                .fold(String::new(), |mut s, l| {
                    s.push_str(l);
                    s.push('\n');
                    s
                });
            let file = CFile::parse_str(&path, &text).unwrap();
            let js = joints(&file, &model).unwrap();
            assert!(parents(&js).is_some(), "{model}");
            // Export relies on every model sharing this skeleton.
            assert_eq!(
                js.iter().map(|j| j.children).collect::<Vec<_>>(),
                crate::VILLAGER_JOINT_CHILDREN.map(usize::from),
                "{model}"
            );
            count += 1;
            for j in &js {
                let Some(dl) = &j.dl else { continue };
                let list = file.get(dl).unwrap().init.as_list().unwrap();
                if let Err(e) = gbi::interpret(list, &format!("{model}_v"), usize::MAX / 2) {
                    refused
                        .entry(model.clone())
                        .or_insert_with(|| format!("{dl}: {e}"));
                }
            }
        }
        eprintln!("{count} models; refused: {refused:#?}");
        assert_eq!(count, 72);
        let refused: Vec<_> = refused.keys().map(String::as_str).collect();
        assert_eq!(refused, SPECIAL_NPC_MODELS);
    }

    #[test]
    #[ignore = "needs an ac-decomp checkout in MINT_AC_DECOMP"]
    fn villager_table_and_pose_parse() {
        let root = PathBuf::from(std::env::var("MINT_AC_DECOMP").expect("MINT_AC_DECOMP"));
        let table = CFile::read(&root.join(DRAW_DATA_FILE), &[]).unwrap();
        let villagers = parse_villagers(&table).unwrap();
        eprintln!("{} entries with texture sets", villagers.len());
        let cat = villagers.iter().find(|v| v.texture_set == "cat_2").unwrap();
        assert_eq!(cat.model, "cat_1");
        assert_eq!(cat.eyes.len(), 8);
        assert_eq!(cat.mouths.len(), 6);

        let pose = CFile::read(&root.join(POSE_FILE), &[]).unwrap();
        let rotations = anim::sample(&pose, POSE_ANIMATION, 26, 1.0).unwrap();
        assert_eq!(rotations.len(), 26);
        eprintln!("{rotations:?}");
    }

    #[test]
    fn joint_names_come_from_display_lists() {
        assert_eq!(joint_name(Some("head_cat_model"), 24), "head");
        assert_eq!(joint_name(Some("Lfoot2_rcd_model"), 4), "Lfoot2");
        assert_eq!(joint_name(None, 5), "joint 5");
    }
}
