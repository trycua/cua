# Linux async-io embedding fixture

This workspace member compiles and links the native SDK alongside `oo7 0.6.0`
with its `async-std` and `native_crypto` features. That dependency path selects
ashpd's `async-io` backend, reproducing the backend-unification constraint of an
async-io embedding host without requiring the complete downstream application.

From `libs/cua-driver/rust` on Linux:

```bash
cargo run -p cua-embedded-async-io-fixture --locked
```

The workspace lockfile pins the complete graph. The Linux unit workflow runs
this fixture explicitly. On other operating systems the fixture is a no-op and
does not add Linux dependencies to the build.

Restoring only platform-linux's former `ashpd/tokio` selection makes compilation
fail with `You can't enable both async-io & tokio features at once`. Restoring the
candidate's `async-io` selection makes it build and run again.

The executable references SDK and oo7 types without opening a keyring, desktop
session, or portal. It proves compile/link compatibility, not full GPUI/Maple
behavior, keyring access, or portal delivery.
