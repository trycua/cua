# OpenClaw profile: recording and replay

Use recording and replay only when the live provider exposes them for the
current execution.

The trusted host owns recording output roots, replay inputs, and video helper
selection. Start recording without a native output path when the managed schema
injects a session-owned root. Replay only an opaque artifact id accepted by the
same session. Do not discover, construct, or reuse a local path.

Recording is session-scoped. Stop it before ending the Cua session when the
tool is present. Session teardown must remain the final cleanup fallback.

Replay performs real actions. Follow the same permission mode, capture scope,
and verification rules as live actions. Element indices and browser refs from
the original run are stale. Replay attempt, success, and failure counts remain
visible to the model. Native paths and nested platform error details remain
host-side.

Video availability depends on the trusted host's platform setup. The model
must not install `ffmpeg`, select an executable, or fall back to ambient
`PATH`.
