//! Side-panel sections that edit the project: palette, layers, and the
//! picked shapes.

use eframe::egui::{self, Color32, Sense};
use mint_core::{Layer, PaletteIndex, Rgba8, Style, VariantRef};

use crate::editor::Editor;
use crate::scene;
use crate::texture_view::{Paint, Selection};

/// Most colors an Animal Crossing villager's palette holds.
const AC_PALETTE: usize = 16;

/// The edit's result, or `None` with the reason it was refused in
/// `message`.
fn report<T, E: std::fmt::Display>(
    result: Result<T, E>,
    message: &mut Option<String>,
) -> Option<T> {
    result.map_err(|e| *message = Some(e.to_string())).ok()
}

/// Runs `f` on a variant as an undoable step of its own, reporting a
/// refused edit.
fn edit_variant<T>(
    editor: &mut Editor,
    r: VariantRef,
    message: &mut Option<String>,
    f: impl FnOnce(&mut mint_core::Variant) -> T,
) -> Option<T> {
    report(editor.edit_variant(r, f), message)
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
    let ac = project.target().id == mint_ac::TARGET;
    if ac {
        let text = format!("{} of {AC_PALETTE} colors", colors.len());
        if colors.len() > AC_PALETTE {
            ui.colored_label(Color32::from_rgb(255, 170, 90), text);
        } else {
            ui.label(text);
        }
    }
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

pub fn layers(
    ui: &mut egui::Ui,
    editor: &mut Editor,
    r: VariantRef,
    selection: &mut Selection,
    message: &mut Option<String>,
) {
    let variant = editor.project().variant(r);
    let count = variant.layers().len();
    // Top layer first, as they stack.
    for i in (0..count).rev() {
        let layer = editor.project().variant(r).layers()[i].clone();
        ui.horizontal(|ui| {
            let mut visible = layer.visible;
            if ui
                .checkbox(&mut visible, "")
                .on_hover_text("Visible; hidden layers don't export")
                .changed()
            {
                edit_variant(editor, r, message, |v| v.layers_mut()[i].visible = visible);
            }
            let mut locked = layer.locked;
            if ui
                .toggle_value(&mut locked, "lock")
                .on_hover_text("Locked layers can't be picked or drawn on")
                .changed()
            {
                edit_variant(editor, r, message, |v| v.layers_mut()[i].locked = locked);
            }
            if ui
                .selectable_label(
                    selection.layer == i,
                    format!("{} ({})", layer.name, layer.shapes().len()),
                )
                .clicked()
                && selection.layer != i
            {
                *selection = Selection {
                    layer: i,
                    ..Selection::default()
                };
            }
            let mut opacity = layer.opacity;
            let slider = ui.add(
                egui::Slider::new(&mut opacity, 0.0..=1.0)
                    .show_value(false)
                    .text("")
                    .clamping(egui::SliderClamping::Always),
            );
            if slider.changed() {
                report(
                    editor.edit_variant_continuous(("layer opacity", r, i), r, |v| {
                        v.layers_mut()[i].opacity = opacity;
                    }),
                    message,
                );
            }
            slider.on_hover_text("Opacity in the views; export ignores it");
        });
    }
    ui.horizontal_wrapped(|ui| {
        if ui.button("New").clicked() {
            let at = if count == 0 { 0 } else { selection.layer + 1 };
            let name = format!("Layer {}", count + 1);
            if edit_variant(editor, r, message, |v| v.insert_layer(at, Layer::new(name))).is_some()
            {
                *selection = Selection {
                    layer: at,
                    ..Selection::default()
                };
            }
        }
        if count == 0 {
            return;
        }
        let i = selection.layer;
        let locked = editor.project().variant(r).layers()[i].locked;
        if ui
            .add_enabled(!locked, egui::Button::new("Delete"))
            .on_disabled_hover_text("The layer is locked")
            .clicked()
        {
            edit_variant(editor, r, message, |v| {
                v.remove_layer(i);
            });
            *selection = Selection {
                layer: i.saturating_sub(1),
                ..Selection::default()
            };
        }
        if ui
            .add_enabled(i + 1 < count, egui::Button::new("Up"))
            .clicked()
        {
            edit_variant(editor, r, message, |v| v.move_layer(i, i + 1));
            selection.layer = i + 1;
        }
        if ui.add_enabled(i > 0, egui::Button::new("Down")).clicked() {
            edit_variant(editor, r, message, |v| v.move_layer(i, i - 1));
            selection.layer = i - 1;
        }
        if ui.button("Duplicate").clicked() {
            edit_variant(editor, r, message, |v| {
                let mut copy = v.layers()[i].clone();
                copy.name = format!("{} copy", copy.name);
                v.insert_layer(i + 1, copy);
            });
            *selection = Selection {
                layer: i + 1,
                ..Selection::default()
            };
        }
        copy_to_variants(ui, editor, r, i, message);
    });
    if let Some(layer) = editor.project().variant(r).layers().get(selection.layer) {
        let mut name = layer.name.clone();
        let i = selection.layer;
        ui.horizontal(|ui| {
            ui.label("Name");
            if ui.text_edit_singleline(&mut name).changed() {
                report(
                    editor.edit_variant_continuous(("layer name", r, i), r, |v| {
                        v.layers_mut()[i].name = name;
                    }),
                    message,
                );
            }
        });
    }
}

/// A menu that copies layer `i` onto the top of other variants of the same
/// texture.
fn copy_to_variants(
    ui: &mut egui::Ui,
    editor: &mut Editor,
    r: VariantRef,
    i: usize,
    message: &mut Option<String>,
) {
    let names: Vec<String> = editor
        .project()
        .texture(r.texture)
        .variants()
        .iter()
        .map(|v| v.name.clone())
        .collect();
    if names.len() < 2 {
        return;
    }
    ui.menu_button("Copy to…", |ui| {
        let layer = editor.project().variant(r).layers()[i].clone();
        let targets: Vec<usize> = (0..names.len()).filter(|&k| k != r.variant).collect();
        if ui.button("All other variants").clicked() {
            copy_layer(editor, r, &layer, &targets, message);
            ui.close();
        }
        for k in targets {
            if ui.button(format!("Variant {}", names[k])).clicked() {
                copy_layer(editor, r, &layer, &[k], message);
                ui.close();
            }
        }
    });
}

fn copy_layer(
    editor: &mut Editor,
    r: VariantRef,
    layer: &Layer,
    targets: &[usize],
    message: &mut Option<String>,
) {
    let result = editor.edit(|p| {
        for &k in targets {
            let to = VariantRef {
                texture: r.texture,
                variant: k,
            };
            p.edit_variant(to, |v| v.insert_layer(v.layers().len(), layer.clone()))?;
        }
        Ok::<_, mint_core::EditError>(())
    });
    if let Err(e) = result {
        *message = Some(e.to_string());
    }
}

/// Settings and actions for the picked shapes.
pub fn shapes(
    ui: &mut egui::Ui,
    editor: &mut Editor,
    r: VariantRef,
    selection: &mut Selection,
    paint: Paint,
    message: &mut Option<String>,
) {
    let layer = selection.layer;
    let picked = selection.shapes.clone();
    let Some(&first) = picked.first() else {
        ui.label("Pick shapes with the Select tool.");
        return;
    };
    if editor.project().variant(r).layers()[layer].locked {
        ui.label("The picked shapes' layer is locked.");
        return;
    }
    let style = editor.project().variant(r).layers()[layer].shapes()[first].style();
    ui.horizontal_wrapped(|ui| {
        if ui.button("Use paint color").clicked() {
            let color = paint.color;
            edit_variant(editor, r, message, |v| {
                for &s in &picked {
                    v.layers_mut()[layer].shapes_mut()[s].set_color(color);
                }
            });
        }
        let mut fill = style == Style::Fill;
        if ui.checkbox(&mut fill, "Filled").changed() {
            // Every picked shape takes the box's new value; strokes keep
            // their width and fills become closed strokes.
            let width = paint.width;
            set_style(editor, None, r, layer, &picked, message, |s| {
                match (fill, s) {
                    (true, _) => Style::Fill,
                    (false, Style::Fill) => Style::Stroke {
                        width,
                        closed: true,
                    },
                    (false, stroke) => stroke,
                }
            });
        }
        // Width and closing apply to each picked stroke, keeping its other
        // settings; the first picked shape shows the values.
        if let Style::Stroke { width, closed } = style {
            let mut w = width;
            let drag = ui.add(
                egui::DragValue::new(&mut w)
                    .range(0.1..=64.0)
                    .clamp_existing_to_range(false)
                    .speed(0.05)
                    .suffix(" px"),
            );
            if drag.changed() {
                let key = Some(("stroke width", r, layer));
                set_style(editor, key, r, layer, &picked, message, |s| match s {
                    Style::Stroke { closed, .. } => Style::Stroke { width: w, closed },
                    Style::Fill => Style::Fill,
                });
            }
            let mut c = closed;
            if ui.checkbox(&mut c, "Closed").changed() {
                set_style(editor, None, r, layer, &picked, message, |s| match s {
                    Style::Stroke { width, .. } => Style::Stroke { width, closed: c },
                    Style::Fill => Style::Fill,
                });
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        let count = editor.project().variant(r).layers()[layer].shapes().len();
        let mut sorted = picked.clone();
        sorted.sort_unstable();
        if ui.button("Duplicate").clicked() {
            duplicate(editor, r, selection, message);
        }
        if ui.button("Forward").clicked() && sorted.last().is_some_and(|&s| s + 1 < count) {
            edit_variant(editor, r, message, |v| {
                for &s in sorted.iter().rev() {
                    v.layers_mut()[layer].move_shape(s, s + 1);
                }
            });
            selection.shapes = picked.iter().map(|s| s + 1).collect();
        }
        if ui.button("Backward").clicked() && sorted.first().is_some_and(|&s| s > 0) {
            edit_variant(editor, r, message, |v| {
                for &s in &sorted {
                    v.layers_mut()[layer].move_shape(s, s - 1);
                }
            });
            selection.shapes = picked.iter().map(|s| s - 1).collect();
        }
        let names: Vec<String> = editor
            .project()
            .variant(r)
            .layers()
            .iter()
            .map(|l| l.name.clone())
            .collect();
        ui.menu_button("To layer", |ui| {
            let locked: Vec<bool> = editor
                .project()
                .variant(r)
                .layers()
                .iter()
                .map(|l| l.locked)
                .collect();
            for (k, name) in names.iter().enumerate() {
                if k != layer
                    && ui
                        .add_enabled(!locked[k], egui::Button::new(name))
                        .clicked()
                {
                    let moved = edit_variant(editor, r, message, |v| {
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
                        *selection = Selection {
                            layer: k,
                            shapes: (start..start + sorted.len()).collect(),
                            anchor: None,
                        };
                    }
                    ui.close();
                }
            }
        });
    });
}

/// Restyles each picked shape with `restyle` of its own style, as part of
/// the continuous edit `key` if any.
fn set_style(
    editor: &mut Editor,
    key: Option<(&str, VariantRef, usize)>,
    r: VariantRef,
    layer: usize,
    picked: &[usize],
    message: &mut Option<String>,
    restyle: impl Fn(Style) -> Style,
) {
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
        Some(key) => editor.edit_continuous(key, apply),
        None => editor.edit(apply),
    };
    report(result, message);
}

/// Copies the picked shapes on top of their layer and picks the copies.
pub fn duplicate(
    editor: &mut Editor,
    r: VariantRef,
    selection: &mut Selection,
    message: &mut Option<String>,
) {
    let layer = selection.layer;
    let mut picked = selection.shapes.clone();
    picked.sort_unstable();
    if picked.is_empty() || editor.project().variant(r).layers()[layer].locked {
        return;
    }
    let start = edit_variant(editor, r, message, |v| {
        let l = &mut v.layers_mut()[layer];
        let copies: Vec<_> = picked.iter().map(|&s| l.shapes()[s].clone()).collect();
        let start = l.shapes().len();
        for s in copies {
            l.insert_shape(l.shapes().len(), s);
        }
        start
    });
    if let Some(start) = start {
        selection.shapes = (start..start + picked.len()).collect();
        selection.anchor = None;
    }
}
