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

### Painting

**Palette**:
The project's ordered list of colors; every shape and every underlay pixel of
an exported texture refers to a palette entry rather than holding its own color.
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

**Material**:
How triangles draw with a texture: the texture, a wrap mode per axis (clamp,
repeat, or mirror), an alpha mode (opaque or cutout), whether
back faces show, and a tint.
_Avoid_: Shader, render mode

**Variant**:
One alternative image of a texture, such as one eye expression, with its own
underlay and layer stack; the Model View shows one variant per texture at a
time.
_Avoid_: Expression, frame, version

**Underlay**:
The imported raster image beneath a variant's layers; it is never edited, only
shown, hidden, or replaced.
_Avoid_: Base texture, background

**Layer**:
An ordered list of shapes in one variant, with its own visibility (hidden
layers neither show nor export), lock, and display-only opacity that never
affects export.
_Avoid_: Group, folder

**Shape**:
A single vector element in texture space: a Bézier path drawn in one style and
one palette color.
_Avoid_: Stroke, vector, brush mark

**Style**:
How a shape covers pixels: filled, or stroked at a width along an open or
closed path.
_Avoid_: Fill mode, brush settings

**Pick**:
To choose the layer, and shapes in it, that edits apply to; new shapes go on
the picked layer.
_Avoid_: Selection, active layer

**Stamp**:
A saved group of shapes in the user's stamp library; placing it copies its
shapes into a layer with no link back, mapping each of its colors to a palette
entry the user picks.
_Avoid_: Preset, brush, symbol

### Views

**Model View**:
The 3D pane, where painting lands on the texture under the cursor.
_Avoid_: 3D view, viewport

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
