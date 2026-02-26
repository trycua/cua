"""Generate a self-contained HTML trajectory report.

Reads turn data (screenshots + agent response JSON) from a trajectory
session directory and produces a single HTML file that can be opened
offline in any browser.  Screenshots are compressed to JPEG quality 75
and base64-encoded inline.
"""

from __future__ import annotations

import base64
import io
import json
import re
from datetime import datetime
from pathlib import Path
from typing import Any


def _compress_screenshot(png_path: Path, quality: int = 75) -> str:
    """Read a PNG, convert to JPEG at *quality*, return base64 data URI."""
    try:
        from PIL import Image
    except ImportError:
        # Fallback: embed the raw PNG (larger but still works)
        raw = png_path.read_bytes()
        b64 = base64.b64encode(raw).decode("ascii")
        return f"data:image/png;base64,{b64}"

    img = Image.open(png_path)
    if img.mode in ("RGBA", "P"):
        img = img.convert("RGB")
    buf = io.BytesIO()
    img.save(buf, format="JPEG", quality=quality, optimize=True)
    b64 = base64.b64encode(buf.getvalue()).decode("ascii")
    return f"data:image/jpeg;base64,{b64}"


def _parse_action(json_path: Path) -> dict[str, Any]:
    """Extract the action dict and optional reasoning from an agent response."""
    data = json.loads(json_path.read_text())
    action: dict[str, Any] = {}
    reasoning: str | None = None

    outputs = data.get("response", {}).get("output", [])
    for item in outputs:
        if item.get("type") == "computer_call":
            action = item.get("action", {})
        elif item.get("type") == "message":
            # Extract reasoning text from message content
            for block in item.get("content", []):
                if isinstance(block, dict) and block.get("type") == "output_text":
                    reasoning = block.get("text", "")
                elif isinstance(block, str):
                    reasoning = block

    # Also check top-level for the older format (computer_call_result)
    if not action and "item" in data:
        action = data["item"].get("action", {})

    return {
        "action": action,
        "reasoning": reasoning,
        "raw": data,
    }


def _action_label(action: dict[str, Any]) -> str:
    """Human-readable label for an action."""
    atype = action.get("type", "unknown")
    if atype in ("click", "dclick", "double_click"):
        x, y = action.get("x", "?"), action.get("y", "?")
        btn = action.get("button", "")
        prefix = "dclick" if atype in ("dclick", "double_click") else "click"
        suffix = f" ({btn})" if btn and btn != "left" else ""
        return f"{prefix} ({x}, {y}){suffix}"
    if atype == "drag":
        path = action.get("path", [])
        if len(path) >= 2:
            sx, sy = path[0].get("x", "?"), path[0].get("y", "?")
            ex, ey = path[-1].get("x", "?"), path[-1].get("y", "?")
            return f"drag ({sx},{sy}) -> ({ex},{ey})"
        sx = action.get("startX", action.get("start_x", "?"))
        sy = action.get("startY", action.get("start_y", "?"))
        ex = action.get("endX", action.get("end_x", "?"))
        ey = action.get("endY", action.get("end_y", "?"))
        return f"drag ({sx},{sy}) -> ({ex},{ey})"
    if atype == "type":
        text = action.get("text", "")
        display = text[:40] + ("..." if len(text) > 40 else "")
        return f'type: "{display}"'
    if atype == "key" or atype == "hotkey":
        keys = action.get("keys", [])
        key = action.get("key", "")
        combo = "+".join(keys) if keys else key
        return f"key: {combo}"
    if atype == "scroll":
        x, y = action.get("x", "?"), action.get("y", "?")
        dx = action.get("scroll_x", action.get("deltaX", 0))
        dy = action.get("scroll_y", action.get("deltaY", 0))
        return f"scroll ({x},{y}) dx={dx} dy={dy}"
    if atype == "move":
        x, y = action.get("x", "?"), action.get("y", "?")
        return f"move ({x}, {y})"
    if atype == "screenshot":
        return "screenshot"
    if atype == "wait":
        return "wait"
    return atype


