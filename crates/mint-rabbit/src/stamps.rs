//! The Stamp Library, one file per stamp in a folder that every project
//! shares, and the stamp the Stamp tool places.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::{self, Color32, ColorImage, TextureHandle, TextureOptions};
use mint_core::file::{self, STAMP_EXTENSION};
use mint_core::{Palette, PaletteError, PaletteIndex, Project, Rgba8, Stamp};

/// Longest stamp name, in bytes, leaving room in a file name for the
/// extension.
const MAX_NAME: usize = 200;
/// Most pixels a side of a stamp's thumbnail image has.
const THUMBNAIL_SIDE: u32 = 64;

/// The library's folder: `mint-rabbit/stamps` in the XDG data directory,
/// `$XDG_DATA_HOME` or else `~/.local/share`.
pub fn default_dir() -> Result<PathBuf, String> {
    dir_from(std::env::var_os("XDG_DATA_HOME"), std::env::var_os("HOME"))
}

fn dir_from(xdg: Option<OsString>, home: Option<OsString>) -> Result<PathBuf, String> {
    // The XDG spec ignores a relative path.
    let data = xdg
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            home.map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .map(|h| h.join(".local/share"))
        })
        .ok_or("Neither XDG_DATA_HOME nor HOME is set, so there is no stamp folder")?;
    Ok(data.join("mint-rabbit/stamps"))
}

/// `name` without spaces at its ends, if it can name a stamp file.
pub fn check_name(name: &str) -> Result<&str, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Give the stamp a name".into());
    }
    if name.starts_with('.') {
        return Err("A stamp's name can't start with a dot".into());
    }
    if name.contains(['/', '\\']) || name.chars().any(char::is_control) {
        return Err("A stamp's name can't hold a slash or a control character".into());
    }
    if name.len() > MAX_NAME {
        return Err(format!("A stamp's name is at most {MAX_NAME} bytes"));
    }
    Ok(name)
}

/// A stamp file in the library.
pub struct Entry {
    /// The file's name without its extension.
    pub name: String,
    path: PathBuf,
    /// The stamp, or why its file couldn't be read.
    pub stamp: Result<Arc<Stamp>, String>,
    thumbnail: Option<TextureHandle>,
}

impl Entry {
    /// The image of `stamp`, this entry's, in its own colors, drawn the
    /// first time it is asked for.
    pub fn thumbnail(&mut self, ctx: &egui::Context, stamp: &Stamp) -> TextureHandle {
        let handle = self.thumbnail.get_or_insert_with(|| {
            let raster = stamp.raster(THUMBNAIL_SIDE);
            let size = raster.size();
            let pixels = raster
                .pixels()
                .iter()
                .map(|p| match p {
                    Some(i) => {
                        let c = stamp.colors()[usize::from(i.0)];
                        Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
                    }
                    None => Color32::TRANSPARENT,
                })
                .collect();
            let image = ColorImage::new([size.x as usize, size.y as usize], pixels);
            ctx.load_texture("stamp", image, TextureOptions::NEAREST)
        });
        handle.clone()
    }
}

/// The stamps in the library folder, as last read.
pub struct Library {
    /// The folder, or why there is none.
    dir: Result<PathBuf, String>,
    /// By name, ignoring case.
    entries: Vec<Entry>,
    /// Why the folder couldn't be read, when it exists.
    problem: Option<String>,
}

impl Library {
    pub fn new(dir: Result<PathBuf, String>) -> Self {
        let mut library = Self {
            dir,
            entries: Vec::new(),
            problem: None,
        };
        library.scan();
        library
    }

    pub fn dir(&self) -> Result<&Path, String> {
        self.dir.as_deref().map_err(Clone::clone)
    }

    /// Why the stamps can't be listed, if they can't.
    pub fn problem(&self) -> Option<String> {
        self.dir().err().or_else(|| self.problem.clone())
    }

    pub fn entries_mut(&mut self) -> &mut [Entry] {
        &mut self.entries
    }

