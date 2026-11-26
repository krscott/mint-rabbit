//! The Texture View: one texture variant, pixel-exact, with its shapes drawn
//! as outlines and edited with the drawing tools.

use eframe::egui::{
    self, Color32, Key, KeyboardShortcut, Modifiers, PointerButton, Pos2, Rect, Sense, Stroke,
    pos2, vec2,
};
use glam::{Affine2, Vec2};
use mint_core::{
    Anchor, Layer, LayerKind, ModifierKind, PaletteIndex, Project, Raster, Shape, Style, TextureId,
    VariantRef, shape,
};

use crate::editor::Editor;
use crate::previews::{Previews, checkerboard};
use crate::scene;
use crate::stamps::Placing;

/// Pointer distance, in screen points, within which handles and outlines
/// are hit.
const HIT: f32 = 7.0;
/// The key of a canvas drag's continuous edit.
const CANVAS_DRAG: &str = "canvas drag";
/// Freehand simplification tolerance in texels.
const BRUSH_TOLERANCE: f32 = 0.25;
/// Room, in screen points, left around the fitted image for the handles of
/// modifier guides.
const GUIDE_MARGIN: f32 = 16.0;
/// How far outside the image a mirror line's handles sit, in screen points.
const GUIDE_REACH: f32 = 9.0;

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
    Stamp,
}

impl Tool {
    pub const ALL: [Self; 9] = [
        Self::Select,
        Self::Pen,
        Self::Brush,
        Self::Ellipse,
        Self::Rectangle,
        Self::Pencil,
        Self::Eraser,
        Self::Fill,
        Self::Stamp,
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
            Self::Stamp => "Stamp",
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
            Self::Stamp => key(Key::S),
        }
    }

    /// Whether the tool draws on a layer of `kind`. The shape tools and
    /// Stamp paint their shapes into a raster layer's pixels.
    pub fn works_on(self, kind: LayerKind) -> bool {
        match self {
            Self::Select | Self::Pen | Self::Ellipse | Self::Rectangle | Self::Stamp => true,
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
    /// A pick of the top layer of `r`, the first a variant gets when it is
    /// shown, and none of its shapes. An external texture has no layers to
    /// pick.
    pub fn top(project: &Project, r: VariantRef) -> Self {
        let layers = project
            .texture(r.texture)
            .variants()
            .get(r.variant)
            .map_or(0, |v| v.layers().len());
        Self {
            layer: layers.saturating_sub(1),
            ..Self::default()
        }
    }

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
    /// A freehand stroke through the points of each of `runs`, a shape per
    /// run, drawn into the picked layer as it goes; `before` is that layer
    /// as it was, once the stroke is drawn. A stroke painted on the model
    /// breaks into runs where it leaves a UV island.
    Brush {
        runs: Vec<Vec<Vec2>>,
        before: Option<Layer>,
    },
    /// Pencil or eraser pixels, painted with `value` as the pointer moves
    /// on from `last`.
    Pixels {
        layer: usize,
        last: Vec2,
        value: Option<PaletteIndex>,
        width: f32,
    },
    /// An ellipse or rectangle from corner `start` to `end`, drawn like a
    /// brush stroke.
    Box {
        start: Vec2,
        end: Vec2,
        tool: Tool,
        before: Option<Layer>,
    },
    /// A modifier guide of layer `layer` being moved, `offset` texels from
    /// the pointer along its axis, as where it was grabbed.
    Guide {
        layer: usize,
        modifier: usize,
        part: GuidePart,
        offset: f32,
    },
}

/// The line of a modifier guide that a handle moves: the vertical line (X)
/// or the horizontal one (Y), numbered as [`ModifierKind::guides`] gives
/// them. A radial center is where the two cross.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GuidePart {
    X = 0,
    Y = 1,
}

/// A handle of a modifier guide, on screen.
struct GuideHandle {
    modifier: usize,
    part: GuidePart,
    at: Pos2,
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
    /// Where the pointer over the Model View meets this texture, to show
    /// the Ghost Cursor at.
    pub ghost: Option<Vec2>,
    /// Whether the pointer is over the Model View, whose keys reach the
    /// tools too.
    pub model_hovered: bool,
    /// The stamp the Stamp tool places, once one is chosen.
    pub stamp: Option<&'a mut Placing>,
}

/// A view the tools take pointer input from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum View {
    Texture,
    Model,
}

/// A point of the pointer's path, in texture space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub point: Vec2,
    /// Whether the path left the texture or crossed onto another UV island
    /// before `point`, so a stroke breaks there.
    pub jump: bool,
}

/// The pointer over the Model View, where it meets the texture the Texture
/// View shows.
#[derive(Clone, Debug)]
pub struct ModelPointer {
    /// Where the press that started a drag landed, if on the texture.
    pub press: Option<Vec2>,
    /// Where the pointer is now, if on the texture.
    pub here: Option<Vec2>,
    /// The pointer's path since the last frame, while dragging.
    pub path: Vec<Sample>,
    /// Texels per screen point where the pointer is.
    pub texels_per_point: f32,
}

#[derive(Default)]
pub struct TextureView {
    /// Magnification over fitting the texture to the view.
    zoom: f32,
    /// Offset of the image from the view's center, in screen points.
    pan: egui::Vec2,
    /// The drag in progress, with the view it started in, which moves and
    /// ends it.
    drag: Option<(View, Drag)>,
    /// The layer and shapes a transform drag edits, fixed when it starts.
    targets: (usize, Vec<usize>),
    /// Anchors of the pen path being drawn.
    pen: Vec<Anchor>,
    /// Whether a text field had the keyboard when the view was last shown.
    typing: bool,
    /// The texture point under the pointer, while it is over the view.
    hover: Option<Vec2>,
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

    /// The texture point under the pointer, while it is over the view.
    pub fn hover(&self) -> Option<Vec2> {
        self.hover
    }

    /// Whether a pen path is open.
    pub fn drawing_path(&self) -> bool {
        !self.pen.is_empty()
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
            Some((_, Drag::PenHandle)) => self.pen.clear(),
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
        self.hover = None;
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
        let room = rect.size() - egui::Vec2::splat(2.0 * GUIDE_MARGIN);
        let fit = (room.x / size.x).min(room.y / size.y).max(1.0);
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
                f.stamp.as_deref(),
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
            ghost,
            model_hovered,
            stamp,
            ..
        } = f;
        if tool != Tool::Pen && !self.pen.is_empty() {
            self.pen.clear();
        }
        let pointer = response
            .interact_pointer_pos()
            .or_else(|| response.hover_pos())
            .map(|p| at.to_texture(p));
        self.hover = response.hover_pos().map(|p| at.to_texture(p));
        let shift = ui.input(|i| i.modifiers.shift);

