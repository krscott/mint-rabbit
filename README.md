# mint-rabbit

A 3D model and vector texture painter. Its first export target is Animal
Crossing (GameCube) villagers in an [ac-decomp](https://github.com/ACreTeam/ac-decomp)
checkout. See [GLOSSARY.md](GLOSSARY.md) for the domain language and
[docs/adr](docs/adr) for design decisions.

## Usage

```sh
mint-rabbit [--decomp PATH/TO/ac-decomp] [--game-version GAFE01_00] [--project DIR]
```

The decomp must be built once with your own disc image so its villager assets
are extracted (`python configure.py && ninja`). Pick a villager and press
Import to see it in the Model View (left) and Texture View (right).

Paint in the Texture View with the tools in the Paint section: Select (V),
Pen (P), Brush (B), Ellipse (E), and Rectangle (R). Shapes go on the picked
layer in the palette color picked below them. With Select, drag a shape to move
it, its corner handles to scale (Shift keeps proportions), and the round handle
to rotate; drag a single shape's anchors and handles to reshape it, double-click
its outline to add an anchor, and press Delete to remove the picked anchor or
shapes. The pen adds an anchor per click (drag to pull out handles), closes on
its first anchor, and finishes an open path with Enter. Scroll to zoom,
middle-drag to pan. Ctrl+Z and Ctrl+Shift+Z undo and redo, Ctrl+S saves, and
Ctrl+D duplicates the picked shapes.

A project saves as a folder (Save, Save as). Export either replaces the
imported villager's texture set in the decomp, after which the decomp must be
built with `python configure.py --non-matching && ninja`, or writes a new
texture set to a folder with the steps for adding it.

To try the app without a disc, `just demo` writes a generated villager to a
decomp-shaped directory and opens it.

## Development

Enter the development shell with `nix develop`, then run:

```sh
just build
just run -- --decomp PATH
just test
just format
just check
```

- `just test-decomp-sources [PATH]` parses every model source and the
  villager table of a decomp checkout, built or not.
- `just test-decomp PATH` also round-trips every villager (import, export,
  import), saves and loads each as a project, and checks that each texture
  export could replace its source in the decomp; it needs a built checkout.
- `just snapshot [PATH]` renders the first villager of a decomp offscreen to
  `target/model-view.png`; it needs a GPU.

`just check` checks formatting, runs ShellCheck and Clippy, and validates the
flake without rewriting source files. Formatting and shell checks cover tracked
files; stage new files with `git add` to include them.

Install the formatting hook with `just install-hooks`. It formats fully staged
files and leaves partially staged files alone.

Update dependencies with `nix flake update`.

## License

mint-rabbit is licensed under the GNU General Public License, version 3 only
(`GPL-3.0-only`); see [LICENSE](LICENSE).
