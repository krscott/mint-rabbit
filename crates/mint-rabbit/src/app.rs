//! The application window: decomp and villager picker, texture list, and the
//! split Model View / Texture View.

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{self, Color32, Sense};
use eframe::egui_wgpu;
use glam::Vec3;
use mint_ac::{Decomp, VillagerRef};
use mint_core::{Project, TextureContent, TextureId};

use crate::camera::Orbit;
use crate::render::{ModelCallback, Renderer, SceneData};
use crate::texture_view::TextureView;

pub struct Options {
    pub decomp: Option<PathBuf>,
    /// Game version the decomp was built for, such as `GAFE01_00`.
    pub game_version: String,
}

struct Loaded {
    project: Project,
    /// Variant shown for each texture.
    variants: Vec<usize>,
    /// Texture shown in the Texture View; `None` when there are none.
    selected: Option<TextureId>,
    scene: Arc<SceneData>,
}

pub struct App {
    decomp_path: String,
    version: String,
    decomp: Option<Decomp>,
    villagers: Vec<VillagerRef>,
    villager: usize,
    loaded: Option<Loaded>,
    generation: u64,
    camera: Orbit,
    linear_filter: bool,
    error: Option<String>,
    texture_view: TextureView,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, options: Options) -> Self {
        if let Some(rs) = cc.wgpu_render_state.as_ref() {
            Renderer::install(rs);
        }
        let mut app = Self {
            decomp_path: options
                .decomp
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            version: options.game_version,
            decomp: None,
            villagers: Vec::new(),
            villager: 0,
            loaded: None,
            generation: 0,
            camera: Orbit::framing(Vec3::splat(-1.0), Vec3::ONE),
            linear_filter: true,
            error: None,
            texture_view: TextureView::default(),
        };
        if options.decomp.is_some() {
            app.open_decomp();
        }
        app
    }

    fn open_decomp(&mut self) {
        self.error = None;
        let result = Decomp::open(self.decomp_path.trim(), self.version.trim())
            .and_then(|d| d.villagers().map(|v| (d, v)));
        match result {
            Ok((d, v)) => {
                self.decomp = Some(d);
                self.villagers = v;
                self.villager = 0;
            }
            Err(e) => {
                self.decomp = None;
                self.villagers.clear();
                self.error = Some(e.to_string());
            }
        }
    }

    fn import(&mut self) {
        self.error = None;
        let (Some(decomp), Some(v)) = (&self.decomp, self.villagers.get(self.villager)) else {
            return;
        };
        match decomp.import(v) {
            Ok(project) => {
                let variants = vec![0; project.textures().len()];
                let selected = project
                    .textures()
                    .iter()
                    .position(|t| matches!(t.content(), TextureContent::Painted(_)))
                    .or_else(|| (!project.textures().is_empty()).then_some(0))
                    .map(|i| TextureId(i as u32));
                self.generation += 1;
                let scene = Arc::new(SceneData::new(&project, &variants, self.generation));
                self.camera = Orbit::framing(scene.mesh.bounds_min, scene.mesh.bounds_max);
                self.loaded = Some(Loaded {
                    project,
                    variants,
                    selected,
                    scene,
                });
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Decomp");
            let path = ui.add(
                egui::TextEdit::singleline(&mut self.decomp_path)
                    .hint_text("path to a built ac-decomp checkout")
                    .desired_width(320.0),
            );
            ui.label("Version");
            ui.add(egui::TextEdit::singleline(&mut self.version).desired_width(90.0));
            let enter = path.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.button("Open").clicked() || enter {
                self.open_decomp();
            }
            if !self.villagers.is_empty() {
                ui.separator();
                let label = |v: &VillagerRef| format!("{} ({})", v.texture_set, v.model);
                egui::ComboBox::from_id_salt("villager")
                    .selected_text(label(&self.villagers[self.villager]))
                    .height(400.0)
                    .show_ui(ui, |ui| {
                        for (i, v) in self.villagers.iter().enumerate() {
                            ui.selectable_value(&mut self.villager, i, label(v));
                        }
                    });
                if ui.button("Import").clicked() {
                    self.import();
                }
            }
        });
    }

    fn side_panel(&mut self, ui: &mut egui::Ui) {
        let Some(loaded) = &mut self.loaded else {
            ui.label("Open a decomp and import a villager.");
            return;
        };
        ui.heading("Textures");
        let mut changed = false;
        for (i, t) in loaded.project.textures().iter().enumerate() {
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
                ui.selectable_value(&mut loaded.selected, Some(id), text);
                let count = t.variants().len();
                if count > 1 {
                    egui::ComboBox::from_id_salt(("variant", i))
                        .width(40.0)
                        .selected_text(&t.variants()[loaded.variants[i]].name)
                        .show_ui(ui, |ui| {
                            for (k, v) in t.variants().iter().enumerate() {
                                changed |= ui
                                    .selectable_value(&mut loaded.variants[i], k, &v.name)
                                    .changed();
                            }
                        });
                }
            });
        }
        ui.separator();
        ui.heading("Palette");
        ui.horizontal_wrapped(|ui| {
            for &c in loaded.project.palette().colors() {
                let c = crate::scene::display_color(&loaded.project, c);
                let (rect, response) =
                    ui.allocate_exact_size(egui::vec2(18.0, 18.0), Sense::hover());
                ui.painter().rect_filled(
                    rect,
                    2.0,
                    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a),
                );
                response.on_hover_text(format!("#{:02X}{:02X}{:02X} alpha {}", c.r, c.g, c.b, c.a));
            }
        });
        ui.separator();
        ui.checkbox(&mut self.linear_filter, "Smooth filtering (as in game)");
        if changed {
            self.generation += 1;
            loaded.scene = Arc::new(SceneData::new(
                &loaded.project,
                &loaded.variants,
                self.generation,
            ));
        }
    }

    fn model_view(&mut self, ui: &mut egui::Ui) {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::drag());
        let Some(loaded) = &self.loaded else {
            return;
        };
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
                scene: loaded.scene.clone(),
                view_proj: self.camera.view_proj(aspect),
                linear_filter: self.linear_filter,
            },
        ));
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("top").show(ui, |ui| self.top_bar(ui));
        if let Some(error) = &self.error {
            egui::Panel::bottom("error").show(ui, |ui| {
                ui.colored_label(Color32::from_rgb(255, 110, 110), error);
            });
        }
        egui::Panel::left("side")
            .default_size(260.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.side_panel(ui));
            });
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
                if let Some(loaded) = &self.loaded
                    && let Some(t) = loaded.selected
                {
                    self.texture_view.show(
                        ui,
                        &loaded.project,
                        self.generation,
                        t,
                        loaded.variants[t.index()],
                    );
                }
            });
        });
    }
}
