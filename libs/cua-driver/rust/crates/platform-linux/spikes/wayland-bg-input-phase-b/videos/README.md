# Demo recordings (webm)

Real screen captures of the libei/EIS background-injection pipeline, recorded
with `wf-recorder` off the compositor's output (tinywl + `wlr-screencopy`).
Windows are GTK4 ink canvases that stroke the pointer path they receive; the
input is driven entirely by a libei client through the EIS server — no window
focused/raised, no real pointer moved.

| File | Use case |
|---|---|
| `16-window-cursive-multicursor.webm` | 16 background windows, 16 independent cursors, concurrent cursive (scale). |
| `4-window-independent-strokes.webm` | 4 windows, each a distinct stroke (independence, readable). |
| `1-window-precise-freehand.webm` | one large window, slow precise freehand drag (fidelity). |

`preview-*.png` are single frames from each clip.
