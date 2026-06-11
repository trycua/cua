# Linux parallel multi-cursor drag GIF test
#
# Pilots cua-driver through its Linux MPX `parallel_mouse_drag` path: two
# per-session master pointers drawing concurrent strokes into the SAME window,
# while a separate control window keeps the input focus. Proves the three
# guarantees the feature is built on:
#
#   1. Concurrent delivery — both masters' presses/motions/releases reach the
#      target window as cooked, window-targeted XI2 events (an XI2 paint app
#      logs every event with its device id; we assert two distinct devices).
#   2. No focus steal — the "shield grab" keeps the window manager blind to
#      the presses, so the control window stays active throughout.
#   3. Pixel-exact endpoints — the recorded GIF shows two crossing strokes.
#
# Unlike the other Linux visual tests, this one needs a REAL Xorg server, not
# Xvfb: the MPX path attaches uinput slave devices to per-session master
# pointers, and only a real Xorg with the libinput input driver enumerates
# uinput devices as X input devices (Xvfb — like the old Xtigervnc setup —
# does not). So we launch Xorg with the `dummy` video driver + libinput, with
# the `uinput` kernel module loaded.
#
# To run: nix build .#checks.x86_64-linux.cua-driver-linux-parallel-drag-gif
#
{
  pkgs,
  lib ? pkgs.lib,
  cuaDriverModule,
  ...
}:

