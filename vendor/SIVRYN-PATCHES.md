# Patched crates

Dioxus Native 0.7 renders with Blitz 0.2. Three of its crates are vendored here with small
patches and wired in through `[patch.crates-io]` in the workspace `Cargo.toml`. All are
upstream https://github.com/dioxuslabs/blitz, licensed MIT OR Apache-2.0. Search for
"Sivryn patch" to find every change.

## blitz-paint 0.2.1

- `src/render.rs` (`draw_canvas`): canvases larger than 8192 px (the usual GPU texture limit,
  e.g. a zoomed image) render into a smaller texture that is stretched to the canvas; the
  full-size texture failed validation and crashed the app.
- `src/text.rs` (`stroke_text_cut`), `src/render.rs` (`draw_inline_layout`): one line of text in
  a box with `white-space: nowrap` and `overflow: hidden` that doesn't fit is cut off with an
  ellipsis (three dots in the text colour). Stylo has `text-overflow` disabled, so that pair is
  the cue.
- `src/render.rs` (`render_element`): draws the scroll bar thumb of scroll containers (see
  `scrollbar.rs` in blitz-dom).
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

- `src/layout/inline.rs`: `white-space: nowrap` keeps text on one line; upstream wrapped it.
- `src/layout/mod.rs` (textarea): the text wraps at the textarea's width (upstream kept it on one
  endless line), and a textarea with `data-autogrow` is as tall as its text. A textarea taller
  than its box scrolls (`overflow-y: auto`) and follows the caret while typing
  (`input_editing::keep_caret_visible`, called from `events/keyboard.rs`). The wrap width is only
  handed to the editor when it changes (`TextInputData::set_wrap_width`): parley lays out the
  whole text again for every width it gets, which made long text lag. blitz-paint draws only the
  lines in view (`draw_text_input_text`).
- `src/layout/construct.rs`: text inputs honour `font-family` and `font-weight`; before,
  they always used the default font.
- `src/node/element.rs` (`TextInputData::set_text`): a value set by the app moves the caret to
  the end, as in browsers. It stayed at its old byte position, e.g. between `@` and an inserted
  mention.
- `src/stylo_to_cursor_icon.rs`: `cursor: none` falls back to the default pointer; upstream hit a
  `todo!()` and crashed the app.
- `src/document.rs` (`forget_removed_nodes`, + `mutator.rs`), `src/traversal.rs`
  (`node_layout_ancestors`): removing nodes clears hover, active, mousedown and focus references
  to them, and a missing node ends an ancestor path. The hovered item of a menu that closed on
  click stayed "hovered" and the next mouse move panicked with "invalid key".
- `src/events/driver.rs`: mouse presses and releases hit-test again instead of reusing the node
  from the last move. After the layout changed under a still pointer, clicks went to a stale or
  removed node and were lost.
- `src/input_editing.rs` (+ `events/mouse.rs`, `events/mod.rs`, `events/driver.rs`): in text
  inputs a double click selects a word and a triple click everything; a drag selection keeps
  going when the pointer leaves the input, and releasing it over another element doesn't click
  that element; only the main button moves the caret; a right click records the input and
  whether it has a selection, so the app can offer Copy, Cut and Paste (`request`), which
  blitz-shell runs as the matching Ctrl key; `request_focus` focuses an input by its `id`.
- `src/node/node.rs`, `src/mutator.rs` (`remove_node_if_unparented`): only drops nodes that were
  in the document once (`WAS_IN_DOCUMENT`). dioxus-native-dom calls it with stale element-id
  mappings; once the slab reused such a key for a cached template node, the template was dropped
  and its next use crashed ("invalid key" in `deep_clone_node`, a missing child in
  `node_at_path`).
- `src/scrollbar.rs` (+ `lib.rs`): vertical scroll bars, which Blitz didn't have. Elements with
  `overflow-y: auto | scroll` and overflowing content get a thin bar over their right edge (no
  layout space). blitz-paint draws it, blitz-shell passes the mouse to it: the thumb can be
  dragged, a click in the track jumps there, hovering brightens it.
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
- `src/document.rs` (`is_animating`): a `<canvas>` no longer makes the document count as
  animating. Upstream repainted the whole window at the display's refresh rate for as long as
  any canvas existed; with Vello that cost 10–20 % of a fast GPU for one small (even still)
  image. Canvas sources request repaints themselves (see `canvas_frames.rs` in blitz-shell).
- `src/zoom_anchor.rs` (+ `document.rs`, `lib.rs`): a scroll container with a `data-zoom`
  attribute (the current scale) keeps the point of its content under the mouse in place when the
  scale changes (the middle when the mouse is elsewhere), instead of zoomed content growing from
  the top left corner (image viewer). blitz-shell reports the mouse position.

## blitz-shell 0.2.3

- `src/file_drop.rs`, `src/window.rs`: files dragged onto the window are forwarded to a
  handler the app installs (`blitz_shell::file_drop::set_handler`); Blitz ignored them.
- `src/scrolling.rs`, `src/window.rs`: browser-like wheel scrolling (100 px per notch, eased
  over ~45 ms instead of jumping), middle-click autoscroll (speed grows with the distance from
  where it started; any click ends it) and Ctrl+wheel going to an app handler
  (`blitz_shell::scrolling::set_ctrl_wheel_handler`) instead of zooming the page. A plain wheel
  handler (`set_wheel_handler`) may take a notch before the page scrolls (zooming a screen share).
- `src/fullscreen.rs`, `src/window.rs` (`View::init`): `blitz_shell::fullscreen::set_fullscreen`
  puts the app's windows into borderless fullscreen and back, `set_cursor_visible` hides the
  pointer; apps had no handle to their window.
- `src/window.rs` (`View::poll`): polls the document until it has nothing left to do. Only a
  poll that ends pending registers the waker for the next piece of work; after one that did
  work, wake-ups were lost until some window event happened, so a hidden window stopped running
  its tasks (it couldn't be shown again from the notification area, nor quit).
- `src/window_control.rs`, `src/application.rs` (close requests): the app can show, hide and
  focus its window, get its native handle, and keep running when the close button is pressed
  (`set_close_handler`, used to close to the notification area).
- `src/window.rs` (mouse input): scroll bars get the pointer first; while one is dragged the
  page sees neither the moves nor the press and release.
- `src/canvas_frames.rs`, `src/window.rs` (`View::init`, `View::poll`):
  `blitz_shell::canvas_frames::request_frame()` asks every window for one repaint from any
  thread; requests before that repaint are coalesced. Video surfaces call it when a new frame
  arrives, so the window only repaints when something changed.
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
