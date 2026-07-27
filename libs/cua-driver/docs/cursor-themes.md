# Agent cursor themes

Cua Driver ships one built-in cursor theme: `cua.default`. It is the default on
macOS, Windows, and Linux and cannot be removed. The theme uses a colored
pointer with a white outline over a larger, cursor-shaped glow in the same
session color. The glow softly fades to transparent around the full pointer
silhouette, while the white outline preserves a crisp boundary on light, dark,
and similarly colored backgrounds. Its semantic action and modifier marks are
white for reliable contrast. The
anonymous/default cursor uses Cua blue. Named sessions receive a stable fill
from the built-in session palette, so concurrent agents remain visually
distinct.

The cursor is a visual aid for people watching an agent. It is not a security
indicator, an authorization prompt, or evidence that a tool call succeeded.
Authorization is enforced separately.

## Use the default theme

Declare a session, then pass that session to cursor controls:

```bash
cua-driver start_session '{"session":"demo"}'
cua-driver set_agent_cursor_enabled \
  '{"session":"demo","enabled":true}'
cua-driver set_agent_cursor_theme \
  '{"session":"demo","theme_id":"cua.default","reduced_motion":"auto"}'
cua-driver get_agent_cursor_state '{"session":"demo"}'
```

`set_agent_cursor_motion` changes only movement physics and visibility timing.
It does not change artwork. The removed `set_agent_cursor_style` operation and
its `cursor_id`, shape, color, label, size, opacity, image-path, gradient, and
bloom styling fields are not accepted. Input-delivery tools may still use
`cursor_id` to name a virtual pointer; it does not select cursor artwork.

The default theme fill is derived from the declared `session` id. There is no
separate fill-color tool argument. Installed custom themes keep the colors
compiled into their own artwork.

The same four typed operations are available on `CuaDriver` and
`CuaDriverSession` in the Python and TypeScript SDKs:

- `set_agent_cursor_enabled`
- `set_agent_cursor_motion`
- `set_agent_cursor_theme`
- `get_agent_cursor_state`

`StartSessionInput.cursor_theme` may select an installed theme when the session
is created. `reduced_motion` is `auto`, `on`, or `off`. `auto` follows the host
accessibility preference where the platform exposes one.

## Semantic profile

A full custom theme must provide all twelve action animations:

| Action | Playback |
| --- | --- |
| `idle` | resting loop |
| `observe` | loop |
| `click` | one shot |
| `drag` | held |
| `scroll` | loop |
| `text` | held |
| `key` | one shot |
| `navigate` | one shot |
| `app` | one shot |
| `transfer` | loop |
| `record` | loop |
| `system` | one shot |

It must also provide six transparent modifier animations. The renderer
composites at most one delivery modifier and one target modifier over the
action:

- delivery: `background`, `foreground`
- target: `ax`, `pixel`, `browser`, `desktop`

Unknown tools do not invent an animation. Visual events are best effort and
never affect tool dispatch or results.

## Author a custom theme

The source is a dotLottie archive with a Cua semantic manifest:

```text
theme.lottie
├── manifest.json
├── a/
│   ├── action_idle.json
│   └── ...
└── cua/
    └── theme.json
```

Every animation must be a transparent 128×128 Lottie animation at 30 fps and
contain no more than 120 frames. A compiled theme may contain at most 1,000
frames in total across its actions and modifiers. Profile v1 supports the
bounded subset accepted by the bundled compiler. Expressions, scripts,
external URLs, fonts, images, unbounded archives, and unsupported renderer
features are rejected.

`manifest.json` must contain every referenced animation ID. A minimal
`cua/theme.json` has this shape:

```json
{
  "schema": "cua.cursor-theme/1",
  "id": "com.example.cursor.studio",
  "name": "Studio Cursor",
  "version": "1.0.0",
  "author": "Example Studio",
  "license": "MIT",
  "compatibility": {
    "profile": "cua-driver-full-v1",
    "semantics": 1
  },
  "canvas": { "width": 128, "height": 128, "fps": 30 },
  "hotspot": { "x": 55, "y": 30 },
  "actions": {
    "idle": { "animation": "action_idle", "still_frame": 0 },
    "observe": { "animation": "action_observe", "still_frame": 18 },
    "click": { "animation": "action_click", "still_frame": 8 },
    "drag": { "animation": "action_drag", "still_frame": 20 },
    "scroll": { "animation": "action_scroll", "still_frame": 16 },
    "text": { "animation": "action_text", "still_frame": 8 },
    "key": { "animation": "action_key", "still_frame": 8 },
    "navigate": { "animation": "action_navigate", "still_frame": 12 },
    "app": { "animation": "action_app", "still_frame": 18 },
    "transfer": { "animation": "action_transfer", "still_frame": 16 },
    "record": { "animation": "action_record", "still_frame": 16 },
    "system": { "animation": "action_system", "still_frame": 18 }
  },
  "modifiers": {
    "background": { "animation": "modifier_background", "still_frame": 0 },
    "foreground": { "animation": "modifier_foreground", "still_frame": 0 },
    "ax": { "animation": "modifier_ax", "still_frame": 0 },
    "pixel": { "animation": "modifier_pixel", "still_frame": 0 },
    "browser": { "animation": "modifier_browser", "still_frame": 0 },
    "desktop": { "animation": "modifier_desktop", "still_frame": 0 }
  }
}
```

The `still_frame` is used when reduced motion is active. Use a reverse-DNS
theme ID. Profile v1 does not compile dotLottie color/theme variants; publish a
visually distinct variant under a separate theme ID.

## Validate, compile, and install

Run the authoring workflow locally, outside an agent tool call:

```bash
cua-driver cursor-theme validate theme.lottie
cua-driver cursor-theme build theme.lottie --output theme.cua-theme
cua-driver cursor-theme inspect theme.cua-theme
cua-driver cursor-theme preview theme.cua-theme --output preview
cua-driver cursor-theme install theme.cua-theme
cua-driver cursor-theme list
```

Then select it by ID:

```bash
cua-driver set_agent_cursor_theme \
  '{"session":"demo","theme_id":"com.example.cursor.studio"}'
```

Remove it with:

```bash
cua-driver cursor-theme uninstall com.example.cursor.studio
```

The authoring compiler is a short-lived, unprivileged sidecar. It converts
Lottie source into bounded, premultiplied RGBA frames. The privileged overlay
loads only the compiled `.cua-theme` format; it never parses ZIP, JSON, Lottie,
fonts, expressions, URLs, or arbitrary source paths. Agent-facing tools can
select an already-installed ID but cannot install a theme or pass inline theme
data.

The embedded default is rendered directly as native vector paths for smaller
artifacts and lower idle memory. It follows the same semantic and reduced-motion
contract as compiled Lottie themes.
