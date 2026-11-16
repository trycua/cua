-- Cua Driver Bench v2, Hyprland port: the desktop settings the BenchLab trials need.
-- Load it last from ~/.config/hypr/hyprland.lua:  dofile(os.getenv("HOME") .. "/.config/hypr/bench.lua")
-- (Hyprland 0.55+ with a Lua config, as on Omarchy edge). launch/hyprland_session.py applies the same window
-- rules on every trial, so only the screen size needs this file.

-- 1920x1080 so the shared frames fit: BenchLab at 40,40 760x540, the User activity window at 420,340 900x520.
hl.monitor({ output = "", mode = "1920x1080@60", position = "0x0", scale = 1 })

-- BenchLab and the sentinel float at the frames the session gives them (Hyprland would tile them).
-- BenchLab's windows never take focus when they open, and its dialogs open over BenchLab's top-left corner,
-- clear of the User activity window: an app working in the background does not cover the user's window with
-- its own dialogs (on macOS a sheet hangs from BenchLab's title bar; on Windows the dialog opens just above
-- BenchLab).
hl.window_rule({ name = "cdb-benchlab", match = { class = "^BenchLab$" }, float = true, no_initial_focus = true })
hl.window_rule({
  name = "cdb-benchlab-dialogs",
  match = { class = "^BenchLab$", title = "^(Allow BenchLab.*|Save changes.*|Cookie preferences)$" },
  float = true,
  no_initial_focus = true,
  move = "80 120",
})
hl.window_rule({ name = "cdb-sentinel", match = { class = "^BenchSentinel$" }, float = true })
