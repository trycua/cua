# CUA Driver native-Wayland integration test (per desktop) — TDD red.
#
# Brings up a native Wayland session, verifies the MCP handshake + doctor
# recognise the Wayland environment, then launches a native Wayland terminal
# (foot) and asserts cua-driver can ENUMERATE it via list_windows. The driver
# is X11-only today, so list_windows finds nothing and this fails — the spec for
# native Wayland window enumeration.
#
# To run: nix build .#checks.x86_64-linux.cua-driver-wayland-<desktop>-integration
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

  testScriptPy = pkgs.writeText "wayland-integration.py" ''
    import sys, time
    sys.path.insert(0, "/tmp")
    from driver_client import Driver

    d = Driver()
    try:
        d.initialize("nixos-wayland-integration")

        # Handshake sanity: required tools are advertised.
        d._send("tools/list", {}, req_id=999)
        resp = d._recv()
        names = [t["name"] for t in resp.get("result", {}).get("tools", [])]
        for t in ("list_windows", "get_window_state", "click", "type_text"):
            assert t in names, f"{t} not advertised: {names}"
        print(f"tools advertised: {len(names)}", flush=True)

        # The native-Wayland spec: list_windows must surface the foot terminal.
        pid, wid = d.find_window("cua-wayland-foot", timeout=30)
        print(f"RESOLVED native wayland window pid={pid} window_id={wid}", flush=True)
        print("integration test complete", flush=True)
    finally:
        d.close()
  '';
in

pkgs.testers.nixosTest {
  name = "cua-driver-wayland-${desktop}-integration-test";
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

    with subtest("Binary exists and lists tools"):
        machine.succeed("cua-driver --help")
        tools = machine.succeed("cua-driver list-tools")
        for t in ("list_windows", "get_window_state", "click", "type_text"):
            assert t in tools, f"{t} missing from list-tools"

    with subtest("Bring up ${session.label}"):
        machine.execute("${session.start} >/tmp/session.log 2>&1 &")
        try:
            machine.wait_for_file("/tmp/wl-ready", timeout=120)
        except Exception:
            machine.log(machine.execute("cat /tmp/session.log || true")[1])
            machine.log(machine.execute("cat /tmp/compositor.log || true")[1])
            raise
        wl = machine.succeed("cat /tmp/wl-display").strip()
        machine.log(f"WAYLAND_DISPLAY={wl}")

    with subtest("doctor recognises native Wayland (no DISPLAY)"):
        wl = machine.succeed("cat /tmp/wl-display").strip()
        result = machine.succeed(
            f"timeout 20 env -u DISPLAY WAYLAND_DISPLAY={wl} "
            "XDG_RUNTIME_DIR=/run/user/0 cua-driver doctor 2>&1 || true"
        )
        machine.log(result)
        assert "Wayland" in result, f"doctor did not report Wayland: {result}"

    with subtest("Launch native Wayland terminal (foot)"):
        wl = machine.succeed("cat /tmp/wl-display").strip()
        machine.execute(
            f"sh -lc 'env -u DISPLAY WAYLAND_DISPLAY={wl} XDG_RUNTIME_DIR=/run/user/0 "
            "foot --title=cua-wayland-foot >/tmp/foot.log 2>&1 & echo $! >/tmp/foot-pid.txt'"
        )
        # Give the client a moment to map its surface.
        machine.succeed("sleep 3")

    with subtest("cua-driver enumerates the native Wayland window (RED until Wayland support lands)"):
        wl = machine.succeed("cat /tmp/wl-display").strip()
        machine.copy_from_host("${driverClient}", "/tmp/driver_client.py")
        machine.copy_from_host("${testScriptPy}", "/tmp/wayland-integration.py")
        result = machine.succeed(
            f"timeout 90 env -u DISPLAY WAYLAND_DISPLAY={wl} "
            "XDG_RUNTIME_DIR=/run/user/0 python3 /tmp/wayland-integration.py 2>&1"
        )
        machine.log(result)
        assert "integration test complete" in result, result
  '';
}