        // A click ending a drag (egui can report both in one frame) is part
        // of the drag, not a click.
        let was_dragging = self.drag.is_some();
        let mut canvas = Canvas {
            editor,
            selection,
            paint,
            message,
            stamp,
        };
        // A drag, by the primary button only, starts where the button went
        // down.
        if response.drag_started_by(PointerButton::Primary)
            && let Some(origin) = ui
                .input(|i| i.pointer.press_origin())
                .map(|p| at.to_texture(p))
        {
            take_keyboard(ui, canvas.editor);
            // Any tool moves the picked layer's modifier guides.
            let (project, layer) = (canvas.editor.project(), canvas.selection.layer);
            let drag = if let Some(guide) = guide_drag(project, r, layer, origin, at) {
                Some(guide)
            } else if tool == Tool::Select {
                start_select(project, r, canvas.selection, origin, at)
            } else {
                self.start(canvas.reborrow(), tool, r, origin, at.hit())
            };
            self.drag = drag.map(|d| (View::Texture, d));
            self.targets = (canvas.selection.layer, canvas.selection.shapes.clone());
        } else if response.clicked()
            && !was_dragging
            && let Some(p) = pointer
            && guide_hit(
                canvas.editor.project(),
                r,
                canvas.selection.layer,
                at,
                at.to_screen(p),
            )
            .is_none()
        {
            // A click on the picked shape's anchor picks the nearest.
            let anchor = (tool == Tool::Select && !shift)
                .then(|| anchor_under(canvas.editor.project(), r, canvas.selection, p, at.hit()))
                .flatten();
            if let Some(k) = anchor {
                canvas.selection.anchor = Some(k);
            } else {
                self.click(canvas.reborrow(), tool, r, p, at.hit(), shift);
            }
        }
        let Canvas {
            editor, selection, ..
        } = canvas.reborrow();
        if response.double_clicked()
            && tool == Tool::Select
            && let (Some(p), Some(s)) = (pointer, selection.single())
            && guide_hit(editor.project(), r, selection.layer, at, at.to_screen(p)).is_none()
            && let Some(k) = insert_anchor(editor, r, selection.layer, s, p, at)
        {
            selection.anchor = Some(k);
        }

        if let (Some((View::Texture, drag)), Some(p)) = (&mut self.drag, pointer) {
            let path = [Sample {
                point: p,
                jump: false,
            }];
            continue_drag(
                canvas.reborrow(),
                drag,
                &self.targets,
                r,
                &path,
                &mut self.pen,
                shift,
            );
        }
        // The pointer shows which way a guide under it moves.
        let (editor, selection) = (&mut *canvas.editor, &mut *canvas.selection);
        let part = match &self.drag {
            Some((_, Drag::Guide { part, .. })) => Some(*part),
            Some(_) => None,
            None => response
                .hover_pos()
                .and_then(|q| guide_hit(editor.project(), r, selection.layer, at, q))
                .map(|h| h.part),
        };
        if let Some(part) = part {
            ui.ctx().set_cursor_icon(match part {
                GuidePart::X => egui::CursorIcon::ResizeHorizontal,
                GuidePart::Y => egui::CursorIcon::ResizeVertical,
            });
        }
        self.release(ui, response, View::Texture, editor, selection, r);
        let hovered = response.hovered() || model_hovered;
        self.keys(ui, canvas.reborrow(), r, tool, hovered, typing);

