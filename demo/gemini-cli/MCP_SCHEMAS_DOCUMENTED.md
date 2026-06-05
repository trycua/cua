# cua-driver-rs MCP Server - Tool Schemas and Response Formats

Extracted from cua-driver.exe v0.5.0 on 2026-06-03

## Connection Protocol

1. Send `initialize` request with `protocolVersion: "2024-11-05"`
2. Wait for initialize response
3. Send `notifications/initialized` notification (no response expected)
4. Call tools via `tools/call` method

## Response Format

All tool responses follow this structure:

```json
{
  "jsonrpc": "2.0",
  "id": <request_id>,
  "result": {
    "content": [
      {
        "type": "text" | "image",
        "text": "..." | (for text type)
        "data": "base64..." | (for image type)
        "mimeType": "image/png" (for image type)
      }
    ],
    "structuredContent": {
      // Tool-specific structured data
    }
  }
}
```

**Key Insight**: `structuredContent` contains parsed, structured data for programmatic use. `content` contains human-readable text or binary data (like screenshots).

## Core Tools

### 1. list_windows

**Purpose**: List all top-level windows currently known to the window manager.

**Input Schema**:
```json
{
  "type": "object",
  "additionalProperties": false,
  "properties": {
    "pid": {
      "type": "integer",
      "description": "Optional pid filter. When set, only this pid's windows are returned."
    },
    "on_screen_only": {
      "type": "boolean",
      "description": "When true, drop windows that aren't currently on-screen. Default false."
    }
  }
}
```

**Response Format**:
- `content[0].type`: "text"
- `content[0].text`: Human-readable list like:
  ```
  ✅ Found 18 window(s) across 12 app(s); 18 on-screen. (SkyLight Space SPIs unavailable — on_current_space / space_ids omitted.)
  - chrome.exe (pid 26452) "YouTube - Google Chrome" [window_id: 67124]
  - Code.exe (pid 24888) "README.md - Visual Studio Code" [window_id: 3871452]
  ...
  ```
- `structuredContent`: NOT provided (text-only response)

**Important**: This returns human-readable text, NOT JSON. Parse the text to extract pid and window_id values.

### 2. get_window_state

**Purpose**: Walk a running app's UIA tree and return a Markdown rendering of its UI, tagging every actionable element with [element_index N]. Also captures a screenshot.

**Input Schema**:
```json
{
  "type": "object",
  "additionalProperties": false,
  "required": ["pid", "window_id"],
  "properties": {
    "pid": {
      "type": "integer",
      "description": "Process ID from list_apps."
    },
    "window_id": {
      "type": "integer",
      "description": "HWND of the target window. Must belong to pid. Enumerate via list_windows or read from launch_app's windows array."
    },
    "capture_mode": {
      "type": "string",
      "enum": ["som", "vision", "ax"],
      "description": "som=tree+screenshot (default), vision=screenshot only, ax=tree only."
    },
    "query": {
      "type": "string",
      "description": "Optional case-insensitive substring. When set, tree_markdown only contains lines that match plus their ancestor chain; element indices and element_count are unchanged."
    }
  }
}
```

**Response Format**:

**capture_mode="som" (default)** - Tree + Screenshot:
- `content[]` array with TWO items:
  - `content[0]`: `{type: "text", text: "✅ Snapped..."}`
  - `content[1]`: `{type: "image", data: "iVBORw0KGgoAAAA...", mimeType: "image/png"}`
- `structuredContent`:
  ```json
  {
    "pid": 12345,
    "window_id": 67124,
    "element_count": 56,
    "screenshot_width": 1421,
    "screenshot_height": 657,
    "tree_markdown": "- Window \"Title\"\n  - [0] Button \"OK\" [actions=[invoke]]..."
  }
  ```

**capture_mode="vision"** - Screenshot only:
- `content[]` array with ONE item:
  - `content[0]`: `{type: "image", data: "iVBORw0KGgoAAAA...", mimeType: "image/png"}`
- `structuredContent`:
  ```json
  {
    "pid": 12345,
    "window_id": 67124,
    "screenshot_width": 1421,
    "screenshot_height": 657
  }
  ```

**capture_mode="ax"** - Tree only:
- `content[0]`: `{type: "text", text: "..."}`
- `structuredContent`:
  ```json
  {
    "pid": 12345,
    "window_id": 67124,
    "element_count": 56,
    "tree_markdown": "..."
  }
  ```

**Screenshot Format**: 
- Base64-encoded PNG image
- Coordinates are **window-relative** (top-left origin)
- This is the coordinate space used for `click(x, y)` pixel coordinates

### 3. click

**Purpose**: Left-click against a target pid. Supports both element_index (UIA) and pixel coordinates.

