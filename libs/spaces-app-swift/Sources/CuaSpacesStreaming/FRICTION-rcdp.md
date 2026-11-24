# Friction log: building a live stream client against rcdp and the Spaces MCP

> **Status (historical):** recorded against rcdpd and the Python Spaces MCP, both
> since removed. Streaming now goes through cua-spacesd's media plane
> (`Stream.OpenMedia` tickets, RVD2/RAU2 wire, see `libs/cua/proto/MEDIA.md`)
> via `CuaSDK.SpaceStreamSession`. Type names below (`RCDPConnection`, etc.)
> refer to the code at the time.

Every entry below is something the protocol, the daemon or the MCP forced into
the shape of the code in this directory. Each names the file that absorbs it.
This is written for whoever derives an SDK from this, so each entry says what
the SDK should have done instead.

All of it was found against the live Space `local:cua-space-e3c1b54907`
(rcdpd `0.1.0-alpha.1`, macOS, 1024x768 @2x), not read off a spec.

---

## 1. The input sequence must start at 1, and nothing says so

`protocol-v1.md` says an `interactive_input` batch carries "a strictly
contiguous sequence range". It does not say where the range begins. Starting at
0 (the obvious choice for a counter) fails, and the error is actively
misleading:

```
error=stale_target: input sequence must start at 1, received 0
```

`stale_target` is the code for a window handle used with a dead epoch. A client
that trusts the code goes looking at target lifetimes and finds nothing wrong.
Worse, the failure is *self-perpetuating*: the local counter has already
advanced, so every following batch is rejected too (`received 2`, `received 3`,
…) and the stream looks like it has no input capability at all rather than like
it made one off-by-one mistake.

Cost: the first end-to-end input attempt reported `delivered=false,
input_events_dispatched=0` with every event well-formed.

*Absorbed by* `RCDPConnection.nextInputSequence`, which starts at 1 and resets
per session.

*An SDK should* own the sequence entirely and never expose it. There is no
reason a caller should be able to get this wrong.

## 2. `delivered: false` carries no reason unless you ask for it

`InteractiveInputAcknowledgement` has an `error` field, but it is easy to model
the ack without it (`session_id`, `through_sequence`, `delivered` look like the
whole story), and then the failure is a bare `false` with no diagnosis. The
first version of `RCDPWire.swift` did exactly that and the run above was
undebuggable until the field was added.

*An SDK should* make the acknowledgement a `Result`, so the error cannot be
dropped by omission.

## 3. The token rotates per boot and is only reachable out of band

`rcdpd` is launched with `--token "$(openssl rand -hex 16)"`, and writes it to
`~/.rcdp-token` **inside the Space**. There is no endpoint that mints, refreshes
or validates it, and no way to tell a stale token from a wrong one without
attempting a connection. So every client needs a side channel to the Space
(MCP `local_rcdp`, or SSH) purely to learn how to talk to it, and must be
prepared to go back to that side channel mid-session.

*Absorbed by* `RCDPTokenProviding` and `RCDPConnection.connect()`, which retries
exactly once with `forceRefresh: true`.

*An SDK should* take a token *provider*, never a token, and own the refresh.

## 4. The token goes in a message, but the MCP hands you a URL that implies otherwise

`local_rcdp` returns:

```
ws://<space-ip>:8765/ws?token=<token>
```

`rcdpd/src/ws.rs` calls `tokio_tungstenite::accept_async` and never parses the
request URI. The query parameter is decorative; the token only counts as the
first `authenticate` control frame. A client that trusts the URL connects
successfully, sends `hello`, and is disconnected with `hello_required`.

*Absorbed by* the comment in `RCDPConnection.connectOnce`, which drops the query
and authenticates explicitly.

*An SDK should* return a structured endpoint (`host`, `port`, `token`), not a
URL that encodes a credential the server ignores.

## 5. There is no full-desktop target, and the switcher has to bridge two transports

RCDP captures **one window**. The macOS provider builds an `SCContentFilter`
around a single `SCWindow`; there is no display filter anywhere in the daemon.
`list_windows` will never return a desktop target. The product requirement
"toggle between the full desktop and a single application window" therefore
cannot be served by one protocol.

What the Space does expose, on the same host and behind the same token, is the
cua-driver MCP on port 8801, whose `get_desktop_state` returns the whole display
as a PNG and whose input tools accept `{kind: "desktop", display_id: "primary"}`.
So the desktop source is a **poll loop over a request/response tool**: the
canonical shape of a thing that wants to be a subscription. Measured: 6.8-7.0
captures/s, at ~1.6 MB of PNG per capture, versus H.264 at ~900 bytes/frame for
a comparable window.

*Absorbed by* `DesktopFrameSource`, and by `StreamSource.desktop` hiding the
split from the UI.

*An SDK should* expose a desktop target through the same session API as a
window, whatever it has to do underneath. Failing that, the streaming protocol
needs a display filter, the poll loop is a 1000x bandwidth regression for the
one source users reach for first.

## 6. `target` and `scope` are mutually exclusive, and the wrong combination fails quietly in the direction that matters

The driver's `move_cursor` accepts a modern `target` and a legacy `scope`.
Passing both is rejected:

```
invalid_action_target: target cannot be combined with legacy scope, pid, or window_id fields
```

