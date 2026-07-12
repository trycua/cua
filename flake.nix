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
          pkgs = import nixpkgs {
            inherit system;
            # Several Electron-based apps in the background-GUI test matrix
            # (logseq, joplin, zettlr) pin Electron releases that nixos-26.05
            # flags as EOL/insecure. These are read-only smoke tests running in
            # throwaway CI containers, so permit insecure Electron specifically
            # (version-agnostic, so a nixpkgs bump to a newer EOL Electron keeps
            # working without editing an exact version string here).
            config.allowInsecurePredicate =
              pkg: nixpkgs.lib.hasPrefix "electron" (nixpkgs.lib.getName pkg);
          };

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
              # Source-built, headless Rust checks. The desktop behavioral
              # matrix remains an explicit maintainer-dispatched e2e lane.
              cua-driver-linux-rust-unit = import ./nix/cua-driver/tests/rust-unit.nix {
                inherit pkgs;
                src = rustSrc;
              };
            }
            // pkgs.lib.optionalAttrs (system == "x86_64-linux") {
              # NixOS container integration test (x86_64-linux only)
              cua-driver-integration = import ./nix/cua-driver/tests/integration.nix {
                inherit pkgs;
                inherit (pkgs) lib;
                cuaDriverModule = {
                  imports = [ ./nix/cua-driver/module.nix ];
                  services.cua-driver.package = cuaDriverPackage;
                };
              };

              # set_config persistence test — regression for #1923 (fixed in
              # #1928): the {key, value} write shape must persist and read back
              # via get_config (it was silently dropped on Linux before).
              cua-driver-set-config = import ./nix/cua-driver/tests/set-config.nix {
                inherit pkgs;
                inherit (pkgs) lib;
                cuaDriverModule = {
                  imports = [ ./nix/cua-driver/module.nix ];
                  services.cua-driver.package = cuaDriverPackage;
                };
              };

              # set_value EditableText test — regression for #1924 (fixed in
              # #1929): set_value on a GTK4 editable failed with "neither
              # EditableText nor Value". Launches a packaged GTK4 editor
              # (gnome-text-editor) and proves set_value writes via AT-SPI
              # EditableText::SetTextContents.
              cua-driver-set-value = import ./nix/cua-driver/tests/set-value.nix {
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

              # Multi-cursor (MPX) parallel-drag test on a REAL Xorg brought up
              # by NixOS services.xserver (dummy video + libinput, on a seat via
              # a display manager). This is the CI-viable replacement for the
              # hand-launched-Xorg linux-parallel-drag-gif.nix (which timed out
              # because a self-launched Xorg couldn't get a VT/seat in the
              # emulated nixos-test VM). Proves uinput slaves enumerate as X
              # devices, two cursors draw concurrent window-targeted events, and
              # the shield grab keeps focus off the drag.
              cua-driver-linux-parallel-drag-xserver = import ./nix/cua-driver/tests/linux-parallel-drag-xserver.nix {
                inherit pkgs;
                inherit (pkgs) lib;
                cuaDriverModule = {
                  imports = [ ./nix/cua-driver/module.nix ];
                  services.cua-driver.package = cuaDriverPackage;
                };
              };

              # NOTE: cua-driver-linux-parallel-drag-gif (nix/cua-driver/tests/
              # linux-parallel-drag-gif.nix) is intentionally NOT a flake check —
              # it hand-launches Xorg, which can't get a VT/seat in the emulated
              # GHA nixos-test VM. It is superseded by the services.xserver test
              # above and kept only for local/real-X manual runs.
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
                  # "firefox" remains disabled: historically it did not surface its
                  # window within the launch timeout in CI. Container tests run at
                  # native speed, so this may now pass — left disabled pending
                  # verification.
                ) [
                  "chromium"
                  "tk"
                  # GTK3
                  "gtk3-gedit"
                  "gtk3-mousepad"
                  # gtk3-geany / gtk3-abiword temporarily disabled: their huge
                  # AT-SPI trees made the bounds walk + recorder grind in the
                  # previous emulated VM and the jobs timed out. Container tests run
                  # at native speed, so this may now pass — re-enable once verified
                  # fast enough for 700+-node trees.
                  # "gtk3-geany"
                  "gtk3-scite"
                  # "gtk3-abiword"
                  # GTK4
                  "gtk4-characters"
                  # Qt5
                  "qt5-manuskript"
                  "qt5-klog"
                  "qt5-openambit"
                  # Qt6
                  "qt6-kate"
                  "qt6-kcalc"
                  "qt6-okular"
                  "qt6-qownnotes"
                  # Electron
                  "electron-zettlr"
                  "electron-joplin"
                  "electron-logseq"
                ]
              )
            )
            # Native-Wayland TDD matrix — reproduce the cua-driver scenarios on
            # real, NATIVE Wayland sessions (XFCE on labwc/wayfire/sway, plus KDE
            # and GNOME). Apps run as Wayland clients and the tests never set
            # DISPLAY, so the X11-only driver cannot see them: this is a RED suite
            # specifying native Wayland support. One check per (desktop × scenario)
            # and per (desktop × background-GUI app). See
            # nix/cua-driver/tests/wayland/README.md.
            // pkgs.lib.optionalAttrs (system == "x86_64-linux") (
              let
                # NOTE: xfce-wayfire dropped — the wayfire package fails to
                # build in the current nixpkgs pin (wf-config can't link
                # -ldoctest), an upstream packaging bug unrelated to cua-driver.
                # labwc + sway still cover XFCE-on-wlroots.
                waylandDesktops = [
                  "xfce-labwc"
                  "xfce-sway"
                  "kde"
                  "gnome"
                ];
                waylandScenarios = {
                  integration = ./nix/cua-driver/tests/wayland/integration.nix;
                  screenshot = ./nix/cua-driver/tests/wayland/screenshot.nix;
                  cursor-click-gif = ./nix/cua-driver/tests/wayland/cursor-click-gif.nix;
                  background-terminal-gif = ./nix/cua-driver/tests/wayland/background-terminal-gif.nix;
                  parallel-drag = ./nix/cua-driver/tests/wayland/parallel-drag.nix;
                };
                waylandBgApps = [
                  "foot"
                  "gtk3-gedit"
                  "qt6-kcalc"
                ];
                waylandModule = {
                  imports = [ ./nix/cua-driver/module.nix ];
                  services.cua-driver.package = cuaDriverPackage;
                };
                scenarioChecks = pkgs.lib.listToAttrs (
                  pkgs.lib.concatMap (
                    desktop:
                    map (
                      scenario:
                      pkgs.lib.nameValuePair "cua-driver-wayland-${desktop}-${scenario}" (
                        import waylandScenarios.${scenario} {
                          inherit pkgs desktop;
                          inherit (pkgs) lib;
                          cuaDriverModule = waylandModule;
                        }
                      )
                    ) (builtins.attrNames waylandScenarios)
                  ) waylandDesktops
                );
                bgGuiChecks = pkgs.lib.listToAttrs (
                  pkgs.lib.concatMap (
                    desktop:
                    map (
                      app:
                      pkgs.lib.nameValuePair "cua-driver-wayland-${desktop}-background-gui-${app}" (
                        import ./nix/cua-driver/tests/wayland/background-gui.nix {
                          inherit pkgs desktop app;
                          inherit (pkgs) lib;
                          cuaDriverModule = waylandModule;
                        }
                      )
                    ) waylandBgApps
                  ) waylandDesktops
                );
              in
              scenarioChecks // bgGuiChecks
            );
        }
      )
    // {
      # NixOS module — consumers must set services.cua-driver.package
      # (or use the per-system package from self.packages)
      nixosModules.cua-driver = ./nix/cua-driver/module.nix;
    };
}
