//! The open project with its undo history.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;

use mint_core::{EditError, Project, Variant, VariantRef};

/// A control's key as stored in a [`Gesture`].
fn hash(key: impl Hash) -> u64 {
    let mut h = DefaultHasher::new();
    key.hash(&mut h);
    h.finish()
}

/// How many edits undo can go back.
const UNDO_LIMIT: usize = 200;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Gesture {
    None,
    /// Edits until the gesture ends undo as one step; `recorded` once that
    /// step is on the undo stack. `key` names the control making them.
    /// `dirty` is whether rolling the gesture back leaves changes unsaved:
    /// there were some before it, or it was saved partway.
    Open {
        key: u64,
        recorded: bool,
        dirty: bool,
    },
}

/// A project being edited: every change goes through [`Editor::edit`], which
/// records it for undo and marks the project changed.
pub struct Editor {
    project: Project,
    /// The project directory it was last opened from or saved to.
    path: Option<PathBuf>,
    undo: Vec<Project>,
    redo: Vec<Project>,
    /// What could be redone before the open continuous edit cleared it,
    /// restored if that edit is reverted.
    gesture_redo: Vec<Project>,
    gesture: Gesture,
    /// Whether there are changes since the last open or save.
    dirty: bool,
    /// Bumped on every change, so views know to redraw from the project.
    generation: u64,
}

