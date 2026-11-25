//! Controls that edit the project: the palette, the layer list, and the style
//! bar for new and picked shapes.

use eframe::egui::{self, Color32, Key, RichText, Sense};
use mint_core::{
    Axes, HalfPoint, Layer, LayerKind, Mirror, Modifier, ModifierKind, PaletteIndex, Radial,
    Raster, Rgba8, Style, Turns, Variant, VariantRef,
};

use crate::editor::Editor;
use crate::icons::{self, Icon};
use crate::previews::{Previews, thumbnail};
use crate::scene;
use crate::texture_view::{self, Paint, Selection, Tool};

/// Most colors an Animal Crossing villager's palette holds.
const AC_PALETTE: usize = 16;

/// What the style bar's layer menu does.
const TO_LAYER: &str = "Move to another layer";

/// Most height the palette's swatches take before they scroll.
const PALETTE_HEIGHT: f32 = 140.0;

/// Size of a layer's preview in its row.
const LAYER_PREVIEW: egui::Vec2 = egui::vec2(40.0, 28.0);

/// The edit's result, or `None` with the reason it was refused in
/// `message`.
fn report<T, E: std::fmt::Display>(
    result: Result<T, E>,
    message: &mut Option<String>,
) -> Option<T> {
    result.map_err(|e| *message = Some(e.to_string())).ok()
}

/// The variant the controls edit and what is picked in it.
pub struct Target<'a> {
    editor: &'a mut Editor,
    variant: VariantRef,
    selection: &'a mut Selection,
    /// Set when an edit is refused, to show the user.
    message: &'a mut Option<String>,
}

impl<'a> Target<'a> {
    /// Drops picks that no longer apply to the variant, as after an undo.
    pub fn new(
        editor: &'a mut Editor,
        variant: VariantRef,
        selection: &'a mut Selection,
        message: &'a mut Option<String>,
    ) -> Self {
        selection.fit(editor.project(), variant);
        Self {
            editor,
            variant,
            selection,
            message,
        }
    }

    pub fn selection(&self) -> &Selection {
        self.selection
    }

    /// Runs `f` on the variant as an undoable step of its own, reporting a
    /// refused edit.
    fn edit<T>(&mut self, f: impl FnOnce(&mut Variant) -> T) -> Option<T> {
        report(self.editor.edit_variant(self.variant, f), self.message)
    }

    fn layers(&self) -> &[Layer] {
        self.editor.project().variant(self.variant).layers()
    }

    /// Picks `layer`, and none of its shapes.
    fn pick_layer(&mut self, layer: usize) {
        *self.selection = Selection {
            layer,
            ..Selection::default()
        };
    }
}

fn swatch(ui: &mut egui::Ui, color: Rgba8, picked: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), Sense::click());
    ui.painter().rect_filled(
        rect,
        2.0,
        Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a),
    );
    if picked {
        ui.painter().rect_stroke(
            rect.expand(1.0),
            2.0,
            egui::Stroke::new(2.0, Color32::WHITE),
            egui::StrokeKind::Outside,
        );
    }
    response
}

pub fn palette(
    ui: &mut egui::Ui,
    editor: &mut Editor,
    paint: &mut Paint,
    message: &mut Option<String>,
) {
    let project = editor.project();
    let colors = project.palette().colors().to_vec();
    if usize::from(paint.color.0) >= colors.len() {
        paint.color = PaletteIndex(0);
    }
    ui.horizontal(|ui| {
        ui.strong("Palette");
        if project.target().id == mint_ac::TARGET {
            let text = format!("{} of {AC_PALETTE} colors", colors.len());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if colors.len() > AC_PALETTE {
                    ui.colored_label(Color32::from_rgb(255, 170, 90), text);
                } else {
                    ui.weak(text);
                }
            });
        }
    });
    // A long palette scrolls, leaving room under it.
    egui::ScrollArea::vertical()
        .id_salt("palette")
        .max_height(PALETTE_HEIGHT)
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (i, &c) in colors.iter().enumerate() {
                    let shown = scene::display_color(project, c);
                    let index = PaletteIndex(i as u8);
                    let r = swatch(ui, shown, paint.color == index).on_hover_text(format!(
                        "Entry {i}: #{:02X}{:02X}{:02X}, alpha {}",
                        c.r, c.g, c.b, c.a
                    ));
                    if r.clicked() {
                        paint.color = index;
                    }
                }
            });
        });
    let Some(&current) = colors.get(usize::from(paint.color.0)) else {
        return;
    };
    ui.horizontal(|ui| {
        let mut rgba = current.to_array();
        let changed = ui.color_edit_button_srgba_unmultiplied(&mut rgba).changed();
        if changed {
            // One undo step while the picker is held; the app ends it.
            let [r, g, b, a] = rgba;
            let color = paint.color;
            report(
                editor.edit_continuous(("palette color", color.0), |p| {
                    p.set_palette_color(color, Rgba8::new(r, g, b, a))
                }),
                message,
            );
        }
        if ui
            .button("Add")
            .on_hover_text("Add a copy of this color")
            .clicked()
        {
            match editor.edit(|p| p.add_palette_color(current)) {
                Ok(i) => paint.color = i,
                Err(e) => *message = Some(e.to_string()),
            }
        }
        ui.menu_button("Merge into", |ui| {
            ui.label("Recolor everything using this entry, then remove it:");
            for (i, &c) in colors.iter().enumerate() {
                let into = PaletteIndex(i as u8);
                if into == paint.color {
                    continue;
                }
                let shown = scene::display_color(editor.project(), c);
                if swatch(ui, shown, false)
                    .on_hover_text(format!("Entry {i}"))
                    .clicked()
                {
                    let from = paint.color;
                    match editor.edit(|p| p.merge_palette_color(from, into)) {
                        Ok(()) => {
                            paint.color = if into > from {
                                PaletteIndex(into.0 - 1)
                            } else {
                                into
                            };
                        }
                        Err(e) => *message = Some(e.to_string()),
                    }
                    ui.close();
                }
            }
        });
    });
}

