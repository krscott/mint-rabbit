//! Import, export, and re-import must agree: same node tree, palette,
//! textures, and triangles with the same resolved corners and materials.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use mint_core::{Model, NodeId, Palette, Project, Rgba8, TargetId, TextureContent, Transform};

use crate::c::CFile;
use crate::import::{SPECIAL_NPC_MODELS, build_project, parse_villagers};
use crate::{Decomp, ExportError, export, gx};

/// Slot, size, variant pixels, and whether the texture is external.
type CanonicalTexture = (String, [u32; 2], Vec<Vec<u8>>, bool);
/// Name, parent, translation, and triangle descriptions in drawing order.
type CanonicalNode = (String, Option<u32>, [i64; 3], Vec<String>);

/// A comparable summary of a project that ignores what the exporter is free
/// to change: vertex layout, texture names and placement, and colors beyond
/// what the target can show.
#[derive(Debug, PartialEq)]
struct Canonical {
    palette: Vec<[u8; 4]>,
    textures: Vec<CanonicalTexture>,
    nodes: Vec<CanonicalNode>,
    pose: String,
}

fn canonical(p: &Project) -> Canonical {
    let palette = p
        .palette()
        .colors()
        .iter()
        .map(|&c| gx::quantize_rgb5a3(c).to_array())
        .collect();
    let textures = p
        .textures()
        .iter()
        .map(|t| {
            let variants = t
                .variants()
                .iter()
                .map(|v| v.underlay().pixels().iter().map(|i| i.0).collect())
                .collect();
            let external = matches!(t.content(), TextureContent::External { .. });
            (t.slot.0.clone(), t.size().to_array(), variants, external)
        })
        .collect();
    let model = p.model();
    let nodes = model
        .ids()
        .map(|id| {
            let n = model.node(id);
            let tris: Vec<String> = n
                .mesh
                .iter()
                .flat_map(|m| {
                    m.triangles.iter().map(|t| {
                        let mat = p.material(t.material);
                        let size = p.texture(mat.texture).size().as_vec2();
                        let mut s = format!(
                            "tex{} {:?} {:?} ds={} tint={:?} |",
                            mat.texture.0, mat.wrap, mat.alpha, mat.double_sided, mat.tint
                        );
                        for c in t.corners {
                            let v = m.vertices[c.vertex as usize];
                            let st = (c.uv * size * 32.0).round();
                            let nrm = (v.normal * 127.0).round();
                            let _ = write!(
                                s,
                                " p{:?} n{:?} f{} st{:?}",
                                v.position.round().to_array(),
                                nrm.to_array(),
                                v.followed(id).0,
                                st.to_array()
                            );
                        }
                        s
                    })
                })
                .collect();
            let t = n.transform.translation.round();
            (
                n.name.clone(),
                n.parent().map(|p| p.0),
                [t.x as i64, t.y as i64, t.z as i64],
                tris,
            )
        })
        .collect();
    Canonical {
        palette,
        textures,
        nodes,
        pose: format!("{:?}", p.pose()),
    }
}

/// Exports `project` and imports the result again.
fn reimport(project: &Project, pose_file: &CFile) -> Project {
    let out = export(project, "rt_1", "rt_1").unwrap();
    let model = CFile::parse_str(Path::new("rt_1.c"), &out.model_c).unwrap();
    let tex = CFile::parse_str(Path::new("rt_1_tex.c"), &out.texture_c).unwrap();
    let table = CFile::parse_str(
        Path::new("table.c"),
        &format!(
            "aNPC_draw_data_c npc_draw_data_tbl[] = {{\n{}}};",
            out.draw_data_entry
        ),
    )
    .unwrap();
    let v = parse_villagers(&table).unwrap().remove(0);
    build_project(&model, &tex, pose_file, &v).unwrap()
}

/// Joints of the synthetic villager that have display lists: body (slot 0)
/// and head (slot 1).
const BODY_JOINT: usize = 13;
const HEAD_JOINT: usize = 24;

