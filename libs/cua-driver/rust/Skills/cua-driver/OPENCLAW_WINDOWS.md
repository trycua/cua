# OpenClaw profile: Windows

Cua Driver must run in an interactive user desktop, never Session 0. A
structured interactive-desktop refusal is a host setup failure, not permission
to use another machine or transport.

Prefer UI Automation actions from a fresh `get_window_state`. Use target-local
background pixels only when UIA cannot address the control. Use foreground
delivery through the live action schema only after background refusal or failed
verification.

Windows may host an app window through another process such as
`ApplicationFrameHost`. Keep the exact `pid` and `window_id` returned by the
provider and refresh them after a window replacement. Never infer identity
from a title alone.

Secure desktop, elevation prompts, lock screens, and protected system surfaces
are outside ordinary provider authority. Return the structured refusal to the
host.
