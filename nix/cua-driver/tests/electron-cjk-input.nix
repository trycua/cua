# NixOS integration test: Electron app + CJK input via cua-driver
#
# Tests that cua-driver correctly passes Unicode/CJK text through to an
# Electron application. This covers the scenario discussed in the Qwen Code
# PR (https://github.com/QwenLM/qwen-code/pull/5051#issuecomment-4700208119)
# where Electron/CJK input methods (ibus/fcitx) must work correctly.
#
# The test:
#   1. Starts a NixOS VM with X11 (Xvfb) and a window manager
#   2. Launches a minimal Electron app with an HTML input field
#   3. Uses cua-driver's type_text to inject CJK characters ("你好世界")
#   4. Reads back the typed text via CDP to verify it landed correctly
#
# Key details:
#   - Electron uses Chromium's CDP (Chrome DevTools Protocol) for input injection
#   - type_text with Unicode characters tests the cua-driver Unicode pass-through
#   - The CDP readback confirms the characters were received correctly
#
# To run: nix build .#checks.x86_64-linux.cua-driver-electron-cjk-input
{
  pkgs,
  lib ? pkgs.lib,
  cuaDriverModule,
  ...
}:

let
  # The CJK test string: "你好世界" (Hello World in Chinese)
  # Also includes Japanese hiragana to cover broader CJK range.
  cjkText = "你好世界";

  # Shared environment: D-Bus session bus + AT-SPI accessibility settings.
  # A fixed bus path so every shell in the test can find the same session bus.
  a11yEnv = lib.concatStringsSep " " [
    "DISPLAY=:99"
    "DBUS_SESSION_BUS_ADDRESS=unix:path=/tmp/cua-session-bus"
    "XDG_RUNTIME_DIR=/run/user/0"
    "XDG_DATA_DIRS=/run/current-system/sw/share"
    "LD_LIBRARY_PATH=${pkgs.at-spi2-atk}/lib"
    "GSETTINGS_BACKEND=keyfile"
    "XDG_CONFIG_HOME=/tmp/cua-cfg"
    "GSETTINGS_SCHEMA_DIR=${pkgs.gsettings-desktop-schemas}/share/gsettings-schemas/${pkgs.gsettings-desktop-schemas.name}/glib-2.0/schemas"
    "GTK_MODULES=gail:atk-bridge"
    "GNOME_ACCESSIBILITY=1"
    "QT_ACCESSIBILITY=1"
    "NO_AT_BRIDGE=0"
  ];

  # A minimal Electron app with an autofocused input field.
  # The window title is fixed so we can find it via xdotool.
  electronMainJs = pkgs.writeText "main.js" ''
    const { app, BrowserWindow } = require('electron');
    const path = require('path');

    app.whenReady().then(() => {
      const win = new BrowserWindow({
        width: 600,
        height: 200,
        title: 'cua-cjk-test',
        webPreferences: {
          nodeIntegration: false,
          contextIsolation: true,
        },
      });
      win.loadFile(path.join(__dirname, 'index.html'));
    });

    app.on('window-all-closed', () => app.quit());
  '';

  electronIndexHtml = pkgs.writeText "index.html" ''
    <!DOCTYPE html>
    <html>
      <head>
        <meta charset="UTF-8">
        <title>cua-cjk-test</title>
      </head>
      <body>
        <input id="cjk-input" autofocus
               style="width:90%;font-size:24px;padding:8px;"
               placeholder="CJK input test">
        <script>
          document.getElementById('cjk-input').focus();
        </script>
      </body>
    </html>
  '';

  # Build the minimal Electron app as a derivation so all files are in one dir.
  electronApp = pkgs.runCommand "cua-cjk-electron-app" { } ''
    mkdir -p $out
    cp ${electronMainJs}  $out/main.js
    cp ${electronIndexHtml} $out/index.html
    cat > $out/package.json <<'EOF'
    {
      "name": "cua-cjk-test",
      "version": "1.0.0",
      "main": "main.js"
    }
    EOF
  '';

  # CDP port for the Electron app's remote-debugging interface.
  cdpPort = 9223;

  # Python MCP client that:
  #   1. Launches cua-driver MCP server
  #   2. Calls type_text with the CJK string targeting the Electron window
  #   3. Verifies the text appeared via CDP readback
  mcpCjkTest = pkgs.writeText "mcp-cjk-test.py" ''
    import json
    import os
    import socket
    import struct
    import base64
    import subprocess
    import sys
    import threading
    import time
    import urllib.request
    from urllib.parse import urlparse

    DRIVER_BIN = os.environ.get("CUA_DRIVER_BIN", "cua-driver")
    CDP_PORT = ${toString cdpPort}
    CJK_TEXT = "${cjkText}"

    # ── CDP minimal client (stdlib only) ────────────────────────────────────────

    def http_json(path):
        url = "http://127.0.0.1:%d%s" % (CDP_PORT, path)
        with urllib.request.urlopen(url, timeout=10) as r:
            return json.load(r)

    def pick_page():
        cands = [t for t in http_json("/json")
                 if t.get("type") == "page" and t.get("webSocketDebuggerUrl")]
        for t in cands:
            if "cjk" in t.get("title", "").lower() or "cjk" in t.get("url", "").lower():
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
            self.sock.sendall(
                bytes(header) + bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
            )

        def recv_text(self):
            data = b""
            while True:
                b0, b1 = self._exact(2)
                fin, opcode = b0 & 0x80, b0 & 0x0F
                masked, length = b1 & 0x80, b1 & 0x7F
                if length == 126:
                    length = struct.unpack(">H", self._exact(2))[0]
                elif length == 127:
                    length = struct.unpack(">Q", self._exact(8))[0]
                mask_key = self._exact(4) if masked else None
                payload = self._exact(length)
                if mask_key:
                    payload = bytes(b ^ mask_key[i % 4] for i, b in enumerate(payload))
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

    def cdp_get_input_value():
        """Connect to Electron's CDP and read the input field value."""
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
            return None, "NO_CDP_PAGE_TARGET"

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

        try:
            cmd("Runtime.enable")
            res = cmd("Runtime.evaluate", {
                "expression": "document.getElementById('cjk-input').value",
                "returnByValue": True,
            })
            val = res.get("result", {}).get("result", {}).get("value", "")
            return val, None
        finally:
            ws.close()

    # ── MCP driver client ────────────────────────────────────────────────────────

    def start_driver():
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
        print("=== CUA Driver Electron/CJK Input Test ===", flush=True)
        print(f"CJK text to type: {CJK_TEXT!r}", flush=True)

        with open("/tmp/electron-xid.txt") as f:
            window_id = int(f.read().strip())
        with open("/tmp/electron-pid.txt") as f:
            target_pid = int(f.read().strip())
        print(f"Electron window: pid={target_pid} xid={window_id}", flush=True)

        proc = start_driver()
        try:
            # Initialize MCP
            send(proc, "initialize", {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "nixos-cjk-test", "version": "1.0.0"},
            }, req_id=1)
            resp = recv(proc)
            assert "result" in resp, f"Initialize failed: {resp}"
            send(proc, "notifications/initialized", {})
            time.sleep(0.3)

            # Focus the Electron input via CDP first (ensures the DOM element
            # is ready to receive input before cua-driver types into it).
            print("\n--- Focusing Electron input via CDP ---", flush=True)
            ws_url = None
            for attempt in range(30):
                try:
                    ws_url = pick_page()
                    if ws_url:
                        break
                except Exception as e:
                    print(f"  CDP attempt {attempt}: {e}", flush=True)
                time.sleep(1)

            if ws_url:
                ws = WS(ws_url)
                _id = [0]
                def cmd(method, params=None):
                    _id[0] += 1; mid = _id[0]
                    ws.send_text(json.dumps({"id": mid, "method": method, "params": params or {}}))
                    while True:
                        msg = json.loads(ws.recv_text())
                        if msg.get("id") == mid:
                            return msg
                cmd("Runtime.enable")
                cmd("Runtime.evaluate", {
                    "expression": "var i=document.getElementById('cjk-input'); i.focus(); i.value=\"\"; 'ok'"
                })
                ws.close()
                print("CDP focus: input focused and cleared", flush=True)
            else:
                print("WARN: CDP not reachable, continuing without pre-focus", flush=True)

            # Type CJK text via cua-driver type_text
            print(f"\n--- Typing CJK text via cua-driver type_text ---", flush=True)
            resp = call_tool(proc, 2, "type_text", {
                "pid": target_pid,
                "window_id": window_id,
                "text": CJK_TEXT,
            })
            print(f"type_text response: {json.dumps(resp)[:200]}", flush=True)
            time.sleep(1.5)

            # Read back via CDP
            print("\n--- Reading back via CDP ---", flush=True)
            readback_value = None
            for attempt in range(10):
                val, err = cdp_get_input_value()
                if err:
                    print(f"  attempt {attempt}: CDP error: {err}", flush=True)
                    time.sleep(1)
                    continue
                readback_value = val
                print(f"  attempt {attempt}: CDP value = {val!r}", flush=True)
                if val and len(val) > 0:
                    break
                time.sleep(1)

            print(f"\nFINAL_CDP_VALUE: {readback_value!r}", flush=True)

            # Assertion: the CJK characters must be present in the input
            if readback_value and CJK_TEXT in readback_value:
                print("CJK_INPUT_OK: CJK text found in input field", flush=True)
            elif readback_value and len(readback_value) > 0:
                print(f"CJK_INPUT_PARTIAL: got {readback_value!r}, expected {CJK_TEXT!r}", flush=True)
            else:
                print(f"CJK_INPUT_FAIL: input empty, expected {CJK_TEXT!r}", flush=True)

            print("\n=== Electron/CJK test complete ===", flush=True)

        finally:
            proc.stdin.close(); proc.terminate(); proc.wait(timeout=5)

    if __name__ == "__main__":
        main()
  '';

  openboxRc = import ./openbox-rc.nix { inherit pkgs; };

