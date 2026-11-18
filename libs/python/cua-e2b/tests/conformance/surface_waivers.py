"""Public names of the real `e2b` package that cua-e2b deliberately does not
mirror, each with the reason. `tests/test_surface.py` fails on any difference
from a recorded surface that no entry here names, and on any entry that names
nothing (a stale waiver).

An entry is `(pattern, status, reason)`:

- `pattern`: an `fnmatch` glob over the problem path `module.Name[.member]`;
- `status`: `internal` (transport, generated or plumbing code that user code
  does not call: the SDK constructs it), or `native-error` (the feature is
  impossible on Cua; the call raises e2b's own exception class and says why);
- `reason`: one sentence.

Features that map onto Cua are never listed here: they are implemented and
tested.
"""

from __future__ import annotations

from fnmatch import fnmatchcase
from typing import List, Optional, Tuple

Waiver = Tuple[str, str, str]

WAIVERS: List[Waiver] = [
    # -- plumbing classes: the Sandbox builds them, user code never does ------
    (
        "e2b.sandbox_async.commands.command.Commands.__init__*",
        "internal",
        "Constructed by AsyncSandbox with its transport (here: the sandbox's machine handle).",
    ),
    (
        "e2b.sandbox_async.commands.pty.Pty.__init__*",
        "internal",
        "Constructed by AsyncSandbox with its transport.",
    ),
    (
        "e2b.sandbox_async.filesystem.filesystem.Filesystem.__init__*",
        "internal",
        "Constructed by AsyncSandbox with its transport.",
    ),
    (
        "e2b.sandbox_async.commands.command_handle.AsyncCommandHandle.__init__*",
        "internal",
        "Constructed by Commands.run/connect with the process stream of the transport.",
    ),
    (
        "e2b.sandbox_async.filesystem.watch_handle.AsyncWatchHandle.__init__*",
        "internal",
        "Constructed by Filesystem.watch_dir with the watcher of the transport.",
    ),
    # -- constructors the SDK itself calls (the sandbox builds these) -----------------
    (
        "e2b.sandbox_sync.*.__init__*",
        "internal",
        "Constructed by Sandbox with the transport of the async implementation underneath.",
    ),
    (
        "e2b.sandbox_async.main.AsyncSandbox.__init__*",
        "internal",
        "Deprecated upstream: sandboxes come from create/connect; here the constructor takes the Space handle.",
    ),
    (
        "e2b.sandbox_async.commands.command_input_handle.AsyncCommandInputHandle.__init__*",
        "internal",
        "Constructed by Commands.stream_input / Pty.stream_input with the process of the transport.",
    ),
    (
        "e2b.sandbox.filesystem.filesystem.*FileStreamReader.__init__*",
        "internal",
        "Constructed by files.read(format='stream') around the stream of the transport (an HTTP response upstream).",
    ),
    # -- helpers of E2B's HTTP/protobuf transport ----------------------------------------
    (
        "e2b.sandbox.filesystem.filesystem.map_entry_info",
        "internal",
        "Maps an envd protobuf EntryInfo; cua-e2b builds EntryInfo from cua-spacesd's file service.",
    ),
    (
        "e2b.sandbox.filesystem.filesystem.map_file_type",
        "internal",
        "Maps an envd protobuf FileType; cua-e2b maps cua-spacesd's file types.",
    ),
    (
        "e2b.sandbox.filesystem.filesystem.multipart_body_is_streamed",
        "internal",
        "Decides whether an httpx multipart upload streams; cua-e2b uploads through cua-spacesd, not multipart HTTP.",
    ),
    (
        "e2b.sandbox.filesystem.filesystem.to_upload_body*",
        "internal",
        "Builds the httpx request body of E2B's file upload; cua-e2b streams file objects to cua-spacesd instead.",
    ),
    (
        "e2b.sandbox.filesystem.watch_handle.map_event_type",
        "internal",
        "Maps an envd protobuf EventType; cua-e2b maps cua-spacesd's watcher events.",
    ),
    (
        "e2b.sandbox.commands.stream_input",
        "internal",
        "Builds envd protobuf StreamInput requests; cua-e2b writes the same ordered input to cua-spacesd.",
    ),
    (
        "e2b.sandbox._git*",
        "internal",
        "Underscore (private) package of command-line builders and parsers; cua-e2b has its own git module with the public Git/GitStatus API.",
    ),
    # -- control-plane transport and layout --------------------------------------
    (
        "e2b.retry.RetryableTransport",
        "internal",
        "The httpx transport that retries E2B REST calls; cua-e2b has no REST control plane (the retry rules are mirrored as pure functions in e2b.retry).",
    ),
    (
        "e2b.retry.AsyncRetryableTransport",
        "internal",
        "The async httpx transport that retries E2B REST calls; cua-e2b has no REST control plane.",
    ),
    (
        "e2b.secret.Secret",
        "internal",
        "e2b 2.42 defined Secret in the module e2b.secret itself; later releases (and cua-e2b) define it in e2b.secret.secret_sync and export the same name from e2b.secret.",
    ),
]


def waiver_for(problem: str) -> Optional[Waiver]:
    path = problem.split(": ", 1)[0]
    # `module.Name(param)` -> `module.Name`
    path = path.split("(", 1)[0]
    for w in WAIVERS:
        if fnmatchcase(path, w[0]) or fnmatchcase(problem, w[0]):
            return w
    return None
