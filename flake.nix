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

          # nixpkgs builds the AT-SPI launcher for NixOS's system profile.
          # The E2E shell also runs on non-NixOS hosts such as GitHub's Ubuntu
          # image, so point its private accessibility bus at store binaries.
          hostAtSpi = pkgs.at-spi2-core.overrideAttrs (old: {
            mesonFlags = map (
              flag:
              if pkgs.lib.hasPrefix "-Ddbus_daemon=" flag then
                "-Ddbus_daemon=${pkgs.dbus}/bin/dbus-daemon"
              else if pkgs.lib.hasPrefix "-Ddbus_broker=" flag then
                "-Ddbus_broker=${pkgs.dbus-broker}/bin/dbus-broker-launch"
              else
                flag
            ) old.mesonFlags;
          });

          waylandE2eLibraries = with pkgs; [
            alsa-lib
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
            # hostAtSpi is referenced by absolute launcher path below, but is
            # deliberately not a shell package: adding its rebuilt library and
            # typelib hooks alongside GTK's stock AT-SPI closure loads two ATK
            # copies and crashes PyGObject during Gtk import.
            packages = with pkgs; [
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
              # Keep the GTK3 fixture on the mature Python/PyGObject combination.
              (python312.withPackages (pythonPackages: [ pythonPackages.pygobject3 ]))
            ];
            buildInputs = waylandE2eLibraries;
            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath waylandE2eLibraries;
            shellHook = ''
              export NO_AT_BRIDGE=0
              export CUA_AT_SPI_BUS_LAUNCHER="${hostAtSpi}/libexec/at-spi-bus-launcher"
              export XDG_DATA_DIRS="${hostAtSpi}/share''${XDG_DATA_DIRS:+:$XDG_DATA_DIRS}"
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