    /// Reads the folder again. A folder that doesn't exist yet holds no
    /// stamps.
    pub fn scan(&mut self) {
        self.entries.clear();
        self.problem = None;
        let Ok(dir) = &self.dir else { return };
        let read = match std::fs::read_dir(dir) {
            Ok(read) => read,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => {
                self.problem = Some(format!("{}: {e}", dir.display()));
                return;
            }
        };
        for item in read.flatten() {
            let path = item.path();
            if path.extension().is_none_or(|e| e != STAMP_EXTENSION) || !path.is_file() {
                continue;
            }
            // A name that isn't UTF-8 can't be shown or typed.
            let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            self.entries.push(Entry {
                name: name.to_owned(),
                stamp: file::load_stamp(&path)
                    .map(Arc::new)
                    .map_err(|e| e.to_string()),
                path,
                thumbnail: None,
            });
        }
        self.entries
            .sort_by_cached_key(|e| (e.name.to_lowercase(), e.name.clone()));
    }

    /// Whether a stamp has `name`.
    pub fn taken(&self, name: &str) -> bool {
        self.entries.iter().any(|e| e.name == name)
    }

    /// The file a new stamp named `name` goes in.
    fn path(&self, name: &str) -> Result<PathBuf, String> {
        let name = check_name(name)?;
        Ok(self.dir()?.join(format!("{name}.{STAMP_EXTENSION}")))
    }

    /// The file of the listed stamp named exactly `name`.
    fn existing(&self, name: &str) -> Result<PathBuf, String> {
        self.entries
            .iter()
            .find(|e| e.name == name)
            .map(|e| e.path.clone())
            .ok_or_else(|| format!("There's no stamp named {name:?}"))
    }

    /// Saves `stamp` as a new stamp named `name`, creating the folder if
    /// needed.
    pub fn save(&mut self, name: &str, stamp: &Stamp) -> Result<(), String> {
        let path = self.path(name)?;
        let result = self
            .create_dir()
            .and_then(|()| file::save_stamp(stamp, &path).map_err(|e| file_error(e, name)));
        self.scan();
        result
    }

    /// Renames stamp `from` to `to`, unless another file has that name.
    pub fn rename(&mut self, from: &str, to: &str) -> Result<(), String> {
        let (old, new) = (self.existing(from)?, self.path(to)?);
        let result = file::rename_stamp(&old, &new).map_err(|e| file_error(e, to));
        self.scan();
        result
    }

    /// Removes stamp `name`'s file.
    pub fn delete(&mut self, name: &str) -> Result<(), String> {
        let path = self.existing(name)?;
        let result = std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()));
        self.scan();
        result
    }

    /// Creates the folder if it doesn't exist.
    pub fn create_dir(&self) -> Result<(), String> {
        let dir = self.dir()?;
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))
    }
}

/// What went wrong writing the stamp named `name`.
fn file_error(e: file::StampFileError, name: &str) -> String {
    match e {
        file::StampFileError::Exists(_) => {
            format!("A stamp named {:?} already exists", name.trim())
        }
        e => e.to_string(),
    }
}

/// What a stamp color becomes on placing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// The palette entry at this index, as long as it still has this color.
    Entry(PaletteIndex, Rgba8),
    /// A new palette entry of the stamp color, shared by the stamp's colors
    /// of the same value.
    New,
}

impl Choice {
    fn entry(palette: &Palette, i: PaletteIndex) -> Self {
        Self::Entry(i, palette.colors()[usize::from(i.0)])
    }
}

/// For each of `stamp`'s colors, the nearest palette entry, or a new one if
/// the palette is empty.
pub fn nearest_choices(stamp: &Stamp, palette: &Palette) -> Vec<Choice> {
    stamp
        .colors()
        .iter()
        .map(|&c| {
            palette
                .nearest(c)
                .map_or(Choice::New, |i| Choice::entry(palette, i))
        })
        .collect()
}

/// How many entries `choices` add to the palette for `stamp`.
pub fn new_entries(stamp: &Stamp, choices: &[Choice]) -> usize {
    let mut colors: Vec<Rgba8> = stamp
        .colors()
        .iter()
        .zip(choices)
        .filter(|(_, c)| **c == Choice::New)
        .map(|(&c, _)| c)
        .collect();
    colors.sort_unstable_by_key(|c| c.to_array());
    colors.dedup();
    colors.len()
}