/// A layer's new name while it is typed, entered into the project when the
/// field loses focus.
pub struct Rename {
    variant: VariantRef,
    layer: usize,
    typed: String,
}

/// An icon that toggles something on its row, dimmed when `dim`.
fn row_toggle(ui: &mut egui::Ui, icon: Icon, dim: bool) -> egui::Response {
    let color = if dim {
        ui.visuals().weak_text_color()
    } else {
        ui.visuals().text_color()
    };
    let text = icons::text(icon, icons::SMALL).color(color);
    ui.add(egui::Button::new(text).frame_when_inactive(false))
}

/// The variant's layers, top layer first, as they stack. Returns what was
/// chosen from a layer's right-click menu, for [`apply_layer_action`].
pub fn layer_list(
    ui: &mut egui::Ui,
    t: &mut Target<'_>,
    previews: &mut Previews,
    rename: &mut Option<Rename>,
) -> Option<LayerAction> {
    let count = t.layers().len();
    // A rename ends with its layer, as when the variant shown changes.
    if rename
        .as_ref()
        .is_some_and(|n| n.variant != t.variant || n.layer >= count)
    {
        *rename = None;
    }
    if count == 0 {
        ui.weak("No layers yet; painting adds one.");
    }
    let mut action = None;
    for i in (0..count).rev() {
        let layer = &t.layers()[i];
        let (visible, locked, kind) = (layer.visible, layer.locked, layer.kind());
        let (name, shapes) = (layer.name.clone(), layer.shapes().len());
        let picked = t.selection.layer == i;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let eye = if visible { Icon::Eye } else { Icon::EyeOff };
            if row_toggle(ui, eye, !visible)
                .on_hover_text("Visible; hidden layers don't export")
                .clicked()
            {
                t.edit(|v| v.layers_mut()[i].visible = !visible);
            }
            let lock = if locked { Icon::Lock } else { Icon::LockOpen };
            if row_toggle(ui, lock, !locked)
                .on_hover_text("Locked layers can't be picked or drawn on")
                .clicked()
            {
                t.edit(|v| v.layers_mut()[i].locked = !locked);
            }
            let preview = |ctx: &egui::Context| {
                let (project, generation) = (t.editor.project(), t.editor.generation());
                previews.layer(ctx, project, generation, t.variant, i)
            };
            let mut rows = vec![thumbnail(ui, LAYER_PREVIEW, picked, preview)];
            if let Some(marker) = modifier_marker(ui, &t.layers()[i]) {
                rows.push(marker);
            }
            if let Some(n) = rename.as_mut().filter(|n| n.layer == i) {
                let field =
                    ui.add(egui::TextEdit::singleline(&mut n.typed).desired_width(f32::INFINITY));
                if field.lost_focus() {
                    // Escape leaves the name as it was.
                    if !ui.input(|i| i.key_pressed(Key::Escape)) {
                        let name = std::mem::take(&mut n.typed);
                        t.edit(|v| v.layers_mut()[i].name = name);
                    }
                    *rename = None;
                } else if !field.has_focus() {
                    field.request_focus();
                }
            } else {
                // A raster layer has pixels rather than a count of shapes.
                let (marker, what) = match kind {
                    LayerKind::Vector => (RichText::new(shapes.to_string()).weak(), "Vector layer"),
                    LayerKind::Raster => (
                        icons::text(Icon::Grid3x3, icons::SMALL).weak(),
                        "Raster layer",
                    ),
                };
                let button = egui::Button::selectable(picked, name.as_str())
                    .truncate()
                    .right_text(marker);
                let size = [ui.available_width(), LAYER_PREVIEW.y];
                let row = ui
                    .add_sized(size, button)
                    .on_hover_text(format!("{name}\n{what}; double-click to rename"));
                if row.double_clicked() {
                    *rename = Some(Rename {
                        variant: t.variant,
                        layer: i,
                        typed: name,
                    });
                }
                rows.push(row);
            }
            // A right-click picks the layer, then opens the menu of what
            // can be done to it.
            let pick = rows.iter().any(|r| r.clicked() || r.secondary_clicked());
            if pick && !picked {
                t.pick_layer(i);
            }
            let mut chosen = None;
            for row in rows {
                row.context_menu(|ui| {
                    if let Some(a) = layer_menu(ui, t, i) {
                        chosen = Some(a);
                        ui.close();
                    }
                });
            }
            // The action is for the row whose menu was open, whatever is
            // picked by now.
            if let Some(a) = chosen {
                if t.selection.layer != i {
                    t.pick_layer(i);
                }
                action = Some(a);
            }
        });
    }
    action
}

