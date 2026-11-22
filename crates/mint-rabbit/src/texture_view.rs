//! The Texture View: one texture variant, pixel-exact, with its shapes drawn
//! as outlines and edited with the drawing tools.

use eframe::egui::{
    self, Color32, ColorImage, Key, PointerButton, Pos2, Rect, Sense, Stroke, TextureHandle,
    TextureOptions, pos2, vec2,
};
use glam::{Affine2, Vec2};
use mint_core::{Anchor, Layer, PaletteIndex, Project, Shape, Style, TextureId, VariantRef, shape};

use crate::editor::Editor;
use crate::scene;

/// Pointer distance, in screen points, within which handles and outlines
/// are hit.
const HIT: f32 = 7.0;
/// Freehand simplification tolerance in texels.
const BRUSH_TOLERANCE: f32 = 0.25;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Pen,
    Brush,
    Ellipse,
    Rectangle,
}

impl Tool {
    pub const ALL: [Self; 5] = [
        Self::Select,
        Self::Pen,
        Self::Brush,
        Self::Ellipse,
        Self::Rectangle,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Select => "Select",
            Self::Pen => "Pen",
            Self::Brush => "Brush",
            Self::Ellipse => "Ellipse",
            Self::Rectangle => "Rectangle",
        }
    }
}

/// How new shapes are painted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Paint {
    pub color: PaletteIndex,
    /// Filled; otherwise stroked.
    pub fill: bool,
    pub width: f32,
}

impl Paint {
    /// The style of a new shape: brush and open pen paths are always
    /// strokes.
    fn style(self, closed: bool) -> Style {
        if self.fill && closed {
            Style::Fill
        } else {
            Style::Stroke {
                width: self.width,
                closed,
            }
        }
    }
}

/// Shapes picked for editing, all in one layer of the shown variant.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    /// The layer new shapes go into, and the one the picked shapes are in.
    pub layer: usize,
    /// Indices into the layer's shapes.
    pub shapes: Vec<usize>,
    /// With exactly one shape picked, its picked anchor.
    pub anchor: Option<usize>,
}

impl Selection {
    /// Drops picks that no longer exist, for example after an undo.
    pub fn fit(&mut self, project: &Project, r: VariantRef) {
        let layers = project.variant(r).layers();
        if self.layer >= layers.len() {
            *self = Self {
                layer: layers.len().saturating_sub(1),
                ..Self::default()
            };
            return;
        }
        // Shapes on a locked or hidden layer can't stay picked.
        let layer = &layers[self.layer];
        if layer.locked || !layer.visible {
            self.shapes.clear();
        }
        let count = layer.shapes().len();
        self.shapes.retain(|&s| s < count);
        if self.shapes.len() != 1
            || self
                .anchor
                .is_some_and(|a| a >= layers[self.layer].shapes()[self.shapes[0]].anchors().len())
        {
            self.anchor = None;
        }
    }

    pub fn single(&self) -> Option<usize> {
        match self.shapes[..] {
            [s] => Some(s),
            _ => None,
        }
    }
}

/// What the shown image is, so it is uploaded again only when it changes.
#[derive(Clone, Copy, PartialEq, Eq)]
struct ImageKey {
    generation: u64,
    texture: TextureId,
    variant: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Point,
    In,
    Out,
}

/// A drag in progress, with what it started from.
enum Drag {
    Move {
        start: Vec2,
        original: Vec<Shape>,
    },
    Scale {
        pivot: Vec2,
        start: Vec2,
        original: Vec<Shape>,
    },
    Rotate {
        center: Vec2,
        start: Vec2,
        original: Vec<Shape>,
    },
    Anchor {
        anchor: usize,
        part: Part,
        original: Shape,
    },
    PenHandle,
    Brush(Vec<Vec2>),
    Box {
        start: Vec2,
        end: Vec2,
        tool: Tool,
    },
}

/// Everything the view needs from the app for one frame.
pub struct Frame<'a> {
    pub editor: &'a mut Editor,
    pub texture: TextureId,
    pub variant: usize,
    pub tool: Tool,
    pub paint: Paint,
    pub selection: &'a mut Selection,
    pub show_uv: bool,
    /// Set when the view refuses an edit, to show the user.
    pub message: &'a mut Option<String>,
}

