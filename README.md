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
layer, which must be visible and unlocked, in the palette color picked below
them.

- Select: drag a shape to move it, the square handles outside its corners to
  scale (Shift keeps proportions), and the round handle to rotate (Shift snaps
  to 15°). With one shape picked, drag its anchors and handles to reshape it
  (Shift breaks a smooth anchor's handles apart), double-click its outline to
  add an anchor, and press Delete to remove the picked anchor or shapes.
- Pen: each click adds an anchor (drag to pull out handles), clicking the first
  anchor closes the path, Enter finishes it open, and Delete removes the last
  anchor.
- Escape drops a drawing in progress. Scroll to zoom, middle-drag to pan, and
  middle double-click to fit.
- Ctrl+Z undoes, Ctrl+Shift+Z or Ctrl+Y redoes, Ctrl+S saves, and Ctrl+D
  duplicates the picked shapes.

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