/// An icon on a layer's row when it has modifiers, dimmed when they are all
/// off, naming them on hover.
fn modifier_marker(ui: &mut egui::Ui, layer: &Layer) -> Option<egui::Response> {
    if layer.modifiers.is_empty() {
        return None;
    }
    let on = layer.modifiers.iter().any(|m| m.enabled);
    let color = if on {
        ui.visuals().text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    let names: Vec<String> = layer
        .modifiers
        .iter()
        .map(|m| {
            let off = if m.enabled { "" } else { " (off)" };
            format!("{}{off}", modifier_name(m.kind))
        })
        .collect();
    let icon = icons::text(Icon::Wrench, icons::SMALL).color(color);
    let hover = format!("Modifiers: {}", names.join(", "));
    Some(
        ui.add(egui::Label::new(icon).sense(Sense::click()))
            .on_hover_text(hover),
    )
}

/// A modifier's kind and settings in a few words.
fn modifier_name(kind: ModifierKind) -> String {
    match kind {
        ModifierKind::Mirror(m) => match m.axes {
            Axes::X => "Mirror X".into(),
            Axes::Y => "Mirror Y".into(),
            Axes::Both => "Mirror X and Y".into(),
        },
        ModifierKind::Radial(r) => match r.turns() {
            Turns::Half => "Radial 180°".into(),
            Turns::Quarter => "Radial 90°".into(),
        },
    }
}

/// The picked layer's modifiers, first applied at the top, with controls to
/// add, turn on and off, set, apply, reorder, and delete them. A locked
/// layer's modifiers only turn on and off.
pub fn modifier_stack(ui: &mut egui::Ui, t: &mut Target<'_>) {
    let (variant, i) = (t.variant, t.selection.layer);
    let Some(layer) = t.layers().get(i) else {
        return;
    };
    let (locked, kind) = (layer.locked, layer.kind());
    let modifiers = layer.modifiers.clone();
    let size = t.editor.project().variant(variant).size();
    let center = HalfPoint::center(size);
    let why_locked = "The picked layer is locked";
    ui.horizontal(|ui| {
        ui.strong("Modifiers");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let add = ui.add_enabled_ui(!locked, |ui| {
                ui.menu_button("Add", |ui| {
                    let mut added = None;
                    if ui
                        .button("Mirror")
                        .on_hover_text("Reflect the layer across a line")
                        .clicked()
                    {
                        added = Some(ModifierKind::Mirror(Mirror {
                            axes: Axes::X,
                            at: center,
                        }));
                    }
                    if ui
                        .button("Radial")
                        .on_hover_text("Turn copies of the layer about a point")
                        .clicked()
                    {
                        let r = Radial::nearest(Turns::Half, center.to_vec2(), size);
                        added = Some(ModifierKind::Radial(r));
                    }
                    if let Some(kind) = added {
                        t.edit(|v| v.layers_mut()[i].modifiers.push(Modifier::new(kind)));
                        ui.close();
                    }
                })
            });
            add.response.on_disabled_hover_text(why_locked);
        });
    });
    if modifiers.is_empty() {
        ui.weak("None on this layer");
    }
    let count = modifiers.len();
    // Changes to the stack wait until every row's fields have entered their
    // edits, which a click elsewhere does in the same frame, so those edits
    // reach the modifier they were made on.
    let mut restack = None;
    // A long stack scrolls rather than squeezing out the layer list.
    let max_height = ui.ctx().content_rect().height() * 0.35;
    egui::ScrollArea::vertical()
        .max_height(max_height)
        .show(ui, |ui| {
            for (k, m) in modifiers.into_iter().enumerate() {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let eye = if m.enabled { Icon::Eye } else { Icon::EyeOff };
                        if row_toggle(ui, eye, !m.enabled)
                            .on_hover_text("On; a modifier that is off neither shows nor exports")
                            .clicked()
                        {
                            t.edit(|v| v.layers_mut()[i].modifiers[k].enabled = !m.enabled);
                        }
                        let name = match m.kind {
                            ModifierKind::Mirror(_) => "Mirror",
                            ModifierKind::Radial(_) => "Radial",
                        };
                        ui.strong(name);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let more =
                                ui.menu_button(icons::text(Icon::Ellipsis, icons::SMALL), |ui| {
                                    // A menu item, disabled with the reason `refusal`.
                                    let item =
                                        |ui: &mut egui::Ui, refusal: Option<&str>, label: &str| {
                                            let refusal =
                                                if locked { Some(why_locked) } else { refusal };
                                            ui.add_enabled(
                                                refusal.is_none(),
                                                egui::Button::new(label),
                                            )
                                            .on_disabled_hover_text(refusal.unwrap_or_default())
                                            .clicked()
                                        };
                                    let first = (k == 0).then_some("It applies first");
                                    if item(ui, first, "Apply sooner") {
                                        restack = Some(Restack::Swap(k - 1));
                                    }
                                    let last = (k + 1 == count).then_some("It applies last");
                                    if item(ui, last, "Apply later") {
                                        restack = Some(Restack::Swap(k));
                                    }
                                    ui.separator();
                                    if item(ui, None, "Delete") {
                                        restack = Some(Restack::Delete(k));
                                    }
                                });
                            more.response.on_hover_text("Reorder or delete");
                            let refusal = if locked {
                                Some(why_locked)
                            } else if k > 0 {
                                Some("Apply the modifiers above it first")
                            } else if !m.enabled {
                                Some("Turn it on to apply it")
                            } else {
                                None
                            };
                            let hover = match kind {
                                LayerKind::Vector => {
                                    "Write its copies into the layer as shapes and remove \
                                     it; whole shapes can show parts the copies cut off"
                                }
                                LayerKind::Raster => {
                                    "Write its copies into the layer and remove it"
                                }
                            };
                            let apply = ui
                                .add_enabled(refusal.is_none(), egui::Button::new("Apply"))
                                .on_hover_text(hover)
                                .on_disabled_hover_text(refusal.unwrap_or_default());
                            if apply.clicked() {
                                restack = Some(Restack::Apply);
                            }
                        });
                    });
                    ui.add_enabled_ui(!locked, |ui| {
                        modifier_settings(ui, t, k, m.kind, size);
                    })
                    .response
                    .on_disabled_hover_text(why_locked);
                });
            }
        });
    match restack {
        Some(Restack::Apply) => apply_first_modifier(t),
        Some(Restack::Swap(k)) => {
            t.edit(|v| v.layers_mut()[i].modifiers.swap(k, k + 1));
        }
        Some(Restack::Delete(k)) => {
            t.edit(|v| v.layers_mut()[i].modifiers.remove(k));
        }
        None => {}
    }
}

