-- Scuttle setup for Hyprland 0.55 and newer (Lua config).
-- Add this to ~/.config/hypr/hyprland.lua, or save it as its own file
-- and require() it from there.

-- Start Scuttle with the session.
hl.on("hyprland.start", function()
    hl.exec_cmd("scuttle")
end)

-- Tap Super on its own to open the launcher. Super held with another
-- key still works as a normal keybind.
hl.bind("Super_L", hl.dsp.exec_cmd("scuttle toggle launcher"), { release = true })

-- Clipboard history and the emoji picker open as launcher modes.
hl.bind("SUPER + V", hl.dsp.exec_cmd("scuttle launcher ';'"))
hl.bind("SUPER + period", hl.dsp.exec_cmd("scuttle launcher ':'"))

-- The assistant panel. Super + / because it is the question mark key.
hl.bind("SUPER + slash", hl.dsp.exec_cmd("scuttle toggle assistant"))

-- Volume and brightness keys. "locked" lets them work on the lock
-- screen and "repeating" lets you hold them down.
local held = { repeating = true, locked = true }
hl.bind("XF86AudioRaiseVolume", hl.dsp.exec_cmd("scuttle volume up"), held)
hl.bind("XF86AudioLowerVolume", hl.dsp.exec_cmd("scuttle volume down"), held)
hl.bind("XF86AudioMute", hl.dsp.exec_cmd("scuttle volume mute"), { locked = true })
hl.bind("XF86MonBrightnessUp", hl.dsp.exec_cmd("scuttle brightness up"), held)
hl.bind("XF86MonBrightnessDown", hl.dsp.exec_cmd("scuttle brightness down"), held)

-- Media keys.
hl.bind("XF86AudioPlay", hl.dsp.exec_cmd("scuttle media toggle"), { locked = true })
hl.bind("XF86AudioNext", hl.dsp.exec_cmd("scuttle media next"), { locked = true })
hl.bind("XF86AudioPrev", hl.dsp.exec_cmd("scuttle media previous"), { locked = true })

-- Blur what is behind Scuttle's translucent panels. ignore_alpha keeps
-- the fully transparent gaps around the bar from being blurred too.
-- no_anim turns off Hyprland's own open and close animation for these
-- surfaces: Scuttle animates them itself, and the two together show as
-- a rectangle growing behind the launcher.
local surfaces = { "scuttle-bar", "scuttle-launcher", "scuttle-toasts", "scuttle-osd", "scuttle-assistant" }
for _, surface in ipairs(surfaces) do
    hl.layer_rule({
        name = surface,
        match = { namespace = surface },
        blur = true,
        ignore_alpha = 0.3,
        no_anim = true,
    })
end

-- The settings window opens floating in the middle of the screen,
-- not tiled.
hl.window_rule({
    name = "scuttle-settings",
    match = { title = "^(Scuttle Settings)$" },
    float = true,
    center = true,
})
