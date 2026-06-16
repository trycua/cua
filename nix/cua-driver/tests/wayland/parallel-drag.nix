# CUA Driver native-Wayland parallel multi-cursor drag test (per desktop) — TDD red.
#
# Native-Wayland analogue of ../../tests/linux-parallel-drag-xserver.nix, and
# the hardest of the suite. parallel_mouse_drag relies on X11 XInput2 MPX master
# pointers and kernel uinput slaves; Wayland has no MPX and the headless
# compositor has no libinput seat. There is no native-Wayland multi-pointer
# protocol in wide use, so this is expected to fail hard. It is kept as a check
# to record exactly how the driver reports the limitation per compositor and to
# anchor any future multi-seat / virtual-pointer Wayland design.
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
  session = import ./session.nix { inherit pkgs desktop; };
  driverClient = import ./driver-client.nix { inherit pkgs; };
  recordGifScript = import ./record-wayland-gif.nix { inherit pkgs; };

  testScriptPy = pkgs.writeText "wayland-parallel-drag.py" ''
    import sys, time
    sys.path.insert(0, "/tmp")
    from driver_client import Driver

    d = Driver()
    try:
        d.initialize("nixos-wayland-parallel-drag")
        pid, wid = d.find_window("cua-wayland-paint", timeout=30)
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
    machine.succeed("modprobe uinput && test -e /dev/uinput")

    with subtest("Bring up ${session.label} + native Wayland target terminal"):
        machine.execute("${session.start} >/tmp/session.log 2>&1 &")
        try:
            machine.wait_for_file("/tmp/wl-ready", timeout=120)
        except Exception:
            machine.log(machine.execute("cat /tmp/session.log || true")[1])
            machine.log(machine.execute("cat /tmp/compositor.log || true")[1])
            raise
        wl = machine.succeed("cat /tmp/wl-display").strip()
        env = f"env -u DISPLAY WAYLAND_DISPLAY={wl} XDG_RUNTIME_DIR=/run/user/0"
        machine.execute(f"sh -lc '{env} foot --app-id=cua-wayland-paint --title=cua-wayland-paint >/tmp/paint.log 2>&1 &'")
        machine.succeed("sleep 3")

    with subtest("Record GIF and pilot cua-driver through parallel_mouse_drag (expected RED)"):
        wl = machine.succeed("cat /tmp/wl-display").strip()
        env = f"env -u DISPLAY WAYLAND_DISPLAY={wl} XDG_RUNTIME_DIR=/run/user/0"
        machine.copy_from_host("${driverClient}", "/tmp/driver_client.py")
        machine.copy_from_host("${testScriptPy}", "/tmp/wayland-parallel-drag.py")
        machine.execute(
            f"sh -lc '{env} ${recordGifScript} /tmp/drag-frames "
            "/tmp/cua-driver-wayland-${desktop}-parallel-drag.gif /tmp/stop-drag-recorder "
            "/tmp/rec-drag.log 8 0.12 >/dev/null 2>&1 & echo $! >/tmp/rec-drag.pid'"
        )
        # Non-raising so the grim session GIF is captured even on a red run.
        result = machine.execute(f"timeout 90 {env} python3 /tmp/wayland-parallel-drag.py 2>&1")[1]
        machine.log(result)
        machine.execute("touch /tmp/stop-drag-recorder")
        machine.execute("sh -lc 'for i in $(seq 1 60); do kill -0 $(cat /tmp/rec-drag.pid) 2>/dev/null || break; sleep 1; done'")

    with subtest("Copy GIF out of the VM (best-effort)"):
        machine.execute("test -e /tmp/cua-driver-wayland-${desktop}-parallel-drag.gif || : > /tmp/cua-driver-wayland-${desktop}-parallel-drag.gif")
        machine.copy_from_machine("/tmp/cua-driver-wayland-${desktop}-parallel-drag.gif", "")

    with subtest("parallel_mouse_drag succeeded on native Wayland (expected RED)"):
        assert "parallel drag test complete" in result, result
  '';
}