/// A change to the picked layer's modifier stack that moves modifiers to
/// other indices.
enum Restack {
    /// Apply the first.
    Apply,
    /// Swap modifier `k` with the next.
    Swap(usize),
    /// Delete modifier `k`.
    Delete(usize),
}

/// Applies the picked layer's first modifier, keeping its picked shapes
/// picked where the copies push them up the layer.
fn apply_first_modifier(t: &mut Target<'_>) {
    let i = t.selection.layer;
    let before = t.layers()[i].shapes().len();
    if t.edit(|v| v.layers_mut()[i].apply_first_modifier())
        .is_some()
    {
        let copies = t.layers()[i].shapes().len() - before;
        for s in &mut t.selection.shapes {
            *s += copies;
        }
    }
}

/// The settings of modifier `k` of the picked layer, which is of `kind`.
fn modifier_settings(
    ui: &mut egui::Ui,
    t: &mut Target<'_>,
    k: usize,
    kind: ModifierKind,
    size: glam::UVec2,
) {
    let (variant, i) = (t.variant, t.selection.layer);
    let set = |t: &mut Target<'_>, kind: ModifierKind, continuous: Option<&str>| {
        let edit = |v: &mut Variant| v.layers_mut()[i].modifiers[k].kind = kind;
        let result = match continuous {
            Some(what) => {
                t.editor
                    .edit_variant_continuous(("modifier", what, variant, i, k), variant, edit)
            }
            None => t.editor.edit_variant(variant, edit),
        };
        report(result, t.message);
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        // Narrow fields, so the row fits the narrowest dock.
        ui.spacing_mut().interact_size.x = 36.0;
        if let ModifierKind::Radial(r) = kind {
            for (turns, label, hover) in [
                (Turns::Half, "180°", "One copy, half a turn around"),
                (Turns::Quarter, "90°", "Three copies, a quarter turn apart"),
            ] {
                if ui
                    .selectable_label(r.turns() == turns, label)
                    .on_hover_text(hover)
                    .clicked()
                    && r.turns() != turns
                {
                    let r = Radial::nearest(turns, r.center().to_vec2(), size);
                    set(t, ModifierKind::Radial(r), None);
                }
            }
        }
        let (at, lines) = kind.guides();
        let at = at.to_vec2();
        for (axis, label) in [(0, "X"), (1, "Y")] {
            if let ModifierKind::Mirror(m) = kind {
                let mut checked = lines[axis];
                let mut flags = lines;
                flags[axis] = !checked;
                // The last line stays; deleting the modifier removes it.
                let other = Axes::from_flags(flags[0], flags[1]);
                let hover = if axis == 0 {
                    "Reflect left and right across a vertical line"
                } else {
                    "Reflect top and bottom across a horizontal line"
                };
                let box_ = ui
                    .add_enabled(other.is_some(), egui::Checkbox::new(&mut checked, label))
                    .on_hover_text(hover)
                    .on_disabled_hover_text("A mirror needs a line; delete it to remove it");
                if box_.changed()
                    && let Some(axes) = other
                {
                    set(t, ModifierKind::Mirror(Mirror { axes, ..m }), None);
                }
            }
            let hover = if axis == 0 {
                "The vertical line, in texels from the left"
            } else {
                "The horizontal line, in texels from the top"
            };
            let mut v = at[axis];
            let field = position(&mut v, size[axis], f64::from(kind.step()));
            if ui
                .add_enabled(lines[axis], field)
                .on_hover_text(hover)
                .changed()
            {
                set(t, kind.with_coordinate(axis, v, size), Some(label));
            }
        }
        if ui
            .button(icons::text(Icon::LocateFixed, icons::SMALL))
            .on_hover_text("Move to the texture's center")
            .clicked()
        {
            set(t, kind.centered(size), None);
        }
    });
}

