# Builds cua-driver (Rust MCP server binary) for Linux.
#
# The cua-driver binary speaks MCP JSON-RPC 2.0 over stdio and provides
# 40+ tools for screen capture, mouse/keyboard input, window management,
# and accessibility-based element interaction.
#
# Usage:
#   cuaDriver = import ./package.nix { inherit pkgs; src = ./../../libs/cua-driver/rust; };
#
{
  pkgs,
  src,
  ...
}:

pkgs.rustPlatform.buildRustPackage {
  pname = "cua-driver";
  # Read the version from the single source of truth (the workspace manifest
  # bumpversion edits) so it can never drift from Cargo.toml the way a
  # hardcoded literal silently did across 0.5.3 -> 0.5.6.
  version = (pkgs.lib.importTOML "${src}/Cargo.toml").workspace.package.version;

  inherit src;

  # Vendor straight from the committed Cargo.lock instead of a manual cargoHash.
  # `importCargoLock` derives every dependency's fixed-output hash from the
  # lockfile itself, so there is NO hash to hand-maintain: Cargo.lock can change
  # (a version bump, a dependency update) and the build keeps working. The old
  # cargoHash approach hashed the vendored lockfile, so any Cargo.lock change —
  # even a workspace-version bump the release bump leaves behind — silently
  # invalidated it and turned every nix job red until someone recomputed it.
  # Every dependency here is a crates.io registry crate (the macOS-only
  # apple-cf/apple-metal/objc2 crates included) and there are zero git deps, so
  # importCargoLock needs no `outputHashes` overrides.
  cargoLock.lockFile = "${src}/Cargo.lock";

  # Build only the main binary crate. The workspace also contains
  # platform-macos, platform-windows, and cua-driver-uia
  # which are gated behind cfg(target_os) and won't compile on Linux.
  # Using -p cua-driver ensures Cargo only resolves Linux dependencies.
  #
  # Enable both portal capabilities so the Nix build pulls the GNOME/KDE
  # portal stack (PipeWire ScreenCast per-window capture + libei
  # RemoteDesktop input). nixpkgs provides a recent enough
  # PipeWire (>= 0.3.40 needed by libspa-sys) and libei, so this feature
  # builds fine here. Release builds enable portal input as well; keeping
  # capture explicit here validates both halves of the split feature contract.
  cargoBuildFlags = [ "-p" "cua-driver" "--features" "portal-input,portal-capture" ];
  cargoTestFlags = [ "-p" "cua-driver" "--features" "portal-input,portal-capture" ];

  # Mostly pure Rust:
  #   x11rb     -> RustConnection (no libxcb C binding)
  #   ureq      -> rustls (no openssl)
  #   tiny-skia -> pure Rust 2D graphics
  #   ring      -> compiles own C/asm via stdenv's cc
  # The `x11` crate (raw Xlib FFI for MPX multi-cursor drags) needs
  # libX11/libXi/libXtst via pkg-config. The Wayland-parity work
  # (round 2/3 — wayland::portal_screencast / wayland::libei) adds:
  #   pipewire 0.8 -> needs libpipewire-0.3 + libspa headers via pkg-config
  #                   + bindgen (clang) for SPA pod generation
  #   reis 0.7     -> pure Rust libei binding, but the workspace pulls
  #                   libei-dev transitively through ashpd's tokio feature
  nativeBuildInputs = with pkgs; [
    pkg-config
    # bindgen needs a libclang at build time for the libpipewire / libspa
    # SPA pod codegen. rust-bindgen also picks up `stdbool.h` etc. from
    # clang's resource dir, not glibc.
    rustPlatform.bindgenHook
  ];
  buildInputs = with pkgs; [
    libx11
    libxi
    libxtst
    libxext
    # Wayland-parity additions: PipeWire is needed by the portal
    # ScreenCast capture path (wayland::portal_screencast). pipewire
    # already pulls libspa transitively in nixpkgs.
    pipewire
    # libei via reis — same as the ashpd RemoteDesktop+EIS flow.
    libei
    # reis links its keyboard support directly against libxkbcommon.
    libxkbcommon
  ];

  # Skip tests that require a running X11 display or AT-SPI bus
  doCheck = false;

  meta = with pkgs.lib; {
    description = "Cross-platform MCP server for computer-use automation";
    homepage = "https://github.com/trycua/cua";
    license = licenses.mit;
    mainProgram = "cua-driver";
    platforms = platforms.linux;
  };
}
