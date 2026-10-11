# Contributing

Thanks for helping. hrwm was vibe coded (see the README), so careful review and
real-world testing are especially welcome.

## Setup

You need a recent stable Rust (1.88 or later), libxkbcommon, and, for the
end-to-end test, river 0.4, foot and any font. On Arch:

```sh
sudo pacman -S rustup libxkbcommon river foot ttf-dejavu
rustup default stable
cargo install cargo-nextest --locked
```

## Before you open a pull request

Run what CI runs:

```sh
cargo fmt --check                                # formatting
cargo clippy --locked --all-targets -- -D warnings
cargo nextest run --locked                       # unit tests
cargo build --all-targets && tests/smoke.sh      # end to end, in a headless river
```

`tests/smoke.sh` starts its own headless river in a temporary runtime
directory, so it is safe to run from inside a river session.

To try your build interactively, run river nested in your current session
(`river -c path/to/target/debug/hrwm`) or hot-swap the running window manager.
River keeps windows open while the window manager restarts.

## Guidelines

- Keep herbstluftwm's behaviour and command names where hrwm has an
  equivalent. Port from its source rather than guessing, and mention which
  part you ported. Additions should be clearly marked as hrwm extensions.
- Add a unit test in `src/tree.rs` for layout or frame logic, and extend
  `tests/smoke.sh` for behaviour that needs a real compositor.
- The files in `protocol/` are river's protocol definitions, copied verbatim.
  Update them from a river release, don't edit them.
- Pull requests from outside contributors run CI only after a maintainer
  approves the workflow run.

## Security issues

Please report vulnerabilities privately through GitHub's
"Report a vulnerability" button on the Security tab, not in a public issue.
