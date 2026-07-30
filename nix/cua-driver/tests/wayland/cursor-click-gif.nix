# CUA Driver native-Wayland cursor-click GIF test (per desktop).
#
# Launches two native Wayland terminals (foot) THROUGH cua-driver (launch_app),
# clicks the target, types a command through cua-driver's Wayland injection path,
# and verifies the command RAN by checking a file it writes
# (display-server-agnostic proof).
# Records a GIF of the composited output via grim. On wlroots desktops the driver
# can drive the host compositor, but compositor-specific focus heuristics make
# virtual-keyboard delivery after a click unreliable in headless CI. This visual
# test therefore uses the same EIS-backed nested compositor as the background
# terminal test so it remains a deterministic click+type regression guard across
# labwc, sway, KDE, and GNOME hosts.
#
# To run: nix build .#checks.x86_64-linux.cua-driver-wayland-<desktop>-cursor-click-gif
{
  pkgs,
  lib ? pkgs.lib,
  cuaDriverModule,
  desktop,
  ...
}:

let
  session = import ./session.nix { inherit pkgs desktop; eis = true; };
  driverClient = import ./driver-client.nix { inherit pkgs; };
  recordGifScript = import ./record-wayland-gif.nix { inherit pkgs; };

  # GIF recorder env: native points grim at the host socket; nested self-resolves
  # the driver's published nested socket ($XDG_RUNTIME_DIR/.cua-nested-display).
  recorderWlEnv =
    if session.nested then
      "env XDG_RUNTIME_DIR=/run/user/0"
    else
      "env XDG_RUNTIME_DIR=/run/user/0 WAYLAND_DISPLAY=$(cat /tmp/wl-display)";

  testScriptPy = pkgs.writeText "wayland-cursor-click.py" ''
    import sys, time
    sys.path.insert(0, "/tmp")
    from driver_client import Driver

    d = Driver()
    try:
        d.initialize("nixos-wayland-click")
        # Launch a control terminal then the target. The click still exercises
        # the target pointer path, while type_text/press_key use the EIS-backed
        # injection path so headless compositor focus policy cannot make the
        # test flaky.
        d.launch_app("foot --app-id=cua-wayland-control --title=cua-wayland-control")
        time.sleep(1)
        d.launch_app("foot --app-id=cua-wayland-target --title=cua-wayland-target")
        pid, wid = d.find_window("cua-wayland-target", timeout=40)
        print(f"target pid={pid} window_id={wid}", flush=True)
        d.call("set_agent_cursor_enabled", {"enabled": True})
        d.call("move_cursor", {"x": 1100.0, "y": 900.0})
        time.sleep(0.6)
        d.call("click", {"pid": pid, "window_id": wid, "x": 120.0, "y": 120.0})
        time.sleep(0.4)
        d.call("type_text", {"pid": pid, "window_id": wid,
            "text": "echo click-focus > /tmp/click-focus.txt"})
        d.call("press_key", {"pid": pid, "window_id": wid, "key": "enter"})
        time.sleep(1.5)
        print("click GIF test complete", flush=True)
    finally:
        d.close()
  '';
in

pkgs.testers.nixosTest {
  name = "cua-driver-wayland-${desktop}-cursor-click-gif-test";
  meta.maintainers = [ ];

  nodes.machine =
    { pkgs, ... }:
    {
      imports = [ cuaDriverModule ];
      virtualisation = {
        cores = 2;
        memorySize = 3072;
      };
      services.cua-driver.enable = true;
      boot.kernelModules = [ "uinput" ];
      hardware.graphics.enable = true;
      services.udev.extraRules = ''
        KERNEL=="uinput", MODE="0660", GROUP="input", OPTIONS+="static_node=uinput"
      '';
      environment.systemPackages = session.packages ++ (with pkgs; [ python3 jq ]);
    };

  testScript = ''
    machine.start()
    machine.wait_for_unit("multi-user.target")
    machine.succeed("modprobe uinput && test -e /dev/uinput")

    with subtest("Bring up ${session.label}"):
        machine.execute("${session.start} >/tmp/session.log 2>&1 &")
        try:
            machine.wait_for_file("/tmp/wl-ready", timeout=120)
        except Exception:
            machine.log(machine.execute("cat /tmp/session.log || true")[1])
            machine.log(machine.execute("cat /tmp/compositor.log || true")[1])
            raise

    with subtest("Record GIF + driver launches, clicks, and types into a native Wayland terminal"):
        machine.copy_from_host("${driverClient}", "/tmp/driver_client.py")
        machine.copy_from_host("${testScriptPy}", "/tmp/wayland-cursor-click.py")
        machine.execute(
            "sh -lc '${recorderWlEnv} ${recordGifScript} /tmp/click-frames "
            "/tmp/cua-driver-wayland-${desktop}-cursor-click.gif /tmp/stop-click-recorder "
            "/tmp/rec-click.log 10 0.15 >/dev/null 2>&1 & echo $! >/tmp/rec-click.pid'"
        )
        result = machine.execute(
            "timeout 120 env CUA_DRIVER_BIN=${session.driverWrapper} "
            "XDG_RUNTIME_DIR=/run/user/0 python3 /tmp/wayland-cursor-click.py 2>&1"
        )[1]
        machine.log(result)
        machine.execute("touch /tmp/stop-click-recorder")
        machine.execute("sh -lc 'for i in $(seq 1 60); do kill -0 $(cat /tmp/rec-click.pid) 2>/dev/null || break; sleep 1; done'")

    with subtest("Copy GIF out of the VM (best-effort)"):
        # Ensure the path exists so copy never errors before the real assertion;
        # it may be empty on compositors where grim could not capture.
        machine.execute("test -e /tmp/cua-driver-wayland-${desktop}-cursor-click.gif || : > /tmp/cua-driver-wayland-${desktop}-cursor-click.gif")
        machine.copy_from_machine("/tmp/cua-driver-wayland-${desktop}-cursor-click.gif", "")

    with subtest("Driver completed click+type AND the command ran in the Wayland terminal"):
        assert "click GIF test complete" in result, result
        machine.wait_until_succeeds("test -f /tmp/click-focus.txt", timeout=20)
  '';
}
