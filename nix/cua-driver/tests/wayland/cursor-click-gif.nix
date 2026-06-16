# CUA Driver native-Wayland cursor-click GIF test (per desktop) — TDD red.
#
# Native-Wayland analogue of ../../tests/linux-cursor-click-gif.nix. Launches a
# native Wayland terminal (foot running a shell), finds it via cua-driver
# list_windows, clicks into it and types a command, then verifies the command
# RAN by checking a file it writes (display-server-agnostic proof). Records a
# GIF of the composited output via grim. Fails today: the driver can neither
# enumerate nor inject into native Wayland surfaces.
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
  session = import ./session.nix { inherit pkgs desktop; };
  driverClient = import ./driver-client.nix { inherit pkgs; };
  recordGifScript = import ./record-wayland-gif.nix { inherit pkgs; };

  testScriptPy = pkgs.writeText "wayland-cursor-click.py" ''
    import sys, time
    sys.path.insert(0, "/tmp")
    from driver_client import Driver

    d = Driver()
    try:
        d.initialize("nixos-wayland-click")
        pid, wid = d.find_window("cua-wayland-target", timeout=30)
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

    with subtest("Bring up ${session.label} + native Wayland target/control terminals"):
        machine.execute("${session.start} >/tmp/session.log 2>&1 &")
        try:
            machine.wait_for_file("/tmp/wl-ready", timeout=120)
        except Exception:
            machine.log(machine.execute("cat /tmp/session.log || true")[1])
            machine.log(machine.execute("cat /tmp/compositor.log || true")[1])
            raise
        wl = machine.succeed("cat /tmp/wl-display").strip()
        env = f"env -u DISPLAY WAYLAND_DISPLAY={wl} XDG_RUNTIME_DIR=/run/user/0"
        machine.execute(f"sh -lc '{env} foot --title=cua-wayland-control >/tmp/control.log 2>&1 &'")
        machine.execute(f"sh -lc '{env} foot --title=cua-wayland-target >/tmp/target.log 2>&1 &'")
        machine.succeed("sleep 3")

    with subtest("Record GIF and run driver click+type into native Wayland terminal"):
        wl = machine.succeed("cat /tmp/wl-display").strip()
        env = f"env -u DISPLAY WAYLAND_DISPLAY={wl} XDG_RUNTIME_DIR=/run/user/0"
        machine.copy_from_host("${driverClient}", "/tmp/driver_client.py")
        machine.copy_from_host("${testScriptPy}", "/tmp/wayland-cursor-click.py")
        machine.execute(
            f"sh -lc '{env} ${recordGifScript} /tmp/click-frames "
            "/tmp/cua-driver-wayland-${desktop}-cursor-click.gif /tmp/stop-click-recorder "
            "/tmp/rec-click.log 10 0.15 >/dev/null 2>&1 & echo $! >/tmp/rec-click.pid'"
        )
        # Non-raising so the grim session GIF is captured even on a red run.
        result = machine.execute(f"timeout 90 {env} python3 /tmp/wayland-cursor-click.py 2>&1")[1]
        machine.log(result)
        machine.execute("touch /tmp/stop-click-recorder")
        machine.execute("sh -lc 'for i in $(seq 1 60); do kill -0 $(cat /tmp/rec-click.pid) 2>/dev/null || break; sleep 1; done'")

    with subtest("Copy GIF out of the VM (best-effort)"):
        # Ensure the path exists so copy never errors before the real assertion;
        # it may be empty on compositors where grim could not capture.
        machine.execute("test -e /tmp/cua-driver-wayland-${desktop}-cursor-click.gif || : > /tmp/cua-driver-wayland-${desktop}-cursor-click.gif")
        machine.copy_from_machine("/tmp/cua-driver-wayland-${desktop}-cursor-click.gif", "")

    with subtest("Driver completed click+type AND the command ran (RED until Wayland input lands)"):
        assert "click GIF test complete" in result, result
        machine.wait_until_succeeds("test -f /tmp/click-focus.txt", timeout=20)
  '';
}
