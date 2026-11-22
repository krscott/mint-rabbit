//! Writes a project as decomp C sources: a model file (vertices, display
//! lists, skeleton) and a texture file (palette, eye, mouth, and body
//! textures), plus the villager table entry to paste.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use mint_core::{
    AlphaMode, MaterialId, NodeId, Project, Slot, Texture, TextureContent, TextureId, Wrap,
};

use crate::gbi::VERTEX_CACHE;
use crate::gx;
use crate::{
    OPTION_MODEL, OPTION_TEXTURE_SET, SLOT_BODY, SLOT_CLOTHING, SLOT_EYES, SLOT_MOUTH, TARGET,
    VillagerRef,
};

/// Most triangles one `gsSPNTrianglesInit_5b` group can count.
const MAX_GROUP_TRIANGLES: usize = 128;
const EYE_VARIANTS: usize = 8;
const MOUTH_VARIANTS: usize = 6;
const PALETTE_LEN: usize = 16;

/// Child counts of the skeleton every villager model shares, in depth-first
/// order. The shared animations and the game's per-joint draw hooks assume
/// exactly this tree.
pub const VILLAGER_JOINT_CHILDREN: [u8; 26] = [
    1, 4, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 0, 3, 1, 1, 0, 1, 1, 1, 0, 2, 1, 0, 1, 0,
];

/// One vertex use in a display list: the mesh vertex and its texel
/// coordinates.
type CornerKey = (u32, [i16; 2]);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ExportError {
    #[error("the project targets {0:?}, not {TARGET:?}")]
    WrongTarget(String),
    #[error(
        "the project has no {0:?} export target option; texture-only export needs a project imported from a decomp"
    )]
    MissingOption(&'static str),
    #[error("{0:?} is not a C identifier")]
    BadName(String),
    #[error(
        "the node tree does not match the villager skeleton (26 joints with fixed children): {0}"
    )]
    Skeleton(String),
    #[error("the palette has {0} colors; AC villagers allow at most 16")]
    PaletteTooLarge(usize),
    #[error("texture {texture:?} has slot {slot:?}, which AC villagers do not have")]
    UnknownSlot { texture: String, slot: String },
    #[error("there are two {0} textures; AC villagers have one")]
    DuplicateSlot(&'static str),
    #[error("the {slot} texture has {actual} variants; AC villagers need {expected}")]
    VariantCount {
        slot: &'static str,
        expected: usize,
        actual: usize,
    },
    #[error("texture {texture:?}: {source}")]
    Gx {
        texture: String,
        source: gx::GxError,
    },
    #[error("a body texture would start at {0:#x}; texture addresses hold offsets below 0x1000000")]
    BodyTooLarge(usize),
    #[error("the clothing texture is {}x{}; the game's clothing is 32x32", .0.x, .0.y)]
    ClothingSize(glam::UVec2),
    #[error("node {node:?}: {problem}")]
    Node { node: String, problem: String },
}

/// The exported sources of a model and its texture set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Export {
    /// Contents of `src/data/npc/model/mdl/<model>.c`.
    pub model_c: String,
    pub textures: TextureExport,
}

/// The exported sources of a texture set drawn on an existing model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextureExport {
    pub(crate) model: String,
    pub(crate) texture_set: String,
    pub(crate) texture_c: String,
    /// The arrays `texture_c` defines.
    pub(crate) symbols: TextureSymbols,
    /// Slot and size of each of the project's textures, in order, which
    /// together set where the model finds each one.
    pub(crate) texture_sizes: Vec<(Slot, glam::UVec2)>,
}

/// Names of the arrays a texture set defines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TextureSymbols {
    pub palette: String,
    pub eyes: Vec<String>,
    pub mouths: Vec<String>,
    pub body: String,
}

impl TextureSymbols {
    fn new(set: &str, layout: &Layout) -> Self {
        Self {
            palette: format!("{set}_pal"),
            eyes: (1..=layout.eyes.len())
                .map(|k| format!("{set}_eye{k}_TA_tex_txt"))
                .collect(),
            mouths: (1..=layout.mouths.len())
                .map(|k| format!("{set}_mouth{k}_TA_tex_txt"))
                .collect(),
            body: format!("{set}_tmem_txt"),
        }
    }

