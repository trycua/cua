# AT-SPI Unicode insertion regression

`set_value_fallback.rs` invokes the real Linux `native::insert_text` and
`native::set_value` APIs against a separate, owned AT-SPI process. The fixture
rejects `SetTextContents` by default and consumes the `InsertText` length in
UTF-8 bytes. It reports application text, caret, and protocol calls independently
through a pipe. No duplicated Rust length-conversion helper is tested.

## Run

Prerequisites on Debian/Ubuntu: `dbus`, `at-spi2-core`, `python3-dbus`, and
`python3-gi`, plus the driver's normal Rust build dependencies.
From `libs/cua-driver/rust`, run:

```sh
env -u DISPLAY -u WAYLAND_DISPLAY -u XAUTHORITY -u AT_SPI_BUS_ADDRESS \
  -u DBUS_SESSION_BUS_ADDRESS -u NO_AT_BRIDGE \
  dbus-run-session -- env CUA_NATIVE_GTK_TEST=1 \
  cargo test --locked -p platform-linux --test set_value_fallback -- \
  --ignored --nocapture --test-threads=1
```

The `CI: Rust Linux unit` workflow installs these fixture dependencies and runs
this entire ignored test binary with the command above, before ordinary unit
tests. The tracked E2E inventory test guards the whole-binary CI route.

The retained opt-in environment variable is historical; this fixture needs no
GTK window or display. A private session/accessibility bus prevents access to
personal applications. The fixture is killed and reaped when its owner drops;
it also has a 90-second lifetime limit.

## Coverage and limits

- Exact Unicode sentence, ASCII, emoji, combining marks, empty insertion, and
  insertion into a Unicode prefix at a nonzero **character** caret offset.
- Direct-set acceptance/rejection/error, caret-read error/default zero, and
  insertion error/false with the current exact `no_value_route` error.
- Byte-bounded insertion is the only production behavior changed. Existing
  set-value fallback insertion-at-caret behavior and selection/replacement
  semantics remain unchanged.
- Checked `i32` conversion prevents byte-length narrowing. The oversized-input
  error branch is not exercised: no multi-gigabyte string is allocated.
- This is controlled AT-SPI protocol evidence, not real-toolkit, desktop,
  MCP transport, or complete cross-platform acceptance. Normal `cargo test`
  leaves these opt-in tests ignored.
