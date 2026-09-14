# X11 indexed click identity candidate

An observed element index is an address within one snapshot. Enabling or removing
an earlier control can change the next live traversal's indices. The old X11 click
resolver selected the live nth actionable node, which could activate a different
button while returning success.

The candidate keeps a sparse index-to-AT-SPI-identity map for each runtime, PID,
X11 window, and exact observed snapshot. Each identity contains the object's unique
D-Bus owner and path plus its owning frame's owner and path. Well-known bus names
are pinned to their unique owner once per walk; failure to establish an owner
leaves discovery available but cannot authorize an indexed click.

X11 indexed click matches that same identity in a fresh, window-correlated tree.
The matched live proxy is retained for classification, geometry and activation.
Its current ordinal is used only to address the existing geometry helper over
that same retained tree. A missing, disabled, ambiguous, or differently framed
object fails before input with the existing stale-token message. A newer snapshot
invalidates its old token rather than replacing its cached map. Object liveness
is checked after the cursor overlay await, before delivery. Uncertain activation
failures never replay through pointer input.

This is limited to X11 indexed click. Other indexed actions and native Wayland's
actuation path have not acquired this guarantee. X11 property-only fallback
snapshots have no AT-SPI identity and cannot authorize indexed clicks. Existing
ambiguity refusal for coincident top-level window bounds remains. Toolkit object
identity is its AT-SPI bus owner/path contract; a toolkit that silently recycles a
live path for a different object without invalidation cannot be distinguished by
this address alone. As with native input, application changes between final
validation and action submission remain a race; this change does not lock apps.

Validation before promotion must use the exact candidate build:

- `INDEX-L01`: observe OK, enable an earlier disabled button, click retained OK;
  the intended OK effect must occur or a clear pre-input stale refusal must leave
  all action effects unchanged. The historical fork baseline activated Cancel.
- `INDEX-L02`: remove observed OK, click retained OK; require clear stale refusal
  and no other button effect. The historical fork baseline activated Enable earlier control.
- Existing Calc indexed-selection and exact-window/modal regression cases must
  remain valid, including controls that require pointer delivery.

The pure cache test covers sparse indices, exact snapshot replacement, window
isolation, and runtime isolation. Local parsing is not Linux build or GUI evidence.
The candidate is unpromoted until Linux compilation and real regressions pass.

## Reproducible fixture and tool sequence

On a disposable X11 desktop with an EWMH window manager, AT-SPI enabled,
Python GI and GTK3 installed, launch:

```sh
NO_AT_BRIDGE=0 GDK_BACKEND=x11 /usr/bin/python3 \
  libs/cua-driver/tests/fixtures/apps/linux/gtk-index-drift.py
```

Use one persistent driver runtime/session for all calls. Starting another runtime
would test token ownership rejection rather than live index drift.

1. Use `list_windows` to find **Cua Index Lab** and its actual PID/X11 window ID.
2. Call `get_window_state` with `{"pid":PID,"window_id":XID}`. Retain the
   `element_token` for **OK** from `structuredContent.elements`. Confirm the
   visible labels say **Earlier: disabled** and **Action: none**.
3. Click **Enable earlier control** manually, or using its token from the same
   observation. Do **not** request another accessibility snapshot before step 4;
   that would invalidate the old token and mask this bug.
4. Call `click` with `{"pid":PID,"window_id":XID,"element_token":"RETAINED_OK_TOKEN"}`.
   Substitute the observed numeric IDs and actual token in these JSON examples.
5. Now observe the window. Accept **Action: OK**, or an explicit stale refusal
   with **Action: none**. **Action: CANCEL** or another control's effect fails.
6. Relaunch the fixture to reset it. Repeat the original observation, click
   **Remove OK**, then click the retained OK token without an intervening state
   request. Require stale refusal, **Action: none**, and OK absent.
7. Close only the fixture window/process created for the reproduction.

Run baseline and candidate separately with fresh fixture processes and runtimes.
Record their source SHAs, initial state, raw click responses, and final visible
state. The committed pure tests exercise matching logic; this fixture sequence
exercises actual AT-SPI object identity and user-visible control effects.