    /// Every name, in the order `texture_c` defines them.
    pub fn all(&self) -> impl Iterator<Item = &String> {
        std::iter::once(&self.palette)
            .chain(&self.eyes)
            .chain(&self.mouths)
            .chain([&self.body])
    }
}

impl TextureExport {
    /// The model the textures are drawn on, such as `cat_1`.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    /// The texture set's name, such as `cat_2`.
    #[must_use]
    pub fn texture_set(&self) -> &str {
        &self.texture_set
    }

    /// Contents of `src/data/npc/model/tex/<texture_set>.c`.
    #[must_use]
    pub fn texture_c(&self) -> &str {
        &self.texture_c
    }

    /// The declarations `src/data/npc/npc_draw_data.c` needs for the table
    /// entry.
    #[must_use]
    pub fn table_externs(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "extern cKF_Skeleton_R_c cKF_bs_r_{};", self.model);
        let _ = writeln!(out, "extern unsigned short {}[];", self.symbols.palette);
        for n in self.symbols.all().skip(1) {
            let _ = writeln!(out, "extern unsigned char {n}[];");
        }
        out
    }

    /// The `npc_draw_data_tbl` entry for the texture set. It copies the
    /// model bank and the fields after the textures from `base`, a villager
    /// on the same model, or leaves `MODEL_BANK` and comments to fill in
    /// without one. `TEXTURE_BANK` is always left to replace with the new
    /// texture set's object bank.
    pub fn table_entry(&self, base: Option<&VillagerRef>) -> Result<String, crate::WriteError> {
        if let Some(b) = base.filter(|b| b.model != self.model) {
            return Err(crate::WriteError::OtherModel {
                texture_set: b.texture_set.clone(),
                actual: b.model.clone(),
                expected: self.model.clone(),
            });
        }
        let list = |names: &[String], len: usize| {
            (0..len)
                .map(|i| names.get(i).map_or("NULL", String::as_str).to_owned())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let (model_bank, texture_extra, extra) = match base {
            Some(b) => (
                b.fields.model_bank.clone(),
                b.fields.texture_extra.join(", "),
                b.fields.extra.join(", "),
            ),
            None => (
                "MODEL_BANK".to_owned(),
                "/* the base villager's remaining texture fields */".to_owned(),
                "/* the base villager's remaining fields: scale, talk type, ... */".to_owned(),
            ),
        };
        let s = &self.symbols;
        Ok(format!(
            "    {{
        {model_bank}, TEXTURE_BANK,
        &cKF_bs_r_{},
        {{ {}, {}, {{ {} }}, {{ {} }}, {texture_extra} }},
        {extra}
    }},
",
            self.model,
            s.body,
            s.palette,
            list(&s.eyes, EYE_VARIANTS),
            list(&s.mouths, MOUTH_VARIANTS),
        ))
    }

    /// Writes `<texture_set>.c`, and `<texture_set>_draw_data.txt` with the
    /// rest of what a new texture set needs in the decomp, into `dir`,
    /// returning their paths.
    pub fn write_to(
        &self,
        dir: &std::path::Path,
        base: Option<&VillagerRef>,
    ) -> Result<[std::path::PathBuf; 2], crate::WriteError> {
        let set = &self.texture_set;
        let io = |path: &std::path::Path| {
            let path = path.to_owned();
            move |e| crate::WriteError::Io(path, e)
        };
        std::fs::create_dir_all(dir).map_err(io(dir))?;
        let c = dir.join(format!("{set}.c"));
        let table = dir.join(format!("{set}_draw_data.txt"));
        let entry = self.table_entry(base)?;
        let notes = format!(
            "// To add texture set {set} to the decomp:
// 1. Copy {set}.c to src/data/npc/model/tex/{set}.c.
// 2. List it in configure.py beside the other texture sets,
//        Object(Matching, \"data/npc/model/tex/{set}.c\"),
//    and add it to its module's link order with configure.py's
//    link_order_callback (see the comment above it there).
// 3. Add these declarations to src/data/npc/npc_draw_data.c:
{}// 4. The game finds a villager's row in npc_draw_data_tbl by its NPC id, so
//    replace the row of the villager this texture set is for with this
//    one (adding a row would shift every later villager), replacing
//    TEXTURE_BANK with the texture set's object bank{}:
{}// 5. Build with `python configure.py --non-matching && ninja`.
",
            self.table_externs(),
            if base.is_some() {
                ""
            } else {
                ", MODEL_BANK with the model's, and the commented fields with the base villager's"
            },
            entry
        );
        mint_core::file::write_atomic(&c, self.texture_c.as_bytes()).map_err(io(&c))?;
        mint_core::file::write_atomic(&table, notes.as_bytes()).map_err(io(&table))?;
        Ok([c, table])
    }
}