/// A field for a guide line's position in texels, up to `max`, stepping by
/// `step` texels (as the arrow keys do); the edit moves it to the nearest
/// place the modifier allows.
fn position(value: &mut f32, max: u32, step: f64) -> egui::DragValue<'_> {
    egui::DragValue::new(value)
        .range(0.0..=max as f32)
        .speed(step)
        .fixed_decimals(1)
        // A typed value snaps once it is entered, not at each keystroke.
        .update_while_editing(false)
}

/// The options for layer `i` in its right-click menu.
fn layer_menu(ui: &mut egui::Ui, t: &Target<'_>, i: usize) -> Option<LayerAction> {
    let layer = t.layers().get(i)?;
    let mut action = None;
    if ui.button("Rename").clicked() {
        action = Some(LayerAction::Rename);
    }
    if ui.button("Duplicate").clicked() {
        action = Some(LayerAction::Duplicate);
    }
    // Other variants of the same texture to copy the layer onto.
    let texture = t.editor.project().texture(t.variant.texture);
    let others = || (0..texture.variants().len()).filter(|&k| k != t.variant.variant);
    if others().next().is_some() {
        ui.menu_button("Copy to", |ui| {
            if ui.button("All other variants").clicked() {
                action = Some(LayerAction::CopyTo(others().collect()));
            }
            for k in others() {
                let name = &texture.variants()[k].name;
                if ui.button(format!("Variant {name}")).clicked() {
                    action = Some(LayerAction::CopyTo(vec![k]));
                }
            }
        });
    }
    let locked = "The layer is locked";
    if layer.kind() == LayerKind::Vector
        && ui
            .add_enabled(!layer.locked, egui::Button::new("Rasterize"))
            .on_hover_text("Turn the shapes into pixels; only undo turns them back")
            .on_disabled_hover_text(locked)
            .clicked()
    {
        action = Some(LayerAction::Rasterize);
    }
    ui.separator();
    if ui
        .add_enabled(!layer.locked, egui::Button::new("Delete"))
        .on_disabled_hover_text(locked)
        .clicked()
    {
        action = Some(LayerAction::Delete);
    }
    action
}