        let cursor = pointer.or(ghost);
        let project = canvas.editor.project();
        let stamp = canvas.stamp.as_deref();
        self.overlay(
            ui,
            project,
            r,
            canvas.selection,
            at,
            cursor,
            tool,
            paint,
            stamp,
        );
    }

    /// Input from the Model View, already in the space of `r`, the variant
    /// the Texture View shows: the drawing tools draw where the pointer
    /// meets it on the model, and Select picks there. Keys are left to
    /// [`Self::show`].
    pub fn model_interact(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        mut canvas: Canvas<'_>,
        r: VariantRef,
        tool: Tool,
        pointer: &ModelPointer,
    ) {
        // The Texture View fits the picks later in the frame; tools here
        // need them fitted now.
        canvas.selection.fit(canvas.editor.project(), r);
        if tool != Tool::Pen && !self.pen.is_empty() {
            self.pen.clear();
        }
        let shift = ui.input(|i| i.modifiers.shift);
        let reach = HIT * pointer.texels_per_point;
        let was_dragging = self.drag.is_some();
        if response.drag_started_by(PointerButton::Primary)
            && tool != Tool::Select
            && let Some(p) = pointer.press
        {
            take_keyboard(ui, canvas.editor);
            let drag = self.start(canvas.reborrow(), tool, r, p, reach);
            self.drag = drag.map(|d| (View::Model, d));
            self.targets = (canvas.selection.layer, canvas.selection.shapes.clone());
        } else if response.clicked()
            && !was_dragging
            && let Some(p) = pointer.here
        {
            self.click(canvas.reborrow(), tool, r, p, reach, shift);
        }
        if let Some((View::Model, drag)) = &mut self.drag {
            let path = &pointer.path;
            continue_drag(
                canvas.reborrow(),
                drag,
                &self.targets,
                r,
                path,
                &mut self.pen,
                shift,
            );
        }
        self.release(
            ui,
            response,
            View::Model,
            canvas.editor,
            canvas.selection,
            r,
        );
    }

    /// Ends a drag that started in `view`, whose `response` this is, when
    /// the pointer lets go of it: the primary button's release finishes it,
    /// and any other end cancels it, decided by the frame's first release.
    /// Escape cancels it first, even with a release in the same frame, and
    /// is used up doing so.
    fn release(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        view: View,
        editor: &mut Editor,
        selection: &mut Selection,
        r: VariantRef,
    ) {
        if !matches!(self.drag, Some((v, _)) if v == view) {
            return;
        }
        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.cancel_drag(editor);
            return;
        }
        if !response.drag_stopped() {
            return;
        }
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
            if let Some((_, drag)) = self.drag.take() {
                finish_drag(editor, drag, selection, r);
            }
            editor.end_gesture();
        } else {
            self.cancel_drag(editor);
        }
    }

    /// The tools' keys, while no drag is in progress, no text field has
    /// the keyboard, and the pointer is over either view (`hovered`) or a
    /// pen path is open: Enter finishes the path, Escape drops it or else
    /// the picks, and Delete removes the path's last anchor or, with Select,
    /// the picked artwork.
    fn keys(
        &mut self,
        ui: &egui::Ui,
        mut canvas: Canvas<'_>,
        r: VariantRef,
        tool: Tool,
        hovered: bool,
        typing: bool,
    ) {
        if self.drag.is_some() || typing || (!hovered && self.pen.is_empty()) {
            return;
        }
        let escape = ui.input(|i| i.key_pressed(Key::Escape));
        let (enter, delete) = ui.input(|i| {
            (
                i.key_pressed(Key::Enter),
                i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace),
            )
        });
        if enter && !self.pen.is_empty() {
            // Enter finishes an open path; clicking the first anchor closes
            // one.
            let anchors = std::mem::take(&mut self.pen);
            add_shape(canvas.reborrow(), r, Tool::Pen, anchors, false);
        }
        let selection = &mut *canvas.selection;
        if escape {
            if self.pen.is_empty() {
                selection.shapes.clear();
                selection.anchor = None;
            } else {
                self.pen.clear();
            }
        }
        if delete {
            // The pen removes its path's last anchor; only Select deletes
            // picked artwork.
            match tool {
                Tool::Pen => {
                    self.pen.pop();
                }
                Tool::Select => delete_selected(canvas.editor, r, selection),
                _ => {}
            }
        }
    }

    /// A click of `tool` at `p`: a pen anchor, a brush dot, pixels, a
    /// stamp, or a pick. `reach` is how near, in texels, a click finds what it's near.
    fn click(
        &mut self,
        canvas: Canvas<'_>,
        tool: Tool,
        r: VariantRef,
        p: Vec2,
        reach: f32,
        add: bool,
    ) {
        match tool {
            Tool::Pen => self.pen_click(canvas, r, p, reach),
            Tool::Brush => add_shape(canvas, r, tool, vec![Anchor::corner(p)], false),
            Tool::Select => pick(canvas.editor.project(), r, canvas.selection, p, reach, add),
            Tool::Pencil | Tool::Eraser | Tool::Fill => {
                press_pixels(canvas, r, tool, p, None);
            }
            Tool::Stamp => place_stamp(canvas, r, p),
            Tool::Ellipse | Tool::Rectangle => {}
        }
    }

    /// Starts a drag of a drawing tool pressed at `p`.
    fn start(
        &mut self,
        canvas: Canvas<'_>,
        tool: Tool,
        r: VariantRef,
        p: Vec2,
        reach: f32,
    ) -> Option<Drag> {
        if matches!(tool, Tool::Brush | Tool::Ellipse | Tool::Rectangle)
            && let Err(why) = drawable(canvas.editor.project(), r, canvas.selection)
        {
            *canvas.message = Some(why);
            return None;
        }
        match tool {
            Tool::Brush => {
                let mut drag = Drag::Brush {
                    runs: vec![vec![p]],
                    before: None,
                };
                redraw(canvas, &mut drag, r);
                Some(drag)
            }
            Tool::Pencil | Tool::Eraser => {
                let paint = canvas.paint;
                press_pixels(canvas, r, tool, p, Some(CANVAS_DRAG)).map(|layer| Drag::Pixels {
                    layer,
                    last: p,
                    value: pixel_value(tool, paint),
                    width: paint.width,
                })
            }
            // A fill or stamp acts at once, where the press began.
            Tool::Fill => {
                press_pixels(canvas, r, tool, p, None);
                None
            }
            Tool::Stamp => {
                place_stamp(canvas, r, p);
                None
            }
            Tool::Ellipse | Tool::Rectangle => Some(Drag::Box {
                start: p,
                end: p,
                tool,
                before: None,
            }),
            Tool::Pen => {
                self.pen_click(canvas, r, p, reach);
                (!self.pen.is_empty()).then_some(Drag::PenHandle)
            }
            Tool::Select => None,
        }
    }

    /// A pen click: closes the path on its first anchor, or adds an anchor.
    fn pen_click(&mut self, canvas: Canvas<'_>, r: VariantRef, p: Vec2, reach: f32) {
        if let Err(why) = drawable(canvas.editor.project(), r, canvas.selection) {
            *canvas.message = Some(why);
            self.pen.clear();
            return;
        }
        if self.pen.len() >= 2 && self.pen[0].point.distance(p) <= reach {
            let anchors = std::mem::take(&mut self.pen);
            add_shape(canvas, r, Tool::Pen, anchors, true);
        } else if self.pen.last().is_none_or(|a| a.point.distance(p) > reach) {
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
        stamp: Option<&Placing>,
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
        // What other tools paint around the cursor: a ring a stroke's width
        // across, or a dot.
        if !matches!(tool, Tool::Pencil | Tool::Eraser)
            && let (Some(p), Some(radius)) = (cursor, footprint(tool, paint))
        {
            let (center, radius) = (at.to_screen(p), (radius * at.scale).max(1.5));
            painter.circle_stroke(
                center,
                radius,
                Stroke::new(3.0, Color32::from_black_alpha(150)),
            );
            painter.circle_stroke(center, radius, Stroke::new(1.0, Color32::WHITE));
        }
        // Where the stamp would go, light on dark like the ring.
        if tool == Tool::Stamp
            && let (Some(p), Some(placing)) = (cursor, stamp)
        {
            let entries = vec![PaletteIndex(0); placing.stamp.colors().len()];
            for s in placing.stamp.placed(p, &entries) {
                let o = s.outline();
                line(&o, Stroke::new(3.0, Color32::from_black_alpha(150)));
                line(&o, Stroke::new(1.0, Color32::WHITE));
            }
        }
        // The pen's path in progress. Other drawings are already in the
        // layer.
        let preview = Stroke::new(1.5, Color32::from_rgb(255, 120, 200));
        if !self.pen.is_empty() {
            let mut anchors = self.pen.clone();
            if let Some(p) = cursor
                && !matches!(self.drag, Some((_, Drag::PenHandle)))
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
        if let Some(layer) = layers.get(selection.layer) {
            draw_guides(&painter, layer, at);
        }
    }
}

/// The handles of the guides of layer `layer`'s enabled Mirror and Radial
/// modifiers, if the layer is visible: each line has one past either end of
/// the image, where the view shows it. Zoomed in past the image's edges, a
/// line has no handle there, so that nothing on the image is a handle.
fn guide_handles(layer: &Layer, at: Placement) -> Vec<GuideHandle> {
    if !layer.visible {
        return Vec::new();
    }
    let image = at.image;
    let mut out = Vec::new();
    for (modifier, m) in layer.modifiers.iter().enumerate() {
        if !m.enabled {
            continue;
        }
        let (c, [x, y]) = m.kind.guides();
        let c = at.to_screen(c.to_vec2());
        if x {
            for end in [image.top() - GUIDE_REACH, image.bottom() + GUIDE_REACH] {
                let at = pos2(c.x, end);
                out.push(GuideHandle {
                    modifier,
                    part: GuidePart::X,
                    at,
                });
            }
        }
        if y {
            for end in [image.left() - GUIDE_REACH, image.right() + GUIDE_REACH] {
                let at = pos2(end, c.y);
                out.push(GuideHandle {
                    modifier,
                    part: GuidePart::Y,
                    at,
                });
            }
        }
    }
    out.retain(|h| at.view.contains(h.at));
    out
}

/// The guide handle nearest screen point `q` within reach, among those of
/// layer `layer` if it can be edited.
fn guide_hit(
    project: &Project,
    r: VariantRef,
    layer: usize,
    at: Placement,
    q: Pos2,
) -> Option<GuideHandle> {
    let layer = project.variant(r).layers().get(layer)?;
    if layer.locked {
        return None;
    }
    // Of handles in one place, the later modifier's, as the one added or
    // moved last; the fields move the others.
    guide_handles(layer, at)
        .into_iter()
        .rev()
        .map(|h| (h.at.distance(q), h))
        .filter(|(d, _)| *d <= HIT)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, h)| h)
}

/// A drag of the guide of layer `layer` whose handle is under `p`, if any.
fn guide_drag(
    project: &Project,
    r: VariantRef,
    layer: usize,
    p: Vec2,
    at: Placement,
) -> Option<Drag> {
    let h = guide_hit(project, r, layer, at, at.to_screen(p))?;
    let axis = h.part as usize;
    let kind = project.variant(r).layers()[layer].modifiers[h.modifier].kind;
    Some(Drag::Guide {
        layer,
        modifier: h.modifier,
        part: h.part,
        offset: kind.guides().0.to_vec2()[axis] - p[axis],
    })
}

