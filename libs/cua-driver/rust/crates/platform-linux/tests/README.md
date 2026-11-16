# Private AT-SPI `set_value` fallback fixture

`set_value_fallback.rs` calls the real `native::set_value` and
`native::insert_text` against `fixtures/rejecting_editable.py`, an AT-SPI
application with one editable that rejects `SetTextContents`. The application
registers with the real AT-SPI registry on a private session bus, so the driver
finds it as it finds any application. Each test runs once per
`Application.ToolkitName` the application claims: `GTK` (InsertText reads
UTF-8 bytes), `Qt` (it reads exactly that many UTF-16 units) and none (the
property is absent). The application reports its text, caret and calls over a
pipe, not over AT-SPI.

## Run

Needs `dbus-daemon`, `at-spi2-core`, `python3-dbus` and `python3-gi`, plus the
driver's normal Rust build dependencies. From `libs/cua-driver/rust`:

```sh
env -u DISPLAY -u WAYLAND_DISPLAY -u XAUTHORITY -u AT_SPI_BUS_ADDRESS \
  -u DBUS_SESSION_BUS_ADDRESS -u NO_AT_BRIDGE \
  dbus-run-session -- env CUA_PRIVATE_ATSPI_TEST=1 \
  cargo test --locked -p platform-linux --test set_value_fallback -- \
  --ignored --nocapture --test-threads=1
```

`CI: Rust Linux unit` runs this whole ignored binary with that command, and the
E2E inventory test keeps it routed. No display is needed, and the private bus
keeps the driver away from the user's applications. The fixture is killed when
its owner drops and exits on its own after 90 seconds.

## Coverage

- A rejected `SetTextContents` falls back to `InsertText` at the caret, with the
  length in the toolkit's unit: a Unicode sentence, ASCII, emoji, combining
  marks, empty text, and a Unicode prefix at a nonzero character caret.
- With no toolkit name, non-ASCII text is never inserted and `set_value` fails
  with the exact `no_value_route` error; ASCII is still inserted.
- `SetTextContents` accepted or raising, a caret read error, and an
  `InsertText` error or `false` answer.

It is protocol evidence against a scripted application, not a run against real
GTK or Qt widgets. The fixture counts caret offsets in characters for every
toolkit. The driver hands `CaretOffset` back to `InsertText` unchanged, so this
checks that round trip, not Qt's own offset unit.
