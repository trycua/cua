# CUA Driver native-Wayland background-terminal GIF test (per desktop).
#
# The hard one: type into an UNFOCUSED window. Stock Wayland delivers keyboard
# only to the focused surface, so cua-driver nests its own cua-compositor (EIS
# mode) which injects wl_keyboard straight into a target surface by app_id,
# bypassing focus. A control terminal is launched AFTER the target so the target
# is NOT focused; cua-driver types a command into the inactive target and we
# verify it ran via a file side-effect. Records a GIF via grim.
#
# To run: nix build .#checks.x86_64-linux.cua-driver-wayland-<desktop>-background-terminal-gif
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

  testScriptPy = pkgs.writeText "wayland-background-terminal.py" ''
    import sys, time
    sys.path.insert(0, "/tmp")
    from driver_client import Driver

    d = Driver()
    try:
        d.initialize("nixos-wayland-bg-terminal")
        # Launch the TARGET first, then the CONTROL — so control holds focus and
        # the target is the inactive window we must type into focus-free.
        d.launch_app("foot --app-id=cua-wayland-target --title=cua-wayland-target")
        time.sleep(1.2)
        d.launch_app("foot --app-id=cua-wayland-control --title=cua-wayland-control")
        tpid, twid = d.find_window("cua-wayland-target", timeout=40)
        cpid, cwid = d.find_window("cua-wayland-control", timeout=40)
        print(f"target pid={tpid} window_id={twid} control pid={cpid} window_id={cwid}", flush=True)
        d.call("set_agent_cursor_enabled", {"enabled": True})
        # Focus-free type into the INACTIVE target while control stays focused.
        d.call("type_text", {"pid": tpid, "window_id": twid,
            "text": "echo hello | tee /tmp/background-hello.txt"})
        d.call("press_key", {"pid": tpid, "window_id": twid, "key": "enter"})
        time.sleep(1.8)
        print("background terminal GIF test complete", flush=True)
    finally:
        d.close()
  '';
in

pkgs.testers.nixosTest {
  name = "cua-driver-wayland-${desktop}-background-terminal-gif-test";
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

    with subtest("Bring up ${session.label}"):
        machine.execute("${session.start} >/tmp/session.log 2>&1 &")
        try:
            machine.wait_for_file("/tmp/wl-ready", timeout=120)
        except Exception:
            machine.log(machine.execute("cat /tmp/session.log || true")[1])
            machine.log(machine.execute("cat /tmp/compositor.log || true")[1])
            raise

    with subtest("Record GIF + driver types focus-free into the INACTIVE Wayland terminal"):
        machine.copy_from_host("${driverClient}", "/tmp/driver_client.py")
        machine.copy_from_host("${testScriptPy}", "/tmp/wayland-background-terminal.py")
        # Recorder self-resolves the driver's nested cua-compositor socket.
        machine.execute(
            "sh -lc 'env XDG_RUNTIME_DIR=/run/user/0 ${recordGifScript} /tmp/background-frames "
            "/tmp/cua-driver-wayland-${desktop}-background-terminal.gif /tmp/stop-background-recorder "
            "/tmp/rec-background.log 10 0.15 >/dev/null 2>&1 & echo $! >/tmp/rec-background.pid'"
        )
        result = machine.execute(
            "timeout 120 env CUA_DRIVER_BIN=${session.driverWrapper} "
            "XDG_RUNTIME_DIR=/run/user/0 python3 /tmp/wayland-background-terminal.py 2>&1"
        )[1]
        machine.log(result)
        machine.execute("touch /tmp/stop-background-recorder")
        machine.execute("sh -lc 'for i in $(seq 1 60); do kill -0 $(cat /tmp/rec-background.pid) 2>/dev/null || break; sleep 1; done'")

    with subtest("Copy GIF out of the VM (best-effort)"):
        machine.execute("test -e /tmp/cua-driver-wayland-${desktop}-background-terminal.gif || : > /tmp/cua-driver-wayland-${desktop}-background-terminal.gif")
        machine.copy_from_machine("/tmp/cua-driver-wayland-${desktop}-background-terminal.gif", "")

    with subtest("Driver typed into inactive terminal AND the command ran"):
        assert "background terminal GIF test complete" in result, result
        machine.wait_until_succeeds("grep -Fx 'hello' /tmp/background-hello.txt", timeout=20)
  '';
}
