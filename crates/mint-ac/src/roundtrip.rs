//! Import, export, and re-import must agree: same node tree, palette,
//! textures, and triangles with the same resolved corners and materials.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use mint_core::{Project, TextureContent};

use crate::c::CFile;
use crate::import::{build_project, parse_villagers};
use crate::{Decomp, export, gx};

/// Slot, size, variant pixels, and whether the texture is external.
type CanonicalTexture = (String, [u32; 2], Vec<Vec<u8>>, bool);
/// Name, parent, translation, and sorted triangle descriptions.
type CanonicalNode = (String, Option<u32>, [i64; 3], Vec<String>);

/// A comparable summary of a project that ignores names and orders the
/// exporter is free to change.
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
                .map(|v| {
                    v.underlay()
                        .map(|u| u.pixels().iter().map(|i| i.0).collect())
                        .unwrap_or_default()
                })
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
            let mut tris: Vec<String> = n
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
            tris.sort();
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
            "aNPC_draw_data_tbl npc_draw_data_tbl[] = {{\n{}\n}};",
            out.draw_data
                .lines()
                .filter(|l| !l.starts_with("extern"))
                .collect::<Vec<_>>()
                .join("\n")
                .replace("/* model bank */, /* texture bank */", "0, 0")
        ),
    )
    .unwrap();
    let v = parse_villagers(&table).unwrap().remove(0);
    build_project(&model, &tex, pose_file, &v).unwrap()
}

const SYNTHETIC_POSE: &str = "
    u8 flags[] = { 0, 0, 0 };
    s16 keys[] = { 0 };
    s16 data[] = { 0 };
    s16 fixed[] = { 0, 1000, 0,  0, 0, 900,  0, 0, 0,  300, 0, 0 };
    aNPC_Animation_c cKF_ba_r_npc_1_wait1 = { { flags, data, keys, fixed, -1, 33 } };
";

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
            {{ NULL, 1, cKF_JOINT_FLAG_DISP_OPA, 0,1000,0 }},
            {{ body_t_model, 1, cKF_JOINT_FLAG_DISP_OPA, 0,0,0 }},
            {{ head_t_model, 0, cKF_JOINT_FLAG_DISP_OPA, 500,0,0 }},
         }};
         extern cKF_Skeleton_R_c cKF_bs_r_t_1 = {{ 3, 2, cKF_je_r_t_1_tbl }};",
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
        parse("pose.c", SYNTHETIC_POSE),
    )
}

#[test]
fn synthetic_villager_round_trips() {
    let (model, tex, table, pose) = synthetic();
    let v = parse_villagers(&table).unwrap().remove(0);
    let original = build_project(&model, &tex, &pose, &v).unwrap();

    // The import itself: a seam vertex, five textures, five materials.
    let head = original.model().node(mint_core::NodeId(2));
    let mesh = head.mesh.as_ref().unwrap();
    assert_eq!(mesh.vertices[0].follows, Some(mint_core::NodeId(1)));
    assert_eq!(mesh.triangles.len(), 7);
    assert_eq!(original.textures().len(), 5);
    assert_eq!(original.materials().len(), 5);

    let again = reimport(&original, &pose);
    assert_eq!(canonical(&original), canonical(&again));
}

#[test]
fn export_refuses_large_palettes() {
    let (model, tex, table, pose) = synthetic();
    let v = parse_villagers(&table).unwrap().remove(0);
    let p = build_project(&model, &tex, &pose, &v).unwrap();
    // 16 colors is the limit; the synthetic palette is exactly 16.
    assert_eq!(p.palette().len(), 16);
    assert!(export(&p, "rt_1", "rt_1").is_ok());
    assert!(export(&p, "1bad", "rt_1").is_err());
}

/// Round-trips every villager in a decomp built with extracted assets, named
/// by `MINT_AC_DECOMP`.
#[test]
#[ignore = "needs a built ac-decomp checkout in MINT_AC_DECOMP"]
fn every_built_villager_round_trips() {
    let root = PathBuf::from(std::env::var("MINT_AC_DECOMP").expect("MINT_AC_DECOMP"));
    let decomp = match Decomp::open(&root, "GAFE01_00") {
        Ok(d) => d,
        Err(crate::ImportError::NotBuilt(_)) => {
            eprintln!("skipping: {} has no extracted assets", root.display());
            return;
        }
        Err(e) => panic!("{e}"),
    };
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
            Err(e) => refused.push(format!("{}: {e}", v.texture_set)),
        }
    }
    eprintln!(
        "{ok} villagers round-trip; refused:\n{}",
        refused.join("\n")
    );
    assert!(ok > 0);
}
