//! The application window: menus, the Model View beside the Texture View
//! with its painting controls, the dock of palette, layers, and stamps, and
//! dialogs.

use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};

use eframe::egui::{self, Color32, Key, KeyboardShortcut, Modifiers, Sense};
use eframe::egui_wgpu;
use glam::{Vec2, Vec3};
use mint_ac::{Decomp, VillagerRef};
use mint_core::{
    Layer, LayerKind, PaletteIndex, Project, Stamp, Texture, TextureContent, TextureId, VariantRef,
};

use crate::camera::Orbit;
use crate::editor::Editor;
use crate::icons::{self, Icon};
use crate::model_view::{Mode, ModelView};
use crate::panels::{self, Target};
use crate::picker::{self, Gpu, Picker};
use crate::previews::{Previews, thumbnail};
use crate::render::{Ghost, ModelCallback, Renderer, SceneData};
use crate::scene;
use crate::stamps::{self, Choice, Library, Placing};
use crate::surface::Surface;
use crate::texture_view::{self, Canvas, Frame, ModelPointer, Paint, Selection, TextureView, Tool};

const UNDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
const REDO: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Z);
const SAVE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
const DUPLICATE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::D);
const REDO_Y: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Y);

const WARNING: Color32 = Color32::from_rgb(255, 170, 90);
const ERROR: Color32 = Color32::from_rgb(255, 110, 110);

/// Shown in place of the views while no project is open.
const NOTHING_OPEN: &str =
    "Use the File menu to open a project, or to open a decomp and import a villager.";

/// Size of a texture's or variant's thumbnail above the Texture View.
const THUMBNAIL: egui::Vec2 = egui::vec2(40.0, 32.0);

/// Height of the mode selector above the Model View.
const MODE_BAR_HEIGHT: f32 = 28.0;

/// Width of the tool strip at the Texture View's left.
const TOOLS_WIDTH: f32 = 36.0;

/// Size of a stamp's thumbnail in the Stamp Library.
const STAMP_THUMBNAIL: egui::Vec2 = egui::vec2(48.0, 40.0);

/// Most height the Stamp Library's thumbnails take before they scroll.
const STAMPS_HEIGHT: f32 = 150.0;

/// Most height the stamp colors dialog's description and rows take before
/// they scroll.
const STAMP_COLORS_HEIGHT: f32 = 360.0;
/// Height of the stamp colors dialog without its description and rows,
/// which the window keeps room for.
const STAMP_COLORS_ROOM: f32 = 180.0;

pub struct Options {
    pub decomp: Option<PathBuf>,
    /// Game version the decomp was built for, such as `GAFE01_00`.
    pub game_version: String,
    /// A project directory to open.
    pub project: Option<PathBuf>,
}

/// The open project and how it is shown.
struct Doc {
    editor: Editor,
    /// Variant shown for each texture.
    variants: Vec<usize>,
    /// Texture shown in the Texture View; `None` when there are none.
    selected: Option<TextureId>,
    selection: Selection,
    previews: Previews,
    /// The layer name being typed in the layer list.
    rename: Option<panels::Rename>,
    scene: Arc<SceneData>,
    /// The scene's surface, for the pointer in the Model View.
    surface: Surface,
    /// What `scene` was built from.
    scene_for: (u64, Vec<usize>),
    /// Whether the project could export now, as of an editor generation.
    check: Option<(u64, Result<(), String>)>,
}

impl Doc {
    /// A document for `project`. Its scene is the `generation`th built, and
    /// its surface finds cutout edges as a view sampling textures `smooth`ly
    /// or not shows them.
    fn new(project: Project, path: Option<PathBuf>, generation: u64, smooth: bool) -> Self {
        let variants = vec![0; project.textures().len()];
        let selected = project
            .textures()
            .iter()
            .position(|t| matches!(t.content(), TextureContent::Painted(_)))
            .or_else(|| (!project.textures().is_empty()).then_some(0))
            .map(|i| TextureId(i as u32));
        let scene = Arc::new(SceneData::new(&project, &variants, generation));
        let selection = selected.map_or_else(Selection::default, |texture| {
            Selection::top(
                &project,
                VariantRef {
                    texture,
                    variant: 0,
                },
            )
        });
        Self {
            editor: Editor::new(project, path),
            scene_for: (0, variants.clone()),
            variants,
            selected,
            selection,
            previews: Previews::default(),
            rename: None,
            surface: Surface::new(scene.clone(), smooth),
            scene,
            check: None,
        }
    }

    /// Rebuilds the scene if the project or the variants shown changed. The
    /// surface follows the scene, except while `dragging`, so a drag meets
    /// the surface as it was when it began. `generation` counts rebuilds;
    /// `smooth` is whether the view samples textures smoothly.
    fn refresh_scene(&mut self, generation: &mut u64, smooth: bool, dragging: bool) {
        if self.scene_stale() {
            let key = (self.editor.generation(), self.variants.clone());
            *generation += 1;
            self.scene = Arc::new(SceneData::new(
                self.editor.project(),
                &self.variants,
                *generation,
            ));
            self.scene_for = key;
        }
        let surface = &self.surface;
        let stale = !Arc::ptr_eq(surface.scene(), &self.scene) || surface.smooth() != smooth;
        if stale && !dragging {
            self.surface = surface.with_scene(self.scene.clone(), smooth);
        }
    }

    /// Whether the project or the variants shown changed since the scene
    /// was built.
    fn scene_stale(&self) -> bool {
        self.scene_for.0 != self.editor.generation() || self.scene_for.1 != self.variants
    }

    /// The painted variant shown in the Texture View, if any.
    fn variant(&self) -> Option<VariantRef> {
        let t = self.selected?;
        let painted = !self.editor.project().texture(t).variants().is_empty();
        painted.then(|| VariantRef {
            texture: t,
            variant: self.variants[t.index()],
        })
    }

    /// Undoes the last step. Picks are dropped, since shapes may have moved
    /// between the indices they name.
    fn undo(&mut self) {
        self.editor.undo();
        self.selection.shapes.clear();
        self.selection.anchor = None;
    }

    fn redo(&mut self) {
        self.editor.redo();
        self.selection.shapes.clear();
        self.selection.anchor = None;
    }

    fn name(&self) -> String {
        if let Some(p) = self.editor.path() {
            return p.file_name().map_or_else(
                || p.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
        }
        mint_ac::imported_from(self.editor.project())
            .map_or_else(|_| "untitled".into(), |i| i.texture_set.to_owned())
    }
}

/// An action waiting for the user to confirm it.
enum Confirm {
    /// Replace unsaved changes with another project.
    Discard(Replace),
    /// Overwrite the texture set's source file in the decomp.
    ReplaceInDecomp(mint_ac::TextureExport, PathBuf),
    /// Save into a folder that holds another project.
    SaveOver(PathBuf),
    /// Export a texture set over files already in a folder.
    OverwriteFolder(PathBuf),
    /// Remove a stamp from the library.
    DeleteStamp(String),
}

/// The texture-only export of a project to the texture set it was imported
/// from.
fn export_imported(project: &Project) -> Result<mint_ac::TextureExport, String> {
    mint_ac::imported_from(project)
        .and_then(|i| mint_ac::export_textures(project, i.texture_set))
        .map_err(|e| e.to_string())
}

/// The line at the bottom of the window: the last thing done or what
/// went wrong.
enum Status {
    Info(String),
    Error(String),
    /// Why an edit was refused, shown until the project next changes (by
    /// an edit, undo, or redo) from `generation`, the editor's then.
    Refused {
        message: String,
        generation: u64,
    },
}

/// The one dialog open over the window.
enum Dialog {
    Confirm(Confirm),
    /// The decomp checkout and game version to open, as typed so far.
    OpenDecomp {
        path: String,
        version: String,
    },
    /// The name of a texture set to export to a folder, as typed so far.
    ExportSet {
        name: String,
    },
    /// The name of a new stamp of the picked shapes, as typed so far.
    SaveStamp {
        name: String,
        stamp: Stamp,
    },
    /// A stamp's new name, as typed so far.
    RenameStamp {
        from: String,
        name: String,
    },
    /// What each color of the stamp being placed becomes, as chosen so far.
    StampColors {
        choices: Vec<Choice>,
    },
}

/// A texture's name and pixel size.
fn texture_label(texture: &Texture) -> String {
    let size = texture.size();
    let external = matches!(texture.content(), TextureContent::External { .. });
    format!(
        "{} {}×{}{}",
        texture.name,
        size.x,
        size.y,
        if external { " (external)" } else { "" }
    )
}

/// A dim label after a row of thumbnails, kept in one piece when the row
/// wraps.
fn caption(ui: &mut egui::Ui, text: String) {
    ui.add(egui::Label::new(egui::RichText::new(text).weak()).extend());
}

fn tool_icon(tool: Tool) -> Icon {
    match tool {
        Tool::Select => Icon::MousePointer2,
        Tool::Pen => Icon::PenTool,
        Tool::Brush => Icon::Brush,
        Tool::Ellipse => Icon::Circle,
        Tool::Rectangle => Icon::Square,
        Tool::Pencil => Icon::Pencil,
        Tool::Eraser => Icon::Eraser,
        Tool::Fill => Icon::PaintBucket,
        Tool::Stamp => Icon::Stamp,
    }
}

/// The Model View's Ghost Cursor at texture point `at` for `tool`: the ring
/// it paints in, or the bounds of the stamp it would place there.
fn ghost(
    tool: Tool,
    paint: Paint,
    placing: Option<&Placing>,
    texture: TextureId,
    at: Vec2,
) -> Option<Ghost> {
    if tool == Tool::Stamp {
        let stamp = &placing?.stamp;
        let (min, max) = stamp.bounds();
        return Some(Ghost {
            texture,
            center: stamp.offset(at) + (min + max) / 2.0,
            half: (max - min) / 2.0,
            radius: 0.0,
        });
    }
    let radius = texture_view::footprint(tool, paint)?;
    Some(Ghost {
        texture,
        center: at,
        half: Vec2::ZERO,
        radius,
    })
}

/// Opens `dir` in the desktop's file manager. The opener keeps running; the
/// returned channel gets why if it then exits with an error.
fn open_folder(ctx: &egui::Context, dir: &Path) -> Result<mpsc::Receiver<String>, String> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let child = std::process::Command::new(opener)
        .arg(dir)
        .spawn()
        .map_err(|e| format!("Couldn't open {} with {opener}: {e}", dir.display()))?;
    // Reaped once it exits, which the opener does as soon as it hands over.
    let (send, failed) = mpsc::channel();
    let (ctx, dir) = (ctx.clone(), dir.to_owned());
    std::thread::spawn(move || {
        let mut child = child;
        let why = match child.wait() {
            Ok(status) if status.success() => return,
            Ok(status) => format!("{opener} couldn't open {}: {status}", dir.display()),
            Err(e) => format!("{opener} couldn't open {}: {e}", dir.display()),
        };
        let _ = send.send(why);
        ctx.request_repaint();
    });
    Ok(failed)
}

