# Cursor gallery

Maintainer preview for the production `cua.default` renderer. Generated media
is built from `cursor-overlay` and is intentionally not committed.

From the repository root:

```bash
./libs/cua-driver/scripts/cursor-gallery.sh serve
./libs/cua-driver/scripts/cursor-gallery.sh export-docs
```

`serve` opens no browser and serves the gallery at `http://127.0.0.1:3001`.
`export-docs` requires Chrome, Node.js with WebSocket support, Python 3, and
ffmpeg. It regenerates the public documentation GIFs deterministically from the
same rendered frames.

The gallery starts with an interactive production cursor configurator covering
all twelve actions, optional `background` / `foreground` delivery, and optional
`ax` / `pixel` / `browser` / `desktop` targets. It then shows all fifteen badge
context states and the twelve isolated theme-owned action animations. Delivery
and target glyphs appear only in their authoritative runtime location inside
the badge.

## Motion lab

`motion-lab/` compares candidate agent-cursor motion styles side by side. It is
plain ES modules (no build) and does not need the renderer assets:

```bash
./libs/cua-driver/scripts/cursor-gallery.sh motion-lab   # http://127.0.0.1:3001/motion-lab/
./libs/cua-driver/scripts/cursor-gallery.sh motion-test  # node --test
```

- `motion/` is the motion library: a pure, seeded function from
  (candidate, scene, seed, params, timing) to timestamped points plus events
  (hover, press, release, click, scroll, think, ...). `candidates.js` holds the
  catalog; each entry documents its technique and parameters.
- The gallery (`#/`) animates every candidate on the same targets. Use the Set,
  Scene and Timing menus (`Native`, distance-aware `Fitts`, or `Fixed 1.43 s`),
  the speed slider, and Shuffle seed. Star a tile to pick it.
- `#/c/<id>` inspects one candidate with speed and acceleration plots and
  editable parameters; `#/compare` shows picked candidates and exports them as
  JSON; `#/sheet` is a static contact sheet of full trajectories.
- `capture/record.mjs` drives a headless Chrome (`?capture=1` freezes the clock)
  to take exact screenshots and frame sequences for video.

The cursor is drawn in the scene with the `cua.default` body path. The lab
never moves or restyles the real system pointer.
