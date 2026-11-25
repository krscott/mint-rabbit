# Raster layers replace the underlay

A variant is only a stack of layers, each either a vector layer of shapes or a
raster layer of palette indices (or nothing) per pixel. Importing puts the
base image in a locked raster layer that is otherwise ordinary: it can be
unlocked, painted, hidden, reordered, or deleted. This lets users touch up
imported pixels directly, which an immutable underlay beneath the shapes could
not, at the cost of two kinds of layer, separate pixel tools, and pixels that
don't scale with output resolution as shapes do.

## Considered Options

- **Immutable underlay plus vector layers** (the original design): one
  editing model, but every pixel fix had to be drawn as shapes.
- **A special base layer** that can't be deleted or leave the bottom: it
  guarantees every pixel is covered, but adds a concept and its exceptions.

## Consequences

- Pixels that no visible layer covers show and export as palette entry 0,
  by position, so entry 0 can't be merged away while any exist.
- Rasterizing a vector layer is one-way; only undo brings its shapes back.
- Raster layers hold pixels at the texture's size. An export at a multiple
  of that size, which ADR 0001 keeps possible for shapes, would scale their
  pixels up rather than redraw them.
