---
name: gui-automation
description: >-
  Use when you need to visually interact with a GUI — test buttons, fill forms,
  verify visual layouts, fuzz web pages, automate user flows, take screenshots,
  or perform end-to-end QA on any application. Works on cloud VMs, Docker
  containers, local machines, and sandboxes. Install: pip install cua.
---

# GUI Automation

CUA gives you **eyes and hands on a real computer**. You can see the screen,
move the mouse, click buttons, type text, drag elements, and manage windows —
exactly like a human sitting at the keyboard.

Use this skill whenever the task requires **visual interaction** that cannot be
done through a shell command or API call: testing a UI, filling a form, verifying
a layout, fuzzing a web page, or driving an end-to-end user flow.

Every action you take is automatically recorded to a replayable trajectory.
When you finish, share the trajectory link so the user can review your work.

## Prerequisites

Before your first action, ensure the CLI is available:

```bash
# Check if cua is installed
cua --version

# If missing, install it
pip install cua
```

**Target setup** — connect to the machine you will automate:

```bash
# Cloud VM
cua do switch cloud my-vm

# Docker container
cua do switch docker my-container

# Local machine (requires one-time consent)
cua do-host-consent
cua do switch host
```

> **ANTHROPIC_API_KEY** is optional. If set, `cua do snapshot` returns an
> AI-annotated summary with element coordinates. Without it, use
> `cua do screenshot` and read the image yourself to determine coordinates.
> Both approaches work — snapshot is a convenience, not a requirement.

## Common Scenarios

### 1. Test a button

> "Click the Export PDF button and verify the download starts."

```bash
cua do switch cloud my-vm
cua do screenshot                       # see the current screen
# (read the image to locate the Export PDF button)
cua do click 450 280                    # click it
cua do screenshot                       # confirm: did a download dialog appear?
```

### 2. Fill a form and submit

> "Fill out the signup form with test data and submit it."

```bash
cua do screenshot                       # see what fields are present
cua do click 400 200                    # click the Name field
cua do type "Jane Doe"
cua do key tab
cua do type "jane@example.com"
cua do key tab
cua do type "SecureP@ss123"
cua do click 400 500                    # click Submit
cua do screenshot                       # verify success message
```

### 3. Handle a file upload dialog

> "Upload report.pdf through the file picker."

```bash
cua do screenshot                       # locate the upload button
cua do click 350 400                    # click "Choose File"
cua do screenshot                       # file picker is now open
cua do type "/home/user/report.pdf"     # type the file path
cua do key enter                        # confirm selection
cua do screenshot                       # verify the file name appears
```

### 4. Visual verification

> "Check that the dashboard chart renders correctly after filtering."

```bash
cua do screenshot                       # baseline state
cua do click 600 120                    # click the date filter dropdown
cua do screenshot                       # see dropdown options
cua do click 600 180                    # select "Last 7 days"
cua do screenshot                       # verify: chart updated, no blank areas, labels correct
```

### 5. End-to-end user flow

> "Walk through the checkout flow from cart to confirmation."

```bash
cua do screenshot                       # see the cart page
cua do click 700 500                    # click "Proceed to Checkout"
cua do screenshot                       # shipping form
cua do click 400 200
cua do type "123 Main St"
cua do key tab
cua do type "Springfield"
cua do key tab
cua do type "62701"
cua do click 700 500                    # click "Continue to Payment"
cua do screenshot                       # payment form
cua do click 400 200
cua do type "4111111111111111"
cua do key tab
cua do type "12/28"
cua do key tab
cua do type "123"
cua do click 700 500                    # click "Place Order"
cua do screenshot                       # confirmation page — verify order number present
```

### 6. Cross-window drag and drop

> "Drag the file icon from the file manager into the upload zone in Chrome."

```bash
cua do window ls                        # list open windows with IDs
cua do screenshot                       # see both windows
cua do drag 150 300 650 400             # drag from file manager → upload zone
cua do screenshot                       # verify the file landed in the drop zone
```

### 7. Fuzz a web page

> "Try unexpected inputs on every text field in the form."

```bash
cua do screenshot                       # see the form
cua do click 400 200                    # first field
cua do type "<script>alert(1)</script>"
cua do key tab
cua do type "'; DROP TABLE users; --"
cua do key tab
cua do type "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
cua do click 400 500                    # submit
cua do screenshot                       # check for errors, crashes, or unexpected behavior
```