#[derive(Default)]
pub struct TextureView {
    cached: Option<(ImageKey, TextureHandle)>,
    /// Magnification over fitting the texture to the view.
    zoom: f32,
    /// Offset of the image from the view's center, in screen points.
    pan: egui::Vec2,
    drag: Option<Drag>,
    /// Anchors of the pen path being drawn.
    pen: Vec<Anchor>,
}

/// Screen placement of the image: texture space to screen and back.
#[derive(Clone, Copy)]
struct Placement {
    /// The view's area, which drawing stays inside.
    view: Rect,
    image: Rect,
    /// Screen points per texel.
    scale: f32,
}

impl Placement {
    fn to_screen(self, p: Vec2) -> Pos2 {
        self.image.min + vec2(p.x, p.y) * self.scale
    }

    fn to_texture(self, p: Pos2) -> Vec2 {
        let d = (p - self.image.min) / self.scale;
        Vec2::new(d.x, d.y)
    }

    /// [`HIT`] in texels.
    fn hit(self) -> f32 {
        HIT / self.scale
    }
}

impl TextureView {
    /// Cancels any drawing in progress, as when the tool or texture changes.
    pub fn cancel(&mut self) {
        self.drag = None;
        self.pen.clear();
    }

    pub fn show(&mut self, ui: &mut egui::Ui, f: Frame<'_>) {
        if self.zoom == 0.0 {
            self.zoom = 1.0;
        }
        let r = VariantRef {
            texture: f.texture,
            variant: f.variant,
        };
        let tex = f.editor.project().texture(f.texture);
        let (size, editable) = (tex.size().as_vec2(), !tex.variants().is_empty());
        if editable {
            f.selection.fit(f.editor.project(), r);
        }
        let handle = self.image(
            ui,
            f.editor.project(),
            f.editor.generation(),
            f.texture,
            f.variant,
        );

        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let fit = (rect.width() / size.x).min(rect.height() / size.y).max(1.0);
        let scale = fit * self.zoom;
        let image = Rect::from_center_size(rect.center() + self.pan, vec2(size.x, size.y) * scale);
        let at = Placement {
            view: rect,
            image,
            scale,
        };

        self.navigate(ui, &response, at);
        let painter = ui.painter_at(rect);
        checkerboard(&painter, image, scale);
        painter.image(
            handle.id(),
            image,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        painter.rect_stroke(
            image,
            0.0,
            Stroke::new(1.0, Color32::GRAY),
            egui::StrokeKind::Outside,
        );
        if f.show_uv {
            let stroke = Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 0, 110));
            for tri in scene::uv_triangles(f.editor.project(), f.texture) {
                let p = tri.map(|uv| at.to_screen(uv * size));
                painter.add(egui::Shape::closed_line(p.to_vec(), stroke));
            }
        }
        if editable {
            self.interact(ui, &response, at, f, r);
        }
    }

    /// The composited image of a variant as an egui texture.
    fn image(
        &mut self,
        ui: &egui::Ui,
        project: &Project,
        generation: u64,
        texture: TextureId,
        variant: usize,
    ) -> TextureHandle {
        let key = ImageKey {
            generation,
            texture,
            variant,
        };
        if let Some((k, h)) = &self.cached
            && *k == key
        {
            return h.clone();
        }
        let tex = project.texture(texture);
        let size = tex.size();
        let pixels: Vec<Color32> = scene::texture_pixels(project, tex, variant)
            .iter()
            .map(|c| Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a))
            .collect();
        let image = ColorImage::new([size.x as usize, size.y as usize], pixels);
        let h = ui
            .ctx()
            .load_texture("texture view", image, TextureOptions::NEAREST);
        self.cached = Some((key, h.clone()));
        h
    }

    /// Zooms with the scroll wheel around the pointer, and pans with the
    /// middle button.
    fn navigate(&mut self, ui: &egui::Ui, response: &egui::Response, at: Placement) {
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0
                && let Some(p) = response.hover_pos()
            {
                let factor = (scroll * 0.003).exp();
                let zoom = (self.zoom * factor).clamp(0.25, 64.0);
                let applied = zoom / self.zoom;
                // Keep the texel under the pointer in place.
                let center = at.image.center();
                self.pan += (center - p) * (applied - 1.0);
                self.zoom = zoom;
            }
        }
        if response.dragged_by(PointerButton::Middle) {
            self.pan += response.drag_delta();
        }
        if response.double_clicked_by(PointerButton::Middle) {
            self.zoom = 1.0;
            self.pan = egui::Vec2::ZERO;
        }
    }

    fn interact(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        at: Placement,
        f: Frame<'_>,
        r: VariantRef,
    ) {
        let Frame {
            editor,
            tool,
            paint,
            selection,
            message,
            ..
        } = f;
        if tool != Tool::Pen && !self.pen.is_empty() {
            self.pen.clear();
        }
        let pointer = response
            .interact_pointer_pos()
            .or_else(|| response.hover_pos())
            .map(|p| at.to_texture(p));
        let shift = ui.input(|i| i.modifiers.shift);

        // A drag, by the primary button only, starts where the button went
        // down.
        if response.drag_started_by(PointerButton::Primary)
            && let Some(origin) = ui
                .input(|i| i.pointer.press_origin())
                .map(|p| at.to_texture(p))
        {
            self.drag = self.start(editor, tool, paint, selection, r, origin, at, message);
        } else if response.clicked()
            && let Some(p) = pointer
        {
            match tool {
                Tool::Pen => self.pen_click(editor, paint, selection, r, p, at, message),
                Tool::Brush => {
                    add_shape(
                        editor,
                        selection,
                        r,
                        vec![Anchor::corner(p)],
                        paint.style(false),
                        paint.color,
                        message,
                    );
                }
                Tool::Select => pick(editor.project(), r, selection, p, at, shift),
                Tool::Ellipse | Tool::Rectangle => {}
            }
        }
        if response.double_clicked()
            && tool == Tool::Select
            && let (Some(p), Some(s)) = (pointer, selection.single())
            && let Some(k) = insert_anchor(editor, r, selection.layer, s, p, at)
        {
            selection.anchor = Some(k);
        }

        if let (Some(drag), Some(p)) = (&mut self.drag, pointer) {
            continue_drag(editor, drag, selection, r, p, &mut self.pen, shift);
        }
        let escape = ui.input(|i| i.key_pressed(Key::Escape));
        if escape && self.drag.is_some() {
            // Drawing is dropped; edits a drag already made stay one step.
            self.drag = None;
            editor.end_gesture();
        } else if response.drag_stopped_by(PointerButton::Primary) {
            if let Some(drag) = self.drag.take() {
                finish_drag(editor, drag, selection, r, paint, message);
            }
            editor.end_gesture();
        }

        // Keys, while the view has the pointer.
        // Keys a text field is using are its own.
        if (response.hovered() || !self.pen.is_empty()) && !ui.ctx().egui_wants_keyboard_input() {
            let (enter, delete) = ui.input(|i| {
                (
                    i.key_pressed(Key::Enter),
                    i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace),
                )
            });
            if enter && !self.pen.is_empty() {
                // Enter finishes an open path; clicking the first anchor
                // closes one.
                let anchors = std::mem::take(&mut self.pen);
                add_shape(
                    editor,
                    selection,
                    r,
                    anchors,
                    paint.style(false),
                    paint.color,
                    message,
                );
            }
            if escape {
                if self.pen.is_empty() {
                    selection.shapes.clear();
                    selection.anchor = None;
                } else {
                    self.pen.clear();
                }
            }
            if delete {
                // While drawing a pen path, its last anchor goes first.
                if self.pen.pop().is_none() {
                    delete_selected(editor, r, selection);
                }
            }
        }

        self.overlay(ui, editor.project(), r, selection, at, pointer, tool);
    }

    #[allow(clippy::too_many_arguments)]
    fn start(
        &mut self,
        editor: &mut Editor,
        tool: Tool,
        paint: Paint,
        selection: &mut Selection,
        r: VariantRef,
        p: Vec2,
        at: Placement,
        message: &mut Option<String>,
    ) -> Option<Drag> {
        match tool {
            Tool::Brush => Some(Drag::Brush(vec![p])),
            Tool::Ellipse | Tool::Rectangle => Some(Drag::Box {
                start: p,
                end: p,
                tool,
            }),
            Tool::Pen => {
                self.pen_click(editor, paint, selection, r, p, at, message);
                (!self.pen.is_empty()).then_some(Drag::PenHandle)
            }
            Tool::Select => start_select(editor.project(), r, selection, p, at),
        }
    }

    /// A pen click: closes the path on its first anchor, or adds an anchor.
    #[allow(clippy::too_many_arguments)]
    fn pen_click(
        &mut self,
        editor: &mut Editor,
        paint: Paint,
        selection: &mut Selection,
        r: VariantRef,
        p: Vec2,
        at: Placement,
        message: &mut Option<String>,
    ) {
        if self.pen.len() >= 2 && self.pen[0].point.distance(p) <= at.hit() {
            let anchors = std::mem::take(&mut self.pen);
            add_shape(
                editor,
                selection,
                r,
                anchors,
                paint.style(true),
                paint.color,
                message,
            );
        } else if self
            .pen
            .last()
            .is_none_or(|a| a.point.distance(p) > at.hit())
        {
            self.pen.push(Anchor::corner(p));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn overlay(
        &self,
        ui: &egui::Ui,
        project: &Project,
        r: VariantRef,
        selection: &Selection,
        at: Placement,
        cursor: Option<Vec2>,
        tool: Tool,
    ) {
        let painter = ui.painter_at(at.view);
        let layers = project.variant(r).layers();
        let faint = Stroke::new(1.0, Color32::from_rgba_unmultiplied(200, 200, 255, 90));
        let picked = Stroke::new(1.5, Color32::from_rgb(80, 220, 255));
        let line = |o: &mint_core::Outline, stroke: Stroke| {
            let pts: Vec<Pos2> = o.points().iter().map(|&q| at.to_screen(q)).collect();
            if o.closed() && pts.len() > 2 {
                painter.add(egui::Shape::closed_line(pts, stroke));
            } else if pts.len() > 1 {
                painter.add(egui::Shape::line(pts, stroke));
            } else if let Some(&q) = pts.first() {
                painter.circle_stroke(q, 2.0, stroke);
            }
        };
        if let Some(layer) = layers.get(selection.layer).filter(|l| l.visible) {
            for (i, s) in layer.shapes().iter().enumerate() {
                let stroke = if selection.shapes.contains(&i) {
                    picked
                } else {
                    faint
                };
                line(&s.outline(), stroke);
            }
            let shapes: Vec<&Shape> = selection
                .shapes
                .iter()
                .map(|&i| &layer.shapes()[i])
                .collect();
            if tool == Tool::Select
                && let Some((min, max)) = bounds(shapes.iter().copied())
            {
                let rect = Rect::from_min_max(at.to_screen(min), at.to_screen(max));
                painter.rect_stroke(
                    rect,
                    0.0,
                    Stroke::new(1.0, Color32::from_gray(160)),
                    egui::StrokeKind::Outside,
                );
                for c in scale_handles(min, max, at) {
                    painter.rect_filled(
                        Rect::from_center_size(c, vec2(7.0, 7.0)),
                        0.0,
                        Color32::WHITE,
                    );
                }
                painter.circle_filled(
                    rotate_handle(min, max, at),
                    4.5,
                    Color32::from_rgb(255, 200, 80),
                );
                if let [s] = shapes[..] {
                    for (k, a) in s.anchors().iter().enumerate() {
                        let p = at.to_screen(a.point);
                        for h in [a.handle_in, a.handle_out] {
                            if h != Vec2::ZERO {
                                let q = at.to_screen(a.point + h);
                                painter.line_segment(
                                    [p, q],
                                    Stroke::new(1.0, Color32::from_gray(200)),
                                );
                                painter.circle_filled(q, 3.0, Color32::from_gray(230));
                            }
                        }
                        let color = if selection.anchor == Some(k) {
                            Color32::from_rgb(255, 120, 80)
                        } else {
                            Color32::from_rgb(80, 220, 255)
                        };
                        painter.rect_filled(Rect::from_center_size(p, vec2(6.0, 6.0)), 0.0, color);
                    }
                }
            }
        }
        // Drawing in progress.
        let preview = Stroke::new(1.5, Color32::from_rgb(255, 120, 200));
        match &self.drag {
            Some(Drag::Brush(points)) => {
                let pts: Vec<Pos2> = points.iter().map(|&q| at.to_screen(q)).collect();
                painter.add(egui::Shape::line(pts, preview));
            }
            Some(Drag::Box { start, end, tool }) => {
                let anchors = box_anchors(*tool, *start, *end);
                if let Ok(s) = Shape::new(anchors, Style::Fill, PaletteIndex(0)) {
                    line(&s.outline(), preview);
                }
            }
            _ => {}
        }
        if !self.pen.is_empty() {
            let mut anchors = self.pen.clone();
            if let Some(p) = cursor
                && !matches!(self.drag, Some(Drag::PenHandle))
            {
                anchors.push(Anchor::corner(p));
            }
            if let Ok(s) = Shape::new(
                anchors,
                Style::Stroke {
                    width: 1.0,
                    closed: false,
                },
                PaletteIndex(0),
            ) {
                line(&s.outline(), preview);
            }
            for a in &self.pen {
                painter.rect_filled(
                    Rect::from_center_size(at.to_screen(a.point), vec2(6.0, 6.0)),
                    0.0,
                    Color32::from_rgb(255, 120, 200),
                );
            }
        }
    }
}

fn checkerboard(painter: &egui::Painter, image: Rect, scale: f32) {
    let cell = (scale * 2.0).max(8.0);
    let (cols, rows) = (
        (image.width() / cell).ceil() as i32,
        (image.height() / cell).ceil() as i32,
    );
    for y in 0..rows {
        for x in 0..cols {
            let shade = if (x + y) % 2 == 0 { 70 } else { 90 };
            let min = image.min + vec2(x as f32, y as f32) * cell;
            let r = Rect::from_min_max(min, (min + vec2(cell, cell)).min(image.max));
            painter.rect_filled(r, 0.0, Color32::from_gray(shade));
        }
    }
}

fn bounds<'a>(shapes: impl Iterator<Item = &'a Shape>) -> Option<(Vec2, Vec2)> {
    shapes
        .map(|s| s.outline().bounds())
        .reduce(|(a0, a1), (b0, b1)| (a0.min(b0), a1.max(b1)))
}