/// A pose for the 26-joint villager skeleton: fixed rotations only.
fn synthetic_pose() -> String {
    let mut fixed = vec![0, 1000, 0];
    for j in 0..26 {
        fixed.extend(match j {
            0 => [0, 0, 900],
            HEAD_JOINT => [300, 0, 0],
            _ => [0, 0, 0],
        });
    }
    let fixed: Vec<String> = fixed.iter().map(ToString::to_string).collect();
    format!(
        "u8 flags[] = {{ {} }};
         s16 keys[] = {{ 0 }};
         s16 data[] = {{ 0 }};
         s16 fixed[] = {{ {} }};
         aNPC_Animation_c cKF_ba_r_npc_1_wait1 = {{ {{ flags, data, keys, fixed, -1, 33 }} }};",
        ["0"; 26].join(", "),
        fixed.join(", ")
    )
}

/// The villager joint table with display lists on the body and head.
fn synthetic_joints() -> String {
    crate::VILLAGER_JOINT_CHILDREN
        .iter()
        .enumerate()
        .map(|(j, children)| {
            let (dl, t) = match j {
                0 => ("NULL", "0,1000,0"),
                BODY_JOINT => ("body_t_model", "0,0,0"),
                HEAD_JOINT => ("head_t_model", "500,0,0"),
                _ => ("NULL", "0,0,0"),
            };
            format!("{{ {dl}, {children}, cKF_JOINT_FLAG_DISP_OPA, {t} }},")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn prelude(
    texture: &str,
    w: u32,
    h: u32,
    tlut: u32,
    wrap: &str,
    render: &str,
    cull: &str,
) -> String {
    format!(
        "gsSPTexture(0, 0, 0, 0, G_ON),
         gsDPSetRenderMode(G_RM_FOG_SHADE_A, {render}),
         gsDPSetCombineLERP(TEXEL0, 0, SHADE, 0, 0, 0, 0, TEXEL0, PRIMITIVE, 0, COMBINED, 0, 0, 0, 0, COMBINED),
         gsDPLoadTextureBlock_4b_Dolphin({texture}, G_IM_FMT_CI, {w}, {h}, {tlut}, {wrap}, 0, 0),
         gsDPSetPrimColor(0, 128, 255, 250, 255, 255),
         gsSPLoadGeometryMode(G_ZBUFFER | G_SHADE{cull} | G_FOG | G_LIGHTING | G_SHADING_SMOOTH),"
    )
}

fn synthetic() -> (CFile, CFile, CFile, CFile) {
    let model = format!(
        "static Vtx t_1_v[] = {{
            {{ 0, 0, 0, 0, 0, 0, 0, 127, 0, 255 }},
            {{ 100, 0, 0, 0, 512, 0, 0, 127, 0, 255 }},
            {{ 0, 100, 0, 0, 0, 512, 0, 127, 0, 255 }},
            {{ 0, 0, 10, 0, 0, 0, 0, 127, 0, 255 }},
            {{ 50, 0, 0, 0, 1024, 0, 127, 0, 0, 255 }},
            {{ 0, 50, -5, 0, 0, 256, 129, 0, 0, 255 }},
         }};
         static Gfx body_t_model[] = {{
            {body}
            gsSPVertex(&t_1_v[0], 3, 0),
            gsSPNTrianglesInit_5b(1, 0, 1, 2, 0, 0, 0, 0, 0, 0),
            gsSPEndDisplayList(),
         }};
         static Gfx head_t_model[] = {{
            gsSPMatrix(anime_6_model, G_MTX_NOPUSH | G_MTX_LOAD | G_MTX_MODELVIEW),
            gsSPVertex(&t_1_v[3], 1, 0),
            gsSPMatrix(anime_6_model + sizeof(Mtx) * 1, G_MTX_NOPUSH | G_MTX_LOAD | G_MTX_MODELVIEW),
            {eye}
            gsSPVertex(&t_1_v[4], 2, 1),
            gsSPNTrianglesInit_5b(1, 0, 1, 2, 0, 0, 0, 0, 0, 0),
            {mouth}
            gsSPNTrianglesInit_5b(1, 0, 2, 1, 0, 0, 0, 0, 0, 0),
            {shirt}
            gsSPNTrianglesInit_5b(1, 1, 0, 2, 0, 0, 0, 0, 0, 0),
            {body2}
            gsSPNTrianglesInit_5b(4, 2, 1, 0, 1, 2, 0, 0, 1, 2),
            gsSPNTriangles_5b(1, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0),
            gsSPEndDisplayList(),
         }};
         static cKF_Joint_R_c cKF_je_r_t_1_tbl[] = {{
            {joints}
         }};
         extern cKF_Skeleton_R_c cKF_bs_r_t_1 = {{ 26, 2, cKF_je_r_t_1_tbl }};",
        joints = synthetic_joints(),
        body = prelude("anime_4_txt", 16, 16, 15, "GX_CLAMP, GX_CLAMP", "G_RM_AA_ZB_OPA_SURF2", " | G_CULL_BACK"),
        eye = prelude("anime_1_txt", 32, 16, 15, "GX_CLAMP, GX_CLAMP", "G_RM_AA_ZB_OPA_SURF2", " | G_CULL_BACK"),
        mouth = prelude("anime_2_txt", 32, 16, 15, "GX_MIRROR, GX_CLAMP", "G_RM_AA_ZB_OPA_SURF2", " | G_CULL_BACK"),
        shirt = prelude("anime_3_txt", 32, 32, 14, "GX_REPEAT, GX_CLAMP", "G_RM_AA_ZB_OPA_SURF2", " | G_CULL_BACK"),
        body2 = prelude("anime_4_txt + 0x80", 16, 8, 15, "GX_MIRROR, GX_REPEAT", "G_RM_AA_ZB_TEX_EDGE2", ""),
    );
    let bytes = |seed: u32, n: u32| {
        (0..n)
            .map(|i| format!("{}", (i * 7 + seed * 13) % 256))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut tex = String::from(
        "unsigned short t_1_pal[] = { 0xFFFF, 0x8000, 0x7F00, 0x0ABC, 0x8421, 0xC210, 0x1234, 0xFC00, \
         0x83E0, 0x801F, 0x6666, 0xBBBB, 0x8888, 0x9999, 0xAAAA, 0x0000 };\n",
    );
    for k in 1..=8 {
        let _ = writeln!(
            tex,
            "unsigned char t_1_eye{k}_TA_tex_txt[] = {{ {} }};",
            bytes(k, 256)
        );
    }
    for k in 1..=6 {
        let _ = writeln!(
            tex,
            "unsigned char t_1_mouth{k}_TA_tex_txt[] = {{ {} }};",
            bytes(20 + k, 256)
        );
    }
    let _ = writeln!(
        tex,
        "unsigned char t_1_tmem_txt[] = {{ {} }};",
        bytes(99, 0xC0)
    );
    let eyes: Vec<String> = (1..=8).map(|k| format!("t_1_eye{k}_TA_tex_txt")).collect();
    let mouths: Vec<String> = (1..=6)
        .map(|k| format!("t_1_mouth{k}_TA_tex_txt"))
        .collect();
    let table = format!(
        "aNPC_draw_data_c npc_draw_data_tbl[] = {{
            {{ 1, 2, &cKF_bs_r_t_1, {{ t_1_tmem_txt, t_1_pal, {{ {} }}, {{ {} }}, 0, 32, 128 }}, 0.009, 0 }},
         }};",
        eyes.join(", "),
        mouths.join(", ")
    );
    let parse = |name: &str, text: &str| CFile::parse_str(Path::new(name), text).unwrap();
    (
        parse("t_1.c", &model),
        parse("t_1_tex.c", &tex),
        parse("table.c", &table),
        parse("pose.c", &synthetic_pose()),
    )
}

