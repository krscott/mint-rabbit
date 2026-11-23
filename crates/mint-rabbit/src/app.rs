//! The application window: decomp and villager picker, project and export
//! actions, the editing panels, and the split Model View / Texture View.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::{self, Color32, Key, KeyboardShortcut, Modifiers, Sense};
use eframe::egui_wgpu;
use glam::Vec3;
use mint_ac::{Decomp, VillagerRef};
use mint_core::{PaletteIndex, Project, TextureContent, TextureId, VariantRef};

use crate::camera::Orbit;
use crate::editor::Editor;
use crate::panels;
use crate::picker::{self, Gpu, Picker};
use crate::render::{ModelCallback, Renderer, SceneData};
use crate::texture_view::{Frame, Paint, Selection, TextureView, Tool};

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

/// An export the side panel asks for, run once its borrows end.
enum Request {
    ExportInPlace,
    ExportToFolder,
}

/// Where a replacing project comes from.
enum Replace {
    Import(VillagerRef),
    Open(PathBuf),
    /// Close the window.
    Quit,
}

pub struct App {
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
    confirm: Option<Confirm>,
    texture_view: TextureView,
    tool: Tool,
    paint: Paint,
    /// Name for a texture set exported to a folder.
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
            confirm: None,
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
            self.confirm = Some(Confirm::Discard(replace));
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
            self.confirm = Some(Confirm::SaveOver(dir));
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
            Ok((export, path)) => self.confirm = Some(Confirm::ReplaceInDecomp(export, path)),
            Err(e) => self.fail(e),
        }
    }

    /// Asks for a folder to export a new texture set to, and whether to
    /// overwrite an earlier export there.
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
            self.confirm = Some(Confirm::OverwriteFolder(dir));
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

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Open…").clicked()
                && let Some(dir) = rfd::FileDialog::new()
                    .set_title("Open project folder")
                    .pick_folder()
            {
                self.replace(Replace::Open(dir), ui.ctx());
            }
            let has_doc = self.doc.is_some();
            if ui.add_enabled(has_doc, egui::Button::new("Save")).clicked() {
                self.save();
            }
            if ui
                .add_enabled(has_doc, egui::Button::new("Save as…"))
                .clicked()
            {
                self.save_as();
            }
            ui.separator();
            ui.label("Decomp");
            let path = ui.add(
                egui::TextEdit::singleline(&mut self.decomp_path)
                    .hint_text("path to a built ac-decomp checkout")
                    .desired_width(260.0),
            );
            ui.label("Version");
            ui.add(egui::TextEdit::singleline(&mut self.version).desired_width(90.0));
            let enter = path.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            if ui.button("Open decomp").clicked() || enter {
                self.open_decomp();
                self.offer_import();
            }
            if let Some(picker) = &mut self.picker {
                ui.separator();
                if ui.button("Import…").clicked() {
                    picker.open();
                }
            }
        });
    }

    fn side_panel(&mut self, ui: &mut egui::Ui) -> Option<Request> {
        let Some(doc) = &mut self.doc else {
            ui.label("Open a project, or open a decomp and import a villager.");
            return None;
        };
        let mut request = None;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(doc.editor.can_undo(), egui::Button::new("Undo"))
                .clicked()
            {
                doc.undo();
            }
            if ui
                .add_enabled(doc.editor.can_redo(), egui::Button::new("Redo"))
                .clicked()
            {
                doc.redo();
            }
        });
        // Panel edits, such as deleting a layer or locking it, can leave
        // picks that no longer apply.
        if let Some(r) = doc.variant() {
            doc.selection.fit(doc.editor.project(), r);
        }

        egui::CollapsingHeader::new("Export").default_open(true).show(ui, |ui| {
            let generation = doc.editor.generation();
            if doc.check.as_ref().is_none_or(|(g, _)| *g != generation) {
                let project = doc.editor.project();
                let result = export_imported(project).map(|_| ());
                doc.check = Some((generation, result));
            }
            match &doc.check {
                Some((_, Ok(()))) => {
                    ui.label("Ready to export.");
                }
                Some((_, Err(e))) => {
                    ui.colored_label(Color32::from_rgb(255, 170, 90), e);
                }
                None => {}
            }
            // The picker holds the open decomp.
            let ready = matches!(doc.check, Some((_, Ok(())))) && self.picker.is_some();
            let why_not = if self.picker.is_none() {
                "Open the decomp this project came from"
            } else {
                "Fix what the export check above reports"
            };
            let set = mint_ac::imported_from(doc.editor.project()).map(|i| i.texture_set.to_owned());
            if let Ok(set) = &set {
                if ui
                    .add_enabled(ready, egui::Button::new(format!("Replace {set} in the decomp…")))
                    .on_disabled_hover_text(why_not)
                    .clicked()
                {
                    request = Some(Request::ExportInPlace);
                }
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.folder_set).desired_width(110.0));
                    if ui
                        .add_enabled(ready, egui::Button::new("Export new set to folder…"))
                        .on_hover_text("A new texture set, based on this villager, with the steps to add it")
                        .clicked()
                    {
                        request = Some(Request::ExportToFolder);
                    }
                });
            }
        });

        egui::CollapsingHeader::new("Textures")
            .default_open(true)
            .show(ui, |ui| {
                for (i, t) in doc.editor.project().textures().iter().enumerate() {
                    let id = TextureId(i as u32);
                    ui.horizontal(|ui| {
                        let size = t.size();
                        let external = matches!(t.content(), TextureContent::External { .. });
                        let text = format!(
                            "{} {}x{}{}",
                            t.name,
                            size.x,
                            size.y,
                            if external { " (external)" } else { "" }
                        );
                        if ui
                            .selectable_value(&mut doc.selected, Some(id), text)
                            .clicked()
                        {
                            doc.selection = Selection::default();
                            self.texture_view.clear_pen();
                        }
                        let count = t.variants().len();
                        if count > 1 {
                            egui::ComboBox::from_id_salt(("variant", i))
                                .width(40.0)
                                .selected_text(&t.variants()[doc.variants[i]].name)
                                .show_ui(ui, |ui| {
                                    for (k, v) in t.variants().iter().enumerate() {
                                        if ui
                                            .selectable_value(&mut doc.variants[i], k, &v.name)
                                            .changed()
                                            && doc.selected == Some(id)
                                        {
                                            doc.selection = Selection::default();
                                            self.texture_view.clear_pen();
                                        }
                                    }
                                });
                        }
                    });
                }
            });

        egui::CollapsingHeader::new("Paint")
            .default_open(true)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for tool in Tool::ALL {
                        if ui
                            .selectable_label(self.tool == tool, tool.label())
                            .clicked()
                        {
                            self.tool = tool;
                            self.texture_view.clear_pen();
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.paint.fill, true, "Fill");
                    ui.radio_value(&mut self.paint.fill, false, "Stroke");
                    ui.add(
                        egui::DragValue::new(&mut self.paint.width)
                            .range(0.1..=64.0)
                            .speed(0.05)
                            .suffix(" px"),
                    )
                    .on_hover_text("Stroke width, for strokes and the brush");
                });
                panels::palette(ui, &mut doc.editor, &mut self.paint, &mut self.refusal);
            });

        if let Some(r) = doc.variant() {
            egui::CollapsingHeader::new("Layers")
                .default_open(true)
                .show(ui, |ui| {
                    panels::layers(
                        ui,
                        &mut doc.editor,
                        r,
                        &mut doc.selection,
                        &mut self.refusal,
                    );
                });
            egui::CollapsingHeader::new("Shapes")
                .default_open(true)
                .show(ui, |ui| {
                    panels::shapes(
                        ui,
                        &mut doc.editor,
                        r,
                        &mut doc.selection,
                        self.paint,
                        &mut self.refusal,
                    );
                });
        }

        egui::CollapsingHeader::new("View").show(ui, |ui| {
            ui.checkbox(&mut self.linear_filter, "Smooth filtering (as in game)");
            ui.checkbox(&mut self.show_uv, "UV layout in the Texture View");
            ui.label(
                "Texture View: scroll to zoom, middle-drag to pan, middle double-click to fit.",
            );
        });
        request
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
                texture: t,
                variant: doc.variants[t.index()],
                tool: self.tool,
                paint: self.paint,
                selection: &mut doc.selection,
                show_uv: self.show_uv,
                input: self.confirm.is_none(),
                message: &mut self.refusal,
            },
        );
    }

    /// Keyboard shortcuts that work anywhere outside text fields.
    fn shortcuts(&mut self, ctx: &egui::Context) {
        let command = |key| KeyboardShortcut::new(Modifiers::COMMAND, key);
        let shift_command = |key| KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, key);
        let pressed = |s: KeyboardShortcut| ctx.input_mut(|i| i.consume_shortcut(&s));
        // Text fields keep their own undo, and nothing changes the project
        // under an open confirmation.
        // Nor mid-drag, which would keep editing behind them.
        if ctx.text_edit_focused() || self.confirm.is_some() || ctx.input(|i| i.pointer.any_down())
        {
            return;
        }
        if pressed(shift_command(Key::Z)) || pressed(command(Key::Y)) {
            if let Some(d) = &mut self.doc {
                d.redo();
            }
        } else if pressed(command(Key::Z))
            && let Some(d) = &mut self.doc
        {
            d.undo();
        }
        if pressed(command(Key::S)) && self.doc.is_some() {
            self.save();
        }
        if pressed(command(Key::D))
            && let Some(d) = &mut self.doc
            && let Some(r) = d.variant()
        {
            panels::duplicate(&mut d.editor, r, &mut d.selection, &mut self.refusal);
        }
        let tools = [
            (Key::V, Tool::Select),
            (Key::P, Tool::Pen),
            (Key::B, Tool::Brush),
            (Key::E, Tool::Ellipse),
            (Key::R, Tool::Rectangle),
        ];
        for (key, tool) in tools {
            if ctx.input(|i| i.key_pressed(key) && i.modifiers.is_none()) && self.tool != tool {
                self.tool = tool;
                self.texture_view.clear_pen();
            }
        }
    }

    fn confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(confirm) = &self.confirm else { return };
        let (title, body, action) = match confirm {
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
        let mut choice = None;
        let modal = egui::Modal::new(egui::Id::unique("confirm")).show(ctx, |ui| {
            ui.heading(title);
            ui.label(body);
            ui.horizontal(|ui| {
                if ui.button(action).clicked() {
                    choice = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    choice = Some(false);
                }
            });
        });
        // Escape or a click outside cancels.
        if choice.is_none() && modal.should_close() {
            choice = Some(false);
        }
        match choice {
            Some(true) => match self.confirm.take() {
                Some(Confirm::Discard(r)) => self.do_replace(r, ctx),
                Some(Confirm::ReplaceInDecomp(export, _)) => {
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
                Some(Confirm::SaveOver(dir)) => self.save_to(dir),
                Some(Confirm::OverwriteFolder(dir)) => self.write_folder(&dir),
                None => {}
            },
            Some(false) => self.confirm = None,
            None => {}
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
            self.confirm = Some(Confirm::Discard(Replace::Quit));
        }
        // Controls wait while a canvas drag is in progress, so nothing (an
        // export, a save) acts on a drag that may still be cancelled.
        let idle = !self.texture_view.dragging();
        egui::Panel::top("top").show(ui, |ui| {
            ui.add_enabled_ui(idle, |ui| self.top_bar(ui));
        });
        egui::Panel::bottom("status").show(ui, |ui| match &self.status {
            Some(Status::Error(e)) => {
                ui.colored_label(Color32::from_rgb(255, 110, 110), e);
            }
            Some(Status::Info(m)) => {
                ui.label(m);
            }
            None => {
                ui.label(" ");
            }
        });
        let request = egui::Panel::left("side")
            .default_size(300.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .show(ui, |ui| {
                        ui.add_enabled_ui(idle, |ui| self.side_panel(ui)).inner
                    })
                    .inner
            })
            .inner;
        match request {
            Some(Request::ExportInPlace) => self.export_in_place(),
            Some(Request::ExportToFolder) => self.export_to_folder(),
            None => {}
        }
        egui::CentralPanel::default().show(ui, |ui| {
            let full = ui.available_rect_before_wrap();
            let half = full.width() * 0.5;
            let left = egui::Rect::from_min_size(full.min, egui::vec2(half - 2.0, full.height()));
            let right =
                egui::Rect::from_min_max(egui::pos2(full.min.x + half + 2.0, full.min.y), full.max);
            ui.scope_builder(egui::UiBuilder::new().max_rect(left), |ui| {
                self.model_view(ui);
            });
            ui.scope_builder(egui::UiBuilder::new().max_rect(right), |ui| {
                self.texture_view(ui);
            });
        });
        if let Some(picker) = &mut self.picker
            && let Some(v) = picker.show(&ctx)
        {
            self.replace(Replace::Import(v), &ctx);
        }
        // After the views, so a drag released this frame is finished first.
        self.shortcuts(&ctx);
        self.confirm_dialog(&ctx);
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
