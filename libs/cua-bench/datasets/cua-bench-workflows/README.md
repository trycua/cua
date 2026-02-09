# CUA-Bench Workflows

Workflow-based computer-use tasks that involve real desktop applications and multi-step interactions.

## Tasks

### Creative Applications

- **photoshop-tasks** - Adobe Photoshop document creation and manipulation tasks
  - Create new documents with text layers
  - Open PSD files and analyze layer structure
  - Save and export documents

### Game Development

- **unity-tasks** - Unity game engine workflow tasks
  - Create new projects
  - Add GameObjects and scripts
  - Build projects and configure settings

## Task Structure

Each task folder contains:

- `main.py` - Task configuration, setup, evaluation, and solution
- `assets/` - Required files for the task (PSD files, images, etc.)

## Requirements

These tasks require native application support:

- **Windows**: Adobe Photoshop (native installation)
- **Linux**: Adobe Photoshop via WINE (see adobe_photoshop.py app definition)

## Running Tasks

```bash
# Run a specific task interactively
cb interact datasets/cua-bench-workflows/photoshop-tasks --variant-id 0

# Run with specific variant
cb interact datasets/cua-bench-workflows/photoshop-tasks --variant-id 2
```

## Task Variants

### photoshop-tasks

| Variant | Description                                      |
| ------- | ------------------------------------------------ |
| 0       | Create document with "Hello World" text layer    |
| 1       | Create document with "Welcome to CUA" text layer |
| 2       | Open PSD and count layers                        |
| 3       | Open PSD and describe layers                     |
| 4       | Create and save document as specific filename    |
