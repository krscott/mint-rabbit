//! Writes a decomp-shaped directory holding one generated villager, for
//! trying mint-rabbit without a disc image:
//!
//! ```sh
//! cargo run -p mint-ac --example demo_decomp -- /tmp/demo-decomp
//! cargo run -p mint-rabbit -- --decomp /tmp/demo-decomp
//! ```

use std::f32::consts::{PI, TAU};
use std::path::Path;

use glam::{UVec2, Vec2, Vec3};
use mint_core::{
    AlphaMode, Corner, IndexedImage, Material, MaterialId, Mesh, Model, NodeId, Palette,
    PaletteIndex, Pose, Project, Rgba8, RgbaImage, Slot, TargetId, Texture, TextureId, Transform,
    Triangle, Vertex, Wrap,
};

const PALETTE: [[u8; 3]; 16] = [
    [255, 255, 255],
    [24, 24, 32],
    [240, 200, 150],
    [200, 150, 100],
    [120, 80, 50],
    [230, 90, 90],
    [90, 160, 230],
    [250, 220, 90],
    [120, 200, 120],
    [180, 120, 200],
    [255, 180, 200],
    [60, 60, 70],
    [160, 160, 170],
    [210, 210, 220],
    [100, 60, 30],
    [255, 140, 60],
];

fn image(size: UVec2, f: impl Fn(u32, u32) -> u8) -> IndexedImage {
    let pixels = (0..size.y)
        .flat_map(|y| (0..size.x).map(move |x| (x, y)))
        .map(|(x, y)| PaletteIndex(f(x, y)))
        .collect();
    IndexedImage::new(size, pixels).expect("size matches")
}

fn quad(
    mesh: &mut Mesh,
    corners: [Vec3; 4],
    normal: Vec3,
    uv: [Vec2; 4],
    material: MaterialId,
    follows: [Option<NodeId>; 4],
) {
    let base = mesh.vertices.len() as u32;
    for (p, f) in corners.into_iter().zip(follows) {
        mesh.vertices.push(Vertex {
            position: p,
            normal,
            follows: f,
        });
    }
    let c = |i: u32| Corner {
        vertex: base + i,
        uv: uv[i as usize],
    };
    mesh.triangles.push(Triangle {
        corners: [c(0), c(1), c(2)],
        material,
    });
    mesh.triangles.push(Triangle {
        corners: [c(0), c(2), c(3)],
        material,
    });
}

