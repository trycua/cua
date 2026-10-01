A sandbox's `id()` is its ref, and every call that takes a sandbox name accepts one:

| Ref | Meaning |
| --- | --- |
| `local:<name>` | A sandbox on this machine |
| `cloud:<name>` | A cloud sandbox |
| `direct:<host:port>` | A machine reached by address |
| `relay:<machine-id>` | An account machine reached through the relay (a Space) |
| `<name>` | A bare name, searched across locations; `AmbiguousSandbox` when it matches more than one |

Readiness never assumes a guest agent: the provider reports the sandbox running, then the `wait_for` probes run, and creation fails fast if the sandbox exits.
