# Task Registry Format

This document describes the structure of `task_registry.json` produced by `cua_bench/scripts/dump_task_registry.py`.

## Overview

- **Type**: JSON array of dataset objects
- **Location**: `cua_bench/scripts/task_registry.json`

Each element corresponds to a dataset (e.g., `cua-bench-basic`) and contains an `environments` array with metadata for each environment.

## Dataset object

- **id**: string — Dataset identifier, e.g. `cua-bench-basic`.
- **github_url**: string — Link to the dataset folder in the registry, e.g. `https://github.com/trycua/cua-bench-registry/tree/main/datasets/cua-bench-basic/`.
- **description**: string — Human-readable description of the dataset.
- **num_environments**: number — Count of discovered environments in the dataset.
- **num_tasks**: number — Sum of task counts across all environments (first-pass estimate from `@cb.tasks_config`).
- **environments**: array — List of environment objects (see below).

Example:

```json
{
  "id": "cua-bench-basic",
  "github_url": "https://github.com/trycua/cua-bench-registry/tree/main/datasets/cua-bench-basic/",
  "description": "A collection of basic, miniwob-style desktop environments for CUA agents.",
  "num_environments": 15,
  "num_tasks": 1532,
  "environments": [ /* ... */ ]
}
```

## Environment object

- **id**: string — Environment identifier (directory name), e.g. `click-button`.
- **github_url**: string — Link to the environment folder in the registry, e.g. `https://github.com/trycua/cua-bench-registry/tree/main/datasets/cua-bench-basic/click-button`.
- **description**: string|null — Description from `[tool.cua-bench].description` or `[project].description` in `pyproject.toml`.
- **num_tasks**: number — Number of tasks reported by the environment's `@cb.tasks_config`.
- **license**: string|null — From `[project].license`.
- **version**: string|null — From `[project].version`.
- **authors**: array|null — From `[project].authors`, e.g. `[{"name": "...", "email": "..."}]`.
- **difficulty**: string|null — From `[tool.cua-bench].difficulty` (e.g., `easy`, `medium`, `hard`).
- **category**: string|null — From `[tool.cua-bench].category` (e.g., `grounding`, `software-engineering`).
- **tags**: array|null — From `[tool.cua-bench].tags`.
- **previews**: array — Preview objects describing early task setups (see below).

Example:

```json
{
  "id": "click-button",
  "github_url": "https://github.com/trycua/cua-bench-registry/tree/main/datasets/cua-bench-basic/click-button",
  "description": "Click a simple button.",
  "num_tasks": 42,
  "license": "MIT",
  "version": "0.1.0",
  "authors": [{ "name": "Jane Doe", "email": "jane@example.com" }],
  "difficulty": "easy",
  "category": "grounding",
  "tags": ["click"],
  "previews": [ /* ... */ ]
}
```

## Preview object

For each environment, up to the first 5 tasks are initialized via `env.reset(task_id=i)` and saved inline.

- **index**: number — Task index used for setup.
- **screenshot**: string — Data URI of the screenshot, `image/jpeg` (quality 95) when available, otherwise `image/png`. Example: `data:image/jpeg;base64, ...`.
- **task**: object — Task configuration summary:
  - **description**: string|null — Task description.
  - **task_id**: string|number|null — Task ID (falls back to the preview index if missing).
  - **metadata**: object|null — Any task metadata provided by the environment.

Example:

```json
{
  "index": 0,
  "screenshot": "data:image/jpeg;base64,/9j/4AAQSkZJRgAB...",
  "task": {
    "description": "Click the \"Click Me\" button on the page.",
    "task_id": 0,
    "metadata": { "button_text": "Click Me", "os_type": "macos" }
  }
}
```

## Notes

- Environments are discovered in the registry at `~/cua-bench-registry/datasets/<dataset_id>/*`.
- Metadata is parsed from `pyproject.toml` if present; fields may be `null` when missing.
- Task counting and previews rely on programmatically importing environments and calling their `@cb.tasks_config`/`setup`.
- Data URIs can be rendered directly in HTML `<img src="..." />` tags.
