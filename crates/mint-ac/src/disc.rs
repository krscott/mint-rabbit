//! Reads villagers' in-game names from the disc image a decomp was set up
//! from. The decomp's sources don't hold them, and asset extraction leaves
//! them on the disc.

use std::fs::File;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};

use crate::import::VILLAGER_COUNT;

/// The archive at the disc root that holds the name table.
const NAME_ARCHIVE: &str = "forest_2nd.arc";
/// One fixed-width record per villager, in villager id order.
const NAME_TABLE: &str = "npc_name_str_table.bin";
/// Bytes per name record, padded with spaces.
const NAME_LEN: usize = 8;
/// The most a read from the disc may take; the files names need are a few
/// MiB.
const MAX_READ: u32 = 64 << 20;
/// Other disc image formats the decomp builds from.
const COMPRESSED_DISCS: [&str; 5] = ["rvz", "wia", "wbfs", "ciso", "gcz"];

#[derive(Debug, thiserror::Error)]
pub enum NamesError {
    #[error("no disc image (.iso or .gcm) in {0}")]
    NoDisc(PathBuf),
    #[error(
        "{0} is compressed; names are read from an uncompressed disc image, so convert it to .iso with Dolphin"
    )]
    Compressed(PathBuf),
    #[error("{0} holds several disc images, so it's unclear which to read names from: {1:?}")]
    ManyDiscs(PathBuf, Vec<PathBuf>),
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("{path}: {problem}")]
    Malformed { path: PathBuf, problem: String },
}

/// The names of the villagers, in villager table order, read from the one
/// disc image (`.iso` or `.gcm`) in `dir`.
pub(crate) fn villager_names(dir: &Path) -> Result<Vec<String>, NamesError> {
    let path = find_disc(dir)?;
    let mut disc = Disc::open(&path)?;
    let arc = disc.root_file(NAME_ARCHIVE)?;
    let malformed = |problem: String| NamesError::Malformed {
        path: path.clone(),
        problem: format!("{NAME_ARCHIVE}: {problem}"),
    };
    let table = rarc_file(&arc, NAME_TABLE).map_err(malformed)?;
    let in_table = |p: String| NamesError::Malformed {
        path: path.clone(),
        problem: format!("{NAME_ARCHIVE}/{NAME_TABLE}: {p}"),
    };
    decode_names(table, VILLAGER_COUNT).map_err(in_table)
}

fn find_disc(dir: &Path) -> Result<PathBuf, NamesError> {
    let entries = std::fs::read_dir(dir).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => NamesError::NoDisc(dir.to_owned()),
        _ => NamesError::Io(dir.to_owned(), e),
    })?;
    let mut discs = Vec::new();
    let mut compressed = None;
    for entry in entries {
        let path = entry.map_err(|e| NamesError::Io(dir.to_owned(), e))?.path();
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        let ext = ext.to_ascii_lowercase();
        if !path.is_file() {
            continue;
        }
        if ext == "iso" || ext == "gcm" {
            discs.push(path);
        } else if COMPRESSED_DISCS.contains(&ext.as_str()) {
            compressed = Some(path);
        }
    }
    discs.sort();
    match discs.len() {
        0 => Err(compressed.map_or_else(
            || NamesError::NoDisc(dir.to_owned()),
            NamesError::Compressed,
        )),
        1 => Ok(discs.remove(0)),
        _ => Err(NamesError::ManyDiscs(dir.to_owned(), discs)),
    }
}

