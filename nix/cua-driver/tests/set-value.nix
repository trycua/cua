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
#   2. launch a packaged GTK4 app exposing a real editable text field,
#   3. over MCP stdio: get_window_state to locate the editable element_index,
#      assert the field does NOT already contain the test string (baseline),
#      set_value to write the test string, then get_window_state again and
#      assert the field now DOES contain it.
#
# GTK4 choice — why gnome-characters:
# #1924 is GTK4-SPECIFIC (the focus-gated EditableText advertisement is a GTK4
# behaviour), so this test must drive a GTK4 editable — a GTK3 fallback would
# not exercise the regression. We drive gnome-characters because it is the ONE
# GTK4 app the linux-background-gui matrix found reliably surfaces a window
# under headless Xvfb (the other GNOME GTK4 apps, gnome-text-editor included,
# launch but never map an X11 window without a real compositor/portal, so the
# window-find times out). gnome-characters is GTK4 and its search field is a
# GtkText/GtkSearchEntry — a real GTK4 editable that advertises the AT-SPI
# EditableText interface once focused, which is exactly the surface #1924's fix
# touches (GrabFocus, then EditableText::SetTextContents). It is a packaged Nix
# app, so its wrapper wires up GTK4 + all typelibs (unlike a hand-rolled
# PyGObject script, which needs the full GObject-introspection typelib set on
# GI_TYPELIB_PATH). The search field starts empty, so the baseline "field is
# empty" assert holds and a pass can only mean the write took.
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
  # The GTK4-specific exports (GTK_A11Y=atspi, x11 backend, cairo renderer) are
  # the same ones linux-background-gui.nix uses for its GTK4 entries.
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
    "GTK_A11Y=atspi"
    "GDK_BACKEND=x11"
    "GSK_RENDERER=cairo"
  ];

  # Packaged GTK4 app whose search field is an editable GtkText/GtkSearchEntry
  # (the GTK4 editable surface #1924 is about). The one GTK4 app proven to map a
  # window under headless Xvfb — see the rationale above.
  gtk4App = pkgs.gnome-characters;
  gtk4Launch = "${gtk4App}/bin/gnome-characters";
  # Its WM_CLASS for the xdotool window search (same match the passing
  # gtk4-characters entry in linux-background-gui.nix uses).
  gtk4WindowMatch = "--class org.gnome.Characters";

  # Python MCP client that drives cua-driver over stdio against the live GTK4
  # window: find the editable element_index, baseline-read, set_value, read back.
  # Same MCP-stdio structure as set-config.nix.
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

    # Roles that denote an actually-editable text widget. STRICT: we only ever
    # accept one of these. We must NOT fall back to a window/group/label/panel —
    # set_value on a non-editable (e.g. the window at index 0) correctly fails
    # with "neither EditableText nor Value", which would masquerade as the #1929
    # fix being broken when really we just picked the wrong element.
    EDITABLE_ROLES = ("entry", "text box", "textbox", "password text", "editable")

    def find_editable_index(result):
        # Indexed actionable lines look like `- [N] <role> "..." [...]
        # [actions=[...]]`. A GtkText/GtkSearchEntry renders with role "entry"
        # or "text"; match the role token that sits right after `- [N] `. Returns
        # None (never a window/group fallback) if no editable role is present.
        text = tree_text(result)
        for line in text.splitlines():
            s = line.strip()
            if not s.startswith("- ["):
                continue
            try:
                idx = int(s[s.index("[") + 1 : s.index("]")])
            except Exception:
                continue
            # The role is the text between the closing `]` of the index and the
            # first double-quote of the name.
            after_idx = s[s.index("]") + 1 :].lstrip()
            role = after_idx.split('"', 1)[0].strip().lower()
            # role "text" (bare) is GtkText/GtkTextView's AT-SPI role; also accept
            # the explicit editable roles. Reject "static text"/"label"/etc.
            if role == "text" or role in EDITABLE_ROLES:
                return idx
        return None

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

            # ── Locate the editable element. gnome-characters' GtkSearchEntry is
            # only realized (and thus only appears in the AT-SPI tree) once the
            # search is opened, so the helper testScript opens search via xdotool
            # BEFORE this client runs. Retry: GTK4 builds its accessible tree a
            # moment after first paint / after the search bar slides in. ─────────
            print("\n--- get_window_state (locate editable) ---", flush=True)
            idx = None
            before_state = None
            for _ in range(20):
                before_state = get_window_state(proc, 2, target_pid, target_xid)
                idx = find_editable_index(before_state)
                if idx is not None:
                    break
                time.sleep(1.0)
            if idx is None:
                # Dump the full tree so a "no editable" failure is diagnosable: it
                # tells us whether the search entry realized at all.
                print("NO_EDITABLE_TREE_BEGIN", flush=True)
                print(tree_text(before_state or {}), flush=True)
                print("NO_EDITABLE_TREE_END", flush=True)
            assert idx is not None, (
                "no editable (entry/text) element found in the GTK4 window — the "
                "search entry may not have realized. Tree dumped above."
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
            for _ in range(15):
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

  # Window-find: match the GTK4 app's class, then fall back to the launched PID's
  # window, then the newest visible window. Store-path script (one safe token for
  # the Python testScript string), same idiom as linux-background-gui.nix.
  windowFindCmd = pkgs.writeShellScript "cua-setvalue-window-find.sh" ''
    export DISPLAY=:99
    # No --sync: a non-matching class makes xdotool block until killed, which
    # would swallow the whole wait_until_succeeds budget before the fallbacks
    # below ever run. wait_until_succeeds re-runs this script to poll instead.
    xid=$(${pkgs.xdotool}/bin/xdotool search --onlyvisible ${gtk4WindowMatch} 2>/dev/null | head -1)
    if [ -z "$xid" ]; then
      xid=$(${pkgs.xdotool}/bin/xdotool search --all --pid "$(cat /tmp/target-pid.txt)" 2>/dev/null | head -1)
    fi
    if [ -z "$xid" ]; then
      xid=$(${pkgs.xdotool}/bin/xdotool search --onlyvisible "" 2>/dev/null | tail -1)
    fi
    test -n "$xid" && printf "%s" "$xid" >/tmp/target-xid.txt && test -s /tmp/target-xid.txt
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
        memorySize = 4096;
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
        gtk4App # gnome-characters (GTK4 editable search field) — properly packaged
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

    with subtest("Launch the GTK4 editable app (gnome-characters)"):
        machine.execute("sh -lc '${a11yEnv} ${gtk4Launch} >/tmp/target.log 2>&1 & echo $! >/tmp/target-pid.txt'")
        # Surface the app's own stdout/stderr early so a launch failure is visible
        # instead of just a window-find timeout.
        machine.sleep(5)
        machine.log("target.log after launch: " + machine.execute("cat /tmp/target.log")[1])
        machine.wait_until_succeeds("${windowFindCmd}", timeout=120)
        machine.log("target-xid: " + machine.execute("cat /tmp/target-xid.txt")[1])
        # Resolve the PID that actually owns the mapped window (gnome-characters
        # is single-instance: the launched shell may differ from the GTK process),
        # and persist it for the driver.
        machine.execute(
            "sh -lc 'export DISPLAY=:99; "
            "wpid=$(${pkgs.xdotool}/bin/xdotool getwindowpid $(cat /tmp/target-xid.txt) 2>/dev/null); "
            "[ -n \"$wpid\" ] && echo $wpid >/tmp/target-pid.txt; true'"
        )
        machine.log("target-pid: " + machine.execute("cat /tmp/target-pid.txt")[1])
        # Make sure the editor window holds focus so GTK4 advertises EditableText
        # on its text view (the fix also GrabFocus-es, but this gives the tree a
        # focused editable from the start).
        machine.succeed("DISPLAY=:99 xdotool windowactivate --sync $(cat /tmp/target-xid.txt)")

    with subtest("Realize the GtkSearchEntry so it appears in the AT-SPI tree"):
        # gnome-characters' search field is a GtkSearchEntry that is NOT realized
        # (and so is absent from the default AT-SPI tree) until search is opened.
        # Open it via the GTK "find" accelerator and a typed key (Characters also
        # supports type-to-search), then give the bar a moment to slide in and
        # register its accessible. Without this the only indexed element is the
        # window itself — set_value on which correctly fails, masking the real fix.
        xid = machine.succeed("cat /tmp/target-xid.txt").strip()
        machine.succeed(f"DISPLAY=:99 xdotool windowactivate --sync {xid}")
        machine.execute(f"DISPLAY=:99 xdotool key --clearmodifiers --window {xid} ctrl+f")
        machine.sleep(1)
        # Type-to-search fallback: a plain letter also pops the search bar.
        machine.execute(f"DISPLAY=:99 xdotool key --clearmodifiers --window {xid} a")
        machine.sleep(2)

    with subtest("set_value writes the test string into the GTK4 editable via EditableText"):
        machine.copy_from_host("${setValueTest}", "/tmp/set-value-test.py")
        result = machine.succeed(
            "${a11yEnv} timeout 240 python3 /tmp/set-value-test.py 2>&1"
        )
        machine.log(result)
        assert "set_value EditableText test passed" in result, f"set_value test failed: {result}"
  '';
}
