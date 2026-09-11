# SDK integration tests

## Linux async-io embedding

`async_io_embedding.rs` compiles and links the native SDK alongside the
Linux-only dev dependency `oo7 0.6.0`, with its `async-std` and `native_crypto`
features. That dependency path selects ashpd's `async-io` backend, reproducing
the feature-unification constraint of an async-io embedding host without
requiring the complete downstream application.

From `libs/cua-driver/rust` on Linux:

```bash
cargo test -p cua-driver-sdk --test async_io_embedding --locked
```

The test references SDK and oo7 types without opening a keyring, desktop session,
or portal. Successful compilation and linking are the regression oracle; this
is not evidence of full GPUI/Maple behavior or portal delivery. The Linux unit
workflow runs it explicitly. Other platforms do not build the Linux-only dev
dependency or run this test.

Restoring only platform-linux's former `ashpd/tokio` selection makes compilation
fail with `You can't enable both async-io & tokio features at once`. Restoring
the candidate's `async-io` selection makes it build again.
