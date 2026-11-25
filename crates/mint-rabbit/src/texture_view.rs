//! The Texture View: one texture variant, pixel-exact, with its shapes drawn
//! as outlines and edited with the drawing tools.

use eframe::egui::{
    self, Color32, Key, KeyboardShortcut, Modifiers, PointerButton, Pos2, Rect, Sense, Stroke,
    pos2, vec2,
};
use glam::{Affine2, Vec2};
use mint_core::{
    Anchor, Layer, LayerKind, PaletteIndex, Project, Raster, Shape, Style, TextureId, VariantRef,
    shape,
};

use crate::editor::Editor;
use crate::previews::{Previews, checkerboard};
use crate::scene;

/// Pointer distance, in screen points, within which handles and outlines
/// are hit.
const HIT: f32 = 7.0;
/// The key of a canvas drag's continuous edit.
const CANVAS_DRAG: &str = "canvas drag";
/// Freehand simplification tolerance in texels.
const BRUSH_TOLERANCE: f32 = 0.25;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Pen,
    Brush,
    Ellipse,
    Rectangle,
    Pencil,
    Eraser,
    Fill,
}

impl Tool {
    pub const ALL: [Self; 8] = [
        Self::Select,
        Self::Pen,
        Self::Brush,
        Self::Ellipse,
        Self::Rectangle,
        Self::Pencil,
        Self::Eraser,
        Self::Fill,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Select => "Select",
            Self::Pen => "Pen",
            Self::Brush => "Brush",
            Self::Ellipse => "Ellipse",
            Self::Rectangle => "Rectangle",
            Self::Pencil => "Pencil",
            Self::Eraser => "Eraser",
            Self::Fill => "Fill",
        }
    }

    /// The keys that switch to the tool.
    pub fn shortcut(self) -> KeyboardShortcut {
        let key = |key| KeyboardShortcut::new(Modifiers::NONE, key);
        match self {
            Self::Select => key(Key::V),
            Self::Pen => key(Key::P),
            Self::Brush => key(Key::B),
            Self::Ellipse => key(Key::E),
            Self::Rectangle => key(Key::R),
            Self::Pencil => key(Key::N),
            Self::Eraser => KeyboardShortcut::new(Modifiers::SHIFT, Key::E),
            Self::Fill => key(Key::G),
        }
    }

    /// Whether the tool draws on a layer of `kind`. The shape tools paint
    /// their finished shape into a raster layer's pixels.
    pub fn works_on(self, kind: LayerKind) -> bool {
        match self {
            Self::Select | Self::Pen | Self::Ellipse | Self::Rectangle => true,
            Self::Brush => kind == LayerKind::Vector,
            Self::Pencil | Self::Eraser | Self::Fill => kind == LayerKind::Raster,
        }
    }

    /// The tool to use on a layer of `kind`: this one if it works there,
    /// else its counterpart for that kind, else Select.
    pub fn on(self, kind: LayerKind) -> Self {
        match self {
            _ if self.works_on(kind) => self,
            Self::Brush => Self::Pencil,
            Self::Pencil => Self::Brush,
            _ => Self::Select,
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
    /// Drops picks that no longer exist, as after deleting a layer, and
    /// picks on a locked or hidden layer.
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
    /// The corner `start` follows the pointer's movement from `press`,
    /// with the opposite corner `pivot` fixed.
    Scale {
        pivot: Vec2,
        start: Vec2,
        press: Vec2,
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
    /// Pencil or eraser pixels, painted with `value` as the pointer moves
    /// on from `last`.
    Pixels {
        layer: usize,
        last: Vec2,
        value: Option<PaletteIndex>,
        width: f32,
    },
    Box {
        start: Vec2,
        end: Vec2,
        tool: Tool,
    },
}

/// Everything the view needs from the app for one frame.
pub struct Frame<'a> {
    pub editor: &'a mut Editor,
    pub previews: &'a mut Previews,
    pub texture: TextureId,
    pub variant: usize,
    pub tool: Tool,
    pub paint: Paint,
    pub selection: &'a mut Selection,
    pub show_uv: bool,
    /// Whether the view takes pointer and key input; not while a dialog is
    /// open.
    pub input: bool,
    /// Set when the view refuses an edit, to show the user.
    pub message: &'a mut Option<String>,
}

#[derive(Default)]
pub struct TextureView {
    /// Magnification over fitting the texture to the view.
    zoom: f32,
    /// Offset of the image from the view's center, in screen points.
    pan: egui::Vec2,
    drag: Option<Drag>,
    /// The layer and shapes a transform drag edits, fixed when it starts.
    targets: (usize, Vec<usize>),
    /// Anchors of the pen path being drawn.
    pen: Vec<Anchor>,
    /// Whether a text field had the keyboard when the view was last shown.
    typing: bool,
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
    /// Whether a drag is in progress, during which nothing else should
    /// change the project.
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Drops the pen path being drawn, as when the tool or texture
    /// changes. No drag is in progress then: controls and shortcuts wait
    /// for drags to end.
    pub fn clear_pen(&mut self) {
        self.pen.clear();
    }

    /// Cancels a drag in progress: a drawing is dropped, the pen's path
    /// with it, and a transform's edits are rolled back.
    fn cancel_drag(&mut self, editor: &mut Editor) {
        match self.drag.take() {
            Some(Drag::PenHandle) => self.pen.clear(),
            Some(_) => {}
            None => return,
        }
        editor.revert_gesture(CANVAS_DRAG);
    }

    pub fn show(&mut self, ui: &mut egui::Ui, f: Frame<'_>) {
        if self.zoom == 0.0 {
            self.zoom = 1.0;
        }
        // A key that ends a text field's edit, such as Enter or Escape, was
        // typed into the field, though the field has let go of the keyboard
        // by now.
        let now = ui.ctx().text_edit_focused();
        let typing = std::mem::replace(&mut self.typing, now) || now;
        let r = VariantRef {
            texture: f.texture,
            variant: f.variant,
        };
        let tex = f.editor.project().texture(f.texture);
        let (size, editable) = (tex.size().as_vec2(), !tex.variants().is_empty());
        if editable {
            f.selection.fit(f.editor.project(), r);
        }
        let handle = f
            .previews
            .variant(ui.ctx(), f.editor.project(), f.editor.generation(), r);

        // Clicks and drags without keyboard focus, which would stop the
        // app's keys.
        let (rect, response) =
            ui.allocate_exact_size(ui.available_size(), Sense::CLICK | Sense::DRAG);
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
        checkerboard(&painter, image, (scale * 2.0).max(8.0));
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
        if !editable {
            return;
        }
        if f.input {
            self.interact(ui, &response, at, f, r, typing);
        } else {
            self.cancel_drag(f.editor);
            self.pen.clear();
            self.overlay(
                ui,
                f.editor.project(),
                r,
                f.selection,
                at,
                None,
                f.tool,
                f.paint,
            );
        }
    }

    /// Shows the whole texture, as large as fits the view.
    pub fn fit(&mut self) {
        self.zoom = 1.0;
        self.pan = egui::Vec2::ZERO;
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
            self.fit();
        }
    }

    fn interact(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        at: Placement,
        f: Frame<'_>,
        r: VariantRef,
        typing: bool,
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

        // A click ending a drag (egui can report both in one frame) is part
        // of the drag, not a click.
        let was_dragging = self.drag.is_some();
        // A drag, by the primary button only, starts where the button went
        // down.
        if response.drag_started_by(PointerButton::Primary)
            && let Some(origin) = ui
                .input(|i| i.pointer.press_origin())
                .map(|p| at.to_texture(p))
        {
            // A canvas drag ends any other control's open edit and takes
            // keyboard focus from it and closes popups such as the color
            // picker, so nothing typed mid-drag reaches them.
            editor.end_gesture();
            ui.memory_mut(|m| {
                if let Some(id) = m.focused() {
                    m.surrender_focus(id);
                }
            });
            egui::Popup::close_all(ui.ctx());
            self.drag = self.start(editor, tool, paint, selection, r, origin, at, message);
            self.targets = (selection.layer, selection.shapes.clone());
        } else if response.clicked()
            && !was_dragging
            && let Some(p) = pointer
        {
            match tool {
                Tool::Pen => self.pen_click(editor, paint, selection, r, p, at, message),
                Tool::Brush => {
                    add_shape(
                        editor,
                        selection,
                        r,
                        tool,
                        vec![Anchor::corner(p)],
                        paint.style(false),
                        paint.color,
                        message,
                    );
                }
                Tool::Select => {
                    // A click on the picked shape's anchor picks the nearest.
                    let anchor = selection.single().and_then(|s| {
                        let shapes = editor
                            .project()
                            .variant(r)
                            .layers()
                            .get(selection.layer)?
                            .shapes();
                        shapes[s]
                            .anchors()
                            .iter()
                            .enumerate()
                            .map(|(k, a)| (k, a.point.distance(p)))
                            .filter(|&(_, d)| d <= at.hit())
                            .min_by(|a, b| a.1.total_cmp(&b.1))
                            .map(|(k, _)| k)
                    });
                    match anchor {
                        Some(k) if !shift => selection.anchor = Some(k),
                        _ => pick(editor.project(), r, selection, p, at, shift),
                    }
                }
                Tool::Pencil | Tool::Eraser | Tool::Fill => {
                    press_pixels(editor, selection, r, tool, paint, p, None, message);
                }
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
            continue_drag(editor, drag, &self.targets, r, p, &mut self.pen, shift);
        }
        let escape = ui.input(|i| i.key_pressed(Key::Escape));
        let cancelled = escape && self.drag.is_some();
        if cancelled {
            self.cancel_drag(editor);
        } else if response.drag_stopped() {
            // Only the primary button's release finishes a drag; any other
            // end cancels it, decided by the frame's first release.
            let first_release = ui.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::PointerButton {
                        button,
                        pressed: false,
                        ..
                    } => Some(*button),
                    _ => None,
                })
            });
            if first_release == Some(PointerButton::Primary) {
                if let Some(drag) = self.drag.take() {
                    finish_drag(editor, drag, selection, r, paint, message);
                }
                editor.end_gesture();
            } else {
                self.cancel_drag(editor);
            }
        }

        // Keys, while the view has the pointer.
        // Keys a text field is using are its own, and none act mid-drag.
        if (response.hovered() || !self.pen.is_empty()) && !typing && self.drag.is_none() {
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
                    Tool::Pen,
                    anchors,
                    paint.style(false),
                    paint.color,
                    message,
                );
            }
            if escape && !cancelled {
                if self.pen.is_empty() {
                    selection.shapes.clear();
                    selection.anchor = None;
                } else {
                    self.pen.clear();
                }
            }
            if delete {
                // The pen removes its path's last anchor; only Select
                // deletes picked artwork.
                match tool {
                    Tool::Pen => {
                        self.pen.pop();
                    }
                    Tool::Select => delete_selected(editor, r, selection),
                    _ => {}
                }
            }
        }

        self.overlay(ui, editor.project(), r, selection, at, pointer, tool, paint);
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
        if matches!(tool, Tool::Brush | Tool::Ellipse | Tool::Rectangle)
            && let Err(why) = drawable(editor.project(), r, selection)
        {
            *message = Some(why);
            return None;
        }
        match tool {
            Tool::Brush => Some(Drag::Brush(vec![p])),
            Tool::Pencil | Tool::Eraser => {
                let edit = Some(CANVAS_DRAG);
                press_pixels(editor, selection, r, tool, paint, p, edit, message).map(|layer| {
                    Drag::Pixels {
                        layer,
                        last: p,
                        value: pixel_value(tool, paint),
                        width: paint.width,
                    }
                })
            }
            // A fill acts at once, where the press began.
            Tool::Fill => {
                press_pixels(editor, selection, r, tool, paint, p, None, message);
                None
            }
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
        if let Err(why) = drawable(editor.project(), r, selection) {
            *message = Some(why);
            self.pen.clear();
            return;
        }
        if self.pen.len() >= 2 && self.pen[0].point.distance(p) <= at.hit() {
            let anchors = std::mem::take(&mut self.pen);
            add_shape(
                editor,
                selection,
                r,
                Tool::Pen,
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
        paint: Paint,
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
        // The pixels a pencil or eraser tip would cover.
        if matches!(tool, Tool::Pencil | Tool::Eraser)
            && let Some(p) = cursor
        {
            let shade = Color32::from_rgba_unmultiplied(255, 255, 255, 70);
            let size = project.variant(r).size();
            let inside = |&(x, y): &(i64, i64)| mint_core::texel_index(size, x, y).is_some();
            for (x, y) in mint_core::pencil_tip(p, paint.width)
                .into_iter()
                .filter(inside)
            {
                let min = Vec2::new(x as f32, y as f32);
                let rect = Rect::from_min_max(at.to_screen(min), at.to_screen(min + Vec2::ONE));
                painter.rect_filled(rect, 0.0, shade);
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

/// Picks the topmost shape under `p` in a visible, unlocked layer, unless a
/// set pixel of such a raster layer, at some opacity, covers it, or with
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
    let mut hit = None;
    for (li, l) in layers.iter().enumerate().rev() {
        if !l.visible || l.locked {
            continue;
        }
        // Pixels painted over a shape hide it from picks too, unless their
        // layer shows at no opacity.
        if l.opacity > 0.0 && l.raster_pixels().is_some_and(|px| px.at(p).is_some()) {
            break;
        }
        if let Some(si) = l.shapes().iter().rposition(|s| hits(s, p, at.hit())) {
            hit = Some((li, si));
            break;
        }
    }
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
        // The nearest anchor or handle within reach.
        let nearest = s
            .anchors()
            .iter()
            .enumerate()
            .flat_map(|(k, a)| {
                [
                    (k, Part::Point, a.point),
                    (k, Part::In, a.point + a.handle_in),
                    (k, Part::Out, a.point + a.handle_out),
                ]
                .into_iter()
                .filter(move |&(_, part, q)| part == Part::Point || q != a.point)
            })
            .map(|(k, part, q)| (k, part, q.distance(p)))
            .filter(|&(_, _, d)| d <= at.hit())
            .min_by(|a, b| a.2.total_cmp(&b.2));
        if let Some((k, part, _)) = nearest {
            selection.anchor = Some(k);
            return Some(Drag::Anchor {
                anchor: k,
                part,
                original: s.clone(),
            });
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
                press: p,
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

/// Replaces a drag's target shapes, `(layer, indices)`, with `shapes`, in
/// the same order.
fn replace_targets(
    editor: &mut Editor,
    r: VariantRef,
    (layer, picked): &(usize, Vec<usize>),
    shapes: Vec<Shape>,
) {
    let (layer, picked) = (*layer, picked.clone());
    // A drag's edits undo as one step.
    let _ = editor.edit_variant_continuous(CANVAS_DRAG, r, |v| {
        let Some(l) = v.layers_mut().get_mut(layer) else {
            return;
        };
        let slots = l.shapes_mut();
        for (&i, s) in picked.iter().zip(shapes) {
            if let Some(slot) = slots.get_mut(i) {
                *slot = s;
            }
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
    targets: &(usize, Vec<usize>),
    r: VariantRef,
    p: Vec2,
    pen: &mut [Anchor],
    shift: bool,
) {
    match drag {
        Drag::Move { start, original } => {
            let t = Affine2::from_translation(p - *start);
            replace_targets(editor, r, targets, transformed(original, t));
        }
        Drag::Scale {
            pivot,
            start,
            press,
            original,
        } => {
            let from = *start - *pivot;
            let to = *start + (p - *press) - *pivot;
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
            replace_targets(editor, r, targets, transformed(original, t));
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
            replace_targets(editor, r, targets, transformed(original, t));
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
            replace_targets(editor, r, targets, vec![s]);
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
        Drag::Pixels {
            layer,
            last,
            value,
            width,
        } => {
            if p == *last {
                return;
            }
            let (from, layer, value, width) = (*last, *layer, *value, *width);
            *last = p;
            let _ = editor.edit_variant_continuous(CANVAS_DRAG, r, |v| {
                if let Some(raster) = v
                    .layers_mut()
                    .get_mut(layer)
                    .and_then(Layer::raster_pixels_mut)
                {
                    raster.draw_line(from, p, width, value);
                }
            });
        }
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
                Tool::Brush,
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
                tool,
                box_anchors(tool, start, end),
                style,
                paint.color,
                message,
            );
        }
        _ => {}
    }
}

/// Whether new shapes can go on the selected layer, or why not.
fn drawable(project: &Project, r: VariantRef, selection: &Selection) -> Result<(), String> {
    match project.variant(r).layers().get(selection.layer) {
        Some(l) if l.locked => Err(format!("{} is locked; unlock it to draw on it.", l.name)),
        Some(l) if !l.visible => Err(format!("{} is hidden; show it to draw on it.", l.name)),
        _ => Ok(()),
    }
}

/// What the pencil, eraser, or fill sets pixels to.
fn pixel_value(tool: Tool, paint: Paint) -> Option<PaletteIndex> {
    (tool != Tool::Eraser).then_some(paint.color)
}

/// Edits the picked layer with `f` for `tool`, or a new layer of the kind
/// `tool` draws on if the variant has none, as a step of its own or as part
/// of the continuous edit `continuous`, and picks that layer. Refuses a
/// locked or hidden layer, or one `tool` doesn't draw on.
fn edit_picked_layer<T>(
    editor: &mut Editor,
    selection: &mut Selection,
    r: VariantRef,
    tool: Tool,
    continuous: Option<&str>,
    f: impl FnOnce(&mut Layer) -> T,
) -> Result<T, String> {
    drawable(editor.project(), r, selection)?;
    let layers = editor.project().variant(r).layers();
    let layer = if layers.is_empty() {
        0
    } else {
        selection.layer
    };
    if let Some(l) = layers.get(layer)
        && !tool.works_on(l.kind())
    {
        let kind = match l.kind() {
            LayerKind::Vector => "vector",
            LayerKind::Raster => "raster",
        };
        let tool = tool.label().to_lowercase();
        return Err(format!("The {tool} doesn't draw on {kind} layers."));
    }
    let edit = |v: &mut mint_core::Variant| {
        if v.layers().is_empty() {
            let first = if tool.works_on(LayerKind::Vector) {
                Layer::new("Layer 1")
            } else {
                Layer::raster("Layer 1", Raster::clear(v.size()))
            };
            v.insert_layer(0, first);
        }
        f(&mut v.layers_mut()[layer])
    };
    let out = match continuous {
        Some(key) => editor.edit_variant_continuous(key, r, edit),
        None => editor.edit_variant(r, edit),
    };
    let out = out.map_err(|e| e.to_string())?;
    selection.layer = layer;
    Ok(out)
}

/// A pencil or eraser dot, or a fill, at `p` on the picked raster layer, as
/// [`edit_picked_layer`] makes it. Returns the layer painted, or reports why
/// it couldn't be. A click (with no `continuous` edit) that touches no pixel
/// of the texture does nothing.
#[allow(clippy::too_many_arguments)]
fn press_pixels(
    editor: &mut Editor,
    selection: &mut Selection,
    r: VariantRef,
    tool: Tool,
    paint: Paint,
    p: Vec2,
    continuous: Option<&str>,
    message: &mut Option<String>,
) -> Option<usize> {
    let value = pixel_value(tool, paint);
    // A click off the texture paints nothing, so it adds no first layer
    // either. A drag that starts off it still paints where it enters.
    let size = editor.project().variant(r).size();
    // A fill starts from the pixel under `p`, a width-1 tip.
    let width = if tool == Tool::Fill { 1.0 } else { paint.width };
    let touches = mint_core::pencil_tip(p, width)
        .iter()
        .any(|&(x, y)| mint_core::texel_index(size, x, y).is_some());
    if continuous.is_none() && !touches {
        return None;
    }
    let painted = edit_picked_layer(editor, selection, r, tool, continuous, |l| {
        let Some(raster) = l.raster_pixels_mut() else {
            return;
        };
        if tool == Tool::Fill {
            raster.fill(p, value);
        } else {
            raster.draw_line(p, p, paint.width, value);
        }
    });
    match painted {
        Ok(()) => Some(selection.layer),
        Err(why) => {
            *message = Some(why);
            None
        }
    }
}

/// Adds a shape drawn with `tool` on top of the picked layer, creating a
/// first layer if the variant has none, and picks it. On a raster layer, the
/// shape is painted into its pixels instead.
#[allow(clippy::too_many_arguments)]
pub fn add_shape(
    editor: &mut Editor,
    selection: &mut Selection,
    r: VariantRef,
    tool: Tool,
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
    let added = edit_picked_layer(editor, selection, r, tool, None, |l| {
        if let Some(raster) = l.raster_pixels_mut() {
            raster.paint_shape(&shape);
            return None;
        }
        l.insert_shape(l.shapes().len(), shape);
        Some(l.shapes().len() - 1)
    });
    match added {
        Ok(index) => {
            selection.shapes = index.into_iter().collect();
            selection.anchor = None;
        }
        Err(why) => *message = Some(why),
    }
}

/// Deletes the picked anchor, or the picked shapes. A shape's last anchor
/// takes the shape with it.
pub fn delete_selected(editor: &mut Editor, r: VariantRef, selection: &mut Selection) {
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
    // The nearest of 32 samples per segment, refined twice around itself.
    let nearest = |c: [Vec2; 4], lo: f32, hi: f32| {
        (0..=32)
            .map(|k| {
                let t = lo + (hi - lo) * k as f32 / 32.0;
                (t, cubic(c, t).distance(p))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .expect("samples exist")
    };
    let (seg, t, d) = s
        .segments()
        .enumerate()
        .map(|(i, c)| {
            let (mut t, mut d) = nearest(c, 0.0, 1.0);
            for step in [1.0 / 32.0, 1.0 / 1024.0] {
                (t, d) = nearest(c, (t - step).max(0.0), (t + step).min(1.0));
            }
            (i, t, d)
        })
        .min_by(|a, b| a.2.total_cmp(&b.2))?;
    // An anchor itself isn't a place to split.
    if t <= 0.0 || t >= 1.0 {
        return None;
    }
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

#[cfg(test)]
mod tests {
    use glam::UVec2;
    use mint_core::{ExportTarget, Model, Palette, Pose, Rgba8, Slot, Texture, Transform, Variant};

    use super::*;

    /// An editor on a project with one 4x4 texture whose one variant has
    /// no layers.
    fn layerless() -> (Editor, VariantRef) {
        let size = UVec2::new(4, 4);
        let texture =
            Texture::painted("t", Slot("body".into()), size, [Variant::new("1", size)]).unwrap();
        let project = Project::new(
            ExportTarget::new("test"),
            Palette::from_colors([Rgba8::new(0, 0, 0, 255)]).unwrap(),
            Model::new("root", Transform::default()),
            vec![texture],
            Vec::new(),
            Pose::default(),
        )
        .unwrap();
        let r = VariantRef {
            texture: TextureId(0),
            variant: 0,
        };
        (Editor::new(project, None), r)
    }

    #[test]
    fn clicks_off_the_texture_add_no_layer() {
        let (mut editor, r) = layerless();
        let (mut selection, mut message) = (Selection::default(), None);
        let paint = Paint {
            color: PaletteIndex(0),
            fill: true,
            width: 1.0,
        };
        let margin = Vec2::new(-0.5, 1.5);
        for tool in [Tool::Pencil, Tool::Eraser, Tool::Fill] {
            let pressed = press_pixels(
                &mut editor,
                &mut selection,
                r,
                tool,
                paint,
                margin,
                None,
                &mut message,
            );
            assert_eq!(pressed, None, "{tool:?}");
        }
        assert!(editor.project().variant(r).layers().is_empty());
        assert!(!editor.can_undo());

        // A wide tip hanging over the edge reaches the texture.
        let wide = Paint {
            width: 3.0,
            ..paint
        };
        let pressed = press_pixels(
            &mut editor,
            &mut selection,
            r,
            Tool::Pencil,
            wide,
            margin,
            None,
            &mut message,
        );
        assert_eq!(pressed, Some(0));
        let layers = editor.project().variant(r).layers();
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].kind(), LayerKind::Raster);
    }

    #[test]
    fn tools_follow_the_picked_layer_kind() {
        use LayerKind::{Raster, Vector};
        // Brush and Pencil trade places; tools with no counterpart fall
        // back to Select; tools that draw on both kinds stay.
        let cases = [
            (Tool::Brush, Raster, Tool::Pencil),
            (Tool::Pencil, Vector, Tool::Brush),
            (Tool::Eraser, Vector, Tool::Select),
            (Tool::Fill, Vector, Tool::Select),
            (Tool::Pen, Raster, Tool::Pen),
            (Tool::Ellipse, Raster, Tool::Ellipse),
            (Tool::Rectangle, Vector, Tool::Rectangle),
            (Tool::Select, Raster, Tool::Select),
        ];
        for (tool, kind, expected) in cases {
            assert_eq!(tool.on(kind), expected, "{tool:?} on {kind:?}");
        }
        for tool in Tool::ALL {
            for kind in [Vector, Raster] {
                assert!(tool.on(kind).works_on(kind), "{tool:?} on {kind:?}");
            }
        }
    }
}
