# CUA Driver set_value EditableText Test
#
# Regression test for #1924 (fixed in PR #1929): `set_value` on a GTK4 editable
# text box failed with "element N exposes neither EditableText nor Value". GTK4
# only advertises the AT-SPI EditableText interface on a widget once it holds
# keyboard focus, so the interface list captured during the unfocused tree walk
# was missing it. The fix GrabFocus-es the target, then resolves EditableText
# live over D-Bus and writes via EditableText::SetTextContents (falling back to
# InsertText, then Value). This test proves that exact path end-to-end:
#
#   1. boot a NixOS VM with a session D-Bus + AT-SPI bus (same scaffolding as
#      linux-background-gui.nix),
#   2. launch a GTK4 app exposing a real editable text field,
#   3. over MCP stdio: get_window_state to locate the editable element_index,
#      assert the field does NOT already contain the test string (baseline),
#      set_value to write the test string, then get_window_state again and
#      assert the field now DOES contain it.
#
# GTK4 choice / why a self-contained app:
# #1924 is GTK4-SPECIFIC (the focus-gated EditableText advertisement is a GTK4
# behaviour), so this test must drive a GTK4 editable — a GTK3 fallback would
# not exercise the regression. The real GNOME GTK4 apps named in the issue
# (gnome-text-editor) were dropped from the linux-background-gui matrix because
# they never mapped a window within 120s headless in CI (missing portals / EDS
# / VTE runtime). So, exactly like the `tk` and `chromium` targets in
# linux-background-gui.nix, this test ships its OWN minimal GTK4 app: a single
# GtkApplicationWindow holding one GtkEntry (a genuine GTK4 editable that
# exposes Text/EditableText), with no portal/EDS/VTE dependencies, so it maps
# reliably headless. The GtkEntry starts empty, guaranteeing the baseline
# "field is empty" assert holds and a pass can only mean the write took.
#
# To run: nix build .#checks.x86_64-linux.cua-driver-set-value
{
  pkgs,
  lib ? pkgs.lib,
  cuaDriverModule,
  ...
}:

