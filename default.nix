{
  lib,
  rustPlatform,
  stdenv,
  libGL,
  libxkbcommon,
  vulkan-loader,
  wayland,
  libx11,
  libxcursor,
  libxi,
  libxrandr,
}:

let
  # winit and wgpu load these with dlopen at runtime.
  runtimeLibs = lib.optionals stdenv.hostPlatform.isLinux [
    libGL
    libxkbcommon
    vulkan-loader
    wayland
    libx11
    libxcursor
    libxi
    libxrandr
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
    license = lib.licenses.gpl3Only;
    mainProgram = "mint-rabbit";
    platforms = lib.platforms.unix;
  };
}
