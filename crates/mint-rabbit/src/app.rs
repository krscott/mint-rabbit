//! The application window: menus, the Model View beside the Texture View
//! with its painting controls, the palette and layer dock, and dialogs.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::{self, Color32, Key, KeyboardShortcut, Modifiers, Sense};
use eframe::egui_wgpu;
use glam::Vec3;
use mint_ac::{Decomp, VillagerRef};
use mint_core::{
    Layer, LayerKind, PaletteIndex, Project, Texture, TextureContent, TextureId, VariantRef,
};

use crate::camera::Orbit;
use crate::editor::Editor;
use crate::icons::{self, Icon};
use crate::panels::{self, Target};
use crate::picker::{self, Gpu, Picker};
use crate::previews::{Previews, thumbnail};
use crate::render::{ModelCallback, Renderer, SceneData};
use crate::texture_view::{Frame, Paint, Selection, TextureView, Tool};

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

/// Width of the tool strip at the Texture View's left.
const TOOLS_WIDTH: f32 = 36.0;

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
    /// What `scene` was built from.
    scene_for: (u64, Vec<usize>),
    /// Whether the project could export now, as of an editor generation.
    check: Option<(u64, Result<(), String>)>,
}

impl Doc {
    fn new(project: Project, path: Option<PathBuf>, generation: u64) -> Self {
        let variants = vec![0; project.textures().len()];
        let selected = project
            .textures()
            .iter()
            .position(|t| matches!(t.content(), TextureContent::Painted(_)))
            .or_else(|| (!project.textures().is_empty()).then_some(0))
            .map(|i| TextureId(i as u32));
        let scene = Arc::new(SceneData::new(&project, &variants, generation));
        Self {
            editor: Editor::new(project, path),
            scene_for: (0, variants.clone()),
            variants,
            selected,
            selection: Selection::default(),
            previews: Previews::default(),
            rename: None,
            scene,
            check: None,
        }
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
    }
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
    camera: Orbit,
    linear_filter: bool,
    show_uv: bool,
    status: Option<Status>,
    /// Why the last edit in a panel or view was refused, moved into
    /// `status` after them.
    refusal: Option<String>,
    dialog: Option<Dialog>,
    texture_view: TextureView,
    tool: Tool,
    paint: Paint,
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
            camera: Orbit::framing(Vec3::splat(-1.0), Vec3::ONE),
            linear_filter: true,
            show_uv: true,
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
        let doc = Doc::new(project, path, self.generation);
        self.camera = Orbit::framing(doc.scene.mesh.bounds_min, doc.scene.mesh.bounds_max);
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
            doc.selection = Selection::default();
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
        ui.checkbox(&mut self.linear_filter, "Smooth filtering (as in game)");
        ui.checkbox(&mut self.show_uv, "UV layout in the Texture View");
        ui.separator();
        if ui.button("Fit texture to view").clicked() {
            self.texture_view.fit();
        }
        ui.separator();
        ui.weak("Texture View: scroll to zoom, middle-drag to pan,");
        ui.weak("middle double-click to fit.");
        ui.weak("Model View: drag to turn, scroll to zoom.");
    }

    /// The last thing done or what went wrong, and at the right whether the
    /// project can export.
    fn status_bar(&mut self, ui: &mut egui::Ui) {
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
                        Some(Status::Error(e)) => egui::RichText::new(e).color(ERROR),
                        Some(Status::Info(m)) => egui::RichText::new(m),
                        None => egui::RichText::new(" "),
                    };
                    ui.add(egui::Label::new(text).truncate());
                });
            });
        });
    }

    /// The palette, and the layers of the variant shown.
    fn dock(&mut self, ui: &mut egui::Ui) {
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
        // The actions keep to the bottom, and the list scrolls in the rest.
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
                    let mut target = Target::new(
                        &mut doc.editor,
                        variant,
                        &mut doc.selection,
                        &mut self.refusal,
                    );
                    panels::style_bar(ui, &mut target, &mut self.paint, self.tool);
                });
            });
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
            let works = kind.is_none_or(|k| tool.works_on(k));
            let disabled = match kind {
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

    fn model_view(&mut self, ui: &mut egui::Ui) {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::drag());
        let Some(doc) = &mut self.doc else {
            return;
        };
        let key = (doc.editor.generation(), doc.variants.clone());
        if doc.scene_for != key {
            self.generation += 1;
            doc.scene = Arc::new(SceneData::new(
                doc.editor.project(),
                &doc.variants,
                self.generation,
            ));
            doc.scene_for = key;
        }
        if response.dragged_by(egui::PointerButton::Primary) {
            let d = response.drag_delta();
            self.camera.yaw -= d.x * 0.01;
            self.camera.pitch = (self.camera.pitch + d.y * 0.01).clamp(-1.5, 1.5);
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            self.camera.distance *= (-scroll * 0.002).exp();
        }
        let aspect = rect.width() / rect.height().max(1.0);
        ui.painter().add(egui_wgpu::Callback::new_paint_callback(
            rect,
            ModelCallback {
                scene: doc.scene.clone(),
                view_proj: self.camera.view_proj(aspect),
                linear_filter: self.linear_filter,
            },
        ));
    }

    fn texture_view(&mut self, ui: &mut egui::Ui) {
        let Some(doc) = &mut self.doc else { return };
        let Some(t) = doc.selected else { return };
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
                show_uv: self.show_uv,
                input: self.dialog.is_none(),
                message: &mut self.refusal,
            },
        );
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
            if pressed && kind.is_none_or(|k| tool.works_on(k)) {
                self.set_tool(tool);
            }
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        match self.dialog.take() {
            Some(Dialog::Confirm(confirm)) => self.confirm_dialog(ctx, confirm),
            Some(Dialog::OpenDecomp { path, version }) => self.decomp_dialog(ctx, path, version),
            Some(Dialog::ExportSet { name }) => self.export_set_dialog(ctx, name),
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
                self.model_view(ui);
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
        // After the views, so a drag released this frame is finished first.
        self.shortcuts(&ctx);
        self.dialogs(&ctx);
        if let Some(refusal) = self.refusal.take() {
            self.fail(refusal);
        }
        // A continuous edit (a drag, a slider, typing) is one undo step until
        // the pointer is released and no text field has focus.
        if let Some(d) = &mut self.doc
            && !ctx.input(|i| i.pointer.any_down())
            && !ctx.text_edit_focused()
        {
            d.editor.end_gesture();
        }
        let title = self.title();
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }
}
