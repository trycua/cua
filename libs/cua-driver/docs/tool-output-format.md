# cua-driver MCP Tool Output Format

Every tool call returns a ✅ checkmark + concise summary. No structured JSON output.

## screenshot capture

There is no standalone screenshot tool. `get_window_state` returns a
window screenshot alongside its accessibility tree, `get_desktop_state`
returns a full-display screenshot, and `zoom` returns a cropped region.

## get_window_state
```
✅ Blender — 11 elements, turn 3 + screenshot
⚠️  Small AX tree (11 elements) — this app likely uses custom rendering
    (e.g. Blender, games, Electron). Use pixel clicks: click(pid, x, y)
    with coordinates from the screenshot.

- AXApplication "Blender"
  - [0] AXWindow "* Untitled - Blender 5.1.1" actions=[AXRaise]
    - [1] AXButton
    ...
```

## click
```
✅ Posted click to pid 6808.
```

## zoom
```
✅ Zoomed region captured at native resolution. To click a target in
this image, use `click(pid, x, y, from_zoom=true)` where x,y are pixel
coordinates in THIS zoomed image — the driver maps them back automatically.
```
