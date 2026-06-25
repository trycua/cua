#
# Regression repro for trycua/cua issue #2022 (SDL/Allegro side).
#
# Runs an SDL-on-X11 app under Xvfb + Openbox with a separate control xterm
# focused, then calls cua-driver click against the SDL window. The click tool is
# expected to report success, but current main still routes through XSendEvent,
# so SDL ignores the synthetic ButtonPress/Release because send_event=True and
# the app never records SDL_MOUSEBUTTONDOWN.
#
# This test is intentionally RED on current main.
#
# Example direct run without adding a flake check:
#   nix build --impure --expr '
#     let
#       flake = builtins.getFlake (toString ./.);
#       pkgs = flake.inputs.nixpkgs.legacyPackages.x86_64-linux;
#     in
#     import ./nix/cua-driver/tests/linux-sdl-send-event-click-regression-issue-2022.nix {
#       inherit pkgs;
#       inherit (pkgs) lib;
#       cuaDriverModule = {
#         imports = [ ./nix/cua-driver/module.nix ];
#         services.cua-driver.package = flake.packages.x86_64-linux.cua-driver;
#       };
#     }'
#
{
  pkgs,
  lib ? pkgs.lib,
  cuaDriverModule,
  ...
}:

let
  sdlClickAppSrc = pkgs.writeText "issue-2022-sdl-click-app.c" ''
    #include <SDL.h>

    #include <stdarg.h>
    #include <stdbool.h>
    #include <stdio.h>

    static void append_log(const char *fmt, ...) {
      FILE *log = fopen("/tmp/issue-2022-sdl-events.log", "a");
      if (log == NULL) {
        return;
      }

      va_list args;
      va_start(args, fmt);
      vfprintf(log, fmt, args);
      va_end(args);
      fputc('\n', log);
      fclose(log);
    }

    static void write_marker(const char *path, const char *value) {
      FILE *out = fopen(path, "w");
      if (out == NULL) {
        return;
      }
      fputs(value, out);
      fclose(out);
    }

    int main(void) {
      if (SDL_Init(SDL_INIT_VIDEO) != 0) {
        fprintf(stderr, "SDL_Init failed: %s\n", SDL_GetError());
        return 1;
      }

      SDL_Window *window = SDL_CreateWindow(
        "Issue 2022 SDL Click Repro",
        SDL_WINDOWPOS_CENTERED,
        SDL_WINDOWPOS_CENTERED,
        480,
        320,
        SDL_WINDOW_SHOWN
      );
      if (window == NULL) {
        fprintf(stderr, "SDL_CreateWindow failed: %s\n", SDL_GetError());
        SDL_Quit();
        return 1;
      }

      SDL_Renderer *renderer = SDL_CreateRenderer(window, -1, SDL_RENDERER_SOFTWARE);
      if (renderer == NULL) {
        fprintf(stderr, "SDL_CreateRenderer failed: %s\n", SDL_GetError());
        SDL_DestroyWindow(window);
        SDL_Quit();
        return 1;
      }

      append_log("window_created");
      write_marker("/tmp/issue-2022-sdl-window-ready.txt", "ready\n");

      bool saw_click = false;
      const Uint32 start = SDL_GetTicks();

      while ((SDL_GetTicks() - start) < 30000U) {
        SDL_Event event;
        while (SDL_PollEvent(&event)) {
          switch (event.type) {
            case SDL_QUIT:
              append_log("quit");
              break;
            case SDL_WINDOWEVENT:
              append_log("window_event=%u", (unsigned int) event.window.event);
              break;
            case SDL_MOUSEMOTION:
              append_log(
                "mouse_motion x=%d y=%d state=%u",
                event.motion.x,
                event.motion.y,
                (unsigned int) event.motion.state
              );
              break;
            case SDL_MOUSEBUTTONDOWN:
              append_log(
                "mouse_button_down button=%u x=%d y=%d clicks=%u",
                (unsigned int) event.button.button,
                event.button.x,
                event.button.y,
                (unsigned int) event.button.clicks
              );
              write_marker("/tmp/issue-2022-sdl-click-received.txt", "received\n");
              saw_click = true;
              break;
            case SDL_MOUSEBUTTONUP:
              append_log(
                "mouse_button_up button=%u x=%d y=%d clicks=%u",
                (unsigned int) event.button.button,
                event.button.x,
                event.button.y,
                (unsigned int) event.button.clicks
              );
              break;
            default:
              break;
          }
        }

        if (saw_click) {
          SDL_SetRenderDrawColor(renderer, 0x00, 0x88, 0x33, 0xff);
        } else {
          SDL_SetRenderDrawColor(renderer, 0x99, 0x11, 0x11, 0xff);
        }
        SDL_RenderClear(renderer);

        SDL_Rect button = { 120, 90, 240, 140 };
        if (saw_click) {
          SDL_SetRenderDrawColor(renderer, 0xdd, 0xff, 0xdd, 0xff);
        } else {
          SDL_SetRenderDrawColor(renderer, 0xff, 0xdd, 0xdd, 0xff);
        }
        SDL_RenderFillRect(renderer, &button);
        SDL_RenderPresent(renderer);

        if (saw_click) {
          SDL_Delay(500);
          break;
        }
        SDL_Delay(10);
      }

      if (!saw_click) {
        append_log("timed_out_without_mouse_button_down");
      }

      SDL_DestroyRenderer(renderer);
      SDL_DestroyWindow(window);
      SDL_Quit();
      return 0;
    }
  '';

  sdlClickApp = pkgs.stdenv.mkDerivation {
    pname = "issue-2022-sdl-click-app";
    version = "0";
    dontUnpack = true;
    nativeBuildInputs = [
      pkgs.pkg-config
    ];
    buildInputs = [
      pkgs.SDL2
    ];
    buildPhase = ''
      cp ${sdlClickAppSrc} main.c
      $CC -O2 -Wall -Wextra main.c -o issue-2022-sdl-click-app $(pkg-config --cflags --libs sdl2)
    '';
    installPhase = ''
      mkdir -p $out/bin
      cp issue-2022-sdl-click-app $out/bin/
    '';
  };

  mcpSdlClickTest = pkgs.writeText "mcp-issue-2022-sdl-click.py" ''
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

    def call_tool(proc, req_id, name, arguments):
        send(proc, "tools/call", {"name": name, "arguments": arguments}, req_id=req_id)
        resp = recv(proc, timeout=45)
        if "error" in resp and resp["error"] is not None:
            raise RuntimeError(f"{name} failed: {resp}")
        if resp.get("result", {}).get("isError"):
            raise RuntimeError(f"{name} returned isError: {resp}")
        return resp

    def main():
        with open("/tmp/issue-2022-sdl-xid.txt", "r", encoding="utf-8") as f:
            window_id = int(f.read().strip())
        with open("/tmp/issue-2022-sdl-pid.txt", "r", encoding="utf-8") as f:
            pid = int(f.read().strip())

        proc = start_driver()
        try:
            send(proc, "initialize", {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "nixos-issue-2022-sdl-click-test", "version": "1.0.0"},
            }, req_id=1)
            recv(proc)
            send(proc, "notifications/initialized", {})
            time.sleep(0.3)

            click_result = call_tool(proc, 2, "click", {
                "pid": pid,
                "window_id": window_id,
                "x": 240.0,
                "y": 160.0,
            })

            with open("/tmp/issue-2022-click-result.json", "w", encoding="utf-8") as f:
                json.dump(click_result, f, indent=2, sort_keys=True)
                f.write("\n")

            print("driver click returned success", flush=True)
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
  name = "cua-driver-linux-sdl-send-event-click-regression-issue-2022-test";
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
        sdlClickApp
      ];
    };

  testScript = ''
    # cache-bust 2026-06-25.1: this comment is part of the derivation, so
    # bumping it forces a fresh run instead of a binary-cache substitution.
    machine.start()
    machine.wait_for_unit("multi-user.target")

    with subtest("Start X11 desktop, focus a control xterm, and launch the SDL target"):
        machine.execute("Xvfb :99 -screen 0 1280x1024x24 >/tmp/xvfb.log 2>&1 &")
        machine.wait_until_succeeds("test -e /tmp/.X11-unix/X99", timeout=10)
        machine.execute("DISPLAY=:99 openbox --config-file ${openboxRc} >/tmp/openbox.log 2>&1 &")
        machine.execute("DISPLAY=:99 picom --backend xrender >/tmp/picom.log 2>&1 &")
        machine.execute("sh -lc \"DISPLAY=:99 xterm -T 'Issue 2022 Control' -fa Monospace -fs 14 -geometry 70x24+40+80 >/tmp/issue-2022-control.log 2>&1 & echo \\$! >/tmp/issue-2022-control-pid.txt\"")
        machine.wait_until_succeeds("DISPLAY=:99 xdotool search --sync --name 'Issue 2022 Control' >/tmp/issue-2022-control-xid.txt", timeout=20)
        machine.succeed("DISPLAY=:99 xdotool windowactivate --sync $(head -1 /tmp/issue-2022-control-xid.txt)")
        machine.succeed("DISPLAY=:99 xdotool windowfocus --sync $(head -1 /tmp/issue-2022-control-xid.txt)")
        machine.execute("sh -lc \"DISPLAY=:99 SDL_VIDEODRIVER=x11 ${sdlClickApp}/bin/issue-2022-sdl-click-app >/tmp/issue-2022-sdl.log 2>&1 & echo \\$! >/tmp/issue-2022-sdl-pid.txt\"")
        machine.wait_until_succeeds("test -f /tmp/issue-2022-sdl-window-ready.txt", timeout=20)
        machine.wait_until_succeeds("DISPLAY=:99 xdotool search --sync --name 'Issue 2022 SDL Click Repro' >/tmp/issue-2022-sdl-xid.txt", timeout=20)

    with subtest("Confirm the SDL window is a background target before the click"):
        control = machine.succeed("head -1 /tmp/issue-2022-control-xid.txt").strip()
        active = machine.succeed("DISPLAY=:99 xdotool getactivewindow").strip()
        assert control == active, f"expected control xterm {control} to stay focused, got {active}"

    with subtest("cua-driver click reports success against the SDL window"):
        machine.copy_from_host("${mcpSdlClickTest}", "/tmp/mcp-issue-2022-sdl-click.py")
        result = machine.succeed("timeout 60 env DISPLAY=:99 SDL_VIDEODRIVER=x11 python3 /tmp/mcp-issue-2022-sdl-click.py 2>&1")
        machine.log(result)
        assert "driver click returned success" in result, result

    with subtest("INTENTIONAL RED: SDL never records a button event on current main"):
        machine.succeed(
            "sh -lc '"
            "for i in $(seq 1 10); do "
            "  test -f /tmp/issue-2022-sdl-click-received.txt && exit 0; "
            "  sleep 1; "
            "done; "
            "echo \"cua-driver click reported success, but SDL never wrote /tmp/issue-2022-sdl-click-received.txt\"; "
            "echo \"=== click result ===\"; cat /tmp/issue-2022-click-result.json || true; "
            "echo \"=== active window ===\"; DISPLAY=:99 xdotool getactivewindow || true; "
            "echo \"=== SDL events ===\"; cat /tmp/issue-2022-sdl-events.log || true; "
            "echo \"=== SDL app log ===\"; cat /tmp/issue-2022-sdl.log || true; "
            "exit 1'"
        )
  '';
}
