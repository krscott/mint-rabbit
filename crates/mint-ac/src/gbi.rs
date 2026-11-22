//! Interprets villager display lists: the `gs*` macro calls that load
//! matrices, vertices, and textures and draw triangles.

use std::collections::HashMap;
use std::fmt;

use mint_core::{AlphaMode, Rgba8, Wrap};

use crate::c::{Expr, Init};

/// Bytes between consecutive joint matrices in segment 0x0D.
const MTX_SIZE: i64 = 64;
/// Entries in the vertex cache addressed by 5-bit triangle indices.
pub const VERTEX_CACHE: usize = 32;

/// One `Vtx` as stored in the decomp.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawVtx {
    pub pos: [i16; 3],
    pub flag: u16,
    /// Texture coordinates in 1/32 texel units.
    pub st: [i16; 2],
    /// Normal components as signed bytes. The fourth byte, alpha, is unused:
    /// villager combiners take alpha from the texture alone.
    pub normal: [i8; 3],
}

impl RawVtx {
    pub fn from_init(init: &Init) -> Option<Self> {
        let mut v = Vec::with_capacity(10);
        flatten_ints(init, &mut v)?;
        let [x, y, z, flag, s, t, nx, ny, nz, a]: [i64; 10] = v.try_into().ok()?;
        let i16_ = |v: i64| i16::try_from(v).ok();
        let u8_ = |v: i64| u8::try_from(v).ok();
        u8_(a)?;
        Some(Self {
            pos: [i16_(x)?, i16_(y)?, i16_(z)?],
            flag: u16::try_from(flag).ok()?,
            st: [i16_(s)?, i16_(t)?],
            normal: [u8_(nx)? as i8, u8_(ny)? as i8, u8_(nz)? as i8],
        })
    }
}

fn flatten_ints(init: &Init, out: &mut Vec<i64>) -> Option<()> {
    match init {
        Init::Expr(e) => out.push(e.as_int()?),
        Init::List(items) => {
            for i in items {
                flatten_ints(i, out)?;
            }
        }
    }
    Some(())
}

/// Which texture a load refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TexSource {
    Eye,
    Mouth,
    Clothing,
    /// A texture at a byte offset in the body buffer.
    Body {
        offset: u32,
    },
}

impl fmt::Display for TexSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TexSource::Eye => f.write_str("eye"),
            TexSource::Mouth => f.write_str("mouth"),
            TexSource::Clothing => f.write_str("clothing"),
            TexSource::Body { offset } => write!(f, "body+{offset:#05x}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TexLoad {
    pub source: TexSource,
    pub width: u32,
    pub height: u32,
    pub wrap: [Wrap; 2],
}

/// The full draw state a triangle is drawn with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DrawState {
    pub tex: TexLoad,
    pub alpha: AlphaMode,
    pub double_sided: bool,
    pub tint: Rgba8,
}

/// A vertex as one display list uses it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UsedVertex {
    /// Index into the model's vertex array.
    pub index: usize,
    /// Joint matrix slot it was loaded under, or `None` for the caller's
    /// matrix (the joint's own).
    pub matrix: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DlTriangle {
    pub vertices: [UsedVertex; 3],
    pub state: DrawState,
}

/// The geometry one display list draws.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DlMesh {
    pub triangles: Vec<DlTriangle>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("command {index} ({command}): {problem}")]
pub struct DlError {
    pub index: usize,
    pub command: String,
    pub problem: String,
}

const STANDARD_COMBINE: [&str; 16] = [
    "TEXEL0",
    "0",
    "SHADE",
    "0",
    "0",
    "0",
    "0",
    "TEXEL0",
    "PRIMITIVE",
    "0",
    "COMBINED",
    "0",
    "0",
    "0",
    "0",
    "COMBINED",
];
const REQUIRED_GEOMETRY: [&str; 5] = [
    "G_ZBUFFER",
    "G_SHADE",
    "G_FOG",
    "G_LIGHTING",
    "G_SHADING_SMOOTH",
];

