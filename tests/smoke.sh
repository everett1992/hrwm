#!/bin/bash
# End-to-end check against a real river (0.4 or later): start a headless river running
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
activate=$root/target/debug/examples/activate
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
[[ $("$hrwm" list_tags) == $'one\tthis-monitor focused empty\ntwo\tempty' ]] \
    || fail "list_tags: $("$hrwm" list_tags)"

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

# focusing away from a fullscreen window lays it out over the monitor as a
# normal window (river draws nothing above a fullscreen one), so the focused
# window can be stacked above it
fs_geom() { "$hrwm" list_clients | awk -F'\t' '$1 == "0x2" {print $3, $4}'; }
"$hrwm" chain , fullscreen on , focus up
sleep 0.3
[[ $(fs_geom) == "fullscreen "*+0+0 ]] || fail "covering: $(fs_geom)"
"$hrwm" focus down
sleep 0.3
[[ $(fs_geom) == "fullscreen -" ]] || fail "refocused fullscreen: $(fs_geom)"
"$hrwm" fullscreen off

# tags
"$hrwm" move two
"$hrwm" tag_status | grep -q ':two' || fail "move: $("$hrwm" tag_status)"
"$hrwm" use two
[[ $("$hrwm" attr tags.focus.name) == two ]] || fail "use"
[[ $("$hrwm" attr tags.by-name.two.client_count) == 1 ]] || fail "by-name: $("$hrwm" attr tags.by-name.two.client_count)"
! "$hrwm" attr tags.by-name.nope.client_count 2>/dev/null || fail "by-name: missing tag"

# restarts keep tags, frame layouts and which window is where: river keeps
# the windows and the next hrwm finds them through the state file
"$hrwm" add three
"$hrwm" use one
"$hrwm" split right 0.3
state() {
    for t in one two three; do "$hrwm" dump "$t"; done | sed -E 's/0x[0-9a-f]+/W/g'
    "$hrwm" list_clients | cut -f2,3 | sort
    "$hrwm" attr tags.focus.name
}
wait_restart() {
    for _ in $(seq 100); do
        [[ -f $tmp/autostart-done ]] && "$hrwm" true 2>/dev/null && return 0
        sleep 0.1
    done
    fail "hrwm did not come back after $1"
}
before=$(state)
rm "$tmp/autostart-done"
"$hrwm" wmexec
wait_restart wmexec
[[ $(state) == "$before" ]] || fail $'wmexec lost state:\n'"$before"$'\n---\n'"$(state)"
kill -0 "$river_pid" || fail "river exited on wmexec"

wm_pid=$(pgrep -P "$river_pid" -x hrwm) || fail "no hrwm under river"
rm "$tmp/autostart-done"
kill "$wm_pid"
for _ in $(seq 50); do kill -0 "$wm_pid" 2>/dev/null || break; sleep 0.1; done
"$hrwm" >> "$tmp/river.log" 2>&1 &
wait_restart "a kill"
[[ $(state) == "$before" ]] || fail $'restart lost state:\n'"$before"$'\n---\n'"$(state)"

# input settings: accepted (a headless river has no libinput devices to show them)
"$hrwm" input tap on || fail "input tap on"
"$hrwm" input accel_speed 0.2 touchpad || fail "input accel_speed"
if "$hrwm" input tap sideways 2>/dev/null; then fail "input accepted a bad value"; fi

# gestures: river (0.5+) accepts their setup (a protocol error would end hrwm)
"$hrwm" gesturebind 3-swipe-left use_index +1 || fail "gesturebind swipe"
"$hrwm" gesturebind 2-pinch-out fullscreen toggle || fail "gesturebind pinch"
"$hrwm" gesturebind edge-bottom spawn foot || fail "gesturebind edge"
if "$hrwm" gesturebind 1-pinch-in close 2>/dev/null; then fail "gesturebind accepted a 1 finger pinch"; fi
"$hrwm" gestureunbind 2-pinch-out
[[ $("$hrwm" list_gesturebinds) == $'3-swipe-left\tuse_index\t+1\nedge-bottom\tspawn\tfoot' ]] \
    || fail "list_gesturebinds: $("$hrwm" list_gesturebinds)"
sleep 0.2
"$hrwm" attr tags.focus.name >/dev/null || fail "hrwm died after gesturebind"

# activation requests mark a window urgent instead of focusing it; needs a
# river with hrwm-activation-v1 (not upstream), so skip on a stock river
globals=$(WAYLAND_DEBUG=1 "$vinput" move 0 0 2>&1)
if [[ $globals == *'"hrwm_activation_v1"'* ]]; then
    "$hrwm" use one
    "$activate" 1 &
    activate_pid=$!
    wait_clients 3
    act=$("$hrwm" list_clients | awk -F'\t' '$0 ~ /hrwm-activate/ {print $1; exit}')
    [[ -n $act ]] || fail "activate window not found: $("$hrwm" list_clients)"
    "$hrwm" use two
    for _ in $(seq 50); do
        [[ $("$hrwm" attr "clients.$act.urgent") == true ]] && break
        sleep 0.1
    done
    [[ $("$hrwm" attr "clients.$act.urgent") == true ]] || fail "activation did not mark $act urgent"
    [[ $("$hrwm" attr tags.focus.name) == two ]] || fail "activation stole focus"
    "$hrwm" tag_status | grep -q '!one' || fail "urgent tag_status: $("$hrwm" tag_status)"
    "$hrwm" jumpto urgent
    [[ $("$hrwm" attr "clients.$act.urgent") == false ]] || fail "focus did not clear urgent"
    kill "$activate_pid"
else
    echo "skipping activation: river lacks hrwm_activation_v1"
fi

echo "smoke test passed"