fn check_name(name: &str) -> Result<(), ExportError> {
    let mut chars = name.chars();
    let ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if ok {
        Ok(())
    } else {
        Err(ExportError::BadName(name.to_owned()))
    }
}

/// Where each painted texture lives in the game's memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Placement {
    Eye,
    Mouth,
    Clothing,
    Body { offset: usize },
}

struct Layout {
    placements: Vec<Placement>,
    body: Vec<u8>,
    eyes: Vec<Vec<u8>>,
    mouths: Vec<Vec<u8>>,
}

fn variant_bytes(texture: &Texture) -> Result<Vec<Vec<u8>>, ExportError> {
    texture
        .variants()
        .iter()
        .map(|v| {
            let img = v.rasterize();
            gx::encode_ci4(img.pixels(), img.size().x, img.size().y).map_err(|source| {
                ExportError::Gx {
                    texture: texture.name.clone(),
                    source,
                }
            })
        })
        .collect()
}

fn layout(project: &Project) -> Result<Layout, ExportError> {
    let mut out = Layout {
        placements: Vec::new(),
        body: Vec::new(),
        eyes: Vec::new(),
        mouths: Vec::new(),
    };
    let (mut seen_eye, mut seen_mouth, mut seen_clothing) = (false, false, false);
    for texture in project.textures() {
        let size = texture.size();
        gx::check_size(size.x, size.y).map_err(|source| ExportError::Gx {
            texture: texture.name.clone(),
            source,
        })?;
        let once = |seen: &mut bool, slot: &'static str| {
            if std::mem::replace(seen, true) {
                Err(ExportError::DuplicateSlot(slot))
            } else {
                Ok(())
            }
        };
        let check_count = |slot: &'static str, expected: usize| {
            let actual = texture.variants().len();
            if actual == expected {
                Ok(())
            } else {
                Err(ExportError::VariantCount {
                    slot,
                    expected,
                    actual,
                })
            }
        };
        let placement = match (texture.slot.0.as_str(), texture.content()) {
            (SLOT_EYES, TextureContent::Painted(_)) => {
                once(&mut seen_eye, SLOT_EYES)?;
                check_count(SLOT_EYES, EYE_VARIANTS)?;
                out.eyes = variant_bytes(texture)?;
                Placement::Eye
            }
            (SLOT_MOUTH, TextureContent::Painted(_)) => {
                once(&mut seen_mouth, SLOT_MOUTH)?;
                check_count(SLOT_MOUTH, MOUTH_VARIANTS)?;
                out.mouths = variant_bytes(texture)?;
                Placement::Mouth
            }
            (SLOT_CLOTHING, TextureContent::External { .. }) => {
                once(&mut seen_clothing, SLOT_CLOTHING)?;
                // The game supplies clothing at this size; eye, mouth, and
                // body textures are the villager's own arrays, sized by
                // their loads.
                if size != glam::UVec2::new(32, 32) {
                    return Err(ExportError::ClothingSize(size));
                }
                Placement::Clothing
            }
            (SLOT_BODY, TextureContent::Painted(_)) => {
                check_count(SLOT_BODY, 1)?;
                let offset = out.body.len();
                // Offsets share a 32-bit address with the segment number.
                if offset >= 0x100_0000 {
                    return Err(ExportError::BodyTooLarge(offset));
                }
                out.body.extend(variant_bytes(texture)?.concat());
                Placement::Body { offset }
            }
            (slot, _) => {
                return Err(ExportError::UnknownSlot {
                    texture: texture.name.clone(),
                    slot: slot.to_owned(),
                });
            }
        };
        out.placements.push(placement);
    }
    Ok(out)
}

