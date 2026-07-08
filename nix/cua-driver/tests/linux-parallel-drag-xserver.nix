# Linux parallel multi-cursor drag test — real Xorg via services.xserver
#
# The MPX path needs a REAL Xorg with the libinput input backend so cua-driver's
# uinput slaves enumerate as X input devices. Here NixOS's services.xserver
# brings up Xorg properly on a seat via a display manager, with the `dummy`
# video driver (software framebuffer, headless) and libinput.
# A normal user is auto-logged-in to an icewm session; the session runs
# `xhost +local:` so the root-run test driver / cua-driver can connect to :0.
#
# Proves: two per-session master pointers draw concurrent cooked, window-
# targeted XI2 events into one window (assert 2 distinct devices), the shield
# grab keeps focus on a separate control window, and a GIF is produced.
#
# To run: nix build .#checks.x86_64-linux.cua-driver-linux-parallel-drag-xserver
{
  pkgs,
  lib ? pkgs.lib,
  cuaDriverModule,
  ...
}:

let
  # XI2 paint + event logger (selects XI2 for all master devices; logs each
  # press/motion/release with its device id and paints per-device squares).
  xi2paintSrc = pkgs.writeText "xi2paint.c" ''
    #include <X11/Xlib.h>
    #include <X11/extensions/XInput2.h>
    #include <stdio.h>
    #include <string.h>

    int main(int argc, char **argv) {
        const char *logpath = argc > 1 ? argv[1] : "/tmp/xi2paint-events.log";
        Display *dpy = XOpenDisplay(NULL);
        if (!dpy) { fprintf(stderr, "no display\n"); return 1; }
        int xi_opcode, ev, err;
        if (!XQueryExtension(dpy, "XInputExtension", &xi_opcode, &ev, &err)) {
            fprintf(stderr, "no XInputExtension\n"); return 1;
        }
        int major = 2, minor = 3;
        XIQueryVersion(dpy, &major, &minor);
        int scr = DefaultScreen(dpy);
        Window win = XCreateSimpleWindow(dpy, RootWindow(dpy, scr), 40, 40, 800, 600,
                                         1, BlackPixel(dpy, scr), WhitePixel(dpy, scr));
        XStoreName(dpy, win, "XI2 MPX Paint");
        XSelectInput(dpy, win, ExposureMask);
        unsigned char mask[XIMaskLen(XI_LASTEVENT)];
        memset(mask, 0, sizeof mask);
        XISetMask(mask, XI_ButtonPress);
        XISetMask(mask, XI_Motion);
        XISetMask(mask, XI_ButtonRelease);
        XIEventMask em;
        em.deviceid = XIAllMasterDevices;
        em.mask_len = sizeof mask;
        em.mask = mask;
        XISelectEvents(dpy, win, &em, 1);
        XMapWindow(dpy, win);
        XFlush(dpy);
        GC gc = XCreateGC(dpy, win, 0, NULL);
        FILE *logf = fopen(logpath, "w");
        if (!logf) { fprintf(stderr, "cannot open log\n"); return 1; }
        setvbuf(logf, NULL, _IOLBF, 0);
        printf("READY 0x%lx\n", win); fflush(stdout);
        int down[256]; memset(down, 0, sizeof down);
        unsigned long colors[] = { 0xd00000, 0x0040d0, 0x00a000, 0xc08000 };
        for (;;) {
            XEvent e;
            XNextEvent(dpy, &e);
            if (e.type == GenericEvent && e.xcookie.extension == xi_opcode &&
                XGetEventData(dpy, &e.xcookie)) {
                int t = e.xcookie.evtype;
                if (t == XI_ButtonPress || t == XI_Motion || t == XI_ButtonRelease) {
                    XIDeviceEvent *de = e.xcookie.data;
                    const char *n = t == XI_ButtonPress ? "PRESS"
                                  : t == XI_Motion ? "MOTION" : "RELEASE";
                    fprintf(logf, "%s dev=%d x=%.0f y=%.0f\n",
                            n, de->deviceid, de->event_x, de->event_y);
                    int d = de->deviceid & 255;
                    if (t == XI_ButtonPress) down[d] = 1;
                    if (t == XI_ButtonRelease) down[d] = 0;
                    if ((t == XI_Motion && down[d]) || t == XI_ButtonPress) {
                        XSetForeground(dpy, gc, colors[de->deviceid % 4]);
                        XFillRectangle(dpy, win, gc, (int)de->event_x - 3,
                                       (int)de->event_y - 3, 6, 6);
                        XFlush(dpy);
                    }
                }
                XFreeEventData(dpy, &e.xcookie);
            }
        }
        return 0;
    }
  '';

  xi2paint = pkgs.runCommandCC "xi2paint" {
    buildInputs = [ pkgs.xorg.libX11 pkgs.xorg.libXi ];
  } ''
    mkdir -p $out/bin
    cc -O2 -o $out/bin/xi2paint ${xi2paintSrc} -lX11 -lXi
  '';

  mcpDragTest = pkgs.writeText "mcp-parallel-drag-test.py" ''
    import json, os, subprocess, sys, threading, time
    DRIVER_BIN = os.environ.get("CUA_DRIVER_BIN", "cua-driver")

    def start_driver():
        proc = subprocess.Popen([DRIVER_BIN, "mcp", "--no-daemon-relaunch"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env={**os.environ})
        def drain():
            for line in proc.stderr:
                sys.stderr.buffer.write(line); sys.stderr.buffer.flush()
        threading.Thread(target=drain, daemon=True).start()
        return proc

    def send(proc, method, params=None, req_id=None):
        msg = {"jsonrpc": "2.0", "method": method}
        if params is not None: msg["params"] = params
        if req_id is not None: msg["id"] = req_id
        proc.stdin.write((json.dumps(msg) + "\n").encode()); proc.stdin.flush()

    def recv(proc, timeout=30):
        result = [None]
        def reader(): result[0] = proc.stdout.readline()
        th = threading.Thread(target=reader); th.start(); th.join(timeout)
        if th.is_alive(): raise TimeoutError("no response")
        line = result[0].decode().strip()
        if not line: raise RuntimeError("empty response")
        return json.loads(line)

    def call_tool(proc, rid, name, args, timeout=60):
        send(proc, "tools/call", {"name": name, "arguments": args}, req_id=rid)
        resp = recv(proc, timeout=timeout)
        if resp.get("error"): raise RuntimeError(f"{name} failed: {resp}")
        if resp.get("result", {}).get("isError"): raise RuntimeError(f"{name} isError: {resp}")
        return resp

    def main():
        with open("/tmp/paint-xid.txt") as f:
            wid = int(f.readline().strip())
        proc = start_driver()
        try:
            send(proc, "initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                 "clientInfo": {"name": "nixos-parallel-drag-xserver", "version": "1.0.0"}}, req_id=1)
            recv(proc); send(proc, "notifications/initialized", {}); time.sleep(0.3)
            # Two concurrent strokes (one per cursor) into an UNFOCUSED window;
            # each is a single held-path drag (press once, glide, release once).
            call_tool(proc, 2, "parallel_mouse_drag", {"drags": [
                {"session": "agent-1", "window_id": wid, "from_x": 100.0, "from_y": 100.0, "to_x": 380.0, "to_y": 420.0, "duration_ms": 2200, "steps": 80},
                {"session": "agent-2", "window_id": wid, "from_x": 700.0, "from_y": 100.0, "to_x": 420.0, "to_y": 420.0, "duration_ms": 2200, "steps": 80},
            ]})
            time.sleep(0.6)
            call_tool(proc, 3, "parallel_mouse_drag", {"drags": [
                {"session": "agent-1", "window_id": wid, "from_x": 100.0, "from_y": 420.0, "to_x": 380.0, "to_y": 120.0, "duration_ms": 2200, "steps": 80},
                {"session": "agent-2", "window_id": wid, "from_x": 700.0, "from_y": 420.0, "to_x": 420.0, "to_y": 120.0, "duration_ms": 2200, "steps": 80},
            ]})
            time.sleep(0.6)
            print("parallel drag test complete", flush=True)
        finally:
            proc.stdin.close(); proc.terminate(); proc.wait(timeout=5)

    if __name__ == "__main__":
        main()
  '';

  recordGifScript = import ./record-x11-gif.nix { inherit pkgs; };
in

pkgs.testers.nixosTest {
  name = "cua-driver-linux-parallel-drag-xserver-test";
  meta.maintainers = [ ];

  nodes.machine =
    { pkgs, ... }:
    {
      imports = [ cuaDriverModule ];
      virtualisation = {
        cores = 2;
        memorySize = 2048;
      };
      services.cua-driver.enable = true;
      boot.kernelModules = [ "uinput" ];
      # Root opens /dev/uinput directly; the group rule is belt-and-suspenders.
      services.udev.extraRules = ''
        KERNEL=="uinput", MODE="0660", GROUP="input", OPTIONS+="static_node=uinput"
      '';

      # Real Xorg on a seat (DM handles the VT), dummy video, libinput input.
      services.xserver = {
        enable = true;
        videoDrivers = [ "dummy" ];
        deviceSection = ''VideoRam 256000'';
        monitorSection = ''
          HorizSync 30.0 - 1000.0
          VertRefresh 30.0 - 200.0
          Modeline "1280x1024" 109.00 1280 1368 1496 1712 1024 1027 1034 1063 -hsync +vsync
        '';
        screenSection = ''
          DefaultDepth 24
          SubSection "Display"
            Depth 24
            Modes "1280x1024"
            Virtual 1280 1024
          EndSubSection
        '';
        windowManager.icewm.enable = true;
        # A display manager is what actually starts Xorg on a seat/VT — the bit
        # the hand-launched Xorg couldn't arrange in the emulated VM. lightdm is
        # the lightest. (lightdm.enable still lives under xserver.displayManager;
        # autoLogin/defaultSession moved to the top-level services.displayManager.)
        displayManager.lightdm.enable = true;
        # Disable X access control outright: this is a throwaway single-user
        # test VM, and the root-run cua-driver / clients must reach :0. Relying
        # on the session's `xhost +local:` proved unreliable across uids (the
        # server's auth ACL only lists the autologin user), so `-ac` is the
        # bulletproof grant. `xhost +local:` is kept as belt-and-suspenders.
        displayManager.xserverArgs = [ "-ac" ];
        displayManager.sessionCommands = ''
          ${pkgs.xorg.xhost}/bin/xhost +local: || true
        '';
      };
      services.libinput.enable = true;
      services.displayManager = {
        defaultSession = "none+icewm";
        autoLogin = {
          enable = true;
          user = "cua";
        };
      };
      users.users.cua = {
        isNormalUser = true;
        extraGroups = [ "input" ];
      };

      environment.systemPackages = with pkgs; [
        xorg.xinput
        xorg.xhost
        xorg.xdpyinfo # the :0 connectivity probe in the test script
        xi2paint
        xterm
        xdotool
        imagemagick
        python3
        jq
        procps
      ];
    };

  testScript = ''
    machine.start()
    machine.wait_for_unit("multi-user.target")
    machine.succeed("modprobe uinput && test -e /dev/uinput")

    with subtest("Real Xorg up via the display manager"):
        try:
            machine.wait_for_x()
            # With `-ac` the root-run clients can connect to :0 immediately.
            machine.wait_until_succeeds("DISPLAY=:0 xdpyinfo >/dev/null 2>&1", timeout=30)
        except Exception:
            # Diagnostics: surface why X/lightdm did not come up or why root
            # can't reach :0, so we don't burn a CI round-trip guessing.
            machine.log(machine.execute("systemctl status display-manager.service --no-pager || true")[1])
            machine.log(machine.execute("journalctl -u display-manager.service --no-pager | tail -n 200 || true")[1])
            machine.log(machine.execute("ls -la /tmp/.X11-unix/ || true")[1])
            machine.log(machine.execute("cat /var/log/X.0.log 2>/dev/null | tail -n 200 || true")[1])
            machine.log(machine.execute("find / -name 'Xorg.0.log' 2>/dev/null | head; cat $(find / -name 'Xorg.0.log' 2>/dev/null | head -1) 2>/dev/null | tail -n 120 || true")[1])
            raise
        machine.log(machine.succeed("DISPLAY=:0 xinput list --short || true"))

    with subtest("Launch XI2 paint target + a control window that holds focus"):
        machine.execute(
            "sh -lc \"DISPLAY=:0 xi2paint /tmp/xi2paint-events.log >/tmp/paint.log 2>&1 & echo \\$! >/tmp/paint-pid.txt\""
        )
        machine.execute(
            "sh -lc \"DISPLAY=:0 xterm -T 'Control' -fa Monospace -fs 14 -geometry 50x12+980+120 >/tmp/control.log 2>&1 & echo \\$! >/tmp/control-pid.txt\""
        )
        machine.wait_until_succeeds("DISPLAY=:0 xdotool search --sync --name 'XI2 MPX Paint' >/tmp/paint-xid.txt", timeout=20)
        machine.wait_until_succeeds("DISPLAY=:0 xdotool search --sync --pid $(cat /tmp/control-pid.txt) >/tmp/control-xid.txt", timeout=20)
        machine.succeed("DISPLAY=:0 xdotool windowactivate --sync $(head -1 /tmp/control-xid.txt)")
        machine.succeed("DISPLAY=:0 xdotool windowfocus --sync $(head -1 /tmp/control-xid.txt)")

    with subtest("Record GIF and pilot cua-driver through parallel_mouse_drag"):
        machine.copy_from_host("${mcpDragTest}", "/tmp/mcp-parallel-drag-test.py")
        machine.execute(
            "sh -lc '${recordGifScript} :0 /tmp/drag-frames /tmp/cua-driver-linux-parallel-drag-xserver.gif "
            "/tmp/stop-drag-recorder /tmp/ffmpeg-drag.log 8 0.12 >/dev/null 2>&1 & echo $! >/tmp/ffmpeg-drag.pid'"
        )
        result = machine.succeed("timeout 90 env DISPLAY=:0 python3 /tmp/mcp-parallel-drag-test.py 2>&1")
        machine.log(result)
        assert "parallel drag test complete" in result, result
        machine.succeed("touch /tmp/stop-drag-recorder")
        machine.wait_until_succeeds("! kill -0 $(cat /tmp/ffmpeg-drag.pid) 2>/dev/null", timeout=60)

    with subtest("Both cursors delivered cooked, window-targeted events"):
        machine.log(machine.succeed("cat /tmp/xi2paint-events.log"))
        machine.succeed("test \"$(grep -c '^PRESS ' /tmp/xi2paint-events.log)\" -eq 4")
        distinct = machine.succeed(
            "grep '^PRESS ' /tmp/xi2paint-events.log | sed -n 's/.*dev=\\([0-9]*\\).*/\\1/p' | sort -u | wc -l"
        ).strip()
        assert int(distinct) == 2, f"expected 2 distinct devices, got {distinct}"
        machine.succeed("test \"$(grep -c '^MOTION ' /tmp/xi2paint-events.log)\" -ge 4")

    with subtest("The drags did not steal focus from the control window"):
        active = machine.succeed("DISPLAY=:0 xdotool getactivewindow").strip()
        control = machine.succeed("head -1 /tmp/control-xid.txt").strip()
        assert active == control, f"focus stolen: active={active} control={control}"

    with subtest("GIF artifact exists"):
        machine.succeed("test -s /tmp/cua-driver-linux-parallel-drag-xserver.gif")

    with subtest("Copy GIF out of the VM"):
        machine.copy_from_machine("/tmp/cua-driver-linux-parallel-drag-xserver.gif", "")
  '';
}
