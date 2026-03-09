"""Trace command - View and analyze trace datasets.

Usage:
    cb trace view <id>   # View a single trace (run_id or session_id)
    cb trace traj <id>   # View agent trajectories from a run in cua.ai/trajectory-viewer
"""

from __future__ import annotations

import base64
import json
import os
import shutil
import tempfile
import threading
import urllib.parse
import webbrowser
import zipfile as _zipfile
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any, List, Optional, Tuple

from datasets import load_from_disk

RESET = "\033[0m"
BOLD = "\033[1m"
CYAN = "\033[36m"
GREEN = "\033[92m"
YELLOW = "\033[33m"
RED = "\033[91m"
GREY = "\033[90m"


def _get_runs_dir() -> Path:
    """Get the default runs output directory (XDG compliant)."""
    xdg_data = os.environ.get("XDG_DATA_HOME", os.path.expanduser("~/.local/share"))
    return Path(xdg_data) / "cua-bench" / "runs"


def _resolve_trace_path(identifier: str) -> Optional[Path]:
    """Resolve a run_id or session_id to a trace path.

    Args:
        identifier: Either a run_id (8 chars) or session_id (task-<run_id>-<task>-v<variant>)

    Returns:
        Path to the trace directory, or None if not found
    """
    runs_dir = _get_runs_dir()

    # Check if it's a session_id (contains "task-")
    if identifier.startswith("task-"):
        # Parse session_id: task-<run_id>-<task_name>-v<variant>
        parts = identifier.split(
            "-", 2
        )  # Split into ["task", "<run_id>", "<task_name>-v<variant>"]
        if len(parts) < 3:
            return None

        run_id = parts[1]
        task_variant = parts[2]  # e.g., "click-button-v0" or could have dashes in task name

        # Find the variant number (last -v<N>)
        import re

        match = re.match(r"(.+)-v(\d+)$", task_variant)
        if not match:
            return None

        task_name = match.group(1)
        variant = match.group(2)

        # Build path: <runs_dir>/<run_id>/<task_name>_v<variant>/task_0_trace
        task_dir = runs_dir / run_id / f"{task_name}_v{variant}"
        trace_dir = task_dir / "task_0_trace"

        if trace_dir.exists():
            return trace_dir
        return None
    else:
        # Assume it's a run_id - just check if the directory exists
        run_dir = runs_dir / identifier
        if run_dir.exists():
            return run_dir
        return None


def _collect_run_traces(run_dir: Path) -> List[Tuple[str, Path]]:
    """Collect all trace folders in a run directory.

    Args:
        run_dir: Path to the run directory (e.g., .../runs/<run_id>/)

    Returns:
        List of (name, trace_path) tuples
    """
    traces: List[Tuple[str, Path]] = []
    if not run_dir.exists():
        return traces

    # Each task variant directory is named <task_name>_v<variant>
    for task_dir in sorted(run_dir.iterdir()):
        if not task_dir.is_dir():
            continue

        # Look for task_0_trace subdirectory
        trace_dir = task_dir / "task_0_trace"
        if trace_dir.exists():
            try:
                # Verify it's a valid trace dataset
                _ = load_from_disk(str(trace_dir))
                traces.append((task_dir.name, trace_dir))
            except Exception:
                continue

    return traces


def register_parser(subparsers):
    """Register the trace command parser."""
    trace_parser = subparsers.add_parser("trace", help="View and analyze trace datasets")
    trace_subparsers = trace_parser.add_subparsers(dest="trace_command")

    # cb trace view <id>
    view_parser = trace_subparsers.add_parser("view", help="View a single trace in browser")
    view_parser.add_argument(
        "identifier",
        help='Run ID or session ID (e.g., "30c12572" or "task-30c12572-click-button-v0")',
    )

    # cb trace traj <run_id>
    traj_parser = trace_subparsers.add_parser(
        "traj", help="View agent trajectories from a run in cua.ai/trajectory-viewer"
    )
    traj_parser.add_argument("identifier", help='Run ID (e.g., "30c12572")')
    traj_parser.add_argument(
        "--port",
        "-p",
        type=int,
        default=8090,
        help="Local file server port (default: 8090)",
    )


def execute(args):
    """Execute the trace command."""
    trace_command = getattr(args, "trace_command", None)

    if trace_command == "view":
        return cmd_view(args)
    elif trace_command == "traj":
        return cmd_traj(args)
    else:
        print(f"{YELLOW}Usage: cb trace <command> <id>{RESET}")
        print(f"\n{GREY}Commands:{RESET}")
        print("  view <id>   View a single trace (run_id or session_id)")
        print("  traj <id>   View agent trajectories in cua.ai/trajectory-viewer")
        return 1