/// Gives a dialog's first text field the keyboard while nothing else has it,
/// so the dialog opens ready for typing.
fn focus_if_none(ui: &egui::Ui, field: &egui::Response) {
    if ui.memory(|m| m.focused().is_none()) {
        field.request_focus();
    }
}

/// Whether Enter was pressed in a text field.
fn entered(ui: &egui::Ui, field: &egui::Response) -> bool {
    field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter))
}

/// Shows a dialog with `body`, a button for `action`, which `enabled`
/// allows, and Cancel. `body` returns whether it asks for the action itself,
/// as by Enter in its text field. Returns whether the action was chosen, once
/// the dialog is answered; Escape or a click outside cancels.
fn modal(
    ctx: &egui::Context,
    title: &str,
    action: &str,
    enabled: bool,
    body: impl FnOnce(&mut egui::Ui) -> bool,
) -> Option<bool> {
    let mut choice = None;
    let modal = egui::Modal::new(egui::Id::unique("dialog")).show(ctx, |ui| {
        ui.heading(title);
        if body(ui) && enabled {
            choice = Some(true);
        }
        ui.horizontal(|ui| {
            if ui.add_enabled(enabled, egui::Button::new(action)).clicked() {
                choice = Some(true);
            }
            if ui.button("Cancel").clicked() {
                choice = Some(false);
            }
        });
    });
    if choice.is_none() && modal.should_close() {
        choice = Some(false);
    }
    choice
}

/// Where a replacing project comes from.
enum Replace {
    Import(VillagerRef),
    Open(PathBuf),
    /// Close the window.
    Quit,
}

/// The View menu's settings.
struct ViewOptions {
    linear_filter: bool,
    show_uv: bool,
    /// Whether each view shows the Ghost Cursor where the pointer over the
    /// other meets the texture.
    ghost_cursor: bool,
}

