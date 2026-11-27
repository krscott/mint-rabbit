# Geometry is edited in external programs

mint-rabbit has no geometry tools. A project sends its model to a model file
(glTF binary), the user edits it in a program such as Blender, and the project
re-reads the file whenever it changes, keeping its textures and painting.
Blender already moves vertices, adds meshes, and edits UVs far better than an
in-app tool set could, so mint-rabbit stays a painter and an export adapter.

The model file holds one mesh skinned to the project's nodes, each vertex
weighted fully to the node it follows; coincident vertices that follow the
same node and share a normal are one vertex in Blender, so seams don't open
while editing. The skeleton is posed in the project's display pose, which
becomes the rest pose in the other program, because AC's own rest pose is
not a usable shape. Reading it back undoes the pose and assigns each
triangle to the deepest node among its corners; corners that follow another
node become seam vertices. The node tree is fixed by the export target: a
vertex on a bone the project lacks is refused, and reading replaces only the
nodes' meshes.

## Considered Options

- **In-app geometry tools** (primitives, node and vertex transforms, a UV
  island tool): no round trip, but a second, weaker modeler to maintain, and
  painted shapes would have to follow islands the tool moves.
- **A Blender add-on that reads and writes AC models**: no exchange file, but
  the AC adapter would exist twice, in Rust and Python.
- **One model-file object per node**: keeps which node owns each triangle,
  but seam vertices become copies the user must keep in step by hand.

## Consequences

- A triangle whose corners all follow another node moves to that node's mesh,
  drawing the same at another point in drawing order; a few AC models (bird,
  wolf, and `xct_1` tails) have two such triangles.
- A seam whose two sides have different normals, a hard edge, stays two
  vertices in Blender, which can be moved apart.
- Blender writes each object's faces grouped by material, so a node's
  drawing order can change, which matters only for coincident faces.
- The file's nonzero normals are unit length, and the few zero ones in AC
  models stay zero; reading gives an edited normal the length the export
  target uses (AC: 120/127, about as long as AC's own).
- Positions are scaled by a factor the export target gives (AC: 0.001 m per
  unit), so a model is a usable size in meters.
- A vertex the file holds exactly as the project would write it keeps the
  project's exact position, and normal if that is as written too, so
  reading a file nobody edited changes nothing, however often it is sent
  and read, while any edit, however small, is read.
- Added geometry shares its node's textures, such as the body atlas; a
  separate texture per added mesh needs target support first.
- Blender's glTF importer must merge vertices, or it splits the welded mesh
  at UV seams, and must not pack images, or it won't see repainted ones.
