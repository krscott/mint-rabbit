# Agent notes

- The ac-decomp checkout is in `$MINT_AC_DECOMP`, which the app and the
  decomp `just` recipes default to. If it's unset, ask the user where the
  checkout is; don't write its path into the repo.
- Run builds and tests in the dev shell (`nix develop`). Clippy and the Vulkan
  loader the app and GPU tests need come from it.
- To drive the app on a headless display, use X11 with `WGPU_BACKEND=vulkan`;
  the headless Wayland surface and GL both fail to get an adapter.
