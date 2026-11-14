# Build-bound macOS private worker

A macOS build with `CUA_DRIVER_EMBEDDED_PARENT_REQUIREMENT` set at compilation
admits only this directly spawned entry:

```text
cua-driver __private-worker --generation <generation> --transport-fd=<fd>
```

The requirement is a build input sealed by signing the resulting binary. It
must identify the trusted host's signing identity and code. A runtime variable,
UID, or the protocol's advisory host bundle identifier cannot replace it.
The driver checks its actual parent's running code through Security.framework
before creating the runtime, then checks that the parent has not changed.

The descriptor must be at least 3 and refer to a connected Unix stream. The host
creates a socket pair, explicitly inherits one endpoint, and closes unrelated
copies. The driver marks its endpoint close-on-exec. Existing private-worker
JSON-line requests, generation checks, SDK dispatch and permission checks apply.
No pathname listener or HTTP service is started. Stdio never carries requests
in this build; other command entries and alternate HTTP/lifetime settings fail
closed. Channel closure stops dispatch and ends the worker's runtime.

Ordinary macOS builds preserve their existing standalone and stdio-worker behavior
and refuse the new descriptor option. They are separate from the guarded build.
The embedding host must verify its signed child before launch.

Run `tests/runners/macos/inherited-stream.sh` from the driver component to build
a guarded child and two signed native hosts. The opt-in fixture checks framing,
parent substitution, invalid descriptors, alternate entries and EOF cleanup.
It uses temporary state and metadata requests, with no desktop input or consent.