/// A button holding only an icon, named by `hover` whether or not it is
/// enabled.
fn icon_button(ui: &mut egui::Ui, icon: Icon, enabled: bool, hover: &str) -> egui::Response {
    let button = egui::Button::new(icons::text(icon, icons::SMALL));
    ui.add_enabled(enabled, button)
        .on_hover_text(hover)
        .on_disabled_hover_text(hover)
}

/// The picked layer's opacity in the views.
pub fn layer_opacity(ui: &mut egui::Ui, t: &mut Target<'_>) {
    let (variant, i) = (t.variant, t.selection.layer);
    let Some(layer) = t.layers().get(i) else {
        return;
    };
    let mut opacity = layer.opacity;
    ui.horizontal(|ui| {
        ui.label("Opacity");
        ui.spacing_mut().slider_width = ui.available_width();
        let slider = ui.add(
            egui::Slider::new(&mut opacity, 0.0..=1.0)
                .show_value(false)
                .clamping(egui::SliderClamping::Always),
        );
        if slider.changed() {
            let edit =
                t.editor
                    .edit_variant_continuous(("layer opacity", variant, i), variant, |v| {
                        v.layers_mut()[i].opacity = opacity;
                    });
            report(edit, t.message);
        }
        slider.on_hover_text("The picked layer's opacity in the views; export ignores it");
    });
}

/// What a click among the layer actions or in a layer's menu asks for, done
/// to the picked layer.
pub enum LayerAction {
    New,
    NewRaster,
    Rename,
    Rasterize,
    Delete,
    Up,
    Down,
    Duplicate,
    /// Copy onto the top of these other variants of the texture.
    CopyTo(Vec<usize>),
}

/// The buttons that add, remove, and reorder layers. Returns what was
/// clicked, for [`apply_layer_action`] once the layer list has run, so a
/// rename the click ended is entered before layers change places.
pub fn layer_actions(ui: &mut egui::Ui, t: &Target<'_>) -> Option<LayerAction> {
    let count = t.layers().len();
    let i = t.selection.layer;
    let mut action = None;
    ui.horizontal(|ui| {
        if icon_button(ui, Icon::Plus, true, "New vector layer").clicked() {
            action = Some(LayerAction::New);
        }
        if icon_button(ui, Icon::ImagePlus, true, "New raster layer").clicked() {
            action = Some(LayerAction::NewRaster);
        }
        let Some(layer) = t.layers().get(i) else {
            return;
        };
        let delete = if layer.locked {
            "The picked layer is locked"
        } else {
            "Delete the picked layer"
        };
        if icon_button(ui, Icon::Trash2, !layer.locked, delete).clicked() {
            action = Some(LayerAction::Delete);
        }
        if icon_button(ui, Icon::ArrowUp, i + 1 < count, "Move the picked layer up").clicked() {
            action = Some(LayerAction::Up);
        }
        if icon_button(ui, Icon::ArrowDown, i > 0, "Move the picked layer down").clicked() {
            action = Some(LayerAction::Down);
        }
    });
    action
}

/// Does `action` to the picked layer, and picks the layer to act on next:
/// the new, moved, or duplicated layer, or the one under a deleted layer.
pub fn apply_layer_action(t: &mut Target<'_>, rename: &mut Option<Rename>, action: LayerAction) {
    let count = t.layers().len();
    let i = t.selection.layer;
    match action {
        LayerAction::New | LayerAction::NewRaster => {
            let at = if count == 0 { 0 } else { i + 1 };
            let name = format!("Layer {}", count + 1);
            let added = t.edit(|v| {
                let layer = match action {
                    LayerAction::NewRaster => Layer::raster(name, Raster::clear(v.size())),
                    _ => Layer::new(name),
                };
                v.insert_layer(at, layer);
            });
            if added.is_some() {
                t.pick_layer(at);
            }
        }
        // The rest need a picked layer.
        _ if i >= count => {}
        LayerAction::Rename => {
            *rename = Some(Rename {
                variant: t.variant,
                layer: i,
                typed: t.layers()[i].name.clone(),
            });
        }
        // A locked layer stays as it is, whatever was picked when the
        // action was chosen.
        LayerAction::Rasterize | LayerAction::Delete if t.layers()[i].locked => {}
        LayerAction::Rasterize => {
            t.edit(|v| v.rasterize_layer(i));
            t.pick_layer(i);
        }
        LayerAction::Delete => {
            let removed = t.edit(|v| {
                v.remove_layer(i);
            });
            if removed.is_some() {
                t.pick_layer(i.saturating_sub(1));
            }
        }
        LayerAction::Up if i + 1 < count => {
            t.edit(|v| v.move_layer(i, i + 1));
            t.selection.layer = i + 1;
        }
        LayerAction::Down if i > 0 => {
            t.edit(|v| v.move_layer(i, i - 1));
            t.selection.layer = i - 1;
        }
        LayerAction::Up | LayerAction::Down => {}
        LayerAction::Duplicate => {
            t.edit(|v| {
                let mut copy = v.layers()[i].clone();
                copy.name = format!("{} copy", copy.name);
                v.insert_layer(i + 1, copy);
            });
            t.pick_layer(i + 1);
        }
        LayerAction::CopyTo(variants) => {
            let texture = t.variant.texture;
            let layer = t.layers()[i].clone();
            let result = t.editor.edit(|p| {
                for variant in variants {
                    let to = VariantRef { texture, variant };
                    p.edit_variant(to, |v| v.insert_layer(v.layers().len(), layer.clone()))?;
                }
                Ok::<_, mint_core::EditError>(())
            });
            report(result, t.message);
        }
    }
}

