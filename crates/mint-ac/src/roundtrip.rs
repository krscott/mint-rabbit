//! Import, export, and re-import must agree: same node tree, palette,
//! textures, and triangles with the same resolved corners and materials.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use mint_core::{
    Corner, Mesh, Model, NodeId, Palette, Project, Rgba8, TextureContent, Transform, Triangle,
    Vertex, model_file,
};

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
                .map(|v| v.rasterize().pixels().iter().map(|i| i.0).collect())
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
                            // Adding zero turns -0.0, from float noise, into 0.0.
                            let st = (c.uv * size * 32.0).round() + 0.0;
                            let nrm = (v.normal * 127.0).round() + 0.0;
                            let _ = write!(
                                s,
                                " p{:?} n{:?} f{} st{:?}",
                                (v.position.round() + 0.0).to_array(),
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
/// Parses one villager table entry.
fn table_row(entry: &str) -> crate::VillagerRef {
    let table = CFile::parse_str(
        Path::new("table.c"),
        &format!("aNPC_draw_data_c npc_draw_data_tbl[] = {{\n{entry}}};"),
    )
    .unwrap();
    parse_villagers(&table).unwrap().remove(0)
}

fn reimport(project: &Project, pose_file: &CFile) -> Project {
    let out = export(project, "rt_1", "rt_1").unwrap();
    let model = CFile::parse_str(Path::new("rt_1.c"), &out.model_c).unwrap();
    let tex = CFile::parse_str(Path::new("rt_1_tex.c"), &out.textures.texture_c).unwrap();
    let v = table_row(&out.textures.table_entry(None).unwrap());
    build_project(&model, &tex, pose_file, &v).unwrap()
}

/// `project` with its model sent to a model file and read back.
fn through_model_file(project: &Project) -> Project {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model.glb");
    let variants = vec![0; project.textures().len()];
    model_file::write(project, &path, &variants, crate::METERS_PER_UNIT).unwrap();
    let model = model_file::read(project, &path, crate::METERS_PER_UNIT).unwrap();
    let mut out = project.clone();
    out.set_model(model).unwrap();
    out
}

/// `project` with each triangle in the mesh of the deepest node its corners
/// follow, as reading a model file places it, and how many triangles moved.
fn by_deepest_corner(project: &Project) -> (Project, usize) {
    let model = project.model();
    let mut depth = vec![0usize; model.nodes().len()];
    for id in model.ids() {
        if let Some(p) = model.node(id).parent() {
            depth[id.index()] = depth[p.index()] + 1;
        }
    }
    let mut meshes: Vec<Option<Mesh>> = vec![None; model.nodes().len()];
    let mut moved = 0;
    for id in model.ids() {
        let Some(mesh) = &model.node(id).mesh else {
            continue;
        };
        for t in &mesh.triangles {
            let followed = t
                .corners
                .map(|c| mesh.vertices[c.vertex as usize].followed(id));
            let owner = *followed
                .iter()
                .max_by_key(|f| (depth[f.index()], std::cmp::Reverse(f.0)))
                .unwrap();
            moved += usize::from(owner != id);
            let out = meshes[owner.index()].get_or_insert_with(Mesh::default);
            let corners = t.corners.map(|c| {
                let v = mesh.vertices[c.vertex as usize];
                let f = v.followed(id);
                out.vertices.push(Vertex {
                    follows: (f != owner).then_some(f),
                    ..v
                });
                Corner {
                    vertex: out.vertices.len() as u32 - 1,
                    uv: c.uv,
                }
            });
            out.triangles.push(Triangle { corners, ..*t });
        }
    }
    let mut out = model.clone();
    for (id, mesh) in model.ids().zip(meshes) {
        out.node_mut(id).mesh = mesh;
    }
    let mut p = project.clone();
    p.set_model(out).unwrap();
    (p, moved)
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

/// The synthetic villager's table: one entry, texture set `t_1`.
fn synthetic_table() -> String {
    let eyes: Vec<String> = (1..=8).map(|k| format!("t_1_eye{k}_TA_tex_txt")).collect();
    let mouths: Vec<String> = (1..=6)
        .map(|k| format!("t_1_mouth{k}_TA_tex_txt"))
        .collect();
    format!(
        "aNPC_draw_data_c npc_draw_data_tbl[] = {{
            {{ 1, 2, &cKF_bs_r_t_1, {{ t_1_tmem_txt, t_1_pal, {{ {} }}, {{ {} }}, 0, 32, 128 }}, 0.009, 0 }},
         }};",
        eyes.join(", "),
        mouths.join(", ")
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
    let parse = |name: &str, text: &str| CFile::parse_str(Path::new(name), text).unwrap();
    (
        parse("t_1.c", &model),
        parse("t_1_tex.c", &tex),
        parse("table.c", &synthetic_table()),
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

#[test]
fn synthetic_villager_round_trips_through_a_model_file() {
    let (original, pose) = synthetic_project();
    let again = through_model_file(&original);
    assert_eq!(canonical(&original), canonical(&again));
    assert_eq!(canonical(&original), canonical(&reimport(&again, &pose)));
}

/// Export writes what the layers paint over the base layer, and hidden layers
/// are left out.
#[test]
fn export_rasterizes_painted_layers() {
    use glam::Vec2;
    use mint_core::{Layer, PaletteIndex, Shape, Style, TextureId, VariantRef, shape};
    let (mut p, pose) = synthetic_project();
    let texture = p
        .textures()
        .iter()
        .position(|t| t.slot.0 == crate::SLOT_EYES)
        .unwrap();
    let r = VariantRef {
        texture: TextureId(texture as u32),
        variant: 2,
    };
    let size = p.texture(r.texture).size().as_vec2();
    // A shown ellipse, with a hidden rectangle over all of it.
    let shapes = [
        Shape::new(
            shape::ellipse(size / 2.0, size / 3.0),
            Style::Fill,
            PaletteIndex(3),
        ),
        Shape::new(
            shape::rectangle(Vec2::ZERO, size),
            Style::Fill,
            PaletteIndex(5),
        ),
    ];
    let base = p.variant(r).rasterize();
    p.edit_variant(r, |v| {
        for (i, s) in shapes.into_iter().enumerate() {
            let mut layer = Layer::new(format!("layer {i}"));
            layer.insert_shape(0, s.unwrap());
            v.insert_layer(v.layers().len(), layer);
            v.layers_mut().last_mut().unwrap().visible = i == 0;
        }
    })
    .unwrap();
    let painted = p.variant(r).rasterize();
    let center = painted.pixel(size.x as u32 / 2, size.y as u32 / 2);
    assert_eq!(center, PaletteIndex(3));
    assert_ne!(painted, base);

    // It comes back as the base layer alone.
    let again = reimport(&p, &pose);
    assert_eq!(again.variant(r).layers().len(), 1);
    assert_eq!(again.variant(r).rasterize(), painted);
    assert_eq!(canonical(&p), canonical(&again));
}

/// A texture-only export replaces an existing texture set's source in a
/// decomp, and only where the villager table will still link.
#[test]
fn texture_export_replaces_a_texture_set_in_the_decomp() {
    use crate::{Decomp, WriteError, export_textures};
    use mint_core::{Layer, PaletteIndex, Shape, Style, TextureId, VariantRef, shape};

    let (mut p, _) = synthetic_project();
    let dir = tempfile::tempdir().unwrap();
    let write = |rel: &str, text: &str| {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    let full = export(&p, "t_1", "t_1").unwrap();
    write("src/data/npc/model/mdl/t_1.c", &full.model_c);
    write("src/data/npc/npc_1_wait1.c", &synthetic_pose());
    write("src/data/npc/npc_draw_data.c", &synthetic_table());
    write("src/data/npc/model/tex/t_1.c", &full.textures.texture_c);
    std::fs::create_dir_all(dir.path().join("build/GAFE01_00/include/assets/npc")).unwrap();
    let decomp = Decomp::open(dir.path(), "GAFE01_00").unwrap();

    let r = VariantRef {
        texture: TextureId(0),
        variant: 0,
    };
    let size = p.texture(r.texture).size().as_vec2();
    let dot = Shape::new(
        shape::ellipse(size / 2.0, size / 4.0),
        Style::Fill,
        PaletteIndex(7),
    );
    p.edit_variant(r, |v| {
        let mut layer = Layer::new("paint");
        layer.insert_shape(0, dot.unwrap());
        v.insert_layer(v.layers().len(), layer);
    })
    .unwrap();
    let painted = export_textures(&p, "t_1").unwrap();
    assert_eq!(painted.model, "t_1");
    let path = decomp.write_texture_set(&painted).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), painted.texture_c);

    let v = decomp.villagers().unwrap().remove(0);
    let again = decomp.import(&v).unwrap();
    assert_eq!(again.variant(r).rasterize(), p.variant(r).rasterize());

    // A folder export for a new texture set is checked against the base
    // villager's model too.
    let folder = tempfile::tempdir().unwrap();
    let new_set = export_textures(&p, "t_9").unwrap();
    let [c, notes] = decomp
        .write_texture_folder(&new_set, "t_1", folder.path())
        .unwrap();
    assert_eq!(std::fs::read_to_string(c).unwrap(), new_set.texture_c);
    assert!(
        std::fs::read_to_string(notes)
            .unwrap()
            .contains("t_9_tmem_txt")
    );
    assert!(matches!(
        decomp.write_texture_folder(&painted, "t_1", folder.path()),
        Err(WriteError::Exists(set, _)) if set == "t_1"
    ));
    let mut shrunk = new_set.clone();
    shrunk.texture_sizes[0].1 /= 2;
    assert!(matches!(
        decomp.write_texture_folder(&shrunk, "t_1", folder.path()),
        Err(WriteError::TextureSizes { .. })
    ));

    // A new villager's entry copies the base's settings.
    let entry = painted.table_entry(Some(&v)).unwrap();
    assert!(entry.contains("1, TEXTURE_BANK"), "{entry}");
    assert!(entry.contains("0, 32, 128 }"), "{entry}");
    assert!(entry.contains("0.009, 0"), "{entry}");

    let mut renamed = painted.clone();
    renamed.texture_set = "t_2".into();
    assert!(matches!(
        decomp.write_texture_set(&renamed),
        Err(WriteError::NotInTable(set)) if set == "t_2"
    ));
    let mut other = painted.clone();
    other.model = "u_1".into();
    assert!(matches!(
        decomp.write_texture_set(&other),
        Err(WriteError::OtherModel { .. })
    ));
    let mut resized = painted.clone();
    resized.texture_sizes[0].1 /= 2;
    assert!(matches!(
        decomp.write_texture_set(&resized),
        Err(WriteError::TextureSizes { .. })
    ));
    let mut swapped = painted.clone();
    swapped.symbols.eyes.swap(1, 2);
    assert!(matches!(
        decomp.write_texture_set(&swapped),
        Err(WriteError::TableNames { .. })
    ));
    // The file defines an array the export would drop.
    write(
        "src/data/npc/model/tex/t_1.c",
        &format!(
            "{}unsigned char t_1_extra[] = {{ 0 }};\n",
            painted.texture_c
        ),
    );
    let Err(WriteError::Symbols { missing, extra, .. }) = decomp.write_texture_set(&painted) else {
        panic!("expected a symbol mismatch");
    };
    assert_eq!((missing, extra), (vec!["t_1_extra".to_owned()], vec![]));
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .ends_with("t_1_extra[] = { 0 };\n")
    );
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
        mint_core::ExportTarget::new(target),
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

    // A 24-wide body texture cannot repeat across U.
    let mut textures = p.textures().to_vec();
    let body = textures
        .iter()
        .position(|t| t.slot.0 == crate::SLOT_BODY)
        .unwrap();
    let narrow = mint_core::IndexedImage::new(
        glam::UVec2::new(24, 8),
        vec![mint_core::PaletteIndex(0); 24 * 8],
    )
    .unwrap();
    textures[body] = mint_core::Texture::painted(
        "narrow",
        mint_core::Slot(crate::SLOT_BODY.into()),
        glam::UVec2::new(24, 8),
        [mint_core::Variant::imported("1", narrow)],
    )
    .unwrap();
    let mut materials = p.materials().to_vec();
    for m in &mut materials {
        if m.texture == mint_core::TextureId(body as u32) {
            m.wrap = [mint_core::Wrap::Repeat, mint_core::Wrap::Clamp];
        }
    }
    let narrow = Project::new(
        p.target().clone(),
        p.palette().clone(),
        p.model().clone(),
        textures,
        materials,
        p.pose().clone(),
    )
    .unwrap();
    assert!(matches!(
        export(&narrow, "rt_1", "rt_1"),
        Err(ExportError::Gx {
            source: gx::GxError::WrapNeedsPowerOfTwo { .. },
            ..
        })
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
    let mut moved = Vec::new();
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
                // A triangle whose corners all follow another node moves to
                // that node's mesh.
                let (expected, n) = by_deepest_corner(&p);
                if n > 0 {
                    moved.push(format!("{} ({n})", v.texture_set));
                }
                let sent = through_model_file(&p);
                assert_eq!(
                    canonical(&expected),
                    canonical(&sent),
                    "{} changed through a model file",
                    v.texture_set
                );
                // Reading welds vertices the import kept apart, so the
                // export may order its vertices differently.
                assert_eq!(
                    canonical(&expected),
                    canonical(&reimport(&sent, &pose)),
                    "{} exports differently after a model file",
                    v.texture_set
                );
                let dir = tempfile::tempdir().unwrap();
                mint_core::file::save(&p, dir.path()).unwrap();
                let loaded = mint_core::file::load(dir.path()).unwrap();
                assert!(loaded == p, "{} changed in save and load", v.texture_set);
                // Its texture-only export could replace its own source, and
                // a table entry based on it parses back to the same row.
                let textures = crate::export_textures(&p, &v.texture_set).unwrap();
                let mut row = table_row(&textures.table_entry(Some(&v)).unwrap());
                row.index = v.index;
                assert_eq!(row, v, "{}", v.texture_set);
                decomp
                    .check_texture_set(&textures)
                    .unwrap_or_else(|e| panic!("{}: {e}", v.texture_set));
                ok += 1;
            }
            Err(e) => refused.push((v.model.clone(), format!("{}: {e}", v.texture_set))),
        }
    }
    eprintln!(
        "{ok} villagers round-trip, also through a model file, save and load, and could be replaced"
    );
    eprintln!(
        "triangles that move to another mesh through a model file: {}",
        moved.join(", ")
    );
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
