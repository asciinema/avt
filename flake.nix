{
  description = "asciinema virtual terminal";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixpkgs-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    let
      packageToml =
        (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package;
      msrv = packageToml.rust-version;
    in flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };
        mkDevShell = rust:
          pkgs.mkShell {
            nativeBuildInputs = [
              (rust.override { extensions = [ "rust-src" "rust-analyzer" ]; })
            ];
          };
      in {
        devShells.default = mkDevShell pkgs.rust-bin.stable.latest.default;
        devShells.msrv = mkDevShell pkgs.rust-bin.stable.${msrv}.default;
      });
}
