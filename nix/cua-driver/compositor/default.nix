# cua-compositor: a minimal headless wlroots compositor that cua-driver spawns
# as its nested session (CUA_WAYLAND_NEST_COMPOSITOR=cua-compositor) to perform
# focus-free keyboard injection and multi-cursor pointer injection via a unix
# control socket. Built from wlroots' own upstream tinywl example (pinned to the
# same wlroots_0_19 the patch targets) transformed by cua_compositor_patch.py —
# so it tracks the exact wlroots API in nixpkgs and needs no vendored C.
{
  stdenv,
  lib,
  pkg-config,
  python3,
  wayland,
  wayland-scanner,
  wayland-protocols,
  wlroots_0_19,
  libxkbcommon,
  pixman,
  wayland-utils,
}:

stdenv.mkDerivation {
  pname = "cua-compositor";
  version = "0.1.0";

  src = lib.cleanSource ./.;

  nativeBuildInputs = [
    pkg-config
    python3
    wayland-scanner
  ];
  buildInputs = [
    wayland
    wayland-protocols
    wlroots_0_19
    libxkbcommon
    pixman
  ];

  buildPhase = ''
    runHook preBuild
    cp ${wlroots_0_19.src}/tinywl/tinywl.c tinywl.c
    python3 cua_compositor_patch.py tinywl.c cua-compositor.c
    wp=${wayland-protocols}/share/wayland-protocols
    wayland-scanner server-header "$wp/stable/xdg-shell/xdg-shell.xml" xdg-shell-protocol.h
    wayland-scanner private-code  "$wp/stable/xdg-shell/xdg-shell.xml" xdg-shell-protocol.c
    $CC -DWLR_USE_UNSTABLE -I. cua-compositor.c xdg-shell-protocol.c -o cua-compositor \
      $(pkg-config --cflags --libs wlroots-0.19 wayland-server xkbcommon pixman-1)
    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall
    install -Dm755 cua-compositor $out/bin/cua-compositor
    runHook postInstall
  '';

  # cua-driver draws the agent cursor only on a compositor that advertises
  # wlr-layer-shell; without it the cursor and badge are silently absent.
  doInstallCheck = true;
  nativeInstallCheckInputs = [ wayland-utils ];
  installCheckPhase = ''
    runHook preInstallCheck
    export XDG_RUNTIME_DIR=$(mktemp -d) WLR_BACKENDS=headless WLR_RENDERER=pixman \
      WLR_LIBINPUT_NO_DEVICES=1 WAYLAND_DISPLAY=wayland-0
    $out/bin/cua-compositor > compositor.log 2>&1 &
    compositor=$!
    for _ in $(seq 100); do
      [ -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" ] && break
      sleep 0.1
    done
    wayland-info > globals.txt || true
    kill $compositor
    for global in zwlr_layer_shell_v1 zwlr_screencopy_manager_v1 zwlr_foreign_toplevel_manager_v1; do
      grep -q "'$global'" globals.txt || {
        echo "cua-compositor does not advertise $global" >&2
        cat globals.txt compositor.log >&2
        exit 1
      }
    done
    runHook postInstallCheck
  '';

  meta = {
    description = "Headless wlroots compositor with focus-free + multi-cursor input injection for cua-driver";
    platforms = lib.platforms.linux;
    mainProgram = "cua-compositor";
  };
}
