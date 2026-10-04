-- Cua sandbox wiring for Omarchy edge (libs/images/omarchy). Included last
-- from ~/.config/hypr/hyprland.lua.
--
-- Omarchy edge ships cua-driver-bin and cua-hyprland-plugin but enables
-- neither. This turns them on the way omacom/omarchy#11342 does: the driver's
-- native Wayland backend in the session environment, and the compositor
-- plugin loaded with plugin:cua:enabled. The plugin adds the Cua-Agent and
-- Cua-Agent-2 seats, the OS-level input path every cua-driver in the guest
-- (Omarchy's and cua-spacesd's) goes through.
hl.env("CUA_DRIVER_RS_ENABLE_WAYLAND", "1")

-- A fixed, unscaled output: the image claims 1280x800, and a VM's virtual
-- display reports no physical size to scale from.
hl.env("GDK_SCALE", "1")
hl.monitor({ output = "", mode = "1280x800@60", position = "0x0", scale = 1 })

hl.plugin.load("/usr/lib/cua/hyprland/cua-hyprland-plugin.so")
-- The plugin is qualified for the plain evdev/pc105/us keymap.
hl.config({
  plugin = { cua = { enabled = true } },
  input = { kb_rules = "evdev", kb_model = "pc105", kb_layout = "us",
            kb_variant = "", kb_options = "", numlock_by_default = false },
})

-- Publish the session environment for cua-spacesd and the fixtures (a system
-- service outside this session): /run/cua-desktop/desktop.env.
hl.on("hyprland.start", function()
  hl.exec_cmd("/opt/cua/bin/cua-omarchy-session-env")
end)
