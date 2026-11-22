# Palette-indexed color in the core

Shapes and underlay pixels refer to entries in the project's palette instead of
holding RGB values, even though the core is target-agnostic. The first target
(Animal Crossing villagers) gives each character one 16-color palette shared by
every body, eye, and mouth texture, so colors must be chosen deliberately;
quantizing free RGB at export would reorder and merge colors unpredictably.
Indexed color also makes recolor variants a palette edit.

## Consequences

- Removing a palette entry that is in use is only possible by merging it into
  another entry, so no shape can refer to a missing color.
- Placing a stamp asks the user to map each stamp color to an entry (defaulting
  to the nearest) or add it as a new one.
- Palette entries are stored as RGBA8; views show them quantized to the
  target's precision. A future high-resolution RGBA target resolves entries to
  their colors at export.
