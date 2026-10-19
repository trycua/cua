# CUA-Bench Basic

Basic computer-use interaction tasks for benchmarking fundamental UI automation capabilities.

## Tasks

### Button & Form Interactions

- **click-button** - Click specific buttons on a page
- **fill-form** - Fill out and submit forms with various input types

### Text & Input

- **typing-input** - Type text into input fields
- **spreadsheet-cell** - Enter data into spreadsheet cells

### Selection & Pickers

- **select-dropdown** - Select items from dropdown menus
- **color-picker** - Select specific colors from a palette
- **date-picker** - Select dates using a date picker
- **click-icon** - Click specific icons from a grid

### Controls & Widgets

- **drag-slider** - Adjust slider controls to target values
- **toggle-switch** - Enable/disable settings with toggle switches
- **video-player** - Control video playback (play, pause, volume, mute)

### Advanced Interactions

- **right-click-menu** - Right-click and select context menu options
- **drag-drop** - Drag and drop items into categories

## Task Structure

Each task folder contains:

- `main.py` - Task configuration, setup, evaluation, and solution
- `gui/index.html` - UI interface with Tailwind styling
- `pyproject.toml` - Project metadata and dependencies
- `CLAUDE.md` - cua-bench scaffolding documentation

## Running Tasks

```bash
# Run a specific task interactively
python -m cua_bench.interact <task-folder>/main.py

# Example
python -m cua_bench.interact click-button/main.py
```

## Image

The tasks open their page in a bench-ui (pywebview) window, so they run on
the `bench-web` image (`cua_bench.images.BENCH_WEB`), which ships bench-ui,
pywebview and WebKit2GTK. It has a container variant (docker, gVisor) and a
VM variant (QEMU, KubeVirt) in one index; `--runtime` picks one. Override
the image with `--image` or `CUA_BENCH_IMAGE_BENCH_WEB`.

```bash
# The whole set with the scripted solutions, locally in a container
cb run dataset libs/cua-bench/datasets/cua-bench-basic --oracle
# The same on a local VM, and on Fleet
cb run dataset libs/cua-bench/datasets/cua-bench-basic --oracle --kind vm
cb run dataset libs/cua-bench/datasets/cua-bench-basic --oracle --on cloud
```
