# Patched crates

Dioxus Native 0.7 renders with Blitz 0.2. Three of its crates are vendored here with small
patches and wired in through `[patch.crates-io]` in the workspace `Cargo.toml`. All are
upstream https://github.com/dioxuslabs/blitz, licensed MIT OR Apache-2.0. Search for
"Sivryn patch" to find every change.

## blitz-paint 0.2.1

- `src/render.rs`: the text input caret uses the element's text colour instead of
  hard-coded black (upstream supports `caret-color` from Blitz 0.3).
- `src/render.rs`: `<input type="password">` is painted as evenly spaced dots, with caret
  and selection drawn at the dot positions (`draw_password_input`). Upstream has no
  password masking yet. Mouse hit-testing still uses the real glyph positions.
- `src/render.rs` (`draw_border_edge`): edges without a border width are skipped. Filling the
  empty edge shape still drew a faint anti-aliased hairline along borderless elements.
- `src/text.rs`: text baselines are snapped to whole device pixels so hinted glyphs land on
  the pixel grid; fractional baselines made text look blurry at 100 % display scaling.

## blitz-dom 0.2.4

- `src/layout/construct.rs`: text inputs honour `font-family` and `font-weight`; before,
  they always used the default font.
- `src/caret.rs` (+ `document.rs`, `events/keyboard.rs`, `events/mouse.rs`, and
  `blitz-paint/src/render.rs`): the text caret blinks at the Windows rate (530 ms) and
  restarts visible on typing and clicks. The app schedules a repaint at each toggle.
- `src/bundled_fonts.rs` (+ `document.rs`, `lib.rs`): apps register their bundled font files
  before creating a document. Blitz only loads `@font-face` from external stylesheets and
  ignored the rules in Dioxus' inline `<style>` blocks, so Inter and Twemoji were never used.
- `src/text_selection.rs` (+ `events/mod.rs`, `events/keyboard.rs`, `lib.rs` and
  `blitz-paint/src/render.rs`): static text inside an element with the class `selectable` can
  be selected with the mouse (drag, double-click for a word, triple-click for a line) and
  copied with Ctrl+C. Upstream only selects inside text inputs.
- `src/stick_bottom.rs` (+ `document.rs`): a scroll container with a `data-stick-bottom`
  attribute starts at the bottom and follows new content until the user scrolls up (chat logs).
- `src/node/node.rs` (`Node::hit`): a node's own box is hit-tested before its scroll offset is
  applied, and content overflowing a clipping box is ignored. Upstream added the offset first,
  so clicks above or beside a scrolled container landed on its hidden, scrolled-away children.
  Inline text only counts as hit where a line actually has text: `Cluster::from_point` snaps
  to the nearest glyph, so a click in the empty rest of a line opened the link at its end.
- `src/zoom_anchor.rs` (+ `document.rs`, `lib.rs`): a scroll container with a `data-zoom`
  attribute (the current scale) keeps the point in its middle in place when the scale changes,
  instead of zoomed content growing from the top left corner (image viewer).

## blitz-shell 0.2.3

- `src/file_drop.rs`, `src/window.rs`: files dragged onto the window are forwarded to a
  handler the app installs (`blitz_shell::file_drop::set_handler`); Blitz ignored them.
- `src/scrolling.rs`, `src/window.rs`: browser-like wheel scrolling (100 px per notch, eased
  over ~45 ms instead of jumping), middle-click autoscroll (speed grows with the distance from
  where it started; any click ends it) and Ctrl+wheel going to an app handler
  (`blitz_shell::scrolling::set_ctrl_wheel_handler`) instead of zooming the page.
- `src/escape.rs`, `src/window.rs`: Escape goes to an app handler first
  (`blitz_shell::escape::set_escape_handler`). Key events only reach the focused element, so
  overlays without a focused child (image viewer, settings) never saw Escape.
- `src/window.rs` (`View::redraw`): nothing is drawn while the window is minimized. Its size is
  0×0 then, and drawing to the surface panicked ("failed to get surface texture: Outdated"),
  which closed the app on minimize.

## sonora-agc2 0.2.0 and sonora-common-audio 0.2.0

Part of https://github.com/dignifiedquire/sonora (BSD-3-Clause), the Rust port of WebRTC's
audio processing used for echo cancellation and automatic gain.

- `sonora-agc2/src/vad_wrapper.rs`, `sonora-common-audio/src/channel_buffer.rs`,
  `sonora-common-audio/src/sinc_resampler.rs` and both `Cargo.toml`: hand-written `Debug` impls replace
  `derive_more::Debug`. Cargo unifies features, so `derive_more/debug` would also be
  enabled for stylo (Blitz's CSS engine), whose `#[macro_use] extern crate derive_more`
  then shadows the standard `Debug` derive and fails to compile.

The 11 MB of test audio in `sonora-agc2/tests/` is left out; the crate's own tests aren't run here.

Drop a patch once Sivryn moves to a release that covers it.