def _collect_turns(session_dir: Path) -> list[dict[str, Any]]:
    """Collect all turns from a session directory.

    Returns a sorted list of dicts with keys:
      screenshot_path, action_data, turn_number, turn_name
    """
    turns: list[dict[str, Any]] = []

    # Find all turn_* directories
    turn_dirs = sorted(
        [d for d in session_dir.iterdir() if d.is_dir() and d.name.startswith("turn_")],
        key=lambda d: d.name,
    )

    for turn_dir in turn_dirs:
        # Parse turn number from dir name
        m = re.match(r"turn_(\d+)", turn_dir.name)
        turn_number = int(m.group(1)) if m else 0

        # Find screenshot
        screenshot = turn_dir / "screenshot.png"
        if not screenshot.exists():
            # Try alternate names
            for candidate in turn_dir.iterdir():
                if candidate.suffix == ".png":
                    screenshot = candidate
                    break

        # Find agent response JSON
        json_path = None
        for candidate in sorted(turn_dir.iterdir()):
            if candidate.suffix == ".json" and "agent_response" in candidate.name:
                json_path = candidate
                break
        if json_path is None:
            # Try alternate naming (e.g. NNNN_computer_call_result.json)
            for candidate in sorted(turn_dir.iterdir()):
                if candidate.suffix == ".json":
                    json_path = candidate
                    break

        action_data = (
            _parse_action(json_path) if json_path else {"action": {}, "reasoning": None, "raw": {}}
        )

        turns.append(
            {
                "turn_number": turn_number,
                "turn_name": turn_dir.name,
                "screenshot_path": screenshot if screenshot.exists() else None,
                "action_data": action_data,
            }
        )

    return turns


def generate_html(
    session_dir: Path,
    *,
    jpeg_quality: int = 75,
) -> str:
    """Generate a self-contained HTML trajectory report.

    Parameters
    ----------
    session_dir:
        Path to a trajectory session directory containing ``turn_NNN/`` subdirs.
    jpeg_quality:
        JPEG compression quality for screenshots (default 75).

    Returns
    -------
    str
        Complete HTML document as a string.
    """
    session_dir = Path(session_dir)
    machine_name = session_dir.parent.name
    session_ts = session_dir.name

    # Try to parse a human-readable timestamp
    try:
        dt = datetime.strptime(session_ts, "%Y%m%d-%H%M%S")
        session_display = dt.strftime("%Y-%m-%d %H:%M:%S")
    except ValueError:
        session_display = session_ts

    turns = _collect_turns(session_dir)
    total_turns = len(turns)

    # Build JS data arrays
    screenshots_js: list[str] = []
    actions_js: list[dict[str, Any]] = []

    for t in turns:
        # Screenshot
        if t["screenshot_path"]:
            data_uri = _compress_screenshot(t["screenshot_path"], quality=jpeg_quality)
        else:
            data_uri = ""
        screenshots_js.append(data_uri)

        # Action info
        action = t["action_data"]["action"]
        actions_js.append(
            {
                "turn": t["turn_number"],
                "name": t["turn_name"],
                "action": action,
                "label": _action_label(action),
                "reasoning": t["action_data"].get("reasoning"),
                "raw": t["action_data"]["raw"],
            }
        )

    screenshots_json = json.dumps(screenshots_js)
    actions_json = json.dumps(actions_js, indent=2)

    html = _HTML_TEMPLATE.replace("{{MACHINE_NAME}}", _html_escape(machine_name))
    html = html.replace("{{SESSION_DISPLAY}}", _html_escape(session_display))
    html = html.replace("{{SESSION_TS}}", _html_escape(session_ts))
    html = html.replace("{{TOTAL_TURNS}}", str(total_turns))
    html = html.replace("{{SCREENSHOTS_JSON}}", screenshots_json)
    html = html.replace("{{ACTIONS_JSON}}", actions_json)

    return html


def _html_escape(s: str) -> str:
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;").replace('"', "&quot;")


# ---------------------------------------------------------------------------
# HTML Template
# ---------------------------------------------------------------------------

