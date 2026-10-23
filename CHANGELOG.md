# Changelog

What changed in each release, newest first. The release workflow uses the section of a tag as
the notes of its GitHub release and of its update.

## v0.1.0-alpha.16

### Message history for new members

- New members of a server can read the messages written before they joined, still end-to-end
  encrypted: messages also get a copy sealed with the channel's history key, and whoever adds
  someone to a channel hands them the keys inside MLS. Removing someone starts a new key.
- On by default; server settings have a switch ("New members see earlier messages"). It applies
  to messages sent from now on. Direct messages and groups don't share history.
- Needs the server update (database migration).

### Voice

- In a server's AFK channel the microphone is always muted and can't be unmuted there.

### Members and channels

- Right-clicking yourself in the member list offers the roles you can give, like for others.
- Channel names that don't fit stay on one line and end with "…" instead of wrapping.
  `white-space: nowrap` now works in general, and text cut off by `overflow: hidden` ends with an
  ellipsis.

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