fn main() {
    let out = std::env::args()
        .nth(1)
        .expect("usage: demo_decomp OUTPUT_DIR");
    let root = Path::new(&out);

    let body_size = UVec2::new(32, 32);
    let head_size = UVec2::new(32, 32);
    let face_size = UVec2::new(32, 16);
    let textures = vec![
        Texture::painted(
            "eyes",
            Slot(mint_ac::SLOT_EYES.into()),
            face_size,
            (0..8).map(|k| {
                let r = 2.0 + k as f32 * 0.6;
                let img = image(face_size, |x, y| {
                    let d = |cx: f32| ((x as f32 - cx).powi(2) + (y as f32 - 8.0).powi(2)).sqrt();
                    if d(8.0) < r || d(24.0) < r {
                        1
                    } else if d(8.0) < 5.5 || d(24.0) < 5.5 {
                        0
                    } else {
                        2
                    }
                });
                (format!("{}", k + 1), Some(img))
            }),
        )
        .unwrap(),
        Texture::painted(
            "mouth",
            Slot(mint_ac::SLOT_MOUTH.into()),
            face_size,
            (0..6).map(|k| {
                let img = image(face_size, |x, y| {
                    let curve = 6.0 + ((x as f32 - 16.0) / 6.0).powi(2) * (k as f32 - 2.5) * 0.4;
                    if (y as f32 - curve).abs() < 1.2 && (6..26).contains(&x) {
                        5
                    } else {
                        2
                    }
                });
                (format!("{}", k + 1), Some(img))
            }),
        )
        .unwrap(),
        Texture::external(
            mint_ac::SLOT_CLOTHING,
            Slot(mint_ac::SLOT_CLOTHING.into()),
            RgbaImage::filled(UVec2::new(32, 32), Rgba8::new(200, 200, 210, 255)),
        ),
        Texture::painted(
            "body",
            Slot(mint_ac::SLOT_BODY.into()),
            body_size,
            [(
                "1".to_owned(),
                Some(image(body_size, |x, y| {
                    if (x / 4 + y / 4) % 2 == 0 { 6 } else { 13 }
                })),
            )],
        )
        .unwrap(),
        Texture::painted(
            "head",
            Slot(mint_ac::SLOT_BODY.into()),
            head_size,
            [(
                "1".to_owned(),
                Some(image(head_size, |x, y| {
                    if y > 28 {
                        4
                    } else if (x + y) % 11 == 0 {
                        3
                    } else {
                        2
                    }
                })),
            )],
        )
        .unwrap(),
    ];
    let white = Rgba8::new(255, 255, 255, 255);
    let material = |t: u32, wrap: Wrap| Material {
        texture: TextureId(t),
        wrap: [wrap, Wrap::Clamp],
        alpha: AlphaMode::Opaque,
        double_sided: false,
        tint: white,
    };
    let materials = vec![
        material(0, Wrap::Clamp),
        material(1, Wrap::Clamp),
        material(2, Wrap::Repeat),
        material(3, Wrap::Repeat),
        material(4, Wrap::Repeat),
    ];
    let (eyes, mouth, clothing, body_m, head_m) = (
        MaterialId(0),
        MaterialId(1),
        MaterialId(2),
        MaterialId(3),
        MaterialId(4),
    );

    let mut model = Model::new(
        "root",
        Transform::from_translation(Vec3::new(0.0, 300.0, 0.0)),
    );
    let body = model.add_child(NodeId::ROOT, "body", Transform::default());
    let head = model.add_child(
        body,
        "head",
        Transform::from_translation(Vec3::new(0.0, 250.0, 0.0)),
    );

    // Body: a box whose sides wear the clothing texture.
    let mut mesh = Mesh::default();
    let h = Vec3::new(150.0, 125.0, 100.0);
    let faces = [
        (Vec3::X, Vec3::Z),
        (Vec3::NEG_X, Vec3::NEG_Z),
        (Vec3::Z, Vec3::NEG_X),
        (Vec3::NEG_Z, Vec3::X),
        (Vec3::Y, Vec3::X),
        (Vec3::NEG_Y, Vec3::X),
    ];
    for (k, (n, u)) in faces.into_iter().enumerate() {
        let v = n.cross(u);
        let corner = |a: f32, b: f32| (n + u * a + v * b) * h;
        let uv = [
            Vec2::new(0.0, 1.0),
            Vec2::new(2.0, 1.0),
            Vec2::new(2.0, 0.0),
            Vec2::new(0.0, 0.0),
        ];
        let m = if k < 4 { clothing } else { body_m };
        quad(
            &mut mesh,
            [
                corner(-1.0, -1.0),
                corner(1.0, -1.0),
                corner(1.0, 1.0),
                corner(-1.0, 1.0),
            ],
            n,
            uv,
            m,
            [None; 4],
        );
    }
    model.node_mut(body).mesh = Some(mesh);

    // Head: a sphere whose bottom ring follows the body, plus face quads.
    let mut mesh = Mesh::default();
    let (lat, lon, r) = (6u32, 12u32, 140.0);
    for i in 0..=lat {
        for j in 0..=lon {
            let (theta, phi) = (i as f32 / lat as f32 * PI, j as f32 / lon as f32 * TAU);
            let n = Vec3::new(
                theta.sin() * phi.sin(),
                theta.cos(),
                theta.sin() * phi.cos(),
            );
            let follows = (i == lat).then_some(body);
            // A seam vertex is stored in the space of the node it follows.
            let position = n * r
                + if follows.is_some() {
                    Vec3::new(0.0, 250.0, 0.0)
                } else {
                    Vec3::ZERO
                };
            mesh.vertices.push(Vertex {
                position,
                normal: n,
                follows,
            });
        }
    }
    let idx = |i: u32, j: u32| i * (lon + 1) + j;
    for i in 0..lat {
        for j in 0..lon {
            let uv = |i: u32, j: u32| Vec2::new(j as f32 / lon as f32 * 2.0, i as f32 / lat as f32);
            let c = |i: u32, j: u32| Corner {
                vertex: idx(i, j),
                uv: uv(i, j),
            };
            mesh.triangles.push(Triangle {
                corners: [c(i, j), c(i + 1, j), c(i + 1, j + 1)],
                material: head_m,
            });
            mesh.triangles.push(Triangle {
                corners: [c(i, j), c(i + 1, j + 1), c(i, j + 1)],
                material: head_m,
            });
        }
    }
    let face_uv = [
        Vec2::new(0.0, 1.0),
        Vec2::new(1.0, 1.0),
        Vec2::new(1.0, 0.0),
        Vec2::new(0.0, 0.0),
    ];
    let face = |y: f32, w: f32, hh: f32| {
        [
            Vec3::new(-w, y - hh, r + 2.0),
            Vec3::new(w, y - hh, r + 2.0),
            Vec3::new(w, y + hh, r + 2.0),
            Vec3::new(-w, y + hh, r + 2.0),
        ]
    };
    quad(
        &mut mesh,
        face(25.0, 70.0, 35.0),
        Vec3::Z,
        face_uv,
        eyes,
        [None; 4],
    );
    quad(
        &mut mesh,
        face(-45.0, 50.0, 25.0),
        Vec3::Z,
        face_uv,
        mouth,
        [None; 4],
    );
    model.node_mut(head).mesh = Some(mesh);

    let palette = Palette::from_colors(PALETTE.map(|[r, g, b]| Rgba8::new(r, g, b, 255))).unwrap();
    let project = Project::new(
        TargetId(mint_ac::TARGET.into()),
        palette,
        model,
        textures,
        materials,
        Pose::default(),
    )
    .unwrap();
    let export = mint_ac::export(&project, "demo_1", "demo_1").unwrap();

    let save = |rel: &str, text: &str| {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    save("src/data/npc/model/mdl/demo_1.c", &export.model_c);
    save("src/data/npc/model/tex/demo_1.c", &export.texture_c);
    let entry: String = export
        .draw_data
        .lines()
        .filter(|l| !l.starts_with("extern"))
        .fold(String::new(), |mut s, l| {
            s.push_str(l);
            s.push('\n');
            s
        })
        .replace("/* model bank */, /* texture bank */", "0, 0");
    save(
        "src/data/npc/npc_draw_data.c",
        &format!("extern aNPC_draw_data_c npc_draw_data_tbl[] = {{\n{entry}}};\n"),
    );
    // Three joints: root translation, then a Z/Y/X rotation per joint, all
    // fixed, tilting the head a little.
    save(
        "src/data/npc/npc_1_wait1.c",
        "u8 flags[] = { 0, 0, 0 };\ns16 keys[] = { 0 };\ns16 data[] = { 0 };\n\
         s16 fixed[] = { 0, 300, 0,  0, 0, 0,  0, 0, 0,  0, 150, 80 };\n\
         aNPC_Animation_c cKF_ba_r_npc_1_wait1 = { { flags, data, keys, fixed, -1, 1 } };\n",
    );
    std::fs::create_dir_all(root.join("build/GAFE01_00/include/assets/npc")).unwrap();
    println!("wrote demo decomp to {}", root.display());
}
