# CUA Driver native-Wayland integration test (per desktop).
#
# Brings up the desktop session, then launches a native Wayland terminal (foot)
# THROUGH cua-driver (launch_app) and asserts cua-driver can ENUMERATE it via
# list_windows. On native (wlroots) desktops the driver talks to the host
# compositor; on kde/gnome it drives its own nested labwc (CUA_WAYLAND_NEST=1,
# set by the session driver wrapper). Either way the app lands in the session
# the driver owns.
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
        for t in ("list_windows", "get_window_state", "click", "type_text", "launch_app"):
            assert t in names, f"{t} not advertised: {names}"
        print(f"tools advertised: {len(names)}", flush=True)

        # Launch foot through cua-driver so it lands in the session the driver
        # owns (host compositor on native desktops; nested labwc on kde/gnome).
        d.launch_app("foot --app-id=cua-wayland-foot --title=cua-wayland-foot")

        # list_windows must surface the foot terminal the driver just launched.
        pid, wid = d.find_window("cua-wayland-foot", timeout=40)
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
        machine.log("host WAYLAND_DISPLAY=" + machine.succeed("cat /tmp/wl-display").strip())

    with subtest("doctor reports a Wayland status line"):
        result = machine.succeed("timeout 30 ${session.driverWrapper} doctor 2>&1 || true")
        machine.log(result)
        assert "Wayland" in result, f"doctor did not report Wayland: {result}"

    with subtest("cua-driver launches + enumerates a native Wayland window"):
        machine.copy_from_host("${driverClient}", "/tmp/driver_client.py")
        machine.copy_from_host("${testScriptPy}", "/tmp/wayland-integration.py")
        result = machine.succeed(
            "timeout 120 env CUA_DRIVER_BIN=${session.driverWrapper} "
            "XDG_RUNTIME_DIR=/run/user/0 python3 /tmp/wayland-integration.py 2>&1"
        )
        machine.log(result)
        assert "integration test complete" in result, result
  '';
}