/// A stamp the Stamp tool places, and what each of its colors becomes.
pub struct Placing {
    pub name: String,
    pub stamp: Arc<Stamp>,
    /// A choice per stamp color, or `None` until they are chosen.
    pub choices: Option<Vec<Choice>>,
}

impl Placing {
    /// `stamp` named `name`, its colors chosen already if each matches a
    /// palette entry exactly: the first of that color.
    pub fn new(name: String, stamp: Arc<Stamp>, palette: &Palette) -> Self {
        let choices = stamp
            .colors()
            .iter()
            .map(|&c| Some(Choice::entry(palette, palette.find(c)?)))
            .collect();
        Self {
            name,
            stamp,
            choices,
        }
    }

    /// The choices, if each entry chosen still has the color it was chosen
    /// with; otherwise they are dropped, to be made again.
    pub fn current(&mut self, palette: &Palette) -> Option<Vec<Choice>> {
        let holds = |c: &Choice| match *c {
            Choice::Entry(i, color) => palette.colors().get(usize::from(i.0)) == Some(&color),
            Choice::New => true,
        };
        if !self.choices.as_ref().is_some_and(|c| c.iter().all(holds)) {
            self.choices = None;
        }
        self.choices.clone()
    }

    /// Chooses `entries` of `palette`, one per stamp color, from now on.
    pub fn choose(&mut self, palette: &Palette, entries: &[PaletteIndex]) {
        self.choices = Some(entries.iter().map(|&i| Choice::entry(palette, i)).collect());
    }

