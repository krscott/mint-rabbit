//! Evaluates the shared `cKF` keyframe animations, matching
//! `cKF_SkeletonInfo_R_play` and `cKF_KeyCalc` in the decomp.

use crate::c::CFile;

const ROT_X: u8 = 1 << 2;
const TRANS_X: u8 = 1 << 5;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AnimError {
    #[error("animation {0} not found")]
    Missing(String),
    #[error("animation {name}: {problem}")]
    Malformed { name: String, problem: String },
}

/// Reads a value stored in an `s16` table, accepting the signed or the
/// unsigned spelling (`-175` or `0xFF51`).
fn s16(v: i64) -> Option<i64> {
    let bits = u16::try_from(v)
        .ok()
        .or_else(|| i16::try_from(v).ok().map(|v| v as u16))?;
    Some(i64::from(bits as i16))
}

struct Tables {
    flags: Vec<i64>,
    data: Vec<i64>,
    keys: Vec<i64>,
    fixed: Vec<i64>,
}

fn tables(file: &CFile, name: &str) -> Result<Tables, AnimError> {
    let malformed = |problem: &str| AnimError::Malformed {
        name: name.to_owned(),
        problem: problem.to_owned(),
    };
    let decl = file
        .get(name)
        .ok_or_else(|| AnimError::Missing(name.to_owned()))?;
    let header = decl
        .init
        .as_list()
        .and_then(|l| l.first())
        .and_then(|h| h.as_list())
        .ok_or_else(|| malformed("header is not a brace list"))?;
    let table = |i: usize| -> Result<Vec<i64>, AnimError> {
        let sym = header
            .get(i)
            .and_then(|e| e.as_expr())
            .and_then(|e| e.as_ident())
            .ok_or_else(|| malformed("table reference is not a name"))?;
        file.get(sym)
            .and_then(|d| d.init.as_ints())
            .and_then(|v| {
                // The flag table is u8; the others are s16.
                if i == 0 {
                    v.into_iter()
                        .map(|v| u8::try_from(v).ok().map(i64::from))
                        .collect()
                } else {
                    v.into_iter().map(s16).collect()
                }
            })
            .ok_or_else(|| {
                malformed(&format!(
                    "table {sym} is missing or has values out of range"
                ))
            })
    };
    Ok(Tables {
        flags: table(0)?,
        data: table(1)?,
        keys: table(2)?,
        fixed: table(3)?,
    })
}

/// Samples animation `name` from `file` at `frame` for a skeleton of
/// `joints` joints, returning each joint's (x, y, z) rotation in degrees,
/// applied in Z, Y, X order.
pub fn sample(
    file: &CFile,
    name: &str,
    joints: usize,
    frame: f32,
) -> Result<Vec<[f32; 3]>, AnimError> {
    let t = tables(file, name)?;
    let malformed = |problem: String| AnimError::Malformed {
        name: name.to_owned(),
        problem,
    };
    if t.flags.len() < joints {
        return Err(malformed(format!(
            "{} joint flags for {joints} joints",
            t.flags.len()
        )));
    }
    let mut keys = t.keys.iter().copied();
    let mut fixed = t.fixed.iter().copied();
    let mut data_index = 0usize;
    let mut value = |animated: bool| -> Result<f32, AnimError> {
        if animated {
            let n = keys
                .next()
                .ok_or_else(|| malformed("key table too short".into()))?;
            let n =
                usize::try_from(n).map_err(|_| malformed(format!("key count {n} is negative")))?;
            let v = key_value(&t.data, data_index, n, frame)
                .ok_or_else(|| malformed("data table too short".into()))?;
            data_index = data_index.saturating_add(n);
            Ok(f32::from(v))
        } else {
            fixed
                .next()
                .map(|v| v as f32)
                .ok_or_else(|| malformed("fixed table too short".into()))
        }
    };

    // The root translation comes first; display poses only use rotations.
    for axis in 0..3 {
        value(t.flags[0] as u8 & (TRANS_X >> axis) != 0)?;
    }
    let mut rotations = Vec::with_capacity(joints);
    for &flags in &t.flags[..joints] {
        let mut rot = [0.0; 3];
        for (axis, out) in rot.iter_mut().enumerate() {
            // Stored in tenths of a degree.
            *out = value(flags as u8 & (ROT_X >> axis) != 0)? * 0.1;
        }
        rotations.push(rot);
    }
    Ok(rotations)
}