/// The identifiers OR-ed together in a flag expression.
fn flag_names(e: &Expr) -> Option<Vec<&str>> {
    match e {
        Expr::Ident(s) => Some(vec![s]),
        Expr::Binary(crate::c::BinOp::Or, l, r) => {
            let mut v = flag_names(l)?;
            v.extend(flag_names(r)?);
            Some(v)
        }
        _ => None,
    }
}

struct Interp<'a> {
    vertex_array: &'a str,
    matrix: Option<u32>,
    cache: [Option<UsedVertex>; VERTEX_CACHE],
    tex: Option<TexLoad>,
    alpha: Option<AlphaMode>,
    double_sided: Option<bool>,
    tint: Rgba8,
    combine_ok: bool,
    pending_triangles: usize,
    out: DlMesh,
}

/// Interprets one display list whose vertices come from `vertex_array`.
///
/// Draw state never carries over from another display list: each list must
/// set everything its triangles use.
pub fn interpret(
    list: &[Init],
    vertex_array: &str,
    vertex_count: usize,
) -> Result<DlMesh, DlError> {
    let mut it = Interp {
        vertex_array,
        matrix: None,
        cache: [None; VERTEX_CACHE],
        tex: None,
        alpha: None,
        double_sided: None,
        tint: Rgba8::new(255, 255, 255, 255),
        combine_ok: false,
        pending_triangles: 0,
        out: DlMesh::default(),
    };
    for (index, item) in list.iter().enumerate() {
        let fail = |problem: String| DlError {
            index,
            command: item
                .as_expr()
                .map_or_else(|| "{...}".into(), Expr::to_string),
            problem,
        };
        let Some(Expr::Call(name, args)) = item.as_expr() else {
            return Err(fail("not a macro call".into()));
        };
        if it.command(name, args, vertex_count).map_err(fail)? {
            return Ok(it.out);
        }
    }
    Err(DlError {
        index: list.len(),
        command: String::new(),
        problem: "display list has no gsSPEndDisplayList".into(),
    })
}

