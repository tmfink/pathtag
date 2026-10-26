# Usage:
# Use existing rustup toolchain:
#   nix develop
# Use Rust and Cargo from Nix:
#   nix develop .#nix-rust

{
  description = "Rust + rexiv2 development shell";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];

      forAllSystems = nixpkgs.lib.genAttrs systems;
    in {
      devShells = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};

          commonPackages = with pkgs; [
            pkg-config
            glib
            gexiv2_0_10
            exiv2

            # Native GUI / Wayland dependencies
            wayland
            libxkbcommon

            # OpenGL / EGL
            libglvnd
          ];

          mkDevShell = extraPackages: pkgs.mkShell {
            packages = commonPackages ++ extraPackages;

            shellHook = ''
              export LD_LIBRARY_PATH="/run/opengl-driver/lib:${pkgs.lib.makeLibraryPath [
                pkgs.wayland
                pkgs.libxkbcommon
                pkgs.libglvnd
              ]}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

              export __EGL_VENDOR_LIBRARY_FILENAMES="/run/opengl-driver/share/glvnd/egl_vendor.d/50_mesa.json"
            '';
          };
        in {
          default = mkDevShell [];
          nix-rust = mkDevShell (with pkgs; [
            rustc
            cargo
          ]);
        });
    };
}