/// `cKF_KeyCalc`: Hermite interpolation over `(frame, value, tangent)` keys.
fn key_value(data: &[i64], start: usize, n: usize, frame: f32) -> Option<i16> {
    let end = start.checked_add(n)?.checked_mul(3)?;
    let keys: Vec<[f32; 3]> = data
        .get(start.checked_mul(3)?..end)?
        .chunks(3)
        .map(|k| [k[0] as f32, k[1] as f32, k[2] as f32])
        .collect();
    let (first, last) = (keys.first()?, keys.last()?);
    if first[0] >= frame {
        return Some(first[1] as i16);
    }
    if last[0] <= frame {
        return Some(last[1] as i16);
    }
    let next = keys.iter().position(|k| k[0] > frame)?;
    let (a, b) = (keys[next - 1], keys[next]);
    let span = b[0] - a[0];
    if span == 0.0 {
        return Some(a[1] as i16);
    }
    let t = (frame - a[0]) / span;
    let tension = span / 30.0;
    let v = hermite(t, tension, a[1], b[1], a[2], b[2]);
    Some((v + 0.5) as i16)
}

/// `cKF_HermitCalc`.
fn hermite(t: f32, tension: f32, p0: f32, p1: f32, m0: f32, m1: f32) -> f32 {
    let t2 = t * t;
    let t3 = t2 * t;
    let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
    let h01 = -2.0 * t3 + 3.0 * t2;
    let h10 = t3 - 2.0 * t2 + t;
    let h11 = t3 - t2;
    h00 * p0 + h01 * p1 + (h10 * m0 + h11 * m1) * tension
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn samples_fixed_and_keyed_values() {
        // Two joints. Root: Y translation keyed; joint 0: Z rotation keyed;
        // joint 1: everything fixed.
        let file = CFile::parse_str(
            Path::new("a.c"),
            "u8 flags[] = { 16 | 1, 0 };
             s16 keys[] = { 2, 1 };
             s16 fixed[] = { 5, 7, 900, 0, 0, 0, 1800 };
             s16 data[] = { 1, 1000, 0, 9, 1050, 0, 1, 450, 0 };
             aNPC_Animation_c anim = { { flags, data, keys, fixed, -1, 33 }, 1.0f };",
        )
        .unwrap();
        let f = sample(&file, "anim", 2, 1.0).unwrap();
        let close = |a: &[f32], b: &[f32]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4);
        assert!(
            close(&f.concat(), &[90.0, 0.0, 45.0, 0.0, 0.0, 180.0]),
            "{f:?}"
        );
    }

    #[test]
    fn reads_unsigned_spellings_as_s16() {
        let file = CFile::parse_str(
            Path::new("a.c"),
            "u8 flags[] = { 0 };
             s16 keys[] = { 0 };
             s16 data[] = { 0 };
             s16 fixed[] = { 0, 0, 0, 0xFF51, 0, 0 };
             aNPC_Animation_c anim = { { flags, data, keys, fixed, -1, 1 } };",
        )
        .unwrap();
        let rotations = sample(&file, "anim", 1, 1.0).unwrap();
        assert!((rotations[0][0] + 17.5).abs() < 1e-4, "{rotations:?}");
    }

    #[test]
    fn hermite_hits_endpoints() {
        assert!((hermite(0.0, 1.0, 3.0, 9.0, 5.0, -2.0) - 3.0).abs() < 1e-6);
        assert!((hermite(1.0, 1.0, 3.0, 9.0, 5.0, -2.0) - 9.0).abs() < 1e-6);
    }
}