pub struct App {
    /// The decomp checkout and game version last asked for.
    decomp_path: String,
    version: String,
    /// The open decomp, and its villagers to import.
    picker: Option<Picker>,
    /// The window's GPU, which also renders the picker's thumbnails.
    gpu: Option<Gpu>,
    doc: Option<Doc>,
    /// Counts scene rebuilds, so the renderer uploads each new one.
    generation: u64,
    model: ModelView,
    view: ViewOptions,
    /// Where the pointer over the Model View, in Paint mode, meets the
    /// model this frame, for the Texture View's Ghost Cursor.
    model_ghost: Option<(TextureId, Vec2)>,
    /// The texture point under the pointer over the Texture View last
    /// frame, for the Model View's Ghost Cursor.
    texture_ghost: Option<(TextureId, Vec2)>,
    status: Option<Status>,
    /// Why the last edit in a panel or view was refused, moved into
    /// `status` after them.
    refusal: Option<String>,
    dialog: Option<Dialog>,
    texture_view: TextureView,
    tool: Tool,
    paint: Paint,
    library: Library,
    /// The stamp the Stamp tool places, once one is chosen.
    placing: Option<Placing>,
    /// Where why opening the stamp folder failed arrives, while the
    /// opener runs.
    opener: Option<mpsc::Receiver<String>>,
    /// Name last given to a texture set exported to a folder.
    folder_set: String,
    /// The window title last set.
    title: String,
    /// Closing was confirmed, so unsaved changes no longer stop it.
    quitting: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, options: Options) -> Self {
        if let Some(rs) = cc.wgpu_render_state.as_ref() {
            Renderer::install(rs);
        }
        let gpu = cc
            .wgpu_render_state
            .as_ref()
            .map(|rs| (rs.device.clone(), rs.queue.clone()));
        // A press anywhere else, a drag included, leaves a text field, so
        // keys and edits go where the pointer went.
        cc.egui_ctx.options_mut(|o| {
            o.input_options.surrender_focus_on = egui::SurrenderFocusOn::Presses;
        });
        icons::install(&cc.egui_ctx);
        let mut app = Self {
            decomp_path: options
                .decomp
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            version: options.game_version,
            picker: None,
            gpu,
            doc: None,
            generation: 0,
            model: ModelView::new(Orbit::framing(Vec3::splat(-1.0), Vec3::ONE)),
            view: ViewOptions {
                linear_filter: true,
                show_uv: true,
                ghost_cursor: true,
            },
            model_ghost: None,
            texture_ghost: None,
            status: None,
            refusal: None,
            dialog: None,
            texture_view: TextureView::default(),
            tool: Tool::Select,
            paint: Paint {
                color: PaletteIndex(0),
                fill: true,
                width: 1.0,
            },
            library: Library::new(stamps::default_dir()),
            placing: None,
            opener: None,
            folder_set: String::new(),
            title: String::new(),
            quitting: false,
        };
        if options.decomp.is_some() {
            app.open_decomp();
        }
        if let Some(dir) = options.project {
            app.open_project(&dir);
        }
        app.offer_import();
        app
    }

    fn fail(&mut self, error: impl Into<String>) {
        self.status = Some(Status::Error(error.into()));
    }

    fn tell(&mut self, message: impl Into<String>) {
        self.status = Some(Status::Info(message.into()));
    }

    fn ask(&mut self, confirm: Confirm) {
        self.dialog = Some(Dialog::Confirm(confirm));
    }

    fn open_decomp(&mut self) {
        self.status = None;
        let result = Decomp::open(self.decomp_path.trim(), self.version.trim())
            .and_then(|d| Picker::new(d, self.gpu.clone()));
        match result {
            Ok(picker) => self.picker = Some(picker),
            Err(e) => {
                self.picker = None;
                self.fail(e.to_string());
            }
        }
    }

    fn decomp(&self) -> Option<&Decomp> {
        self.picker.as_ref().map(Picker::decomp)
    }

    /// Opens the picker when nothing is open yet, since importing is then
    /// the next step.
    fn offer_import(&mut self) {
        if self.doc.is_none()
            && let Some(picker) = &mut self.picker
        {
            picker.open();
        }
    }

    fn set_doc(&mut self, project: Project, path: Option<PathBuf>) {
        self.generation += 1;
        let doc = Doc::new(project, path, self.generation, self.view.linear_filter);
        self.model.camera = Orbit::framing(doc.scene.mesh.bounds_min, doc.scene.mesh.bounds_max);
        self.texture_view = TextureView::default();
        self.paint.color = PaletteIndex(0);
        self.folder_set = mint_ac::imported_from(doc.editor.project())
            .map(|i| format!("{}_new", i.texture_set))
            .unwrap_or_default();
        self.doc = Some(doc);
    }

    fn dirty(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| d.editor.dirty())
    }

    /// Runs `replace` now, or asks first when it would lose changes.
    fn replace(&mut self, replace: Replace, ctx: &egui::Context) {
        if self.dirty() {
            self.ask(Confirm::Discard(replace));
        } else {
            self.do_replace(replace, ctx);
        }
    }

    fn do_replace(&mut self, replace: Replace, ctx: &egui::Context) {
        match replace {
            Replace::Import(v) => self.import(&v),
            Replace::Open(dir) => self.open_project(&dir),
            Replace::Quit => {
                self.quitting = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn import(&mut self, v: &VillagerRef) {
        self.status = None;
        let Some(decomp) = self.decomp() else {
            return;
        };
        // The scene is checked here and rebuilt for the open document.
        match picker::import(decomp, v) {
            Ok((project, _)) => {
                self.set_doc(project, None);
                if let Some(picker) = &mut self.picker {
                    picker.imported(v);
                }
            }
            // The picker stays open, to pick another.
            Err(e) => self.fail(format!("Couldn't import {}: {e}", v.texture_set)),
        }
    }

    fn open_project(&mut self, dir: &Path) {
        self.status = None;
        match mint_core::file::load(dir) {
            Ok(project) => {
                self.set_doc(project, Some(dir.to_owned()));
                self.tell(format!("Opened {}", dir.display()));
            }
            Err(e) => self.fail(e.to_string()),
        }
    }

    /// Saves to the project's folder, or asks for one.
    fn save(&mut self) {
        let Some(path) = self.doc.as_ref().and_then(|d| d.editor.path().cloned()) else {
            self.save_as();
            return;
        };
        self.save_to(path);
    }

    fn save_as(&mut self) {
        let Some(dir) = rfd::FileDialog::new()
            .set_title("Save project to folder")
            .pick_folder()
        else {
            return;
        };
        let current = self.doc.as_ref().and_then(|d| d.editor.path());
        if dir.join(mint_core::file::PROJECT_FILE).exists() && current != Some(&dir) {
            self.ask(Confirm::SaveOver(dir));
        } else {
            self.save_to(dir);
        }
    }

    fn save_to(&mut self, dir: PathBuf) {
        let Some(doc) = &mut self.doc else { return };
        match mint_core::file::save(doc.editor.project(), &dir) {
            Ok(()) => {
                let saved = format!("Saved {}", dir.display());
                doc.editor.saved(dir);
                self.tell(saved);
            }
            Err(e) => self.fail(e.to_string()),
        }
    }

    /// Asks to replace the imported texture set's source in the decomp.
    fn export_in_place(&mut self) {
        self.status = None;
        let (Some(doc), Some(decomp)) = (&self.doc, self.decomp()) else {
            return;
        };
        let project = doc.editor.project();
        let result = export_imported(project).and_then(|e| {
            decomp
                .check_texture_set(&e)
                .map(|p| (e, p))
                .map_err(|e| e.to_string())
        });
        match result {
            Ok((export, path)) => self.ask(Confirm::ReplaceInDecomp(export, path)),
            Err(e) => self.fail(e),
        }
    }

    /// Asks for a folder to export the texture set named `folder_set` to,
    /// and whether to overwrite an earlier export there.
    fn export_to_folder(&mut self) {
        let Some(dir) = rfd::FileDialog::new()
            .set_title("Export texture set to folder")
            .pick_folder()
        else {
            return;
        };
        let set = self.folder_set.trim();
        let taken = mint_ac::TextureExport::folder_files(set, &dir)
            .iter()
            .any(|f| f.exists());
        if taken {
            self.ask(Confirm::OverwriteFolder(dir));
        } else {
            self.write_folder(&dir);
        }
    }

    fn write_folder(&mut self, dir: &Path) {
        self.status = None;
        let (Some(doc), Some(decomp)) = (&self.doc, self.decomp()) else {
            return;
        };
        let project = doc.editor.project();
        let result = mint_ac::imported_from(project)
            .and_then(|i| {
                mint_ac::export_textures(project, self.folder_set.trim())
                    .map(|e| (i.texture_set.to_owned(), e))
            })
            .map_err(|e| e.to_string())
            .and_then(|(base, e)| {
                decomp
                    .write_texture_folder(&e, &base, dir)
                    .map_err(|e| e.to_string())
            });
        match result {
            Ok([c, notes]) => {
                self.tell(format!(
                    "Wrote {} and {}; the notes say how to add it to the decomp",
                    c.display(),
                    notes.display()
                ));
            }
            Err(e) => self.fail(e),
        }
    }

    /// Whether the open project can export now, or why not; `None` with no
    /// project open.
    fn export_ready(&mut self) -> Option<Result<(), String>> {
        // The picker holds the open decomp.
        let has_decomp = self.picker.is_some();
        let doc = self.doc.as_mut()?;
        let generation = doc.editor.generation();
        if doc.check.as_ref().is_none_or(|(g, _)| *g != generation) {
            let result = export_imported(doc.editor.project()).map(|_| ());
            doc.check = Some((generation, result));
        }
        let (_, check) = doc.check.as_ref()?;
        Some(match check {
            Err(e) => Err(e.clone()),
            Ok(()) if !has_decomp => Err("Open the decomp this project came from".into()),
            Ok(()) => Ok(()),
        })
    }

    /// The painted variant shown and what is picked in it, for a control to
    /// edit.
    fn target(&mut self) -> Option<Target<'_>> {
        let doc = self.doc.as_mut()?;
        let variant = doc.variant()?;
        Some(Target::new(
            &mut doc.editor,
            variant,
            &mut doc.selection,
            &mut self.refusal,
        ))
    }

    fn set_tool(&mut self, tool: Tool) {
        if self.tool != tool {
            self.tool = tool;
            self.texture_view.clear_pen();
        }
    }

    /// Whether a stamp can be placed on the picked layer now, or why not.
    fn stamp_ready(&self) -> Result<(), String> {
        let doc = self.doc.as_ref().ok_or("Open a project to place stamps")?;
        let r = doc
            .variant()
            .ok_or("An external texture is filled in by the game, and isn't painted.")?;
        texture_view::drawable(doc.editor.project(), r, &doc.selection)
    }

    /// Picks the Stamp tool to place `stamp`, named `name`. Its colors are
    /// chosen at once if each matches a palette entry exactly; otherwise
    /// [`Self::ask_stamp_colors`] asks.
    fn arm(&mut self, name: String, stamp: Arc<Stamp>) {
        let Some(doc) = &self.doc else { return };
        self.placing = Some(Placing::new(name, stamp, doc.editor.project().palette()));
        self.set_tool(Tool::Stamp);
    }

    /// Asks what the Stamp tool's stamp colors become, if they are still to
    /// be chosen.
    fn ask_stamp_colors(&mut self) {
        if self.tool != Tool::Stamp || self.dialog.is_some() {
            return;
        }
        let (Some(placing), Some(doc)) = (&self.placing, &self.doc) else {
            return;
        };
        if placing.choices.is_none() {
            let choices = stamps::nearest_choices(&placing.stamp, doc.editor.project().palette());
            self.dialog = Some(Dialog::StampColors { choices });
        }
    }

    /// Asks for the name of a new stamp of the picked shapes.
    fn ask_stamp_name(&mut self) {
        let Some(doc) = &self.doc else { return };
        let Some(r) = doc.variant() else { return };
        let project = doc.editor.project();
        let Some(layer) = project.variant(r).layers().get(doc.selection.layer) else {
            return;
        };
        let mut picked = doc.selection.shapes.clone();
        picked.sort_unstable();
        let shapes = picked
            .iter()
            .filter_map(|&s| layer.shapes().get(s).cloned())
            .collect();
        match Stamp::from_shapes(shapes, project.palette()) {
            Ok(stamp) => {
                self.dialog = Some(Dialog::SaveStamp {
                    name: String::new(),
                    stamp,
                });
            }
            Err(e) => self.fail(e.to_string()),
        }
    }

    /// The kind of the picked layer, or `None` with no painted variant
    /// shown or no layers in it. A pick past the top, as after an undo, is
    /// of the top layer, as [`Selection::fit`] makes it.
    fn picked_kind(&self) -> Option<LayerKind> {
        let doc = self.doc.as_ref()?;
        let layers = doc.editor.project().variant(doc.variant()?).layers();
        layers
            .get(doc.selection.layer)
            .or(layers.last())
            .map(Layer::kind)
    }

    /// Whether `tool` can be picked with a layer of `kind` picked: it works
    /// there, and for Stamp, a stamp has been chosen.
    fn can_use(&self, tool: Tool, kind: Option<LayerKind>) -> bool {
        kind.is_none_or(|k| tool.works_on(k)) && (tool != Tool::Stamp || self.placing.is_some())
    }

    /// Switches to a tool that works on the picked layer, if the current one
    /// doesn't.
    fn fit_tool(&mut self) {
        if let Some(kind) = self.picked_kind() {
            self.set_tool(self.tool.on(kind));
        }
    }

    /// Shows a variant in the Texture View, and that variant of its texture
    /// in the Model View. Picks don't carry over to another variant.
    fn show_variant(&mut self, r: VariantRef) {
        let Some(doc) = &mut self.doc else { return };
        let i = r.texture.index();
        if (doc.selected, doc.variants[i]) != (Some(r.texture), r.variant) {
            doc.selected = Some(r.texture);
            doc.variants[i] = r.variant;
            doc.selection = Selection::top(doc.editor.project(), r);
            self.texture_view.clear_pen();
        }
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| self.file_menu(ui));
            ui.menu_button("Edit", |ui| self.edit_menu(ui));
            ui.menu_button("View", |ui| self.view_menu(ui));
        });
    }

    fn file_menu(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let has_doc = self.doc.is_some();
        if ui.button("Open project…").clicked()
            && let Some(dir) = rfd::FileDialog::new()
                .set_title("Open project folder")
                .pick_folder()
        {
            self.replace(Replace::Open(dir), &ctx);
        }
        let save = egui::Button::new("Save").shortcut_text(ctx.format_shortcut(&SAVE));
        if ui.add_enabled(has_doc, save).clicked() {
            self.save();
        }
        if ui
            .add_enabled(has_doc, egui::Button::new("Save as…"))
            .clicked()
        {
            self.save_as();
        }
        ui.separator();
        if ui.button("Open decomp…").clicked() {
            self.dialog = Some(Dialog::OpenDecomp {
                path: self.decomp_path.clone(),
                version: self.version.clone(),
            });
        }
        let import = egui::Button::new("Import villager…");
        if ui
            .add_enabled(self.picker.is_some(), import)
            .on_disabled_hover_text("Open a decomp first")
            .clicked()
            && let Some(picker) = &mut self.picker
        {
            picker.open();
        }
        ui.separator();
        let folder = self.library.dir();
        let why = folder.as_ref().err().cloned().unwrap_or_default();
        if ui
            .add_enabled(folder.is_ok(), egui::Button::new("Open stamp folder"))
            .on_hover_text("The folder of the Stamp Library, one file per stamp")
            .on_disabled_hover_text(why)
            .clicked()
        {
            let opened = self
                .library
                .create_dir()
                .and_then(|()| open_folder(ui.ctx(), self.library.dir()?));
            match opened {
                Ok(failed) => self.opener = Some(failed),
                Err(e) => self.fail(e),
            }
        }
        ui.separator();
        let ready = self.export_ready();
        let can_export = matches!(ready, Some(Ok(())));
        let why_not = match &ready {
            Some(Err(e)) => e.as_str(),
            _ => "No project is open",
        };
        let set = self.doc.as_ref().and_then(|d| {
            mint_ac::imported_from(d.editor.project())
                .ok()
                .map(|i| i.texture_set.to_owned())
        });
        let replace = set.map_or_else(
            || "Replace in the decomp…".to_owned(),
            |set| format!("Replace {set} in the decomp…"),
        );
        if ui
            .add_enabled(can_export, egui::Button::new(replace))
            .on_disabled_hover_text(why_not)
            .clicked()
        {
            self.export_in_place();
        }
        if ui
            .add_enabled(can_export, egui::Button::new("Export new texture set…"))
            .on_hover_text("A new texture set, based on this villager, with the steps to add it")
            .on_disabled_hover_text(why_not)
            .clicked()
        {
            self.dialog = Some(Dialog::ExportSet {
                name: self.folder_set.clone(),
            });
        }
    }

    fn edit_menu(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let item = |text: &str, shortcut: &KeyboardShortcut| {
            egui::Button::new(text.to_owned()).shortcut_text(ctx.format_shortcut(shortcut))
        };
        let (undo, redo) = self.doc.as_ref().map_or((false, false), |d| {
            (d.editor.can_undo(), d.editor.can_redo())
        });
        if ui.add_enabled(undo, item("Undo", &UNDO)).clicked()
            && let Some(d) = &mut self.doc
        {
            d.undo();
        }
        if ui.add_enabled(redo, item("Redo", &REDO)).clicked()
            && let Some(d) = &mut self.doc
        {
            d.redo();
        }
        ui.separator();
        let (shapes, anchor) = self.target().map_or((false, false), |t| {
            let picked = t.selection();
            (!picked.shapes.is_empty(), picked.anchor.is_some())
        });
        if ui
            .add_enabled(shapes, item("Duplicate shapes", &DUPLICATE))
            .clicked()
            && let Some(mut t) = self.target()
        {
            panels::duplicate(&mut t);
        }
        if ui
            .add_enabled(shapes, egui::Button::new("Save as stamp…"))
            .clicked()
        {
            self.ask_stamp_name();
        }
        let delete = if anchor {
            "Delete anchor"
        } else {
            "Delete shapes"
        };
        if ui.add_enabled(shapes, egui::Button::new(delete)).clicked()
            && let Some(mut t) = self.target()
        {
            panels::delete(&mut t);
        }
    }

    fn view_menu(&mut self, ui: &mut egui::Ui) {
        ui.checkbox(
            &mut self.view.linear_filter,
            "Smooth filtering (as in game)",
        );
        ui.checkbox(&mut self.view.show_uv, "UV layout in the Texture View");
        ui.checkbox(
            &mut self.view.ghost_cursor,
            "Ghost Cursor in the other view",
        );
        ui.separator();
        if ui.button("Fit texture to view").clicked() {
            self.texture_view.fit();
        }
        ui.separator();
        ui.weak("Texture View: scroll to zoom, middle-drag to pan,");
        ui.weak("middle double-click to fit.");
        ui.weak("Model View: in Orbit mode, drag to turn; in Paint mode,");
        ui.weak("draw with the tools and middle-drag to turn. Scroll to zoom.");
    }

    /// The last thing done or what went wrong, and at the right whether the
    /// project can export.
    fn status_bar(&mut self, ui: &mut egui::Ui) {
        // A refusal lasts until the project changes, as when the user fixes
        // what caused it.
        if let Some(Status::Refused { generation, .. }) = self.status
            && self
                .doc
                .as_ref()
                .is_none_or(|d| d.editor.generation() != generation)
        {
            self.status = None;
        }
        let ready = self.export_ready();
        ui.horizontal(|ui| {
            let half = ui.available_width() * 0.5;
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                match &ready {
                    Some(Ok(())) => {
                        ui.label("Ready to export");
                    }
                    Some(Err(e)) => {
                        let text = egui::RichText::new(format!("Can't export: {e}")).color(WARNING);
                        ui.scope(|ui| {
                            ui.set_max_width(half);
                            // Hovering a cut-off label shows all of it.
                            ui.add(egui::Label::new(text).truncate());
                        });
                    }
                    None => {}
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let text = match &self.status {
                        Some(Status::Error(e) | Status::Refused { message: e, .. }) => {
                            egui::RichText::new(e).color(ERROR)
                        }
                        Some(Status::Info(m)) => egui::RichText::new(m),
                        None => egui::RichText::new(" "),
                    };
                    ui.add(egui::Label::new(text).truncate());
                });
            });
        });
    }

    /// The palette, the layers of the variant shown, and under them the
    /// Stamp Library.
    fn dock(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("stamps")
            .resizable(false)
            .frame(egui::Frame::NONE)
            .show(ui, |ui| self.stamp_library(ui));
        let Some(doc) = &mut self.doc else { return };
        panels::palette(ui, &mut doc.editor, &mut self.paint, &mut self.refusal);
        ui.separator();
        ui.strong("Layers");
        let Some(variant) = doc.variant() else {
            ui.weak("An external texture has no layers.");
            return;
        };
        let mut target = Target::new(
            &mut doc.editor,
            variant,
            &mut doc.selection,
            &mut self.refusal,
        );
        // The picked layer's modifiers keep to the bottom, the layer actions
        // over them, and the list scrolls in the rest.
        egui::Panel::bottom("modifiers")
            .resizable(false)
            .frame(egui::Frame::NONE)
            .show(ui, |ui| panels::modifier_stack(ui, &mut target));
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            let action = panels::layer_actions(ui, &target);
            panels::layer_opacity(ui, &mut target);
            ui.separator();
            let menu = ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        panels::layer_list(ui, &mut target, &mut doc.previews, &mut doc.rename)
                    })
                    .inner
            });
            if let Some(action) = action.or(menu.inner) {
                panels::apply_layer_action(&mut target, &mut doc.rename, action);
            }
        });
    }

    /// The stamps in the library, which a click picks to place and a
    /// right-click renames or deletes. Opening the section reads the folder
    /// again.
    fn stamp_library(&mut self, ui: &mut egui::Ui) {
        enum Action {
            Place(String, Arc<Stamp>),
            Rename(String),
            Delete(String),
        }
        ui.separator();
        let ready = self.stamp_ready();
        let armed = self
            .placing
            .as_ref()
            .filter(|_| self.tool == Tool::Stamp)
            .map(|p| (p.name.clone(), p.stamp.clone()));
        let problem = self.library.problem();
        let mut action = None;
        let header = egui::CollapsingHeader::new(egui::RichText::new("Stamps").strong())
            .id_salt("stamps")
            .default_open(true)
            .show(ui, |ui| {
                if let Some(why) = problem {
                    ui.weak(why);
                    return;
                }
                let entries = self.library.entries_mut();
                if entries.is_empty() {
                    ui.weak("No stamps yet. Pick shapes, then use Save as stamp.");
                    return;
                }
                egui::ScrollArea::vertical()
                    .id_salt("stamps")
                    .max_height(STAMPS_HEIGHT)
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            for entry in entries {
                                ui.vertical(|ui| {
                                    ui.set_width(STAMP_THUMBNAIL.x);
                                    let name = entry.name.clone();
                                    let response = match entry.stamp.clone() {
                                        Ok(stamp) => {
                                            // The one placing, unless its file
                                            // has changed since.
                                            let picked = armed
                                                .as_ref()
                                                .is_some_and(|(n, s)| *n == name && *s == stamp);
                                            let r = thumbnail(ui, STAMP_THUMBNAIL, picked, |ctx| {
                                                // As many pixels as it shows, so
                                                // nothing drawn is dropped.
                                                let side = STAMP_THUMBNAIL.min_elem()
                                                    * ctx.pixels_per_point();
                                                entry.thumbnail(ctx, &stamp, side as u32)
                                            });
                                            let hover = match &ready {
                                                Ok(()) => format!("{name}\nClick to place it"),
                                                Err(why) => format!("{name}\n{why}"),
                                            };
                                            let r = r.on_hover_text(hover);
                                            if r.clicked() && ready.is_ok() {
                                                action = Some(Action::Place(name.clone(), stamp));
                                            }
                                            r
                                        }
                                        Err(e) => {
                                            let (rect, r) = ui.allocate_exact_size(
                                                STAMP_THUMBNAIL,
                                                Sense::click(),
                                            );
                                            let painter = ui.painter();
                                            painter.rect_filled(
                                                rect,
                                                2.0,
                                                ui.visuals().extreme_bg_color,
                                            );
                                            painter.text(
                                                rect.center(),
                                                egui::Align2::CENTER_CENTER,
                                                Icon::TriangleAlert.unicode(),
                                                icons::font(icons::LARGE),
                                                WARNING,
                                            );
                                            r.on_hover_text(format!("{name} can't be read: {e}"))
                                        }
                                    };
                                    response.context_menu(|ui| {
                                        if ui.button("Rename…").clicked() {
                                            action = Some(Action::Rename(name.clone()));
                                        }
                                        if ui.button("Delete…").clicked() {
                                            action = Some(Action::Delete(name.clone()));
                                        }
                                    });
                                    ui.add(
                                        egui::Label::new(egui::RichText::new(&name).small())
                                            .truncate(),
                                    );
                                });
                            }
                        });
                    });
            });
        // The header keeps its state under its own id.
        let id = header.header_response.id;
        let open =
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
                .is_open();
        if header.header_response.clicked() && open {
            self.library.scan();
            ui.ctx().request_repaint();
        }
        match action {
            Some(Action::Place(name, stamp)) => self.arm(name, stamp),
            Some(Action::Rename(from)) => {
                self.dialog = Some(Dialog::RenameStamp {
                    name: from.clone(),
                    from,
                });
            }
            Some(Action::Delete(name)) => self.ask(Confirm::DeleteStamp(name)),
            None => {}
        }
    }

    /// A thumbnail of each texture, and under them one of each variant of
    /// the texture shown.
    fn texture_strip(&mut self, ui: &mut egui::Ui) {
        let Some(doc) = &mut self.doc else { return };
        let mut show = None;
        let generation = doc.editor.generation();
        let project = doc.editor.project();
        ui.horizontal_wrapped(|ui| {
            for (i, t) in project.textures().iter().enumerate() {
                let r = VariantRef {
                    texture: TextureId(i as u32),
                    variant: doc.variants[i],
                };
                let picked = doc.selected == Some(r.texture);
                let image = |ctx: &egui::Context| doc.previews.variant(ctx, project, generation, r);
                if thumbnail(ui, THUMBNAIL, picked, image)
                    .on_hover_text(texture_label(t))
                    .clicked()
                {
                    show = Some(r);
                }
            }
            if let Some(texture) = doc.selected {
                caption(ui, texture_label(project.texture(texture)));
            }
        });
        let Some(texture) = doc.selected else { return };
        let variants = project.texture(texture).variants();
        if variants.len() > 1 {
            let shown = doc.variants[texture.index()];
            ui.horizontal_wrapped(|ui| {
                for (variant, v) in variants.iter().enumerate() {
                    let r = VariantRef { texture, variant };
                    let image =
                        |ctx: &egui::Context| doc.previews.variant(ctx, project, generation, r);
                    if thumbnail(ui, THUMBNAIL, shown == variant, image)
                        .on_hover_text(format!("Variant {}", v.name))
                        .clicked()
                    {
                        show = Some(r);
                    }
                }
                caption(ui, format!("variant {}", variants[shown].name));
            });
        }
        if let Some(r) = show {
            self.show_variant(r);
        }
    }

    /// The style bar, one row high whatever it holds, so the Texture View
    /// under it stays put when shapes are picked. What doesn't fit scrolls
    /// with the wheel, without a scroll bar, which would cover the controls.
    fn style_bar(&mut self, ui: &mut egui::Ui) {
        let Some(doc) = &mut self.doc else { return };
        let (mut save, mut recolor) = (false, false);
        ui.style_mut().always_scroll_the_only_direction = true;
        egui::ScrollArea::horizontal()
            .id_salt("style bar")
            .auto_shrink([false, true])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let Some(variant) = doc.variant() else {
                        ui.weak("An external texture is filled in by the game, and isn't painted.");
                        return;
                    };
                    if self.tool == Tool::Stamp
                        && let Some(placing) = &self.placing
                    {
                        panels::bar_label(ui, "Stamp", "The stamp a click places");
                        ui.strong(&placing.name);
                        recolor = ui
                            .button("Colors…")
                            .on_hover_text("Choose the palette entry each stamp color becomes")
                            .clicked();
                        ui.separator();
                    }
                    let mut target = Target::new(
                        &mut doc.editor,
                        variant,
                        &mut doc.selection,
                        &mut self.refusal,
                    );
                    save = panels::style_bar(ui, &mut target, &mut self.paint, self.tool);
                });
            });
        if save {
            self.ask_stamp_name();
        }
        if recolor && let (Some(placing), Some(doc)) = (&mut self.placing, &self.doc) {
            let palette = doc.editor.project().palette();
            let choices = placing
                .current(palette)
                .unwrap_or_else(|| stamps::nearest_choices(&placing.stamp, palette));
            self.dialog = Some(Dialog::StampColors { choices });
        }
    }

    /// The tools, those that don't work on the picked layer disabled.
    fn tool_strip(&mut self, ui: &mut egui::Ui) {
        let kind = self.picked_kind();
        for tool in Tool::ALL {
            let icon = icons::text(tool_icon(tool), icons::LARGE);
            let button =
                egui::Button::selectable(self.tool == tool, icon).min_size(egui::vec2(28.0, 28.0));
            let keys = ui.ctx().format_shortcut(&tool.shortcut());
            let hover = format!("{} ({keys})", tool.label());
            let works = self.can_use(tool, kind);
            let disabled = match kind {
                _ if tool == Tool::Stamp => {
                    format!("{hover}\nChoose a stamp under Stamps to use it")
                }
                Some(LayerKind::Vector) => format!("{hover}\nPick a raster layer to use it"),
                _ => format!("{hover}\nPick a vector layer to use it"),
            };
            if ui
                .add_enabled(works, button)
                .on_hover_text(hover)
                .on_disabled_hover_text(disabled)
                .clicked()
            {
                self.set_tool(tool);
            }
        }
    }

    /// The Texture View under its texture strip and style bar, with the
    /// tools at its left. `idle` is whether the controls take input.
    fn texture_pane(&mut self, ui: &mut egui::Ui, idle: bool) {
        ui.add_enabled_ui(idle, |ui| {
            self.texture_strip(ui);
            // After the dock and the texture strip, where the picked layer
            // may have changed, and before the style bar, which shows the
            // tool's settings.
            self.fit_tool();
            self.style_bar(ui);
        });
        ui.separator();
        let painted = self.doc.as_ref().is_some_and(|d| d.variant().is_some());
        let rest = ui.available_rect_before_wrap();
        let (tools, view) = rest.split_left_right_at_x(rest.left() + TOOLS_WIDTH);
        ui.scope_builder(egui::UiBuilder::new().max_rect(tools), |ui| {
            ui.add_enabled_ui(idle && painted, |ui| self.tool_strip(ui));
        });
        ui.scope_builder(egui::UiBuilder::new().max_rect(view), |ui| {
            self.texture_view(ui);
        });
    }

    /// The Model View, with its mode selector. In Paint mode, the tools
    /// draw on the texture under the pointer, which a press shows in the
    /// Texture View. `idle` is whether the selector takes input.
    fn model_view(&mut self, ui: &mut egui::Ui, idle: bool) {
        // The mode selector has a strip of its own, so a press on it never
        // reaches the model.
        let full = ui.available_rect_before_wrap();
        let (bar, rect) = full.split_top_bottom_at_y(full.top() + MODE_BAR_HEIGHT);
        ui.scope_builder(egui::UiBuilder::new().max_rect(bar), |ui| {
            ui.add_enabled_ui(idle, |ui| self.mode_selector(ui));
        });
        // Clicks and drags without keyboard focus, as in the Texture View, so
        // Enter isn't a click here.
        let response = ui.allocate_rect(rect, Sense::CLICK | Sense::DRAG);
        self.model.hovered = response.hovered();
        self.model_ghost = None;
        let dragging = self.texture_view.dragging();
        let Some(doc) = &mut self.doc else {
            return;
        };
        doc.refresh_scene(&mut self.generation, self.view.linear_filter, dragging);
        let paint = self.model.mode == Mode::Paint && self.dialog.is_none();
        // The middle button turns the camera in either mode.
        let orbit = response.dragged_by(egui::PointerButton::Middle)
            || (!paint && response.dragged_by(egui::PointerButton::Primary));
        // The camera holds still while a drag paints, so the drag's path stays
        // on the surface it was traced on.
        let camera = &mut self.model.camera;
        if orbit && !dragging {
            let d = response.drag_delta();
            camera.yaw -= d.x * 0.01;
            camera.pitch = (camera.pitch + d.y * 0.01).clamp(-1.5, 1.5);
        }
        if response.hovered() && !dragging {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            camera.distance *= (-scroll * 0.002).exp();
        }
        let pointer = response
            .interact_pointer_pos()
            .or_else(|| response.hover_pos());
        let hit = pointer
            .filter(|_| paint)
            .and_then(|p| self.model.hit(&doc.surface, rect, p));
        if paint {
            self.paint_model(ui, &response, rect, pointer, hit);
        }
        let dragging = self.texture_view.dragging();
        let Some(doc) = &mut self.doc else {
            return;
        };
        doc.refresh_scene(&mut self.generation, self.view.linear_filter, dragging);

        // The tool's footprint where the pointer meets a painted texture in
        // Paint mode, or else the Texture View's Ghost Cursor.
        let project = doc.editor.project();
        let own = hit
            .filter(|h| response.hovered() && !project.texture(h.texture).variants().is_empty())
            .map(|h| (h.texture, h.point));
        if self.view.ghost_cursor {
            self.model_ghost = own;
        }
        let ghost = own
            .or(self.texture_ghost.filter(|_| self.view.ghost_cursor))
            .and_then(|(texture, at)| {
                ghost(self.tool, self.paint, self.placing.as_ref(), texture, at)
            });
        let aspect = rect.width() / rect.height().max(1.0);
        ui.painter().add(egui_wgpu::Callback::new_paint_callback(
            rect,
            ModelCallback {
                scene: doc.scene.clone(),
                view_proj: self.model.camera.view_proj(aspect),
                linear_filter: self.view.linear_filter,
                ghost,
            },
        ));
    }

    /// The Model View's mode buttons.
    fn mode_selector(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            let modes = [
                (
                    Mode::Orbit,
                    Icon::Orbit,
                    "Orbit",
                    "Drag to turn the camera around the model.",
                ),
                (
                    Mode::Paint,
                    Icon::Paintbrush,
                    "Paint",
                    "Draw on the model with the picked tool.\nMiddle-drag to turn the camera.",
                ),
            ];
            for (mode, icon, label, hover) in modes {
                let text = (icons::text(icon, icons::SMALL), label);
                let button = egui::Button::selectable(self.model.mode == mode, text);
                if ui.add(button).on_hover_text(hover).clicked() {
                    self.model.mode = mode;
                }
            }
        });
    }

    /// Paint-mode input over the Model View, whose pointer is at `pointer`
    /// and meets the model at `hit`.
    fn paint_model(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        rect: egui::Rect,
        pointer: Option<egui::Pos2>,
        hit: Option<crate::surface::Hit>,
    ) {
        // Where the primary button went down and came up this frame, which
        // may differ from where the pointer is by the time the frame sees
        // them.
        let button = |pressed| {
            ui.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: p,
                        ..
                    } if *p == pressed => Some(*pos),
                    _ => None,
                })
            })
        };
        let (pressed_at, release) = (button(true), button(false));
        let origin = pressed_at
            .or_else(|| ui.input(|i| i.pointer.press_origin()))
            .or(pointer);
        // A drag finishes on the texture it began on, even if a new press
        // arrives before its release is seen.
        if response.hovered()
            && ui.input(|i| i.pointer.primary_pressed())
            && !self.texture_view.dragging()
            && let Some(doc) = &self.doc
            && let Some(h) = origin.and_then(|o| self.model.hit(&doc.surface, rect, o))
        {
            self.paint_texture(h.texture);
            // The tools fit the texture now shown before the press acts.
            self.fit_tool();
        }
        let Some(doc) = &mut self.doc else { return };
        let Some(r) = doc.variant() else { return };
        let on = |h: Option<crate::surface::Hit>| h.filter(|h| h.texture == r.texture);
        let mut press = None;
        if response.drag_started_by(egui::PointerButton::Primary)
            && let Some(origin) = origin
        {
            press = on(self.model.press(&doc.surface, rect, origin)).map(|h| h.point);
        }
        // A drag's path, for the tools that follow it, up to where the
        // pointer is, or on the release's frame to where the button let go.
        let primary = egui::PointerButton::Primary;
        let end = if response.drag_stopped_by(primary) {
            release.or(pointer)
        } else {
            pointer.filter(|_| response.dragged_by(primary))
        };
        let path = match end {
            Some(p) if !matches!(self.tool, Tool::Select | Tool::Fill | Tool::Stamp) => {
                self.model.trail(&doc.surface, rect, p, r.texture)
            }
            _ => Vec::new(),
        };
        // A click lands where the button came up, a drag starts where it
        // went down, and either finds what it's near within a reach measured
        // there.
        let acted_at = if response.clicked() {
            release.or(pointer)
        } else if response.drag_started() {
            origin
        } else {
            None
        };
        let acted = acted_at.and_then(|p| Some((p, self.model.hit(&doc.surface, rect, p)?)));
        let texels_per_point = match acted {
            Some((p, h)) if on(Some(h)).is_some() => {
                self.model.texels_per_point(&doc.surface, rect, p, &h)
            }
            _ => 1.0,
        };
        let here = if response.clicked() {
            acted.map(|(_, h)| h)
        } else {
            hit
        };
        let model_pointer = ModelPointer {
            press,
            here: on(here).map(|h| h.point),
            path,
            texels_per_point,
        };
        let canvas = Canvas {
            editor: &mut doc.editor,
            selection: &mut doc.selection,
            paint: self.paint,
            message: &mut self.refusal,
            stamp: self.placing.as_mut(),
        };
        self.texture_view
            .model_interact(ui, response, canvas, r, self.tool, &model_pointer);
    }

    /// Shows `texture`, pressed on in the Model View, in the Texture View so
    /// the tools act on it; refuses an external texture, and another
    /// texture while a pen path is open on this one.
    fn paint_texture(&mut self, texture: TextureId) {
        let Some(doc) = &self.doc else { return };
        let t = doc.editor.project().texture(texture);
        if t.variants().is_empty() {
            self.refusal = Some(format!(
                "{} is filled in by the game, and isn't painted.",
                t.name
            ));
            return;
        }
        if doc.selected == Some(texture) {
            return;
        }
        if self.texture_view.drawing_path() {
            self.refusal = Some(
                "Finish the pen's path, or press Escape to drop it, to draw on another texture."
                    .into(),
            );
            return;
        }
        let variant = doc.variants[texture.index()];
        self.show_variant(VariantRef { texture, variant });
    }

    fn texture_view(&mut self, ui: &mut egui::Ui) {
        let Some(doc) = &mut self.doc else { return };
        let Some(t) = doc.selected else {
            self.texture_ghost = None;
            return;
        };
        self.texture_view.show(
            ui,
            Frame {
                editor: &mut doc.editor,
                previews: &mut doc.previews,
                texture: t,
                variant: doc.variants[t.index()],
                tool: self.tool,
                paint: self.paint,
                selection: &mut doc.selection,
                show_uv: self.view.show_uv,
                input: self.dialog.is_none(),
                message: &mut self.refusal,
                ghost: self
                    .model_ghost
                    .and_then(|(texture, p)| (texture == t).then_some(p)),
                model_hovered: self.model.hovered,
                stamp: self.placing.as_mut(),
            },
        );
        // The Model View, already drawn this frame, shows the Texture View's
        // Ghost Cursor in the next.
        let ghost = self.texture_view.hover().map(|p| (t, p));
        if ghost != self.texture_ghost {
            ui.ctx().request_repaint();
        }
        self.texture_ghost = ghost;
    }

    /// Keyboard shortcuts that work anywhere outside text fields.
    fn shortcuts(&mut self, ctx: &egui::Context) {
        let pressed = |s: &KeyboardShortcut| ctx.input_mut(|i| i.consume_shortcut(s));
        // Text fields keep their own undo, and nothing changes the project
        // under an open dialog.
        // Nor mid-drag, which would keep editing behind them.
        if ctx.text_edit_focused() || self.dialog.is_some() || ctx.input(|i| i.pointer.any_down()) {
            return;
        }
        let (redo, undo) = (pressed(&REDO) || pressed(&REDO_Y), pressed(&UNDO));
        if let Some(d) = &mut self.doc
            && (redo || undo)
        {
            if redo {
                d.redo();
            } else {
                d.undo();
            }
            // An open menu, such as a layer's, would otherwise stay open on
            // whatever now holds its place.
            egui::Popup::close_all(ctx);
        }
        if pressed(&SAVE) && self.doc.is_some() {
            self.save();
        }
        if pressed(&DUPLICATE)
            && let Some(mut t) = self.target()
        {
            panels::duplicate(&mut t);
        }
        let kind = self.picked_kind();
        for tool in Tool::ALL {
            // The modifiers held with the key press itself, which a quick
            // Shift+E can release before the frame's end.
            let keys = tool.shortcut();
            let pressed = ctx.input(|i| {
                i.events.iter().any(|e| {
                    matches!(e, egui::Event::Key { key, pressed: true, modifiers, .. }
                        if *key == keys.logical_key && modifiers.matches_exact(keys.modifiers))
                })
            });
            if pressed && self.can_use(tool, kind) {
                self.set_tool(tool);
            }
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        match self.dialog.take() {
            Some(Dialog::Confirm(confirm)) => self.confirm_dialog(ctx, confirm),
            Some(Dialog::OpenDecomp { path, version }) => self.decomp_dialog(ctx, path, version),
            Some(Dialog::ExportSet { name }) => self.export_set_dialog(ctx, name),
            Some(Dialog::SaveStamp { name, stamp }) => self.save_stamp_dialog(ctx, name, stamp),
            Some(Dialog::RenameStamp { from, name }) => self.rename_stamp_dialog(ctx, from, name),
            Some(Dialog::StampColors { choices }) => self.stamp_colors_dialog(ctx, choices),
            None => {}
        }
    }

    fn confirm_dialog(&mut self, ctx: &egui::Context, confirm: Confirm) {
        let (title, body, action) = match &confirm {
            Confirm::Discard(_) => (
                "Discard changes?",
                "The open project has unsaved changes.".to_owned(),
                "Discard",
            ),
            Confirm::ReplaceInDecomp(_, path) => (
                "Replace texture set?",
                format!(
                    "This overwrites {}. Build the decomp with `python configure.py --non-matching && ninja` to see it.",
                    path.display()
                ),
                "Replace",
            ),
            Confirm::SaveOver(dir) => (
                "Replace project?",
                format!(
                    "{} already holds a project, which this replaces.",
                    dir.display()
                ),
                "Replace",
            ),
            Confirm::OverwriteFolder(dir) => (
                "Replace export?",
                format!(
                    "{} already holds files for texture set {}, which this replaces.",
                    dir.display(),
                    self.folder_set.trim()
                ),
                "Replace",
            ),
            Confirm::DeleteStamp(name) => (
                "Delete stamp?",
                format!(
                    "This removes {name} from the Stamp Library for good. Shapes placed from it stay."
                ),
                "Delete",
            ),
        };
        let choice = modal(ctx, title, action, true, |ui| {
            ui.label(body);
            false
        });
        match (choice, confirm) {
            (None, confirm) => self.dialog = Some(Dialog::Confirm(confirm)),
            (Some(false), _) => {}
            (Some(true), Confirm::Discard(r)) => self.do_replace(r, ctx),
            (Some(true), Confirm::ReplaceInDecomp(export, _)) => {
                let result = self.decomp().map(|d| d.write_texture_set(&export));
                match result {
                    Some(Ok(path)) => match self.picker.as_mut().map_or(Ok(()), Picker::reload) {
                        Ok(()) => self.tell(format!(
                            "Wrote {}; build with `python configure.py --non-matching && ninja`",
                            path.display()
                        )),
                        Err(e) => self.fail(format!(
                            "Wrote {}, but the import list couldn't be refreshed: {e}",
                            path.display()
                        )),
                    },
                    Some(Err(e)) => self.fail(e.to_string()),
                    None => {}
                }
            }
            (Some(true), Confirm::SaveOver(dir)) => self.save_to(dir),
            (Some(true), Confirm::OverwriteFolder(dir)) => self.write_folder(&dir),
            (Some(true), Confirm::DeleteStamp(name)) => match self.library.delete(&name) {
                Ok(()) => {
                    if self.placing.as_ref().is_some_and(|p| p.name == name) {
                        self.placing = None;
                        if self.tool == Tool::Stamp {
                            self.set_tool(Tool::Select);
                        }
                    }
                    self.tell(format!("Deleted stamp {name}"));
                }
                Err(e) => self.fail(e),
            },
        }
    }

    /// Why `name` can't name a new stamp, if it can't.
    fn stamp_name_problem(&self, name: &str) -> Option<String> {
        match stamps::check_name(name) {
            Err(why) => Some(why),
            Ok(name) if self.library.taken(name) => {
                Some(format!("A stamp named {name:?} already exists"))
            }
            Ok(_) => None,
        }
    }

    /// A field for a stamp's name, which says what's wrong with it. Returns
    /// whether Enter was pressed in it.
    fn stamp_name_field(ui: &mut egui::Ui, name: &mut String, problem: Option<&str>) -> bool {
        let enter = ui
            .horizontal(|ui| {
                ui.label("Name");
                let field = ui.add(egui::TextEdit::singleline(name).desired_width(200.0));
                focus_if_none(ui, &field);
                entered(ui, &field)
            })
            .inner;
        // Nothing to complain about until something is typed.
        match problem {
            Some(why) if !name.is_empty() => {
                ui.colored_label(WARNING, why);
            }
            _ => {
                ui.label(" ");
            }
        }
        enter
    }

    fn save_stamp_dialog(&mut self, ctx: &egui::Context, mut name: String, stamp: Stamp) {
        let problem = self.stamp_name_problem(&name);
        let choice = modal(ctx, "Save as stamp", "Save", problem.is_none(), |ui| {
            ui.label("The picked shapes, saved with their colors to the Stamp Library.");
            Self::stamp_name_field(ui, &mut name, problem.as_deref())
        });
        match choice {
            Some(true) => match self.library.save(&name, &stamp) {
                Ok(()) => self.tell(format!("Saved stamp {}", name.trim())),
                Err(e) => self.fail(e),
            },
            Some(false) => {}
            None => self.dialog = Some(Dialog::SaveStamp { name, stamp }),
        }
    }

    fn rename_stamp_dialog(&mut self, ctx: &egui::Context, from: String, mut name: String) {
        // The name it has needs no complaint, but renames nothing.
        let unchanged = name.trim() == from;
        let problem = (!unchanged)
            .then(|| self.stamp_name_problem(&name))
            .flatten();
        let enabled = !unchanged && problem.is_none();
        let choice = modal(ctx, "Rename stamp", "Rename", enabled, |ui| {
            ui.label(format!("A new name for {from}."));
            Self::stamp_name_field(ui, &mut name, problem.as_deref())
        });
        match choice {
            Some(true) => match self.library.rename(&from, &name) {
                Ok(()) => {
                    let name = name.trim();
                    if let Some(p) = self.placing.as_mut().filter(|p| p.name == from) {
                        name.clone_into(&mut p.name);
                    }
                    self.tell(format!("Renamed stamp {from} to {name}"));
                }
                Err(e) => self.fail(e),
            },
            Some(false) => {}
            None => self.dialog = Some(Dialog::RenameStamp { from, name }),
        }
    }

    /// Chooses what each color of the Stamp tool's stamp becomes: a palette
    /// entry, or a new one. Cancelling before any are chosen leaves the
    /// Stamp tool.
    fn stamp_colors_dialog(&mut self, ctx: &egui::Context, mut choices: Vec<Choice>) {
        let (Some(placing), Some(doc)) = (&mut self.placing, &self.doc) else {
            return;
        };
        let project = doc.editor.project();
        let palette = project.palette().colors();
        let added = stamps::new_entries(&placing.stamp, &choices);
        let choice = modal(ctx, "Stamp colors", "Use", true, |ui| {
            // Many colors, or a long name, scroll, keeping the buttons in
            // view.
            let room = ui.ctx().content_rect().height() - STAMP_COLORS_ROOM;
            let row = ui.spacing().interact_size.y;
            egui::ScrollArea::vertical()
                .id_salt("stamp colors")
                .min_scrolled_height(row)
                .max_height(room.clamp(row, STAMP_COLORS_HEIGHT))
                .show(ui, |ui| {
                    ui.label(format!(
                        "The palette entry each color of {} becomes.",
                        placing.name
                    ));
                    egui::Grid::new("stamp colors")
                        .num_columns(2)
                        .spacing([12.0, 6.0])
                        .show(ui, |ui| {
                            for (color, choice) in placing.stamp.colors().iter().zip(&mut choices) {
                                let shown = scene::display_color(project, *color);
                                panels::swatch(ui, shown, false).on_hover_text(format!(
                                    "Stamp color #{:02X}{:02X}{:02X}, alpha {}",
                                    color.r, color.g, color.b, color.a
                                ));
                                ui.horizontal_wrapped(|ui| {
                                    for (i, &c) in palette.iter().enumerate() {
                                        let index = PaletteIndex(i as u8);
                                        let picked =
                                            matches!(*choice, Choice::Entry(j, _) if j == index);
                                        let shown = scene::display_color(project, c);
                                        if panels::swatch(ui, shown, picked)
                                            .on_hover_text(format!("Entry {i}"))
                                            .clicked()
                                        {
                                            *choice = Choice::Entry(index, c);
                                        }
                                    }
                                    if ui
                                        .selectable_label(*choice == Choice::New, "New")
                                        .on_hover_text("Add the stamp color to the palette")
                                        .clicked()
                                    {
                                        *choice = Choice::New;
                                    }
                                });
                                ui.end_row();
                            }
                        });
                });
            if let Some(limit) = panels::palette_limit(project)
                && palette.len() + added > limit
            {
                ui.colored_label(
                    WARNING,
                    format!("This makes {} of {limit} colors", palette.len() + added),
                );
            }
            false
        });
        match choice {
            Some(true) => placing.choices = Some(choices),
            Some(false) => {
                if placing.choices.is_none() {
                    self.set_tool(Tool::Select);
                }
            }
            None => self.dialog = Some(Dialog::StampColors { choices }),
        }
    }

    fn decomp_dialog(&mut self, ctx: &egui::Context, mut path: String, mut version: String) {
        let filled = !path.trim().is_empty() && !version.trim().is_empty();
        let choice = modal(ctx, "Open decomp", "Open", filled, |ui| {
            let mut enter = false;
            egui::Grid::new("decomp").num_columns(2).show(ui, |ui| {
                ui.label("Checkout");
                ui.horizontal(|ui| {
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut path)
                            .hint_text("path to a built ac-decomp checkout")
                            .desired_width(360.0),
                    );
                    focus_if_none(ui, &field);
                    enter |= entered(ui, &field);
                    if ui.button("Browse…").clicked()
                        && let Some(dir) = rfd::FileDialog::new()
                            .set_title("Open decomp checkout")
                            .pick_folder()
                    {
                        path = dir.display().to_string();
                    }
                });
                ui.end_row();
                ui.label("Game version");
                let field = ui.add(egui::TextEdit::singleline(&mut version).desired_width(120.0));
                enter |= entered(ui, &field);
                ui.end_row();
            });
            enter
        });
        match choice {
            Some(true) => {
                self.decomp_path = path;
                self.version = version;
                self.open_decomp();
                self.offer_import();
            }
            Some(false) => {}
            None => self.dialog = Some(Dialog::OpenDecomp { path, version }),
        }
    }

    fn export_set_dialog(&mut self, ctx: &egui::Context, mut name: String) {
        let filled = !name.trim().is_empty();
        let choice = modal(
            ctx,
            "Export new texture set",
            "Choose folder…",
            filled,
            |ui| {
                ui.label("A new texture set, based on this villager, with the steps to add it.");
                ui.horizontal(|ui| {
                    ui.label("Name");
                    let field = ui.add(egui::TextEdit::singleline(&mut name).desired_width(200.0));
                    focus_if_none(ui, &field);
                    entered(ui, &field)
                })
                .inner
            },
        );
        match choice {
            Some(true) => {
                self.folder_set = name;
                self.export_to_folder();
            }
            Some(false) => {}
            None => self.dialog = Some(Dialog::ExportSet { name }),
        }
    }

    fn title(&self) -> String {
        match &self.doc {
            Some(d) => format!(
                "{}{} — mint-rabbit",
                d.name(),
                if d.editor.dirty() { " *" } else { "" }
            ),
            None => "mint-rabbit".into(),
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Closing the window with unsaved changes asks first.
        if ctx.input(|i| i.viewport().close_requested()) && self.dirty() && !self.quitting {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.ask(Confirm::Discard(Replace::Quit));
        }
        // Controls wait while a canvas drag is in progress, so nothing (an
        // export, a save) acts on a drag that may still be cancelled.
        let idle = !self.texture_view.dragging();
        egui::Panel::top("menu").show(ui, |ui| {
            ui.add_enabled_ui(idle, |ui| self.menu_bar(ui));
        });
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        if self.doc.is_some() {
            egui::Panel::right("dock")
                .default_size(240.0)
                .size_range(200.0..=420.0)
                .show(ui, |ui| {
                    ui.add_enabled_ui(idle, |ui| self.dock(ui));
                });
        }
        egui::CentralPanel::default().show(ui, |ui| {
            if self.doc.is_none() {
                ui.centered_and_justified(|ui| ui.weak(NOTHING_OPEN));
                return;
            }
            let full = ui.available_rect_before_wrap();
            let half = full.width() * 0.5;
            let left = egui::Rect::from_min_size(full.min, egui::vec2(half - 2.0, full.height()));
            let right =
                egui::Rect::from_min_max(egui::pos2(full.min.x + half + 2.0, full.min.y), full.max);
            ui.scope_builder(egui::UiBuilder::new().max_rect(left), |ui| {
                self.model_view(ui, idle);
            });
            ui.scope_builder(egui::UiBuilder::new().max_rect(right), |ui| {
                self.texture_pane(ui, idle);
            });
        });
        if let Some(picker) = &mut self.picker
            && let Some(v) = picker.show(&ctx)
        {
            self.replace(Replace::Import(v), &ctx);
        }
        if let Some(failed) = &self.opener {
            match failed.try_recv() {
                Ok(why) => {
                    self.opener = None;
                    self.fail(why);
                }
                Err(mpsc::TryRecvError::Disconnected) => self.opener = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        // After the views, so a drag released this frame is finished first.
        self.shortcuts(&ctx);
        self.ask_stamp_colors();
        self.dialogs(&ctx);
        if let Some(message) = self.refusal.take()
            && let Some(d) = &self.doc
        {
            let generation = d.editor.generation();
            self.status = Some(Status::Refused {
                message,
                generation,
            });
        }
        // A continuous edit (a drag, a slider, typing) is one undo step until
        // the pointer is released and no text field has focus.
        if let Some(d) = &mut self.doc
            && !ctx.input(|i| i.pointer.any_down())
            && !ctx.text_edit_focused()
        {
            d.editor.end_gesture();
        }
        // Changes after the Model View was drawn, from the Texture View, its
        // strip, keys, or dialogs, show in the next frame.
        if let Some(d) = &self.doc
            && d.scene_stale()
        {
            ctx.request_repaint();
        }
        let title = self.title();
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }
}
