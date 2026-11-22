# mint-rabbit

A standalone tool for building textured 3D characters: start from a base model,
paint on it with editable vector shapes, add geometry, and export to a target
format such as an Animal Crossing (GameCube) villager.

## Language

### Document

**Project**:
The editable document for one character: its model, palette, textures, and
every layer and shape on them.
_Avoid_: File, scene, workspace

**Character**:
The one model plus its set of textures that a project produces.
_Avoid_: Villager, species, NPC (these are Animal Crossing export terms)

**Base Model**:
The node tree and textures a project was imported from.
_Avoid_: Template, base mesh

**Node**:
An element of the model tree with a transform, an optional mesh, and child
nodes; a skeleton's joints are nodes too.
_Avoid_: Joint, bone, part, object

**Mesh**:
Geometry owned by a node: vertices with UVs, and triangles that each draw with
one material.
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
coordinates and assigned a slot; it holds one or more variants.
_Avoid_: Skin, image, map

**Slot**:
The place in an export target's format that a texture fills, with that
target's rules for its size and variants, such as Animal Crossing's Body, Eyes, Mouth, and Clothing.
_Avoid_: Segment, channel, role

**External Texture**:
A texture whose slot the export target fills at runtime, such as a villager's
clothing; it holds only a preview underlay with its own colors and is never
exported.
_Avoid_: Placeholder, runtime texture

**Material**:
How triangles draw with a texture: the texture, a wrap mode per axis (clamp,
repeat, or mirror), and an alpha mode (opaque, cutout, or translucent).
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
An ordered list of shapes in one variant, with its own visibility, lock, and
display-only opacity that never affects export.
_Avoid_: Group, folder

**Shape**:
A single vector element in texture space: a path that is either filled
(closed) or stroked (open), in one palette color.
_Avoid_: Stroke, vector, brush mark

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
