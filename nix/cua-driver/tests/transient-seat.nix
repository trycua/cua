# Containerized evidence collector for the transient Wayland-seat Rust contract.
#
# The Rust binary owns behavioral assertions. This NixOS container only
# provisions the compositor, fixture, GIF capture, and failure-preserving
# evidence export consumed by CI.
{
  pkgs,
  src,
  cuaCompositor,
  ...
}:

let
  rustSrc = "${src}/rust";

  transientSeatTest = pkgs.rustPlatform.buildRustPackage {
    pname = "cua-driver-transient-seat-e2e";
    version = (pkgs.lib.importTOML "${rustSrc}/Cargo.toml").workspace.package.version;
    inherit src;

    cargoLock.lockFile = "${rustSrc}/Cargo.lock";
    postUnpack = ''
      sourceRoot="$sourceRoot/rust"
    '';
    cargoBuildFlags = [
      "-p"
      "cua-driver"
      "--test"
      "transient_seat_behavior_test"
    ];
    doCheck = false;

    nativeBuildInputs = with pkgs; [
      pkg-config
      rustPlatform.bindgenHook
    ];
    buildInputs = with pkgs; [
      libx11
      libxi
      libxtst
      libxext
      pipewire
      libei
      libxkbcommon
    ];

    installPhase = ''
      runHook preInstall
      mkdir -p "$out/bin"
      test_binary="$(find target -type f -path '*/release/deps/transient_seat_behavior_test-*' -perm -u+x -print -quit)"
      test -n "$test_binary"
      install -Dm755 "$test_binary" "$out/bin/transient-seat-behavior"
      runHook postInstall
    '';
  };

  electronFixture = pkgs.runCommand "cua-driver-transient-seat-electron-fixture" { } ''
    mkdir -p "$out/harness-electron/app/web"
    cp -R ${src}/tests/fixtures/apps/cross-platform/electron/. \
      "$out/harness-electron/app/"
    cp -R ${src}/tests/fixtures/shared/web/. \
      "$out/harness-electron/app/web/"
    cat > "$out/harness-electron/CuaTestHarness.Electron" <<'SH'
#!${pkgs.runtimeShell}
exec ${pkgs.electron}/bin/electron "$(dirname "$0")/app" "$@"
SH
    chmod +x "$out/harness-electron/CuaTestHarness.Electron"
  '';

  recordGif = pkgs.writeShellScript "record-transient-seat-gif" ''
    set -eu
    frames_dir="$1"
    output_gif="$2"
    stop_file="$3"
    log_file="$4"

    rm -f "$output_gif" "$stop_file" "$log_file"
    rm -rf "$frames_dir"
    mkdir -p "$frames_dir"

    frame=0
    while [ ! -f "$stop_file" ]; do
      path="$(printf '%s/frame-%04d.png' "$frames_dir" "$frame")"
      grim "$path" >>"$log_file" 2>&1 || true
      frame=$((frame + 1))
      sleep 0.2
    done

    if ls "$frames_dir"/frame-*.png >/dev/null 2>&1; then
      convert -delay 12 -loop 0 "$frames_dir"/frame-*.png "$output_gif" >>"$log_file" 2>&1
    fi
  '';
in
pkgs.testers.nixosTest {
  name = "cua-driver-transient-seat";

  containers.machine = { pkgs, ... }: {
    environment.systemPackages = [
      cuaCompositor
      transientSeatTest
      electronFixture
      pkgs.dbus
      pkgs.grim
      pkgs.imagemagick
      pkgs.coreutils
      pkgs.procps
    ];
  };

  testScript = ''
    machine.start()
    machine.wait_for_unit("multi-user.target")

    machine.succeed("install -d -m 700 /tmp/cua-runtime /tmp/transient-seat-evidence")
    machine.succeed("dbus-daemon --session --address=unix:path=/tmp/cua-session-bus --fork")
    machine.execute(
        "env XDG_RUNTIME_DIR=/tmp/cua-runtime WLR_BACKENDS=headless WLR_RENDERER=pixman "
        "WLR_RENDERER_ALLOW_SOFTWARE=1 WLR_LIBINPUT_NO_DEVICES=1 WLR_HEADLESS_OUTPUTS=1 "
        "CUA_INJECT_SOCKET=/tmp/cua-runtime/cua-inject.sock "
        "cua-compositor >/tmp/transient-seat-compositor.log 2>&1 & echo $! >/tmp/cua-compositor.pid"
    )
    machine.sleep(1)
    machine.succeed("kill -0 $(cat /tmp/cua-compositor.pid)")
    machine.wait_until_succeeds(
        "test -S /tmp/cua-runtime/cua-inject.sock "
        "&& find /tmp/cua-runtime -maxdepth 1 -type s -name 'wayland-*' | grep -q .",
        timeout=20,
    )
    wayland_display = machine.succeed(
        "basename $(find /tmp/cua-runtime -maxdepth 1 -type s -name 'wayland-*' -print -quit)"
    ).strip()

    machine.execute(
        "env XDG_RUNTIME_DIR=/tmp/cua-runtime WAYLAND_DISPLAY=" + wayland_display + " "
        "${recordGif} /tmp/transient-seat-frames /tmp/transient-seat-evidence/transient-seat.gif "
        "/tmp/stop-transient-seat-recorder /tmp/transient-seat-evidence/recorder.log "
        ">/dev/null 2>&1 & echo $! >/tmp/transient-seat-recorder.pid"
    )
    status, output = machine.execute(
        "env XDG_RUNTIME_DIR=/tmp/cua-runtime WAYLAND_DISPLAY=" + wayland_display + " "
        "DBUS_SESSION_BUS_ADDRESS=unix:path=/tmp/cua-session-bus XDG_SESSION_TYPE=wayland "
        "XDG_CURRENT_DESKTOP=sway XDG_SESSION_DESKTOP=sway CUA_DRIVER_RS_ENABLE_WAYLAND=1 "
        "CUA_INJECT_SOCKET=/tmp/cua-runtime/cua-inject.sock "
        "CUA_TEST_APPS_ROOT=${electronFixture} "
        "ELECTRON_OZONE_PLATFORM_HINT=wayland "
        "transient-seat-behavior --ignored --nocapture --test-threads=1 "
        ">/tmp/transient-seat-evidence/test.log 2>&1"
    )
    machine.execute("touch /tmp/stop-transient-seat-recorder")
    machine.execute(
        "timeout 30 sh -lc 'while kill -0 $(cat /tmp/transient-seat-recorder.pid) 2>/dev/null; do sleep 0.1; done'"
    )
    machine.execute("cp /tmp/transient-seat-compositor.log /tmp/transient-seat-evidence/compositor.log")
    machine.succeed("printf '%s\\n' " + str(status) + " >/tmp/transient-seat-evidence/test-status")

    # Copy before evaluating the verdict so GitHub Actions can upload evidence
    # for both passing and failing behavioral runs.
    machine.copy_from_machine("/tmp/transient-seat-evidence/recorder.log", "")
    machine.copy_from_machine("/tmp/transient-seat-evidence/compositor.log", "")
    machine.copy_from_machine("/tmp/transient-seat-evidence/test.log", "")
    machine.copy_from_machine("/tmp/transient-seat-evidence/test-status", "")
    if machine.execute("test -s /tmp/transient-seat-evidence/transient-seat.gif")[0] == 0:
        machine.copy_from_machine("/tmp/transient-seat-evidence/transient-seat.gif", "")
    else:
        machine.log("GIF capture failed; CI will retain logs and fail the artifact check")
  '';
}
