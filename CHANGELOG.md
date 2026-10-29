# Changelog

## Unreleased

The first version of haze:

- Sources: M3U links and files (or a file dropped on the window) and Xtream Codes
  logins, several at once; passwords in the system keychain
- Live channels with logos, groups, search, favourites and recently watched; zapping
  with ↑ ↓, the last channel back on the next start, reconnecting when a stream drops
- Hide groups (right-click one): they leave the lists, search and zapping until
  Settings › Show hidden groups, which lasts until haze quits; a PIN can lock them
- Guide (XMLTV) with the programme on now and what's next, the full guide (G) with
  the group's channels against the time, and catch-up from the archive (Xtream, and
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
