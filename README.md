This project was vibe coded: it was written by an AI (Claude) with a human steering, testing and using it.
It is a port of [herbstluftwm](https://herbstluftwm.org) by Thorsten Wißmann and contributors, whose frame tree, layouts and commands it reimplements.

# hrwm

[![CI](https://github.com/everett1992/hrwm/actions/workflows/ci.yml/badge.svg)](https://github.com/everett1992/hrwm/actions/workflows/ci.yml)

A herbstluftwm-style manual tiling window manager for the [river](https://codeberg.org/river/river)
Wayland compositor (0.4 or later, which runs the window manager as a separate program).

Like herbstluftwm, you split the screen into frames that stay put even when
empty, put windows into them, and configure and drive everything from the
command line: `hrwm` is both the window manager and its `herbstclient`.

## Features

- herbstluftwm's frame tree: `split`, `remove`, `focus`, `shift`, `resize`,
  `rotate`, `cycle_layout`, and the vertical, horizontal, max and grid layouts,
  with the same gap and border arithmetic
- tags shown per monitor, `use`/`move`/`use_index`, swapping tags between monitors
- floating windows, tag floating mode, pseudotiling, fullscreen
- keybindings and mousebindings (move, resize, zoom), rules, settings
- frame decorations drawn like herbstluftwm's
- hooks for bars: `hrwm --idle`, `tag_status`, plus waybar helpers
- additions:
  - `swap DIR` trades places with the window `focus DIR` would reach
  - an `auto` layout follows the frame's long axis
  - `smart_gaps` keeps one window gap between frames instead of two
  - `keymap LAYOUT [VARIANT [OPTIONS]]` sets the keyboard layout
  - `jumpto urgent` (waiting on river to report urgency)

## Install

Prebuilt x86_64 Linux binaries are on the
[releases page](https://github.com/everett1992/hrwm/releases); they need
libxkbcommon and river 0.4. To build from source instead (needs Rust):

```sh
cargo install --path .
```

Have river start it from `~/.config/river/init`:

```sh
#!/bin/sh
exec hrwm
```

On startup and on `hrwm reload`, hrwm runs `~/.config/hrwm/autostart`, usually
a shell script calling `hrwm`, like a herbstluftwm autostart:

```sh
#!/bin/sh
hc() { hrwm "$@"; }
hc keyunbind --all
hc rename default 1
for t in 2 3 4; do hc add $t; done
hc keybind Mod4-Return spawn foot
hc keybind Mod4-u chain , split bottom 0.7 , shift -e down
hc keybind Mod4-o chain , split right 0.6 , shift -e right
hc keybind Mod4-h focus left
hc keybind Mod4-Shift-h shift left
hc keybind Mod4-1 use 1
hc mousebind Mod4-Button1 move
hc set window_gap 6
hc set smart_gaps 1
hc set default_frame_layout auto
hc rule focus=on
```

Most herbstluftwm autostarts work with few changes. `hrwm --help` lists the
client modes; commands mirror herbstluftwm's (`list_keybinds`, `list_clients`,
`dump`, `tag_status`, `attr`, ...).

### waybar

`hrwm --waybar-tag NAME` and `hrwm --waybar-title` print JSON for waybar
custom modules (`"return-type": "json"`). The tag module sets classes
(`this-monitor`, `other-monitor`, `focused`, `occupied`, `empty`, `urgent`)
relative to the bar's own monitor.

## Development

See [CONTRIBUTING.md](CONTRIBUTING.md) for setup and the checks CI runs
(rustfmt, clippy, cargo-nextest and an end-to-end test in a headless river).
Pushing a `v*` tag builds a release.

`examples/vinput.rs` drives a nested or headless river session with a virtual
keyboard and pointer, for testing keybindings without real input:

```sh
WLR_BACKENDS=headless WLR_RENDERER=pixman river -c hrwm &
cargo run --example vinput -- key 64 28    # Super+Return
```

## License

herbstluftwm's Simplified BSD License (SPDX: BSD-2-Clause-Views); see
[LICENSE](LICENSE). Since hrwm's frame tree, layouts and command semantics are
ported from herbstluftwm, its copyright notice is kept there alongside hrwm's.
The river protocol files in `protocol/` are copyright Isaac Freund under the
MIT license stated in each file.
