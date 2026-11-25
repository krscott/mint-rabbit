# mint-rabbit

A 3D model and vector texture painter. Its first export target is Animal
Crossing (GameCube) villagers in an [ac-decomp](https://github.com/ACreTeam/ac-decomp)
checkout. See [GLOSSARY.md](GLOSSARY.md) for the domain language and
[docs/adr](docs/adr) for design decisions.

## Usage

```sh
mint-rabbit [--decomp PATH/TO/ac-decomp] [--game-version GAFE01_00] [--project DIR]
```

The decomp's villager assets must first be extracted from your own disc image
in its `orig/` directory (`python configure.py && ninja pre-compile`, which
`just run --decomp PATH` does for you). `--decomp` defaults to the
`MINT_AC_DECOMP` environment variable.

File > Import villager… picks a villager by its thumbnail, in-game name,
species, or texture set, and shows it in the Model View (left) and Texture View
(right). Names are read from the disc image in the decomp's `orig/<version>/`
directory; without one, villagers show their texture set names. File > Open
decomp… opens another checkout.

The thumbnails above the Texture View choose the texture it shows and, for a
texture with several, the variant. Paint in it with the tools at its left:
Select (V), Pen (P), Brush (B), Ellipse (E), and Rectangle (R) for shapes, and
Pencil (N), Eraser (Shift+E), and Fill (G) for pixels. Painting goes on the
picked layer, which must be visible and unlocked, in the picked palette color
and the style set in the bar above the view. A vector layer holds shapes; a
raster layer holds pixels, and Pen, Ellipse, and Rectangle paint their finished
shape into it. Tools that don't fit the picked layer are disabled. With the
Select tool and shapes picked, that bar shows and changes their style instead,
and can recolor, duplicate, reorder, and move them to another layer.

The panel at the right holds the palette and the layers of the variant shown.
An imported image arrives as a locked raster layer named Base. Pixels no
visible layer covers show and export as the first palette entry. Double-click a
layer's name to rename it, or right-click it to rename, duplicate, copy to
other variants, rasterize, or delete it.

Under the layers, Modifiers lists the picked layer's modifiers, which change
what it shows and exports without changing what it holds. Mirror reflects the
layer across a vertical line (X), a horizontal line (Y), or both; Radial turns
copies of it about a point, one at 180° or three at 90°. Copies draw beneath the
layer's own shapes and pixels and are cut off at the texture's edges. Drag the
arrow handles of a guide's lines in the Texture View, or type their positions;
they snap to half texels, and a 90° center to a pixel corner or center. Pixel
tools on a raster layer edit what it shows, copies included: the layer keeps
only what was drawn, recolors its own pixels at matching places, and erasing a
copy erases what it copies. Apply writes the first modifier's copies into the
layer and removes it; a locked layer's modifiers can only be turned on and off.

- Select: drag a shape to move it, the square handles outside its corners to
  scale (Shift keeps proportions), and the round handle to rotate (Shift snaps
  to 15°). With one shape picked, drag its anchors and handles to reshape it
  (Shift breaks a smooth anchor's handles apart), double-click its outline to
  add an anchor, and press Delete to remove the picked anchor or shapes.
- Pen: each click adds an anchor (drag to pull out handles), clicking the first
  anchor closes the path, Enter finishes it open, and Delete removes the last
  anchor.
- Pencil and Eraser: click or drag to set or clear pixels with a round tip as
  wide as the bar's width, rounded to whole pixels. Fill: click to fill the
  pixels joined to the one clicked that match it (a color, or empty), on the
  picked layer only.
- Escape drops a drawing in progress, puts shapes back during a transform, and
  otherwise clears the picks. Backspace works like Delete. Scroll to zoom,
  middle-drag to pan, and middle double-click to fit.
- Ctrl+Z undoes, Ctrl+Shift+Z or Ctrl+Y redoes, Ctrl+S saves, and Ctrl+D
  duplicates the picked shapes.

A project saves as a folder (File > Save, Save as). The File menu's exports
either replace the imported villager's texture set in the decomp, after which
the decomp must be built with `python configure.py --non-matching && ninja`, or
write a new texture set to a folder with the steps for adding it. The status
bar says whether the project can export, and if not, why.

To try the app without a disc, `just demo` writes a generated villager to a
decomp-shaped directory and opens it.

## Development

Enter the development shell with `nix develop`, then run:

```sh
just build
just run --decomp PATH
just test
just format
just check
```

The decomp recipes take an ac-decomp checkout as PATH, which defaults to
`MINT_AC_DECOMP`.

- `just test-decomp-sources [PATH]` parses every model source and the
  villager table of a decomp checkout, built or not.
- `just test-decomp [PATH]` also round-trips every villager (import, export,
  import), saves and loads each as a project, checks that each texture
  export could replace its source in the decomp, and reads the villager names
  from the disc image when there is an uncompressed one; it needs a built
  checkout.
- `just snapshot [PATH]` renders the first villager of a decomp (the demo
  when neither is given) offscreen to `target/model-view.png`; it needs a
  GPU.

`just check` checks formatting, runs ShellCheck and Clippy, and validates the
flake without rewriting source files. Formatting and shell checks cover tracked
files; stage new files with `git add` to include them.

Install the formatting hook with `just install-hooks`. It formats fully staged
files and leaves partially staged files alone.

Update dependencies with `nix flake update`.

## License

mint-rabbit is licensed under the GNU General Public License, version 3 only
(`GPL-3.0-only`); see [LICENSE](LICENSE).