fn corners(min: Vec2, max: Vec2) -> [Vec2; 4] {
    [min, Vec2::new(max.x, min.y), max, Vec2::new(min.x, max.y)]
}

/// Screen positions of the scale handles, just outside the selection's
/// corners so they don't cover anchors there.
fn scale_handles(min: Vec2, max: Vec2, at: Placement) -> [Pos2; 4] {
    let out = [
        vec2(-1.0, -1.0),
        vec2(1.0, -1.0),
        vec2(1.0, 1.0),
        vec2(-1.0, 1.0),
    ];
    let c = corners(min, max);
    std::array::from_fn(|k| at.to_screen(c[k]) + out[k] * 8.0)
}

/// The rotation handle, a little above the selection's top edge.
fn rotate_handle(min: Vec2, max: Vec2, at: Placement) -> Pos2 {
    at.to_screen(Vec2::new(f32::midpoint(min.x, max.x), min.y)) - vec2(0.0, 18.0)
}

fn box_anchors(tool: Tool, a: Vec2, b: Vec2) -> Vec<Anchor> {
    if tool == Tool::Ellipse {
        shape::ellipse((a + b) / 2.0, (b - a).abs() / 2.0)
    } else {
        shape::rectangle(a, b)
    }
}

/// Whether point `p` hits shape `s`, within `slack` texels of its path.
fn hits(s: &Shape, p: Vec2, slack: f32) -> bool {
    let o = s.outline();
    o.covers(p) || o.distance(p) <= slack
}

