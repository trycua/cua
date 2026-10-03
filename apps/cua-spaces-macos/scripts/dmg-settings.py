# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# dmgbuild settings for the Cua Spaces disk image (package-dmg.sh passes
# -D app=... -D alias=... -D art=...). The window is the spaces.cua.ai hero
# with the blue Cua Spaces key missing: the app sits on the left, and the
# Applications alias, whose icon is that key's outline, sits where the key was.
# Positions and sizes match scripts/dmg-art/art.html.
import os.path

app = defines["app"]  # noqa: F821 (dmgbuild provides defines)
alias = defines["alias"]  # noqa: F821
art = defines["art"]  # noqa: F821
app_name = os.path.basename(app)

format = "UDZO"
compression_level = 9
filesystem = "HFS+"
files = [app, alias]
icon = os.path.join(app, "Contents", "Resources", "AppIcon.icns")  # the mounted volume's icon

background = os.path.join(art, "background.png")  # dmgbuild adds background@2x.png beside it
# Finder counts the title bar in the window height; 32 more keeps all 480 of the art visible
window_rect = ((200, 140), (720, 512))
default_view = "icon-view"
show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False
show_icon_preview = False
include_icon_view_settings = True
arrange_by = None
label_pos = "bottom"
text_size = 12
# The key is 640 of the icon's 1024 grid: 90 points here, a little under the hero's 108,
# which keeps the Applications label above the terracotta key below it.
icon_size = 144
hide_extensions = [app_name]
icon_locations = {
    # the empty left side, where the hero's copy was
    app_name: (173, 240),
    # the blue key's spot in the hero, (78%, 40%), raised so its label sits on sky;
    # the icon draws its outline 18 points low and 6 left, so its label sits as far below the
    # outline as the app's sits below its tile, centred on the tilted key's front edge
    os.path.basename(alias): (566, 147),
}
