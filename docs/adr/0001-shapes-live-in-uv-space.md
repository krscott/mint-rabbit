# Shapes live in UV space over an immutable underlay

Shapes are stored in normalized UV coordinates of the texture they belong to,
not anchored to the mesh surface. Painting in the Model View raycasts to a UV
coordinate, so both views edit the same data and export is plain rasterization.
The imported underlay is never edited; every change is a vector shape on top of
it, which keeps a single editing model and a single undo history.

## Consequences

- A shape that crosses a UV seam does not continue across it in the Model
  View; it is clipped at the island edge or must be drawn on each side.
- Moving a mesh's UVs moves the painting that lands on it.
- Normalized coordinates make shapes independent of texture resolution, so the
  same shapes can later rasterize at a higher resolution (for example for
  texture-pack mods).

## Status

Accepted for v1. Revisit if seam-crossing shapes become a real pain point; a
surface-anchored representation projected into UV space is the main
alternative.