/// The fill-or-stroke choice, returning the other one when it is clicked.
fn fill_or_stroke(ui: &mut egui::Ui, fill: bool) -> Option<bool> {
    let mut choice = None;
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 1.0;
        if ui.selectable_label(fill, "Fill").clicked() && !fill {
            choice = Some(true);
        }
        if ui.selectable_label(!fill, "Stroke").clicked() && fill {
            choice = Some(false);
        }
    });
    choice
}

/// Says whose style the bar shows, at one width so the controls after it
/// stay where they are when it changes.
fn bar_label(ui: &mut egui::Ui, text: &str, hover: &str) {
    let size = [46.0, ui.spacing().interact_size.y];
    ui.add_sized(size, egui::Label::new(RichText::new(text).weak()))
        .on_hover_text(hover);
}

fn stroke_width(value: &mut f32) -> egui::DragValue<'_> {
    egui::DragValue::new(value)
        .range(0.1..=64.0)
        .speed(0.05)
        .suffix(" px")
}

/// With the Select tool and shapes picked, their style and what else can be
/// done to them; otherwise the style new shapes are painted in. A drawing
/// tool picks each shape it adds, and still shows the style of the next.
pub fn style_bar(ui: &mut egui::Ui, t: &mut Target<'_>, paint: &mut Paint, tool: Tool) {
    let layer = t.selection.layer;
    let picked = if tool == Tool::Select {
        t.selection.shapes.clone()
    } else {
        Vec::new()
    };
    if matches!(tool, Tool::Pencil | Tool::Eraser) {
        bar_label(ui, "Tip", "The pencil and eraser tip");
        ui.add(stroke_width(&mut paint.width))
            .on_hover_text("Tip width, rounded to whole pixels");
        return;
    }
    if tool == Tool::Fill {
        bar_label(ui, "Fill", "The fill tool");
        ui.weak("Fills the joined pixels of one color, on the picked layer only");
        return;
    }
    let Some(&first) = picked.first() else {
        bar_label(ui, "New", "The style new shapes are painted in");
        if let Some(fill) = fill_or_stroke(ui, paint.fill) {
            paint.fill = fill;
        }
        ui.add(stroke_width(&mut paint.width))
            .on_hover_text("Stroke width, for strokes and the brush");
        return;
    };
    let count = match picked.len() {
        1 => "1 picked shape".to_owned(),
        n => format!("{n} picked shapes"),
    };
    bar_label(ui, "Picked", &format!("The style of the {count}"));
    // The first picked shape shows the values; an edit applies to each
    // picked shape, keeping its other settings.
    let style = t.layers()[layer].shapes()[first].style();
    if let Some(fill) = fill_or_stroke(ui, style == Style::Fill) {
        // Strokes keep their width, and fills become closed strokes.
        let width = paint.width;
        set_style(t, None, &picked, |s| match (fill, s) {
            (true, _) => Style::Fill,
            (false, Style::Fill) => Style::Stroke {
                width,
                closed: true,
            },
            (false, stroke) => stroke,
        });
    }
    if let Style::Stroke { width, closed } = style {
        let mut w = width;
        if ui
            .add(stroke_width(&mut w).clamp_existing_to_range(false))
            .changed()
        {
            set_style(t, Some("stroke width"), &picked, |s| match s {
                Style::Stroke { closed, .. } => Style::Stroke { width: w, closed },
                Style::Fill => Style::Fill,
            });
        }
        let mut c = closed;
        if ui.checkbox(&mut c, "Closed").changed() {
            set_style(t, None, &picked, |s| match s {
                Style::Stroke { width, .. } => Style::Stroke { width, closed: c },
                Style::Fill => Style::Fill,
            });
        }
    }
    ui.separator();
    if icon_button(
        ui,
        Icon::PaintBucket,
        true,
        "Recolor with the picked palette entry",
    )
    .clicked()
    {
        let color = paint.color;
        t.edit(|v| {
            for &s in &picked {
                v.layers_mut()[layer].shapes_mut()[s].set_color(color);
            }
        });
    }
    if icon_button(ui, Icon::Copy, true, "Duplicate").clicked() {
        duplicate(t);
    }
    let count = t.layers()[layer].shapes().len();
    let mut sorted = picked.clone();
    sorted.sort_unstable();
    let can_raise = sorted.last().is_some_and(|&s| s + 1 < count);
    if icon_button(
        ui,
        Icon::ArrowUp,
        can_raise,
        "Forward: draw over the next shape up",
    )
    .clicked()
    {
        t.edit(|v| {
            for &s in sorted.iter().rev() {
                v.layers_mut()[layer].move_shape(s, s + 1);
            }
        });
        t.selection.shapes = picked.iter().map(|s| s + 1).collect();
    }
    let can_lower = sorted.first().is_some_and(|&s| s > 0);
    if icon_button(
        ui,
        Icon::ArrowDown,
        can_lower,
        "Backward: draw under the next shape down",
    )
    .clicked()
    {
        t.edit(|v| {
            for &s in &sorted {
                v.layers_mut()[layer].move_shape(s, s - 1);
            }
        });
        t.selection.shapes = picked.iter().map(|s| s - 1).collect();
    }
    // There is nowhere to move them with only their own layer.
    ui.add_enabled_ui(t.layers().len() > 1, |ui| {
        let to_layer = ui.menu_button(icons::text(Icon::Layers, icons::SMALL), |ui| {
            // Shapes go only onto unlocked vector layers.
            let layers: Vec<(String, bool)> = t
                .layers()
                .iter()
                .map(|l| (l.name.clone(), !l.locked && l.kind() == LayerKind::Vector))
                .collect();
            for (k, (name, open)) in layers.iter().enumerate() {
                if k == layer || !ui.add_enabled(*open, egui::Button::new(name)).clicked() {
                    continue;
                }
                let moved = t.edit(|v| {
                    let mut shapes = Vec::new();
                    for &s in sorted.iter().rev() {
                        shapes.push(v.layers_mut()[layer].remove_shape(s));
                    }
                    let to = &mut v.layers_mut()[k];
                    let start = to.shapes().len();
                    for s in shapes.into_iter().rev() {
                        to.insert_shape(to.shapes().len(), s);
                    }
                    start
                });
                if let Some(start) = moved {
                    *t.selection = Selection {
                        layer: k,
                        shapes: (start..start + sorted.len()).collect(),
                        anchor: None,
                    };
                }
                ui.close();
            }
        });
        to_layer
            .response
            .on_hover_text(TO_LAYER)
            .on_disabled_hover_text(TO_LAYER);
    });
}

