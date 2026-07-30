# OpenClaw profile: Linux

Use the provider-reported display backend. X11 can support background AT-SPI
and target-local pixel delivery. Wayland support is compositor-specific and may
refuse raw background input even when observation succeeds.

Prefer fresh AT-SPI state and `element_index`. If the provider reports that a
pixel or browser action requires foreground delivery, retry only through the
live `delivery_mode:"foreground"` field. Do not switch display servers, start a
new compositor, or select a different helper.

The trusted host owns the graphical session, accessibility bus, portals, and
helper processes. A missing portal grant, inaccessible AT-SPI bus, or absent
interactive display is a host setup failure.

Refresh state after popups and child windows. When an unfocused popup is
suspected but absent from window state, use `get_desktop_state` only after an
authorized desktop escalation.
