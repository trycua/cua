# Cursor gallery

Maintainer preview for the production `cua.default` renderer. Generated media
is built from `cursor-overlay` and is intentionally not committed.

From the repository root:

```bash
./libs/cua-driver/scripts/cursor-gallery.sh dev
./libs/cua-driver/scripts/cursor-gallery.sh serve
./libs/cua-driver/scripts/cursor-gallery.sh export-docs
```

`dev` starts a repository-only animation workbench at
`http://127.0.0.1:3001`. It watches the default theme source generator at
`rust/crates/cursor-overlay/assets/build_default_theme.py`, compiles its output,
and rebuilds exact production-renderer previews. A failed edit is shown in the
diagnostics panel while the last valid render remains available. It also adds
timeline scrubbing, isolated/runtime scenes, a movement scene driven by the
renderer’s production `MoveTo` path, default speed profile, and host-platform
motion tick. On macOS the movement export uses the runtime's 16 ms render
budget, fixed 45-degree arrival heading, first-action seed, click offset, and
arrival-driven command sequencing. It also includes reduced-motion stills,
background checks, and comparison with the previous valid build. The workflow does not add
commands or options to the installed `cua-driver` executable.

`serve` opens no browser and serves the gallery at `http://127.0.0.1:3001`.
`export-docs` requires Chrome, Node.js with WebSocket support, Python 3, and
ffmpeg. It regenerates the public documentation GIFs deterministically from the
same rendered frames.

The gallery starts with the focused animation workbench, followed by an
interactive production cursor configurator covering all twelve actions,
optional `background` / `foreground` delivery, and optional `ax` / `pixel` /
`browser` / `desktop` targets. It then shows all fifteen badge context states
and the twelve isolated theme-owned action animations. Delivery and target
glyphs appear only in their authoritative runtime location inside the badge.
