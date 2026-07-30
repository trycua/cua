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
          rustTestSrc = pkgs.lib.fileset.toSource {
            root = ./libs/cua-driver;
            fileset = pkgs.lib.fileset.unions [
              ./libs/cua-driver/rust
              ./libs/cua-driver/wayland-helper
              ./libs/cua-driver/compat-fixtures
              ./libs/cua-driver/tests/fixtures/shared/web/index.html
            ];
          };

          cuaDriverPackage = import ./nix/cua-driver/package.nix {
            inherit pkgs;
            src = rustSrc;
          };

          cuaCompositorPackage = pkgs.callPackage ./nix/cua-driver/compositor { };

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

          waylandE2eShell = extraPackages: pkgs.mkShell {
            # hostAtSpi is referenced by absolute launcher path below, but is
            # deliberately not a shell package: adding its rebuilt library and
            # typelib hooks alongside GTK's stock AT-SPI closure loads two ATK
            # copies and crashes PyGObject during Gtk import.
            packages = (with pkgs; [
              cargo
              clang
              chromium
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
              unzip
              wf-recorder
              wtype
              # Keep the GTK3 fixture on the mature Python/PyGObject combination.
              (python312.withPackages (pythonPackages: [ pythonPackages.pygobject3 ]))
            ]) ++ extraPackages;
            buildInputs = waylandE2eLibraries;
            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath waylandE2eLibraries;
            shellHook = ''
              export NO_AT_BRIDGE=0
              export CUA_AT_SPI_BUS_LAUNCHER="${hostAtSpi}/libexec/at-spi-bus-launcher"
              export XDG_DATA_DIRS="${hostAtSpi}/share''${XDG_DATA_DIRS:+:$XDG_DATA_DIRS}"
            '';
          };
        in
        {
          packages = {
            cua-compositor = cuaCompositorPackage;
            cua-driver = cuaDriverPackage;
            default = cuaDriverPackage;
          };

          checks = {
            cua-compositor-build = cuaCompositorPackage;
            cua-driver-build = cuaDriverPackage;
            cua-driver-linux-rust-unit = import ./nix/cua-driver/tests/rust-unit.nix {
              inherit pkgs;
              src = rustTestSrc;
              sourceSubdir = "rust";
            };
            cua-driver-policy-yaml = import ./nix/cua-driver/tests/policy-yaml.nix {
              inherit pkgs;
              cuaDriver = cuaDriverPackage;
            };
            cua-driver-policy-rego = import ./nix/cua-driver/tests/policy-rego.nix {
              inherit pkgs;
              cuaDriver = cuaDriverPackage;
            };
          }
          // pkgs.lib.optionalAttrs (system == "x86_64-linux") (
            let
              cuaDriverModule = {
                imports = [ ./nix/cua-driver/module.nix ];
                services.cua-driver.package = cuaDriverPackage;
              };
              x11Check = file: import file {
                inherit pkgs cuaDriverModule;
                inherit (pkgs) lib;
              };
              waylandDesktops = [ "xfce-labwc" "xfce-sway" "kde" "gnome" ];
              waylandScenarios = {
                integration = ./nix/cua-driver/tests/wayland/integration.nix;
                screenshot = ./nix/cua-driver/tests/wayland/screenshot.nix;
                cursor-click-gif = ./nix/cua-driver/tests/wayland/cursor-click-gif.nix;
                background-terminal-gif = ./nix/cua-driver/tests/wayland/background-terminal-gif.nix;
                parallel-drag = ./nix/cua-driver/tests/wayland/parallel-drag.nix;
              };
              waylandChecks = pkgs.lib.listToAttrs (pkgs.lib.concatMap (
                desktop: map (scenario: pkgs.lib.nameValuePair
                  "cua-driver-wayland-${desktop}-${scenario}"
                  (import waylandScenarios.${scenario} {
                    inherit pkgs desktop cuaDriverModule;
                    inherit (pkgs) lib;
                  })
                ) (builtins.attrNames waylandScenarios)
              ) waylandDesktops);
              waylandGuiChecks = pkgs.lib.listToAttrs (pkgs.lib.concatMap (
                desktop: map (app: pkgs.lib.nameValuePair
                  "cua-driver-wayland-${desktop}-background-gui-${app}"
                  (import ./nix/cua-driver/tests/wayland/background-gui.nix {
                    inherit pkgs desktop app cuaDriverModule;
                    inherit (pkgs) lib;
                  })
                ) [ "foot" "gtk3-gedit" "qt6-kcalc" ]
              ) waylandDesktops);
              x11GuiChecks = pkgs.lib.listToAttrs (map (app: pkgs.lib.nameValuePair
                "cua-driver-linux-background-gui-${app}"
                (import ./nix/cua-driver/tests/linux-background-gui.nix {
                  inherit pkgs app cuaDriverModule;
                  inherit (pkgs) lib;
                })
              ) [ "gtk4-characters" "qt6-kcalc" "electron-zettlr" ]);
            in
            {
              # Rust owns behavior assertions; these NixOS checks certify the
              # package in desktop sessions and preserve visual evidence.
              cua-driver-integration = x11Check ./nix/cua-driver/tests/integration.nix;
              cua-driver-screenshot = x11Check ./nix/cua-driver/tests/screenshot.nix;
              cua-driver-linux-cursor-click-gif = x11Check ./nix/cua-driver/tests/linux-cursor-click-gif.nix;
              cua-driver-linux-background-terminal-gif = x11Check ./nix/cua-driver/tests/linux-background-terminal-gif.nix;
              cua-driver-linux-parallel-drag-xserver = x11Check ./nix/cua-driver/tests/linux-parallel-drag-xserver.nix;
            }
            // x11GuiChecks
            // waylandChecks
            // waylandGuiChecks
          );

          devShells.cua-driver-wayland-e2e = waylandE2eShell [ ];
          devShells.cua-driver-inject-e2e = waylandE2eShell [ cuaCompositorPackage ];
        }
      )
    // {
      # NixOS module — consumers must set services.cua-driver.package
      # (or use the per-system package from self.packages)
      nixosModules.cua-driver = ./nix/cua-driver/module.nix;
    };
}
