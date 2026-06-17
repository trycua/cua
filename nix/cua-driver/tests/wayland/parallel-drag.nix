# CUA Driver native-Wayland parallel multi-cursor drag test (per desktop).
#
# Two simultaneous drag gestures on one window. Stock Wayland has a single seat
# cursor and no multi-pointer protocol, so cua-driver nests its own
# cua-compositor (EIS mode), which drives N independent logical cursors and
# injects wl_pointer per cursor by app_id. parallel_mouse_drag routes through
# the compositor's control socket (window-local coords, no X11 MPX). Records a
# GIF via grim.
#
# To run: nix build .#checks.x86_64-linux.cua-driver-wayland-<desktop>-parallel-drag
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

  testScriptPy = pkgs.writeText "wayland-parallel-drag.py" ''
    import sys, time
    sys.path.insert(0, "/tmp")
    from driver_client import Driver

    d = Driver()
    try:
        d.initialize("nixos-wayland-parallel-drag")
        d.launch_app("foot --app-id=cua-wayland-paint --title=cua-wayland-paint")
        pid, wid = d.find_window("cua-wayland-paint", timeout=40)
        print(f"target pid={pid} window_id={wid}", flush=True)
        d.call("parallel_mouse_drag", {"drags": [
            {"session": "agent-1", "window_id": wid, "from_x": 100.0, "from_y": 100.0, "to_x": 380.0, "to_y": 420.0, "duration_ms": 2000, "steps": 60},
            {"session": "agent-2", "window_id": wid, "from_x": 700.0, "from_y": 100.0, "to_x": 420.0, "to_y": 420.0, "duration_ms": 2000, "steps": 60},
        ]})
        time.sleep(0.6)
        print("parallel drag test complete", flush=True)
    finally:
        d.close()
  '';
in

pkgs.testers.nixosTest {
  name = "cua-driver-wayland-${desktop}-parallel-drag-test";
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
      services.libinput.enable = true;
      environment.systemPackages = session.packages ++ (with pkgs; [ python3 jq ]);
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

    with subtest("Record GIF + pilot cua-driver through parallel_mouse_drag (multi-cursor)"):
        machine.copy_from_host("${driverClient}", "/tmp/driver_client.py")
        machine.copy_from_host("${testScriptPy}", "/tmp/wayland-parallel-drag.py")
        machine.execute(
            "sh -lc 'env XDG_RUNTIME_DIR=/run/user/0 ${recordGifScript} /tmp/drag-frames "
            "/tmp/cua-driver-wayland-${desktop}-parallel-drag.gif /tmp/stop-drag-recorder "
            "/tmp/rec-drag.log 8 0.12 >/dev/null 2>&1 & echo $! >/tmp/rec-drag.pid'"
        )
        result = machine.execute(
            "timeout 120 env CUA_DRIVER_BIN=${session.driverWrapper} "
            "XDG_RUNTIME_DIR=/run/user/0 python3 /tmp/wayland-parallel-drag.py 2>&1"
        )[1]
        machine.log(result)
        machine.execute("touch /tmp/stop-drag-recorder")
        machine.execute("sh -lc 'for i in $(seq 1 60); do kill -0 $(cat /tmp/rec-drag.pid) 2>/dev/null || break; sleep 1; done'")

    with subtest("Copy GIF out of the VM (best-effort)"):
        machine.execute("test -e /tmp/cua-driver-wayland-${desktop}-parallel-drag.gif || : > /tmp/cua-driver-wayland-${desktop}-parallel-drag.gif")
        machine.copy_from_machine("/tmp/cua-driver-wayland-${desktop}-parallel-drag.gif", "")

    with subtest("parallel_mouse_drag completed on native Wayland (multi-cursor via EIS compositor)"):
        assert "parallel drag test complete" in result, result
  '';
}
