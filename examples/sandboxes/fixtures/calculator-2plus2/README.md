# Calculator "2 + 2" trajectory (pre-provided)

This directory holds a cua-driver **trajectory** that the Windows smoke test
(`examples/sandboxes/test_windows_cloud_driver.py`) replays via

```
cua-driver call replay_trajectory {"dir": "C:\\cua\\traj", "delay_ms": 500}
```

to open Windows Calculator and compute **2 + 2**.

> It lives under `fixtures/` (not `trajectories/`) because `trajectories/` is
> gitignored as generated output — this one is a committed test asset.

The trajectory is **pre-provided**: record it once, commit it here. Until a real
recording with a `session.json` exists, the smoke test **skips** the replay (so
CI stays green before the asset is supplied).

## Expected layout

```
calculator-2plus2/
  session.json
  turn-00001/
    action.json
    screenshot.png
    app_state.json
  turn-00002/
    ...
```

## How to record it

On a Windows machine/VM with the driver running in the **interactive** session:

```powershell
cua-driver serve                              # daemon (Session 1+, not Session 0)
cua-driver recording start C:\path\calculator-2plus2
# open Calculator and enter the calculation with the KEYBOARD:
'{"aumid":"Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"}' | cua-driver call launch_app
# then press_key 2 / plus / 2 / equals   (or type_text "2+2=")
cua-driver recording stop
```

Then copy the resulting directory's contents here and commit them.

## IMPORTANT: replay-safe actions only

`element_index` values are **per-session** and do **not** survive replay. Record
using **keyboard** (`press_key` / `type_text`) or **pixel** (`click {pid,x,y}`)
actions only. See `libs/cua-driver/rust/Skills/cua-driver/RECORDING.md`.