/// Picks the topmost shape under `p` in a visible, unlocked layer, or with
/// `add`, adds or removes it from a selection in the same layer.
fn pick(
    project: &Project,
    r: VariantRef,
    selection: &mut Selection,
    p: Vec2,
    at: Placement,
    add: bool,
) {
    let layers = project.variant(r).layers();
    let hit = layers
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, l)| l.visible && !l.locked)
        .find_map(|(li, l)| {
            l.shapes()
                .iter()
                .enumerate()
                .rev()
                .find(|(_, s)| hits(s, p, at.hit()))
                .map(|(si, _)| (li, si))
        });
    match hit {
        Some((layer, shape)) if add && layer == selection.layer => {
            if let Some(k) = selection.shapes.iter().position(|&s| s == shape) {
                selection.shapes.remove(k);
            } else {
                selection.shapes.push(shape);
            }
            selection.anchor = None;
        }
        Some((layer, shape)) => {
            *selection = Selection {
                layer,
                shapes: vec![shape],
                anchor: None,
            };
        }
        None if !add => {
            selection.shapes.clear();
            selection.anchor = None;
        }
        None => {}
    }
}

/// Starts a Select-tool drag on whatever is under `p`: an anchor or handle
/// of the one picked shape, a transform handle, or a shape to move.
fn start_select(
    project: &Project,
    r: VariantRef,
    selection: &mut Selection,
    p: Vec2,
    at: Placement,
) -> Option<Drag> {
    let layers = project.variant(r).layers();
    let layer = layers.get(selection.layer)?;
    let picked: Vec<Shape> = selection
        .shapes
        .iter()
        .map(|&i| layer.shapes()[i].clone())
        .collect();
    if let [s] = &picked[..] {
        for (k, a) in s.anchors().iter().enumerate() {
            let parts = [
                (Part::Point, a.point),
                (Part::In, a.point + a.handle_in),
                (Part::Out, a.point + a.handle_out),
            ];
            for (part, q) in parts {
                if (part == Part::Point || q != a.point) && q.distance(p) <= at.hit() {
                    selection.anchor = Some(k);
                    return Some(Drag::Anchor {
                        anchor: k,
                        part,
                        original: s.clone(),
                    });
                }
            }
        }
    }
    if let Some((min, max)) = bounds(picked.iter()) {
        let screen = at.to_screen(p);
        if rotate_handle(min, max, at).distance(screen) <= HIT {
            return Some(Drag::Rotate {
                center: (min + max) / 2.0,
                start: p,
                original: picked,
            });
        }
        let c = corners(min, max);
        let handles = scale_handles(min, max, at);
        if let Some(k) = (0..4).find(|&k| handles[k].distance(screen) <= HIT) {
            return Some(Drag::Scale {
                pivot: c[(k + 2) % 4],
                start: c[k],
                original: picked,
            });
        }
        if picked.iter().any(|s| hits(s, p, at.hit())) {
            return Some(Drag::Move {
                start: p,
                original: picked,
            });
        }
    }
    // Not on the selection: pick what is under the pointer and move it.
    pick(project, r, selection, p, at, false);
    let layer = project.variant(r).layers().get(selection.layer)?;
    let s = selection.single()?;
    Some(Drag::Move {
        start: p,
        original: vec![layer.shapes()[s].clone()],
    })
}

