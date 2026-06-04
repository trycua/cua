{
  description = "CUA - Computer Use Agent";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
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
        in
        {
          packages = {
            cua-driver = cuaDriverPackage;
            default = cuaDriverPackage;
          };

          checks =
            {
              cua-driver-build = cuaDriverPackage;
            }
            // pkgs.lib.optionalAttrs (system == "x86_64-linux") {
              # NixOS VM integration test (x86_64-linux only)
              cua-driver-integration = import ./nix/cua-driver/tests/integration.nix {
                inherit pkgs;
                inherit (pkgs) lib;
                cuaDriverModule = {
                  imports = [ ./nix/cua-driver/module.nix ];
                  services.cua-driver.package = cuaDriverPackage;
                };
              };

              # Screenshot test — uses cua-driver's own get_window_state tool
              # to capture a screenshot via MCP, proving the driver can see the display
              cua-driver-screenshot = import ./nix/cua-driver/tests/screenshot.nix {
                inherit pkgs;
                inherit (pkgs) lib;
                cuaDriverModule = {
                  imports = [ ./nix/cua-driver/module.nix ];
                  services.cua-driver.package = cuaDriverPackage;
                };
              };

              cua-driver-linux-cursor-click-gif = import ./nix/cua-driver/tests/linux-cursor-click-gif.nix {
                inherit pkgs;
                inherit (pkgs) lib;
                cuaDriverModule = {
                  imports = [ ./nix/cua-driver/module.nix ];
                  services.cua-driver.package = cuaDriverPackage;
                };
              };

              cua-driver-linux-background-terminal-gif = import ./nix/cua-driver/tests/linux-background-terminal-gif.nix {
                inherit pkgs;
                inherit (pkgs) lib;
                cuaDriverModule = {
                  imports = [ ./nix/cua-driver/module.nix ];
                  services.cua-driver.package = cuaDriverPackage;
                };
              };
            }
            // pkgs.lib.optionalAttrs (system == "x86_64-linux") (
              # Background GUI input coverage — one independent matrix job per
              # app, proving focus-free typing into real toolkit/browser windows.
              pkgs.lib.listToAttrs (
                map (
                  app:
                  pkgs.lib.nameValuePair "cua-driver-linux-background-gui-${app}" (
                    import ./nix/cua-driver/tests/linux-background-gui.nix {
                      inherit pkgs app;
                      inherit (pkgs) lib;
                      cuaDriverModule = {
                        imports = [ ./nix/cua-driver/module.nix ];
                        services.cua-driver.package = cuaDriverPackage;
                      };
                    }
                  )
                  # Real-app matrix: 5 apps per toolkit category run as a LENIENT,
                  # READ-ONLY skeleton (find window + driver page/get_text + GIF;
                  # focus-free WRITE / typed-text assertions are added later via
                  # trajectories). chromium keeps the full CDP focus-free-write
                  # override; tk is the negative-control full entry (Tk `send`).
                  # "firefox" remains disabled: under the emulated CI VM (no KVM)
                  # it does not surface its window within the launch timeout.
                ) [
                  "chromium"
                  "tk"
                  # GTK3
                  "gtk3-gedit"
                  "gtk3-mousepad"
                  "gtk3-geany"
                  "gtk3-scite"
                  "gtk3-abiword"
                  # GTK4
                  "gtk4-text-editor"
                  "gtk4-characters"
                  "gtk4-console"
                  "gtk4-contacts"
                  "gtk4-calendar"
                  # Qt5
                  "qt5-manuskript"
                  "qt5-klog"
                  "qt5-wsjtx"
                  "qt5-qsstv"
                  "qt5-openambit"
                  # Qt6
                  "qt6-kate"
                  "qt6-kcalc"
                  "qt6-okular"
                  "qt6-ghostwriter"
                  "qt6-qownnotes"
                  # Electron
                  "electron-marktext"
                  "electron-zettlr"
                  "electron-vscodium"
                  "electron-joplin"
                  "electron-logseq"
                ]
              )
            );
        }
      )
    // {
      # NixOS module — consumers must set services.cua-driver.package
      # (or use the per-system package from self.packages)
      nixosModules.cua-driver = ./nix/cua-driver/module.nix;
    };
}
