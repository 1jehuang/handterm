{
  description = "Wayland-native terminal emulator focused on low latency";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        handterm = pkgs.rustPlatform.buildRustPackage {
          pname = "handterm";
          version = "0.1.0";
          src = ./.;

          cargoLock.lockFile = ./Cargo.lock;

          nativeBuildInputs = with pkgs; [
            pkg-config
          ];

          buildInputs = with pkgs; [
            freetype
            fontconfig
            wayland
          ] ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
            pkgs.libiconv
          ];

          meta = {
            description = "Wayland-native terminal focused on low latency";
            homepage = "https://github.com/levonk/handterm";
            license = pkgs.lib.licenses.mit;
            mainProgram = "handterm";
          };
        };
      in
      {
        packages.default = handterm;
        apps.default = {
          type = "app";
          program = "${handterm}/bin/handterm";
        };
      }
    );
}
