# Cua Perception Synthetic Quality Corpus

This directory contains nine deterministic synthetic screenshots for evaluating
perception behavior without redistributing application screenshots or user data.
The scenarios cover native controls, browser canvas content, a remote-desktop-like
surface, light and dark themes, high-DPI and downsampled rendering, small text,
icon-only controls, overlapping OCR/control boxes, an empty surface, and noise.

`manifest.json` is the closed corpus contract. Coordinates are integer
`[x, y, width, height]` boxes in each PNG's source pixels, with the origin at the
top left. It records each image's SHA-256 digest and dimensions, scenario tags,
and expected text and control annotations. Metric definitions describe how a
consumer can report results; the corpus intentionally sets no quality floors.
The canonical scorer matches detections one-to-one at `IoU >= 0.5`. Its result
kind mapping treats both `control` and `icon` regions as control detections so
generic Rust regions and Python `cua-som` icon-only output share a denominator,
but leaves the annotated subtype unknown unless the producer explicitly
supplies `control_kind`.

Regenerate the checked-in PNGs and manifest with:

```sh
python3 generate.py
```

Verify schema semantics, coverage, hashes, dimensions, annotation bounds, the
closed image set, and byte-for-byte reproducibility with:

```sh
python3 generate.py --check
```

Generation uses only the Python standard library. Do not hand-edit generated
PNGs or `manifest.json`; change `generate.py`, regenerate, and rerun the check.

All material in this directory is Cua-authored synthetic test data released
under the MIT license in `LICENSE`. See `PROVENANCE.md` for provenance details.