/// The anchor of the one picked shape nearest `p` within `reach` texels.
fn anchor_under(
    project: &Project,
    r: VariantRef,
    selection: &Selection,
    p: Vec2,
    reach: f32,
) -> Option<usize> {
    let shape = &project.variant(r).layers().get(selection.layer)?.shapes()[selection.single()?];
    shape
        .anchors()
        .iter()
        .enumerate()
        .map(|(k, a)| (k, a.point.distance(p)))
        .filter(|&(_, d)| d <= reach)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(k, _)| k)
}

/// Readies a canvas drag: it ends any other control's open edit, takes
/// keyboard focus from it, and closes popups such as the color picker, so
/// nothing typed mid-drag reaches them.
fn take_keyboard(ui: &egui::Ui, editor: &mut Editor) {
    editor.end_gesture();
    ui.memory_mut(|m| {
        if let Some(id) = m.focused() {
            m.surrender_focus(id);
        }
    });
    egui::Popup::close_all(ui.ctx());
}

/// The radius, in texels, of what `tool` paints around the cursor: half
/// the width of a stroke, or nothing for a fill or a point. Select paints
/// nothing, and a stamp has a footprint of its own.
pub fn footprint(tool: Tool, paint: Paint) -> Option<f32> {
    let stroke = paint.width / 2.0;
    match tool {
        Tool::Select | Tool::Stamp => None,
        Tool::Brush | Tool::Pencil | Tool::Eraser => Some(stroke),
        Tool::Pen | Tool::Ellipse | Tool::Rectangle if !paint.fill => Some(stroke),
        Tool::Pen | Tool::Ellipse | Tool::Rectangle | Tool::Fill => Some(0.0),
    }
}

/// Draws the guides of `layer`'s enabled modifiers if it is visible, faint
/// on a locked layer, whose guides don't move. A radial center is ringed.
fn draw_guides(painter: &egui::Painter, layer: &Layer, at: Placement) {
    if !layer.visible {
        return;
    }
    let alpha = if layer.locked { 90 } else { 230 };
    let color = Color32::from_rgba_unmultiplied(90, 200, 255, alpha);
    let stroke = Stroke::new(1.5, color);
    let image = at.image;
    for m in layer.modifiers.iter().filter(|m| m.enabled) {
        let (c, [x, y]) = m.kind.guides();
        let c = at.to_screen(c.to_vec2());
        if x {
            let line = [pos2(c.x, image.top()), pos2(c.x, image.bottom())];
            painter.extend(egui::Shape::dashed_line(&line, stroke, 5.0, 4.0));
        }
        if y {
            let line = [pos2(image.left(), c.y), pos2(image.right(), c.y)];
            painter.extend(egui::Shape::dashed_line(&line, stroke, 5.0, 4.0));
        }
        if let ModifierKind::Radial(_) = m.kind {
            painter.circle_stroke(c, 6.0, stroke);
        }
    }
    for h in guide_handles(layer, at) {
        // A triangle pointing at the image.
        let toward = match h.part {
            GuidePart::X if h.at.y < image.center().y => vec2(0.0, 1.0),
            GuidePart::X => vec2(0.0, -1.0),
            GuidePart::Y if h.at.x < image.center().x => vec2(1.0, 0.0),
            GuidePart::Y => vec2(-1.0, 0.0),
        };
        let side = vec2(toward.y, -toward.x) * 6.0;
        let tip = h.at + toward * 5.0;
        let back = h.at - toward * 5.0;
        painter.add(egui::Shape::convex_polygon(
            vec![tip, back + side, back - side],
            color,
            Stroke::NONE,
        ));
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

/// The topmost shape under `p` in a visible, unlocked layer, as `(layer,
/// shape)`, unless a visible layer above it, locked or not, shows a pixel
/// there (painted, or a copy) at some opacity. Within a layer, a shape
/// covering `p` or the center of the pixel under it, as it covers pixels,
/// comes first; where the layer shows nothing, one within reach of its
/// outline does.
fn shape_under(project: &Project, r: VariantRef, p: Vec2, reach: f32) -> Option<(usize, usize)> {
    let (layers, size) = (project.variant(r).layers(), project.variant(r).size());
    let pixel = mint_core::texel_index(size, p.x.floor() as i64, p.y.floor() as i64);
    let center = p.floor() + Vec2::splat(0.5);
    let covers = |s: &Shape| {
        let o = s.outline();
        o.covers(p) || (pixel.is_some() && o.covers(center))
    };
    let near = |s: &Shape| s.outline().distance(p) <= reach;
    for (li, l) in layers.iter().enumerate().rev() {
        if !l.visible {
            continue;
        }
        let shapes = if l.locked { &[][..] } else { l.shapes() };
        if let Some(si) = shapes.iter().rposition(covers) {
            return Some((li, si));
        }
        // What the layer shows hides the layers under it, pixels and copies
        // alike, unless it shows at no opacity.
        if l.opacity > 0.0 && pixel.is_some_and(|i| l.coverage(size)[i].is_some()) {
            return None;
        }
        if let Some(si) = shapes.iter().rposition(near) {
            return Some((li, si));
        }
    }
    None
}

/// Picks the [`shape_under`] `p`, or with `add`, adds or removes it from a
/// selection in the same layer.
fn pick(
    project: &Project,
    r: VariantRef,
    selection: &mut Selection,
    p: Vec2,
    reach: f32,
    add: bool,
) {
    match shape_under(project, r, p, reach) {
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
        // A press where a picked shape shows on top moves them all.
        let on_top = shape_under(project, r, p, at.hit());
        if on_top.is_some_and(|(l, s)| l == selection.layer && selection.shapes.contains(&s)) {
            return Some(Drag::Move {
                start: p,
                original: picked,
            });
        }
    }
    // Not on the selection: pick what is under the pointer and move it.
    pick(project, r, selection, p, at.hit(), false);
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

/// What the tools edit and paint with, and where they report a refused
/// edit.
pub struct Canvas<'a> {
    pub editor: &'a mut Editor,
    pub selection: &'a mut Selection,
    pub paint: Paint,
    pub message: &'a mut Option<String>,
    pub stamp: Option<&'a mut Placing>,
}

impl Canvas<'_> {
    fn reborrow(&mut self) -> Canvas<'_> {
        Canvas {
            editor: self.editor,
            selection: self.selection,
            paint: self.paint,
            message: self.message,
            stamp: self.stamp.as_deref_mut(),
        }
    }
}

