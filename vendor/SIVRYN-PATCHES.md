# Patched Blitz crates

Dioxus Native 0.7 renders with Blitz 0.2. Two of its crates are vendored here with small
patches and wired in through `[patch.crates-io]` in the workspace `Cargo.toml`. Both are
upstream https://github.com/dioxuslabs/blitz, licensed MIT OR Apache-2.0. Search for
"Sivryn patch" to find every change.

## blitz-paint 0.2.1

- `src/render.rs`: the text input caret uses the element's text colour instead of
  hard-coded black (upstream supports `caret-color` from Blitz 0.3).
- `src/render.rs`: `<input type="password">` is painted as evenly spaced dots, with caret
  and selection drawn at the dot positions (`draw_password_input`). Upstream has no
  password masking yet. Mouse hit-testing still uses the real glyph positions.

## blitz-dom 0.2.4

- `src/layout/construct.rs`: text inputs honour `font-family` and `font-weight`; before,
  they always used the default font.

Drop a patch once Sivryn moves to a Blitz release that covers it.