fn byte_array(out: &mut String, name: &str, data: &[u8]) {
    let _ = writeln!(
        out,
        "extern unsigned char {name}[] __attribute__((aligned(32))) = {{"
    );
    for row in data.chunks(16) {
        out.push_str("   ");
        for b in row {
            let _ = write!(out, " 0x{b:02X},");
        }
        out.push('\n');
    }
    out.push_str("};\n\n");
}

fn texture_c(project: &Project, set: &str, layout: &Layout) -> String {
    let colors = project.palette().colors();
    let mut out = String::new();
    let _ = writeln!(
        out,
        "extern unsigned short {set}_pal[] __attribute__((aligned(32))) = {{"
    );
    out.push_str("   ");
    for i in 0..PALETTE_LEN {
        let c = colors
            .get(i)
            .copied()
            .unwrap_or(mint_core::Rgba8::new(0, 0, 0, 0));
        let _ = write!(out, " 0x{:04X},", gx::encode_rgb5a3(c));
    }
    out.push_str("\n};\n\n");
    let names = TextureSymbols::new(set, layout);
    let arrays = layout
        .eyes
        .iter()
        .chain(&layout.mouths)
        .chain([&layout.body]);
    for (name, data) in names.all().skip(1).zip(arrays) {
        byte_array(&mut out, name, data);
    }
    out
}

/// One exported `Vtx`: a core vertex with the texel coordinates of one use.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct OutVtx {
    pos: [i16; 3],
    /// 0 for seam vertices, transformed when loaded under the other node's
    /// matrix; 1 for the node's own, transformed when drawn, as the game's
    /// lists do.
    flag: u8,
    st: [i16; 2],
    normal: [u8; 3],
}

struct ModelWriter<'a> {
    project: &'a Project,
    layout: &'a Layout,
    model: &'a str,
    slots: HashMap<NodeId, u32>,
    vertices: Vec<OutVtx>,
    lists: String,
}

fn wrap_name(w: Wrap) -> &'static str {
    match w {
        Wrap::Clamp => "GX_CLAMP",
        Wrap::Repeat => "GX_REPEAT",
        Wrap::Mirror => "GX_MIRROR",
    }
}

fn to_i16(v: f32, what: &str) -> Result<i16, String> {
    let r = v.round();
    if (f32::from(i16::MIN)..=f32::from(i16::MAX)).contains(&r) {
        Ok(r as i16)
    } else {
        Err(format!("{what} {v} does not fit in 16 bits"))
    }
}

