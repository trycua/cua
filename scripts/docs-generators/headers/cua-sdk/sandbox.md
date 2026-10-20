A sandbox's `id()` is its ref, and every call that takes a sandbox name accepts one:

| Ref | Meaning |
| --- | --- |
| `local:<name>` | A sandbox on this computer |
| `aws:<name>` | A sandbox in your own cloud (`gcp:` and `modal:` the same way) |
| `direct:<host:port>` | A computer reached by address |
| `relay:<computer-id>` | An account computer reached through the relay (a Space) |
| `<name>` | A bare name, searched across locations; `AmbiguousSandbox` when it matches more than one |

Readiness never assumes a guest agent: the provider reports the sandbox running, then the `wait_for` probes run, and creation fails fast if the sandbox exits.