fn be32(bytes: &[u8], at: usize) -> Option<u32> {
    let b = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn be16(bytes: &[u8], at: usize) -> Option<u16> {
    let b = bytes.get(at..at.checked_add(2)?)?;
    Some(u16::from_be_bytes([b[0], b[1]]))
}

/// Whether the NUL-terminated name at `at` is `name`, reading at most
/// `name.len() + 1` bytes.
fn name_at(bytes: &[u8], at: usize, name: &str) -> bool {
    let end = at.saturating_add(name.len());
    bytes.get(at..end) == Some(name.as_bytes()) && bytes.get(end) == Some(&0)
}

/// An uncompressed `GameCube` disc image (GCM).
struct Disc {
    path: PathBuf,
    file: File,
    len: u64,
}

impl Disc {
    fn open(path: &Path) -> Result<Self, NamesError> {
        let io = |e| NamesError::Io(path.to_owned(), e);
        let file = File::open(path).map_err(io)?;
        let len = file.metadata().map_err(io)?.len();
        Ok(Self {
            path: path.to_owned(),
            file,
            len,
        })
    }

    fn malformed(&self, problem: impl Into<String>) -> NamesError {
        NamesError::Malformed {
            path: self.path.clone(),
            problem: problem.into(),
        }
    }

    fn read(&mut self, offset: u32, len: u32) -> Result<Vec<u8>, NamesError> {
        // Checked first, so a bad size never allocates.
        if len > MAX_READ {
            return Err(self.malformed(format!(
                "asks for {len} bytes at {offset:#x}, more than the {MAX_READ} allowed"
            )));
        }
        if u64::from(offset) + u64::from(len) > self.len {
            return Err(self.malformed(format!(
                "is {} bytes, too short for {len} bytes at {offset:#x}; is this an uncompressed GameCube disc image?",
                self.len
            )));
        }
        let mut buf = vec![0; len as usize];
        self.file
            .seek(SeekFrom::Start(u64::from(offset)))
            .and_then(|_| self.file.read_exact(&mut buf))
            .map_err(|e| NamesError::Io(self.path.clone(), e))?;
        Ok(buf)
    }

    /// The contents of the file `name` in the disc's root directory.
    fn root_file(&mut self, name: &str) -> Result<Vec<u8>, NamesError> {
        // The disc header gives the file system table's offset and size.
        let header = self.read(0x424, 8)?;
        let (fst_offset, fst_size) = (be32(&header, 0).unwrap(), be32(&header, 4).unwrap());
        let fst = self.read(fst_offset, fst_size)?;
        // Entry 0 is the root directory; its size field counts every entry.
        let count = be32(&fst, 8).ok_or_else(|| self.malformed("file system table is empty"))?;
        let names = count as usize * 12;
        let mut i = 1;
        while i < count as usize {
            let entry = |field: usize| be32(&fst, i * 12 + field);
            let (Some(kind_name), Some(offset), Some(size)) = (entry(0), entry(4), entry(8)) else {
                return Err(self.malformed(format!("file system table entry {i} is cut off")));
            };
            let is_dir = kind_name >> 24 != 0;
            if is_dir {
                // A directory's size field is the index after its last entry.
                let next = size as usize;
                if next <= i {
                    return Err(self.malformed(format!("directory entry {i} ends before itself")));
                }
                i = next;
                continue;
            }
            if name_at(&fst, names + (kind_name & 0x00ff_ffff) as usize, name) {
                return self.read(offset, size);
            }
            i += 1;
        }
        Err(self.malformed(format!(
            "{name} is not in the disc's root directory; is this an Animal Crossing disc?"
        )))
    }
}

/// The contents of the file `name` in a RARC archive.
fn rarc_file<'a>(arc: &'a [u8], name: &str) -> Result<&'a [u8], String> {
    if arc.starts_with(b"Yaz0") {
        return Err("the archive is compressed, which is not supported".into());
    }
    if !arc.starts_with(b"RARC") {
        return Err("not a RARC archive".into());
    }
    // Offsets in the info block are relative to its start, just after the
    // 0x20-byte header.
    let base = 0x20;
    let cut = || "the archive is cut off".to_owned();
    let data = base + be32(arc, 0x0c).ok_or_else(cut)? as usize;
    let count = be32(arc, base + 0x08).ok_or_else(cut)? as usize;
    let entries = base + be32(arc, base + 0x0c).ok_or_else(cut)? as usize;
    let strings = base + be32(arc, base + 0x14).ok_or_else(cut)? as usize;
    for i in 0..count {
        let at = entries + i * 0x14;
        let (Some(flags_name), Some(offset), Some(size)) =
            (be32(arc, at + 4), be32(arc, at + 8), be32(arc, at + 12))
        else {
            return Err(cut());
        };
        let flags = flags_name >> 24;
        let is_file = flags & 0x01 != 0;
        let name_offset = be16(arc, at + 6).ok_or_else(cut)?;
        if !is_file || !name_at(arc, strings + usize::from(name_offset), name) {
            continue;
        }
        if flags & 0x04 != 0 {
            return Err(format!("{name} is compressed, which is not supported"));
        }
        let start = data + offset as usize;
        return arc
            .get(start..start + size as usize)
            .ok_or_else(|| format!("{name} runs past the end of the archive"));
    }
    Err(format!("{name} is missing"))
}

/// Whether the game's font draws `b` as the same character as ASCII. The
/// font reuses the other codes below 0x80 for accented letters and symbols.
fn ascii_compatible(b: u8) -> bool {
    matches!(b,
        b' ' | b'!' | b'"' | b'%'..=b')' | b','..=b'.' | b'0'..=b':' | b'<'..=b'Z' | b'_' | b'a'..=b'z'
    )
}

