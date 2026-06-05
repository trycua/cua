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
  version = "0.5.1";

  inherit src;

  # Use cargoHash (fetchCargoVendor) rather than cargoLock.lockFile because
  # the workspace Cargo.lock includes macOS-only crates (apple-metal, apple-cf)
  # that may be unreachable from crates.io. fetchCargoVendor handles this
  # gracefully via `cargo vendor`.
  # Bumped when the dependency set changes (added `atspi`/zbus for native
  # AT-SPI). If this mismatches, the nix build prints the expected value.
  cargoHash = "sha256-P+f+ma8ZDWhhk1TTCGgbLTp4zU/uuh4vHYYQMIjlCbU=";

  # Build only the main binary crate. The workspace also contains
  # platform-macos, platform-windows, cua-driver-uia, and focus-monitor-win
  # which are gated behind cfg(target_os) and won't compile on Linux.
  # Using -p cua-driver ensures Cargo only resolves Linux dependencies.
  cargoBuildFlags = [ "-p" "cua-driver" ];
  cargoTestFlags = [ "-p" "cua-driver" ];

  # The entire Linux dependency chain is pure Rust:
  #   x11rb     -> RustConnection (no libxcb C binding)
  #   ureq      -> rustls (no openssl)
  #   tiny-skia -> pure Rust 2D graphics
  #   ring      -> compiles own C/asm via stdenv's cc
  nativeBuildInputs = [ ];
  buildInputs = [ ];

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
