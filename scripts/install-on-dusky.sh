#!/usr/bin/env bash
# Installs Scuttle on top of an existing dusky setup, without touching
# any file that dusky's updater manages.
#
#   scripts/install-on-dusky.sh              install (safe to run again)
#   scripts/install-on-dusky.sh uninstall    put everything back
#   scripts/install-on-dusky.sh status       show what is in place
#
# Options for install:
#   --keep-waybar     leave Waybar running alongside Scuttle's bar
#   --keep-mako       leave mako in charge of notifications
#   --prefix DIR      install the binary under DIR/bin (default: ~/.local)
#   --binary FILE     use an already built scuttle binary instead of building
#   --no-system       only edit config files: no pacman, systemctl, build or
#                     starting and stopping programs
#
# What install changes, all of it undone by uninstall:
#   * adds ~/.config/hypr/edit_here/source/scuttle.lua (autostart, the
#     Super-tap launcher key, blur rules) and one require line for it in
#     ~/.config/hypr/edit_here/hyprland.lua
#   * comments out the Waybar autostart line in
#     ~/.config/hypr/edit_here/source/autostart.lua
#   * disables the mako user service
#
# dusky's own volume and brightness keys keep working: Scuttle shows the
# popups they send.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HYPR="${XDG_CONFIG_HOME:-$HOME/.config}/hypr"
EDIT="$HYPR/edit_here"
LOADER="$EDIT/hyprland.lua"
OURS="$EDIT/source/scuttle.lua"
AUTOSTART="$EDIT/source/autostart.lua"
STATE="${XDG_STATE_HOME:-$HOME/.local/state}/scuttle/dusky"
REQUIRE='require("edit_here.source.scuttle")'
MARK='-- [scuttle] '

ACTION=install
KEEP_WAYBAR=0
KEEP_MAKO=0
SYSTEM=1
PREFIX="$HOME/.local"
BINARY=""

say() { printf '%s\n' "$*"; }
step() { printf '\n==> %s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

while [ $# -gt 0 ]; do
    case "$1" in
        install | uninstall | status) ACTION="$1" ;;
        --keep-waybar) KEEP_WAYBAR=1 ;;
        --keep-mako) KEEP_MAKO=1 ;;
        --no-system) SYSTEM=0 ;;
        --prefix) PREFIX="${2:?--prefix needs a directory}"; shift ;;
        --binary) BINARY="${2:?--binary needs a file}"; shift ;;
        -h | --help) sed -n '2,27p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) die "Unknown option: $1 (try --help)" ;;
    esac
    shift
done

# A real install edits one user's files and services. As root it would
# set up root's desktop instead of yours.
if [ "$SYSTEM" -eq 1 ] && [ "$(id -u)" -eq 0 ]; then
    die "Run this as your normal user, not as root. It asks for sudo when it needs it."
fi

require_dusky() {
    [ -f "$LOADER" ] || die "$LOADER does not exist, so this does not look like a dusky setup with a Lua Hyprland config.
For any other Hyprland setup, install Scuttle normally and run: scuttle hyprland"
}

