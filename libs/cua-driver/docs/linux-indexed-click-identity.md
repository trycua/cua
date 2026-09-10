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
  all action effects unchanged. The 624 baseline activated Cancel.
- `INDEX-L02`: remove observed OK, click retained OK; require clear stale refusal
  and no other button effect. The 624 baseline activated Enable earlier control.
- Existing Calc indexed-selection and exact-window/modal regression cases must
  remain valid, including controls that require pointer delivery.

The pure cache test covers sparse indices, exact snapshot replacement, window
isolation, and runtime isolation. Local parsing is not Linux build or GUI evidence.
The candidate is unpromoted until Linux compilation and real regressions pass.
