#!/bin/bash
# End-to-end check against a real river 0.4: start a headless river running
# hrwm, open terminals through a keybinding (virtual keyboard) and through
# commands, and check the resulting layout.
#
# Needs river, foot and a font installed, and debug builds of hrwm and the
# vinput example:
#   cargo build && cargo build --example vinput && tests/smoke.sh
#
# Runs in its own XDG_RUNTIME_DIR, so it never talks to a running session.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
hrwm=$root/target/debug/hrwm
vinput=$root/target/debug/examples/vinput
tmp=$(mktemp -d)
export XDG_RUNTIME_DIR=$tmp/run XDG_CONFIG_HOME=$tmp/config
mkdir -p "$XDG_RUNTIME_DIR" "$XDG_CONFIG_HOME/hrwm"
chmod 700 "$XDG_RUNTIME_DIR"
unset WAYLAND_DISPLAY DISPLAY

cat > "$XDG_CONFIG_HOME/hrwm/autostart" <<EOF
#!/bin/sh
hc() { "$hrwm" "\$@"; }
hc rename default one
hc add two
hc set window_gap 0
hc set frame_gap 0
hc set window_border_width 0
hc set default_frame_layout auto
hc rule focus=on
hc keybind Mod4-Return spawn foot
touch "$tmp/autostart-done"
EOF
chmod +x "$XDG_CONFIG_HOME/hrwm/autostart"

river_pid=
cleanup() {
    status=$?
    [[ -n $river_pid ]] && kill "$river_pid" 2>/dev/null && wait "$river_pid" 2>/dev/null
    if (( status != 0 )); then
        echo "--- river/hrwm log"
        cat "$tmp/river.log" || true
    fi
    rm -rf "$tmp"
}
trap cleanup EXIT

WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_HEADLESS_OUTPUTS=1 WLR_LIBINPUT_NO_DEVICES=1 \
    river -no-xwayland -c "echo \$WAYLAND_DISPLAY > '$tmp/display'; exec '$hrwm'" \
    > "$tmp/river.log" 2>&1 &
river_pid=$!

for _ in $(seq 100); do
    [[ -f $tmp/autostart-done ]] && break
    sleep 0.1
done
[[ -f $tmp/autostart-done ]] || { echo "FAIL: autostart did not run"; exit 1; }
export WAYLAND_DISPLAY
WAYLAND_DISPLAY=$(cat "$tmp/display")

fail() { echo "FAIL: $*"; exit 1; }
clients() { "$hrwm" list_clients | wc -l; }
wait_clients() {
    for _ in $(seq 50); do
        (( $(clients) == $1 )) && return 0
        sleep 0.1
    done
    fail "expected $1 windows, got $(clients)"
}

[[ $("$hrwm" tag_status) == $'\t#one\t.two\t' ]] || fail "tag_status: $("$hrwm" tag_status)"

# a keybinding through river's xkb bindings
"$vinput" key 64 28 # Super+Return
wait_clients 1

# a second window: the auto layout puts it beside the first on a wide screen
"$hrwm" spawn foot
wait_clients 2
sleep 0.3
mapfile -t geoms < <("$hrwm" list_clients | cut -f4)
[[ ${geoms[0]#*+} != "${geoms[1]#*+}" ]] || fail "windows overlap: ${geoms[*]}"
"$hrwm" dump | grep -q '^(clients auto:1 0x1 0x2)$' || fail "dump: $("$hrwm" dump)"

# frames: split and move the focused window into the new frame
"$hrwm" chain , split bottom 0.5 , shift -e down
"$hrwm" dump | grep -q '^(split vertical:0.500000:1 (clients auto:0 0x1) (clients auto:0 0x2))$' \
    || fail "split: $("$hrwm" dump)"

# tags
"$hrwm" move two
"$hrwm" tag_status | grep -q ':two' || fail "move: $("$hrwm" tag_status)"
"$hrwm" use two
[[ $("$hrwm" attr tags.focus.name) == two ]] || fail "use"

# input settings: accepted (a headless river has no libinput devices to show them)
"$hrwm" input tap on || fail "input tap on"
"$hrwm" input accel_speed 0.2 touchpad || fail "input accel_speed"
if "$hrwm" input tap sideways 2>/dev/null; then fail "input accepted a bad value"; fi

echo "smoke test passed"