But passing only `target: {kind: "desktop"}` is *accepted* and moves the agent's
cursor **overlay**, not the OS pointer. Only `scope: "desktop"` moves the real
pointer. So the natural call (the modern field, on the desktop target) looks
right, returns success, and moves nothing the user can see. Every other desktop
tool (`click`, `scroll`, `press_key`, `type_text`) wants the opposite: `target`,
not `scope`.

Cost: a desktop-input path that reported success while `get_cursor_position`
never moved off its previous coordinate.

*Absorbed by* the comment and the special case in `DesktopFrameSource.moveCursor`.

*An SDK should* not carry two generations of addressing on one call, and a
legacy field that silently changes which cursor moves should be an error.

## 7. Session geometry is not the geometry the window list reported

`list_windows` described the Blender window as `1024x681@1.0`. `open_session`
on that exact handle returned `2048x1362@2.0`, because `max_dimension` is
applied per session against the backing scale. A client that maps clicks against
the descriptor is off by a factor of two on a Retina Space, and since the
window list is also what the *UI* renders, the discrepancy is invisible until
someone clicks.

*Absorbed by* `LiveStreamSession`, which takes its surface size from
`SessionOpened.geometry` and from the video descriptors, never from
`WindowDescriptor`.

*An SDK should* make `WindowDescriptor.geometry` advisory and clearly named as
such, or omit it.

## 8. The keyframe belongs to the encoder's clock, not to your connection

The host emits an IDR when *its* capture starts, not when a client attaches. A
session opened mid-GOP receives only P-frames and decodes nothing until the next
natural keyframe, which for a mostly-static window can be a long time. The
stream is "connected", packets are arriving, and the view is black.

*Absorbed by* `LiveStreamSession.startWindow`, which sends `request_keyframe`
immediately on open, and by `H264Decoder` refusing to decode before the first
keyframe of the current codec epoch rather than feeding VideoToolbox a frame
with no reference.

*An SDK should* send the keyframe request as part of opening a session.

## 9. Frame rate is a property of the app, not of the stream

ScreenCaptureKit only emits on change, so an idle window decodes at ~0 fps and
that is correct behaviour. Any "is it working?" check built on frame rate gives
a false negative against a static app. Measured on the live Space: an idle
Blender window produced 43 frames in 21 s, while the same pipeline against a
window being actively typed into produced 79 frames in 20.2 s with 0 failures:
matching the 4 changes/s being driven into it.

*Absorbed by* the harness, which drives the surface while measuring.

*An SDK should* surface "frames since attach" and "last frame age" rather than
inviting callers to infer health from fps.

## 10. `text_commit` ignores the caret a click just placed

Clicking mid-line in TextEdit and then sending `text_commit` appends at the end
of the document rather than inserting at the click point. The click itself is
delivered correctly, verified independently: a pointer move to normalized
(0.25, 0.75) of a window at screen origin (79, 55) sized 603x505 pt left the
real OS pointer at exactly (229, 433), against a predicted (229.75, 433.75).
The text insertion simply does not travel through the caret.

So `text_commit` is not "typing"; it is a bulk insert with its own idea of
position. Anything order-sensitive has to go through `key` events.

*Absorbed by* `InputEncoder.keyEvents`, which emits a `key` event **and** a
`text_commit` for printable input, so a host that honours either one behaves.

*An SDK should* document `text_commit` as an insertion primitive, not as
keystrokes, and say what it inserts relative to.

## 11. The window list contains the screen-recording indicator, four times

`list_windows` returns, alongside real windows, several 66x20 off-screen
targets titled literally `Window`, one per recording-capable app
(`universalAccessAuthWarn`, `Unity Hub`, `Cua Driver Local`, …). They are the
macOS screen-recording indicator. In a picker they read as a phantom window that
appears and disappears for no reason and belongs to the wrong app.

They are also not the only non-surfaces: 1024x30 menu-bar strips, 64x64 service
shims, and a 1x1 `nsattributedstringagent` window all arrive in the same list.

*Absorbed by* `Array<WindowDescriptor>.presentable`.

*An SDK should* filter these server-side, or flag them, rather than making every
client rediscover the same list of things that are not windows.

## 12. Multiplexing is fine; the geometry lease is what collides

Worth recording because the opposite is widely assumed. `rcdpd` gives every
WebSocket connection its own `Connection` and its own sessions: two viewers of
the same window do not evict each other, and no take-over registry exists. The
one exclusive resource is the per-target **geometry lease**, claimed only by a
session that asks for `geometry_control: "bidirectional"`. Two bidirectional
sessions on one target evict each other in a loop.

*Absorbed by* `RCDPConnection`, which never requests bidirectional geometry.
That is also why the PiP pop-out can share one session and why a second view
cannot black out the first.

## 13. Lifecycle is the only authority for geometry, and it is a callback into a poll-shaped API

`geometry_changed` is the authoritative signal that the coordinate space moved,
and it arrives asynchronously, unrelated to any request. Everything else in the
client is request/response. Threading that one event into the coordinate mapping
by hand, in a way that provably cannot diverge from the overlay, is the single
most delicate piece of state in this directory, and the reason `StreamGeometry`
is a separate type with one conversion function instead of a couple of
multiplications at the call sites.

*An SDK should* publish geometry as an observable value that the coordinate
helpers read, so a resize cannot be half-applied.
