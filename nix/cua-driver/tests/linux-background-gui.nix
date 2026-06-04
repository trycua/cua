# Linux background GUI test — matrix over REAL desktop apps, via AT-SPI.
#
# X11 only routes keystrokes to the focused toplevel's focused widget, so the
# driver reads/types into GUI apps focus-free through AT-SPI instead. This test
# stands up an AT-SPI accessibility bus, launches a real app in the background
# (a separate control terminal stays focused), then drives cua-driver against
# the inactive app window and asserts the accessibility tree is reachable.
#
# Two classes of entry live in `apps`:
#
#   1. SKELETON entries (skeleton = true) — the real-app matrix. These run a
#      LENIENT, READ-ONLY smoke test: find the app window, drive cua-driver
#      `page get_text` (read) against it, assert it returned a non-error
#      accessibility response, assert focus stayed on the control terminal, and
#      produce a GIF. Focus-free WRITE / typed-text assertions are intentionally
#      OUT OF SCOPE here — they are added later per-app via trajectories.
#
#   2. Full entries (skeleton unset) — chromium and tk. These keep their
#      original full behaviour: chromium exercises the approved CDP focus-free
#      *write* override (Input.insertText into the background window); tk
#      exercises the Tk `send` focus-free write override. Both also type via the
#      native AT-SPI path and read it back.
#
# Each run screen-records X11 display :99 into a per-app animated GIF
# (/tmp/cua-driver-linux-background-gui-<app>.gif) and copies it into the test
# derivation's $out, so the Nix workflow's `visual: true` artifact upload finds
# a `.gif` for every matrix job. The GIF is stopped and copied out before any
# toolkit assertion can fail, so even failing jobs still upload one.
#
# `app` selects one entry from `apps`, so flake.nix wires one matrix job per app.
# To run: nix build .#checks.x86_64-linux.cua-driver-linux-background-gui-<app>
{
  pkgs,
  lib ? pkgs.lib,
  cuaDriverModule,
  app,
  ...
}:

