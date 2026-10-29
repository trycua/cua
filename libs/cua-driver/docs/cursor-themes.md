# Agent cursor themes

Cua Driver ships one built-in cursor theme: `cua.default`. It is the default on
macOS, Windows, and Linux and cannot be removed. By default the agent cursor is
a plain, upright arrow: a colored pointer with a white outline over a soft,
cursor-shaped glow in the same session color. The arrow stands upright like the
system pointer, with its left edge vertical, and keeps that pose while it moves.
The glow softly fades to transparent around the full pointer silhouette, while
the white outline preserves a crisp boundary on light, dark, and similarly
colored backgrounds. The anonymous/default cursor uses Cua blue. Named sessions
receive a stable fill from the built-in session palette, so concurrent agents
remain visually distinct.

A resting cursor levitates: it rises about 4 pt and settles back on a
four-second loop, straight up and down, so the arrow stays upright. The cursor
fades out after its idle-hide delay. Turn the float off with
`cua-driver config set cursor.float off`; a resting cursor is then still, so
the macOS, Windows, X11, and Wayland overlays stop rendering once activity
settles.

### Extras

Apart from the float, everything beyond the plain arrow is off by default. Each
extra is a saved setting in `~/.cua-driver/config.json` that applies at once to
every cursor on screen. Turn one on with `cua-driver config set <key> on`, or
with `set_config`:

| Key | What it adds |
| --- | --- |
| `cursor.marks` | Action marks: a small cue next to the arrow while the agent observes, clicks, types, presses keys, scrolls, drags, manages apps, moves files, records, or makes system calls. Moves and clicks show the navigate and click marks. On macOS, a clicked element also gets a brief focus box. |
| `cursor.float` | On by default. The resting float: a slow vertical bob on a four-second loop while the cursor rests, until the idle fade hides it. The arrow never tilts. |
| `cursor.rotate` | Rotation: the arrow turns toward the direction it moves and settles upright on arrival. |
| `cursor.motion.effects.glow` | A soft glow behind the pointer that grows with speed. |
| `cursor.motion.effects.ripple` | A ring that expands from each click point. |
| `cursor.motion.effects.squish` | The pointer shrinks briefly on each click and springs back. |

The motion effects can also be set per session with `set_agent_cursor_motion`
(see below). Every semantic mark inherits the float when both are on, so the
pointer and mark move as one visual unit. Reduced-motion mode removes the
float, rotation, and every motion effect, and keeps marks on their still frame.
With `cursor.marks` off, custom themes also paint only their `idle` animation.

The badge below the pointer follows the `cursor.badges` setting, which applies
to every cursor at once:

- `minimal` (default): only the session name, and only while the human pointer
  hovers over the cursor or while two or more agent cursors are on screen.
  Nothing shows while the cursor moves. The GNOME Shell helper draws one
  cursor and has no hover, so under `minimal` it shows no badge.
- `verbose`: the earlier badge, described below, plus a short chip for each key
  combo (such as ⌘C). The chip waits about 0.1 seconds, holds for half a
  second and fades out, about one second in all. It plays once its action has
  ended and the cursor is at rest.
- `off`: no badge.

In `verbose` mode the native renderer places the sanitized public session name
and execution context in a compact badge below the built-in pointer. The badge uses a
session-accent gradient and a circular session marker. The session name appears
when the cursor is first revealed, remains fully visible for two seconds, then
fades over 400 milliseconds while the cursor stays available. Moving the human
pointer over the synthetic cursor temporarily reveals the name again, and
moving it away hides the name.
Hiding and revealing the cursor also shows the badge again. Hover reveal uses
the native global-pointer APIs on macOS, Windows, and X11. Stock Wayland does
not expose another client's global pointer position, so native Wayland keeps
the timed reveal but cannot offer hover reveal without a compositor-owned
adapter. The click-through overlay never captures pointer input to fake this
capability.

Delivery and target context appears as up to two chips inside the same badge.
The delivery chip comes first and is filled; the target chip comes second and
is outlined. Chips stay visible for the active action and fade over 400
milliseconds when it ends. They do not reveal a session name that has already
faded. An unlabeled session gets a compact marker-and-chips badge.