let
  # The known string we write via set_value. Distinctive so a tree-markdown
  # substring match can't collide with any default widget label/name.
  testValue = "cuasetvalue9271";

  # Shared session-bus + a11y environment, identical in spirit to
  # linux-background-gui.nix: the GTK4 app and the driver's native AT-SPI client
  # must reach the same registry. Fixed bus path so every machine.* shell opts in.
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
    # GTK4 talks AT-SPI directly (not via atk-bridge), but only exports its
    # accessible tree when it selects the AT-SPI backend at startup; GTK_A11Y=atspi
    # forces it on. x11 backend + cairo renderer keep it headless-safe. (Same
    # exports linux-background-gui.nix uses for its GTK4 entries.)
    "GTK_A11Y=atspi"
    "GDK_BACKEND=x11"
    "GSK_RENDERER=cairo"
  ];

  # Minimal self-contained GTK4 app: one window, one GtkEntry, autofocused. The
  # GtkEntry is a real GTK4 editable that exposes the AT-SPI Text/EditableText
  # interfaces — exactly the widget class #1924 failed on. No portal/EDS/VTE, so
  # it maps reliably headless. Fixed window title so xdotool can find it.
  gtk4Env = pkgs.python3.withPackages (ps: [ ps.pygobject3 ]);

  # PyGObject loads GTK4 via GObject-introspection typelibs at runtime, so the
  # GTK-4.0 (and its dependency) typelibs must be on GI_TYPELIB_PATH. The
  # `withPackages` env only carries PyGObject itself, not the GTK4 typelib, so
  # point GI at gtk4's girepository dir explicitly before launching the app.
  giTypelibEnv = "GI_TYPELIB_PATH=${pkgs.gtk4}/lib/girepository-1.0:${pkgs.glib}/lib/girepository-1.0:${pkgs.graphene}/lib/girepository-1.0:${pkgs.pango.out}/lib/girepository-1.0:${pkgs.gdk-pixbuf}/lib/girepository-1.0:${pkgs.harfbuzz.out}/lib/girepository-1.0";
  gtk4App = pkgs.writeText "cua-gtk4-entry.py" ''
    import gi
    gi.require_version("Gtk", "4.0")
    from gi.repository import Gtk, GLib

    def on_activate(app):
        win = Gtk.ApplicationWindow(application=app)
        win.set_title("cua-setvalue")
        win.set_default_size(400, 120)
        entry = Gtk.Entry()
        entry.set_hexpand(True)
        # Start EMPTY so the baseline "field does not contain the test string"
        # assertion is guaranteed to hold before the write.
        entry.set_text("")
        win.set_child(entry)
        win.present()
        entry.grab_focus()

    app = Gtk.Application(application_id="com.trycua.SetValueEntry")
    app.connect("activate", on_activate)
    app.run(None)
  '';

  # Python MCP client that drives cua-driver over stdio against the live GTK4
  # window: find the editable element_index, baseline-read, set_value, read back.
  setValueTest = pkgs.writeText "set-value-test.py" ''
    import json, os, sys, threading, time

    DRIVER_BIN = os.environ.get("CUA_DRIVER_BIN", "cua-driver")
    TEST_VALUE = "${testValue}"

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
        line = json.dumps(msg) + "\n"
        print(f"[send] {line.strip()}", flush=True)
        proc.stdin.write(line.encode()); proc.stdin.flush()

    def recv(proc, timeout=60):
        result = [None]
        def reader():
            result[0] = proc.stdout.readline()
        t = threading.Thread(target=reader); t.start(); t.join(timeout)
        if t.is_alive():
            raise TimeoutError("No response within timeout")
        line = result[0].decode().strip()
        if not line:
            raise RuntimeError("Driver returned an empty response")
        print(f"[recv] {line[:400]}", flush=True)
        return json.loads(line)

    def call_tool(proc, req_id, name, arguments):
        send(proc, "tools/call", {"name": name, "arguments": arguments}, req_id=req_id)
        resp = recv(proc)
        if resp.get("error"):
            raise RuntimeError(f"{name} failed: {resp}")
        if resp.get("result", {}).get("isError"):
            raise RuntimeError(f"{name} returned isError: {resp}")
        return resp["result"]

    def tree_text(result):
        # get_window_state returns the AT-SPI tree as Markdown text content; the
        # driver surfaces an editable widget's Text content as the node's display
        # name, so the written value shows up in the tree as `[idx] ... "VALUE"`.
        parts = [c.get("text", "") for c in result.get("content", []) if c.get("type") == "text"]
        sc = result.get("structuredContent", {}) or {}
        if isinstance(sc.get("tree_markdown"), str):
            parts.append(sc["tree_markdown"])
        return "\n".join(parts)

    def find_editable_index(result):
        # Pick the editable element. The tree markdown lines for actionable
        # elements look like `- [N] entry "..." [...] [actions=[...]]`. A GtkEntry
        # renders with role "entry"/"text". Prefer an entry/text line; fall back
        # to the first indexed element if the role name differs across builds.
        text = tree_text(result)
        entry_idx = None
        first_idx = None
        for line in text.splitlines():
            s = line.strip()
            if not s.startswith("- ["):
                continue
            try:
                idx = int(s[s.index("[") + 1 : s.index("]")])
            except Exception:
                continue
            if first_idx is None:
                first_idx = idx
            low = s.lower()
            if ("entry" in low or "text" in low) and entry_idx is None:
                entry_idx = idx
        return entry_idx if entry_idx is not None else first_idx

    def get_window_state(proc, req_id, pid, xid):
        # ax = tree only (no screenshot needed; faster + avoids capture flakiness).
        return call_tool(proc, req_id, "get_window_state", {
            "pid": pid, "window_id": xid, "capture_mode": "ax",
        })

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
                "clientInfo": {"name": "nixos-set-value-test", "version": "1.0.0"},
            }, req_id=1)
            recv(proc)
            send(proc, "notifications/initialized", {})
            time.sleep(0.5)

            # ── Locate the editable element. Retry: GTK4 builds its accessible
            # tree a moment after first paint. ──────────────────────────────────
            print("\n--- get_window_state (locate editable) ---", flush=True)
            idx = None
            before_state = None
            for _ in range(10):
                before_state = get_window_state(proc, 2, target_pid, target_xid)
                idx = find_editable_index(before_state)
                if idx is not None:
                    break
                time.sleep(1.0)
            assert idx is not None, (
                "no editable element found in GTK4 window:\n" + tree_text(before_state or {})
            )
            print(f"editable element_index = {idx}", flush=True)

            # ── Baseline: the field must NOT already contain the test string, so
            # a passing read-back can only mean the write took effect. ───────────
            before_text = tree_text(before_state)
            assert TEST_VALUE not in before_text, (
                f"baseline already contains '{TEST_VALUE}'; cannot prove the write "
                f"took:\n{before_text}"
            )
            print("baseline OK: field does not contain the test string", flush=True)

            # ── The regression: set_value into the GTK4 editable. Pre-fix this
            # errored with "neither EditableText nor Value"; the fix writes via
            # EditableText::SetTextContents. ────────────────────────────────────
            print("\n--- set_value ---", flush=True)
            sv = call_tool(proc, 3, "set_value", {
                "pid": target_pid,
                "window_id": target_xid,
                "element_index": idx,
                "value": TEST_VALUE,
            })
            print("set_value result: " + json.dumps(sv)[:400], flush=True)

            # ── Read back: get_window_state must now show the test string in the
            # editable's Text content. Retry — the write + tree rebuild settle a
            # moment after SetTextContents. ─────────────────────────────────────
            print("\n--- get_window_state (read back) ---", flush=True)
            after_text = ""
            for _ in range(10):
                after_state = get_window_state(proc, 4, target_pid, target_xid)
                after_text = tree_text(after_state)
                if TEST_VALUE in after_text:
                    break
                time.sleep(1.0)
            print("READBACK_BEGIN", flush=True)
            print(after_text, flush=True)
            print("READBACK_END", flush=True)
            assert TEST_VALUE in after_text, (
                f"set_value did not write '{TEST_VALUE}' into the GTK4 editable via "
                f"EditableText; field still reads:\n{after_text}"
            )
            print("\n=== set_value EditableText test passed! ===", flush=True)
        finally:
            proc.stdin.close(); proc.terminate(); proc.wait(timeout=5)

    if __name__ == "__main__":
        main()
  '';

