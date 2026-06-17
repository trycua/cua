# CUA Driver native-Wayland background-GUI skeleton (per desktop, per app) — TDD red.
#
# Native-Wayland analogue of the READ-ONLY skeleton class in
# ../../tests/linux-background-gui.nix. Launches a real toolkit app as a NATIVE
# Wayland client (no GDK_BACKEND/QT_QPA forcing), finds it via cua-driver
# list_windows, and screenshots it via get_window_state (vision). Records a GIF
# via grim. Fails today: the driver can neither enumerate nor capture native
# Wayland surfaces — the spec for native Wayland read support across toolkits.
#
# To run: nix build .#checks.x86_64-linux.cua-driver-wayland-<desktop>-background-gui-<app>
{
  pkgs,
  lib ? pkgs.lib,
  cuaDriverModule,
  desktop,
  app,
  ...
}:

let
  session = import ./session.nix { inherit pkgs desktop; };
  driverClient = import ./driver-client.nix { inherit pkgs; };
  recordGifScript = import ./record-wayland-gif.nix { inherit pkgs; };

  # Representative native-Wayland apps, one per major toolkit. `match` is the
  # identity substring find_window looks for (title/app_id/app/class).
  apps = {
    foot = {
      packages = [ pkgs.foot ];
      # foot is Wayland-only; no backend hint needed.
      env = "";
      launch = "foot --app-id=cua-wayland-foot-app --title=cua-wayland-foot-app";
      match = "cua-wayland-foot-app";
    };
    "gtk3-gedit" = {
      packages = [ pkgs.gedit ];
      # GTK auto-selects the Wayland backend from WAYLAND_DISPLAY.
      env = "GDK_BACKEND=wayland";
      launch = "gedit";
      match = "gedit";
    };
    "qt6-kcalc" = {
      packages = [ pkgs.kdePackages.kcalc ];
      # Qt defaults to xcb; force the NATIVE Wayland platform plugin (not xcb).
      env = "QT_QPA_PLATFORM=wayland";
      launch = "kcalc";
      match = "kcalc";
    };
  };

  cfg = apps.${app} or (throw "wayland/background-gui.nix: unknown app '${app}'");
  outputGif = "/tmp/cua-driver-wayland-${desktop}-background-gui-${app}.gif";
  outputPng = "/tmp/cua-driver-wayland-${desktop}-background-gui-${app}.png";

  # GIF recorder env: native desktops point grim at the host socket; nested
  # desktops leave WAYLAND_DISPLAY unset so the recorder self-resolves the
  # driver's published nested socket ($XDG_RUNTIME_DIR/.cua-nested-display).
  # No quotes around the command substitution: this string is interpolated into
  # a python double-quoted string, and the socket path has no spaces. The inner
  # `sh -c` re-parses and expands $(cat ...) itself.
  recorderWlEnv =
    if session.nested then
      "env XDG_RUNTIME_DIR=/run/user/0"
    else
      "env XDG_RUNTIME_DIR=/run/user/0 WAYLAND_DISPLAY=$(cat /tmp/wl-display)";

  testScriptPy = pkgs.writeText "wayland-bg-gui.py" ''
    import base64, sys
    sys.path.insert(0, "/tmp")
    from driver_client import Driver

    MATCH = "${cfg.match}"
    OUTPNG = "${outputPng}"
    LAUNCH = "${cfg.env} ${cfg.launch}"
    d = Driver()
    try:
        d.initialize("nixos-wayland-bg-gui")
        # Launch through cua-driver so the app lands in the session it owns
        # (host compositor on native desktops; nested labwc on kde/gnome).
        d.launch_app(LAUNCH)
        pid, wid = d.find_window(MATCH, timeout=50)
        print(f"app window pid={pid} window_id={wid}", flush=True)
        resp = d.call("get_window_state", {
            "pid": pid, "window_id": wid, "capture_mode": "vision"}, timeout=45)
        saved = False
        for item in resp.get("result", {}).get("content", []):
            if item.get("type") == "image" and item.get("data"):
                with open(OUTPNG, "wb") as f:
                    f.write(base64.b64decode(item["data"]))
                saved = True
                break
        assert saved, f"no capture for native Wayland window: {resp}"
        print("background gui read test complete", flush=True)
    finally:
        d.close()
  '';
in

pkgs.testers.nixosTest {
  name = "cua-driver-wayland-${desktop}-background-gui-${app}-test";
  meta.maintainers = [ ];

  nodes.machine =
    { pkgs, ... }:
    {
      imports = [ cuaDriverModule ];
      virtualisation = {
        cores = 2;
        memorySize = 4096;
      };
      services.cua-driver.enable = true;
      boot.kernelModules = [ "uinput" ];
      hardware.graphics.enable = true;
      services.udev.extraRules = ''
        KERNEL=="uinput", MODE="0660", GROUP="input", OPTIONS+="static_node=uinput"
      '';
      environment.systemPackages =
        session.packages ++ cfg.packages ++ (with pkgs; [ python3 jq at-spi2-core ]);
    };

  testScript = ''
    machine.start()
    machine.wait_for_unit("multi-user.target")

    with subtest("Bring up ${session.label}"):
        machine.execute("${session.start} >/tmp/session.log 2>&1 &")
        try:
            machine.wait_for_file("/tmp/wl-ready", timeout=120)
        except Exception:
            machine.log(machine.execute("cat /tmp/session.log || true")[1])
            machine.log(machine.execute("cat /tmp/compositor.log || true")[1])
            raise

    with subtest("Launch (via cua-driver) + record GIF + read inactive app window"):
        machine.copy_from_host("${driverClient}", "/tmp/driver_client.py")
        machine.copy_from_host("${testScriptPy}", "/tmp/wayland-bg-gui.py")
        # Start the recorder first; it grabs frames while the driver launches
        # and reads the app. (Native: host socket; nested: self-resolves.)
        machine.execute(
            "sh -lc '${recorderWlEnv} ${recordGifScript} /tmp/bg-gui-frames "
            "${outputGif} /tmp/stop-bg-gui-recorder /tmp/rec-bg-gui.log 12 0.2 "
            ">/dev/null 2>&1 & echo $! >/tmp/rec-bg-gui.pid'"
        )
        result = machine.execute(
            "timeout 150 env CUA_DRIVER_BIN=${session.driverWrapper} "
            "XDG_RUNTIME_DIR=/run/user/0 python3 /tmp/wayland-bg-gui.py 2>&1"
        )[1]
        machine.log(result)
        # Stop + copy the GIF BEFORE asserting so even failing jobs upload one.
        machine.execute("touch /tmp/stop-bg-gui-recorder")
        machine.execute("sh -lc 'for i in $(seq 1 60); do kill -0 $(cat /tmp/rec-bg-gui.pid) 2>/dev/null || break; sleep 1; done'")
        machine.execute("test -e ${outputGif} || : > ${outputGif}")
        machine.copy_from_machine("${outputGif}", "")

    with subtest("cua-driver read/captured the native Wayland app window"):
        assert "background gui read test complete" in result, result
  '';
}
