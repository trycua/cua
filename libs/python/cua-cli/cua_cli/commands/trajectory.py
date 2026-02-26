"""cua trajectory — manage recorded action trajectories.

Subcommands:
  ls      List trajectory sessions
  view    Zip + serve locally, open cua.ai/trajectory-viewer
  export  Generate a self-contained HTML trajectory report
  clean   Delete old sessions
  stop    Stop the local file server
"""

from __future__ import annotations

import argparse
import json
import os
import signal
import subprocess
import sys
import webbrowser
from pathlib import Path
from urllib.parse import quote

from cua_cli.utils.trajectory_recorder import (
    clean_trajectories,
    list_trajectories,
    zip_trajectory,
)

_PID_FILE = Path.home() / ".cua" / "trajectory_server.pid"


def register_parser(subparsers: argparse._SubParsersAction) -> None:
    """Register ``cua trajectory`` (and alias ``cua traj``) commands."""
    for cmd_name in ("trajectory", "traj"):
        p = subparsers.add_parser(
            cmd_name,
            help="Manage recorded action trajectories",
            description="List, view, and clean trajectory recordings from cua do sessions.",
            formatter_class=argparse.RawDescriptionHelpFormatter,
            epilog="""
Examples:
  cua trajectory ls                         List all sessions
  cua trajectory ls my-container            List sessions for a machine
  cua trajectory view                       View latest session in browser
  cua trajectory view my-container          View latest for specific machine
  cua trajectory view --port 9090           Use a custom port
  cua trajectory export                     Export latest session as HTML report
  cua trajectory export my-machine          Export latest session for a machine
  cua trajectory export --output out.html   Specify output path
  cua trajectory stop                       Stop the file server
  cua trajectory clean --older-than 7       Delete sessions older than 7 days
  cua trajectory clean --machine my-ct -y   Delete all sessions for a machine
""",
        )

        sub = p.add_subparsers(dest="traj_action", metavar="action")
        sub.required = True

        # ls
        ls_p = sub.add_parser("ls", help="List trajectory sessions")
        ls_p.add_argument("machine", nargs="?", default=None, help="Filter by machine name")
        ls_p.add_argument("--json", dest="as_json", action="store_true", help="Output as JSON")

        # view
        view_p = sub.add_parser("view", help="Zip and open in cua.ai/trajectory-viewer")
        view_p.add_argument(
            "target",
            nargs="?",
            default=None,
            help="Machine name, session timestamp, or path (default: latest session)",
        )
        view_p.add_argument(
            "--port",
            "-p",
            type=int,
            default=8089,
            help="Port for the local file server (default: 8089)",
        )

        # export
        export_p = sub.add_parser("export", help="Generate a self-contained HTML trajectory report")
        export_p.add_argument(
            "target",
            nargs="?",
            default=None,
            help="Machine name, session timestamp, or path (default: latest session)",
        )
        export_p.add_argument(
            "--output",
            "-o",
            default=None,
            metavar="PATH",
            help="Output HTML file path (default: report.html inside session dir)",
        )
        export_p.add_argument(
            "--quality",
            "-q",
            type=int,
            default=75,
            help="JPEG compression quality 1-100 (default: 75)",
        )
        export_p.add_argument(
            "--no-open",
            action="store_true",
            help="Do not open the report in a browser after generating",
        )

        # clean
        clean_p = sub.add_parser("clean", help="Delete old trajectory sessions")
        clean_p.add_argument(
            "--older-than",
            type=int,
            default=None,
            metavar="DAYS",
            help="Only delete sessions older than DAYS days",
        )
        clean_p.add_argument(
            "--machine", default=None, help="Only delete sessions for this machine"
        )
        clean_p.add_argument("-y", "--yes", action="store_true", help="Skip confirmation prompt")

        # stop
        sub.add_parser("stop", help="Stop the trajectory file server")


# ── ls ───────────────────────────────────────────────────────────────────────