/// Replaces the picked shapes with `shapes`, in the same order.
fn replace_picked(editor: &mut Editor, r: VariantRef, selection: &Selection, shapes: Vec<Shape>) {
    let (layer, picked) = (selection.layer, selection.shapes.clone());
    // A drag's edits undo as one step.
    let _ = editor.edit_variant_continuous("canvas drag", r, |v| {
        let slots = v.layers_mut()[layer].shapes_mut();
        for (&i, s) in picked.iter().zip(shapes) {
            slots[i] = s;
        }
    });
}

fn transformed(original: &[Shape], t: Affine2) -> Vec<Shape> {
    original
        .iter()
        .map(|s| {
            let mut s = s.clone();
            s.transform(t);
            s
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn continue_drag(
    editor: &mut Editor,
    drag: &mut Drag,
    selection: &Selection,
    r: VariantRef,
    p: Vec2,
    pen: &mut [Anchor],
    shift: bool,
) {
    match drag {
        Drag::Move { start, original } => {
            let t = Affine2::from_translation(p - *start);
            replace_picked(editor, r, selection, transformed(original, t));
        }
        Drag::Scale {
            pivot,
            start,
            original,
        } => {
            let from = *start - *pivot;
            let to = p - *pivot;
            let ratio = |a: f32, b: f32| if a.abs() > 1e-3 { b / a } else { 1.0 };
            let mut k = Vec2::new(ratio(from.x, to.x), ratio(from.y, to.y));
            if shift {
                // Uniform, by the larger change.
                let m = if k.x.abs() > k.y.abs() { k.x } else { k.y };
                k = Vec2::splat(m);
            }
            let t = Affine2::from_translation(*pivot)
                * Affine2::from_scale(k)
                * Affine2::from_translation(-*pivot);
            replace_picked(editor, r, selection, transformed(original, t));
        }
        Drag::Rotate {
            center,
            start,
            original,
        } => {
            let a0 = (*start - *center).to_angle();
            let a1 = (p - *center).to_angle();
            let mut angle = a1 - a0;
            if shift {
                let step = std::f32::consts::PI / 12.0;
                angle = (angle / step).round() * step;
            }
            let t = Affine2::from_translation(*center)
                * Affine2::from_angle(angle)
                * Affine2::from_translation(-*center);
            replace_picked(editor, r, selection, transformed(original, t));
        }
        Drag::Anchor {
            anchor,
            part,
            original,
        } => {
            let mut s = original.clone();
            // A smooth anchor, judged as it was, stays smooth unless Shift
            // is held.
            let was = original.anchors()[*anchor];
            let smooth = !shift && is_smooth(was);
            let a = &mut s.anchors_mut()[*anchor];
            match part {
                Part::Point => a.point = p,
                Part::Out => {
                    a.handle_out = p - a.point;
                    if smooth {
                        a.handle_in = opposite(a.handle_out, was.handle_in.length());
                    }
                }
                Part::In => {
                    a.handle_in = p - a.point;
                    if smooth {
                        a.handle_out = opposite(a.handle_in, was.handle_out.length());
                    }
                }
            }
            replace_picked(editor, r, selection, vec![s]);
        }
        Drag::PenHandle => {
            if let Some(a) = pen.last_mut() {
                a.handle_out = p - a.point;
                a.handle_in = -a.handle_out;
            }
        }
        Drag::Brush(points) => {
            if points.last().is_none_or(|q| q.distance(p) > 0.05) {
                points.push(p);
            }
        }
        Drag::Box { end, .. } => *end = p,
    }
}

/// Whether an anchor's handles point opposite ways.
fn is_smooth(a: Anchor) -> bool {
    a.handle_in != Vec2::ZERO
        && a.handle_out != Vec2::ZERO
        && a.handle_in.normalize().dot(a.handle_out.normalize()) < -0.99
}

/// A handle `len` long pointing opposite `handle`.
fn opposite(handle: Vec2, len: f32) -> Vec2 {
    -handle.normalize_or_zero() * len
}

fn finish_drag(
    editor: &mut Editor,
    drag: Drag,
    selection: &mut Selection,
    r: VariantRef,
    paint: Paint,
    message: &mut Option<String>,
) {
    match drag {
        Drag::Brush(points) => {
            let anchors = shape::freehand(&points, BRUSH_TOLERANCE);
            add_shape(
                editor,
                selection,
                r,
                anchors,
                paint.style(false),
                paint.color,
                message,
            );
        }
        Drag::Box { start, end, tool } if (end - start).abs().min_element() > 0.0 => {
            let style = paint.style(true);
            add_shape(
                editor,
                selection,
                r,
                box_anchors(tool, start, end),
                style,
                paint.color,
                message,
            );
        }
        _ => {}
    }
}

/// Adds a shape on top of the selected layer, creating a first layer if the
/// variant has none, and picks it.
pub fn add_shape(
    editor: &mut Editor,
    selection: &mut Selection,
    r: VariantRef,
    anchors: Vec<Anchor>,
    style: Style,
    color: PaletteIndex,
    message: &mut Option<String>,
) {
    let shape = match Shape::new(anchors, style, color) {
        Ok(s) => s,
        Err(e) => {
            *message = Some(e.to_string());
            return;
        }
    };
    let layers = editor.project().variant(r).layers();
    if let Some(l) = layers.get(selection.layer)
        && (l.locked || !l.visible)
    {
        *message = Some("The layer is locked or hidden.".into());
        return;
    }
    let layer = if layers.is_empty() {
        0
    } else {
        selection.layer
    };
    let result = editor.edit(|p| {
        p.edit_variant(r, |v| {
            if v.layers().is_empty() {
                v.insert_layer(0, Layer::new("Layer 1"));
            }
            let l = &mut v.layers_mut()[layer];
            l.insert_shape(l.shapes().len(), shape);
            l.shapes().len() - 1
        })
    });
    match result {
        Ok(index) => {
            *selection = Selection {
                layer,
                shapes: vec![index],
                anchor: None,
            };
        }
        Err(e) => *message = Some(e.to_string()),
    }
}

/// Deletes the picked anchor, or the picked shapes. A shape's last anchor
/// takes the shape with it.
fn delete_selected(editor: &mut Editor, r: VariantRef, selection: &mut Selection) {
    let layer = selection.layer;
    if editor
        .project()
        .variant(r)
        .layers()
        .get(layer)
        .is_none_or(|l| l.locked)
    {
        return;
    }
    if let (Some(s), Some(a)) = (selection.single(), selection.anchor) {
        let removed = editor.edit(|p| {
            p.edit_variant(r, |v| {
                let l = &mut v.layers_mut()[layer];
                let last = l.shapes_mut()[s].remove_anchor(a).is_err();
                if last {
                    l.remove_shape(s);
                }
                last
            })
        });
        selection.anchor = None;
        if removed == Ok(true) {
            selection.shapes.clear();
        }
        return;
    }
    let mut picked = selection.shapes.clone();
    picked.sort_unstable();
    let _ = editor.edit(|p| {
        p.edit_variant(r, |v| {
            for &s in picked.iter().rev() {
                v.layers_mut()[layer].remove_shape(s);
            }
        })
    });
    selection.shapes.clear();
}

/// Splits the picked shape's segment nearest `p` with a new anchor there,
/// keeping the curve's form.
fn insert_anchor(
    editor: &mut Editor,
    r: VariantRef,
    layer: usize,
    shape: usize,
    p: Vec2,
    at: Placement,
) -> Option<usize> {
    let s = &editor.project().variant(r).layers()[layer].shapes()[shape];
    let (seg, t, d) = s
        .segments()
        .enumerate()
        .flat_map(|(i, c)| {
            (1..32).map(move |k| {
                let t = k as f32 / 32.0;
                (i, t, cubic(c, t).distance(p))
            })
        })
        .min_by(|a, b| a.2.total_cmp(&b.2))?;
    if d > at.hit() {
        return None;
    }
    let c: [Vec2; 4] = s.segments().nth(seg).expect("segment exists");
    // De Casteljau at t.
    let lerp = |a: Vec2, b: Vec2| a + (b - a) * t;
    let (a, b, d) = (lerp(c[0], c[1]), lerp(c[1], c[2]), lerp(c[2], c[3]));
    let (before, after) = (lerp(a, b), lerp(b, d));
    let mid = lerp(before, after);
    let n = s.anchors().len();
    let (i, j) = (seg, (seg + 1) % n);
    editor
        .edit_variant(r, |v| {
            let s = &mut v.layers_mut()[layer].shapes_mut()[shape];
            let anchors = s.anchors_mut();
            anchors[i].handle_out = a - c[0];
            anchors[j].handle_in = d - c[3];
            s.insert_anchor(
                i + 1,
                Anchor {
                    point: mid,
                    handle_in: before - mid,
                    handle_out: after - mid,
                },
            );
        })
        .ok()
        .map(|()| i + 1)
}

fn cubic([p0, p1, p2, p3]: [Vec2; 4], t: f32) -> Vec2 {
    let u = 1.0 - t;
    u * u * u * p0 + 3.0 * u * u * t * p1 + 3.0 * u * t * t * p2 + t * t * t * p3
}
