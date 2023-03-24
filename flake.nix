{
  description = "iamb";
  nixConfig.bash-prompt = "\[nix-develop\]$ ";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs { inherit system overlays; };
        rust = pkgs.rust-bin.stable.latest.default;
        rustPlatform = pkgs.makeRustPlatform {
          cargo = rust;
          rustc = rust;
        };
        rustPackage = rustPlatform.buildRustPackage {
          pname = "iamb";
          version = "0.0.7";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          nativeBuildInputs = [ pkgs.openssl pkgs.pkgconfig ];
          buildInputs = [ pkgs.openssl ];
        };
      in {
        packages.default = rustPackage;
        devShell = pkgs.mkShell {
          buildInputs = [ (rust.override {
              extensions = [ "rust-src" ];
            })
            pkgs.pkg-config
            pkgs.cargo-tarpaulin
          ];
        };
      });
}
