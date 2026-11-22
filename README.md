# mint-rabbit

A 3D model and vector texture painter. Its first export target is Animal
Crossing (GameCube) villagers in an [ac-decomp](https://github.com/ACreTeam/ac-decomp)
checkout. See [GLOSSARY.md](GLOSSARY.md) for the domain language and
[docs/adr](docs/adr) for design decisions.

## Usage

```sh
mint-rabbit --decomp PATH/TO/ac-decomp [--game-version GAFE01_00]
```

The decomp must be built once with your own disc image so its villager assets
are extracted (`python configure.py && ninja`). Pick a villager and press
Import to see it in the Model View (left) and Texture View (right).

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

- `just test-decomp [PATH]` also parses every model source in a decomp
  checkout and round-trips every villager (import, export, import); the
  round trip needs a built checkout.
- `just snapshot [PATH]` renders the first villager of a decomp offscreen to
  `target/model-view.png`; it needs a GPU.

`just check` checks formatting, runs ShellCheck and Clippy, and validates the
flake without rewriting source files. Formatting and shell checks cover tracked
files; stage new files with `git add` to include them.

Install the formatting hook with `just install-hooks`. It formats fully staged
files and leaves partially staged files alone.

Update dependencies with `nix flake update`.
