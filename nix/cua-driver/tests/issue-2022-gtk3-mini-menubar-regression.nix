# GTK3 menubar regression for issue #2022.
#
# Repro shape:
#   1. Prove a real XTest-backed pointer click (`xdotool click`) activates a
#      minimal GTK3 menubar item and writes a side-effect file.
#   2. Relaunch the same app in the background, keep a control xterm focused,
#      invoke `cua-driver click(...)` at the same window-local pixel, and expect
#      the same side effect.
#
# Current main reports the pixel click as successful but GTK3 ignores the
# synthetic XSendEvent ButtonPress/ButtonRelease, so the final assertion fails.
{
  pkgs,
  lib ? pkgs.lib,
  cuaDriverModule,
  ...
}:

let
  gtkMiniSource = pkgs.writeText "issue-2022-gtk3-mini-menubar.c" ''
    #include <gtk/gtk.h>
    #include <stdarg.h>
    #include <stdio.h>

    static const char *fired_path = NULL;
    static const char *coords_path = NULL;
    static const char *log_path = NULL;

    static void append_log(const char *fmt, ...) {
        if (log_path == NULL || *log_path == '\0') {
            return;
        }
        FILE *fp = fopen(log_path, "a");
        if (fp == NULL) {
            return;
        }
        va_list ap;
        va_start(ap, fmt);
        vfprintf(fp, fmt, ap);
        va_end(ap);
        fputc('\n', fp);
        fclose(fp);
    }

    static gboolean write_coords_once(gpointer data) {
        GtkWidget *item = GTK_WIDGET(data);
        GtkWidget *toplevel = gtk_widget_get_toplevel(item);
        GtkAllocation alloc;
        int tx = 0;
        int ty = 0;
        char buf[256];

        if (!gtk_widget_get_realized(item) || !gtk_widget_get_realized(toplevel)) {
            return G_SOURCE_CONTINUE;
        }

        gtk_widget_get_allocation(item, &alloc);
        if (!gtk_widget_translate_coordinates(item, toplevel, 0, 0, &tx, &ty)) {
            tx = alloc.x;
            ty = alloc.y;
        }

        g_snprintf(
            buf,
            sizeof(buf),
            "CENTER_X=%d\nCENTER_Y=%d\nWIDTH=%d\nHEIGHT=%d\n",
            tx + (alloc.width / 2),
            ty + (alloc.height / 2),
            alloc.width,
            alloc.height
        );
        g_file_set_contents(coords_path, buf, -1, NULL);
        append_log(
            "COORDS center_x=%d center_y=%d width=%d height=%d",
            tx + (alloc.width / 2),
            ty + (alloc.height / 2),
            alloc.width,
            alloc.height
        );
        return G_SOURCE_REMOVE;
    }

    static gboolean on_button_press(GtkWidget *widget, GdkEventButton *event, gpointer data) {
        append_log(
            "BUTTON_PRESS button=%u send_event=%d x=%.1f y=%.1f",
            event->button,
            event->send_event ? 1 : 0,
            event->x,
            event->y
        );
        return FALSE;
    }

    static gboolean on_button_release(GtkWidget *widget, GdkEventButton *event, gpointer data) {
        append_log(
            "BUTTON_RELEASE button=%u send_event=%d x=%.1f y=%.1f",
            event->button,
            event->send_event ? 1 : 0,
            event->x,
            event->y
        );
        return FALSE;
    }

    static void on_activate(GtkMenuItem *item, gpointer data) {
        append_log("ACTIVATE");
        if (fired_path != NULL && *fired_path != '\0') {
            g_file_set_contents(fired_path, "activated\n", -1, NULL);
        }
    }

    int main(int argc, char **argv) {
        GtkWidget *window;
        GtkWidget *box;
        GtkWidget *menubar;
        GtkWidget *trigger_item;
        GtkWidget *label;
        const char *title;

        gtk_init(&argc, &argv);

        fired_path = g_getenv("ISSUE2022_OUT");
        coords_path = g_getenv("ISSUE2022_COORDS");
        log_path = g_getenv("ISSUE2022_LOG");
        title = g_getenv("ISSUE2022_TITLE");

        window = gtk_window_new(GTK_WINDOW_TOPLEVEL);
        gtk_window_set_title(GTK_WINDOW(window), (title && *title) ? title : "Issue 2022 GTK3 Mini");
        gtk_window_set_default_size(GTK_WINDOW(window), 360, 220);
        g_signal_connect(window, "destroy", G_CALLBACK(gtk_main_quit), NULL);

        box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
        gtk_container_add(GTK_CONTAINER(window), box);

        menubar = gtk_menu_bar_new();
        trigger_item = gtk_menu_item_new_with_label("Trigger");
        gtk_widget_add_events(trigger_item, GDK_BUTTON_PRESS_MASK | GDK_BUTTON_RELEASE_MASK);
        g_signal_connect(trigger_item, "button-press-event", G_CALLBACK(on_button_press), NULL);
        g_signal_connect(trigger_item, "button-release-event", G_CALLBACK(on_button_release), NULL);
        g_signal_connect(trigger_item, "activate", G_CALLBACK(on_activate), NULL);
        gtk_menu_shell_append(GTK_MENU_SHELL(menubar), trigger_item);
        gtk_box_pack_start(GTK_BOX(box), menubar, FALSE, FALSE, 0);

        label = gtk_label_new("GTK3 menubar click regression for issue #2022");
        gtk_box_pack_start(GTK_BOX(box), label, TRUE, TRUE, 12);

        gtk_widget_show_all(window);
        append_log("READY");
        g_idle_add(write_coords_once, trigger_item);
        gtk_main();
        return 0;
    }
  '';

  gtkMiniApp = pkgs.stdenv.mkDerivation {
    pname = "issue-2022-gtk3-mini-menubar";
    version = "1";
    src = gtkMiniSource;
    dontUnpack = true;
    nativeBuildInputs = [ pkgs.pkg-config ];
    buildInputs = [ pkgs.gtk3 ];

    buildPhase = ''
      cp "$src" main.c
      $CC main.c -o issue-2022-gtk3-mini-menubar $(pkg-config --cflags --libs gtk+-3.0)
    '';

    installPhase = ''
      mkdir -p "$out/bin"
      cp issue-2022-gtk3-mini-menubar "$out/bin/"
    '';
  };

  mcpClickTest = pkgs.writeText "issue-2022-gtk3-mini-driver.py" ''
    import json
    import os
    import subprocess
    import sys
    import threading
    import time

    DRIVER_BIN = os.environ.get("CUA_DRIVER_BIN", "cua-driver")

    def read_shell_kv(path):
        data = {}
        with open(path, "r", encoding="utf-8") as handle:
            for line in handle:
                line = line.strip()
                if not line or "=" not in line:
                    continue
                key, value = line.split("=", 1)
                data[key] = int(value)
        return data

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

    def call_tool(proc, req_id, name, arguments):
        send(proc, "tools/call", {"name": name, "arguments": arguments}, req_id=req_id)
        resp = recv(proc, timeout=45)
        if "error" in resp and resp["error"] is not None:
            raise RuntimeError(f"{name} failed: {resp}")
        if resp.get("result", {}).get("isError"):
            raise RuntimeError(f"{name} returned isError: {resp}")
        return resp

    def main():
        coords = read_shell_kv("/tmp/issue2022-driver.coords")
        with open("/tmp/issue2022-driver-xid.txt", "r", encoding="utf-8") as handle:
            window_id = int(handle.read().strip())
        with open("/tmp/issue2022-driver.pid", "r", encoding="utf-8") as handle:
            pid = int(handle.read().strip())

        proc = start_driver()
        try:
            send(proc, "initialize", {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "issue-2022-gtk3-mini-test", "version": "1.0.0"},
            }, req_id=1)
            recv(proc)
            send(proc, "notifications/initialized", {})
            time.sleep(0.3)

            resp = call_tool(proc, 2, "click", {
                "pid": pid,
                "window_id": window_id,
                "x": float(coords["CENTER_X"]),
                "y": float(coords["CENTER_Y"]),
            })
            print(json.dumps(resp), flush=True)
            time.sleep(1.0)
            print("driver click complete", flush=True)
        finally:
            proc.stdin.close()
            proc.terminate()
            proc.wait(timeout=5)

    if __name__ == "__main__":
        main()
  '';

  openboxRc = import ./openbox-rc.nix { inherit pkgs; };