impl ModelWriter<'_> {
    fn texture_address(&self, id: TextureId) -> (String, u32) {
        match self.layout.placements[id.index()] {
            Placement::Eye => ("anime_1_txt".into(), 15),
            Placement::Mouth => ("anime_2_txt".into(), 15),
            Placement::Clothing => ("anime_3_txt".into(), 14),
            Placement::Body { offset: 0 } => ("anime_4_txt".into(), 15),
            Placement::Body { offset } => (format!("anime_4_txt + {offset:#05X}"), 15),
        }
    }

    fn matrix(&mut self, node: NodeId) -> Result<u32, String> {
        self.slots.get(&node).copied().ok_or_else(|| {
            format!(
                "vertices follow node {:?}, which has no mesh and so no matrix",
                self.project.model().node(node).name
            )
        })
    }

    /// Writes the display list for `node`'s mesh and returns its name.
    fn display_list(&mut self, node: NodeId) -> Result<String, String> {
        let project = self.project;
        let n = project.model().node(node);
        let mesh = n.mesh.as_ref().expect("only called for nodes with meshes");
        // The decomp names joint lists `<joint>_<species>_model`.
        let species = self.model.split('_').next().unwrap_or(self.model);
        let name = format!("{}_{species}_model", sanitize(&n.name));

        // Runs of consecutive triangles sharing a material, in drawing order:
        // the game keeps the first of two coincident triangles, so
        // reordering could change what shows.
        let mut groups: Vec<(mint_core::MaterialId, Vec<usize>)> = Vec::new();
        for (i, t) in mesh.triangles.iter().enumerate() {
            match groups.last_mut() {
                Some((m, v)) if *m == t.material => v.push(i),
                _ => groups.push((t.material, vec![i])),
            }
        }
        let others: Vec<NodeId> = {
            let mut v: Vec<NodeId> = mesh
                .vertices
                .iter()
                .filter_map(|v| v.follows.filter(|&f| f != node))
                .collect();
            v.sort();
            v.dedup();
            v
        };
        if others.len() > 1 {
            return Err("vertices follow more than one other node; AC joints allow one".into());
        }
        let seam = others.first().copied();

        let mut body = String::new();
        for (material, tris) in &groups {
            let m = *project.material(*material);
            let texture = project.texture(m.texture);
            let size = texture.size();
            let (addr, tlut) = self.texture_address(m.texture);
            let render = match m.alpha {
                AlphaMode::Opaque => "G_RM_AA_ZB_OPA_SURF2",
                AlphaMode::Cutout => "G_RM_AA_ZB_TEX_EDGE2",
            };
            let cull = if m.double_sided { "" } else { " | G_CULL_BACK" };
            let _ = writeln!(body, "    gsSPTexture(0, 0, 0, 0, G_ON),");
            let _ = writeln!(body, "    gsDPSetRenderMode(G_RM_FOG_SHADE_A, {render}),");
            let _ = writeln!(
                body,
                "    gsDPSetCombineLERP(TEXEL0, 0, SHADE, 0, 0, 0, 0, TEXEL0, PRIMITIVE, 0, COMBINED, 0, 0, 0, 0, COMBINED),"
            );
            let _ = writeln!(
                body,
                "    gsDPLoadTextureBlock_4b_Dolphin({addr}, G_IM_FMT_CI, {}, {}, {tlut}, {}, {}, 0, 0),",
                size.x,
                size.y,
                wrap_name(m.wrap[0]),
                wrap_name(m.wrap[1])
            );
            let _ = writeln!(
                body,
                "    gsDPSetTileSize(0, 0, 0, {}, {}),",
                (size.x - 1) * 4,
                (size.y - 1) * 4
            );
            let t = m.tint;
            let _ = writeln!(
                body,
                "    gsDPSetPrimColor(0, 128, {}, {}, {}, {}),",
                t.r, t.g, t.b, t.a
            );
            let _ = writeln!(
                body,
                "    gsSPLoadGeometryMode(G_ZBUFFER | G_SHADE{cull} | G_FOG | G_LIGHTING | G_SHADING_SMOOTH),"
            );

            // Batch triangles so each batch's vertices fit the cache.
            let mut batch: Vec<[CornerKey; 3]> = Vec::new();
            let mut batch_verts: Vec<CornerKey> = Vec::new();
            for &ti in tris {
                let corners = self.corner_keys(node, ti, size)?;
                let mut new: Vec<CornerKey> = corners
                    .iter()
                    .filter(|k| !batch_verts.contains(k))
                    .copied()
                    .collect();
                new.sort_unstable();
                new.dedup();
                if batch_verts.len() + new.len() > VERTEX_CACHE
                    || batch.len() == MAX_GROUP_TRIANGLES
                {
                    self.flush(&mut body, node, seam, &batch, &batch_verts)?;
                    batch.clear();
                    batch_verts.clear();
                    batch_verts.extend(corners);
                    batch_verts.sort_unstable();
                    batch_verts.dedup();
                } else {
                    batch_verts.extend(new);
                }
                batch.push(corners);
            }
            self.flush(&mut body, node, seam, &batch, &batch_verts)?;
        }

        let _ = writeln!(self.lists, "static Gfx {name}[] = {{");
        self.lists.push_str(&body);
        self.lists.push_str("    gsSPEndDisplayList(),\n};\n\n");
        Ok(name)
    }

    /// The (vertex, texel coordinate) of each corner of triangle `ti`.
    fn corner_keys(
        &self,
        node: NodeId,
        ti: usize,
        size: glam::UVec2,
    ) -> Result<[CornerKey; 3], String> {
        let mesh = self.project.model().node(node).mesh.as_ref().expect("mesh");
        let t = &mesh.triangles[ti];
        let mut out = [(0, [0, 0]); 3];
        for (o, c) in out.iter_mut().zip(t.corners) {
            let st = c.uv * size.as_vec2() * 32.0;
            *o = (
                c.vertex,
                [
                    to_i16(st.x, "texture coordinate")?,
                    to_i16(st.y, "texture coordinate")?,
                ],
            );
        }
        Ok(out)
    }

    /// Writes one batch: its vertex loads, then its triangles.
    fn flush(
        &mut self,
        body: &mut String,
        node: NodeId,
        seam: Option<NodeId>,
        batch: &[[CornerKey; 3]],
        verts: &[CornerKey],
    ) -> Result<(), String> {
        if batch.is_empty() {
            return Ok(());
        }
        let mesh = self.project.model().node(node).mesh.as_ref().expect("mesh");
        // Seam vertices first, loaded under the other node's matrix; then the
        // node's own matrix, which is also what the game loads before calling
        // the list, so lists without seam vertices need no matrix commands.
        let (seam_verts, own_verts): (Vec<_>, Vec<_>) = verts
            .iter()
            .copied()
            .partition(|(v, _)| mesh.vertices[*v as usize].followed(node) != node);
        let mut slot_of: HashMap<CornerKey, usize> = HashMap::new();
        let mut load = |this: &mut Self,
                        body: &mut String,
                        group: &[CornerKey],
                        flag: u8|
         -> Result<(), String> {
            if group.is_empty() {
                return Ok(());
            }
            let first = this.vertices.len();
            let first_slot = slot_of.len();
            for &(v, st) in group {
                let vert = mesh.vertices[v as usize];
                let p = vert.position;
                let n = vert.normal * 127.0;
                this.vertices.push(OutVtx {
                    pos: [
                        to_i16(p.x, "position")?,
                        to_i16(p.y, "position")?,
                        to_i16(p.z, "position")?,
                    ],
                    flag,
                    st,
                    normal: [n.x, n.y, n.z].map(|c| c.round().clamp(-128.0, 127.0) as i8 as u8),
                });
                slot_of.insert((v, st), slot_of.len());
            }
            let _ = writeln!(
                body,
                "    gsSPVertex(&{}_v[{first}], {}, {first_slot}),",
                this.model,
                group.len(),
            );
            Ok(())
        };
        if let Some(seam) = seam
            && !seam_verts.is_empty()
        {
            let s = self.matrix(seam)?;
            let _ = writeln!(
                body,
                "    gsSPMatrix(anime_6_model + sizeof(Mtx) * {s}, G_MTX_NOPUSH | G_MTX_LOAD | G_MTX_MODELVIEW),"
            );
            load(self, body, &seam_verts, 0)?;
            let own = self.matrix(node)?;
            let _ = writeln!(
                body,
                "    gsSPMatrix(anime_6_model + sizeof(Mtx) * {own}, G_MTX_NOPUSH | G_MTX_LOAD | G_MTX_MODELVIEW),"
            );
        }
        load(self, body, &own_verts, 1)?;

        let tris: Vec<[usize; 3]> = batch.iter().map(|t| t.map(|k| slot_of[&k])).collect();
        let fmt = |t: Option<&[usize; 3]>| {
            let [a, b, c] = t.copied().unwrap_or([0, 0, 0]);
            format!("{a}, {b}, {c}")
        };
        let _ = writeln!(
            body,
            "    gsSPNTrianglesInit_5b({}, {}, {}, {}),",
            tris.len(),
            fmt(tris.first()),
            fmt(tris.get(1)),
            fmt(tris.get(2))
        );
        for chunk in tris.get(3..).unwrap_or_default().chunks(4) {
            let _ = writeln!(
                body,
                "    gsSPNTriangles_5b({}, {}, {}, {}),",
                fmt(chunk.first()),
                fmt(chunk.get(1)),
                fmt(chunk.get(2)),
                fmt(chunk.get(3))
            );
        }
        Ok(())
    }
}

fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if s.starts_with(|c: char| c.is_ascii_digit()) {
        format!("_{s}")
    } else {
        s
    }
}

fn model_c(project: &Project, model: &str, layout: &Layout) -> Result<String, ExportError> {
    let tree = project.model();
    let node_err = |id: NodeId, problem: String| ExportError::Node {
        node: tree.node(id).name.clone(),
        problem,
    };
    // The game reads joints depth-first, and matrix slot N belongs to the
    // N-th of them with a display list.
    let order = tree.preorder();
    let children: Vec<usize> = order.iter().map(|&id| tree.children(id).count()).collect();
    if let Some((&id, (&found, &expected))) = order
        .iter()
        .zip(children.iter().zip(&VILLAGER_JOINT_CHILDREN))
        .find(|(_, (found, expected))| **found != usize::from(**expected))
    {
        return Err(ExportError::Skeleton(format!(
            "node {:?} has {found} children where the skeleton has {expected}",
            tree.node(id).name
        )));
    }
    if children.len() != VILLAGER_JOINT_CHILDREN.len() {
        return Err(ExportError::Skeleton(format!(
            "{} nodes; the skeleton has {}",
            children.len(),
            VILLAGER_JOINT_CHILDREN.len()
        )));
    }
    let slots: HashMap<NodeId, u32> = order
        .iter()
        .filter(|&&id| tree.node(id).mesh.is_some())
        .enumerate()
        .map(|(slot, &id)| (id, slot as u32))
        .collect();
    let mut w = ModelWriter {
        project,
        layout,
        model,
        slots,
        vertices: Vec::new(),
        lists: String::new(),
    };
    let mut names = HashSet::new();
    let mut joints = Vec::new();
    for (&id, &children) in order.iter().zip(&children) {
        let node = tree.node(id);
        let dl = if node.mesh.is_some() {
            let name = w.display_list(id).map_err(|p| node_err(id, p))?;
            if !names.insert(name.clone()) {
                return Err(node_err(
                    id,
                    format!("another node's display list is also named {name}; rename one"),
                ));
            }
            Some(name)
        } else {
            None
        };
        let t = node.transform;
        if t.rotation != glam::Quat::IDENTITY || t.scale != glam::Vec3::ONE {
            return Err(node_err(
                id,
                "AC joints only translate; rotation and scale are not supported".into(),
            ));
        }
        let pos = t.translation;
        let p = [pos.x, pos.y, pos.z]
            .map(|c| to_i16(c, "joint translation"))
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|p| node_err(id, p))?;
        joints.push((dl, children, p));
    }

    let mut out = String::new();
    out.push_str("#include \"c_keyframe.h\"\n#include \"libforest/gbi_extensions.h\"\n\n");
    let _ = writeln!(out, "static Vtx {model}_v[] = {{");
    for v in &w.vertices {
        let _ = writeln!(
            out,
            "    {{ {}, {}, {}, {}, {}, {}, {}, {}, {}, 255 }},",
            v.pos[0],
            v.pos[1],
            v.pos[2],
            v.flag,
            v.st[0],
            v.st[1],
            v.normal[0],
            v.normal[1],
            v.normal[2]
        );
    }
    out.push_str("};\n\n");
    out.push_str(&w.lists);
    out.push_str("// clang-format off\n");
    let _ = writeln!(out, "static cKF_Joint_R_c cKF_je_r_{model}_tbl[] = {{");
    for (i, (dl, children, p)) in joints.iter().enumerate() {
        let _ = writeln!(
            out,
            "    /* joint {i} */ {{ {}, {children}, cKF_JOINT_FLAG_DISP_OPA, {},{},{} }},",
            dl.as_deref().unwrap_or("NULL"),
            p[0],
            p[1],
            p[2]
        );
    }
    out.push_str("};\n// clang-format on\n\n");
    let shown = joints.iter().filter(|j| j.0.is_some()).count();
    let _ = writeln!(
        out,
        "extern cKF_Skeleton_R_c cKF_bs_r_{model} = {{ {}, {shown}, cKF_je_r_{model}_tbl }};",
        joints.len()
    );
    Ok(out)
}

