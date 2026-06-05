# Gemini 3.5 Flash Computer Use CLI

Correct implementation following the EAP Guide PDF and cua-driver-rs MCP schemas.

## Architecture

1. **Window Scoping**: Select target window with `@window-name`
2. **Gemini Computer Use**: Uses function declarations for desktop actions
3. **Coordinate Translation**: Gemini 0-999 normalized → cua-driver window-relative pixels
4. **Screenshot Handling**: MCP image content → base64 PNG with dimensions

## Gemini API Integration

Following the PDF spec:
- Model: `gemini-3.5-flash`
- Environment: `DESKTOP` (implied by function declarations)
- Actions: `take_screenshot`, `click`, `type`, `press_key`, `hotkey`, `scroll`, `drag_and_drop`, `wait`
- All actions require `intent` parameter explaining reasoning

## cua-driver-rs Integration

Correct MCP tool usage:
- `list_windows` → parse text output (not JSON)
- `get_window_state(capture_mode="som")` → screenshot in `content[1].data` (base64 PNG)
- `click(pid, window_id, x, y)` → window-relative pixels
- `type_text(pid, window_id, text)`
- `press_key(pid, window_id, key, modifiers)`

## Coordinate System

**Gemini → cua-driver translation:**
```
normalized_x (0-999) → pixel_x = (normalized_x / 999) * screenshot_width
normalized_y (0-999) → pixel_y = (normalized_y / 999) * screenshot_height
```

Window screenshot dimensions from `get_window_state` structured_content.

## Usage

```bash
avgdemo
```

1. Type `@spotify` to select Spotify window
2. Screenshot is captured automatically
3. Ask "press play" - Gemini uses computer actions scoped to that window

## Implementation Verified

- ✅ PDF spec: Gemini function call format with intent
- ✅ MCP schema: Correct tool parameters and response parsing
- ✅ Coordinate translation: 0-999 → window pixels
- ✅ Screenshot format: Base64 PNG from MCP image content
- ✅ Window scoping: All actions target selected (pid, window_id)