/// Moves a drag along the pointer's `path` since the last frame. A brush
/// stroke or pencil line breaks where the path jumps; other drags follow
/// its end.
fn continue_drag(
    canvas: Canvas<'_>,
    drag: &mut Drag,
    targets: &(usize, Vec<usize>),
    r: VariantRef,
    path: &[Sample],
    pen: &mut [Anchor],
    shift: bool,
) {
    let Some(&Sample { point: p, .. }) = path.last() else {
        return;
    };
    match drag {
        Drag::Brush { runs, .. } => {
            let mut moved = false;
            for s in path {
                match runs.last_mut() {
                    Some(run) if !s.jump => {
                        if run.last().is_none_or(|q| q.distance(s.point) > 0.05) {
                            run.push(s.point);
                            moved = true;
                        }
                    }
                    _ => {
                        runs.push(vec![s.point]);
                        moved = true;
                    }
                }
            }
            if moved {
                redraw(canvas, drag, r);
            }
            return;
        }
        Drag::Box { end, .. } => {
            if *end != p {
                *end = p;
                redraw(canvas, drag, r);
            }
            return;
        }
        _ => {}
    }
    let editor = canvas.editor;
    match drag {
        Drag::Brush { .. } | Drag::Box { .. } => {}
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
        Drag::Guide {
            layer,
            modifier,
            part,
            offset,
        } => {
            let size = editor.project().variant(r).size();
            let (layer, modifier, axis, offset) = (*layer, *modifier, *part as usize, *offset);
            let _ = editor.edit_variant_continuous(CANVAS_DRAG, r, |v| {
                if let Some(m) = v
                    .layers_mut()
                    .get_mut(layer)
                    .and_then(|l| l.modifiers.get_mut(modifier))
                {
                    m.kind = m.kind.with_coordinate(axis, p[axis] + offset, size);
                }
            });
        }
        Drag::Pixels {
            layer,
            last,
            value,
            width,
        } => {
            if let [s] = path
                && !s.jump
                && s.point == *last
            {
                return;
            }
            let (mut from, layer, value, width) = (*last, *layer, *value, *width);
            *last = p;
            let _ = editor.edit_variant_continuous(CANVAS_DRAG, r, |v| {
                if let Some(l) = v.layers_mut().get_mut(layer) {
                    l.edit_raster(|raster| {
                        for s in path {
                            let start = if s.jump { s.point } else { from };
                            raster.draw_line(start, s.point, width, value);
                            from = s.point;
                        }
                    });
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

/// Shows a brush stroke or box as it now is: puts the picked layer back as
/// it was before the drag, then adds the drawing's shapes to it as
/// [`add_shape`] does. A box with no area adds nothing.
fn redraw(canvas: Canvas<'_>, drag: &mut Drag, r: VariantRef) {
    let Canvas {
        editor,
        selection,
        paint,
        message,
        ..
    } = canvas;
    let (tool, anchors, style, before) = match drag {
        Drag::Brush { runs, before } => (
            Tool::Brush,
            runs.iter()
                .map(|run| shape::freehand(run, BRUSH_TOLERANCE))
                .collect(),
            paint.style(false),
            before,
        ),
        Drag::Box {
            start,
            end,
            tool,
            before,
        } => (
            *tool,
            ((*end - *start).abs().min_element() > 0.0)
                .then(|| box_anchors(*tool, *start, *end))
                .into_iter()
                .collect::<Vec<_>>(),
            paint.style(true),
            before,
        ),
        _ => return,
    };
    let shapes = match anchors
        .into_iter()
        .map(|a| Shape::new(a, style, paint.color))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(shapes) => shapes,
        Err(e) => {
            *message = Some(e.to_string());
            return;
        }
    };
    if before.is_none() && shapes.is_empty() {
        return;
    }
    let drawn = edit_picked_layer(editor, selection, r, tool, Some(CANVAS_DRAG), |l| {
        match before {
            Some(b) => l.clone_from(b),
            None => *before = Some(l.clone()),
        }
        for shape in shapes {
            if l.edit_raster(|raster| raster.paint_shape(&shape)).is_none() {
                l.insert_shape(l.shapes().len(), shape);
            }
        }
    });
    if let Err(why) = drawn {
        *message = Some(why);
    }
}

/// Ends a drag the primary button let go of: a brush stroke or box picks
/// the shapes it drew, as [`add_shape`] does, and a box with no area is
/// dropped.
fn finish_drag(editor: &mut Editor, drag: Drag, selection: &mut Selection, r: VariantRef) {
    match drag {
        Drag::Box { start, end, .. } if (end - start).abs().min_element() <= 0.0 => {
            editor.revert_gesture(CANVAS_DRAG);
        }
        Drag::Brush {
            before: Some(b), ..
        }
        | Drag::Box {
            before: Some(b), ..
        } => {
            let layers = editor.project().variant(r).layers();
            let drawn = match layers.get(selection.layer) {
                Some(l) if l.kind() == LayerKind::Vector => b.shapes().len()..l.shapes().len(),
                _ => 0..0,
            };
            selection.shapes = drawn.collect();
            selection.anchor = None;
        }
        _ => {}
    }
}

/// Whether new shapes can go on the selected layer, or why not.
pub fn drawable(project: &Project, r: VariantRef, selection: &Selection) -> Result<(), String> {
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
    let prepare = |_: &mut Project| Ok(());
    edit_prepared_layer(editor, selection, r, tool, continuous, prepare, |l, ()| {
        f(l)
    })
}

/// [`edit_picked_layer`], after the project edit `prepare`, such as adding
/// palette entries, in the same step; `f` takes what `prepare` returns.
fn edit_prepared_layer<P, T>(
    editor: &mut Editor,
    selection: &mut Selection,
    r: VariantRef,
    tool: Tool,
    continuous: Option<&str>,
    prepare: impl FnOnce(&mut Project) -> Result<P, String>,
    f: impl FnOnce(&mut Layer, P) -> T,
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
    let edit = |project: &mut Project| {
        let prepared = prepare(project)?;
        let edit = |v: &mut mint_core::Variant| {
            if v.layers().is_empty() {
                let first = if tool.works_on(LayerKind::Vector) {
                    Layer::new("Layer 1")
                } else {
                    Layer::raster("Layer 1", Raster::clear(v.size()))
                };
                v.insert_layer(0, first);
            }
            f(&mut v.layers_mut()[layer], prepared)
        };
        project.edit_variant(r, edit).map_err(|e| e.to_string())
    };
    let out = match continuous {
        Some(key) => editor.edit_continuous(key, edit),
        None => editor.edit(edit),
    }?;
    selection.layer = layer;
    Ok(out)
}

/// A pencil or eraser dot, or a fill, at `p` on the picked raster layer, as
/// [`edit_picked_layer`] makes it. Returns the layer painted, or reports why
/// it couldn't be. A click (with no `continuous` edit) that touches no pixel
/// of the texture does nothing.
fn press_pixels(
    canvas: Canvas<'_>,
    r: VariantRef,
    tool: Tool,
    p: Vec2,
    continuous: Option<&str>,
) -> Option<usize> {
    let Canvas {
        editor,
        selection,
        paint,
        message,
        ..
    } = canvas;
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
        l.edit_raster(|raster| {
            if tool == Tool::Fill {
                raster.fill(p, value);
            } else {
                raster.draw_line(p, p, paint.width, value);
            }
        });
    });
    match painted {
        Ok(()) => Some(selection.layer),
        Err(why) => {
            *message = Some(why);
            None
        }
    }
}

/// Adds a shape drawn with `tool` through `anchors`, `closed` or open, on
/// top of the picked layer, creating a first layer if the variant has none,
/// and picks it. On a raster layer, the shape is painted into its pixels
/// instead.
fn add_shape(canvas: Canvas<'_>, r: VariantRef, tool: Tool, anchors: Vec<Anchor>, closed: bool) {
    let Canvas {
        editor,
        selection,
        paint,
        message,
        ..
    } = canvas;
    let shape = match Shape::new(anchors, paint.style(closed), paint.color) {
        Ok(s) => s,
        Err(e) => {
            *message = Some(e.to_string());
            return;
        }
    };
    let added = edit_picked_layer(editor, selection, r, tool, None, |l| {
        if l.edit_raster(|raster| raster.paint_shape(&shape)).is_some() {
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

/// Places the Stamp tool's stamp at `p` on top of the picked layer, or into
/// its pixels, as [`add_shape`] adds a shape, and picks what it placed. The
/// palette entries its colors were chosen to become are added in the same
/// step. A click off the texture places nothing. Choices whose entries have
/// changed are dropped, for the app to ask for again.
fn place_stamp(canvas: Canvas<'_>, r: VariantRef, p: Vec2) {
    let Canvas {
        editor,
        selection,
        message,
        stamp,
        ..
    } = canvas;
    let Some(placing) = stamp else { return };
    let size = editor.project().variant(r).size().as_vec2();
    if p.cmplt(Vec2::ZERO).any() || p.cmpge(size).any() {
        return;
    }
    if let Err(why) = drawable(editor.project(), r, selection) {
        *message = Some(why);
        return;
    }
    let Some(choices) = placing.current(editor.project().palette()) else {
        return;
    };
    let resolve = |project: &mut Project| {
        placing
            .resolve(project, &choices)
            .map_err(|e| e.to_string())
    };
    let place = |l: &mut Layer, entries: Vec<PaletteIndex>| {
        let shapes = placing.stamp.placed(p, &entries);
        let painted = l.edit_raster(|raster| {
            for s in &shapes {
                raster.paint_shape(s);
            }
        });
        if painted.is_some() {
            return (entries, None);
        }
        let start = l.shapes().len();
        for s in shapes {
            l.insert_shape(l.shapes().len(), s);
        }
        (entries, Some(start..l.shapes().len()))
    };
    match edit_prepared_layer(editor, selection, r, Tool::Stamp, None, resolve, place) {
        Ok((entries, shapes)) => {
            placing.choose(editor.project().palette(), &entries);
            selection.shapes = shapes.into_iter().flatten().collect();
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

    const PAINT: Paint = Paint {
        color: PaletteIndex(0),
        fill: true,
        width: 1.0,
    };

    /// A pointer path of one step to `p`.
    fn to(p: Vec2) -> [Sample; 1] {
        [Sample {
            point: p,
            jump: false,
        }]
    }

    /// A canvas on `editor` whose pick and refusals no one looks at.
    fn canvas(editor: &mut Editor) -> Canvas<'_> {
        Canvas {
            editor,
            selection: Box::leak(Box::default()),
            paint: PAINT,
            message: Box::leak(Box::default()),
            stamp: None,
        }
    }

    #[test]
    fn drawings_show_in_the_layer_as_they_are_drawn() {
        let (mut editor, r) = layerless();
        let mut selection = Selection::default();
        let step = |editor: &mut Editor, drag: &mut Drag, p: Vec2| {
            continue_drag(
                canvas(editor),
                drag,
                &(0, Vec::new()),
                r,
                &to(p),
                &mut [],
                false,
            );
        };
        let shapes = |e: &Editor| -> Vec<Shape> {
            let layers = e.project().variant(r).layers();
            layers.first().map_or(Vec::new(), |l| l.shapes().to_vec())
        };
        let mut drag = Drag::Box {
            start: Vec2::ONE,
            end: Vec2::ONE,
            tool: Tool::Rectangle,
            before: None,
        };
        // A box with no area draws nothing, not even a first layer.
        step(&mut editor, &mut drag, Vec2::new(1.0, 3.0));
        assert!(editor.project().variant(r).layers().is_empty());
        step(&mut editor, &mut drag, Vec2::splat(3.0));
        assert_eq!(shapes(&editor).len(), 1);
        // The box follows the pointer; it is one shape, not one per move.
        step(&mut editor, &mut drag, Vec2::splat(2.0));
        let drawn = shapes(&editor);
        assert_eq!(drawn.len(), 1);
        let far = drawn[0].anchors().iter().map(|a| a.point).reduce(Vec2::max);
        assert_eq!(far, Some(Vec2::splat(2.0)));
        finish_drag(&mut editor, drag, &mut selection, r);
        editor.end_gesture();
        assert_eq!(selection.shapes, vec![0]);
        // The whole drag undoes as one step.
        editor.undo();
        assert!(editor.project().variant(r).layers().is_empty());
        assert!(!editor.can_undo());

        // A cancelled stroke leaves nothing.
        let mut drag = Drag::Brush {
            runs: vec![vec![Vec2::ONE]],
            before: None,
        };
        step(&mut editor, &mut drag, Vec2::splat(2.0));
        assert_eq!(shapes(&editor).len(), 1);
        editor.revert_gesture(CANVAS_DRAG);
        assert!(editor.project().variant(r).layers().is_empty());

        // A box dragged back to no area is dropped.
        let mut drag = Drag::Box {
            start: Vec2::ONE,
            end: Vec2::ONE,
            tool: Tool::Ellipse,
            before: None,
        };
        step(&mut editor, &mut drag, Vec2::splat(3.0));
        step(&mut editor, &mut drag, Vec2::ONE);
        finish_drag(&mut editor, drag, &mut selection, r);
        editor.end_gesture();
        assert!(editor.project().variant(r).layers().is_empty());
        assert!(!editor.can_undo());
    }

    #[test]
    fn strokes_break_where_the_path_jumps() {
        let (mut editor, r) = layerless();
        let mut selection = Selection::default();
        let path = [(1.5, false), (3.5, true)].map(|(x, jump)| Sample {
            point: Vec2::new(x, 1.5),
            jump,
        });
        // A brush stroke is a shape per run, all picked when it ends.
        let mut drag = Drag::Brush {
            runs: vec![vec![Vec2::new(0.5, 1.5)]],
            before: None,
        };
        continue_drag(
            canvas(&mut editor),
            &mut drag,
            &(0, Vec::new()),
            r,
            &path,
            &mut [],
            false,
        );
        finish_drag(&mut editor, drag, &mut selection, r);
        editor.end_gesture();
        assert_eq!(editor.project().variant(r).layers()[0].shapes().len(), 2);
        assert_eq!(selection.shapes, vec![0, 1]);
        editor.undo();
        assert!(editor.project().variant(r).layers().is_empty());

        // A pencil line skips the jump.
        editor
            .edit_variant(r, |v| {
                v.insert_layer(0, Layer::raster("l", Raster::clear(v.size())));
            })
            .unwrap();
        let mut drag = Drag::Pixels {
            layer: 0,
            last: Vec2::new(0.5, 1.5),
            value: Some(PaletteIndex(0)),
            width: 1.0,
        };
        continue_drag(
            canvas(&mut editor),
            &mut drag,
            &(0, Vec::new()),
            r,
            &path,
            &mut [],
            false,
        );
        let row: Vec<bool> = editor.project().variant(r).layers()[0].coverage(UVec2::new(4, 4))
            [4..8]
            .iter()
            .map(Option::is_some)
            .collect();
        assert_eq!(row, vec![true, true, false, true]);
    }

    #[test]
    fn guides_move_on_the_half_texel_grid_within_the_texture() {
        use mint_core::{Axes, HalfPoint, Mirror, Modifier, Radial, Turns};
        let (mut editor, r) = layerless();
        let mirror = Mirror {
            axes: Axes::Both,
            at: HalfPoint { x: 4, y: 4 },
        };
        let radial = Radial::nearest(Turns::Quarter, Vec2::new(1.0, 1.0), UVec2::new(4, 4));
        editor
            .edit_variant(r, |v| {
                let mut l = Layer::new("l");
                l.modifiers
                    .push(Modifier::new(ModifierKind::Mirror(mirror)));
                l.modifiers
                    .push(Modifier::new(ModifierKind::Radial(radial)));
                v.insert_layer(0, l);
            })
            .unwrap();
        let before = editor.project().clone();
        // The 4x4 texture at 40 points a texel, inside a margin.
        let at = Placement {
            view: Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 200.0)),
            image: Rect::from_min_size(pos2(20.0, 20.0), vec2(160.0, 160.0)),
            scale: 40.0,
        };
        let above = pos2(100.0, 20.0 - GUIDE_REACH);
        let h = guide_hit(editor.project(), r, 0, at, above).unwrap();
        assert_eq!((h.modifier, h.part), (0, GuidePart::X));
        // The radial center's lines have handles too; the image has none.
        let left = pos2(20.0 - GUIDE_REACH, 60.0);
        let h = guide_hit(editor.project(), r, 0, at, left).unwrap();
        assert_eq!((h.modifier, h.part), (1, GuidePart::Y));
        assert!(guide_hit(editor.project(), r, 0, at, pos2(60.0, 60.0)).is_none());

        let kind = |e: &Editor, k: usize| e.project().variant(r).layers()[0].modifiers[k].kind;
        // A drag keeps the offset it grabbed the handle at: grabbed 0.15
        // texels right of the line and moved 0.2 right, the line stays put.
        let grab = at.to_texture(pos2(106.0, 20.0 - GUIDE_REACH));
        let mut d = guide_drag(editor.project(), r, 0, grab, at).unwrap();
        let moved = to(grab + Vec2::new(0.2, 3.0));
        continue_drag(
            canvas(&mut editor),
            &mut d,
            &(0, Vec::new()),
            r,
            &moved,
            &mut [],
            false,
        );
        assert_eq!(kind(&editor, 0), ModifierKind::Mirror(mirror));
        editor.end_gesture();
        let drag = |e: &mut Editor, modifier, part, p| {
            let mut d = Drag::Guide {
                layer: 0,
                modifier,
                part,
                offset: 0.0,
            };
            continue_drag(
                canvas(e),
                &mut d,
                &(0, Vec::new()),
                r,
                &to(p),
                &mut [],
                false,
            );
        };
        drag(&mut editor, 0, GuidePart::X, Vec2::new(1.3, 9.0));
        let moved = Mirror {
            at: HalfPoint { x: 3, y: 4 },
            ..mirror
        };
        assert_eq!(kind(&editor, 0), ModifierKind::Mirror(moved));
        // Guides stay on the image.
        drag(&mut editor, 0, GuidePart::Y, Vec2::new(0.0, -3.0));
        let moved = Mirror {
            at: HalfPoint { x: 3, y: 0 },
            ..mirror
        };
        assert_eq!(kind(&editor, 0), ModifierKind::Mirror(moved));
        // Quarter turns keep the center on a pixel corner or center.
        drag(&mut editor, 1, GuidePart::X, Vec2::new(1.8, 0.0));
        let ModifierKind::Radial(moved) = kind(&editor, 1) else {
            panic!("not radial");
        };
        assert_eq!(moved.center(), HalfPoint { x: 4, y: 2 });

        editor.end_gesture();
        editor.undo();
        assert_eq!(editor.project(), &before);

        // A locked layer's guides show but don't move.
        editor
            .edit_variant(r, |v| v.layers_mut()[0].locked = true)
            .unwrap();
        assert!(guide_hit(editor.project(), r, 0, at, above).is_none());
    }

    #[test]
    fn what_a_layer_shows_hides_the_shapes_beneath() {
        use mint_core::{Axes, Mirror, Modifier};
        let (mut editor, r) = layerless();
        let square = |min: Vec2, max: Vec2| {
            Shape::new(shape::rectangle(min, max), Style::Fill, PaletteIndex(0)).unwrap()
        };
        // Bottom: a shape over the whole texture. Top: a shape covering pixel
        // (3, 0)'s center but not its left part, mirrored onto (0, 0).
        editor
            .edit_variant(r, |v| {
                let mut bottom = Layer::new("bottom");
                bottom.insert_shape(0, square(Vec2::ZERO, Vec2::splat(4.0)));
                let mut top = Layer::new("top");
                top.insert_shape(0, square(Vec2::new(3.4, 0.0), Vec2::new(4.0, 1.0)));
                top.modifiers
                    .push(Modifier::new(ModifierKind::Mirror(Mirror {
                        axes: Axes::X,
                        at: mint_core::HalfPoint { x: 4, y: 0 },
                    })));
                v.insert_layer(0, bottom);
                v.insert_layer(1, top);
            })
            .unwrap();
        let at = Placement {
            view: Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 200.0)),
            image: Rect::from_min_size(pos2(20.0, 20.0), vec2(160.0, 160.0)),
            scale: 40.0,
        };
        let picked = |e: &Editor, x: f32, y: f32| {
            let mut s = Selection::default();
            pick(e.project(), r, &mut s, Vec2::new(x, y), at.hit(), false);
            s.single().map(|shape| (s.layer, shape))
        };
        let set = |e: &mut Editor, f: fn(&mut Layer)| {
            e.edit_variant(r, |v| f(&mut v.layers_mut()[1])).unwrap();
        };
        // The top shape picks, and its copy hides the bottom shape.
        assert_eq!(picked(&editor, 3.5, 0.5), Some((1, 0)));
        assert_eq!(picked(&editor, 0.5, 0.5), None);
        assert_eq!(picked(&editor, 2.5, 2.5), Some((0, 0)));
        // A click off a shape's outline, on a pixel it covers, picks it.
        assert_eq!(picked(&editor, 3.05, 0.5), Some((1, 0)));
        // A press drags what a click there picks: not the picked bottom
        // shape where the top one, or its copy, shows over it.
        let mut picked_bottom = Selection {
            layer: 0,
            shapes: vec![0],
            anchor: None,
        };
        let grab = |s: &mut Selection, x: f32| {
            start_select(editor.project(), r, s, Vec2::new(x, 0.5), at).map(|_| s.layer)
        };
        assert_eq!(grab(&mut picked_bottom.clone(), 3.5), Some(1));
        assert_eq!(grab(&mut picked_bottom, 0.5), None);
        // A locked layer still hides what is under it.
        set(&mut editor, |l| l.locked = true);
        assert_eq!(picked(&editor, 3.5, 0.5), None);
        assert_eq!(picked(&editor, 0.5, 0.5), None);
        // One at no opacity, or hidden, doesn't.
        set(&mut editor, |l| l.opacity = 0.0);
        assert_eq!(picked(&editor, 0.5, 0.5), Some((0, 0)));
        set(&mut editor, |l| {
            l.opacity = 1.0;
            l.visible = false;
        });
        assert_eq!(picked(&editor, 0.5, 0.5), Some((0, 0)));
    }

    #[test]
    fn a_copy_beside_its_shape_is_no_handle_for_it() {
        use mint_core::{Axes, Mirror, Modifier};
        let (mut editor, r) = layerless();
        // A shape on pixel (1, 0), mirrored onto (2, 0) right beside it.
        editor
            .edit_variant(r, |v| {
                let mut l = Layer::new("l");
                let s = shape::rectangle(Vec2::new(1.0, 0.0), Vec2::new(2.0, 1.0));
                l.insert_shape(0, Shape::new(s, Style::Fill, PaletteIndex(0)).unwrap());
                l.modifiers.push(Modifier::new(ModifierKind::Mirror(Mirror {
                    axes: Axes::X,
                    at: mint_core::HalfPoint { x: 4, y: 0 },
                })));
                v.insert_layer(0, l);
            })
            .unwrap();
        let at = Placement {
            view: Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 200.0)),
            image: Rect::from_min_size(pos2(20.0, 20.0), vec2(160.0, 160.0)),
            scale: 40.0,
        };
        let under = |x: f32, y: f32| shape_under(editor.project(), r, Vec2::new(x, y), at.hit());
        // On the copy, within reach of the shape's outline: nothing.
        assert_eq!(under(2.1, 0.5), None);
        // Within reach of the outline where nothing shows: the shape.
        assert_eq!(under(1.5, 1.1), Some((0, 0)));
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
            let canvas = Canvas {
                editor: &mut editor,
                selection: &mut selection,
                paint,
                message: &mut message,
                stamp: None,
            };
            let pressed = press_pixels(canvas, r, tool, margin, None);
            assert_eq!(pressed, None, "{tool:?}");
        }
        assert!(editor.project().variant(r).layers().is_empty());
        assert!(!editor.can_undo());

        // A wide tip hanging over the edge reaches the texture.
        let wide = Paint {
            width: 3.0,
            ..paint
        };
        let canvas = Canvas {
            editor: &mut editor,
            selection: &mut selection,
            paint: wide,
            message: &mut message,
            stamp: None,
        };
        let pressed = press_pixels(canvas, r, Tool::Pencil, margin, None);
        assert_eq!(pressed, Some(0));
        let layers = editor.project().variant(r).layers();
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].kind(), LayerKind::Raster);
    }

    #[test]
    fn a_stamp_from_another_palette_places_as_one_step() {
        use crate::stamps::Choice;
        const RED: Rgba8 = Rgba8::new(255, 0, 0, 255);
        let black = Rgba8::new(0, 0, 0, 255);
        // A 2x2 red square saved from a project whose palette is black, red.
        let anchors = [(0.0, 0.0), (2.0, 0.0), (2.0, 2.0), (0.0, 2.0)]
            .map(|(x, y)| Anchor::corner(Vec2::new(x, y)))
            .to_vec();
        let square = Shape::new(anchors, Style::Fill, PaletteIndex(1)).unwrap();
        let source = Palette::from_colors([black, RED]).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("square.ron");
        let saved =
            mint_core::Stamp::from_shapes(mint_core::NonEmpty::one(square), &source).unwrap();
        mint_core::file::save_stamp(&saved, &path).unwrap();
        let stamp = std::sync::Arc::new(mint_core::file::load_stamp(&path).unwrap());

        // Placed into a project whose palette is only black, red becoming a
        // new entry.
        let (mut editor, r) = layerless();
        let mut placing = Placing::new("square".into(), stamp, editor.project().palette());
        assert_eq!(placing.choices, None);
        placing.choices = Some(vec![Choice::New]);
        let (mut selection, mut message) = (Selection::default(), None);
        let mut place = |editor: &mut Editor, placing: &mut Placing, at: Vec2| {
            let canvas = Canvas {
                editor,
                selection: &mut selection,
                paint: PAINT,
                message: &mut message,
                stamp: Some(placing),
            };
            place_stamp(canvas, r, at);
        };
        // Centered on the corner (2, 2) nearest the click.
        place(&mut editor, &mut placing, Vec2::new(2.3, 1.6));
        assert_eq!(editor.project().palette().colors(), [black, RED]);
        let pixels: Vec<u8> = editor
            .project()
            .variant(r)
            .rasterize()
            .pixels()
            .iter()
            .map(|p| p.0)
            .collect();
        #[rustfmt::skip]
        assert_eq!(pixels, [
            0, 0, 0, 0,
            0, 1, 1, 0,
            0, 1, 1, 0,
            0, 0, 0, 0,
        ]);
        // Placing again reuses the entry it added.
        assert_eq!(
            placing.choices,
            Some(vec![Choice::Entry(PaletteIndex(1), RED)])
        );
        place(&mut editor, &mut placing, Vec2::new(0.9, 1.0));
        assert_eq!(editor.project().palette().len(), 2);
        assert_eq!(editor.project().variant(r).layers()[0].shapes().len(), 2);

        // One undo step each, the first taking its palette entry with it.
        editor.undo();
        editor.undo();
        assert_eq!(editor.project().palette().colors(), [black]);
        assert!(editor.project().variant(r).layers().is_empty());
        // Its choice then names an entry that's gone, so placing waits for
        // the colors to be chosen again.
        place(&mut editor, &mut placing, Vec2::new(2.0, 2.0));
        assert_eq!(placing.choices, None);
        assert!(editor.project().variant(r).layers().is_empty());
    }

    #[test]
    fn a_stamp_paints_into_a_raster_layer() {
        let (mut editor, r) = layerless();
        editor
            .edit_variant(r, |v| {
                v.insert_layer(0, Layer::raster("px", Raster::clear(v.size())));
            })
            .unwrap();
        let dot = Shape::new(
            vec![Anchor::corner(Vec2::new(0.5, 0.5))],
            Style::Stroke {
                width: 1.0,
                closed: false,
            },
            PaletteIndex(0),
        )
        .unwrap();
        let stamp = mint_core::Stamp::from_shapes(
            mint_core::NonEmpty::one(dot),
            editor.project().palette(),
        )
        .unwrap();
        let mut placing = Placing::new(
            "dot".into(),
            std::sync::Arc::new(stamp),
            editor.project().palette(),
        );
        let (mut selection, mut message) = (Selection::default(), None);
        let canvas = Canvas {
            editor: &mut editor,
            selection: &mut selection,
            paint: PAINT,
            message: &mut message,
            stamp: Some(&mut placing),
        };
        // On the pixel under the pointer.
        place_stamp(canvas, r, Vec2::new(3.2, 1.1));
        let layer = &editor.project().variant(r).layers()[0];
        let set: Vec<usize> = layer
            .raster_pixels()
            .unwrap()
            .pixels()
            .iter()
            .enumerate()
            .filter_map(|(i, p)| p.map(|_| i))
            .collect();
        assert_eq!(set, [3 + 4]);
        assert!(selection.shapes.is_empty());
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