The badge and its context glyphs are not part of the dotLottie theme artifact,
so custom theme authors do not need to provide badge artwork or a font. Cua
Driver strips control
characters, collapses whitespace, and shortens labels longer than 28 characters
before rendering the bundled Inter typeface.

The production pointer footprint is 42 logical points. Actions with a resolved
screen target, including indexed or token-addressed text and value operations,
move the agent cursor to that target before dispatch. Actions without a spatial
target, such as typing into the already focused desktop application, keep the
cursor at its current location and only change the semantic animation.
During a drag, the native gesture's endpoints, step count, and wall-clock
duration drive the same shared pointer-anchor transform on macOS, Windows, X11,
and supported Wayland input routes, so the synthetic cursor follows the held
gesture instead of moving only after release.

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

`get_agent_cursor_state` always includes `position`. It is `null` until the
session cursor first moves, then `{"x": ..., "y": ...}`: the screen point the
cursor tip was last placed on.

Keyboard actions place a named session's cursor before the input.
`press_key`, `hotkey`, and `type_text` put it on the targeted element, else
at its last position, else at the target window's centre, so a session whose
first action is a keyboard action shows its cursor on that window. Anonymous
calls keep their existing cursor behavior.

`set_agent_cursor_motion` changes how the cursor moves and when it hides. It
does not change artwork. `style` picks the trajectory: `signature_arc` (the
default), `spring_settle`, `magnetic`, `comet_swoop`, `adaptive`, or `classic`
(the previous Dubins glide). `timing` is `native` (the style's own), `fitts`
(scales with distance and target size), or `fixed` (`glide_duration_ms`, 1430
ms when 0). `effects` turns `trail`, `glow`, `magnet`, `ripple`, and `squish`
on, off, or `default` (the style's own setting). `signature_arc` turns none of
them on by default; `spring_settle`, `magnetic`, `comet_swoop` and `adaptive`
keep their own. Omit an effect to keep its current setting. From a shell, `cua-driver cursor
motion --session <label> --style magnetic` sets the same fields. The removed `set_agent_cursor_style` operation and
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

`StartSessionInput.cursor_motion` sets the session's motion (the fields of
`set_agent_cursor_motion` without `session`) before the cursor is first shown,
and `start_session` echoes it. A saved default comes from
`cua-driver config set cursor.motion.style|timing|effects.<name>`. Precedence:
`set_agent_cursor_motion`, then `start_session.cursor_motion`, then the saved
default, then the built-in `signature_arc`. Reduced motion wins over all of
them.

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

Delivery and target context is host-owned rather than theme-owned:

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
frames in total across its actions. The compiler samples
animation timing at 30 fps but preserves the artwork as vector paths, ellipses,
rounded rectangles, solid fills, strokes, and transforms. The overlay then
rasterizes those commands at the display's live backing scale.

Profile v2 accepts shape layers with static path geometry and animated layer
position, scale, rotation, opacity, solid color, and stroke width. Shape-layer
transforms must remain at their identity values; place animation on the layer
transform instead. Groups, animated path geometry, expressions, scripts,
external URLs, fonts, images, nested assets, masks, effects, parented layers,
unbounded archives, and other unsupported Lottie features are rejected.

`manifest.json` must contain every referenced animation ID. A minimal
`cua/theme.json` has this shape:

```json
{
  "schema": "cua.cursor-theme/2",
  "id": "com.example.cursor.studio",
  "name": "Studio Cursor",
  "version": "1.0.0",
  "author": "Example Studio",
  "license": "MIT",
  "compatibility": {
    "profile": "cua-driver-actions-v2",
    "semantics": 2
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
  }
}
```

The `still_frame` is used when reduced motion is active. Use a reverse-DNS
theme ID. Profile v2 does not compile dotLottie color/theme variants; publish a
visually distinct variant under a separate theme ID.

Profile v2 is intentionally breaking. V1 source manifests and compiled
artifacts are rejected with rebuild guidance. Remove the `modifiers` section,
set the schema and semantic version to 2, use the
`cua-driver-actions-v2` profile, and rebuild the artifact.

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
tint and shared float transform. Delivery and target chips remain consistent
because the host badge renders them for every theme. The artifact contains no fixed-resolution
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