**Input Schema**:
```json
{
  "type": "object",
  "additionalProperties": false,
  "required": ["pid"],
  "properties": {
    "pid": {
      "type": "integer",
      "description": "Target process ID."
    },
    "window_id": {
      "type": "integer",
      "description": "HWND for the window whose get_window_state produced the element_index. Required when element_index is used."
    },
    "element_index": {
      "type": "integer",
      "description": "Element index from the last get_window_state for the same (pid, window_id)."
    },
    "x": {
      "type": "number",
      "description": "X in window-local screenshot pixels — same space as the PNG get_window_state returns. Must be provided together with y."
    },
    "y": {
      "type": "number",
      "description": "Y in window-local screenshot pixels. Must be provided together with x."
    },
    "button": {
      "type": "string",
      "enum": ["left", "right", "middle"],
      "description": "Mouse button. Windows-only convenience; Swift exposes right-click as a separate right_click tool."
    },
    "count": {
      "type": "integer",
      "minimum": 1,
      "maximum": 3,
      "description": "Click count — 1 (single), 2 (double), 3 (triple). Default 1."
    },
    "dispatch": {
      "type": "string",
      "enum": ["background", "foreground", "auto"],
      "default": "background",
      "description": "Dispatch mode. 'background' (default) never swaps foreground: it routes through UIA Invoke / PostMessage..."
    },
    "from_zoom": {
      "type": "boolean",
      "description": "When true, x and y are pixel coordinates in the last zoom image for this pid. The driver maps them back to window coords."
    }
  }
}
```

**Addressing Modes**:
1. **Element-based** (preferred): Provide `element_index` + `window_id`
   - Uses UIA Invoke pattern via PostMessage
   - Works on backgrounded/hidden/minimized windows
   - No focus steal
   
2. **Pixel-based**: Provide `x` + `y` (window-relative coordinates)
   - First tries UIA hit-test at that position
   - Falls back to PostMessage(WM_LBUTTONDOWN/UP)
   - Coordinates are relative to the screenshot PNG from get_window_state

**Coordinate System**: Window-relative, top-left origin (0,0 = top-left of window content area shown in screenshot)

**Response Format**:
- `content[0].type`: "text"
- `content[0].text`: Success message like "✅ Clicked element [5] Button 'OK'"
- `structuredContent`: May contain click confirmation details

### 4. type_text

**Purpose**: Insert text into the target pid via character-by-character PostMessage(WM_CHAR) or UIA ValuePattern.SetValue.

**Input Schema**:
```json
{
  "type": "object",
  "additionalProperties": false,
  "required": ["pid", "text"],
  "properties": {
    "pid": {
      "type": "integer",
      "description": "Target process ID."
    },
    "text": {
      "type": "string",
      "description": "Text to insert at the focused element's cursor."
    },
    "window_id": {
      "type": "integer",
      "description": "HWND of the target window. Required when element_index is used."
    },
    "element_index": {
      "type": "integer",
      "description": "Optional element_index. Accepted for parity; currently no-op on Windows."
    },
    "delay_ms": {
      "type": "integer",
      "minimum": 0,
      "maximum": 200,
      "description": "Milliseconds between characters. Default 30."
    },
    "dispatch": {
      "type": "string",
      "enum": ["background", "foreground", "auto"],
      "default": "background"
    }
  }
}
```

**Important**: For XAML/WinUI3/UWP apps (modern Notepad, Calculator, etc.), you MUST provide `element_index` + `window_id` because PostMessage WM_CHAR doesn't reach those hosts.

**Response Format**:
- `content[0].type`: "text"
- `content[0].text`: Success message

### 5. press_key

**Purpose**: Press and release a single key, delivered directly to the target pid's top-level window via PostMessage(WM_KEYDOWN/WM_KEYUP).

**Input Schema**:
```json
{
  "type": "object",
  "additionalProperties": false,
  "required": ["pid", "key"],
  "properties": {
    "pid": {
      "type": "integer",
      "description": "Target process ID."
    },
    "key": {
      "type": "string",
      "description": "Key name (return, tab, escape, up, down, left, right, space, delete, home, end, pageup, pagedown, f1-f12, letter, digit)."
    },
    "window_id": {
      "type": "integer",
      "description": "HWND for the target window. Required when element_index is used; otherwise auto-resolves the pid's first visible window."
    },
    "element_index": {
      "type": "integer",
      "description": "Optional element_index. Accepted for parity; currently no-op on Windows."
    },
    "modifiers": {
      "type": "array",
      "items": {"type": "string"},
      "description": "Optional modifier names held while the key is pressed (ctrl/shift/alt/win)."
    },
    "dispatch": {
      "type": "string",
      "enum": ["background", "foreground", "auto"],
      "default": "background"
    }
  }
}
```