fn synthetic_project() -> (Project, CFile) {
    let (model, tex, table, pose) = synthetic();
    let v = parse_villagers(&table).unwrap().remove(0);
    (build_project(&model, &tex, &pose, &v).unwrap(), pose)
}

#[test]
fn synthetic_villager_round_trips() {
    let (original, pose) = synthetic_project();

    // The import itself: a seam vertex, five textures, five materials.
    let mesh = original
        .model()
        .node(NodeId(HEAD_JOINT as u32))
        .mesh
        .as_ref()
        .unwrap();
    assert_eq!(mesh.vertices[0].follows, Some(NodeId(BODY_JOINT as u32)));
    assert_eq!(mesh.triangles.len(), 7);
    assert_eq!(original.textures().len(), 5);
    assert_eq!(original.materials().len(), 5);

    let again = reimport(&original, &pose);
    assert_eq!(canonical(&original), canonical(&again));
}

/// Triangles keep their drawing order even when materials interleave: the
/// game keeps the first of two coincident triangles.
#[test]
fn export_keeps_triangle_order() {
    let (p, pose) = synthetic_project();
    let mut model = p.model().clone();
    let mesh = model
        .node_mut(NodeId(HEAD_JOINT as u32))
        .mesh
        .as_mut()
        .unwrap();
    // Move the last body triangle first, so the body material appears in
    // two separate runs.
    let body = mesh.triangles.pop().unwrap();
    mesh.triangles.insert(0, body);
    let p = variant_of(&p, crate::TARGET, p.palette().clone(), model);
    assert_eq!(canonical(&p), canonical(&reimport(&p, &pose)));
}