/// Restyles each of the `picked` shapes with `restyle` of its own style, as
/// part of the continuous edit `key` if any.
fn set_style(
    t: &mut Target<'_>,
    key: Option<&str>,
    picked: &[usize],
    restyle: impl Fn(Style) -> Style,
) {
    let (r, layer) = (t.variant, t.selection.layer);
    let apply = |p: &mut mint_core::Project| {
        let mut err = None;
        p.edit_variant(r, |v| {
            for &s in picked {
                let shape = &mut v.layers_mut()[layer].shapes_mut()[s];
                if let Err(e) = shape.set_style(restyle(shape.style())) {
                    err = Some(e);
                }
            }
        })
        .map_err(|e| e.to_string())?;
        err.map_or(Ok(()), |e| Err(e.to_string()))
    };
    let result = match key {
        Some(key) => t.editor.edit_continuous((key, r, layer), apply),
        None => t.editor.edit(apply),
    };
    report(result, t.message);
}

/// Deletes the picked anchor, or else the picked shapes.
pub fn delete(t: &mut Target<'_>) {
    texture_view::delete_selected(t.editor, t.variant, t.selection);
}

/// Copies the picked shapes on top of their layer and picks the copies.
pub fn duplicate(t: &mut Target<'_>) {
    let layer = t.selection.layer;
    let mut picked = t.selection.shapes.clone();
    picked.sort_unstable();
    if picked.is_empty() || t.layers()[layer].locked {
        return;
    }
    let start = t.edit(|v| {
        let l = &mut v.layers_mut()[layer];
        let copies: Vec<_> = picked.iter().map(|&s| l.shapes()[s].clone()).collect();
        let start = l.shapes().len();
        for s in copies {
            l.insert_shape(l.shapes().len(), s);
        }
        start
    });
    if let Some(start) = start {
        t.selection.shapes = (start..start + picked.len()).collect();
        t.selection.anchor = None;
    }
}
