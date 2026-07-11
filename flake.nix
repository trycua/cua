{
  description = "CUA - Computer Use Agent";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      ...
    }:
    flake-utils.lib.eachSystem
      [
        "x86_64-linux"
        "aarch64-linux"
      ]
      (
        system:
        let
          pkgs = import nixpkgs { inherit system; };

          rustSrc = ./libs/cua-driver/rust;

          cuaDriverPackage = import ./nix/cua-driver/package.nix {
            inherit pkgs;
            src = rustSrc;
          };

          waylandE2eLibraries = with pkgs; [
            alsa-lib
            atk
            cairo
            cups
            dbus
            expat
            glib
            gtk3
            libayatana-appindicator
            libdrm
            libei
            libgbm
            librsvg
            libsoup_3
            libx11
            libxcb
            libxcomposite
            libxdamage
            libxdo
            libxext
            libxfixes
            libxi
            libxkbcommon
            libxrandr
            libxtst
            mesa
            nspr
            nss
            openssl
            pango
            pipewire
            webkitgtk_4_1
          ];
        in
        {
          packages = {
            cua-driver = cuaDriverPackage;
            default = cuaDriverPackage;
          };

          checks = {
            cua-driver-build = cuaDriverPackage;
            cua-driver-linux-rust-unit = import ./nix/cua-driver/tests/rust-unit.nix {
              inherit pkgs;
              src = rustSrc;
            };
          };

          devShells.cua-driver-wayland-e2e = pkgs.mkShell {
            packages = with pkgs; [
              at-spi2-core
              cargo
              clang
              dbus
              ffmpeg
              gobject-introspection
              grim
              jq
              nodejs
              pkg-config
              procps
              rustc
              rustfmt
              sway
              wf-recorder
              wtype
              (python3.withPackages (pythonPackages: [ pythonPackages.pygobject3 ]))
            ];
            buildInputs = waylandE2eLibraries;
            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath waylandE2eLibraries;
            shellHook = ''
              export NO_AT_BRIDGE=0
            '';
          };
        }
      )
    // {
      # NixOS module — consumers must set services.cua-driver.package
      # (or use the per-system package from self.packages)
      nixosModules.cua-driver = ./nix/cua-driver/module.nix;
    };
}
