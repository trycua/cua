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

          # cua-driver builds inside libs/: its crates reach the shared
          # cua-telemetry crate in the libs/cua workspace (which inherits that
          # workspace's package fields and reads the image catalog and the
          # teleport target list at compile time), and platform-linux embeds
          # the GNOME Shell helper from wayland-helper.
          driverSharedFiles = [
            ./libs/cua-driver/rust
            ./libs/cua-driver/wayland-helper
            ./libs/cua/Cargo.toml
            ./libs/cua/crates/cua-telemetry
            ./libs/cua/crates/cua-teleport/src/ux/targets.json
            ./libs/images/sandbox-images.json
          ];
          rustSrc = pkgs.lib.fileset.toSource {
            root = ./libs;
            fileset = pkgs.lib.fileset.unions driverSharedFiles;
          };
          rustTestSrc = pkgs.lib.fileset.toSource {
            root = ./libs;
            fileset = pkgs.lib.fileset.unions (
              driverSharedFiles
              ++ [
                ./libs/cua-driver/compat-fixtures
                ./libs/cua-driver/tests/fixtures/shared/web/index.html
                ./libs/cua-driver/tests/perception-demo/evidence-manifest.schema.json
              ]
            );
          };

          cuaDriverPackage = import ./nix/cua-driver/package.nix {
            inherit pkgs;
            src = rustSrc;
            sourceSubdir = "cua-driver/rust";
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

          # wf-recorder 0.6.0 always binds linux-dmabuf version 4. niri without
          # a DRM render node (any GPU-less runner) advertises version 3, so the
          # bind is a protocol error and recording never starts. Bind the
          # advertised version and request dmabuf feedback only when it exists.
          niriWfRecorder = pkgs.wf-recorder.overrideAttrs (old: {
            postPatch = (old.postPatch or "") + ''
              substituteInPlace src/main.cpp \
                --replace-fail "const char *interface, uint32_t) {" \
                  "const char *interface, uint32_t version) {" \
                --replace-fail "&zwp_linux_dmabuf_v1_interface, 4);" \
                  "&zwp_linux_dmabuf_v1_interface, version < 4 ? version : 4);" \
                --replace-fail "if (dmabuf) {" "if (dmabuf && version >= 4) {"
            '';
          });

          waylandE2eShell = extraPackages: pkgs.mkShell {
            # hostAtSpi is referenced by absolute launcher path below, but is
            # deliberately not a shell package: adding its rebuilt library and
            # typelib hooks alongside GTK's stock AT-SPI closure loads two ATK
            # copies and crashes PyGObject during Gtk import.
            # Extra packages come first so a lane can shadow a base tool.
            packages = extraPackages ++ (with pkgs; [
              cargo
              clang
              chromium
              dbus
              ffmpeg
              # The shared Wayland history gate requires an unlocked Secret Service.
              gnome-keyring
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
            ]);
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
              sourceSubdir = "cua-driver/rust";
            };
            cua-driver-policy-yaml = import ./nix/cua-driver/tests/policy-yaml.nix {
              inherit pkgs;
              cuaDriver = cuaDriverPackage;
            };
            cua-driver-policy-rego = import ./nix/cua-driver/tests/policy-rego.nix {
              inherit pkgs;
              cuaDriver = cuaDriverPackage;
            };
          };

          devShells.cua-driver-wayland-e2e = waylandE2eShell [ ];
          devShells.cua-driver-inject-e2e = waylandE2eShell [ cuaCompositorPackage ];
          # niri has no headless backend: it runs on its winit backend inside a
          # headless cage, and wlr-randr sizes cage's output. niri renders only
          # through EGL, so name the store's Mesa vendor (llvmpipe without a GPU)
          # instead of whatever the host's glvnd directories provide.
          # GTK on Wayland ignores gtk-xft-dpi in settings.ini and takes it from
          # GSettings' text-scaling-factor; without that schema, WebKitGTK
          # reads a screen DPI of -1 and lays pages out at devicePixelRatio
          # -1/96. Name the store's schemas and keep their defaults in memory,
          # so neither the host's schemas nor its dconf play a part.
          devShells.cua-driver-niri-e2e =
            (waylandE2eShell (
              with pkgs;
              [
                cage
                niri
                niriWfRecorder
                wlr-randr
              ]
            )).overrideAttrs
              (old: {
                shellHook = old.shellHook + ''
                  export __EGL_VENDOR_LIBRARY_FILENAMES="${pkgs.mesa}/share/glvnd/egl_vendor.d/50_mesa.json"
                  export GSETTINGS_SCHEMA_DIR="${pkgs.glib.getSchemaPath pkgs.gsettings-desktop-schemas}"
                  export GSETTINGS_BACKEND=memory
                '';
              });
        }
      )
    // {
      # NixOS module — consumers must set services.cua-driver.package
      # (or use the per-system package from self.packages)
      nixosModules.cua-driver = ./nix/cua-driver/module.nix;
    };
}