    /// Adds a palette entry to `project` for each distinct color `choices`
    /// make new, and returns the entry each stamp color becomes.
    pub fn resolve(
        &self,
        project: &mut Project,
        choices: &[Choice],
    ) -> Result<Vec<PaletteIndex>, PaletteError> {
        let mut added: Vec<(Rgba8, PaletteIndex)> = Vec::new();
        let mut entries = Vec::new();
        for (&color, &choice) in self.stamp.colors().iter().zip(choices) {
            let entry = match choice {
                Choice::Entry(i, _) => i,
                Choice::New => {
                    if let Some(&(_, i)) = added.iter().find(|(c, _)| *c == color) {
                        i
                    } else {
                        let i = project.add_palette_color(color)?;
                        added.push((color, i));
                        i
                    }
                }
            };
            entries.push(entry);
        }
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec2;
    use mint_core::{Anchor, Shape, Style};

    use super::*;

    fn stamp(colors: &[Rgba8]) -> Stamp {
        let shapes = (0..colors.len())
            .map(|i| {
                let p = Vec2::new(i as f32, 0.0);
                Shape::new(vec![Anchor::corner(p)], Style::Fill, PaletteIndex(i as u8)).unwrap()
            })
            .collect();
        Stamp::new(colors.to_vec(), shapes).unwrap()
    }

    const RED: Rgba8 = Rgba8::new(255, 0, 0, 255);
    const GREEN: Rgba8 = Rgba8::new(0, 255, 0, 255);
    const BLUE: Rgba8 = Rgba8::new(0, 0, 255, 255);

    #[test]
    fn the_folder_follows_xdg() {
        let dir = |xdg: Option<&str>, home: Option<&str>| {
            dir_from(xdg.map(Into::into), home.map(Into::into))
        };
        assert_eq!(
            dir(Some("/data"), Some("/home/u")),
            Ok(PathBuf::from("/data/mint-rabbit/stamps"))
        );
        assert_eq!(
            dir(Some("relative"), Some("/home/u")),
            Ok(PathBuf::from("/home/u/.local/share/mint-rabbit/stamps"))
        );
        assert!(dir(None, None).is_err());
    }

    #[test]
    fn names_must_make_plain_file_names() {
        assert_eq!(check_name("  Eye left "), Ok("Eye left"));
        for bad in ["", "  ", ".hidden", "a/b", "a\\b", "a\nb", &"x".repeat(201)] {
            assert!(check_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_library_saves_renames_and_deletes_stamp_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("stamps");
        let mut library = Library::new(Ok(root.clone()));
        assert!(library.entries_mut().is_empty());
        let eye = stamp(&[RED]);
        library.save("eye", &eye).unwrap();
        library.save("Brow", &stamp(&[GREEN])).unwrap();
        assert!(library.save("eye", &eye).is_err());
        let names: Vec<_> = library
            .entries_mut()
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(names, ["Brow", "eye"]);
        assert_eq!(library.entries_mut()[1].stamp.as_deref(), Ok(&eye));

        // Renaming never replaces another stamp.
        assert!(library.rename("eye", "Brow").is_err());
        library.rename("eye", "Eye left").unwrap();
        assert!(root.join("Eye left.ron").is_file() && !root.join("eye.ron").exists());
        library.delete("Brow").unwrap();
        let names: Vec<_> = library
            .entries_mut()
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(names, ["Eye left"]);

        // Names are taken as they are: deleting " Eye left" leaves
        // "Eye left".
        std::fs::copy(root.join("Eye left.ron"), root.join(" Eye left.ron")).unwrap();
        library.scan();
        library.delete(" Eye left").unwrap();
        assert!(root.join("Eye left.ron").is_file() && !root.join(" Eye left.ron").exists());

        // Other files are ignored, and broken stamps listed with why.
        std::fs::write(root.join("notes.txt"), "").unwrap();
        std::fs::write(root.join("broken.ron"), "(").unwrap();
        library.scan();
        assert_eq!(library.entries_mut().len(), 2);
        assert!(library.entries_mut()[0].stamp.is_err());
    }

    #[test]
    fn choices_hold_while_their_entries_keep_their_colors() {
        let palette = Palette::from_colors([RED, BLUE]).unwrap();
        let s = Arc::new(stamp(&[BLUE, GREEN]));
        let mut placing = Placing::new("s".into(), s.clone(), &palette);
        // Green has no exact match, so the choices wait for the user.
        assert_eq!(placing.current(&palette), None);
        let nearest = nearest_choices(&s, &palette);
        assert_eq!(
            nearest,
            [
                Choice::Entry(PaletteIndex(1), BLUE),
                Choice::Entry(PaletteIndex(0), RED)
            ]
        );
        placing.choices = Some(nearest.clone());
        assert_eq!(placing.current(&palette), Some(nearest));
        // Moved entries drop the choices, even where other entries match
        // exactly, since the user may have chosen otherwise.
        let swapped = Palette::from_colors([GREEN, BLUE, RED]).unwrap();
        assert_eq!(placing.current(&swapped), None);
        assert_eq!(placing.current(&palette), None);
        // A new stamp takes exact matches without asking.
        let exact = Placing::new("s".into(), s, &swapped);
        assert_eq!(
            exact.choices,
            Some(vec![
                Choice::Entry(PaletteIndex(1), BLUE),
                Choice::Entry(PaletteIndex(0), GREEN)
            ])
        );
    }

    #[test]
    fn new_entries_are_added_once_per_color() {
        let s = stamp(&[RED, GREEN, GREEN]);
        let choices = [Choice::New, Choice::New, Choice::New];
        assert_eq!(new_entries(&s, &choices), 2);
        let mut project = Project::new(
            mint_core::ExportTarget::new("test"),
            Palette::from_colors([BLUE]).unwrap(),
            mint_core::Model::new("root", mint_core::Transform::default()),
            Vec::new(),
            Vec::new(),
            mint_core::Pose::default(),
        )
        .unwrap();
        let mut placing = Placing {
            name: "s".into(),
            stamp: Arc::new(s),
            choices: None,
        };
        let entries = placing.resolve(&mut project, &choices).unwrap();
        assert_eq!(entries, [PaletteIndex(1), PaletteIndex(2), PaletteIndex(2)]);
        assert_eq!(project.palette().colors(), [BLUE, RED, GREEN]);
        // Chosen, placing reuses them from then on.
        placing.choose(project.palette(), &entries);
        assert_eq!(
            placing.current(project.palette()),
            Some(vec![
                Choice::Entry(PaletteIndex(1), RED),
                Choice::Entry(PaletteIndex(2), GREEN),
                Choice::Entry(PaletteIndex(2), GREEN)
            ])
        );
    }
}