let
  # XI2 paint + event logger. Selects XI2 events for ALL master devices on its
  # own window and, for each ButtonPress/Motion/ButtonRelease, appends a line
  # "<KIND> dev=<id> x=<> y=<>" to a log file and paints a square (per-device
  # colour). The log is what proves cooked, window-delivered, multi-device
  # input — not just raw motion. Prints "READY 0x<xid>" on stdout at startup.
  xi2paintSrc = pkgs.writeText "xi2paint.c" ''
    #include <X11/Xlib.h>
    #include <X11/extensions/XInput2.h>
    #include <stdio.h>
    #include <stdlib.h>
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
        XISetMask(mask, XI_Enter);
        XISetMask(mask, XI_FocusIn);
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
        fprintf(logf, "READY 0x%lx\n", win);

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
                } else if (t == XI_Enter || t == XI_FocusIn) {
                    XIEnterEvent *ee = e.xcookie.data;
                    fprintf(logf, "%s dev=%d\n", t == XI_Enter ? "ENTER" : "FOCUSIN",
                            ee->deviceid);
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

  # Real Xorg config: dummy video driver (software framebuffer) + libinput
  # input hotplug so cua-driver's uinput slaves get enumerated as X devices.
  xorgConf = pkgs.writeText "xorg-dummy.conf" ''
    Section "ServerFlags"
      Option "AutoAddDevices" "true"
      Option "AutoEnableDevices" "true"
      Option "DontVTSwitch" "true"
    EndSection
    Section "Device"
      Identifier "dummy"
      Driver     "dummy"
      VideoRam   256000
    EndSection
    Section "Monitor"
      Identifier  "mon"
      HorizSync   30.0 - 1000.0
      VertRefresh 30.0 - 200.0
      Modeline "1280x1024" 109.00 1280 1368 1496 1712 1024 1027 1034 1063 -hsync +vsync
    EndSection
    Section "Screen"
      Identifier "screen"
      Device     "dummy"
      Monitor    "mon"
      DefaultDepth 24
      SubSection "Display"
        Depth 24
        Modes "1280x1024"
      EndSubSection
    EndSection
  '';

  # Manually-launched Xorg needs an explicit module path covering the server's
  # own modules plus the separately-packaged dummy video + libinput drivers.
  # Each entry must be the PARENT `lib/xorg/modules` directory, not a `drivers`
  # or `input` subdir: Xorg's loader appends the standard subdir itself (e.g.
  # `drivers/` for video drivers), so pointing at `.../modules/drivers` makes it
  # search `.../modules/drivers/drivers/dummy_drv.so` and find nothing — leaving
  # the server with no screens, which it abandons right after opening its socket.
  xorgModulePath = lib.concatStringsSep "," [
    "${pkgs.xorg.xorgserver}/lib/xorg/modules"
    "${pkgs.xorg.xf86videodummy}/lib/xorg/modules"
    "${pkgs.xorg.xf86inputlibinput}/lib/xorg/modules"
  ];

  mcpDragTest = pkgs.writeText "mcp-parallel-drag-test.py" ''
    import json
    import os
    import subprocess
    import sys
    import threading
    import time

    DRIVER_BIN = os.environ.get("CUA_DRIVER_BIN", "cua-driver")

    def start_driver():
        proc = subprocess.Popen(
            [DRIVER_BIN, "mcp", "--no-daemon-relaunch"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env={**os.environ},
        )

        def drain_stderr():
            for line in proc.stderr:
                sys.stderr.buffer.write(line)
                sys.stderr.buffer.flush()

        threading.Thread(target=drain_stderr, daemon=True).start()
        return proc

    def send(proc, method, params=None, req_id=None):
        msg = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            msg["params"] = params
        if req_id is not None:
            msg["id"] = req_id
        proc.stdin.write((json.dumps(msg) + "\n").encode())
        proc.stdin.flush()

    def recv(proc, timeout=30):
        result = [None]

        def reader():
            result[0] = proc.stdout.readline()

        thread = threading.Thread(target=reader)
        thread.start()
        thread.join(timeout)
        if thread.is_alive():
            raise TimeoutError("No response within timeout")
        line = result[0].decode().strip()
        if not line:
            raise RuntimeError("Driver returned an empty response")
        return json.loads(line)

    def call_tool(proc, req_id, name, arguments, timeout=60):
        send(proc, "tools/call", {"name": name, "arguments": arguments}, req_id=req_id)
        resp = recv(proc, timeout=timeout)
        if "error" in resp and resp["error"] is not None:
            raise RuntimeError(f"{name} failed: {resp}")
        if resp.get("result", {}).get("isError"):
            raise RuntimeError(f"{name} returned isError: {resp}")
        return resp

    def main():
        with open("/tmp/paint-xid.txt", "r", encoding="utf-8") as f:
            window_id = int(f.readline().strip())

        proc = start_driver()
        try:
            send(proc, "initialize", {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "nixos-parallel-drag-test", "version": "1.0.0"},
            }, req_id=1)
            recv(proc)
            send(proc, "notifications/initialized", {})
            time.sleep(0.3)

            # Two concurrent strokes forming the left half of an "X" with two
            # cursors, into a window that does NOT hold focus.
            call_tool(proc, 2, "parallel_mouse_drag", {"drags": [
                {"session": "agent-1", "window_id": window_id,
                 "from_x": 100.0, "from_y": 100.0, "to_x": 380.0, "to_y": 420.0,
                 "duration_ms": 2500, "steps": 80},
                {"session": "agent-2", "window_id": window_id,
                 "from_x": 700.0, "from_y": 100.0, "to_x": 420.0, "to_y": 420.0,
                 "duration_ms": 2500, "steps": 80},
            ]})
            time.sleep(0.6)

            # Second pass completes both "X" shapes.
            call_tool(proc, 3, "parallel_mouse_drag", {"drags": [
                {"session": "agent-1", "window_id": window_id,
                 "from_x": 100.0, "from_y": 420.0, "to_x": 380.0, "to_y": 120.0,
                 "duration_ms": 2500, "steps": 80},
                {"session": "agent-2", "window_id": window_id,
                 "from_x": 700.0, "from_y": 420.0, "to_x": 420.0, "to_y": 120.0,
                 "duration_ms": 2500, "steps": 80},
            ]})
            time.sleep(0.6)
            print("parallel drag test complete", flush=True)
        finally:
            proc.stdin.close()
            proc.terminate()
            proc.wait(timeout=5)

    if __name__ == "__main__":
        main()
  '';

  recordGifScript = import ./record-x11-gif.nix { inherit pkgs; };
in

pkgs.testers.nixosTest {
  name = "cua-driver-linux-parallel-drag-gif-test";
  meta.maintainers = [ ];

  nodes.machine =
    {
      pkgs,
      ...
    }:
    {
      imports = [ cuaDriverModule ];
      virtualisation = {
        cores = 2;
        memorySize = 2048;
      };
      services.cua-driver.enable = true;
      # The MPX path opens /dev/uinput and relies on Xorg+libinput enumerating
      # the resulting devices, so the uinput module must be present.
      boot.kernelModules = [ "uinput" ];
      environment.systemPackages = with pkgs; [
        xorg.xorgserver
        xorg.xf86videodummy
        xorg.xf86inputlibinput
        xorg.xinput
        xi2paint
        xterm
        openbox
        picom
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

    with subtest("Start a real Xorg (dummy video + libinput) and a WM"):
        machine.execute(
            "Xorg :99 -ac -noreset -keeptty -nolisten tcp "
            "-config ${xorgConf} -modulepath ${xorgModulePath} "
            "-logfile /tmp/xorg.log >/tmp/xorg.out 2>&1 &"
        )
        machine.wait_until_succeeds("test -e /tmp/.X11-unix/X99", timeout=30)
        try:
            machine.wait_until_succeeds("DISPLAY=:99 xdpyinfo >/dev/null 2>&1", timeout=30)
        except Exception:
            # Xorg opens its socket before completing screen/driver setup, so a
            # missing video/input driver shows up here as an unreachable server.
            # Surface the server log (and stderr) so the failure is diagnosable.
            machine.log(machine.succeed("cat /tmp/xorg.log 2>/dev/null || echo '(no /tmp/xorg.log)'"))
            machine.log(machine.succeed("cat /tmp/xorg.out 2>/dev/null || echo '(no /tmp/xorg.out)'"))
            raise
        machine.execute("DISPLAY=:99 openbox >/tmp/openbox.log 2>&1 &")
        machine.execute("DISPLAY=:99 picom --backend xrender >/tmp/picom.log 2>&1 &")

    with subtest("Launch the XI2 paint target and a control window that holds focus"):
        machine.execute(
            "sh -lc \"DISPLAY=:99 xi2paint /tmp/xi2paint-events.log >/tmp/paint.log 2>&1 & echo \\$! >/tmp/paint-pid.txt\""
        )
        machine.execute(
            "sh -lc \"DISPLAY=:99 xterm -T 'Control' -fa Monospace -fs 14 -geometry 50x12+980+120 >/tmp/control.log 2>&1 & echo \\$! >/tmp/control-pid.txt\""
        )
        machine.wait_until_succeeds("DISPLAY=:99 xdotool search --sync --name 'XI2 MPX Paint' >/tmp/paint-xid.txt", timeout=20)
        machine.wait_until_succeeds("DISPLAY=:99 xdotool search --sync --pid $(cat /tmp/control-pid.txt) >/tmp/control-xid.txt", timeout=20)
        # Give input focus to the CONTROL window — the drags target the paint
        # window, which must never steal it.
        machine.succeed("DISPLAY=:99 xdotool windowactivate --sync $(head -1 /tmp/control-xid.txt)")
        machine.succeed("DISPLAY=:99 xdotool windowfocus --sync $(head -1 /tmp/control-xid.txt)")

    with subtest("Record GIF and pilot cua-driver through parallel_mouse_drag"):
        machine.copy_from_host("${mcpDragTest}", "/tmp/mcp-parallel-drag-test.py")
        machine.execute(
            "sh -lc '${recordGifScript} :99 /tmp/drag-frames /tmp/cua-driver-linux-parallel-drag.gif "
            "/tmp/stop-drag-recorder /tmp/ffmpeg-drag.log 8 0.12 >/dev/null 2>&1 & echo $! >/tmp/ffmpeg-drag.pid'"
        )
        result = machine.succeed("timeout 90 env DISPLAY=:99 python3 /tmp/mcp-parallel-drag-test.py 2>&1")
        machine.log(result)
        assert "parallel drag test complete" in result, result
        machine.succeed("touch /tmp/stop-drag-recorder")
        machine.wait_until_succeeds("! kill -0 $(cat /tmp/ffmpeg-drag.pid) 2>/dev/null", timeout=60)
        machine.log(machine.succeed("sh -lc 'cat /tmp/ffmpeg-drag.log || true'"))

    with subtest("Both cursors delivered cooked, window-targeted events"):
        machine.log(machine.succeed("cat /tmp/xi2paint-events.log"))
        # Four presses total (two strokes x two cursors).
        machine.succeed("test \"$(grep -c '^PRESS ' /tmp/xi2paint-events.log)\" -eq 4")
        # From two DISTINCT master pointer devices — i.e. genuinely concurrent
        # multi-cursor input, not one pointer reused.
        distinct = machine.succeed(
            "grep '^PRESS ' /tmp/xi2paint-events.log | sed -n 's/.*dev=\\([0-9]*\\).*/\\1/p' | sort -u | wc -l"
        ).strip()
        assert int(distinct) == 2, f"expected 2 distinct devices, got {distinct}"
        machine.succeed("test \"$(grep -c '^MOTION ' /tmp/xi2paint-events.log)\" -ge 4")

    with subtest("The drags did not steal focus from the control window"):
        active = machine.succeed("DISPLAY=:99 xdotool getactivewindow").strip()
        control = machine.succeed("head -1 /tmp/control-xid.txt").strip()
        assert active == control, f"focus stolen: active={active} control={control}"

    with subtest("GIF artifact exists"):
        machine.succeed("test -s /tmp/cua-driver-linux-parallel-drag.gif")

    with subtest("Copy GIF out of the VM"):
        machine.copy_from_machine("/tmp/cua-driver-linux-parallel-drag.gif", "")
  '';
}