/// The synthetic project with some parts replaced.
fn variant_of(p: &Project, target: &str, palette: Palette, model: Model) -> Project {
    Project::new(
        TargetId(target.into()),
        palette,
        model,
        p.textures().to_vec(),
        p.materials().to_vec(),
        p.pose().clone(),
    )
    .unwrap()
}

#[test]
fn export_refuses_what_ac_villagers_cannot_hold() {
    let (p, _) = synthetic_project();
    assert!(export(&p, "rt_1", "rt_1").is_ok());
    assert_eq!(
        export(&p, "1bad", "rt_1"),
        Err(ExportError::BadName("1bad".into()))
    );

    let palette = p.palette().clone();
    let other_target = variant_of(&p, "other", palette.clone(), p.model().clone());
    assert!(matches!(
        export(&other_target, "rt_1", "rt_1"),
        Err(ExportError::WrongTarget(_))
    ));

    let seventeen = Palette::from_colors(
        p.palette()
            .colors()
            .iter()
            .copied()
            .chain([Rgba8::new(1, 2, 3, 255)]),
    )
    .unwrap();
    let big = variant_of(&p, crate::TARGET, seventeen, p.model().clone());
    assert_eq!(
        export(&big, "rt_1", "rt_1"),
        Err(ExportError::PaletteTooLarge(17))
    );

    let mut extra = p.model().clone();
    extra.add_child(NodeId::ROOT, "extra", Transform::default());
    let extra = variant_of(&p, crate::TARGET, palette.clone(), extra);
    assert!(matches!(
        export(&extra, "rt_1", "rt_1"),
        Err(ExportError::Skeleton(_))
    ));

    // The game's clothing texture is 32x32.
    let mut textures = p.textures().to_vec();
    let clothing = textures
        .iter()
        .position(|t| t.slot.0 == crate::SLOT_CLOTHING)
        .unwrap();
    textures[clothing] = mint_core::Texture::external(
        crate::SLOT_CLOTHING,
        mint_core::Slot(crate::SLOT_CLOTHING.into()),
        mint_core::RgbaImage::filled(glam::UVec2::new(64, 64), Rgba8::new(0, 0, 0, 255)),
    );
    let big_shirt = Project::new(
        p.target().clone(),
        p.palette().clone(),
        p.model().clone(),
        textures,
        p.materials().to_vec(),
        p.pose().clone(),
    )
    .unwrap();
    assert!(matches!(
        export(&big_shirt, "rt_1", "rt_1"),
        Err(ExportError::ClothingSize(_))
    ));

    // Both display lists would be named body_rt_model.
    let mut renamed = p.model().clone();
    renamed.node_mut(NodeId(HEAD_JOINT as u32)).name = "body".into();
    let renamed = variant_of(&p, crate::TARGET, palette, renamed);
    assert!(matches!(
        export(&renamed, "rt_1", "rt_1"),
        Err(ExportError::Node { .. })
    ));
}

