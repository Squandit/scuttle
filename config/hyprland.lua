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

-- Blur what is behind Scuttle's translucent panels. ignore_alpha keeps
-- the fully transparent gaps around the bar from being blurred too.
hl.layer_rule({
    name = "scuttle-bar",
    match = { namespace = "scuttle-bar" },
    blur = true,
    ignore_alpha = 0.3,
})

hl.layer_rule({
    name = "scuttle-launcher",
    match = { namespace = "scuttle-launcher" },
    blur = true,
    ignore_alpha = 0.3,
})