def _cmd_ls(args: argparse.Namespace) -> int:
    machine = getattr(args, "machine", None)
    sessions = list_trajectories(machine=machine)

    if not sessions:
        if machine:
            print(f"No trajectory sessions found for '{machine}'.")
        else:
            print("No trajectory sessions found.")
        return 0

    if getattr(args, "as_json", False):
        print(json.dumps(sessions, indent=2))
        return 0

    print(f"{'Machine':<20} {'Session':<18} {'Turns':>5}  {'Created'}")
    print("-" * 70)
    for s in sessions:
        print(f"{s['machine']:<20} {s['session']:<18} {s['turns']:>5}  {s['created']}")
    return 0


# ── view ─────────────────────────────────────────────────────────────────────


def _resolve_session(target: str | None) -> str | None:
    """Resolve a target to a session path."""
    if target and Path(target).is_dir():
        return target

    sessions = list_trajectories()
    if not sessions:
        return None

    if target is None:
        return sessions[-1]["path"]

    # Try as machine name (latest for that machine)
    machine_sessions = [s for s in sessions if s["machine"] == target]
    if machine_sessions:
        return machine_sessions[-1]["path"]

    # Try as session timestamp
    for s in sessions:
        if s["session"] == target:
            return s["path"]

    return None


# Minimal CORS-enabled HTTP server script, run as a subprocess.
_SERVER_SCRIPT = """\
import sys, os
from http.server import HTTPServer, SimpleHTTPRequestHandler

class CORSHandler(SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Allow-Methods", "GET, OPTIONS")
        self.send_header("Access-Control-Allow-Headers", "*")
        super().end_headers()
    def do_OPTIONS(self):
        self.send_response(204)
        self.end_headers()
    def log_message(self, format, *args):
        pass

os.chdir(sys.argv[1])
port = int(sys.argv[2])
HTTPServer(("127.0.0.1", port), CORSHandler).serve_forever()
"""