# ============================================================================
# cmd_view - View a single trace dataset
# ============================================================================


def _html_escape(s: str) -> str:
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def cmd_view(args) -> int:
    """View a trace via an in-memory HTTP server."""
    identifier = args.identifier

    # Resolve identifier to trace path
    trace_path = _resolve_trace_path(identifier)

    if trace_path is None:
        print(f"{RED}Trace not found for: {identifier}{RESET}")
        print(f"{GREY}Try:{RESET}")
        print(f"  cb run list           {GREY}# List all runs{RESET}")
        print(f"  cb run info <run_id>  {GREY}# Show sessions in a run{RESET}")
        return 1

    # If it's a run directory (not a specific trace), pick the first trace
    if not (trace_path / "dataset_info.json").exists():
        # It's a run directory, need to pick a specific trace
        traces = _collect_run_traces(trace_path)
        if not traces:
            print(f"{RED}No traces found in run: {identifier}{RESET}")
            return 1

        if len(traces) == 1:
            # Only one trace, use it
            _, trace_path = traces[0]
        else:
            # Multiple traces - show error
            print(
                f"{YELLOW}Run contains {len(traces)} traces. Specify a session_id to view a specific trace:{RESET}"
            )
            for name, _ in traces[:5]:
                session_id = f"task-{identifier}-{name.replace('_v', '-v')}"
                print(f"  cb trace view {session_id}")
            if len(traces) > 5:
                print(f"  {GREY}... and {len(traces) - 5} more{RESET}")
            print(f"\n{GREY}Or use grid to view all:{RESET}")
            print(f"  cb trace grid {identifier}")
            return 1

    path = trace_path

    ds = load_from_disk(str(path))

    # Set image format to PIL for convenience
    try:
        ds = ds.cast_column("data_images", ds.features["data_images"])
        ds.set_format(type=None, columns=None)
    except Exception:
        pass

    rows: List[str] = []
    row_meta: List[dict] = []
    row_data: List[dict] = []

    for i, row in enumerate(ds):
        imgs_html: List[str] = []
        imgs = row.get("data_images") or []
        for j, img in enumerate(imgs):
            try:
                if hasattr(img, "save"):
                    import io

                    buf = io.BytesIO()
                    img.save(buf, format="PNG")
                    b64 = base64.b64encode(buf.getvalue()).decode("ascii")
                    imgs_html.append(
                        f'<div class="img-container"><img class="trace-img" data-row="{i}" data-img="{j}" src="data:image/png;base64,{b64}" style="max-width:300px; max-height:300px; cursor: zoom-in;"/></div>'
                    )
                elif isinstance(img, dict):
                    data = img.get("bytes")
                    if data:
                        b64 = base64.b64encode(data).decode("ascii")
                        imgs_html.append(
                            f'<div class="img-container"><img class="trace-img" data-row="{i}" data-img="{j}" src="data:image/png;base64,{b64}" style="max-width:300px; max-height:300px; cursor: zoom-in;"/></div>'
                        )
                    else:
                        pth = img.get("path")
                        if pth and Path(pth).exists():
                            data = Path(pth).read_bytes()
                            b64 = base64.b64encode(data).decode("ascii")
                            imgs_html.append(
                                f'<div class="img-container"><img class="trace-img" data-row="{i}" data-img="{j}" src="data:image/png;base64,{b64}" style="max-width:300px; max-height:300px; cursor: zoom-in;"/></div>'
                            )
            except Exception:
                continue

        meta = {
            "event_name": row.get("event_name"),
            "timestamp": row.get("timestamp"),
            "trajectory_id": row.get("trajectory_id"),
        }
        row_meta.append(
            {
                "timestamp": meta["timestamp"],
                "trajectory_id": meta["trajectory_id"],
            }
        )

        data_json = row.get("data_json")
        parsed_data: Any
        if isinstance(data_json, (dict, list)):
            parsed_data = data_json
        elif isinstance(data_json, str):
            try:
                parsed_data = json.loads(data_json)
            except Exception:
                parsed_data = data_json
        else:
            parsed_data = data_json

        row_data.append(parsed_data if isinstance(parsed_data, dict) else {})
        data_str = (
            json.dumps(parsed_data, ensure_ascii=False, indent=2)
            if not isinstance(parsed_data, str)
            else str(parsed_data)
        )

        rows.append(
            """
            <tr>
              <td>
                <andypf-json-viewer indent="2" expanded="3" theme="monokai" show-data-types="true" show-toolbar="false" expand-icon-type="arrow" show-copy="true" show-size="true">{meta_json}</andypf-json-viewer>
              </td>
              <td>
                <andypf-json-viewer indent="2" expanded="3" theme="monokai" show-data-types="true" show-toolbar="false" expand-icon-type="arrow" show-copy="true" show-size="true">{data}</andypf-json-viewer>
              </td>
              <td>{images}</td>
            </tr>
            """.format(
                meta_json=_html_escape(json.dumps(meta, ensure_ascii=False, indent=2)),
                data=_html_escape(data_str),
                images="<br/>".join(imgs_html) if imgs_html else "",
            )
        )

    template_path = Path(__file__).resolve().parents[2] / "www" / "trace_viewer.html"
    html_template = template_path.read_text(encoding="utf-8")
    row_data_b64 = base64.b64encode(json.dumps(row_data).encode("utf-8")).decode("ascii")

    html = (
        html_template.replace("__PATH_NAME__", str(path.name))
        .replace("__PATH__", str(path))
        .replace("__ROWS__", "".join(rows))
        .replace("__ROW_META__", json.dumps(row_meta))
        .replace("__ROW_DATA__", f'"{row_data_b64}"')
    )

    class _Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            try:
                body = html.encode("utf-8")
                self.send_response(200)
                self.send_header("Content-Type", "text/html; charset=utf-8")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
            except Exception:
                self.send_response(500)
                self.end_headers()

        def log_message(self, format, *args):
            return

    httpd = ThreadingHTTPServer(("127.0.0.1", 0), _Handler)
    port = httpd.server_address[1]
    url = f"http://127.0.0.1:{port}/"

    t = threading.Thread(target=httpd.serve_forever, daemon=True)
    t.start()
    webbrowser.open(url)
    print(f"{CYAN}Serving trace viewer at:{RESET} {url}\n{GREY}Press Enter to stop...{RESET}")
    try:
        input()
    except KeyboardInterrupt:
        pass
    finally:
        httpd.shutdown()
        httpd.server_close()
    return 0