/// Checks what the target can hold and lays out the textures.
fn prepare(project: &Project, texture_set: &str) -> Result<Layout, ExportError> {
    if project.target().id != TARGET {
        return Err(ExportError::WrongTarget(project.target().id.clone()));
    }
    check_name(texture_set)?;
    let colors = project.palette().len();
    if colors > PALETTE_LEN {
        return Err(ExportError::PaletteTooLarge(colors));
    }
    // Layout checks texture sizes, which the wrap check builds on. Only
    // materials that triangles use become texture loads.
    let layout = layout(project)?;
    let used: HashSet<MaterialId> = project
        .model()
        .nodes()
        .iter()
        .filter_map(|n| n.mesh.as_ref())
        .flat_map(|m| m.triangles.iter().map(|t| t.material))
        .collect();
    for m in used.iter().map(|&id| project.material(id)) {
        let texture = project.texture(m.texture);
        let size = texture.size();
        gx::check_wrap(size.x, size.y, m.wrap).map_err(|source| ExportError::Gx {
            texture: texture.name.clone(),
            source,
        })?;
    }
    Ok(layout)
}

/// Slot and size of each texture, in order.
pub(crate) fn texture_sizes(project: &Project) -> Vec<(Slot, glam::UVec2)> {
    project
        .textures()
        .iter()
        .map(|t| (t.slot.clone(), t.size()))
        .collect()
}