/// Splits a name table of `count` names, trimming their space padding,
/// and checking its size before decoding anything.
fn decode_names(table: &[u8], count: usize) -> Result<Vec<String>, String> {
    if table.len() != count * NAME_LEN {
        return Err(format!(
            "it is {} bytes, but {count} names take {}",
            table.len(),
            count * NAME_LEN
        ));
    }
    table
        .chunks(NAME_LEN)
        .enumerate()
        .map(|(i, record)| {
            if let Some(&b) = record.iter().find(|&&b| !ascii_compatible(b)) {
                return Err(format!(
                    "name {i} has character code {b:#04x}, which is not plain ASCII in the game's font"
                ));
            }
            let name: String = record.iter().map(|&b| char::from(b)).collect();
            Ok(name.trim_end().to_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_trim_padding() {
        assert_eq!(
            decode_names(b"Bob     O'Hare  T-Bone  ", 3).unwrap(),
            ["Bob", "O'Hare", "T-Bone"]
        );
    }

    #[test]
    fn names_refuse_font_only_codes() {
        // 0x23 is an accented letter in the game's font, not '#'.
        let e = decode_names(b"Bo#     ", 1).unwrap_err();
        assert!(e.contains("0x23"), "{e}");
        assert!(decode_names(b"Bob", 1).is_err());
        assert!(decode_names(b"Bob     ", 2).is_err());
    }

    /// A RARC archive holding the files `files`.
    fn rarc(files: &[(&str, &[u8], u8)]) -> Vec<u8> {
        let mut strings = Vec::new();
        let mut data = Vec::new();
        let mut entries = Vec::new();
        for (name, bytes, flags) in files {
            let name_at = strings.len() as u16;
            strings.extend_from_slice(name.as_bytes());
            strings.push(0);
            entries.extend_from_slice(&[0, 0, 0, 0]);
            entries.push(*flags);
            entries.push(0);
            entries.extend_from_slice(&name_at.to_be_bytes());
            entries.extend_from_slice(&(data.len() as u32).to_be_bytes());
            entries.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
            entries.extend_from_slice(&[0; 4]);
            data.extend_from_slice(bytes);
        }
        let info_len = 0x20;
        let entries_at = info_len;
        let strings_at = entries_at + entries.len();
        let data_at = strings_at + strings.len();
        let mut arc = b"RARC".to_vec();
        arc.extend_from_slice(&[0; 8]);
        arc.extend_from_slice(&(data_at as u32).to_be_bytes());
        arc.resize(0x20, 0);
        let mut info = vec![0; info_len];
        info[8..12].copy_from_slice(&(files.len() as u32).to_be_bytes());
        info[12..16].copy_from_slice(&(entries_at as u32).to_be_bytes());
        info[20..24].copy_from_slice(&(strings_at as u32).to_be_bytes());
        arc.extend(info);
        arc.extend(entries);
        arc.extend(strings);
        arc.extend(data);
        arc
    }

    #[test]
    fn rarc_finds_files_by_name() {
        let arc = rarc(&[("a.bin", b"first", 0x01), (NAME_TABLE, b"Bob     ", 0x21)]);
        assert_eq!(rarc_file(&arc, NAME_TABLE).unwrap(), b"Bob     ");
        assert!(rarc_file(&arc, "c.bin").unwrap_err().contains("missing"));
        let packed = rarc(&[(NAME_TABLE, b"x", 0x05)]);
        assert!(
            rarc_file(&packed, NAME_TABLE)
                .unwrap_err()
                .contains("compressed")
        );
    }

    #[test]
    fn compressed_discs_are_named() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(find_disc(dir.path()), Err(NamesError::NoDisc(_))));
        std::fs::write(dir.path().join("game.rvz"), b"").unwrap();
        assert!(matches!(
            find_disc(dir.path()),
            Err(NamesError::Compressed(_))
        ));
        std::fs::write(dir.path().join("game.ISO"), b"").unwrap();
        assert!(find_disc(dir.path()).unwrap().ends_with("game.ISO"));
    }

    /// Reads the names from the disc image in the decomp named by
    /// `MINT_AC_DECOMP`.
    #[test]
    #[ignore = "needs an ac-decomp checkout with a US disc image in MINT_AC_DECOMP"]
    fn disc_names_read() {
        let root = PathBuf::from(std::env::var("MINT_AC_DECOMP").expect("MINT_AC_DECOMP"));
        let names = match villager_names(&root.join("orig/GAFE01_00")) {
            Err(e @ NamesError::Compressed(_)) => {
                // Straight to stderr: the test harness hides a passing
                // test's eprintln! output.
                use std::io::Write as _;
                writeln!(std::io::stderr(), "disc_names_read skipped: {e}").unwrap();
                return;
            }
            names => names.unwrap(),
        };
        assert_eq!(names.len(), VILLAGER_COUNT);
        assert_eq!(names[..3], ["Bob", "Olivia", "Mitzi"]);
        assert!(names.iter().any(|n| n == "O'Hare"), "{names:?}");
    }
}
