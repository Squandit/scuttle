#!/usr/bin/env bash
# Tests scripts/install-on-dusky.sh against a throwaway home directory
# laid out like a dusky install. Nothing outside that directory is
# touched: the installer runs with --no-system, so it only edits files.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INSTALLER="$ROOT/scripts/install-on-dusky.sh"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

FAILED=0
pass() { printf '  ok    %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; FAILED=1; }
check() { if "${@:2}"; then pass "$1"; else fail "$1"; fi; }

# Builds a fake home with the files dusky creates.
make_home() {
    local home="$1"
    mkdir -p "$home/.config/hypr/edit_here/source" "$home/bin"
    printf '#!/bin/sh\nexit 0\n' > "$home/bin/scuttle"
    chmod +x "$home/bin/scuttle"
    cat > "$home/.config/hypr/edit_here/hyprland.lua" << 'EOF'
-- User overlay loader
require("edit_here.source.keybinds")
require("edit_here.source.autostart")
EOF
    cat > "$home/.config/hypr/edit_here/source/autostart.lua" << 'EOF'
hl.on("hyprland.start", function()
    hl.exec_cmd("awww-daemon")           -- Wallpaper engine

    -- --- OPTIONAL / USER INTERFACE ---
    hl.exec_cmd("$HOME/user_scripts/waybar/waybar_toggle.sh")
    -- hl.exec_cmd("nm-applet")
end)
EOF
    printf -- '-- keybinds\n' > "$home/.config/hypr/edit_here/source/keybinds.lua"
}

run() {
    local home="$1"
    shift
    HOME="$home" XDG_CONFIG_HOME="$home/.config" XDG_STATE_HOME="$home/.local/state" \
        HYPRLAND_INSTANCE_SIGNATURE="" "$INSTALLER" "$@" --no-system
}

snapshot() { (cd "$1/.config" && find . -type f | sort | xargs sha256sum); }

echo "Install into a dusky-shaped home"
HOME1="$WORK/one"
make_home "$HOME1"
BEFORE="$(snapshot "$HOME1")"
run "$HOME1" install --binary "$HOME1/bin/scuttle" > "$WORK/install.log" 2>&1 || { cat "$WORK/install.log"; fail "install exits cleanly"; }

OURS="$HOME1/.config/hypr/edit_here/source/scuttle.lua"
LOADER="$HOME1/.config/hypr/edit_here/hyprland.lua"
AUTOSTART="$HOME1/.config/hypr/edit_here/source/autostart.lua"

check "writes scuttle.lua" test -f "$OURS"
check "scuttle.lua starts the installed binary" grep -qF "local scuttle = \"$HOME1/bin/scuttle\"" "$OURS"
check "scuttle.lua binds the Super tap" grep -qF '"Super_L"' "$OURS"
check "scuttle.lua blurs every Scuttle surface" grep -qF '"scuttle-bar", "scuttle-launcher", "scuttle-toasts", "scuttle-osd"' "$OURS"
check "loader requires scuttle.lua once" test "$(grep -cF 'require("edit_here.source.scuttle")' "$LOADER")" = 1
check "loader keeps its other requires" grep -qF 'require("edit_here.source.keybinds")' "$LOADER"
check "Waybar line is commented out" grep -qF -- '-- [scuttle] hl.exec_cmd("$HOME/user_scripts/waybar/waybar_toggle.sh")' "$AUTOSTART"
check "other autostart lines are untouched" grep -qE '^    hl\.exec_cmd\("awww-daemon"\)' "$AUTOSTART"
check "already commented lines are not marked" grep -qE '^    -- hl\.exec_cmd\("nm-applet"\)' "$AUTOSTART"
check "originals are backed up" test -f "$HOME1/.local/state/scuttle/dusky/backup/.config_hypr_edit_here_source_autostart.lua"

echo "Run install a second time"
AFTER_FIRST="$(snapshot "$HOME1")"
run "$HOME1" install --binary "$HOME1/bin/scuttle" > "$WORK/again.log" 2>&1 || fail "second install exits cleanly"
check "nothing changes on a second run" test "$AFTER_FIRST" = "$(snapshot "$HOME1")"

echo "Status"
STATUS="$(run "$HOME1" status)"
check "status reports the Hyprland file" grep -q 'Scuttle Hyprland file:    yes' <<< "$STATUS"
check "status reports Waybar off" grep -q 'Waybar autostart off:     yes' <<< "$STATUS"

echo "Uninstall"
run "$HOME1" uninstall > "$WORK/uninstall.log" 2>&1 || { cat "$WORK/uninstall.log"; fail "uninstall exits cleanly"; }
check "scuttle.lua is gone" test ! -e "$OURS"
check "every config file is byte-for-byte as it started" test "$BEFORE" = "$(snapshot "$HOME1")"

echo "Options"
HOME2="$WORK/two"
make_home "$HOME2"
run "$HOME2" install --binary "$HOME2/bin/scuttle" --keep-waybar --keep-mako > /dev/null 2>&1
check "--keep-waybar leaves the Waybar line active" grep -qE '^    hl\.exec_cmd\("\$HOME/user_scripts/waybar/waybar_toggle.sh"\)' "$HOME2/.config/hypr/edit_here/source/autostart.lua"
check "--keep-mako turns Scuttle's notifications off" grep -qF 'enabled = false' "$HOME2/.config/scuttle/config.toml"

echo "Refuses a home that is not dusky"
HOME3="$WORK/three"
mkdir -p "$HOME3"
if run "$HOME3" install --binary /bin/true > "$WORK/notdusky.log" 2>&1; then
    fail "install must fail without dusky's edit_here folder"
else
    pass "install fails without dusky's edit_here folder"
fi
check "the error points to the normal install" grep -q 'scuttle hyprland' "$WORK/notdusky.log"
check "nothing was created" test ! -e "$HOME3/.config"

echo
if [ "$FAILED" -eq 0 ]; then
    echo "All installer checks passed."
else
    echo "Some installer checks failed."
    exit 1
fi
