# Roadmap

What is planned and not built yet, roughly in order. The README lists what works today.

## Next

- **LiMusic.** The bar's media widget works with any MPRIS player. Check it against LiMusic on a real machine, and add whatever LiMusic needs if it does not speak MPRIS.
- **Theme export without matugen.** Scuttle already matches the rest of the desktop through matugen in both directions. For setups without matugen, write Scuttle's own exporters for GTK, the common terminals, Hyprland borders and animation curves, and VS Code.
- **Lock screen.**
- **Settings app.** Every row shows the config line it writes, and edits the same `config.toml`.

## After that

- **Clipboard extras.** Image previews in the detail pane, pinning, and deleting an entry from the launcher.
- **Screen capture.** Region, window and screen; record; colour picker; annotate; send a region to the assistant.
- **Keybind editor.** Searchable list, rebind by pressing keys, conflict check, an overlay cheatsheet.
- **Launcher extras.** Inline controls in results (a brightness slider when you type "bright"), open windows listed under an app, notification history as a mode.
- **Wallpaper-generated theme** as an option beside the hand-made ones.
- **Per-monitor workspaces** on the bar for multi-monitor setups.

## Assistant panel

A side panel, cloud only.

- **Backends.** Use the `claude` command first so it runs on the user's Claude subscription, with Opus 5.5 as the default model there. When the subscription reports its limit is reached, fall back to the API key with Haiku 5.5 as the default, and switch back when the limit resets. Both models are changeable in settings. Whichever of the two a user has set up should work by itself.
- **Abilities.** Control the desktop through Scuttle's own commands, with confirmation for anything risky. See a screen region or the focused window. Answer short questions inline in the launcher. Suggest terminal commands and run them after approval.
- **Read aloud.** An optional button to speak an answer, and to speak selected text from the launcher, using Fish Audio. Off until a key is set.
- **Keys.** API keys live in the system keyring. Never in the repo, the config file or logs.

## Developer tools

Make a development setup that uses Fusion, VS Code and KiCad nicer to work in. Ideas to explore when this comes up: launcher entries for recent projects in each, matching themes for VS Code and KiCad from the theme engine, window rules so their dialogs and tool palettes behave under tiling, and per-project workspace layouts.

## Packaging

- Publish the PKGBUILD to the AUR.
- An install path for a fresh Arch system, once the shell is complete enough to be the whole desktop.