_HTML_TEMPLATE = r"""<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Trajectory — {{MACHINE_NAME}} / {{SESSION_TS}}</title>
<style>
  *, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }

  :root {
    --bg: #0d1117;
    --surface: #161b22;
    --surface-hover: #1c2129;
    --border: #30363d;
    --text: #e6edf3;
    --text-muted: #8b949e;
    --accent: #58a6ff;
    --accent-hover: #79c0ff;
    --red: #f85149;
    --green: #3fb950;
    --orange: #d29922;
    --badge-bg: rgba(0,0,0,0.75);
    --radius: 8px;
    --font: -apple-system, BlinkMacSystemFont, 'Segoe UI', Helvetica, Arial, sans-serif;
    --mono: 'SF Mono', SFMono-Regular, Consolas, 'Liberation Mono', Menlo, monospace;
  }

  html, body {
    height: 100%;
    background: var(--bg);
    color: var(--text);
    font-family: var(--font);
    font-size: 14px;
    line-height: 1.5;
    overflow: hidden;
  }

  /* ── Layout ────────────────────────────────────────────────── */
  .app {
    display: flex;
    flex-direction: column;
    height: 100vh;
  }

  .header {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 12px 20px;
    background: var(--surface);
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }

  .header-title {
    font-size: 15px;
    font-weight: 600;
    color: var(--accent);
  }

  .header-meta {
    font-size: 13px;
    color: var(--text-muted);
  }

  .header-meta span {
    margin-left: 16px;
  }

  .main-area {
    display: flex;
    flex: 1;
    overflow: hidden;
  }

  /* ── Screenshot viewer ─────────────────────────────────────── */
  .viewer {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .screen-wrap {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    position: relative;
    overflow: hidden;
    background: #000;
  }

  .screen-container {
    position: relative;
    max-width: 100%;
    max-height: 100%;
  }

  .screen-container img {
    display: block;
    max-width: 100%;
    max-height: calc(100vh - 180px);
    object-fit: contain;
    border-radius: 2px;
  }

  /* Cursor overlay */
  .cursor-dot {
    position: absolute;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: var(--red);
    border: 2px solid #fff;
    transform: translate(-50%, -50%);
    pointer-events: none;
    z-index: 10;
    box-shadow: 0 0 8px rgba(248,81,73,0.6);
    display: none;
    transition: left 0.15s ease, top 0.15s ease;
  }

  .cursor-dot.visible { display: block; }

  /* Drag line */
  .drag-line {
    position: absolute;
    pointer-events: none;
    z-index: 9;
    display: none;
  }
  .drag-line.visible { display: block; }

  /* Action badge */
  .action-badge {
    position: absolute;
    bottom: 12px;
    left: 12px;
    background: var(--badge-bg);
    color: #fff;
    padding: 6px 14px;
    border-radius: 6px;
    font-size: 13px;
    font-family: var(--mono);
    z-index: 20;
    backdrop-filter: blur(4px);
    max-width: 60%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* ── Controls bar ──────────────────────────────────────────── */
  .controls {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 20px;
    background: var(--surface);
    border-top: 1px solid var(--border);
    flex-shrink: 0;
  }

  .controls button {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--text);
    padding: 6px 14px;
    border-radius: var(--radius);
    cursor: pointer;
    font-size: 13px;
    font-family: var(--font);
    transition: background 0.15s, border-color 0.15s;
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .controls button:hover {
    background: var(--surface-hover);
    border-color: var(--text-muted);
  }

  .controls button:active { transform: scale(0.97); }

  .controls button.active {
    background: var(--accent);
    color: #000;
    border-color: var(--accent);
    font-weight: 600;
  }

  .turn-label {
    font-size: 13px;
    color: var(--text-muted);
    min-width: 110px;
    text-align: center;
    font-variant-numeric: tabular-nums;
  }

  .spacer { flex: 1; }

  /* Timeline scrubber */
  .timeline-wrap {
    flex: 1;
    min-width: 100px;
    max-width: 600px;
  }

  .timeline {
    width: 100%;
    height: 6px;
    -webkit-appearance: none;
    appearance: none;
    background: var(--border);
    border-radius: 3px;
    outline: none;
    cursor: pointer;
  }

  .timeline::-webkit-slider-thumb {
    -webkit-appearance: none;
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: var(--accent);
    cursor: pointer;
    border: 2px solid var(--bg);
    box-shadow: 0 0 4px rgba(88,166,255,0.4);
  }

  .timeline::-moz-range-thumb {
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: var(--accent);
    cursor: pointer;
    border: 2px solid var(--bg);
  }

  /* ── Side panel ────────────────────────────────────────────── */
  .side-panel {
    width: 380px;
    background: var(--surface);
    border-left: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    flex-shrink: 0;
    transition: width 0.2s ease;
    overflow: hidden;
  }

  .side-panel.collapsed { width: 0; border-left: none; }

  .panel-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 16px;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }

  .panel-header h3 {
    font-size: 13px;
    font-weight: 600;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .panel-toggle {
    background: transparent;
    border: none;
    color: var(--text-muted);
    cursor: pointer;
    font-size: 18px;
    padding: 2px 6px;
    border-radius: 4px;
  }

  .panel-toggle:hover { background: var(--surface-hover); color: var(--text); }

  .panel-body {
    flex: 1;
    overflow-y: auto;
    padding: 12px 16px;
  }

  .panel-section {
    margin-bottom: 16px;
  }

  .panel-section-title {
    font-size: 11px;
    font-weight: 600;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    margin-bottom: 6px;
  }

  .panel-action-label {
    font-family: var(--mono);
    font-size: 14px;
    color: var(--accent);
    margin-bottom: 4px;
  }

  .panel-reasoning {
    font-size: 13px;
    color: var(--text);
    line-height: 1.6;
    white-space: pre-wrap;
    word-break: break-word;
  }

  .panel-json {
    font-family: var(--mono);
    font-size: 11px;
    color: var(--text-muted);
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 10px;
    overflow-x: auto;
    white-space: pre;
    max-height: 400px;
    overflow-y: auto;
    line-height: 1.5;
  }

  /* ── Keyboard hint ─────────────────────────────────────────── */
  .kbd-hint {
    font-size: 11px;
    color: var(--text-muted);
    display: flex;
    gap: 12px;
  }

  .kbd-hint kbd {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 3px;
    padding: 1px 5px;
    font-family: var(--mono);
    font-size: 11px;
  }

  /* ── Responsive ────────────────────────────────────────────── */
  @media (max-width: 900px) {
    .side-panel { width: 280px; }
    .header-meta span { margin-left: 10px; }
  }

  @media (max-width: 700px) {
    .side-panel { position: absolute; right: 0; top: 0; bottom: 0; z-index: 100; width: 320px; }
    .side-panel.collapsed { width: 0; }
    .header { flex-wrap: wrap; gap: 8px; }
    .kbd-hint { display: none; }
  }

  /* Scrollbar styling */
  ::-webkit-scrollbar { width: 8px; height: 8px; }
  ::-webkit-scrollbar-track { background: var(--bg); }
  ::-webkit-scrollbar-thumb { background: var(--border); border-radius: 4px; }
  ::-webkit-scrollbar-thumb:hover { background: var(--text-muted); }
</style>
</head>
<body>

<div class="app">
  <!-- Header -->
  <div class="header">
    <div class="header-title">Trajectory Report</div>
    <div class="header-meta">
      <span>Machine: <strong id="meta-machine">{{MACHINE_NAME}}</strong></span>
      <span>Session: <strong>{{SESSION_DISPLAY}}</strong></span>
      <span>Turns: <strong>{{TOTAL_TURNS}}</strong></span>
    </div>
    <div class="spacer"></div>
    <div class="kbd-hint">
      <span><kbd>&larr;</kbd><kbd>&rarr;</kbd> navigate</span>
      <span><kbd>Space</kbd> play/pause</span>
      <span><kbd>J</kbd> toggle JSON</span>
    </div>
  </div>

  <div class="main-area">
    <!-- Screenshot viewer -->
    <div class="viewer">
      <div class="screen-wrap">
        <div class="screen-container" id="screen-container">
          <img id="screenshot" alt="Screenshot" src="">
          <div class="cursor-dot" id="cursor-dot"></div>
          <svg class="drag-line" id="drag-line" xmlns="http://www.w3.org/2000/svg">
            <defs>
              <marker id="arrowhead" markerWidth="8" markerHeight="6" refX="8" refY="3" orient="auto">
                <polygon points="0 0, 8 3, 0 6" fill="#f85149"/>
              </marker>
            </defs>
            <line id="drag-line-el" stroke="#f85149" stroke-width="2" stroke-dasharray="6,4" marker-end="url(#arrowhead)"/>
          </svg>
          <div class="action-badge" id="action-badge"></div>
        </div>
      </div>

      <!-- Controls -->
      <div class="controls">
        <button id="btn-prev" title="Previous turn">&larr; Prev</button>
        <button id="btn-play" title="Play / Pause">&#9654; Play</button>
        <button id="btn-next" title="Next turn">Next &rarr;</button>
        <div class="turn-label" id="turn-label">Turn 1 of {{TOTAL_TURNS}}</div>
        <div class="timeline-wrap">
          <input type="range" class="timeline" id="timeline" min="0" max="0" value="0">
        </div>
        <button id="btn-panel" title="Toggle JSON panel">{ }</button>
      </div>
    </div>

    <!-- Side panel -->
    <div class="side-panel" id="side-panel">
      <div class="panel-header">
        <h3>Turn Details</h3>
        <button class="panel-toggle" id="btn-panel-close" title="Close panel">&times;</button>
      </div>
      <div class="panel-body">
        <div class="panel-section">
          <div class="panel-section-title">Action</div>
          <div class="panel-action-label" id="panel-action"></div>
        </div>
        <div class="panel-section" id="reasoning-section" style="display:none;">
          <div class="panel-section-title">Reasoning</div>
          <div class="panel-reasoning" id="panel-reasoning"></div>
        </div>
        <div class="panel-section">
          <div class="panel-section-title">Full JSON</div>
          <pre class="panel-json" id="panel-json"></pre>
        </div>
      </div>
    </div>
  </div>
</div>

<script>
(function() {
  "use strict";

  // ── Embedded data ─────────────────────────────────────────
  var SCREENSHOTS = {{SCREENSHOTS_JSON}};
  var ACTIONS = {{ACTIONS_JSON}};
  var TOTAL = SCREENSHOTS.length;

  // ── State ─────────────────────────────────────────────────
  var currentTurn = 0;
  var playing = false;
  var playTimer = null;
  var panelOpen = true;

  // ── DOM refs ──────────────────────────────────────────────
  var imgEl = document.getElementById("screenshot");
  var cursorDot = document.getElementById("cursor-dot");
  var dragLine = document.getElementById("drag-line");
  var dragLineEl = document.getElementById("drag-line-el");
  var actionBadge = document.getElementById("action-badge");
  var turnLabel = document.getElementById("turn-label");
  var timeline = document.getElementById("timeline");
  var btnPrev = document.getElementById("btn-prev");
  var btnNext = document.getElementById("btn-next");
  var btnPlay = document.getElementById("btn-play");
  var btnPanel = document.getElementById("btn-panel");
  var btnPanelClose = document.getElementById("btn-panel-close");
  var sidePanel = document.getElementById("side-panel");
  var panelAction = document.getElementById("panel-action");
  var panelReasoning = document.getElementById("panel-reasoning");
  var reasoningSection = document.getElementById("reasoning-section");
  var panelJson = document.getElementById("panel-json");
  var screenContainer = document.getElementById("screen-container");

  // ── Init ──────────────────────────────────────────────────
  timeline.max = Math.max(0, TOTAL - 1);

  function goToTurn(n) {
    if (TOTAL === 0) return;
    currentTurn = Math.max(0, Math.min(n, TOTAL - 1));
    render();
  }

  function render() {
    // Screenshot
    var src = SCREENSHOTS[currentTurn];
    if (src) {
      imgEl.src = src;
      imgEl.style.display = "block";
    } else {
      imgEl.style.display = "none";
    }

    // Turn label
    turnLabel.textContent = "Turn " + (currentTurn + 1) + " of " + TOTAL;
    timeline.value = currentTurn;

    var info = ACTIONS[currentTurn] || {};
    var action = info.action || {};
    var atype = action.type || "";

    // Action badge
    actionBadge.textContent = info.label || atype;

    // Cursor dot and drag line — positioned after image loads
    cursorDot.classList.remove("visible");
    dragLine.classList.remove("visible");

    // Wait for image to have dimensions before placing overlays
    if (imgEl.complete && imgEl.naturalWidth > 0) {
      placeOverlays(action, atype);
    } else {
      imgEl.onload = function() { placeOverlays(action, atype); };
    }

    // Side panel
    panelAction.textContent = info.label || atype;

    if (info.reasoning) {
      reasoningSection.style.display = "block";
      panelReasoning.textContent = info.reasoning;
    } else {
      reasoningSection.style.display = "none";
    }

    panelJson.textContent = JSON.stringify(info.raw || {}, null, 2);
  }

  function placeOverlays(action, atype) {
    var imgW = imgEl.naturalWidth;
    var imgH = imgEl.naturalHeight;
    var dispW = imgEl.clientWidth;
    var dispH = imgEl.clientHeight;

    if (!imgW || !dispW) return;

    var scaleX = dispW / imgW;
    var scaleY = dispH / imgH;

    // Click / double-click / move
    if ((atype === "click" || atype === "dclick" || atype === "double_click" || atype === "move") &&
        action.x != null && action.y != null) {
      cursorDot.style.left = (action.x * scaleX) + "px";
      cursorDot.style.top = (action.y * scaleY) + "px";
      cursorDot.classList.add("visible");
    }

    // Scroll — show dot at scroll position
    if (atype === "scroll" && action.x != null && action.y != null) {
      cursorDot.style.left = (action.x * scaleX) + "px";
      cursorDot.style.top = (action.y * scaleY) + "px";
      cursorDot.classList.add("visible");
    }

    // Drag
    if (atype === "drag") {
      var sx, sy, ex, ey;
      var path = action.path;
      if (path && path.length >= 2) {
        sx = path[0].x; sy = path[0].y;
        ex = path[path.length - 1].x; ey = path[path.length - 1].y;
      } else {
        sx = action.startX || action.start_x || 0;
        sy = action.startY || action.start_y || 0;
        ex = action.endX || action.end_x || 0;
        ey = action.endY || action.end_y || 0;
      }

      var dsx = sx * scaleX, dsy = sy * scaleY;
      var dex = ex * scaleX, dey = ey * scaleY;

      // Position SVG to cover the container
      dragLine.setAttribute("width", dispW);
      dragLine.setAttribute("height", dispH);
      dragLine.style.width = dispW + "px";
      dragLine.style.height = dispH + "px";
      dragLine.style.left = "0";
      dragLine.style.top = "0";
      dragLineEl.setAttribute("x1", dsx);
      dragLineEl.setAttribute("y1", dsy);
      dragLineEl.setAttribute("x2", dex);
      dragLineEl.setAttribute("y2", dey);
      dragLine.classList.add("visible");

      // Show cursor at start of drag
      cursorDot.style.left = dsx + "px";
      cursorDot.style.top = dsy + "px";
      cursorDot.classList.add("visible");
    }
  }

  // ── Playback ──────────────────────────────────────────────
  function togglePlay() {
    playing = !playing;
    btnPlay.innerHTML = playing ? "&#9646;&#9646; Pause" : "&#9654; Play";
    btnPlay.classList.toggle("active", playing);
    if (playing) {
      playTimer = setInterval(function() {
        if (currentTurn >= TOTAL - 1) {
          togglePlay();
          return;
        }
        goToTurn(currentTurn + 1);
      }, 2000);
    } else {
      clearInterval(playTimer);
      playTimer = null;
    }
  }

  function togglePanel() {
    panelOpen = !panelOpen;
    sidePanel.classList.toggle("collapsed", !panelOpen);
  }

  // ── Event listeners ───────────────────────────────────────
  btnPrev.addEventListener("click", function() { goToTurn(currentTurn - 1); });
  btnNext.addEventListener("click", function() { goToTurn(currentTurn + 1); });
  btnPlay.addEventListener("click", togglePlay);
  btnPanel.addEventListener("click", togglePanel);
  btnPanelClose.addEventListener("click", togglePanel);

  timeline.addEventListener("input", function() {
    goToTurn(parseInt(this.value, 10));
  });

  document.addEventListener("keydown", function(e) {
    if (e.target.tagName === "INPUT" && e.target.type !== "range") return;
    switch(e.key) {
      case "ArrowLeft":
        e.preventDefault();
        goToTurn(currentTurn - 1);
        break;
      case "ArrowRight":
        e.preventDefault();
        goToTurn(currentTurn + 1);
        break;
      case " ":
        e.preventDefault();
        togglePlay();
        break;
      case "j":
      case "J":
        togglePanel();
        break;
    }
  });

  // Handle window resize — re-place overlays
  window.addEventListener("resize", function() {
    var info = ACTIONS[currentTurn] || {};
    var action = info.action || {};
    placeOverlays(action, action.type || "");
  });

  // ── Start ─────────────────────────────────────────────────
  if (TOTAL > 0) {
    goToTurn(0);
  }
})();
</script>
</body>
</html>"""