## Workflow Pattern

Every GUI task follows the same loop:

1. **Connect** — `cua do switch <provider> <name>`
2. **Look** — `cua do screenshot` (or `snapshot` if API key is set)
3. **Act** — `click`, `type`, `key`, `drag`, `scroll`, etc.
4. **Verify** — `cua do screenshot` again to confirm the result
5. **Repeat** — continue until the task is done
6. **Share** — `cua do trajectory share` to give the user a replay link

> Always re-screenshot after any action that changes the UI. Coordinates go
> stale the moment the screen changes.

## Trajectory Recording and Sharing

Every `cua do` action is automatically recorded to a replayable trajectory at
`~/.cua/trajectories/{machine}/{session}/`. A new session starts each time you
run `cua do switch`.

### Sharing your work

When you finish a task, **always share the trajectory** so the user can replay
and review exactly what happened:

```bash
cua trajectory share                    # upload and get a shareable HTTPS link
```

Tell the user:

> "Here is the trajectory of my session: {url}"

### Other trajectory commands

```bash
cua trajectory ls                       # list all recorded sessions
cua trajectory view                     # open latest session in browser (local)
cua trajectory view my-container        # view latest for a specific machine
cua trajectory export                   # generate a self-contained HTML report
cua trajectory export -o report.html    # export to a specific path
cua trajectory clean --older-than 7     # delete sessions older than 7 days
cua trajectory stop                     # stop the local viewer server
```

### Disabling recording

If the user asks you not to record, pass `--no-record`:

```bash
cua do --no-record click 100 200
```

## Guardrails

- **Re-screenshot after every UI change.** Coordinates are image-space pixels
  from the last screenshot. If the screen changed, your coordinates are wrong.
- **snapshot vs screenshot.** `snapshot` calls an AI model and requires
  `ANTHROPIC_API_KEY`. If the key is missing, fall back to `screenshot` and
  read the image yourself. Both work.
- **Coordinates are image-space.** Zoom and max-length scaling are applied
  automatically — use the pixel coordinates from your most recent screenshot.
- **Host consent.** Controlling the local machine requires a one-time
  `cua do-host-consent` before `switch host`.
- **Window zoom.** Use `cua do zoom "App Name"` to crop screenshots to a
  single window. Coordinates become window-relative. `cua do unzoom` to reset.

## Quick Reference

| Action              | Command                                      |
| ------------------- | -------------------------------------------- |
| Connect to target   | `cua do switch <provider> [name]`            |
| See the screen      | `cua do screenshot`                          |
| AI-annotated screen | `cua do snapshot ["instructions"]`           |
| Click               | `cua do click <x> <y> [left\|right\|middle]` |
| Double-click        | `cua do dclick <x> <y>`                      |
| Type text           | `cua do type "text"`                         |
| Press key           | `cua do key <key>`                           |
| Hotkey              | `cua do hotkey <combo>` (e.g. `cmd+c`)       |
| Scroll              | `cua do scroll <direction> [amount]`         |
| Drag                | `cua do drag <x1> <y1> <x2> <y2>`            |
| Move cursor         | `cua do move <x> <y>`                        |
| Run shell command   | `cua do shell "command"`                     |
| Open URL or file    | `cua do open <url\|path>`                    |
| List windows        | `cua do window ls [app]`                     |
| Focus window        | `cua do window focus <id>`                   |
| Share trajectory    | `cua trajectory share`                       |

See [references/command-reference.md](references/command-reference.md) for full
argument syntax and all subcommands.

## Supported Providers

| Provider     | Target            | Example                             |
| ------------ | ----------------- | ----------------------------------- |
| `cloud`      | CUA cloud VM      | `cua do switch cloud my-vm`         |
| `cloudv2`    | CUA cloud VM (v2) | `cua do switch cloudv2 my-vm`       |
| `docker`     | Docker container  | `cua do switch docker my-container` |
| `lume`       | Lume VM           | `cua do switch lume my-vm`          |
| `lumier`     | Lumier VM         | `cua do switch lumier my-vm`        |
| `winsandbox` | Windows Sandbox   | `cua do switch winsandbox`          |
| `host`       | Local machine     | `cua do switch host`                |