fn texture_export(
    project: &Project,
    model: &str,
    texture_set: &str,
    layout: &Layout,
) -> TextureExport {
    TextureExport {
        model: model.to_owned(),
        texture_set: texture_set.to_owned(),
        texture_c: texture_c(project, texture_set, layout),
        symbols: TextureSymbols::new(texture_set, layout),
        texture_sizes: texture_sizes(project),
    }
}

/// Exports `project` as a new model `model` with texture set `texture_set`.
pub fn export(project: &Project, model: &str, texture_set: &str) -> Result<Export, ExportError> {
    check_name(model)?;
    let layout = prepare(project, texture_set)?;
    Ok(Export {
        model_c: model_c(project, model, &layout)?,
        textures: texture_export(project, model, texture_set, &layout),
    })
}

/// Where an AC project was imported from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImportedFrom<'a> {
    /// The model, such as `cat_1`.
    pub model: &'a str,
    /// The texture set, such as `cat_2`.
    pub texture_set: &'a str,
}

/// The model and texture set an AC project was imported from, as recorded in
/// its [`OPTION_MODEL`] and [`OPTION_TEXTURE_SET`] target options.
pub fn imported_from(project: &Project) -> Result<ImportedFrom<'_>, ExportError> {
    let option = |key: &'static str| {
        project
            .target()
            .options
            .get(key)
            .map(String::as_str)
            .ok_or(ExportError::MissingOption(key))
    };
    Ok(ImportedFrom {
        model: option(OPTION_MODEL)?,
        texture_set: option(OPTION_TEXTURE_SET)?,
    })
}

/// Exports only the textures of `project`, as texture set `texture_set` for
/// the unchanged model it was imported from (see [`imported_from`]).
pub fn export_textures(project: &Project, texture_set: &str) -> Result<TextureExport, ExportError> {
    let model = imported_from(project)?.model;
    check_name(model)?;
    let layout = prepare(project, texture_set)?;
    Ok(texture_export(project, model, texture_set, &layout))
}