impl Interp<'_> {
    fn int(args: &[Expr], i: usize) -> Result<i64, String> {
        args.get(i)
            .and_then(Expr::as_int)
            .ok_or_else(|| format!("argument {} is not an integer", i + 1))
    }

    fn arity(args: &[Expr], n: usize) -> Result<(), String> {
        if args.len() == n {
            Ok(())
        } else {
            Err(format!("expected {n} arguments, found {}", args.len()))
        }
    }

    /// Runs one command; returns `true` at the end of the list.
    fn command(&mut self, name: &str, args: &[Expr], vertex_count: usize) -> Result<bool, String> {
        // emu64 reads the words after a triangle-group header as triangle
        // data, whatever they are.
        if self.pending_triangles != 0 && name != "gsSPNTriangles_5b" {
            return Err(format!(
                "{} triangles of the group are still expected",
                self.pending_triangles
            ));
        }
        match name {
            "gsSPEndDisplayList" => return Ok(true),
            "gsSPMatrix" => {
                Self::arity(args, 2)?;
                let (sym, offset) = args[0]
                    .as_symbol_offset()
                    .ok_or("matrix address is not segment + offset")?;
                if sym != "anime_6_model" || offset % MTX_SIZE != 0 {
                    return Err(format!("unsupported matrix address {}", args[0]));
                }
                let flags = flag_names(&args[1]).unwrap_or_default();
                if !flags.contains(&"G_MTX_LOAD") || flags.contains(&"G_MTX_PUSH") {
                    return Err(format!("unsupported matrix flags {}", args[1]));
                }
                let slot = u32::try_from(offset / MTX_SIZE)
                    .map_err(|_| format!("unsupported matrix address {}", args[0]))?;
                self.matrix = Some(slot);
            }
            "gsSPVertex" => {
                Self::arity(args, 3)?;
                let Expr::AddrOf(target) = &args[0] else {
                    return Err("vertex address is not &array[index]".into());
                };
                let Expr::Index(array, first) = &**target else {
                    return Err("vertex address is not &array[index]".into());
                };
                if array.as_ident() != Some(self.vertex_array) {
                    return Err(format!(
                        "vertices come from {array}, not {}",
                        self.vertex_array
                    ));
                }
                let index = |e: &Expr, what: &str| {
                    e.as_int()
                        .and_then(|v| usize::try_from(v).ok())
                        .ok_or_else(|| format!("vertex {what} {e} is not a non-negative integer"))
                };
                let first = index(first, "index")?;
                let count = index(&args[1], "count")?;
                let slot = index(&args[2], "slot")?;
                if slot.saturating_add(count) > VERTEX_CACHE
                    || first.saturating_add(count) > vertex_count
                {
                    return Err(format!(
                        "loads vertices {first}..{} into slots {slot}..{}, past the array or cache",
                        first + count,
                        slot + count
                    ));
                }
                for k in 0..count {
                    self.cache[slot + k] = Some(UsedVertex {
                        index: first + k,
                        matrix: self.matrix,
                    });
                }
            }
            "gsSPNTrianglesInit_5b" => {
                Self::arity(args, 10)?;
                // The macro stores the count minus one in 7 bits.
                let count = Self::int(args, 0)?;
                if !(1..=128).contains(&count) {
                    return Err(format!("triangle count {count} is outside 1..=128"));
                }
                self.pending_triangles = count as usize;
                self.triangles(&args[1..])?;
            }
            "gsSPNTriangles_5b" => {
                Self::arity(args, 12)?;
                if self.pending_triangles == 0 {
                    return Err("triangles without gsSPNTrianglesInit_5b".into());
                }
                self.triangles(args)?;
            }
            "gsDPLoadTextureBlock_4b_Dolphin" => self.load_texture(args)?,
            "gsDPSetRenderMode" => {
                Self::arity(args, 2)?;
                if args[0].as_ident() != Some("G_RM_FOG_SHADE_A") {
                    return Err(format!("unsupported first-cycle render mode {}", args[0]));
                }
                self.alpha = Some(match args[1].as_ident() {
                    Some("G_RM_AA_ZB_OPA_SURF2") => AlphaMode::Opaque,
                    Some("G_RM_AA_ZB_TEX_EDGE2") => AlphaMode::Cutout,
                    Some("G_RM_AA_ZB_XLU_SURF2") => AlphaMode::Translucent,
                    _ => return Err(format!("unsupported render mode {}", args[1])),
                });
            }
            "gsDPSetCombineLERP" => {
                let names: Vec<String> = args.iter().map(Expr::to_string).collect();
                if names != STANDARD_COMBINE {
                    return Err(
                        "unsupported color combiner; only TEXEL0*SHADE*PRIMITIVE is supported"
                            .into(),
                    );
                }
                self.combine_ok = true;
            }
            "gsSPLoadGeometryMode" => {
                Self::arity(args, 1)?;
                let flags = flag_names(&args[0]).ok_or("geometry mode is not a set of flags")?;
                if let Some(missing) = REQUIRED_GEOMETRY.iter().find(|f| !flags.contains(f)) {
                    return Err(format!("geometry mode lacks {missing}"));
                }
                if let Some(extra) = flags
                    .iter()
                    .find(|f| !REQUIRED_GEOMETRY.contains(f) && **f != "G_CULL_BACK")
                {
                    return Err(format!("unsupported geometry flag {extra}"));
                }
                self.double_sided = Some(!flags.contains(&"G_CULL_BACK"));
            }
            "gsDPSetPrimColor" => {
                Self::arity(args, 6)?;
                let c: Vec<u8> = (2..6)
                    .map(|i| {
                        Self::int(args, i).and_then(|v| u8::try_from(v).map_err(|e| e.to_string()))
                    })
                    .collect::<Result<_, _>>()?;
                self.tint = Rgba8::new(c[0], c[1], c[2], c[3]);
            }
            "gsSPTexture" => {
                let v: Vec<String> = args.iter().map(Expr::to_string).collect();
                if v != ["0", "0", "0", "0", "G_ON"] {
                    return Err("unsupported texture scale".into());
                }
            }
            "gsDPSetTileSize" => {
                Self::arity(args, 5)?;
                if Self::int(args, 1)? != 0 || Self::int(args, 2)? != 0 {
                    return Err("tile origin other than (0, 0) shifts texture coordinates".into());
                }
            }
            other => return Err(format!("unsupported command {other}")),
        }
        Ok(false)
    }

    fn load_texture(&mut self, args: &[Expr]) -> Result<(), String> {
        Self::arity(args, 9)?;
        let (sym, offset) = args[0]
            .as_symbol_offset()
            .ok_or("texture address is not segment + offset")?;
        let tlut = Self::int(args, 4)?;
        let (source, expected_tlut) = match (sym, offset) {
            ("anime_1_txt", 0) => (TexSource::Eye, 15),
            ("anime_2_txt", 0) => (TexSource::Mouth, 15),
            ("anime_3_txt", 0) => (TexSource::Clothing, 14),
            ("anime_4_txt", off) => match u32::try_from(off) {
                Ok(offset) => (TexSource::Body { offset }, 15),
                Err(_) => return Err(format!("unsupported texture address {}", args[0])),
            },
            _ => return Err(format!("unsupported texture address {}", args[0])),
        };
        if tlut != expected_tlut {
            return Err(format!(
                "{source} texture uses palette slot {tlut}, expected {expected_tlut}"
            ));
        }
        if args[1].as_ident() != Some("G_IM_FMT_CI") {
            return Err(format!("unsupported texture format {}", args[1]));
        }
        let wrap = |e: &Expr| match e.as_ident() {
            Some("GX_CLAMP") => Ok(Wrap::Clamp),
            Some("GX_REPEAT") => Ok(Wrap::Repeat),
            Some("GX_MIRROR") => Ok(Wrap::Mirror),
            _ => Err(format!("unsupported wrap mode {e}")),
        };
        if Self::int(args, 7)? != 0 || Self::int(args, 8)? != 0 {
            return Err("texture shifts are not supported".into());
        }
        let side = |i: usize| -> Result<u32, String> {
            let v = Self::int(args, i)?;
            u32::try_from(v).map_err(|_| format!("texture side {v} is out of range"))
        };
        let (width, height) = (side(2)?, side(3)?);
        crate::gx::check_size(width, height).map_err(|e| e.to_string())?;
        self.tex = Some(TexLoad {
            source,
            width,
            height,
            wrap: [wrap(&args[5])?, wrap(&args[6])?],
        });
        Ok(())
    }

    fn triangles(&mut self, indices: &[Expr]) -> Result<(), String> {
        for tri in indices.chunks(3) {
            if self.pending_triangles == 0 {
                break;
            }
            let state = DrawState {
                tex: self.tex.ok_or("triangle drawn before any texture load")?,
                alpha: self.alpha.ok_or("triangle drawn before a render mode")?,
                double_sided: self
                    .double_sided
                    .ok_or("triangle drawn before a geometry mode")?,
                tint: self.tint,
            };
            if !self.combine_ok {
                return Err("triangle drawn before a color combiner".into());
            }
            let mut vertices = [UsedVertex {
                index: 0,
                matrix: None,
            }; 3];
            for (k, e) in tri.iter().enumerate() {
                let slot = e.as_int().ok_or("triangle index is not an integer")? as usize;
                vertices[k] = self
                    .cache
                    .get(slot)
                    .copied()
                    .flatten()
                    .ok_or_else(|| format!("triangle uses empty vertex slot {slot}"))?;
            }
            self.out.triangles.push(DlTriangle { vertices, state });
            self.pending_triangles -= 1;
        }
        Ok(())
    }
}

