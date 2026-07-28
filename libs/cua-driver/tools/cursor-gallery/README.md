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
