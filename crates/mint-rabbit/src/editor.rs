//! The open project with its undo history.

use std::path::PathBuf;

use mint_core::Project;

/// How many edits undo can go back.
const UNDO_LIMIT: usize = 200;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Gesture {
    None,
    /// Edits until the gesture ends undo as one step; `recorded` once that
    /// step is on the undo stack.
    Open {
        recorded: bool,
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
    }

    /// Applies `f` to the project as one undoable step. If it fails, the
    /// project is put back as it was; if it changes nothing, no step is
    /// recorded. Within an open gesture, the whole gesture is one step.
    pub fn edit<T, E>(&mut self, f: impl FnOnce(&mut Project) -> Result<T, E>) -> Result<T, E> {
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
                Gesture::Open { recorded: true } => {}
                Gesture::Open { recorded } => {
                    *recorded = true;
                    self.push_undo(before);
                }
                Gesture::None => self.push_undo(before),
            }
            self.dirty = true;
            self.generation += 1;
        }
        Ok(out)
    }

    /// Opens a gesture, such as a drag: its edits until
    /// [`Self::end_gesture`] undo as one step.
    pub fn begin_gesture(&mut self) {
        if self.gesture == Gesture::None {
            self.gesture = Gesture::Open { recorded: false };
        }
    }

    pub fn end_gesture(&mut self) {
        self.gesture = Gesture::None;
    }

    fn push_undo(&mut self, before: Project) {
        self.undo.push(before);
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
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
    fn a_gesture_undoes_as_one_step() {
        let mut e = editor();
        e.begin_gesture();
        for r in 1..=10 {
            red(&mut e, r);
        }
        e.end_gesture();
        e.undo();
        assert_eq!(current(&e), 0);
        assert!(!e.can_undo());
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