/// Maps matrix slots to joint indices: slot N belongs to the N-th joint, in
/// table order, that has a display list.
#[must_use]
pub fn matrix_slots(joint_has_model: &[bool]) -> HashMap<u32, usize> {
    joint_has_model
        .iter()
        .enumerate()
        .filter(|(_, has)| **has)
        .enumerate()
        .map(|(slot, (joint, _))| (slot as u32, joint))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::c::CFile;

    fn list(text: &str) -> Vec<Init> {
        let f = CFile::parse_str(Path::new("t.c"), &format!("Gfx dl[] = {{ {text} }};")).unwrap();
        f.get("dl").unwrap().init.as_list().unwrap().to_vec()
    }

    const PRELUDE: &str = "
        gsSPTexture(0, 0, 0, 0, G_ON),
        gsSPLoadGeometryMode(G_ZBUFFER | G_SHADE | G_FOG | G_LIGHTING | G_SHADING_SMOOTH),
        gsDPSetRenderMode(G_RM_FOG_SHADE_A, G_RM_AA_ZB_TEX_EDGE2),
        gsDPSetCombineLERP(TEXEL0, 0, SHADE, 0, 0, 0, 0, TEXEL0, PRIMITIVE, 0, COMBINED, 0, 0, 0, 0, COMBINED),
        gsDPLoadTextureBlock_4b_Dolphin(anime_4_txt + 0x200, G_IM_FMT_CI, 16, 16, 15, GX_MIRROR, GX_CLAMP, 0, 0),
    ";

    #[test]
    fn tracks_seam_matrices_and_triangle_counts() {
        let dl = list(&format!(
            "gsSPMatrix(anime_6_model+sizeof(Mtx)*7, G_MTX_NOPUSH | G_MTX_LOAD | G_MTX_MODELVIEW),
             gsSPVertex(&v[0], 2, 0),
             gsSPMatrix(anime_6_model+sizeof(Mtx)*12, G_MTX_NOPUSH | G_MTX_LOAD | G_MTX_MODELVIEW),
             {PRELUDE}
             gsSPVertex(&v[2], 3, 2),
             gsSPNTrianglesInit_5b(5, 0, 1, 2, 1, 3, 2, 2, 3, 4),
             gsSPNTriangles_5b(0, 2, 4, 1, 4, 3, 0, 0, 0, 0, 0, 0),
             gsSPEndDisplayList(),"
        ));
        let mesh = interpret(&dl, "v", 5).unwrap();
        assert_eq!(mesh.triangles.len(), 5);
        let t = mesh.triangles[0];
        assert_eq!(
            t.vertices[0],
            UsedVertex {
                index: 0,
                matrix: Some(7)
            }
        );
        assert_eq!(
            t.vertices[2],
            UsedVertex {
                index: 2,
                matrix: Some(12)
            }
        );
        assert_eq!(t.state.tex.source, TexSource::Body { offset: 0x200 });
        assert_eq!(t.state.tex.wrap, [Wrap::Mirror, Wrap::Clamp]);
        assert_eq!(t.state.alpha, AlphaMode::Cutout);
        assert!(t.state.double_sided);
    }

    #[test]
    fn refuses_unknown_combiner() {
        let dl = list(
            "gsDPSetCombineLERP(TEXEL0, 0, SHADE, 0, TEXEL0, 0, ENVIRONMENT, 0, PRIMITIVE, 0, COMBINED, 0, 0, 0, 0, COMBINED),
             gsSPEndDisplayList(),",
        );
        let err = interpret(&dl, "v", 0).unwrap_err();
        assert!(err.problem.contains("combiner"), "{err}");
    }

    #[test]
    fn refuses_triangles_without_texture() {
        let dl = list(
            "gsSPVertex(&v[0], 3, 0),
             gsSPNTrianglesInit_5b(1, 0, 1, 2, 0, 0, 0, 0, 0, 0),
             gsSPEndDisplayList(),",
        );
        let err = interpret(&dl, "v", 3).unwrap_err();
        assert!(err.problem.contains("texture"), "{err}");
    }

    #[test]
    fn refuses_commands_inside_a_triangle_group() {
        let dl = list(&format!(
            "{PRELUDE}
             gsSPVertex(&v[0], 3, 0),
             gsSPNTrianglesInit_5b(4, 0, 1, 2, 0, 1, 2, 0, 1, 2),
             gsSPVertex(&v[0], 3, 0),
             gsSPEndDisplayList(),"
        ));
        let err = interpret(&dl, "v", 3).unwrap_err();
        assert!(err.problem.contains("still expected"), "{err}");
    }

    #[test]
    fn slots_count_joints_with_models() {
        let slots = matrix_slots(&[false, true, false, true, true]);
        assert_eq!(slots[&0], 1);
        assert_eq!(slots[&1], 3);
        assert_eq!(slots[&2], 4);
    }
}
