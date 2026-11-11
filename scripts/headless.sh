#!/usr/bin/env bash
# Runs Scuttle inside a headless Sway session with a mock Hyprland, so the
# shell can be exercised and screenshotted on a machine with no display.
#
#   scripts/headless.sh start            start the session and Scuttle
#   scripts/headless.sh shot <file.png>  save a screenshot
#   scripts/headless.sh ctl <args...>    run a `scuttle` command in the session
#   scripts/headless.sh stop             stop everything
#
# Needs: sway, grim, and a built `scuttle` (cargo build).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RUN="${SCUTTLE_HEADLESS_DIR:-/tmp/scuttle-headless}"
BIN="${SCUTTLE_BIN:-$ROOT/target/debug/scuttle}"
MOCK="$ROOT/target/debug/examples/mock-hyprland"

export XDG_RUNTIME_DIR="$RUN/runtime"
export XDG_CONFIG_HOME="${SCUTTLE_HEADLESS_CONFIG:-$RUN/config}"
export XDG_DATA_HOME="$RUN/data"
export XDG_CACHE_HOME="$RUN/cache"
export WAYLAND_DISPLAY=wayland-1
export HYPRLAND_INSTANCE_SIGNATURE=mock
export XDG_CURRENT_DESKTOP=Hyprland
# Software rendering: there is no GPU in a headless session.
export WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER=pixman
export GSK_RENDERER=cairo GDK_BACKEND=wayland
export SWAYSOCK="$XDG_RUNTIME_DIR/sway.sock"
# A private D-Bus session, so Scuttle can run its notification service.
export DBUS_SESSION_BUS_ADDRESS="unix:path=$XDG_RUNTIME_DIR/bus"

start() {
    mkdir -p "$XDG_RUNTIME_DIR" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_CACHE_HOME"
    chmod 700 "$XDG_RUNTIME_DIR"
    # SCUTTLE_HEADLESS_BG is a colour like #1c1613 or the path of an image.
    local bg="${SCUTTLE_HEADLESS_BG:-#1c1613}"
    # Sway reads relative paths from its config folder, so make it absolute.
    if [ -f "$bg" ]; then bg="$(realpath "$bg") fill"; else bg="$bg solid_color"; fi
    cat > "$RUN/sway.conf" <<CONF
output HEADLESS-1 resolution ${SCUTTLE_HEADLESS_SIZE:-1440x900} scale ${SCUTTLE_HEADLESS_SCALE:-1} bg $bg
default_border none
gaps inner 12
CONF
    sway -c "$RUN/sway.conf" > "$RUN/sway.log" 2>&1 &
    echo $! > "$RUN/sway.pid"
    for _ in $(seq 50); do
        [ -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" ] && break
        sleep 0.1
    done
    [ -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" ] || { echo "Sway did not start. See $RUN/sway.log" >&2; exit 1; }

    if command -v dbus-daemon > /dev/null; then
        dbus-daemon --session --nofork --address="$DBUS_SESSION_BUS_ADDRESS" > "$RUN/dbus.log" 2>&1 &
        echo $! > "$RUN/dbus.pid"
        sleep 0.2
    fi

    "$MOCK" "$XDG_RUNTIME_DIR/hypr/mock" > "$RUN/mock.log" 2>&1 &
    echo $! > "$RUN/mock.pid"
    sleep 0.2

    "$BIN" "$@" > "$RUN/scuttle.log" 2>&1 &
    echo $! > "$RUN/scuttle.pid"
    sleep "${SCUTTLE_HEADLESS_WAIT:-1.5}"
    echo "Started. Logs are in $RUN."
}

stop() {
    for name in scuttle mock dbus sway; do
        if [ -f "$RUN/$name.pid" ]; then
            kill "$(cat "$RUN/$name.pid")" 2>/dev/null || true
            rm -f "$RUN/$name.pid"
        fi
    done
    sleep 0.2
    rm -rf "$XDG_RUNTIME_DIR"
}

case "${1:-}" in
    start) shift; start "$@" ;;
    stop) stop ;;
    shot) grim "${2:?Give a file name for the screenshot}" ;;
    ctl) shift; "$BIN" "$@" ;;
    env) env | grep -E '^(XDG_|WAYLAND_|HYPRLAND_|SWAYSOCK|GSK_|GDK_|DBUS_)' ;;
    *) sed -n '2,11p' "$0"; exit 2 ;;
esac
