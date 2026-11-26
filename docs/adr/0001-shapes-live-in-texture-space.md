# Shapes live in texture space

Shapes are stored in the texture space of the texture they belong to (its UV
coordinates scaled by its pixel size, so one unit is one texel), not anchored
to the mesh surface. Painting in the Model View raycasts to a UV coordinate,
so both views edit the same data and export is plain rasterization.

## Consequences

- On the model, a shape doesn't continue past its UV island's edge. A
  freehand stroke painted across UV islands in the Model View becomes a
  separate shape on each UV island of its texture that it touches; pen paths
  and boxes run straight in texture space between their points.
- Moving a mesh's UVs moves the painting that lands on it.
- Texel units keep circles round and stroke widths even on non-square
  textures, such as AC's 32x16 eyes, where normalized UV would stretch them.
- Shapes stay independent of output resolution: rasterizing at a multiple of
  the texture's size (for example for texture-pack mods) scales them by that
  factor.

## Status

Accepted for v1. Revisit if shapes stopping at UV island edges become a real
pain point; a surface-anchored representation projected into UV space is the
main alternative. The imported image was first an immutable underlay beneath
the shapes; ADR 0004 replaced it with raster layers.
