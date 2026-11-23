{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      nixpkgs,
      flake-utils,
      ...
    }:
    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
    in
    flake-utils.lib.eachSystem supportedSystems (
      system:
      let
        pkgs = nixpkgs.legacyPackages.${system};

        package = pkgs.callPackage ./default.nix { };
      in
      {
        packages = {
          "mint-rabbit" = package;
          default = package;
        };

        devShells.default = pkgs.mkShell {
          inputsFrom = [ package ];

          packages = with pkgs; [
            just
            nixfmt
            shfmt
            shellcheck
            bash
            cargo
            rustc
            rustfmt
            clippy
            rust-analyzer
            # `just run --decomp` builds the decomp's assets.
            python3
            ninja
          ];

          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath package.runtimeLibs;
        };

        formatter = pkgs.nixfmt;
      }
    );
}