/// A node added after its later siblings still exports in depth-first
/// order, so the game and the importer see the same tree.
#[test]
fn export_writes_joints_depth_first() {
    let (p, pose) = synthetic_project();
    // Give joint 5 (a leaf of the first leg) a mesh, so it takes matrix
    // slot 0 ahead of the body that the head's seam vertices follow.
    let mut with_leg = p.model().clone();
    let leg = with_leg.node_mut(NodeId(5));
    leg.name = "leg".into();
    leg.mesh = p.model().node(NodeId(BODY_JOINT as u32)).mesh.clone();
    let p = variant_of(&p, crate::TARGET, p.palette().clone(), with_leg);
    // Rebuild the same tree, adding joint 5 last, so its id comes after
    // nodes it precedes depth-first.
    let model = p.model();
    let mut shuffled = Model::new(
        model.node(NodeId::ROOT).name.clone(),
        model.node(NodeId::ROOT).transform,
    );
    let mut map = vec![NodeId::ROOT; model.nodes().len()];
    let order: Vec<NodeId> = model
        .ids()
        .filter(|&id| id.0 != 5)
        .chain([NodeId(5)])
        .skip(1)
        .collect();
    for id in order {
        let n = model.node(id);
        map[id.index()] = shuffled.add_child(
            map[n.parent().unwrap().index()],
            n.name.clone(),
            n.transform,
        );
    }
    for id in model.ids() {
        let mut mesh = model.node(id).mesh.clone();
        if let Some(m) = &mut mesh {
            for v in &mut m.vertices {
                v.follows = v.follows.map(|f| map[f.index()]);
            }
        }
        shuffled.node_mut(map[id.index()]).mesh = mesh;
    }
    let shuffled = variant_of(&p, crate::TARGET, p.palette().clone(), shuffled);
    assert_eq!(canonical(&p), canonical(&reimport(&shuffled, &pose)));
}

/// Round-trips every villager in a decomp built with extracted assets, named
/// by `MINT_AC_DECOMP`.
#[test]
#[ignore = "needs a built ac-decomp checkout in MINT_AC_DECOMP"]
fn every_built_villager_round_trips() {
    let root = PathBuf::from(std::env::var("MINT_AC_DECOMP").expect("MINT_AC_DECOMP"));
    let decomp = Decomp::open(&root, "GAFE01_00").unwrap_or_else(|e| panic!("{e}"));
    let pose = CFile::read(&root.join("src/data/npc/npc_1_wait1.c"), &[]).unwrap();
    let mut refused = Vec::new();
    let mut ok = 0;
    for v in decomp.villagers().unwrap() {
        match decomp.import(&v) {
            Ok(p) => {
                assert_eq!(
                    canonical(&p),
                    canonical(&reimport(&p, &pose)),
                    "{}",
                    v.texture_set
                );
                ok += 1;
            }
            Err(e) => refused.push((v.model.clone(), format!("{}: {e}", v.texture_set))),
        }
    }
    eprintln!("{ok} villagers round-trip");
    // Only these may fail: special NPC models the importer cannot represent,
    // and flg_12, whose 0x380-byte body texture is shorter than the 0x400
    // bytes the flg_1 model reads (the game reads past it).
    let unexpected: Vec<&str> = refused
        .iter()
        .filter(|(model, e)| {
            !SPECIAL_NPC_MODELS.contains(&model.as_str()) && !e.starts_with("flg_12:")
        })
        .map(|(_, e)| e.as_str())
        .collect();
    assert!(
        unexpected.is_empty(),
        "unexpected refusals:\n{}",
        unexpected.join("\n")
    );
    assert!(ok > 0);
}