in

pkgs.testers.nixosTest {
  name = "issue-2022-gtk3-mini-menubar-regression";
  meta.maintainers = [ ];

  containers.machine =
    {
      pkgs,
      ...
    }:
    {
      imports = [ cuaDriverModule ];
      services.cua-driver.enable = true;
      environment.systemPackages = with pkgs; [
        xorg.xorgserver
        xterm
        openbox
        picom
        xdotool
        python3
        jq
        procps
        gtkMiniApp
      ];
    };

  testScript = ''
    def launch_mini(tag):
        machine.execute(
            "rm -f /tmp/{tag}-fired /tmp/{tag}.coords /tmp/{tag}.log /tmp/{tag}.stdout "
            "/tmp/{tag}.pid /tmp/{tag}-xid.txt".format(tag=tag)
        )
        machine.execute(
            "sh -lc 'DISPLAY=:99 "
            "ISSUE2022_OUT=/tmp/{tag}-fired "
            "ISSUE2022_COORDS=/tmp/{tag}.coords "
            "ISSUE2022_LOG=/tmp/{tag}.log "
            "ISSUE2022_TITLE=\"{title}\" "
            "${gtkMiniApp}/bin/issue-2022-gtk3-mini-menubar "
            ">/tmp/{tag}.stdout 2>&1 & echo $! >/tmp/{tag}.pid'".format(
                tag=tag,
                title="Issue 2022 " + tag,
            )
        )
        machine.wait_until_succeeds(
            "sh -lc 'DISPLAY=:99 xdotool search --sync --pid $(cat /tmp/{tag}.pid) | head -1 > /tmp/{tag}-xid.txt'".format(tag=tag),
            timeout=20,
        )
        machine.wait_until_succeeds("test -s /tmp/{tag}.coords".format(tag=tag), timeout=20)

    machine.start()
    machine.wait_for_unit("multi-user.target")

    with subtest("Start X11 desktop and focused control terminal"):
        machine.execute("Xvfb :99 -screen 0 1280x1024x24 >/tmp/xvfb.log 2>&1 &")
        machine.wait_until_succeeds("test -e /tmp/.X11-unix/X99", timeout=10)
        machine.execute("DISPLAY=:99 openbox --config-file ${openboxRc} >/tmp/openbox.log 2>&1 &")
        machine.execute("DISPLAY=:99 picom --backend xrender >/tmp/picom.log 2>&1 &")
        machine.execute(
            "sh -lc \"DISPLAY=:99 xterm -T 'Issue 2022 Control' -fa Monospace -fs 14 "
            "-geometry 70x24+720+120 >/tmp/issue2022-control.log 2>&1 & "
            "echo \\$! >/tmp/issue2022-control.pid\""
        )
        machine.wait_until_succeeds(
            "sh -lc 'DISPLAY=:99 xdotool search --sync --pid $(cat /tmp/issue2022-control.pid) | head -1 > /tmp/issue2022-control-xid.txt'",
            timeout=20,
        )
        machine.succeed("DISPLAY=:99 xdotool windowactivate --sync $(cat /tmp/issue2022-control-xid.txt)")
        machine.succeed("DISPLAY=:99 xdotool windowfocus --sync $(cat /tmp/issue2022-control-xid.txt)")

    with subtest("Sanity check: a real pointer click activates the GTK3 menubar item"):
        launch_mini("issue2022-real")
        machine.succeed(
            "sh -lc '. /tmp/issue2022-real.coords; "
            "DISPLAY=:99 xdotool windowactivate --sync $(cat /tmp/issue2022-real-xid.txt); "
            "DISPLAY=:99 xdotool mousemove --sync --window $(cat /tmp/issue2022-real-xid.txt) "
            "$CENTER_X $CENTER_Y; "
            "DISPLAY=:99 xdotool click 1'"
        )
        machine.wait_until_succeeds("test -f /tmp/issue2022-real-fired", timeout=10)
        real_log = machine.succeed("cat /tmp/issue2022-real.log")
        machine.log(real_log)
        assert "BUTTON_PRESS button=1 send_event=0" in real_log, real_log
        assert "ACTIVATE" in real_log, real_log
        machine.execute("kill $(cat /tmp/issue2022-real.pid)")
        machine.wait_until_succeeds("! kill -0 $(cat /tmp/issue2022-real.pid) 2>/dev/null", timeout=10)
        machine.succeed("DISPLAY=:99 xdotool windowactivate --sync $(cat /tmp/issue2022-control-xid.txt)")
        machine.succeed("DISPLAY=:99 xdotool windowfocus --sync $(cat /tmp/issue2022-control-xid.txt)")

    with subtest("Regression: cua-driver reports a click but GTK3 menubar ignores it"):
        launch_mini("issue2022-driver")
        machine.succeed("DISPLAY=:99 xdotool windowactivate --sync $(cat /tmp/issue2022-control-xid.txt)")
        machine.succeed("DISPLAY=:99 xdotool windowfocus --sync $(cat /tmp/issue2022-control-xid.txt)")
        machine.succeed(
            "sh -lc 'test \"$(DISPLAY=:99 xdotool getwindowfocus)\" = \"$(cat /tmp/issue2022-control-xid.txt)\"'"
        )
        machine.copy_from_host("${mcpClickTest}", "/tmp/issue-2022-gtk3-mini-driver.py")
        result = machine.succeed("timeout 60 env DISPLAY=:99 python3 /tmp/issue-2022-gtk3-mini-driver.py 2>&1")
        machine.log(result)
        assert "driver click complete" in result, result
        machine.log(machine.succeed("sh -lc 'cat /tmp/issue2022-driver.log || true'"))
        machine.succeed(
            "sh -lc 'test \"$(DISPLAY=:99 xdotool getwindowfocus)\" = \"$(cat /tmp/issue2022-control-xid.txt)\"'"
        )
        machine.succeed("test -f /tmp/issue2022-driver-fired")
  '';
}
