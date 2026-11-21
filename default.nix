{
  lib,
  stdenv,
}:

stdenv.mkDerivation {
  name = "mint-rabbit";

  src = lib.cleanSource ./.;

  nativeBuildInputs = [ ];
  buildInputs = [ ];

  installPhase = ''
    runHook preInstall

    mkdir -p "$out/bin"
    printf '%s\n' \
      '#!/usr/bin/env bash' \
      'set -euo pipefail' \
      'echo "Hello from mint-rabbit"' \
      >"$out/bin/mint-rabbit"
    chmod +x "$out/bin/mint-rabbit"

    runHook postInstall
  '';

  meta.mainProgram = "mint-rabbit";
}
