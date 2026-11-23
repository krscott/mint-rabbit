//! The import picker: a window of villager tiles, each with a rendered
//! thumbnail, the villager's in-game name, and its texture set, grouped by
//! species and filtered by a search. It holds the open decomp.

use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};

use eframe::egui::{self, Color32, ColorImage, Sense, TextureHandle, TextureOptions, vec2};
use eframe::egui_wgpu::wgpu;
use glam::Vec3;
use mint_ac::{Decomp, ImportError, VillagerRef};
use mint_core::Project;

use crate::camera::Orbit;
use crate::render::{Offscreen, SceneData};

/// Pixel size of rendered thumbnails.
const THUMB_PIXELS: u32 = 128;
/// Size a thumbnail shows at.
const THUMB_SIZE: f32 = 96.0;
const TILE_WIDTH: f32 = THUMB_SIZE + 16.0;
const TILE_HEIGHT: f32 = THUMB_SIZE + 44.0;

/// A table entry as the picker shows it.
struct Entry {
    villager: VillagerRef,
    /// The in-game name, or the texture set name when names are unknown.
    title: String,
    /// The texture set, or the model and table position when the title is
    /// the texture set (which some entries share), and whether it is an
    /// islander.
    detail: String,
    /// Species and detail.
    hover: String,
    /// Lowercase text a search matches against.
    haystack: String,
}

enum Thumb {
    Pending,
    /// Imported, with a thumbnail unless rendering failed.
    Ready(Option<TextureHandle>),
    /// Why importing the entry failed.
    Failed(String),
}

/// GPU handles to render thumbnails with.
pub type Gpu = (wgpu::Device, wgpu::Queue);

/// Progress of the thumbnail thread.
enum Rendering {
    NotStarted,
    Running(Receiver<Message>),
    Done,
}

/// What the thumbnail thread reports.
enum Message {
    /// An entry, by position, imported (with its thumbnail when it
    /// rendered) or failed to import.
    Thumb(usize, Result<Option<ColorImage>, String>),
    /// Why thumbnails stopped rendering.
    NoRender(String),
}

pub struct Picker {
    decomp: Decomp,
    gpu: Option<Gpu>,
    open: bool,
    /// Focus the search on the next frame shown.
    focus_search: bool,
    search: String,
    /// Villagers grouped by species, in species order.
    groups: Vec<(&'static str, Vec<usize>)>,
    /// Special characters and test entries.
    others: Vec<usize>,
    entries: Vec<Entry>,
    thumbs: Vec<Thumb>,
    /// Why names, islanders, or thumbnails are missing, shown above the
    /// tiles.
    notes: Vec<String>,
    rendering: Rendering,
}

impl Picker {
    /// Lists the villager table of `decomp`, naming entries from its disc
    /// image when it can be read. Thumbnails render with `gpu` once the
    /// picker first opens.
    pub fn new(decomp: Decomp, gpu: Option<Gpu>) -> Result<Self, ImportError> {
        let villagers = decomp.villagers()?;
        let mut notes = Vec::new();
        let names = decomp.villager_names().unwrap_or_else(|e| {
            notes.push(format!("No in-game names: {e}"));
            Vec::new()
        });
        let islanders = decomp.islanders().unwrap_or_else(|e| {
            notes.push(format!("Islanders unknown: {e}"));
            Vec::new()
        });
        let entries: Vec<Entry> = villagers
            .into_iter()
            .filter(|v| !v.is_unused())
            .map(|v| {
                let name = v.is_villager().then(|| names.get(v.index)).flatten();
                let islander = v.is_villager() && islanders.get(v.index) == Some(&true);
                let mut detail = if name.is_some() {
                    v.texture_set.clone()
                } else {
                    format!("{} · entry {}", v.model, v.index)
                };
                if islander {
                    detail.push_str(" · Islander");
                }
                let hover = v
                    .species()
                    .map_or_else(|| detail.clone(), |s| format!("{s} · {detail}"));
                let title = name.cloned().unwrap_or_else(|| v.texture_set.clone());
                let haystack = format!("{title} {hover} {}", v.model).to_lowercase();
                Entry {
                    villager: v,
                    title,
                    detail,
                    hover,
                    haystack,
                }
            })
            .collect();
        let mut groups: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        let mut others = Vec::new();
        for (i, e) in entries.iter().enumerate() {
            if e.villager.is_villager() {
                let species = e.villager.species().unwrap_or("Unknown species");
                groups.entry(species).or_default().push(i);
            } else {
                others.push(i);
            }
        }
        Ok(Self {
            decomp,
            gpu,
            open: false,
            focus_search: false,
            search: String::new(),
            groups: groups.into_iter().collect(),
            others,
            thumbs: entries.iter().map(|_| Thumb::Pending).collect(),
            entries,
            notes,
            rendering: Rendering::NotStarted,
        })
    }

    pub fn decomp(&self) -> &Decomp {
        &self.decomp
    }

