# Generic node tree; target structure lives in adapters

The core model is a generic tree of nodes, each with an optional mesh, where a
vertex follows its mesh's node or one other node (a seam vertex). Nothing in
the core knows about Animal Crossing. The AC import/export adapter owns all of
its structure: the shared 26-joint skeleton, the rule that a joint's mesh
follows at most two nodes, texture slots, packing body textures into one
array addressed by offset, and merging added nodes into their nearest joint ancestor's
mesh on export. Each project records its export target, and the target's
constraints are checked while editing rather than only at export.

## Considered Options

- **AC-specific core**: villager, species, and joint types built into the
  model. Simpler for the first target, but non-character models and other
  targets would need a rewrite.
- **Base model plus separate "parts"**: two kinds of geometry with different
  rules. Dropped because parenting a node under a joint already expresses a
  rigid attachment, so the second kind adds nothing.

## Consequences

- Seam vertices exist in the core only because AC (like other N64-era games)
  stitches joints that way; a fresh reader will find them unusual.
- Import and export adapters carry more logic, and a round-trip test (import an
  AC model, export it unchanged, compare data) guards them.
