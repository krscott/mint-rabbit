# mint-rabbit

A standalone tool for building textured 3D models: start from a base model,
paint on it with editable vector shapes, add geometry, and export to a target
format, the first being Animal Crossing (GameCube) villagers.

## Language

### Document

**Project**:
The editable document for one model: its node tree, palette, textures, and
every layer and shape on them.
_Avoid_: File, scene, workspace

**Model**:
The node tree a project edits, with the meshes its nodes own.
_Avoid_: Character, scene; villager and species are Animal Crossing terms

**Base Model**:
The model and textures a project was imported from.
_Avoid_: Template, base mesh

**Node**:
An element of the model tree with a transform, an optional mesh, and child
nodes; a skeleton's joints are nodes too.
_Avoid_: Joint, bone, part, object

**Mesh**:
Geometry owned by a node: vertices, and triangles that each draw with one
material and give each corner a UV coordinate.
_Avoid_: Display list, model, geometry

**Pose**:
Display-only rotations applied to nodes so a model looks as it does in use,
such as the first frame of a game's idle animation; it is never exported.
_Avoid_: Rest pose, animation, frame

**Seam Vertex**:
A mesh vertex that follows a node other than its mesh's node, so that meshes on
neighboring nodes stay joined when posed.
_Avoid_: Stitch vertex, shared vertex

**Model File**:
A copy of a project's model kept outside it for editing in another program;
the project re-reads it whenever it changes.
_Avoid_: glTF, Blender file, exchange file

### Painting

**Palette**:
The project's ordered list of colors; every shape and every raster layer pixel
refers to a palette entry rather than holding its own color. A pixel that no
visible layer covers takes the first entry.
_Avoid_: Color table, TLUT, swatches

**Texture**:
A named image of fixed pixel size, mapped onto the model through UV
coordinates and assigned a slot; it holds one or more variants, or for an
external texture only a preview.
_Avoid_: Skin, image, map

**Slot**:
The place in an export target's format that a texture fills, with that
target's rules for its size and variants, such as Animal Crossing's Body, Eyes, Mouth, and Clothing.
_Avoid_: Segment, channel, role

**External Texture**:
A texture whose slot the export target fills at runtime, such as a villager's
clothing; it holds only a preview image with its own colors and is never
exported.
_Avoid_: Placeholder, runtime texture

**UV Island**:
A connected part of a mesh drawn with one texture whose UVs run on without
a cut or fold; a stroke can't continue straight from one UV island to
another.
_Avoid_: Island, UV shell, chart, seam (a seam vertex is something else)

**Material**:
How triangles draw with a texture: the texture, a wrap mode per axis (clamp,
repeat, or mirror), an alpha mode (opaque or cutout), whether
back faces show, and a tint.
_Avoid_: Shader, render mode

**Variant**:
One alternative image of a texture, such as one eye expression, with its own
layer stack; the Model View shows one variant per texture at a time.
_Avoid_: Expression, frame, version

**Layer**:
One level of a variant's stack, either a vector layer or a raster layer, with
its own visibility (hidden layers neither show nor export), lock,
display-only opacity that never affects export, and modifiers.
_Avoid_: Group, folder

**Modifier**:
One step of a layer's ordered stack that changes what the layer shows and
exports without changing the shapes or pixels it holds; each can be turned off,
which hides its result from both, or applied, which writes its result into the
layer and removes it.
_Avoid_: Effect, filter

**Mirror**:
A modifier that adds copies of its layer reflected across a vertical line, a
horizontal line, or both, each at a position the user sets.
_Avoid_: Flip, horizontal or vertical symmetry

**Radial**:
A modifier that adds copies of its layer turned about a point the user sets:
one copy at a half turn, or three at quarter turns.
_Avoid_: Rotate, rotational symmetry

**Vector Layer**:
A layer holding an ordered list of shapes.
_Avoid_: Shape layer

**Raster Layer**:
A layer holding one palette entry or nothing for each pixel of its texture,
such as the image a variant was imported with.
_Avoid_: Underlay, base texture, bitmap, pixel layer

**Rasterize**:
To turn shapes into the pixels they cover: a vector layer into a raster layer,
or a variant's visible layers into the image it exports.
_Avoid_: Bake

**Shape**:
A single vector element in texture space: a Bézier path drawn in one style and
one palette color.
_Avoid_: Stroke, vector, brush mark

**Style**:
How a shape covers pixels: filled, or stroked at a width along an open or
closed path.
_Avoid_: Fill mode, brush settings

**Pick**:
To choose what painting and edits apply to: a palette entry, a layer, and
shapes in that layer.
_Avoid_: Active layer, current color

**Stamp**:
A saved group of shapes in the user's Stamp Library; placing it copies its
shapes into a layer with no link back, mapping each of its colors to a palette
entry the user picks.
_Avoid_: Preset, brush, symbol

**Stamp Library**:
The user's own collection of stamps, shared by all their projects and kept
outside any of them.
_Avoid_: Asset library, stamp set

### Views

**Model View**:
The 3D pane, where painting lands on the texture under the cursor.
_Avoid_: 3D view, viewport

**Orbit Mode**:
The Model View mode in which dragging turns the camera around the model.
_Avoid_: Rotate mode, camera mode

**Paint Mode**:
The Model View mode in which the picked tool draws on the model, painting the
texture under the cursor.
_Avoid_: Edit mode, draw mode

**Ghost Cursor**:
The tool's footprint shown at the cursor's matching point in the other view,
and wherever else on the model that texture point appears.
_Avoid_: Brush preview, shadow cursor

**Texture View**:
The 2D pane that shows a texture in UV space.
_Avoid_: UV view, 2D view

### Output

**Export**:
The one-way conversion of a project into an export target's format, which
rasterizes shapes and applies the target's constraints.
_Avoid_: Save, bake

**Export Target**:
A destination format together with its constraints, such as an Animal Crossing
villager; each project records one, and its constraints are checked while
editing.
_Avoid_: Platform, backend

### Animal Crossing

The first export target's own language. The rest of the glossary stays
independent of it.

**Villager**:
An animal who can live in an Animal Crossing town or on its island, with an
in-game name, drawn as a texture set on a model of its species. Special
characters, such as Tom Nook, are not villagers.
_Avoid_: NPC, character

**Species**:
The kind of animal a villager is, such as Cat or Wolf; each villager model
belongs to one species.
_Avoid_: Animal type, race

**Texture Set**:
The textures a villager is drawn with (body, eyes, and mouth); each villager
has its own.
_Avoid_: Skin, villager textures

**Islander**:
A villager who lives on the island instead of moving into town.
_Avoid_: Island villager
