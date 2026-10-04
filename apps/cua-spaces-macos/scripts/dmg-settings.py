# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

import os.path

app = defines["app"]  # noqa: F821
alias = defines["alias"]  # noqa: F821
art = defines["art"]  # noqa: F821
app_name = os.path.basename(app)

format = "UDZO"
compression_level = 9
filesystem = "HFS+"
files = [app, alias]
icon = os.path.join(app, "Contents", "Resources", "AppIcon.icns")

background = os.path.join(art, "background.png")
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
icon_size = 144
hide_extensions = [app_name]
icon_locations = {
    app_name: (173, 240),
    os.path.basename(alias): (566, 147),
}
