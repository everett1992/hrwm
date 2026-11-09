# river patch for activation urgency

Stock river drops windows' activation requests (xdg-activation-v1
`activate`, and on X11 `_NET_ACTIVE_WINDOW`, `_NET_WM_STATE_DEMANDS_ATTENTION`
and the ICCCM urgency hint), because river's window management protocol has
no event to pass them on. `river-0.5.0-hrwm-activation.patch` adds a small
global, `hrwm_activation_v1` (`../protocol/hrwm-activation-v1.xml`), that
sends `activation_requested` for a window to the window manager. hrwm marks
the window urgent instead of focusing it. Without the patch, hrwm never sees
the global and activation requests are ignored as before; nothing else changes.

The patch is against river 0.5.0 and may not apply to other versions.

TODO: river's own event for this is in review as
[river PR #1492](https://codeberg.org/river/river/pulls/1492)
(`river_window_v1.activation_requested`, protocol version 7). When a river
release ships it, have hrwm handle that event, then delete this directory and
`protocol/hrwm-activation-v1.xml` and stop binding `hrwm_activation_v1`.

## License

The patch modifies river, so it is under river's license, GPL-3.0-only
(`GPL-3.0-only.txt`), not hrwm's. It is not compiled into hrwm. The protocol
XML it adds is MIT.

## Applying it

From a river 0.5.0 checkout:

```sh
git am /path/to/hrwm/contrib/river-0.5.0-hrwm-activation.patch
zig build -Doptimize=ReleaseSafe --prefix ~/.local install
```

### Nix

Override nixpkgs' river (0.5.0) to add the patch. With hrwm as a flake input
(`hrwm = { url = "github:everett1992/hrwm"; flake = false; };`):

```nix
river = pkgs.river.overrideAttrs (old: {
  patches = (old.patches or [ ]) ++ [
    "${hrwm}/contrib/river-0.5.0-hrwm-activation.patch"
  ];
});
```

Or without flakes:

```nix
let
  hrwm = builtins.fetchGit { url = "https://github.com/everett1992/hrwm"; };
in
pkgs.river.overrideAttrs (old: {
  patches = (old.patches or [ ]) ++ [
    "${hrwm}/contrib/river-0.5.0-hrwm-activation.patch"
  ];
})
```

Then run that river instead of `pkgs.river`, e.g. in `home.packages` or your
session's `exec`. To check the running river has it, look for the global
with `wayland-info | grep hrwm_activation_v1` (from wayland-utils);
`tests/smoke.sh` also tests it end to end when it's there.