def _cmd_view(args: argparse.Namespace) -> int:
    target = getattr(args, "target", None)
    port = getattr(args, "port", 8089)
    session_path = _resolve_session(target)

    if not session_path:
        label = f" for '{target}'" if target else ""
        print(f"No trajectory session found{label}.", file=sys.stderr)
        return 1

    session_dir = Path(session_path)

    # Zip the session
    zip_path = zip_trajectory(session_dir)
    zip_name = zip_path.name

    # Stop any existing server
    _stop_server(quiet=True)

    # Start a CORS-enabled file server in the background
    serve_dir = str(zip_path.parent)
    proc = subprocess.Popen(
        [sys.executable, "-c", _SERVER_SCRIPT, serve_dir, str(port)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )

    # Save PID
    _PID_FILE.parent.mkdir(parents=True, exist_ok=True)
    _PID_FILE.write_text(json.dumps({"pid": proc.pid, "port": port}))

    zip_url = f"http://localhost:{port}/{zip_name}"
    viewer_url = f"https://cua.ai/trajectory-viewer?zip={quote(zip_url, safe='')}"

    machine = session_dir.parent.name
    session_ts = session_dir.name
    print(f"Serving: {machine}/{session_ts}")
    print(f"Viewer:  {viewer_url}")
    print("Stop with: cua trajectory stop")

    try:
        webbrowser.open(viewer_url)
    except Exception:
        pass

    return 0


# ── export ───────────────────────────────────────────────────────────────────


def _cmd_export(args: argparse.Namespace) -> int:
    from cua_cli.utils.trajectory_html import generate_html

    target = getattr(args, "target", None)
    output = getattr(args, "output", None)
    quality = getattr(args, "quality", 75)
    no_open = getattr(args, "no_open", False)

    session_path = _resolve_session(target)
    if not session_path:
        label = f" for '{target}'" if target else ""
        print(f"No trajectory session found{label}.", file=sys.stderr)
        return 1

    session_dir = Path(session_path)
    machine = session_dir.parent.name
    session_ts = session_dir.name

    # Determine output path
    if output:
        out_path = Path(output).expanduser().resolve()
    else:
        out_path = session_dir / "report.html"

    out_path.parent.mkdir(parents=True, exist_ok=True)

    print(f"Exporting: {machine}/{session_ts}")
    print("Generating HTML report...")

    html = generate_html(session_dir, jpeg_quality=quality)
    out_path.write_text(html, encoding="utf-8")

    size_mb = out_path.stat().st_size / (1024 * 1024)
    print(f"Report saved: {out_path}  ({size_mb:.1f} MB)")

    if not no_open:
        try:
            webbrowser.open(out_path.as_uri())
        except Exception:
            pass

    return 0


# ── stop ─────────────────────────────────────────────────────────────────────


def _is_our_server(pid: int) -> bool:
    """Check if the given PID is a Python process we spawned."""
    try:
        os.kill(pid, 0)  # Check if process exists (doesn't actually send a signal)
    except (ProcessLookupError, PermissionError):
        return False
    # On macOS/Linux, verify it's a Python process via /proc or ps
    try:
        import platform

        if platform.system() != "Windows":
            result = subprocess.run(
                ["ps", "-p", str(pid), "-o", "comm="],
                capture_output=True,
                text=True,
                timeout=2,
            )
            comm = result.stdout.strip().lower()
            return "python" in comm
    except Exception:
        pass
    return True  # On Windows or if ps fails, trust the PID file


def _stop_server(quiet: bool = False) -> bool:
    """Kill the background file server. Returns True if a server was stopped."""
    if not _PID_FILE.exists():
        if not quiet:
            print("No file server is running.")
        return False

    try:
        info = json.loads(_PID_FILE.read_text())
        pid = info["pid"]
    except Exception:
        _PID_FILE.unlink(missing_ok=True)
        if not quiet:
            print("Could not read server PID file.", file=sys.stderr)
        return False

    stopped = False
    try:
        if not _is_our_server(pid):
            if not quiet:
                print("Server was not running (stale PID file removed).")
            _PID_FILE.unlink(missing_ok=True)
            return False
        os.kill(pid, signal.SIGTERM)
        stopped = True
        if not quiet:
            print(f"Stopped file server (pid {pid}).")
    except ProcessLookupError:
        if not quiet:
            print("Server was not running (stale PID file removed).")
    except Exception as e:
        if not quiet:
            print(f"Failed to stop server: {e}", file=sys.stderr)
    finally:
        _PID_FILE.unlink(missing_ok=True)

    return stopped


def _cmd_stop(_args: argparse.Namespace) -> int:
    _stop_server()
    return 0


# ── clean ────────────────────────────────────────────────────────────────────


def _cmd_clean(args: argparse.Namespace) -> int:
    older_than = getattr(args, "older_than", None)
    machine = getattr(args, "machine", None)
    yes = getattr(args, "yes", False)

    sessions = list_trajectories(machine=machine)
    if not sessions:
        print("No trajectory sessions to clean.")
        return 0

    if older_than is not None:
        from datetime import datetime, timedelta

        cutoff = datetime.now() - timedelta(days=older_than)
        sessions = [s for s in sessions if datetime.fromisoformat(s["created"]) < cutoff]

    if not sessions:
        print("No sessions match the criteria.")
        return 0

    if not yes:
        print(f"Will delete {len(sessions)} session(s):")
        for s in sessions:
            print(f"  {s['machine']}/{s['session']} ({s['turns']} turns)")
        try:
            answer = input("Continue? [y/N] ").strip().lower()
        except (EOFError, KeyboardInterrupt):
            print("\nCancelled.")
            return 1
        if answer != "y":
            print("Cancelled.")
            return 1

    deleted = clean_trajectories(older_than_days=older_than, machine=machine)
    print(f"Deleted {len(deleted)} session(s).")
    return 0


# ── dispatch ─────────────────────────────────────────────────────────────────


def execute(args: argparse.Namespace) -> int:
    dispatch = {
        "ls": _cmd_ls,
        "view": _cmd_view,
        "export": _cmd_export,
        "clean": _cmd_clean,
        "stop": _cmd_stop,
    }
    handler = dispatch.get(args.traj_action)
    if not handler:
        return 1
    return handler(args)
