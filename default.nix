{
  lib,
  rustPlatform,
  stdenv,
  libGL,
  libxkbcommon,
  vulkan-loader,
  wayland,
  xorg,
}:

let
  # winit and wgpu load these with dlopen at runtime.
  runtimeLibs = lib.optionals stdenv.hostPlatform.isLinux [
    libGL
    libxkbcommon
    vulkan-loader
    wayland
    xorg.libX11
    xorg.libXcursor
    xorg.libXi
    xorg.libXrandr
  ];
in
rustPlatform.buildRustPackage {
  pname = "mint-rabbit";
  version = "0.1.0";

  src = lib.cleanSource ./.;

  cargoLock.lockFile = ./Cargo.lock;

  buildInputs = runtimeLibs;

  postFixup = lib.optionalString stdenv.hostPlatform.isLinux ''
    patchelf --add-rpath ${lib.makeLibraryPath runtimeLibs} "$out/bin/mint-rabbit"
  '';

  passthru = { inherit runtimeLibs; };

  meta = {
    description = "3D model and vector texture painter";
    mainProgram = "mint-rabbit";
    platforms = lib.platforms.unix;
  };
}