impl Editor {
    pub fn new(project: Project, path: Option<PathBuf>) -> Self {
        Self {
            project,
            path,
            undo: Vec::new(),
            redo: Vec::new(),
            gesture_redo: Vec::new(),
            gesture: Gesture::None,
            dirty: false,
            generation: 0,
        }
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    pub fn path(&self) -> Option<&PathBuf> {
        self.path.as_ref()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn dirty(&self) -> bool {
        self.dirty
    }

    /// Records that the project was saved, as it is now, to `path`.
    pub fn saved(&mut self, path: PathBuf) {
        self.path = Some(path);
        self.dirty = false;
        if let Gesture::Open {
            recorded: true,
            dirty,
            ..
        } = &mut self.gesture
        {
            *dirty = true;
        }
    }

    /// Applies `f` to the project as an undoable step of its own. If it
    /// fails, the project is put back as it was; if it changes nothing, no
    /// step is recorded.
    pub fn edit<T, E>(&mut self, f: impl FnOnce(&mut Project) -> Result<T, E>) -> Result<T, E> {
        self.end_gesture();
        self.apply(f)
    }

    /// Like [`Self::edit`], for one of many edits a control makes while
    /// held, such as a drag or a slider, named by `key`: edits by the same
    /// control until [`Self::end_gesture`] undo as one step.
    pub fn edit_continuous<T, E>(
        &mut self,
        key: impl Hash,
        f: impl FnOnce(&mut Project) -> Result<T, E>,
    ) -> Result<T, E> {
        let key = hash(key);
        if !matches!(self.gesture, Gesture::Open { key: open, .. } if open == key) {
            self.gesture = Gesture::Open {
                key,
                recorded: false,
                dirty: self.dirty,
            };
        }
        self.apply(f)
    }

    /// [`Self::edit`] of one variant.
    pub fn edit_variant<T>(
        &mut self,
        r: VariantRef,
        f: impl FnOnce(&mut Variant) -> T,
    ) -> Result<T, EditError> {
        self.edit(|p| p.edit_variant(r, f))
    }

    /// [`Self::edit_continuous`] of one variant.
    pub fn edit_variant_continuous<T>(
        &mut self,
        key: impl Hash,
        r: VariantRef,
        f: impl FnOnce(&mut Variant) -> T,
    ) -> Result<T, EditError> {
        self.edit_continuous(key, |p| p.edit_variant(r, f))
    }

    fn apply<T, E>(&mut self, f: impl FnOnce(&mut Project) -> Result<T, E>) -> Result<T, E> {
        let before = self.project.clone();
        let out = match f(&mut self.project) {
            Ok(out) => out,
            Err(e) => {
                self.project = before;
                return Err(e);
            }
        };
        if self.project != before {
            match &mut self.gesture {
                Gesture::Open { recorded: true, .. } => {}
                Gesture::Open {
                    recorded, dirty, ..
                } => {
                    *recorded = true;
                    *dirty = self.dirty;
                    let redo = std::mem::take(&mut self.redo);
                    self.push_undo(before);
                    self.gesture_redo = redo;
                }
                Gesture::None => self.push_undo(before),
            }
            self.dirty = true;
            self.generation += 1;
        }
        Ok(out)
    }

    /// Rolls back the open continuous edit, if the control named `key`
    /// opened it, dropping its undo step and restoring the redo history it
    /// cleared; then ends whatever edit is open.
    pub fn revert_gesture(&mut self, key: impl Hash) {
        let key = hash(key);
        if let Gesture::Open {
            key: open,
            recorded: true,
            dirty,
        } = self.gesture
            && open == key
            && let Some(before) = self.undo.pop()
        {
            self.project = before;
            self.redo = std::mem::take(&mut self.gesture_redo);
            self.dirty = dirty;
            self.generation += 1;
        }
        self.end_gesture();
    }

    /// Ends a continuous edit, so the next edit is a step of its own.
    pub fn end_gesture(&mut self) {
        self.gesture = Gesture::None;
        self.gesture_redo.clear();
        self.trim_undo();
    }

    /// Records the project as it was before an edit. While a continuous
    /// edit is open, its step may still be rolled back, so the oldest step
    /// stays until it ends.
    fn push_undo(&mut self, before: Project) {
        self.undo.push(before);
        self.redo.clear();
        if self.gesture == Gesture::None {
            self.trim_undo();
        }
    }

    fn trim_undo(&mut self) {
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) {
        self.end_gesture();
        if let Some(p) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.project, p));
            self.dirty = true;
            self.generation += 1;
        }
    }

    pub fn redo(&mut self) {
        self.end_gesture();
        if let Some(p) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.project, p));
            self.dirty = true;
            self.generation += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use mint_core::{ExportTarget, Model, Palette, PaletteIndex, Pose, Rgba8, Transform};

    use super::*;

    fn editor() -> Editor {
        let project = Project::new(
            ExportTarget::new("test"),
            Palette::from_colors([Rgba8::new(0, 0, 0, 255)]).unwrap(),
            Model::new("root", Transform::default()),
            Vec::new(),
            Vec::new(),
            Pose::default(),
        )
        .unwrap();
        Editor::new(project, None)
    }

    fn red(e: &mut Editor, r: u8) {
        e.edit(|p| p.set_palette_color(PaletteIndex(0), Rgba8::new(r, 0, 0, 255)))
            .unwrap();
    }

    fn current(e: &Editor) -> u8 {
        e.project().palette().colors()[0].r
    }

    #[test]
    fn edits_undo_and_redo_one_at_a_time() {
        let mut e = editor();
        red(&mut e, 1);
        red(&mut e, 2);
        assert!(e.dirty());
        e.undo();
        assert_eq!(current(&e), 1);
        e.undo();
        assert_eq!(current(&e), 0);
        assert!(!e.can_undo());
        e.redo();
        assert_eq!(current(&e), 1);
        // A new edit drops what could be redone.
        red(&mut e, 5);
        assert!(!e.can_redo());
    }

    #[test]
    fn a_continuous_edit_undoes_as_one_step() {
        let mut e = editor();
        let continuous = |e: &mut Editor, key: &str, r: u8| {
            e.edit_continuous(key, |p| {
                p.set_palette_color(PaletteIndex(0), Rgba8::new(r, 0, 0, 255))
            })
            .unwrap();
        };
        for r in 1..=10 {
            continuous(&mut e, "picker", r);
        }
        e.end_gesture();
        e.undo();
        assert_eq!(current(&e), 0);
        assert!(!e.can_undo());

        // Another control, or a one-off edit, is a step of its own.
        e.redo();
        continuous(&mut e, "picker", 20);
        continuous(&mut e, "slider", 30);
        e.undo();
        assert_eq!(current(&e), 20);
        continuous(&mut e, "picker", 40);
        red(&mut e, 50);
        e.undo();
        assert_eq!(current(&e), 40);
    }

    #[test]
    fn a_reverted_continuous_edit_leaves_no_step() {
        let mut e = editor();
        red(&mut e, 1);
        for r in 2..5 {
            e.edit_continuous("drag", |p| {
                p.set_palette_color(PaletteIndex(0), Rgba8::new(r, 0, 0, 255))
            })
            .unwrap();
        }
        e.revert_gesture("drag");
        assert_eq!(current(&e), 1);
        e.undo();
        assert_eq!(current(&e), 0);
        assert!(!e.can_undo());

        // What could be redone before it still can be.
        e.edit_continuous("drag", |p| {
            p.set_palette_color(PaletteIndex(0), Rgba8::new(9, 0, 0, 255))
        })
        .unwrap();
        assert!(!e.can_redo());
        e.revert_gesture("drag");
        e.redo();
        assert_eq!(current(&e), 1);

        // Rolled back, it leaves the project as clean as it was, unless it
        // was saved partway.
        e.saved(PathBuf::from("p"));
        e.edit_continuous("drag", |p| {
            p.set_palette_color(PaletteIndex(0), Rgba8::new(8, 0, 0, 255))
        })
        .unwrap();
        assert!(e.dirty());
        e.revert_gesture("drag");
        assert!(!e.dirty());
        e.edit_continuous("drag", |p| {
            p.set_palette_color(PaletteIndex(0), Rgba8::new(8, 0, 0, 255))
        })
        .unwrap();
        e.saved(PathBuf::from("p"));
        e.revert_gesture("drag");
        assert!(e.dirty());

        // Another control's edit is only ended, not rolled back.
        e.edit_continuous("name", |p| {
            p.set_palette_color(PaletteIndex(0), Rgba8::new(7, 0, 0, 255))
        })
        .unwrap();
        e.revert_gesture("drag");
        assert_eq!(current(&e), 7);
        e.undo();
        assert_eq!(current(&e), 1);
    }

    #[test]
    fn a_reverted_edit_at_the_undo_limit_keeps_the_oldest_step() {
        let mut e = editor();
        for r in 1..=u8::try_from(UNDO_LIMIT).unwrap() {
            red(&mut e, r);
        }
        e.edit_continuous("drag", |p| {
            p.set_palette_color(PaletteIndex(0), Rgba8::new(0, 1, 0, 255))
        })
        .unwrap();
        e.revert_gesture("drag");
        let mut steps = 0;
        while e.can_undo() {
            e.undo();
            steps += 1;
        }
        assert_eq!(steps, UNDO_LIMIT);
        assert_eq!(current(&e), 0);
    }

    #[test]
    fn edits_that_fail_or_change_nothing_leave_no_step() {
        let mut e = editor();
        let generation = e.generation();
        red(&mut e, 0);
        let failed: Result<(), ()> = e.edit(|p| {
            p.set_palette_color(PaletteIndex(0), Rgba8::new(9, 9, 9, 255))
                .unwrap();
            Err(())
        });
        assert!(failed.is_err());
        assert!(!e.can_undo());
        assert!(!e.dirty());
        assert_eq!(e.generation(), generation);
        assert_eq!(current(&e), 0);
    }
}
