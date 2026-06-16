# Shared MCP client used by the native-Wayland TDD tests.
#
# It is copied into the VM as /tmp/driver_client.py and imported by the small
# per-test scripts. Crucially, window discovery goes ONLY through cua-driver's
# own `list_windows` tool — there is no xdotool/X11 fallback — so on native
# Wayland (where the driver currently enumerates nothing) `find_window` times
# out and the test fails. That is the intended red state for TDD.
{ pkgs }:

pkgs.writeText "driver_client.py" ''
  import json, os, subprocess, sys, threading, time

  DRIVER_BIN = os.environ.get("CUA_DRIVER_BIN", "cua-driver")


  class Driver:
      def __init__(self):
          self.proc = subprocess.Popen(
              [DRIVER_BIN, "mcp", "--no-daemon-relaunch"],
              stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
              env={**os.environ},
          )
          threading.Thread(target=self._drain, daemon=True).start()
          self._id = 1

      def _drain(self):
          for line in self.proc.stderr:
              sys.stderr.buffer.write(line)
              sys.stderr.buffer.flush()

      def _send(self, method, params=None, req_id=None):
          msg = {"jsonrpc": "2.0", "method": method}
          if params is not None:
              msg["params"] = params
          if req_id is not None:
              msg["id"] = req_id
          self.proc.stdin.write((json.dumps(msg) + "\n").encode())
          self.proc.stdin.flush()

      def _recv(self, timeout=45):
          result = [None]

          def reader():
              result[0] = self.proc.stdout.readline()

          th = threading.Thread(target=reader)
          th.start()
          th.join(timeout)
          if th.is_alive():
              raise TimeoutError("no response from driver within timeout")
          line = result[0].decode().strip()
          if not line:
              raise RuntimeError("driver returned an empty response")
          return json.loads(line)

      def initialize(self, client="nixos-wayland"):
          self._send("initialize", {
              "protocolVersion": "2024-11-05", "capabilities": {},
              "clientInfo": {"name": client, "version": "1.0.0"},
          }, req_id=self._id)
          resp = self._recv()
          self._id += 1
          assert "result" in resp, f"initialize failed: {resp}"
          self._send("notifications/initialized", {})
          time.sleep(0.3)
          return resp

      def call(self, name, args, timeout=60):
          self._send("tools/call", {"name": name, "arguments": args}, req_id=self._id)
          self._id += 1
          resp = self._recv(timeout=timeout)
          if resp.get("error"):
              raise RuntimeError(f"{name} failed: {resp}")
          if resp.get("result", {}).get("isError"):
              raise RuntimeError(f"{name} returned isError: {resp}")
          return resp

      def list_windows(self):
          resp = self.call("list_windows", {})
          result = resp.get("result", {})
          # Preferred: MCP structuredContent = { "windows": [ {window_id, pid,
          # title, ...}, ... ] }.
          structured = result.get("structuredContent")
          if isinstance(structured, dict) and isinstance(structured.get("windows"), list):
              return structured["windows"]
          if isinstance(structured, list):
              return structured
          # Fallback: a text item that happens to be JSON.
          for item in result.get("content", []):
              if item.get("type") == "text":
                  try:
                      data = json.loads(item.get("text", ""))
                  except Exception:
                      continue
                  if isinstance(data, list):
                      return data
                  if isinstance(data, dict) and "windows" in data:
                      return data["windows"]
          return []

      def find_window(self, title_substr, timeout=30, interval=1.0):
          """Poll list_windows until a native Wayland toplevel whose title
          contains `title_substr` appears. Raises on timeout — the red point on
          a driver that cannot yet enumerate Wayland windows."""
          deadline = time.time() + timeout
          needle = title_substr.lower()
          last = []
          while time.time() < deadline:
              last = self.list_windows()
              for w in last:
                  # Match across whatever identity fields a Wayland-capable
                  # list_windows might surface (title today; app_id/app/class
                  # once native Wayland metadata is wired up).
                  hay = " ".join(
                      str(w.get(k, "")) for k in ("title", "app_id", "app", "class")
                  ).lower()
                  if needle in hay:
                      return int(w.get("pid") or 0), int(w["window_id"])
              time.sleep(interval)
          raise AssertionError(
              f"cua-driver never enumerated a Wayland window titled ~{title_substr!r}; "
              f"last list_windows() = {last}"
          )

      def close(self):
          try:
              self.proc.stdin.close()
              self.proc.terminate()
              self.proc.wait(timeout=5)
          except Exception:
              pass
''
