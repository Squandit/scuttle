# Roadmap

What is planned and not built yet, roughly in order. The README lists what works today.

## Next

- **LiMusic.** The bar's media widget works with any MPRIS player. Check it against LiMusic on a real machine, and add whatever LiMusic needs if it does not speak MPRIS.
- **Theme export without matugen.** Scuttle already matches the rest of the desktop through matugen in both directions. For setups without matugen, write Scuttle's own exporters for GTK, the common terminals, Hyprland borders and animation curves, and VS Code.
- **Lock screen.**
- **Tray extras.** Icons for apps that name an icon the theme does not have, and keyboard access to the tray.
- **Settings extras.** Drag to reorder bar widgets instead of typing their names, a colour picker beside the accent box, and a font list.

## After that

- **Clipboard extras.** Image previews in the detail pane, pinning, and deleting an entry from the launcher.
- **Screen capture.** Region, window and screen; record; colour picker; annotate; send a region to the assistant.
- **Keybind editor.** Searchable list, rebind by pressing keys, conflict check, an overlay cheatsheet.
- **Launcher extras.** Inline controls in results (a brightness slider when you type "bright"), open windows listed under an app, notification history as a mode.
- **Per-monitor workspaces** on the bar for multi-monitor setups.

## Assistant extras

The panel itself is built: both backends, the switch between them, desktop actions, and terminal commands with approval. Still to come:

- **See the screen.** Send a screen region or the focused window along with a question.
- **Answers in the launcher.** Short questions answered inline, without opening the panel.
- **Read aloud.** An optional button to speak an answer, using Fish Audio. Off until a key is set, and the key goes in the keyring like the Claude one.
- **Context.** Tell Claude which window is focused and what is playing, when the user asks about "this".
- **Keeping conversations** between restarts, as an option.

## Developer tools

Make a development setup that uses Fusion, VS Code and KiCad nicer to work in. Ideas to explore when this comes up: launcher entries for recent projects in each, matching themes for VS Code and KiCad from the theme engine, window rules so their dialogs and tool palettes behave under tiling, and per-project workspace layouts.

## Packaging

- Publish the PKGBUILD to the AUR.
- An install path for a fresh Arch system, once the shell is complete enough to be the whole desktop.
