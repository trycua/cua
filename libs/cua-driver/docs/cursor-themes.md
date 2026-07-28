# Agent cursor themes

Cua Driver ships one built-in cursor theme: `cua.default`. It is the default on
macOS, Windows, and Linux and cannot be removed. The theme uses a colored
pointer with a white outline over a larger, cursor-shaped glow in the same
session color. The glow softly fades to transparent around the full pointer
silhouette, while the white outline preserves a crisp boundary on light, dark,
and similarly colored backgrounds. Its semantic action and modifier marks use
the same session-colored center and white-outline treatment as the pointer,
plus a tighter, softer glow so they remain legible without competing with it.
Every semantic state inherits the same gentle levitation and rotation as the
idle pointer, with its action-specific motion layered on top. The pointer,
semantic mark, and active modifiers therefore move as one visual unit.
Reduced-motion mode removes this shared movement. The anonymous/default cursor
uses Cua blue. Named sessions receive a stable fill
from the built-in session palette, so concurrent agents remain visually
distinct.

The native renderer places the sanitized public session name in a compact badge
below the built-in pointer. The badge uses a session-accent gradient and a
circular session marker. It appears when the cursor is first revealed, remains
fully visible for two seconds, then fades over 400 milliseconds while the
cursor stays available. Hiding and revealing the cursor shows the badge again.
The badge is not part of the dotLottie theme artifact, so custom theme authors
do not need to provide badge artwork or a font. Cua Driver strips control
characters, collapses whitespace, and shortens labels longer than 28 characters
before rendering the bundled Inter typeface.

The production pointer footprint is 42 logical points. Actions with a resolved
screen target, including indexed or token-addressed text and value operations,
move the agent cursor to that target before dispatch. Actions without a spatial
target, such as typing into the already focused desktop application, keep the
cursor at its current location and only change the semantic animation.

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

| Action     | Playback     |
| ---------- | ------------ |
| `idle`     | resting loop |
| `observe`  | loop         |
| `click`    | one shot     |
| `drag`     | held         |
| `scroll`   | loop         |
| `text`     | held         |
| `key`      | one shot     |
| `navigate` | one shot     |
| `app`      | one shot     |
| `transfer` | loop         |
| `record`   | loop         |
| `system`   | one shot     |

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
frames in total across its actions and modifiers. The compiler samples
animation timing at 30 fps but preserves the artwork as vector paths, ellipses,
rounded rectangles, solid fills, strokes, and transforms. The overlay then
rasterizes those commands at the display's live backing scale.

Profile v1 accepts shape layers with static path geometry and animated layer
position, scale, rotation, opacity, solid color, and stroke width. Shape-layer
transforms must remain at their identity values; place animation on the layer
transform instead. Groups, animated path geometry, expressions, scripts,
external URLs, fonts, images, nested assets, masks, effects, parented layers,
unbounded archives, and other unsupported Lottie features are rejected.

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
Lottie source into bounded vector frames containing only validated geometry,
paint, and transforms. The privileged overlay loads only the compiled
`.cua-theme` format and rasterizes those commands through Skia at the requested
backing scale. It never parses ZIP, JSON, Lottie, fonts, expressions, URLs, or
arbitrary source paths. Agent-facing tools can select an already-installed ID
but cannot install a theme or pass inline theme data.

The built-in default follows the same path. Its canonical
`cua.default.lottie` source and reproducible generator are checked into
`cursor-overlay/assets/`. The resulting bounded `cua.default.cua-theme` is
embedded in the binary and decoded by the same renderer used for installed
custom themes. Only the embedded default receives the runtime session-color
tint and shared float transform. The artifact contains no fixed-resolution
pixel atlas, so the cursor stays crisp at 1×, 2×, 3×, and fractional display
scales.

From the Rust workspace, contributors can reproduce both checked-in artifacts:

```bash
python3 crates/cursor-overlay/assets/build_default_theme.py
cargo run -p cursor-theme-cli -- build \
  crates/cursor-overlay/assets/cua.default.lottie \
  --output crates/cursor-overlay/assets/cua.default.cua-theme
```

A unit test verifies that the compiled artifact's source hash matches the
checked-in dotLottie archive.

## Preview the production renderer

Contributors can inspect every built-in state in the static cursor gallery. The
gallery uses WebM assets generated from `cursor-overlay`, so its pixels and
timing come from the production renderer rather than a browser reimplementation.

From the repository root:

```bash
./libs/cua-driver/scripts/cursor-gallery.sh serve
```

Open `http://127.0.0.1:3001` to pause, replay, change speed, compare the states
on light, dark, blue, or mixed backgrounds, and inspect the exact session-badge
hold and fade behavior. The generated WebM files and raw frames are ignored by
Git.

Regenerate the public documentation GIFs after changing the built-in cursor:

```bash
./libs/cua-driver/scripts/cursor-gallery.sh export-docs
```

Commit both GIFs in `docs/public/img/cua-driver/cursor-themes/` with the renderer
change. The export requires Chrome, Node.js with WebSocket support, Python 3,
and ffmpeg.
