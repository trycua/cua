# Remote app shares over Tailscale

These are `cua-spacesd legacy` modes (the pre-consolidation window-streaming
daemon). The current `cua.env.v1` server, relay and token model are in the
[README](../README.md).

One remote `cua-spacesd` process exposes one explicitly configured application
share. The application selector is evaluated inside the provider before the
target catalog issues opaque handles, so discovery, restoration, capture, and
actions cannot escape the configured application.

The current macOS selector is the application bundle identifier. The same
server-side `application-id` slot is reserved for an application identity or
AUMID on Windows and a desktop application ID on Linux. Display names are not
used for remote authorization.

## Host

### macOS identity and permissions

Native capture must run from a stable signed app identity. Build the host app
with a persistent local certificate for development or a Developer ID
Application identity for distribution:

```sh
CUA_ENV_CODESIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)" \
  ./scripts/build-macos-app.sh
```

Copy `target/macos/Cua Spacesd.app` to `/Applications`, then grant **Cua Spacesd**
Screen Recording and Accessibility access in System Settings. Rebuilding with
the same signing identity and bundle ID preserves the identity macOS uses for
those grants. Ad-hoc signing is supported for a build smoke test, but its
identity is not stable enough for a durable TCC grant.

`cargo run -p cua-spacesd` remains useful for deterministic protocol tests and target
enumeration. It must not be treated as proof of real capture because macOS can
attribute it a different or transient TCC identity.

### Codex-only share

Start the installed app as a Codex-only share on loopback. At least one
Tailscale user or app capability constraint is required:

```sh
open -a "Cua Spacesd" --args legacy \
  --remote-listen 127.0.0.1:7443 \
  --share codex \
  --application-id com.openai.codex \
  --allow-tailscale-user you@example.com
```

Then publish the loopback HTTP/WebSocket server to the tailnet:

```sh
tailscale serve --bg --https=443 http://127.0.0.1:7443
```

For app-capability authorization, add the same capability to both sides:

```sh
open -a "Cua Spacesd" --args legacy \
  --remote-listen 127.0.0.1:7443 \
  --share codex \
  --application-id com.openai.codex \
  --require-tailscale-capability trycua.com/cap/rcdp

tailscale serve --bg --https=443 \
  --accept-app-caps=trycua.com/cap/rcdp \
  http://127.0.0.1:7443
```

Configure the corresponding user, group, device, and application capability
grants in the tailnet policy. If both daemon authorization flags are supplied,
both checks must pass.

`cua-spacesd` refuses a non-loopback remote bind. TLS and tailnet admission terminate
at Tailscale Serve; RCDP independently validates the injected
`Tailscale-User-Login` and/or `Tailscale-App-Capabilities` headers before the
WebSocket upgrade. Do not place a generic reverse proxy between Tailscale
Serve and RCDP unless it preserves this trust boundary.

The loopback boundary assumes local processes on the host are trusted. The
Tailscale identity headers are not an authentication mechanism on a directly
reachable socket and RCDP intentionally refuses to expose that socket on a LAN
or tailnet address.

### Direct client/server mode

RCDP can also expose the same provider-side application selector through its
token-authenticated WebSocket server without Tailscale Serve:

```sh
open -a "Cua Spacesd" --args legacy \
  --listen 127.0.0.1:3211 \
  --application-id com.openai.codex \
  --token "LONG_RANDOM_TOKEN"
```

Connect with `cua-viewer --url ws://127.0.0.1:3211 --token
"LONG_RANDOM_TOKEN"`, substituting a private host address when both machines
share a trusted LAN. A non-loopback listener refuses to start without a token.
Because the direct listener is raw `ws://`, the token and media are not
encrypted on the wire; keep it on a trusted LAN or carry the loopback listener
through SSH/TLS. Do not expose it directly to the public Internet.

This mode removes the Tailscale Serve reverse-proxy hop, but it does not change
the physical network RTT or RCDP's ordered WebSocket/TCP media lane. It is an
important A/B and LAN deployment mode, not a substitute for the future
loss-tolerant media transport.

## Client

Open the host's Tailscale HTTPS name in a browser. The daemon serves the viewer
at `/` and derives `wss://HOST/v1/connect` from the page URL. It discovers the
Codex windows in the share, opens any one of them, renders the stream, and maps
canvas clicks and text requests to normal RCDP actions.

`cua-env-cli` was removed because cua-spacesd is server-only. For
cua.env.v1 debugging use the `cua` CLI (`cua spacesd targets <url> --windows`,
`cua spacesd call <url> <Service/Method> [json]`). The Dock-app generator
(`app install-macos`) was dropped with it; it returns when cua-viewer moves
into libs/cua.

Every remote session has a server-enforced `allow_activation` policy ceiling.
The native client requests `allow_activation`, causing the trusted provider to
open CuaDriver's persistent foreground input session for pointer, wheel, text,
key, shortcut, and drag events. The provider activates the selected app once,
then delivers ordered event batches through the reused native worker while
leaving that app frontmost. Clients may still request `view_only` or
`background_only`; the provider forces background mode for the latter and
ignores any client-supplied delivery-mode override.

## Video path

Remote sessions prefer VideoToolbox H.264 and fall back to tightly packed BGRA
when a client or provider cannot negotiate H.264. The encoder uses a bounded
one-frame mailbox, no frame reordering, periodic IDRs, explicit keyframe
requests, and SPS/PPS on every IDR. The browser derives the AVC profile and
level from the SPS at each codec epoch and decodes Annex B through WebCodecs.

The first H.264 implementation uses a resolution/FPS bitrate heuristic capped
at 8 Mbps. Adaptive bitrate based on network feedback and zero-copy teleport
from ScreenCaptureKit to VideoToolbox remain future media work. The browser's
BGRA renderer remains available as a correct but bandwidth-heavy fallback.

The current share exposes all windows belonging to the selected application as
independent one-window sessions. A future app supervisor can follow window
creation and closure and compose those sessions without changing their action
or freshness contracts.
