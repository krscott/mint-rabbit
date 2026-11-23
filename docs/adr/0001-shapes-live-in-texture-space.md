# Shapes live in texture space over an immutable underlay

Shapes are stored in the texture space of the texture they belong to (its UV
coordinates scaled by its pixel size, so one unit is one texel), not anchored
to the mesh surface. Painting in the Model View raycasts to a UV coordinate,
so both views edit the same data and export is plain rasterization.
The imported underlay is never edited; every change is a vector shape on top of
it, which keeps a single editing model and a single undo history.

## Consequences

- A shape that crosses a UV seam does not continue across it in the Model
  View; it is clipped at the island edge or must be drawn on each side.
- Moving a mesh's UVs moves the painting that lands on it.
- Texel units keep circles round and stroke widths even on non-square
  textures, such as AC's 32x16 eyes, where normalized UV would stretch them.
- Shapes stay independent of output resolution: rasterizing at a multiple of
  the texture's size (for example for texture-pack mods) scales them by that
  factor.

## Status

Accepted for v1. Revisit if seam-crossing shapes become a real pain point; a
surface-anchored representation projected into UV space is the main
alternative.
