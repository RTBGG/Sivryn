# Changelog

What changed in each release, newest first. The release workflow uses the section of a tag as
the notes of its GitHub release and of its update.

## v0.1.0-alpha.15

### Fixed

- The client could crash when switching servers. The same cause was likely behind a rare crash
  while dragging channels.

### Message box

- Right-click menu with Cut and Copy (when text is selected), Paste (when the clipboard holds
  text) and Select all. A right click keeps the selection.
- Double-click selects a word, triple-click the whole text.
- A selection made by dragging keeps going when the mouse leaves the box, and releasing it over
  the "+" button no longer opens the file picker.
- After picking an emoji, the cursor is back in the message box, right after the emoji.