    pub fn open(&mut self) {
        self.open = true;
        self.focus_search = true;
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    /// Lists the decomp's villagers again, so thumbnails show what it holds
    /// now, keeping the window and search as they are.
    pub fn reload(&mut self) -> Result<(), ImportError> {
        let mut new = Self::new(self.decomp.clone(), self.gpu.clone())?;
        new.open = self.open;
        new.search = std::mem::take(&mut self.search);
        *self = new;
        Ok(())
    }

    /// Entries in the order shown: villagers by species, then the others.
    fn shown_order(&self) -> impl Iterator<Item = usize> + '_ {
        self.groups
            .iter()
            .flat_map(|(_, list)| list)
            .chain(&self.others)
            .copied()
    }

    /// Starts rendering thumbnails, in the order shown, on a background
    /// thread. Dropping the picker stops it.
    fn start_rendering(&mut self, ctx: &egui::Context) {
        let (sender, receiver) = channel();
        let order: Vec<(usize, VillagerRef)> = self
            .shown_order()
            .map(|i| (i, self.entries[i].villager.clone()))
            .collect();
        let decomp = self.decomp.clone();
        let gpu = self.gpu.clone();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("thumbnails".into())
            .spawn(move || render_thumbnails(&decomp, &order, gpu, &sender, &ctx));
        match spawned {
            Ok(_) => self.rendering = Rendering::Running(receiver),
            Err(e) => self.stop_rendering(&format!("couldn't start rendering: {e}")),
        }
    }

    /// Ends rendering early: entries still pending stay importable, without
    /// a thumbnail.
    fn stop_rendering(&mut self, why: &str) {
        for t in &mut self.thumbs {
            if matches!(t, Thumb::Pending) {
                *t = Thumb::Ready(None);
            }
        }
        self.notes.push(format!("No more thumbnails: {why}"));
        self.rendering = Rendering::Done;
    }

    fn receive(&mut self, ctx: &egui::Context) {
        let Rendering::Running(receiver) = &self.rendering else {
            return;
        };
        loop {
            let message = match receiver.try_recv() {
                Ok(m) => m,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    // The thread reports every entry before it ends.
                    if self.thumbs.iter().any(|t| matches!(t, Thumb::Pending)) {
                        self.stop_rendering("the thumbnail thread stopped");
                    }
                    self.rendering = Rendering::Done;
                    return;
                }
            };
            match message {
                Message::Thumb(i, Ok(image)) => {
                    let texture = image.map(|image| {
                        ctx.load_texture(format!("thumbnail {i}"), image, TextureOptions::LINEAR)
                    });
                    self.thumbs[i] = Thumb::Ready(texture);
                }
                Message::Thumb(i, Err(e)) => self.thumbs[i] = Thumb::Failed(e),
                Message::NoRender(e) => self.notes.push(format!("No thumbnails: {e}")),
            }
        }
    }

