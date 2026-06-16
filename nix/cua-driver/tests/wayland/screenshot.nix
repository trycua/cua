# CUA Driver native-Wayland screenshot test (per desktop) — TDD red.
#
# Launches a native Wayland terminal (foot), finds it via cua-driver
# list_windows, and asserts get_window_state (capture_mode=vision) returns a
# real PNG of that native Wayland surface. The driver captures via X11 today, so
# both discovery and capture fail on native Wayland — the spec for native
# Wayland window capture.
#
# To run: nix build .#checks.x86_64-linux.cua-driver-wayland-<desktop>-screenshot
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

  testScriptPy = pkgs.writeText "wayland-screenshot.py" ''
    import base64, sys
    sys.path.insert(0, "/tmp")
    from driver_client import Driver

    d = Driver()
    try:
        d.initialize("nixos-wayland-screenshot")
        pid, wid = d.find_window("cua-wayland-foot", timeout=30)
        print(f"window pid={pid} window_id={wid}", flush=True)
        resp = d.call("get_window_state", {
            "pid": pid, "window_id": wid, "capture_mode": "vision"}, timeout=30)
        saved = False
        for item in resp.get("result", {}).get("content", []):
            if item.get("type") == "image" and item.get("data"):
                data = base64.b64decode(item["data"])
                assert len(data) > 0, "empty image payload"
                with open("/tmp/cua-driver-wayland-${desktop}-screenshot.png", "wb") as f:
                    f.write(data)
                saved = True
                break
        assert saved, f"no image returned for native Wayland window: {resp}"
        print("screenshot test complete", flush=True)
    finally:
        d.close()
  '';
in

pkgs.testers.nixosTest {
  name = "cua-driver-wayland-${desktop}-screenshot-test";
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

    with subtest("Bring up ${session.label} + native Wayland foot terminal"):
        machine.execute("${session.start} >/tmp/session.log 2>&1 &")
        try:
            machine.wait_for_file("/tmp/wl-ready", timeout=120)
        except Exception:
            machine.log(machine.execute("cat /tmp/session.log || true")[1])
            machine.log(machine.execute("cat /tmp/compositor.log || true")[1])
            raise
        wl = machine.succeed("cat /tmp/wl-display").strip()
        machine.execute(
            f"sh -lc 'env -u DISPLAY WAYLAND_DISPLAY={wl} XDG_RUNTIME_DIR=/run/user/0 "
            "foot --app-id=cua-wayland-foot --title=cua-wayland-foot >/tmp/foot.log 2>&1 & echo $! >/tmp/foot-pid.txt'"
        )
        machine.succeed("sleep 3")

    with subtest("Screenshot the native Wayland window via cua-driver (RED until Wayland capture lands)"):
        wl = machine.succeed("cat /tmp/wl-display").strip()
        machine.copy_from_host("${driverClient}", "/tmp/driver_client.py")
        machine.copy_from_host("${testScriptPy}", "/tmp/wayland-screenshot.py")
        result = machine.succeed(
            f"timeout 90 env -u DISPLAY WAYLAND_DISPLAY={wl} "
            "XDG_RUNTIME_DIR=/run/user/0 python3 /tmp/wayland-screenshot.py 2>&1"
        )
        machine.log(result)
        assert "screenshot test complete" in result, result

    with subtest("Extract screenshot"):
        machine.copy_from_machine("/tmp/cua-driver-wayland-${desktop}-screenshot.png", "")
  '';
}