in

pkgs.testers.nixosTest {
  name = "cua-driver-set-value-test";
  meta.maintainers = [ ];

  nodes.machine =
    { pkgs, ... }:
    {
      imports = [ cuaDriverModule ];
      virtualisation = {
        cores = 2;
        memorySize = 2048;
        diskSize = 8192;
      };
      services.cua-driver.enable = true;
      services.dbus.enable = true;
      environment.systemPackages = with pkgs; [
        xorg.xorgserver
        xterm
        openbox
        xdotool
        dbus
        at-spi2-core
        python3
        gtk4Env # PyGObject env for the self-contained editable app
        gtk4 # GTK-4.0 typelib + libs (loaded by PyGObject at runtime)
        gobject-introspection
        glib # `gsettings`
        gsettings-desktop-schemas
        procps
      ];
    };

  testScript = ''
    machine.start()
    machine.wait_for_unit("multi-user.target")

    with subtest("Binary exists and runs"):
        machine.succeed("cua-driver --help")

    with subtest("Start X11 + session D-Bus + AT-SPI bus"):
        machine.execute("Xvfb :99 -screen 0 1280x1024x24 >/tmp/xvfb.log 2>&1 &")
        machine.wait_until_succeeds("test -e /tmp/.X11-unix/X99", timeout=10)
        machine.execute("DISPLAY=:99 openbox >/tmp/openbox.log 2>&1 &")
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
        machine.log("atspi-launcher.log: " + machine.execute("cat /tmp/atspi-launcher.log")[1])

    with subtest("Launch the self-contained GTK4 editable app"):
        machine.execute("sh -lc '${a11yEnv} ${giTypelibEnv} ${gtk4Env}/bin/python3 ${gtk4App} >/tmp/target.log 2>&1 & echo $! >/tmp/target-pid.txt'")
        machine.sleep(5)
        machine.log("target.log after launch: " + machine.execute("cat /tmp/target.log")[1])
        # Find the GTK4 window by its fixed title; fall back to the launched PID's
        # window, then the newest visible window.
        machine.wait_until_succeeds(
            "sh -lc 'export DISPLAY=:99; "
            "xid=$(${pkgs.xdotool}/bin/xdotool search --sync --onlyvisible --name cua-setvalue 2>/dev/null | head -1); "
            "[ -z \"$xid\" ] && xid=$(${pkgs.xdotool}/bin/xdotool search --all --pid $(cat /tmp/target-pid.txt) 2>/dev/null | head -1); "
            "[ -z \"$xid\" ] && xid=$(${pkgs.xdotool}/bin/xdotool search --onlyvisible \"\" 2>/dev/null | tail -1); "
            "test -n \"$xid\" && printf %s \"$xid\" >/tmp/target-xid.txt && test -s /tmp/target-xid.txt'",
            timeout=60,
        )
        machine.log("target-xid: " + machine.execute("cat /tmp/target-xid.txt")[1])

    with subtest("set_value writes the test string into the GTK4 editable via EditableText"):
        machine.copy_from_host("${setValueTest}", "/tmp/set-value-test.py")
        result = machine.succeed(
            "${a11yEnv} timeout 180 python3 /tmp/set-value-test.py 2>&1"
        )
        machine.log(result)
        assert "set_value EditableText test passed" in result, f"set_value test failed: {result}"
  '';
}
