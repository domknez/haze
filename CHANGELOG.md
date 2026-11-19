# Changelog

## Unreleased

## 0.1.5 (2026-11-19)

- Fill the window: Z (or Settings) lets the picture cover the window, its edges cut,
  instead of black bars, and stays that way
- A sets the picture's shape (16:9 or 4:3) for a stream that has it wrong, until the
  next channel
- ⌘, (Ctrl+, on Windows and Linux) opens the settings
- Menus too long for the window scroll

## 0.1.4 (2026-11-19)

- A mini player: P (or the button beside full screen) keeps the picture alone in a
  small window above the others; P, Esc or a double-click bring the full window back

## 0.1.3 (2026-11-19)

- Updates are signed: haze installs a new version only when its checksums carry the
  signature of the key the releases are made with
- An AirPlay speaker's buttons are taken only from the speaker haze plays on

## 0.1.2 (2026-11-18)

- A movie or an episode carried on from where it was left shows the right position,
  and its place is no longer lost after ten seconds
- Passwords of Xtream logins no longer end up in the log; a keychain that can't be
  read no longer loses the other sources' passwords on the next save
- Zapping waits for the channel before to let go of the provider and the speaker
- A live channel that keeps ending is tried a few times, not over and over
- Sound comes back on this computer's default output after the device in use is
  unplugged
- Settings, sources, favourites and progress are saved whole or not at all; a file
  that can't be read is kept aside instead of being overwritten

## 0.1.1 (2026-11-05)

- Full screen on macOS is really full screen: the menu bar, the title bar and the Dock
  stay away, even with the pointer at the top of the screen. F, Esc or a double-click
  leave it
- The display no longer dims or sleeps under a playing picture on macOS

## 0.1.0 (2026-10-30)

The first version of haze:

- Sources: M3U links and files (or a file dropped on the window) and Xtream Codes
  logins, several at once; passwords in the system keychain
- Live channels with logos, groups, search, favourites (in an order of your own:
  dragged, or moved from their right-click menu) and recently watched; zapping
  with ↑ ↓, the last channel back on the next start, reconnecting when a stream drops
- Hide groups (right-click one): they leave the lists, search and zapping until
  Settings › Show hidden groups, which lasts until haze quits; a PIN can lock them
- Guide (XMLTV) with the programme on now and what's next, the full guide (G) with
  the channels of any group, or the favourites, against the time, and catch-up from the archive (Xtream, and
  M3U `catchup` modes)
- An Xtream panel's playlist link (`get.php`) signs in to the panel's API, for
  posters, ratings, plots and seasons; sources saved as such a link move over on
  their own, favourites included
- Movies and series from the provider's video library, also sorted out of plain
  playlists; the next episode starts on its own
- Movies and episodes carry on where they were left; a bar shows how much of each
  was watched and a tick the ones seen
- Subtitles: text tracks from the stream, or an `.srt` / `.vtt` file dropped on the
  window
- AirPlay 2 and AirPlay 1 output with lip sync, the speaker's buttons, media keys and
  Now Playing; a picture delay per output
- Channel names read cleanly: the raised letters providers tag them with (`ᶠʰᵈ`,
  `ᴴᴰ`) become plain capitals, accents sit on their letters, and `|` no longer looks
  like `I`
- GPU decoding (VideoToolbox, D3D11, VA-API) and YUV drawn by a shader
- Updates itself: new versions download and verify in the background, then
  *Restart to update* shows in the top bar (Settings › Updates; `haze check-update`
  and `haze self-update` from the command line)