let
  typed = "cuatyped1234";

  # Records an animated GIF of X11 display :99 while the driver interacts with
  # the background window, so every matrix job (not just the dedicated GIF
  # tests) produces a `.gif` artifact for the Nix workflow's `visual: true`
  # upload. Shared with the linux-cursor-click / linux-background-terminal GIF
  # tests. Needs `pkgs.imagemagick` in environment.systemPackages (below).
  recordGifScript = import ./record-x11-gif.nix { inherit pkgs; };

  # Distinct per-app GIF name so concurrent matrix jobs and their artifacts
  # never collide; the workflow's `find -L "<result>/" -name '*.gif'` picks it
  # up once it has been copied into the test derivation's $out.
  outputGif = "/tmp/cua-driver-linux-background-gui-${app}.gif";

  # Plain python3 (stdlib only) to drive the MCP JSON-RPC handshake in the test.
  # The driver now speaks AT-SPI natively over D-Bus (no pyatspi / GI typelibs),
  # so no accessibility Python packages are needed anywhere.
  testPython = pkgs.python3;

  # Shared environment: same session D-Bus + a11y settings for the apps and the
  # driver, so the driver's native AT-SPI client reaches the same registry the
  # apps register with. Fixed bus path so every `machine.*` shell can opt in.
  a11yEnv = lib.concatStringsSep " " [
    "DISPLAY=:99"
    "DBUS_SESSION_BUS_ADDRESS=unix:path=/tmp/cua-session-bus"
    "XDG_RUNTIME_DIR=/run/user/0"
    "XDG_DATA_DIRS=/run/current-system/sw/share"
    # A GTK3 app dlopens libatk-bridge-2.0.so by soname to join the AT-SPI bus;
    # in this hand-rolled session it isn't on the loader path, so expose it.
    # (Chromium ships its own AT-SPI implementation and doesn't need this.)
    "LD_LIBRARY_PATH=${pkgs.at-spi2-atk}/lib"
    # GTK3 only exports its accessible tree when assistive tech is enabled. That
    # flag is the GSettings key org.gnome.desktop.interface toolkit-accessibility.
    # Use the keyfile backend with a shared config dir so a one-shot `gsettings
    # set` is visible to every app + the bus launcher, without poking org.a11y.Bus
    # at runtime (which D-Bus-activates a second launcher and breaks the bus).
    "GSETTINGS_BACKEND=keyfile"
    "XDG_CONFIG_HOME=/tmp/cua-cfg"
    "GSETTINGS_SCHEMA_DIR=${pkgs.gsettings-desktop-schemas}/share/gsettings-schemas/${pkgs.gsettings-desktop-schemas.name}/glib-2.0/schemas"
    "GTK_MODULES=gail:atk-bridge"
    "GNOME_ACCESSIBILITY=1"
    "QT_ACCESSIBILITY=1"
    "NO_AT_BRIDGE=0"
  ];

  # A page whose input is autofocused; its title is fixed so the window can be
  # found by name. Readback is via AT-SPI, so no JS mirroring is needed. Used by
  # the chromium full entry.
  htmlFile = pkgs.writeText "cua-input.html" ''
    <html><head><title>cua-initial</title></head>
    <body><input autofocus></body></html>
  '';

  # ── Per-toolkit launch-environment helpers (shared across the real-app sets) ──

  # GTK4 talks AT-SPI directly (not via atk-bridge), but only exports its
  # accessible tree when it selects the AT-SPI backend at startup; GTK_A11Y=atspi
  # forces it on. x11 backend + cairo renderer keep it headless-safe.
  gtk4EnvExports = ''
    export GTK_A11Y=atspi
    export GDK_BACKEND=x11
    export GSK_RENDERER=cairo
  '';

  # Qt5: point Qt at qtbase's xcb platform plugin (bare apps don't always inherit
  # it) and force the AT-SPI bridge on regardless of the bus enabled-handshake.
  qt5EnvExports = ''
    export QT_QPA_PLATFORM=xcb
    export QT_PLUGIN_PATH=${pkgs.qt5.qtbase}/${pkgs.qt5.qtbase.qtPluginPrefix}
    export QT_QPA_PLATFORM_PLUGIN_PATH=${pkgs.qt5.qtbase}/${pkgs.qt5.qtbase.qtPluginPrefix}/platforms
    export QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1
    export QT_ACCESSIBILITY=1
  '';

  # Qt6: Qt 6.5+ aborts loading the xcb plugin unless libxcb-cursor is present;
  # put it on LD_LIBRARY_PATH and force the AT-SPI bridge on.
  qt6EnvExports = ''
    export QT_QPA_PLATFORM=xcb
    export LD_LIBRARY_PATH=${pkgs.xcb-util-cursor}/lib:''${LD_LIBRARY_PATH:-}
    export QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1
    export QT_ACCESSIBILITY=1
  '';

  # Electron: heavy Chromium embed; standard headless-safe flags.
  electronCommonFlags = "--no-sandbox --disable-gpu --disable-dev-shm-usage";

  # ── CDP (Chrome DevTools Protocol) focus-free write — approved override ──────
  # AT-SPI exposes Chromium read-only, so the driver can't write into a
  # *background* browser window through it. CDP talks to the renderer over the
  # debug socket instead: Input.insertText lands in the page's focused DOM
  # element (document.activeElement) regardless of whether the OS window holds X
  # focus. This is a Chromium-specific override, not the generic path.
  cdpPort = 9222;
  cdpMarker = "cdptyped5678";

  # Self-contained CDP client (stdlib only: HTTP target discovery + a minimal
  # RFC-6455 WebSocket client), so it runs under plain python3 with no extra deps.
  cdpWriteScript = pkgs.writeText "cdp-write.py" ''
    import json, sys, time, socket, base64, os, struct, urllib.request
    from urllib.parse import urlparse

    PORT = ${toString cdpPort}
    MARKER = "${cdpMarker}"

    def http_json(path):
        url = "http://127.0.0.1:%d%s" % (PORT, path)
        with urllib.request.urlopen(url, timeout=10) as r:
            return json.load(r)

    def pick_page():
        cands = [t for t in http_json("/json")
                 if t.get("type") == "page" and t.get("webSocketDebuggerUrl")]
        for t in cands:
            if t.get("title") == "cua-initial" or "cua-input" in t.get("url", ""):
                return t["webSocketDebuggerUrl"]
        return cands[0]["webSocketDebuggerUrl"] if cands else None

    class WS:
        def __init__(self, url):
            u = urlparse(url)
            self.sock = socket.create_connection((u.hostname, u.port), timeout=15)
            key = base64.b64encode(os.urandom(16)).decode()
            path = (u.path or "/") + (("?" + u.query) if u.query else "")
            req = (
                "GET %s HTTP/1.1\r\nHost: %s:%d\r\nUpgrade: websocket\r\n"
                "Connection: Upgrade\r\nSec-WebSocket-Key: %s\r\n"
                "Sec-WebSocket-Version: 13\r\n\r\n"
            ) % (path, u.hostname, u.port, key)
            self.sock.sendall(req.encode())
            self._buf = b""
            while b"\r\n\r\n" not in self._buf:
                chunk = self.sock.recv(4096)
                if not chunk:
                    raise RuntimeError("ws handshake closed early")
                self._buf += chunk
            head, self._buf = self._buf.split(b"\r\n\r\n", 1)
            if b"101" not in head.split(b"\r\n")[0]:
                raise RuntimeError("ws handshake failed: " + head.decode("latin1"))

        def _exact(self, n):
            while len(self._buf) < n:
                chunk = self.sock.recv(4096)
                if not chunk:
                    raise RuntimeError("ws closed")
                self._buf += chunk
            out, self._buf = self._buf[:n], self._buf[n:]
            return out

        def send_text(self, text):
            payload = text.encode()
            n = len(payload)
            header = bytearray([0x81])
            if n < 126:
                header.append(0x80 | n)
            elif n < 65536:
                header.append(0x80 | 126); header += struct.pack(">H", n)
            else:
                header.append(0x80 | 127); header += struct.pack(">Q", n)
            mask = os.urandom(4)
            header += mask
            self.sock.sendall(bytes(header) + bytes(b ^ mask[i % 4] for i, b in enumerate(payload)))

        def recv_text(self):
            data = b""
            while True:
                b0, b1 = self._exact(2)
                fin, opcode, masked, length = b0 & 0x80, b0 & 0x0F, b1 & 0x80, b1 & 0x7F
                if length == 126:
                    length = struct.unpack(">H", self._exact(2))[0]
                elif length == 127:
                    length = struct.unpack(">Q", self._exact(8))[0]
                mask = self._exact(4) if masked else None
                payload = self._exact(length)
                if mask:
                    payload = bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
                if opcode == 0x8:
                    raise RuntimeError("ws closed by server")
                if opcode in (0x9, 0xA):
                    continue
                data += payload
                if fin:
                    return data.decode()

        def close(self):
            try:
                self.sock.close()
            except Exception:
                pass

    ws_url = None
    for _ in range(30):
        try:
            ws_url = pick_page()
        except Exception:
            ws_url = None
        if ws_url:
            break
        time.sleep(1)
    if not ws_url:
        print("NO_CDP_PAGE_TARGET", flush=True); sys.exit(1)

    ws = WS(ws_url)
    _id = [0]
    def cmd(method, params=None):
        _id[0] += 1
        mid = _id[0]
        ws.send_text(json.dumps({"id": mid, "method": method, "params": params or {}}))
        while True:
            msg = json.loads(ws.recv_text())
            if msg.get("id") == mid:
                return msg

    cmd("Runtime.enable")
    cmd("DOM.enable")
    # Focus + clear the page input through the DOM (not OS window focus).
    cmd("Runtime.evaluate", {"expression": 'var i=document.querySelector("input"); i.focus(); i.value=""; "ok"'})
    # The override: CDP injects into the renderer's focused element, no OS focus.
    cmd("Input.insertText", {"text": MARKER})
    time.sleep(0.3)
    res = cmd("Runtime.evaluate", {"expression": "document.querySelector('input').value", "returnByValue": True})
    val = res.get("result", {}).get("result", {}).get("value", "")
    print("CDP_VALUE: " + repr(val), flush=True)
    ws.close()
    if val == MARKER:
        print("CDP_READBACK_OK", flush=True); sys.exit(0)
    print("CDP_READBACK_MISMATCH", flush=True); sys.exit(1)
  '';

  # Tk (tkinter) app — Tk has no AT-SPI bridge, so focus-free writes use Tk's
  # `send` command instead: the app registers itself with a known name, and the
  # driver injects text by invoking `wish` to send Tcl commands over X11 IPC.
  # This is the Tk-specific override (like CDP for Chromium), proving that
  # non-accessible toolkits can still support background input with bespoke paths.
  tkEnv = pkgs.python3.withPackages (ps: [ ps.tkinter ]);
  tkScript = pkgs.writeText "cua-tk.py" ''
    import tkinter as tk
    root = tk.Tk()
    root.title("cua-initial")
    # Register the app with a known name so `send` commands can reach it.
    tk._default_root.tk.call('tk', 'appname', 'cua-tk-target')
    entry = tk.Entry(root, width=40, name='entry')
    entry.pack(padx=20, pady=20)
    entry.focus_set()
    root.geometry("400x120+700+150")
    root.mainloop()
  '';

  # ── Helpers to build real-app skeleton entries tersely ──────────────────────
  # mkSkeleton builds a "skeleton = true" app entry: a launch script (the given
  # `cmd` run with the per-toolkit env exports) plus a `windowMatch` xdotool
  # search expression used to find the background window.
  mkSkeleton =
    {
      packages,
      memoryMB ? 2048,
      envExports ? "",
      cmd,
      windowMatch,
    }:
    {
      inherit packages memoryMB windowMatch;
      skeleton = true;
      launch = pkgs.writeShellScript "cua-launch-${app}.sh" ''
        ${envExports}
        exec ${cmd}
      '';
    };

  apps = {
    # ── GTK3 real apps (READ-ONLY SKELETON) ─────────────────────────────────
    gtk3-gedit = mkSkeleton {
      packages = [ pkgs.gedit ];
      cmd = "${pkgs.gedit}/bin/gedit --new-window";
      windowMatch = "--class gedit";
    };
    gtk3-mousepad = mkSkeleton {
      packages = [ pkgs.mousepad ];
      cmd = "${pkgs.mousepad}/bin/mousepad --disable-server";
      windowMatch = "--class mousepad";
    };
    gtk3-geany = mkSkeleton {
      packages = [ pkgs.geany ];
      cmd = "${pkgs.geany}/bin/geany";
      windowMatch = "--class geany";
    };
    gtk3-scite = mkSkeleton {
      packages = [ pkgs.scite ];
      cmd = "${pkgs.scite}/bin/SciTE";
      windowMatch = "--class scite";
    };
    gtk3-abiword = mkSkeleton {
      packages = [ pkgs.abiword ];
      cmd = "${pkgs.abiword}/bin/abiword";
      windowMatch = "--class abiword";
    };

    # ── GTK4 real apps (READ-ONLY SKELETON) ─────────────────────────────────
    gtk4-text-editor = mkSkeleton {
      packages = [ pkgs.gnome-text-editor ];
      envExports = gtk4EnvExports;
      cmd = "${pkgs.gnome-text-editor}/bin/gnome-text-editor --new-window";
      windowMatch = "--class org.gnome.TextEditor";
    };
    gtk4-characters = mkSkeleton {
      packages = [ pkgs.gnome-characters ];
      envExports = gtk4EnvExports;
      cmd = "${pkgs.gnome-characters}/bin/gnome-characters";
      windowMatch = "--class org.gnome.Characters";
    };
    gtk4-console = mkSkeleton {
      packages = [ pkgs.gnome-console ];
      envExports = gtk4EnvExports;
      cmd = "${pkgs.gnome-console}/bin/kgx";
      windowMatch = "--class org.gnome.Console";
    };
    gtk4-contacts = mkSkeleton {
      packages = [ pkgs.gnome-contacts ];
      envExports = gtk4EnvExports;
      cmd = "${pkgs.gnome-contacts}/bin/gnome-contacts";
      windowMatch = "--class org.gnome.Contacts";
    };
    gtk4-calendar = mkSkeleton {
      packages = [ pkgs.gnome-calendar ];
      envExports = gtk4EnvExports;
      cmd = "${pkgs.gnome-calendar}/bin/gnome-calendar";
      windowMatch = "--class org.gnome.Calendar";
    };

    # ── Qt5 real apps (READ-ONLY SKELETON) ──────────────────────────────────
    # manuskript is PyQt5; klog/wsjtx/qsstv/openambit are Qt5 (qtbase 5.15.x).
    # The latter four are ham-radio / hardware apps and may pop first-run or
    # hardware dialogs; the lenient window matcher + PID/newest-window fallback
    # tolerate that. No lighter Qt5 *editor* was available in the pin (juffed and
    # notepadqq are absent/removed), so these were retained.
    qt5-manuskript = mkSkeleton {
      packages = [ pkgs.manuskript ];
      envExports = qt5EnvExports;
      cmd = "${pkgs.manuskript}/bin/manuskript";
      windowMatch = "--class manuskript";
    };
    qt5-klog = mkSkeleton {
      packages = [ pkgs.klog ];
      envExports = qt5EnvExports;
      cmd = "${pkgs.klog}/bin/klog";
      windowMatch = "--class klog";
    };
    qt5-wsjtx = mkSkeleton {
      packages = [ pkgs.wsjtx ];
      envExports = qt5EnvExports;
      cmd = "${pkgs.wsjtx}/bin/wsjtx";
      windowMatch = "--class wsjtx";
    };
    qt5-qsstv = mkSkeleton {
      packages = [ pkgs.qsstv ];
      envExports = qt5EnvExports;
      cmd = "${pkgs.qsstv}/bin/qsstv";
      windowMatch = "--class qsstv";
    };
    qt5-openambit = mkSkeleton {
      packages = [ pkgs.openambit ];
      envExports = qt5EnvExports;
      cmd = "${pkgs.openambit}/bin/openambit";
      windowMatch = "--class openambit";
    };

    # ── Qt6 real apps (READ-ONLY SKELETON) ──────────────────────────────────
    # All from the kdePackages (Qt6) scope, plus ghostwriter (Qt6) and qownnotes
    # (Qt6). kwrite is not packaged separately in this pin, so qownnotes (a Qt6
    # note editor) takes its slot.
    qt6-kate = mkSkeleton {
      packages = [ pkgs.kdePackages.kate ];
      envExports = qt6EnvExports;
      cmd = "${pkgs.kdePackages.kate}/bin/kate --new";
      windowMatch = "--class kate";
    };
    qt6-kcalc = mkSkeleton {
      packages = [ pkgs.kdePackages.kcalc ];
      envExports = qt6EnvExports;
      cmd = "${pkgs.kdePackages.kcalc}/bin/kcalc";
      windowMatch = "--class kcalc";
    };
    qt6-okular = mkSkeleton {
      packages = [ pkgs.kdePackages.okular ];
      envExports = qt6EnvExports;
      cmd = "${pkgs.kdePackages.okular}/bin/okular";
      windowMatch = "--class okular";
    };
    qt6-ghostwriter = mkSkeleton {
      packages = [ pkgs.kdePackages.ghostwriter ];
      envExports = qt6EnvExports;
      cmd = "${pkgs.kdePackages.ghostwriter}/bin/ghostwriter";
      windowMatch = "--class ghostwriter";
    };
    qt6-qownnotes = mkSkeleton {
      packages = [ pkgs.qownnotes ];
      envExports = qt6EnvExports;
      cmd = "${pkgs.qownnotes}/bin/QOwnNotes";
      windowMatch = "--class qownnotes";
    };

    # ── Electron real apps (READ-ONLY SKELETON) ─────────────────────────────
    # Heavy Chromium embeds; given more memory and a longer CI timeout. Read-only
    # skeleton (CDP write is exercised by the chromium full entry instead).
    electron-marktext = mkSkeleton {
      packages = [ pkgs.marktext ];
      memoryMB = 4096;
      cmd = "${pkgs.marktext}/bin/marktext ${electronCommonFlags}";
      windowMatch = "--class marktext";
    };
    electron-zettlr = mkSkeleton {
      packages = [ pkgs.zettlr ];
      memoryMB = 4096;
      cmd = "${pkgs.zettlr}/bin/zettlr ${electronCommonFlags}";
      windowMatch = "--class zettlr";
    };
    electron-vscodium = mkSkeleton {
      packages = [ pkgs.vscodium ];
      memoryMB = 6144;
      cmd = "${pkgs.vscodium}/bin/codium ${electronCommonFlags} --disable-workspace-trust --skip-welcome --disable-telemetry --new-window";
      windowMatch = "--class codium";
    };
    electron-joplin = mkSkeleton {
      packages = [ pkgs.joplin-desktop ];
      memoryMB = 4096;
      cmd = "${pkgs.joplin-desktop}/bin/joplin-desktop ${electronCommonFlags}";
      windowMatch = "--class joplin";
    };
    electron-logseq = mkSkeleton {
      packages = [ pkgs.logseq ];
      memoryMB = 6144;
      cmd = "${pkgs.logseq}/bin/logseq ${electronCommonFlags}";
      windowMatch = "--class logseq";
    };

    # ── Full entries (unchanged behaviour): chromium (CDP) + tk (send) ───────
    chromium = {
      packages = [ pkgs.chromium ];
      memoryMB = 4096;
      cdp = true;
      windowMatch = "--name cua-initial";
      launch = pkgs.writeShellScript "cua-launch-chromium.sh" ''
        exec ${pkgs.chromium}/bin/chromium \
          --no-sandbox --no-first-run --no-default-browser-check --disable-gpu \
          --force-renderer-accessibility \
          --remote-debugging-port=${toString cdpPort} --remote-allow-origins=* \
          --disable-backgrounding-occluded-windows --disable-renderer-backgrounding \
          --user-data-dir=/tmp/cua-chromium --window-position=700,150 --window-size=480,360 \
          --new-window file://${htmlFile}
      '';
    };
    tk = {
      packages = [ tkEnv pkgs.tk ];
      memoryMB = 2048;
      tksend = true;
      windowMatch = "--name cua-initial";
      launch = pkgs.writeShellScript "cua-launch-tk.sh" ''
        exec ${tkEnv}/bin/python3 ${tkScript}
      '';
    };
  };

  selected = apps.${app};
  isSkeleton = selected.skeleton or false;

  # CDP focus-free write subtest — only for Chromium-backed full entries. Asserts
  # the approved override writes into the *background* window while the control
  # terminal keeps focus. (Skeleton entries skip this.)
  cdpSubtest = lib.optionalString (selected.cdp or false) ''
    with subtest("CDP focus-free write into the background window (approved override)"):
        # CDP reaches the renderer over the debug socket, so Input.insertText lands
        # in the page's focused DOM element while the OS window stays in the
        # background. This is the one path that writes into an unfocused browser
        # window; AT-SPI exposes Chromium read-only.
        machine.copy_from_host("${cdpWriteScript}", "/tmp/cdp-write.py")
        cdp_out = machine.succeed("${a11yEnv} timeout 120 python3 /tmp/cdp-write.py 2>&1")
        machine.log(cdp_out)
        assert "CDP_READBACK_OK" in cdp_out, cdp_out
        # The override must remain focus-free: control terminal still active.
        cdp_control = machine.succeed("head -1 /tmp/control-xid.txt").strip()
        cdp_active = machine.succeed("DISPLAY=:99 xdotool getactivewindow").strip()
        assert cdp_control == cdp_active, "focus moved during CDP write: got " + cdp_active
  '';

  # Tk send focus-free write subtest — only for the Tk full entry. Reads the
  # entry value back over Tk `send`, guarded by a Tcl `after` timer + `timeout`
  # backstop so wish always terminates promptly.
  tkGetScript = pkgs.writeText "tk-get-value.tcl" ''
    set ::rc 1
    after 20000 {
        puts "TK_SEND_READBACK_TIMEOUT"
        flush stdout
        exit 1
    }
    if {[catch {send cua-tk-target {.entry get}} val]} {
        puts "TK_SEND_READBACK_ERROR: $val"
        flush stdout
        exit 1
    }
    puts $val
    flush stdout
    exit 0
  '';
  tkSubtest = lib.optionalString (selected.tksend or false) ''
    with subtest("Tk send focus-free write into the background window (Tk override)"):
        # The driver already typed via inject_tk_send in the main test. Now read
        # the entry widget's value back via Tk send to prove the write landed.
        machine.copy_from_host("${tkGetScript}", "/tmp/tk-get-value.tcl")
        status, tk_readback = machine.execute("${a11yEnv} timeout 30 ${pkgs.tk}/bin/wish /tmp/tk-get-value.tcl 2>&1")
        tk_readback = tk_readback.strip()
        machine.log("Tk send readback (exit=" + str(status) + "): " + repr(tk_readback))
        assert "${typed}" in tk_readback, f"Expected '${typed}' in Tk entry, got (exit={status}): {tk_readback}"
        # The override must remain focus-free: control terminal still active.
        tk_control = machine.succeed("head -1 /tmp/control-xid.txt").strip()
        tk_active = machine.succeed("DISPLAY=:99 xdotool getactivewindow").strip()
        assert tk_control == tk_active, "focus moved during Tk write: got " + tk_active
  '';

  # Full-entry MCP driver script: type via native AT-SPI then read back.
  mcpTest = pkgs.writeText "mcp-background-gui-test.py" ''
    import json, os, sys, threading, time

    DRIVER_BIN = os.environ.get("CUA_DRIVER_BIN", "cua-driver")

    def start_driver():
        import subprocess
        env = {**os.environ, "CUA_ATSPI_DEBUG": "1"}
        proc = subprocess.Popen(
            [DRIVER_BIN, "mcp", "--no-daemon-relaunch"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            env=env,
        )
        def drain():
            for line in proc.stderr:
                sys.stderr.buffer.write(line); sys.stderr.buffer.flush()
        threading.Thread(target=drain, daemon=True).start()
        return proc

    def send(proc, method, params=None, req_id=None):
        msg = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            msg["params"] = params
        if req_id is not None:
            msg["id"] = req_id
        proc.stdin.write((json.dumps(msg) + "\n").encode()); proc.stdin.flush()

    def recv(proc, timeout=45):
        result = [None]
        def reader():
            result[0] = proc.stdout.readline()
        t = threading.Thread(target=reader); t.start(); t.join(timeout)
        if t.is_alive():
            raise TimeoutError("No response within timeout")
        line = result[0].decode().strip()
        if not line:
            raise RuntimeError("Driver returned an empty response")
        return json.loads(line)

    def call_tool(proc, req_id, name, arguments):
        send(proc, "tools/call", {"name": name, "arguments": arguments}, req_id=req_id)
        resp = recv(proc)
        if resp.get("error"):
            raise RuntimeError(f"{name} failed: {resp}")
        if resp.get("result", {}).get("isError"):
            raise RuntimeError(f"{name} returned isError: {resp}")
        return resp

    def main():
        with open("/tmp/target-xid.txt") as f:
            target_xid = int(f.read().strip())
        with open("/tmp/target-pid.txt") as f:
            target_pid = int(f.read().strip())

        proc = start_driver()
        try:
            send(proc, "initialize", {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "nixos-background-gui-test", "version": "1.0.0"},
            }, req_id=1)
            recv(proc)
            send(proc, "notifications/initialized", {})
            time.sleep(0.3)
            # Type into the *inactive* app window — focus-free, via native AT-SPI.
            call_tool(proc, 2, "type_text", {
                "pid": target_pid,
                "window_id": target_xid,
                "text": "${typed}",
            })
            time.sleep(1.5)
            print("background GUI test typed", flush=True)

            # Read it back through the driver's *own* native AT-SPI client.
            readback = ""
            last_resp = None
            for _ in range(8):
                resp = call_tool(proc, 3, "page", {
                    "action": "get_text",
                    "pid": target_pid,
                    "window_id": target_xid,
                })
                last_resp = resp
                content = resp.get("result", {}).get("content", [])
                readback = " ".join(
                    c.get("text", "") for c in content if c.get("type") == "text"
                )
                if "${typed}" in readback:
                    break
                time.sleep(1.0)
            print("RAW_GET_TEXT_RESPONSE: " + json.dumps(last_resp), flush=True)
            print("READBACK_BEGIN", flush=True)
            print(readback, flush=True)
            print("READBACK_END", flush=True)
        finally:
            proc.stdin.close(); proc.terminate(); proc.wait(timeout=5)

    if __name__ == "__main__":
        main()
  '';

  # ── SKELETON: read-only smoke + GIF; focus-free WRITE / typed-text assertions
  # are added later via trajectories. ─────────────────────────────────────────
  # Skeleton MCP driver script: ONLY drives `page get_text` (read) against the
  # found window. It does NOT call type_text and does NOT assert any typed text.
  # It prints the raw get_text response so the testScript can check it returned a
  # non-error accessibility payload.
  skeletonMcpTest = pkgs.writeText "mcp-background-gui-skeleton.py" ''
    import json, os, sys, threading, time

    DRIVER_BIN = os.environ.get("CUA_DRIVER_BIN", "cua-driver")

    def start_driver():
        import subprocess
        env = {**os.environ, "CUA_ATSPI_DEBUG": "1"}
        proc = subprocess.Popen(
            [DRIVER_BIN, "mcp", "--no-daemon-relaunch"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            env=env,
        )
        def drain():
            for line in proc.stderr:
                sys.stderr.buffer.write(line); sys.stderr.buffer.flush()
        threading.Thread(target=drain, daemon=True).start()
        return proc

    def send(proc, method, params=None, req_id=None):
        msg = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            msg["params"] = params
        if req_id is not None:
            msg["id"] = req_id
        proc.stdin.write((json.dumps(msg) + "\n").encode()); proc.stdin.flush()

    def recv(proc, timeout=45):
        result = [None]
        def reader():
            result[0] = proc.stdout.readline()
        t = threading.Thread(target=reader); t.start(); t.join(timeout)
        if t.is_alive():
            raise TimeoutError("No response within timeout")
        line = result[0].decode().strip()
        if not line:
            raise RuntimeError("Driver returned an empty response")
        return json.loads(line)

    def main():
        with open("/tmp/target-xid.txt") as f:
            target_xid = int(f.read().strip())
        with open("/tmp/target-pid.txt") as f:
            target_pid = int(f.read().strip())

        proc = start_driver()
        try:
            send(proc, "initialize", {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "nixos-background-gui-skeleton", "version": "1.0.0"},
            }, req_id=1)
            recv(proc)
            send(proc, "notifications/initialized", {})
            time.sleep(0.3)

            # READ-ONLY: drive page/get_text against the inactive window. Retry a
            # few times — real apps (Electron/KDE) take a moment to build their
            # accessibility tree after first paint.
            readback = ""
            last_resp = None
            is_error = True
            for _ in range(10):
                send(proc, "tools/call", {
                    "name": "page",
                    "arguments": {
                        "action": "get_text",
                        "pid": target_pid,
                        "window_id": target_xid,
                    },
                }, req_id=3)
                resp = recv(proc)
                last_resp = resp
                # A transport-level error or isError=true is a non-result; keep
                # retrying. Any structured content counts as a non-error response.
                if resp.get("error") or resp.get("result", {}).get("isError"):
                    time.sleep(1.0)
                    continue
                content = resp.get("result", {}).get("content", [])
                readback = " ".join(
                    c.get("text", "") for c in content if c.get("type") == "text"
                )
                is_error = False
                if readback.strip():
                    break
                time.sleep(1.0)

            print("RAW_GET_TEXT_RESPONSE: " + json.dumps(last_resp), flush=True)
            print("GET_TEXT_IS_ERROR: " + ("yes" if is_error else "no"), flush=True)
            if not is_error:
                print("GET_TEXT_OK", flush=True)
            print("READBACK_BEGIN", flush=True)
            print(readback, flush=True)
            print("READBACK_END", flush=True)
        finally:
            proc.stdin.close(); proc.terminate(); proc.wait(timeout=5)

    if __name__ == "__main__":
        main()
  '';

  # The window-find shell command, parameterised on the app's windowMatch. Tries
  # the toolkit class/name match first, then falls back to the launched PID's
  # window, then the newest visible window — so real apps that don't expose the
  # expected class still surface a window for the read-only drive.
  # A store-path script (not an inline multi-line string): it is interpolated
  # into the Python testScript as a single `machine.wait_until_succeeds("...")`
  # argument. A multi-line shell snippet with embedded quotes/newlines would
  # break that Python string literal (it did — every GUI job failed the
  # testScript type-check). As a script path it is one safe token.
  windowFindCmd = pkgs.writeShellScript "cua-window-find.sh" ''
    export DISPLAY=:99
    xid=$(${pkgs.xdotool}/bin/xdotool search --sync --onlyvisible ${selected.windowMatch} 2>/dev/null | head -1)
    if [ -z "$xid" ]; then
      xid=$(${pkgs.xdotool}/bin/xdotool search --all --pid "$(cat /tmp/target-pid.txt)" 2>/dev/null | head -1)
    fi
    if [ -z "$xid" ]; then
      xid=$(${pkgs.xdotool}/bin/xdotool search --onlyvisible "" 2>/dev/null | tail -1)
    fi
    test -n "$xid" && printf "%s" "$xid" >/tmp/target-xid.txt && test -s /tmp/target-xid.txt
  '';

  # ── Skeleton (read-only) drive + assertions ─────────────────────────────────
  skeletonDrive = ''
    with subtest("SKELETON read-only: drive cua-driver page/get_text against the inactive window"):
        # SKELETON: read-only smoke + GIF; focus-free WRITE / typed-text
        # assertions are added later via trajectories. This path only proves the
        # app window appeared and the driver can READ its accessibility tree.
        machine.copy_from_host("${skeletonMcpTest}", "/tmp/mcp-background-gui-skeleton.py")
        machine.execute(
            "sh -lc '${recordGifScript} :99 /tmp/gui-frames ${outputGif} "
            "/tmp/stop-gui-recorder /tmp/record-gui.log 10 0.2 >/dev/null 2>&1 & echo $! >/tmp/record-gui.pid'"
        )
        status, result = machine.execute("${a11yEnv} timeout 200 python3 /tmp/mcp-background-gui-skeleton.py 2>&1")
        machine.log(result)
        # Stop the recorder and copy the GIF out *now*, before any assertion can
        # fail, so every matrix job uploads a GIF of the interaction.
        machine.execute("touch /tmp/stop-gui-recorder")
        machine.execute("timeout 60 sh -lc 'while kill -0 $(cat /tmp/record-gui.pid) 2>/dev/null; do sleep 0.2; done'")
        machine.log(machine.execute("sh -lc 'cat /tmp/record-gui.log || true'")[1])
        machine.execute("test -s ${outputGif}")
        machine.copy_from_machine("${outputGif}", "")
        # Lenient assertion: get_text returned a NON-error accessibility response.
        # Do NOT require any specific role (entry/text/...) — any content is fine.
        assert "GET_TEXT_OK" in result, (
            "driver page/get_text did not return a non-error response for the "
            "background app:\n" + result
        )

    with subtest("Focus stayed on the control terminal"):
        control = machine.succeed("head -1 /tmp/control-xid.txt").strip()
        active = machine.succeed("DISPLAY=:99 xdotool getactivewindow").strip()
        assert control == active, "expected active window " + control + ", got " + active
  '';

  # ── Full-entry (chromium/tk) drive + assertions (original behaviour) ─────────
  fullDrive = ''
    with subtest("Drive cua-driver against the inactive window (AT-SPI)"):
        machine.copy_from_host("${mcpTest}", "/tmp/mcp-background-gui-test.py")
        machine.execute(
            "sh -lc '${recordGifScript} :99 /tmp/gui-frames ${outputGif} "
            "/tmp/stop-gui-recorder /tmp/record-gui.log 10 0.2 >/dev/null 2>&1 & echo $! >/tmp/record-gui.pid'"
        )
        status, result = machine.execute("${a11yEnv} timeout 200 python3 /tmp/mcp-background-gui-test.py 2>&1")
        machine.log(result)
        machine.execute("touch /tmp/stop-gui-recorder")
        machine.execute("timeout 60 sh -lc 'while kill -0 $(cat /tmp/record-gui.pid) 2>/dev/null; do sleep 0.2; done'")
        machine.log(machine.execute("sh -lc 'cat /tmp/record-gui.log || true'")[1])
        machine.execute("test -s ${outputGif}")
        machine.copy_from_machine("${outputGif}", "")
        assert "background GUI test typed" in result, result

    with subtest("Input landed: driver's native AT-SPI reads the window back"):
        assert any(tok in result for tok in ('frame "', 'window "', 'document', 'text "', 'entry "')), (
            "driver get_text did not return an accessibility node for the background app:\n"
            + result
        )

    with subtest("Focus stayed on the control terminal"):
        control = machine.succeed("head -1 /tmp/control-xid.txt").strip()
        active = machine.succeed("DISPLAY=:99 xdotool getactivewindow").strip()
        assert control == active, "expected active window " + control + ", got " + active

    ${cdpSubtest}
    ${tkSubtest}
  '';

  driveBody = if isSkeleton then skeletonDrive else fullDrive;