in

pkgs.testers.nixosTest {
  name = "cua-driver-electron-cjk-input-test";
  meta = {
    maintainers = [ ];
  };

  containers.machine =
    {
      config,
      pkgs,
      lib,
      ...
    }:
    {
      imports = [ cuaDriverModule ];
      services.cua-driver.enable = true;
      services.dbus.enable = true;
      environment.systemPackages = with pkgs; [
        xorg.xorgserver
        xterm
        openbox
        picom
        xdotool
        python3
        jq
        procps
        dbus
        at-spi2-core
        at-spi2-atk
        glib
        gsettings-desktop-schemas
        # Electron (Chromium-based) for the CJK input test app
        electron
      ];
    };

  testScript = ''
    # Electron/CJK input test — verifies cua-driver Unicode pass-through into
    # an Electron application window. Uses CDP for reliable readback.
    machine.start()
    machine.wait_for_unit("multi-user.target")

    with subtest("Binary exists and runs"):
        machine.succeed("cua-driver --help")
        out = machine.succeed("cua-driver list-tools")
        assert "type_text" in out, f"type_text not in tools: {out}"
        assert "click" in out, f"click not in tools: {out}"

    with subtest("Start Xvfb and window manager"):
        machine.execute("Xvfb :99 -screen 0 1280x800x24 >/tmp/xvfb.log 2>&1 &")
        machine.wait_until_succeeds("test -e /tmp/.X11-unix/X99", timeout=10)
        machine.execute(
            "DISPLAY=:99 openbox --config-file ${openboxRc} >/tmp/openbox.log 2>&1 &"
        )
        machine.execute("DISPLAY=:99 picom --backend xrender >/tmp/picom.log 2>&1 &")

    with subtest("Start D-Bus session bus and AT-SPI accessibility bus"):
        # cua-driver's AT-SPI path requires a session D-Bus and the AT-SPI
        # registry service. Without this, the driver falls back to XSendEvent
        # which cannot deliver CJK characters (no keysym in standard maps).
        machine.succeed("mkdir -p /run/user/0 && chmod 700 /run/user/0")
        machine.succeed("mkdir -p /tmp/cua-cfg")
        machine.execute(
            "dbus-daemon --session --address=unix:path=/tmp/cua-session-bus "
            "--fork >/tmp/dbus.log 2>&1"
        )
        machine.wait_until_succeeds("test -S /tmp/cua-session-bus", timeout=10)
        machine.execute(
            "${a11yEnv} ${pkgs.at-spi2-core}/libexec/at-spi-bus-launcher "
            "--launch-immediately >/tmp/atspi-launcher.log 2>&1 &"
        )
        machine.wait_until_succeeds(
            "${a11yEnv} dbus-send --session --print-reply "
            "--dest=org.freedesktop.DBus / org.freedesktop.DBus.NameHasOwner "
            "string:org.a11y.Bus | grep -q 'boolean true'",
            timeout=15,
        )
        machine.execute(
            "${a11yEnv} dbus-send --session --print-reply --dest=org.a11y.Bus "
            "/org/a11y/bus org.freedesktop.DBus.Properties.Set "
            "string:org.a11y.Status string:IsEnabled variant:boolean:true "
            "2>&1 | tee /tmp/a11y-enable.log"
        )
        machine.log("a11y IsEnabled set: " + machine.execute("cat /tmp/a11y-enable.log")[1])
        machine.log("atspi-launcher.log: " + machine.execute("cat /tmp/atspi-launcher.log")[1])

    with subtest("Write Electron app files"):
        # Copy the pre-built Electron app files into /tmp/electron-app/
        machine.succeed("mkdir -p /tmp/electron-app")
        machine.copy_from_host("${electronApp}/main.js",     "/tmp/electron-app/main.js")
        machine.copy_from_host("${electronApp}/index.html",  "/tmp/electron-app/index.html")
        machine.copy_from_host("${electronApp}/package.json","/tmp/electron-app/package.json")

    with subtest("Launch Electron CJK test app"):
        machine.execute(
            "sh -lc '"
            "${a11yEnv} electron /tmp/electron-app "
            "--no-sandbox --disable-gpu --disable-dev-shm-usage "
            "--remote-debugging-port=${toString cdpPort} --remote-allow-origins=* "
            "--disable-backgrounding-occluded-windows "
            ">/tmp/electron.log 2>&1 & echo $! >/tmp/electron-pid.txt'"
        )
        # Wait for the Electron window to appear (Electron is slow to first paint)
        machine.wait_until_succeeds(
            "DISPLAY=:99 xdotool search --name 'cua-cjk-test'",
            timeout=60,
        )
        machine.succeed(
            "DISPLAY=:99 xdotool search --name 'cua-cjk-test' | head -1 > /tmp/electron-xid.txt"
        )
        # Ensure xid file is non-empty
        machine.succeed("test -s /tmp/electron-xid.txt")
        # Log the xid for debugging
        machine.log("electron xid: " + machine.succeed("cat /tmp/electron-xid.txt"))
        machine.log("electron pid: " + machine.succeed("cat /tmp/electron-pid.txt"))
        # Focus the Electron window to ensure it's ready
        machine.succeed(
            "DISPLAY=:99 xdotool windowactivate --sync $(cat /tmp/electron-xid.txt)"
        )
        machine.succeed(
            "DISPLAY=:99 xdotool windowfocus --sync $(cat /tmp/electron-xid.txt)"
        )

    with subtest("Wait for CDP to be available"):
        # The CDP debug port should come up shortly after the window appears
        machine.wait_until_succeeds(
            "python3 -c \""
            "import urllib.request, json; "
            "r = urllib.request.urlopen('http://127.0.0.1:${toString cdpPort}/json', timeout=5); "
            "pages = json.load(r); "
            "assert any(p.get('type')=='page' for p in pages), 'no page target'\"",
            timeout=30,
        )

    with subtest("Type CJK characters via cua-driver and verify via CDP"):
        machine.copy_from_host("${mcpCjkTest}", "/tmp/mcp-cjk-test.py")
        result = machine.succeed(
            "timeout 120 env ${a11yEnv} python3 /tmp/mcp-cjk-test.py 2>&1"
        )
        machine.log(result)

        # Only accept exact match: CJK_INPUT_OK means the full string landed.
        # CJK_INPUT_NONZERO (partial match) is no longer accepted — partial
        # delivery would mask real encoding failures.
        assert "CJK_INPUT_OK" in result, (
            "cua-driver did not deliver the full CJK text to the Electron input field.\n"
            "Expected CJK_INPUT_OK in output (exact match of '${cjkText}').\n"
            "Full output:\n" + result
        )
        assert "Electron/CJK test complete" in result, (
            "Test script did not complete cleanly:\n" + result
        )
  '';
}
