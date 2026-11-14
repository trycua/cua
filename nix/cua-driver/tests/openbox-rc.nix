# Shared openbox config for the X11 GUI tests.
#
# Returns an openbox rc.xml whose only non-default setting is
# `<focus><focusNew>no</focusNew></focus>`: openbox must NOT auto-focus a
# window when it is first mapped. Everything else falls back to openbox's
# built-in defaults (missing nodes are defaulted by the parser).
#
# Why: the background-GUI tests launch a real app (chromium/electron) in the
# background while a control terminal stays focused, then assert focus never
# left the control terminal. Under the faster container backend, chromium-based
# apps finish loading and map/raise their toplevel mid-test; with the default
# `focusNew=yes` openbox would shift X input focus to the freshly-mapped window
# and break that invariant. Explicit `xdotool windowactivate` requests are user
# actions and are still honoured, so the control terminal is still focusable.
#
# Usage (in a test's `let`):
#   openboxRc = import ./openbox-rc.nix { inherit pkgs; };
# then launch openbox with it:
#   openbox --config-file ${openboxRc}
{ pkgs }:

pkgs.writeText "openbox-rc.xml" ''
  <?xml version="1.0" encoding="UTF-8"?>
  <openbox_config xmlns="http://openbox.org/3.4/rc">
    <focus>
      <focusNew>no</focusNew>
    </focus>
  </openbox_config>
''
