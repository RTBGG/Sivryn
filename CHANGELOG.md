# Changelog

What changed in each release, newest first. The release workflow uses the section of a tag as
the notes of its GitHub release and of its update.

## v0.1.0-alpha.17

### Fixed

- Muting or deafening with the keyboard shortcut could crash the client.
- A hidden window could stop reacting until it got an event of its own (Blitz polled its tasks
  only once per wake-up).

### Message box

- Long messages wrap onto new lines, and the box grows with its text (up to a third of the
  window). Enter sends, Shift+Enter starts a new line.

### Media player

- Unmuting a video or audio file goes back to the volume it had before muting, instead of 100 %.
- The audio player uses two rows: play, name and download on top, a wide timeline with time and
  volume below.
- Muting a video in fullscreen no longer crashes the client, and the fullscreen volume slider
  follows the mouse smoothly while dragging.

### Channel list

- The buttons that appear when hovering a channel (chat, settings) sit closer to the edge and no
  longer jump while a channel is being dragged.

### Windows

- Sivryn has an icon in the notification area: the Sivryn mark with your status, or in a call a
  green bubble that fills while you speak. Click it to open Sivryn; its menu checks for updates
  or quits.
- New settings page "Windows": start with Windows, start minimized (only the notification area
  icon) and close to the notification area. All three are on by default; starting with Windows
  is only offered by the installed version.
- During a call, the taskbar preview has buttons for camera, microphone, headphones and hanging
  up.
- Starting Sivryn while it is already running brings the running one to the front.

### Look

- Status dots are cut cleanly out of avatars, also on hovered and selected rows.
- Text longer than the message box can show scrolls inside it, with a scroll bar, and follows
  the cursor while typing.

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