**Key Vocabulary**: 
- Special keys: return, tab, escape, up, down, left, right, space, delete, home, end, pageup, pagedown, f1-f12
- Letters: a-z
- Digits: 0-9

**Response Format**:
- `content[0].type`: "text"
- `content[0].text`: Success message

## Additional Tools

### get_cursor_position

**Input**: No parameters required

**Response**:
- `content[0].type`: "text"
- `content[0].text`: "✅ Cursor at (555, 304)"
- `structuredContent`:
  ```json
  {
    "x": 555,
    "y": 304
  }
  ```

**Coordinate System**: Screen coordinates (absolute)

### launch_app

**Input Schema** (simplified):
```json
{
  "properties": {
    "name": {"type": "string"},
    "path": {"type": "string"},
    "aumid": {"type": "string"},
    "bundle_id": {"type": "string"},
    "launch_path": {"type": "string"},
    "additional_arguments": {"type": "array", "items": {"type": "string"}},
    "start_minimized": {"type": "boolean"}
  }
}
```

**Response**:
- `content[0].type`: "text"
- `content[0].text`: "✅ Launched Microsoft.WindowsNotepad_8wekyb3d8bbwe!App (pid 50608) in background.\n\nWindows:\n- \"Untitled - Notepad\" [window_id: 857714]..."
- `structuredContent`:
  ```json
  {
    "pid": 50608,
    "name": "Notepad",
    "active": false,
    "bundle_id": "Microsoft.WindowsNotepad_8wekyb3d8bbwe!App",
    "windows": [
      {
        "window_id": 857714,
        "title": "Untitled - Notepad",
        "bounds": {"x": 100, "y": 200, "width": 800, "height": 600},
        "is_on_screen": true
      }
    ]
  }
  ```

## Other Tools Available

- `double_click` - Same parameters as click
- `right_click` - Same parameters as click  
- `drag` - From (from_x, from_y) to (to_x, to_y)
- `scroll` - Scroll direction, amount, granularity
- `hotkey` - Press key combinations like ["ctrl", "c"]
- `set_value` - Set UIA element value directly
- `move_cursor` - Move the agent cursor overlay
- `screenshot` - Capture screen/window
- `zoom` - Zoom into a region for detailed view
- `list_apps` - List all running and installed apps
- `kill_app` - Force-terminate a process
- `bring_to_front` - Activate a window (breaks no-foreground contract)
- `start_session` / `end_session` - Session management for agent cursor
- `start_recording` / `stop_recording` - Trajectory recording
- `page` - Interact with browser pages (CDP/AppleScript)
- Various cursor configuration tools

## Key Quirks and Best Practices

1. **Coordinates are window-relative**: The x,y coordinates in click() are relative to the top-left of the window content area shown in the screenshot, NOT screen-absolute.

2. **Screenshot format**: Base64-encoded PNG in the `content` array as type "image", NOT in structuredContent.

3. **Element indices are ephemeral**: They only last until the next get_window_state() call for that (pid, window_id).

4. **list_windows returns text, not JSON**: You must parse the human-readable text to extract pid and window_id values.

5. **Prefer element_index over pixel coords**: Element-based clicks work on backgrounded/hidden windows and are more stable.

6. **XAML/UWP apps need element_index**: For modern Windows apps (Notepad, Calculator), type_text requires element_index + window_id.

7. **Background dispatch by default**: All input tools default to dispatch="background" which never steals focus.

8. **structuredContent is the API contract**: While content[] provides human-readable output, structuredContent contains the machine-readable data you should parse programmatically.

## Example Workflow

```python
# 1. Launch app
response = call_tool("launch_app", {"name": "notepad"})
pid = response["result"]["structuredContent"]["pid"]
window_id = response["result"]["structuredContent"]["windows"][0]["window_id"]

# 2. Get window state (tree + screenshot)
response = call_tool("get_window_state", {
    "pid": pid,
    "window_id": window_id,
    "capture_mode": "som"
})

# Extract tree and screenshot
tree = response["result"]["structuredContent"]["tree_markdown"]
screenshot_base64 = response["result"]["content"][1]["data"]  # content[1] is the image

# 3. Click on element (find element_index from tree)
# Look for: [5] Button "OK" [actions=[invoke]]
response = call_tool("click", {
    "pid": pid,
    "window_id": window_id,
    "element_index": 5
})

# 4. Or click at pixel coordinates (window-relative)
response = call_tool("click", {
    "pid": pid,
    "x": 100,
    "y": 200
})

# 5. Type text (for XAML apps, need element_index)
response = call_tool("type_text", {
    "pid": pid,
    "text": "Hello World",
    "window_id": window_id,
    "element_index": 0  # The text editor element
})
```