# Copies a file into the backup folder once, before its first change.
backup() {
    local file="$1" name
    [ -f "$file" ] || return 0
    mkdir -p "$STATE/backup"
    name="$(printf '%s' "${file#"$HOME"/}" | tr '/' '_')"
    [ -f "$STATE/backup/$name" ] || cp -p "$file" "$STATE/backup/$name"
}

running_in_hyprland() {
    [ "$SYSTEM" -eq 1 ] && [ -n "${HYPRLAND_INSTANCE_SIGNATURE:-}" ] && command -v hyprctl > /dev/null
}

install_dependencies() {
    [ "$SYSTEM" -eq 1 ] || return 0
    command -v pacman > /dev/null || die "pacman was not found. This script is for Arch Linux."
    local packages=(gtk4 gtk4-layer-shell alsa-lib xdg-utils)
    # A rustup toolchain is as good as the rust package.
    command -v cargo > /dev/null || packages+=(rust)
    local missing=()
    for package in "${packages[@]}"; do
        pacman -Qq "$package" > /dev/null 2>&1 || missing+=("$package")
    done
    if [ "${#missing[@]}" -gt 0 ]; then
        step "Installing packages: ${missing[*]}"
        sudo pacman -S --needed --noconfirm "${missing[@]}"
    else
        say "All required packages are already installed."
    fi
}

build_and_install_binary() {
    local target="$PREFIX/bin/scuttle"
    if [ -z "$BINARY" ]; then
        [ "$SYSTEM" -eq 1 ] || die "--no-system needs --binary FILE, because it does not build."
        step "Building Scuttle (a few minutes the first time)"
        (cd "$ROOT" && cargo build --release --locked -p scuttle)
        BINARY="$ROOT/target/release/scuttle"
    fi
    if [ "$SYSTEM" -eq 1 ]; then
        [ -x "$BINARY" ] || die "$BINARY is not an executable file."
        step "Installing the binary to $target"
        # Stop a running copy first: its file cannot be replaced while busy.
        "$target" quit > /dev/null 2>&1 || true
        install -Dm755 "$BINARY" "$target"
    else
        target="$BINARY"
    fi
    INSTALLED_BINARY="$target"
}

write_hyprland_file() {
    step "Writing $OURS"
    mkdir -p "$(dirname "$OURS")"
    cat > "$OURS" << EOF
-- Scuttle on dusky. Written by Scuttle's scripts/install-on-dusky.sh.
-- This file lives in dusky's edit_here folder, which dusky's updater
-- leaves alone. Remove it with: scripts/install-on-dusky.sh uninstall

local scuttle = "$INSTALLED_BINARY"

-- Start Scuttle with the session.
hl.on("hyprland.start", function()
    hl.exec_cmd(scuttle)
end)

-- Tap Super on its own to open the launcher. Super held with another
-- key still works as a normal keybind.
hl.bind(
    "Super_L",
    hl.dsp.exec_cmd(scuttle .. " toggle launcher"),
    { description = "Scuttle launcher", release = true }
)

-- Blur what is behind Scuttle's translucent panels.
for _, surface in ipairs({ "scuttle-bar", "scuttle-launcher", "scuttle-toasts", "scuttle-osd" }) do
    hl.layer_rule({
        name = surface,
        match = { namespace = surface },
        blur = true,
        ignore_alpha = 0.3,
    })
end
EOF

    if grep -qF "$REQUIRE" "$LOADER"; then
        say "$LOADER already loads it."
    else
        backup "$LOADER"
        # Make sure the require starts on its own line.
        [ -z "$(tail -c1 "$LOADER")" ] || printf '\n' >> "$LOADER"
        printf '%s\n' "$REQUIRE" >> "$LOADER"
        say "Added the require line to $LOADER."
    fi
}

disable_waybar() {
    if [ "$KEEP_WAYBAR" -eq 1 ]; then
        say "Leaving Waybar alone (--keep-waybar)."
        return 0
    fi
    step "Turning off Waybar's autostart"
    if [ -f "$AUTOSTART" ] && grep -Eq '^[[:space:]]*hl\.exec_cmd\(.*waybar' "$AUTOSTART"; then
        backup "$AUTOSTART"
        # Comment the line out with a marker so uninstall can find it.
        sed -i -E "s|^([[:space:]]*)(hl\.exec_cmd\(.*waybar.*)$|\1${MARK}\2|" "$AUTOSTART"
        say "Commented out the Waybar line in $AUTOSTART."
    else
        say "No active Waybar autostart line found in $AUTOSTART."
    fi
    if [ "$SYSTEM" -eq 1 ]; then
        pkill -x waybar 2> /dev/null || true
    fi
}

disable_mako() {
    if [ "$KEEP_MAKO" -eq 1 ]; then
        say "Leaving mako in charge of notifications (--keep-mako)."
        local config="${XDG_CONFIG_HOME:-$HOME/.config}/scuttle/config.toml"
        if [ ! -f "$config" ]; then
            mkdir -p "$(dirname "$config")"
            printf '[notifications]\nenabled = false\n' > "$config"
            say "Wrote $config with notifications turned off."
        elif ! grep -Eq '^\s*enabled\s*=\s*false' "$config"; then
            say "Add this to $config so Scuttle does not try to show notifications:"
            say "    [notifications]"
            say "    enabled = false"
        fi
        return 0
    fi
    [ "$SYSTEM" -eq 1 ] || return 0
    step "Handing notifications from mako to Scuttle"
    if systemctl --user is-enabled mako.service > /dev/null 2>&1; then
        systemctl --user disable --now mako.service
        touch "$STATE/mako-was-enabled"
        say "Disabled mako.service."
    else
        say "mako.service is not enabled."
    fi
    pkill -x mako 2> /dev/null || true
}

start_scuttle() {
    running_in_hyprland || {
        say "Not inside a running Hyprland session, so nothing was started. Scuttle starts at your next login."
        return 0
    }
    step "Starting Scuttle"
    hyprctl reload > /dev/null 2>&1 || true
    "$INSTALLED_BINARY" quit > /dev/null 2>&1 || true
    sleep 0.3
    setsid -f "$INSTALLED_BINARY" > /dev/null 2>&1
    sleep 1
    if "$INSTALLED_BINARY" ping > /dev/null 2>&1; then
        say "Scuttle is running. Tap Super to open the launcher."
    else
        say "Scuttle did not answer. Run '$INSTALLED_BINARY' in a terminal to see why."
    fi
}

do_install() {
    require_dusky
    mkdir -p "$STATE"
    install_dependencies
    build_and_install_binary
    write_hyprland_file
    disable_waybar
    disable_mako
    start_scuttle
    step "Done"
    say "To undo all of this: $ROOT/scripts/install-on-dusky.sh uninstall"
}

do_uninstall() {
    require_dusky
    local binary="$PREFIX/bin/scuttle"

    step "Removing Scuttle from the Hyprland config"
    if [ "$SYSTEM" -eq 1 ] && [ -x "$binary" ]; then
        "$binary" quit > /dev/null 2>&1 || true
    fi
    rm -f "$OURS"
    if grep -qF "$REQUIRE" "$LOADER"; then
        # Drop only our own line, wherever it ended up.
        grep -vF "$REQUIRE" "$LOADER" > "$LOADER.scuttle-tmp" || true
        cat "$LOADER.scuttle-tmp" > "$LOADER"
        rm -f "$LOADER.scuttle-tmp"
        say "Removed the require line from $LOADER."
    fi

    step "Restoring Waybar and mako"
    if [ -f "$AUTOSTART" ] && grep -qF -- "$MARK" "$AUTOSTART"; then
        sed -i -E "s|^([[:space:]]*)-- \[scuttle\] |\1|" "$AUTOSTART"
        say "Restored the Waybar line in $AUTOSTART."
    fi
    if [ "$SYSTEM" -eq 1 ]; then
        if [ -f "$STATE/mako-was-enabled" ]; then
            systemctl --user enable --now mako.service || true
            rm -f "$STATE/mako-was-enabled"
            say "Enabled mako.service again."
        fi
        if running_in_hyprland; then
            hyprctl reload > /dev/null 2>&1 || true
            local toggle="$HOME/user_scripts/waybar/waybar_toggle.sh"
            if [ -x "$toggle" ]; then
                setsid -f "$toggle" --on > /dev/null 2>&1 || true
            fi
        fi
    fi

    step "Done"
    say "The scuttle binary ($binary) and your settings in ~/.config/scuttle were left in place."
    say "Delete them by hand if you no longer want them."
}

do_status() {
    local yes=yes no=no
    say "dusky override folder:    $([ -f "$LOADER" ] && echo "$yes" || echo "$no ($LOADER missing)")"
    say "Scuttle Hyprland file:    $([ -f "$OURS" ] && echo "$yes" || echo "$no")"
    say "Loaded by edit_here:      $([ -f "$LOADER" ] && grep -qF "$REQUIRE" "$LOADER" && echo "$yes" || echo "$no")"
    say "Waybar autostart off:     $([ -f "$AUTOSTART" ] && grep -qF -- "$MARK" "$AUTOSTART" && echo "$yes" || echo "$no")"
    say "Binary installed:         $([ -x "$PREFIX/bin/scuttle" ] && echo "$PREFIX/bin/scuttle" || echo "$no")"
    if [ "$SYSTEM" -eq 1 ]; then
        say "mako service enabled:     $(systemctl --user is-enabled mako.service 2> /dev/null || echo "$no")"
        say "Scuttle running:          $("$PREFIX/bin/scuttle" ping > /dev/null 2>&1 && echo "$yes" || echo "$no")"
    fi
}

case "$ACTION" in
    install) do_install ;;
    uninstall) do_uninstall ;;
    status) do_status ;;
esac