    /// Shows the window while it is open, and returns the entry clicked.
    /// A click leaves the window open; the caller closes it once the entry
    /// imports.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<VillagerRef> {
        if !self.open {
            return None;
        }
        if matches!(self.rendering, Rendering::NotStarted) {
            self.start_rendering(ctx);
        }
        self.receive(ctx);
        let mut picked = None;
        let mut open = self.open;
        egui::Window::new("Import villager")
            .open(&mut open)
            .default_size([760.0, 560.0])
            .show(ctx, |ui| {
                let searched = ui.horizontal(|ui| {
                    ui.label("Search");
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut self.search)
                            .hint_text("name, species, or texture set")
                            .desired_width(240.0),
                    );
                    if std::mem::take(&mut self.focus_search) {
                        field.request_focus();
                    }
                    field.changed()
                });
                for note in &self.notes {
                    ui.colored_label(Color32::from_rgb(255, 170, 90), note);
                }
                ui.separator();
                let search = self.search.trim().to_lowercase();
                let matches = |i: &usize| self.entries[*i].haystack.contains(&search);
                let mut results = egui::ScrollArea::vertical().auto_shrink(false);
                // New results start from their top.
                if searched.inner {
                    results = results.vertical_scroll_offset(0.0);
                }
                results.show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let mut any = false;
                    for (species, list) in &self.groups {
                        let shown: Vec<usize> = list.iter().copied().filter(matches).collect();
                        if shown.is_empty() {
                            continue;
                        }
                        any = true;
                        ui.strong(*species);
                        if let Some(v) = self.tiles(ui, &shown) {
                            picked = Some(v);
                        }
                        ui.add_space(6.0);
                    }
                    let others: Vec<usize> = self.others.iter().copied().filter(matches).collect();
                    if !others.is_empty() {
                        any = true;
                        let title = format!("Other entries ({})", others.len());
                        let mut show = |ui: &mut egui::Ui| {
                            ui.label("Special characters and test entries.");
                            if let Some(v) = self.tiles(ui, &others) {
                                picked = Some(v);
                            }
                        };
                        // Search results don't hide behind a collapsed header.
                        if search.is_empty() {
                            egui::CollapsingHeader::new(title)
                                .id_salt("other entries")
                                .show(ui, show);
                        } else {
                            ui.strong(title);
                            show(ui);
                        }
                    }
                    if !any {
                        ui.label("Nothing matches the search.");
                    }
                });
            });
        self.open = open;
        picked
    }

    /// Lays out tiles for `entries`, and returns the one clicked.
    fn tiles(&self, ui: &mut egui::Ui, entries: &[usize]) -> Option<VillagerRef> {
        let mut picked = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            for &i in entries {
                let entry = &self.entries[i];
                let thumb = &self.thumbs[i];
                let failed = matches!(thumb, Thumb::Failed(_));
                let (rect, response) =
                    ui.allocate_exact_size(vec2(TILE_WIDTH, TILE_HEIGHT), Sense::click());
                let response = match thumb {
                    Thumb::Failed(e) => response.on_hover_text(format!("Failed to import: {e}")),
                    _ => response.on_hover_text(&entry.hover),
                };
                if !ui.is_rect_visible(rect) {
                    continue;
                }
                let visuals = ui.style().interact(&response);
                // Long text stays within its tile.
                let painter = ui.painter_at(rect);
                if response.hovered() {
                    painter.rect_filled(rect, 4.0, visuals.bg_fill);
                }
                let image_rect = egui::Rect::from_min_size(
                    rect.min + vec2((TILE_WIDTH - THUMB_SIZE) * 0.5, 4.0),
                    vec2(THUMB_SIZE, THUMB_SIZE),
                );
                match thumb {
                    Thumb::Ready(Some(texture)) => {
                        painter.image(
                            texture.id(),
                            image_rect,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    Thumb::Pending | Thumb::Ready(None) | Thumb::Failed(_) => {
                        painter.rect_filled(image_rect, 4.0, ui.visuals().faint_bg_color);
                        let mark = match thumb {
                            Thumb::Pending => "…",
                            Thumb::Failed(_) => "×",
                            Thumb::Ready(_) => "",
                        };
                        painter.text(
                            image_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            mark,
                            egui::FontId::proportional(20.0),
                            ui.visuals().weak_text_color(),
                        );
                    }
                }
                let text_color = if failed {
                    ui.visuals().weak_text_color()
                } else {
                    visuals.text_color()
                };
                let line = |y: f32, text: &str, size: f32, color: Color32| {
                    let galley = painter.layout_no_wrap(
                        text.to_owned(),
                        egui::FontId::proportional(size),
                        color,
                    );
                    let x = rect.center().x - galley.size().x * 0.5;
                    painter.galley(egui::pos2(x, rect.min.y + y), galley, color);
                };
                line(THUMB_SIZE + 8.0, &entry.title, 14.0, text_color);
                line(
                    THUMB_SIZE + 26.0,
                    &entry.detail,
                    11.0,
                    ui.visuals().weak_text_color(),
                );
                // A failed entry may import now, and says why if not.
                if response.clicked() {
                    picked = Some(entry.villager.clone());
                }
            }
        });
        picked
    }
}

/// Imports villager `v` from `decomp`, reporting a panic in the importer
/// as an error instead of ending the app.
pub fn import(decomp: &Decomp, v: &VillagerRef) -> Result<Project, String> {
    catching("importing", || decomp.import(v))?.map_err(|e| e.to_string())
}

/// Runs `f`, turning a panic into an error that says what was `doing`.
fn catching<T>(doing: &str, f: impl FnOnce() -> T) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(f)).map_err(|panic| {
        let why = panic
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown error".into());
        format!("{doing} crashed: {why}")
    })
}

/// Imports and renders each entry in `order`, reporting each one until the
/// picker is dropped. After rendering fails, it only imports.
fn render_thumbnails(
    decomp: &Decomp,
    order: &[(usize, VillagerRef)],
    gpu: Option<Gpu>,
    sender: &Sender<Message>,
    ctx: &egui::Context,
) {
    let send = |message| {
        let sent = sender.send(message).is_ok();
        ctx.request_repaint();
        sent
    };
    let started = gpu
        .ok_or_else(|| "no GPU to render with".to_owned())
        .and_then(|(device, queue)| {
            catching("rendering", || Offscreen::new(device, queue, THUMB_PIXELS))
        });
    let mut offscreen = started.map_err(|e| send(Message::NoRender(e))).ok();
    for (i, villager) in order {
        let result = import(decomp, villager).map(|project| {
            let o = offscreen.as_mut()?;
            let rendered = catching("rendering", || {
                let scene = Arc::new(SceneData::new(
                    &project,
                    &vec![0; project.textures().len()],
                    0,
                ));
                let mesh = &scene.mesh;
                let points = mesh.vertices.iter().map(|v| Vec3::from(v.position));
                let camera = Orbit::portrait(mesh.bounds_min, mesh.bounds_max, points);
                o.render(scene.clone(), camera.view_proj(1.0))
            })
            .and_then(|r| r);
            match rendered {
                Ok(pixels) => {
                    let size = THUMB_PIXELS as usize;
                    Some(ColorImage::from_rgba_unmultiplied([size, size], &pixels))
                }
                Err(e) => {
                    offscreen = None;
                    send(Message::NoRender(e));
                    None
                }
            }
        });
        if !send(Message::Thumb(*i, result)) {
            return;
        }
    }
}