# ============================================================================
# cmd_traj - View agent trajectories via cua.ai/trajectory-viewer
# ============================================================================


def _collect_run_trajectories(run_dir: Path) -> List[Tuple[str, Path]]:
    """Collect all cua-agent trajectory sessions in a run directory.

    Scans for task_*_agent_logs/trajectories/<session>/ inside each task dir.

    Returns:
        List of (task_name, session_dir) tuples sorted by task name.
    """
    sessions: List[Tuple[str, Path]] = []
    if not run_dir.exists():
        return sessions

    for task_dir in sorted(run_dir.iterdir()):
        if not task_dir.is_dir():
            continue
        for agent_logs_dir in sorted(task_dir.glob("task_*_agent_logs")):
            traj_root = agent_logs_dir / "trajectories"
            if not traj_root.exists():
                continue
            for session_dir in sorted(traj_root.iterdir()):
                if session_dir.is_dir():
                    sessions.append((task_dir.name, session_dir))

    return sessions


def cmd_traj(args) -> int:
    """View cua-agent trajectories from a run using the cua.ai trajectory viewer."""
    identifier = args.identifier
    port = getattr(args, "port", 8090)

    run_path = _resolve_trace_path(identifier)
    if run_path is None:
        print(f"{RED}Run not found: {identifier}{RESET}")
        print(f"{GREY}Try:{RESET}")
        print(f"  cb run list  {GREY}# List all runs{RESET}")
        return 1

    if (run_path / "dataset_info.json").exists():
        print(f"{YELLOW}traj requires a run_id, not a session_id.{RESET}")
        print(f"{GREY}To view a single trace: cb trace view {identifier}{RESET}")
        return 1

    sessions = _collect_run_trajectories(run_path)
    if not sessions:
        print(f"{YELLOW}No agent trajectories found in run: {identifier}{RESET}")
        print(f"{GREY}(Trajectories are saved when tasks run with cua-agent){RESET}")
        return 1

    tmp_dir = Path(tempfile.mkdtemp(prefix="cb-traj-"))
    try:
        # Zip each session into the temp dir
        zip_entries: List[Tuple[str, str, str]] = []  # (task_name, session_name, zip_name)
        for task_name, session_dir in sessions:
            zip_name = f"{task_name}__{session_dir.name}.zip"
            zip_path = tmp_dir / zip_name
            with _zipfile.ZipFile(zip_path, "w", _zipfile.ZIP_DEFLATED) as zf:
                for child in sorted(session_dir.rglob("*")):
                    if child.is_file():
                        zf.write(child, child.relative_to(session_dir))
            zip_entries.append((task_name, session_dir.name, zip_name))

        viewer_base = "https://cua.ai/trajectory-viewer"
        zip_names = {e[2] for e in zip_entries}

        # Build index page
        items_html = ""
        for task_name, session_name, zip_name in zip_entries:
            zip_url = f"http://localhost:{port}/{zip_name}"
            viewer_url = f"{viewer_base}?zip={urllib.parse.quote(zip_url, safe='')}"
            items_html += f"""
      <div class="item">
        <div>
          <div class="item-name">{task_name}</div>
          <div class="item-meta">{session_name}</div>
        </div>
        <a class="btn" href="{viewer_url}" target="_blank">Open Viewer</a>
      </div>"""

        index_html = f"""<!doctype html>
<html>
<head>
  <meta charset="utf-8"/>
  <title>Trajectories \u2014 Run {identifier}</title>
  <style>
    body{{margin:24px;background:#0e0e0c;color:#e5e7eb;font-family:ui-sans-serif,system-ui,-apple-system,Segoe UI,Roboto,Ubuntu,Cantarell,Noto Sans,Helvetica Neue,Arial;}}
    h1{{font-size:18px;margin:0 0 16px 0;font-weight:600;}}
    .list{{display:flex;flex-direction:column;gap:8px;}}
    .item{{background:#1b1b18;border:1px solid #333;border-radius:6px;padding:14px 16px;display:flex;align-items:center;justify-content:space-between;}}
    .item-name{{font-weight:600;margin-bottom:3px;}}
    .item-meta{{color:#94a3b8;font-size:12px;}}
    .btn{{background:#2563eb;color:#fff;border:none;border-radius:5px;padding:8px 14px;cursor:pointer;font-size:13px;text-decoration:none;display:inline-block;}}
    .btn:hover{{background:#1d4ed8;}}
  </style>
</head>
<body>
  <h1>Trajectories \u2014 Run {identifier}</h1>
  <div class="list">{items_html}
  </div>
</body>
</html>"""

        index_bytes = index_html.encode("utf-8")

        class _Handler(BaseHTTPRequestHandler):
            def _send_cors(self):
                self.send_header("Access-Control-Allow-Origin", "*")
                self.send_header("Access-Control-Allow-Methods", "GET, OPTIONS")
                self.send_header("Access-Control-Allow-Headers", "*")

            def do_OPTIONS(self):
                self.send_response(204)
                self._send_cors()
                self.end_headers()

            def do_GET(self):
                try:
                    path = self.path.split("?")[0].lstrip("/")
                    if not path:
                        body = index_bytes
                        ct = "text/html; charset=utf-8"
                    elif path in zip_names:
                        body = (tmp_dir / path).read_bytes()
                        ct = "application/zip"
                    else:
                        self.send_response(404)
                        self.end_headers()
                        return
                    self.send_response(200)
                    self.send_header("Content-Type", ct)
                    self.send_header("Content-Length", str(len(body)))
                    self._send_cors()
                    self.end_headers()
                    self.wfile.write(body)
                except Exception:
                    self.send_response(500)
                    self.end_headers()

            def log_message(self, format, *args):
                return

        try:
            httpd = ThreadingHTTPServer(("127.0.0.1", port), _Handler)
        except OSError:
            httpd = ThreadingHTTPServer(("127.0.0.1", 0), _Handler)
            port = httpd.server_address[1]

        t = threading.Thread(target=httpd.serve_forever, daemon=True)
        t.start()

        # Single session → open viewer directly; multiple → open index
        if len(zip_entries) == 1:
            task_name, session_name, zip_name = zip_entries[0]
            zip_url = f"http://localhost:{port}/{zip_name}"
            open_url = f"{viewer_base}?zip={urllib.parse.quote(zip_url, safe='')}"
        else:
            open_url = f"http://localhost:{port}/"

        webbrowser.open(open_url)

        n = len(zip_entries)
        print(f"{CYAN}Serving {n} trajectory session{'s' if n != 1 else ''}:{RESET}")
        for task_name, session_name, zip_name in zip_entries:
            zip_url = f"http://localhost:{port}/{zip_name}"
            viewer_url = f"{viewer_base}?zip={urllib.parse.quote(zip_url, safe='')}"
            print(f"  {GREY}{task_name}:{RESET} {viewer_url}")
        print(f"\n{GREY}Press Enter to stop...{RESET}")

        try:
            input()
        except KeyboardInterrupt:
            pass
        finally:
            httpd.shutdown()
            httpd.server_close()

    finally:
        shutil.rmtree(tmp_dir, ignore_errors=True)

    return 0
