# CUA Driver native-Wayland background-terminal GIF test (per desktop) — TDD red.
#
# Native-Wayland analogue of ../../tests/linux-background-terminal-gif.nix. A
# control terminal is launched after the target so the target is NOT focused;
# cua-driver then types a command into the inactive native Wayland target and we
# verify it ran via a file side-effect. Records a GIF via grim. Fails today: the
# driver's focus-free injection (X11 XSendEvent) does not reach native Wayland
# surfaces — the spec for focus-free Wayland input.
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
  session = import ./session.nix { inherit pkgs desktop; };
  driverClient = import ./driver-client.nix { inherit pkgs; };
  recordGifScript = import ./record-wayland-gif.nix { inherit pkgs; };

  testScriptPy = pkgs.writeText "wayland-background-terminal.py" ''
    import sys, time
    sys.path.insert(0, "/tmp")
    from driver_client import Driver

    d = Driver()
    try:
        d.initialize("nixos-wayland-bg-terminal")
        tpid, twid = d.find_window("cua-wayland-target", timeout=30)
        cpid, cwid = d.find_window("cua-wayland-control", timeout=30)
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
        # Target first, control second so the target is left UNFOCUSED.
        machine.execute(f"sh -lc '{env} foot --app-id=cua-wayland-target --title=cua-wayland-target >/tmp/target.log 2>&1 &'")
        machine.succeed("sleep 1")
        machine.execute(f"sh -lc '{env} foot --app-id=cua-wayland-control --title=cua-wayland-control >/tmp/control.log 2>&1 &'")
        machine.succeed("sleep 3")

    with subtest("Record GIF and type into the inactive native Wayland terminal"):
        wl = machine.succeed("cat /tmp/wl-display").strip()
        env = f"env -u DISPLAY WAYLAND_DISPLAY={wl} XDG_RUNTIME_DIR=/run/user/0"
        machine.copy_from_host("${driverClient}", "/tmp/driver_client.py")
        machine.copy_from_host("${testScriptPy}", "/tmp/wayland-background-terminal.py")
        machine.execute(
            f"sh -lc '{env} ${recordGifScript} /tmp/background-frames "
            "/tmp/cua-driver-wayland-${desktop}-background-terminal.gif /tmp/stop-background-recorder "
            "/tmp/rec-background.log 10 0.15 >/dev/null 2>&1 & echo $! >/tmp/rec-background.pid'"
        )
        # Non-raising so the grim session GIF is captured even on a red run.
        result = machine.execute(f"timeout 90 {env} python3 /tmp/wayland-background-terminal.py 2>&1")[1]
        machine.log(result)
        machine.execute("touch /tmp/stop-background-recorder")
        machine.execute("sh -lc 'for i in $(seq 1 60); do kill -0 $(cat /tmp/rec-background.pid) 2>/dev/null || break; sleep 1; done'")

    with subtest("Copy GIF out of the VM (best-effort)"):
        machine.execute("test -e /tmp/cua-driver-wayland-${desktop}-background-terminal.gif || : > /tmp/cua-driver-wayland-${desktop}-background-terminal.gif")
        machine.copy_from_machine("/tmp/cua-driver-wayland-${desktop}-background-terminal.gif", "")

    with subtest("Driver typed into inactive terminal AND the command ran (RED until Wayland input lands)"):
        assert "background terminal GIF test complete" in result, result
        machine.wait_until_succeeds("grep -Fx 'hello' /tmp/background-hello.txt", timeout=20)
  '';
}