in

pkgs.testers.nixosTest {
  name = "cua-driver-linux-background-gui-${app}-test";
  meta.maintainers = [ ];

  nodes.machine =
    { pkgs, ... }:
    {
      imports = [ cuaDriverModule ];
      virtualisation = {
        cores = 2;
        memorySize = selected.memoryMB;
        diskSize = 8192;
      };
      services.cua-driver.enable = true;
      services.dbus.enable = true;
      environment.systemPackages = with pkgs; [
        xorg.xorgserver
        xterm
        openbox
        picom
        xdotool
        imagemagick               # `import` + `convert` for the screen-recorded GIF
        dbus
        at-spi2-core
        testPython
        jq
        procps
        glib                      # `gsettings` to flip toolkit-accessibility
        gsettings-desktop-schemas # provides org.gnome.desktop.interface schema
      ] ++ selected.packages;
    };

  testScript = ''
    machine.start()
    machine.wait_for_unit("multi-user.target")

    with subtest("Start X11 + session D-Bus + AT-SPI bus"):
        machine.execute("Xvfb :99 -screen 0 1280x1024x24 >/tmp/xvfb.log 2>&1 &")
        machine.wait_until_succeeds("test -e /tmp/.X11-unix/X99", timeout=10)
        machine.execute("DISPLAY=:99 openbox >/tmp/openbox.log 2>&1 &")
        machine.execute("DISPLAY=:99 picom --backend xrender >/tmp/picom.log 2>&1 &")
        machine.succeed("mkdir -p /run/user/0 && chmod 700 /run/user/0")
        machine.execute("dbus-daemon --session --address=unix:path=/tmp/cua-session-bus --fork >/tmp/dbus.log 2>&1")
        machine.wait_until_succeeds("test -S /tmp/cua-session-bus", timeout=10)
        machine.succeed("mkdir -p /tmp/cua-cfg")
        machine.execute("${a11yEnv} ${pkgs.at-spi2-core}/libexec/at-spi-bus-launcher --launch-immediately >/tmp/atspi-launcher.log 2>&1 &")
        machine.wait_until_succeeds(
            "${a11yEnv} dbus-send --session --print-reply "
            "--dest=org.freedesktop.DBus / org.freedesktop.DBus.NameHasOwner "
            "string:org.a11y.Bus | grep -q 'boolean true'",
            timeout=15,
        )
        machine.execute(
            "${a11yEnv} dbus-send --session --print-reply --dest=org.a11y.Bus "
            "/org/a11y/bus org.freedesktop.DBus.Properties.Set "
            "string:org.a11y.Status string:IsEnabled variant:boolean:true 2>&1 | tee /tmp/a11y-enable.log"
        )
        machine.log("a11y IsEnabled set: " + machine.execute("cat /tmp/a11y-enable.log")[1])
        machine.execute(
            "${a11yEnv} dbus-send --session --print-reply --dest=org.a11y.Bus "
            "/org/a11y/bus org.freedesktop.DBus.Properties.Get "
            "string:org.a11y.Status string:IsEnabled 2>&1 | tee /tmp/a11y-get.log"
        )
        machine.log("a11y IsEnabled get: " + machine.execute("cat /tmp/a11y-get.log")[1])
        machine.log("atspi-launcher.log: " + machine.execute("cat /tmp/atspi-launcher.log")[1])

    with subtest("Focused control terminal"):
        machine.execute("sh -lc 'DISPLAY=:99 xterm -T Control -geometry 60x20+40+120 >/tmp/control.log 2>&1 & echo $! >/tmp/control-pid.txt'")
        machine.wait_until_succeeds("DISPLAY=:99 xdotool search --sync --pid $(cat /tmp/control-pid.txt) >/tmp/control-xid.txt", timeout=20)
        machine.succeed("DISPLAY=:99 xdotool windowactivate --sync $(head -1 /tmp/control-xid.txt)")

    with subtest("Launch target app (${app}) in the background"):
        machine.execute("sh -lc '${a11yEnv} ${selected.launch} >/tmp/target.log 2>&1 & echo $! >/tmp/target-pid.txt'")
        # Surface the app's own stdout/stderr early so launch failures (e.g. a Qt
        # platform-plugin error) are visible instead of just a window-find timeout.
        machine.sleep(5)
        machine.log("target.log after launch: " + machine.execute("cat /tmp/target.log")[1])
        # Find the background window via the per-app matcher, with a PID / newest
        # -window fallback. Generous timeout: Electron/KDE are slow to first paint.
        machine.wait_until_succeeds("${windowFindCmd}", timeout=120)
        machine.log("target-xid: " + machine.execute("cat /tmp/target-xid.txt")[1])
        # Keep focus on the control terminal — launching the app in the
        # background must not steal focus.
        machine.succeed("DISPLAY=:99 xdotool windowactivate --sync $(head -1 /tmp/control-xid.txt)")
        machine.succeed("DISPLAY=:99 xdotool windowfocus --sync $(head -1 /tmp/control-xid.txt)")

    ${driveBody}
  '';
}
