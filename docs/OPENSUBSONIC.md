# OpenSubsonic API

Polaris can speak the [OpenSubsonic](https://opensubsonic.github.io/docs/) API, so that apps
written for Subsonic servers can browse, search and play your collection. It implements version
1.16.1 of the API, with the extensions listed below.

## Enabling it

The API is **off by default**. An administrator turns it on:

- in the web interface, under *Settings, API Keys*, with the OpenSubsonic switch (administrators
  only), or
- with the `opensubsonic_enabled` setting, in the configuration file:

  ```toml
  opensubsonic_enabled = true
  ```

  or with `PUT /api/settings` and `{ "opensubsonic_enabled": true }`.

While it is off, every URL under `/rest` answers `404`.

## Signing in

Apps authenticate with every request. Polaris supports two ways:

- **API key** (recommended). Create one in the web interface under *Settings, API Keys*, and paste
  it into the app. A key is shown once, can be revoked at any time, and is only ever accepted by
  the OpenSubsonic API, never by the regular Polaris API. Sent as `apiKey=...`.
- **Username and password**: `u=...&p=...`, where the password is plain or hex-encoded with an
  `enc:` prefix.

Salted-token authentication (`t=...&s=...`) is **not** supported: it requires the server to know
your password in clear text, and Polaris only stores a hash. Apps that can only do this cannot
connect. Such requests get error `41`.

Passwords and keys travel in the URL of every request, so use HTTPS when the server is reachable
from outside your network. Polaris itself logs only the path of requests, never the query string.

**Limits on guessing.** After ten wrong passwords for a user name within ten minutes, password
sign-in for that name is refused (HTTP `429` with a `Retry-After` header) until the ten minutes
are over, even for the right password. API keys cannot be guessed, are not limited, and keep
working during such a lockout. A correct password forgets earlier failures. Counts are kept in
memory, per server.

## What a request looks like

`/rest/<endpoint>` or `/rest/<endpoint>.view`, with `GET`, or `POST` with an
`application/x-www-form-urlencoded` body (the `formPost` extension). Common parameters:

| Parameter | Meaning |
| --- | --- |
| `v` | Protocol version the app speaks (required). Polaris implements `1.16.1`. |
| `c` | Name of the app (required). |
| `f` | `xml` (the default), `json` or `jsonp` (needs a `callback`, restricted to plain identifiers). |
| `apiKey`, `u`, `p` | Credentials, see above. |

Failures are reported inside a normal `200` response with `status="failed"` and an `error` code,
as Subsonic does. Other HTTP statuses are used only for: `404` (API disabled), `415` (a `POST`
that is not form-encoded), `413` (a `POST` body over 64 KiB) and `429` (too many failed
sign-ins). Streaming answers `206` and `416` to range requests.

Responses are sent with `Cache-Control: no-store` and `X-Content-Type-Options: nosniff` (cover art
may be cached for an hour). Any web page may call the API (`Access-Control-Allow-Origin: *`):
requests carry their own credentials and never rely on cookies, so this does not let a page act
as a signed-in user.

## Supported

| Area | Endpoints |
| --- | --- |
| System | `ping`, `getLicense`, `getOpenSubsonicExtensions` (needs no credentials), `tokenInfo` (API key only) |
| Browsing | `getMusicFolders`, `getIndexes`, `getMusicDirectory`, `getGenres`, `getArtists`, `getArtist`, `getAlbum`, `getSong` |
| Information | `getArtistInfo`, `getArtistInfo2`, `getAlbumInfo`, `getAlbumInfo2`, `getSimilarSongs`, `getSimilarSongs2` (answered with empty descriptions: Polaris has no information from outside the library) |
| Lists | `getAlbumList`, `getAlbumList2` (all ten types), `getRandomSongs`, `getSongsByGenre`, `getStarred`, `getStarred2`, `getTopSongs` |
| Search | `search2`, `search3` |
| Media | `stream`, `download`, `getCoverArt`, `getAvatar` (always "not found": there are no avatars) |
| Playlists | `getPlaylists`, `getPlaylist`, `createPlaylist`, `updatePlaylist`, `deletePlaylist` |
| What users do | `star`, `unstar`, `setRating`, `scrobble`, `getNowPlaying`, `reportPlayback` |
| Bookmarks and queue | `getBookmarks`, `createBookmark`, `deleteBookmark`, `getPlayQueue`, `savePlayQueue`, `getPlayQueueByIndex`, `savePlayQueueByIndex` |
| Lyrics | `getLyricsBySongId`, `getLyrics` |
| Server | `getUser`, `getUsers`, `getScanStatus`, `startScan` |

Extensions advertised: `apiKeyAuthentication` (1), `formPost` (1), `playbackReport` (1),
`indexBasedQueue` (1), `topSongsByArtistId` (1), `songLyrics` (1).

**Not supported** (they answer with error `0`, "Endpoint not supported"): video, podcasts,
internet radio, chat, jukebox, sharing, transcoding (`getTranscodeDecision` and
`getTranscodeStream`, and the `transcoding` and `transcodeOffset` extensions), sonic similarity,
the original `search` endpoint, and creating, changing or deleting users and changing passwords
(use the Polaris web interface).

## How things map onto Polaris

- **IDs** are opaque, case-sensitive strings (`so_...` songs, `al_...` albums, `ar_...` artists,
  `pl_...` playlists, `di_...` folders), generated once and stored, so they survive rescans.
  Music folders have numeric IDs, as the specification wants, derived from the folder's name.
- **Albums** are the albums of Polaris's index: a name together with its artists. **Artists** are
  identified by name.
- **Folders.** `getIndexes` and `getMusicDirectory` follow the file tree of each music folder.
  `getMusicDirectory` also opens albums and artists, which the non-ID3 endpoints hand out as
  directories. `musicFolderId` is checked and honored by `getIndexes`; the tag-based endpoints
  accept it but do not filter by folder.
- **Streaming** sends the original file, with range support. Polaris does not transcode, so the
  format, bit rate and time offset an app asks for are ignored.
- **Playlists** are the Polaris playlists of the signed-in user. They are always private
  (`public` is false), a name can be used once per user, and renaming keeps the playlist's ID.
  Polaris playlists do not record when they were made or changed: those times start when a
  playlist is first seen through this API. Administrators can read other users' playlists, but
  only owners can change them.
- **Stars, ratings and plays** belong to a user. `scrobble` counts a play of a song and of its
  album; a "now playing" notification is only kept in memory. `getNowPlaying` shows everyone to
  administrators and only yourself to other users. Top songs come from the user's own play
  counts, and leave out songs never played.
- **Lyrics** come from an `.lrc` file next to the song (synced) and from the song's tags (plain,
  or synced if the text is in LRC form). The language is reported as `xxx`.
- **Tags read from files**: songs report their BPM, MusicBrainz ID and ReplayGain, and albums the
  MusicBrainz ID of their songs. They are read when asked for and remembered in memory, so the
  index is not affected.
- **Search** text is always treated as text: it cannot be used to reach Polaris's own query
  syntax. An empty query, or `*`, lists everything, as apps expect when they sync a library.
- Page sizes are capped at 500.

## Decisions worth knowing about

These are places where the specification is silent or where Polaris deliberately differs.

- **XML namespace.** Subsonic's XML uses the namespace `http://subsonic.org/restapi`, and Polaris
  declares it on the root element. The OpenSubsonic documentation does not show one in its
  examples (it omits the namespace and the XML declaration), so this follows the original
  Subsonic schema.
- **Errors are HTTP 200.** The specification does not state a status code for failed responses;
  this follows Subsonic.
- **A reported play** (`reportPlayback`) is counted once, when the player says a song is
  *starting*, unless `ignoreScrobble` is set or the user was already playing that song.
- **Several credentials, or the same one twice, are an error (`43`)** rather than a guess.
- **Everything about a request is checked before anything changes**: starring, rating,
  scrobbling and playlist or queue changes with a bad ID change nothing.

## Reference

`docs/opensubsonic/` contains an unmodified copy of the OpenSubsonic OpenAPI files (Apache-2.0),
pinned to a commit; see its [README](opensubsonic/README.md). The tests validate Polaris's JSON
responses against those schemas, and check that every endpoint Polaris lists exists in the
specification.

The specification describes JSON only. XML responses follow the original Subsonic
`subsonic-rest-api.xsd`, which is not vendored, so XML is covered by hand-written assertions.

## Trying it

```sh
KEY=...   # an API key from Settings, API Keys
curl "http://localhost:5050/rest/getOpenSubsonicExtensions?f=json"
curl "http://localhost:5050/rest/getAlbumList2?apiKey=$KEY&v=1.16.1&c=curl&f=json&type=newest"
curl -o song.mp3 "http://localhost:5050/rest/stream?apiKey=$KEY&v=1.16.1&c=curl&id=<a song ID>"
```

## What has and has not been verified

Every endpoint has automated tests that run against a real server, in both JSON and XML where
the shapes differ, and the JSON is validated against the specification's own schemas. A real
browser page on another origin has been used to call the API.

Not verified: **compatibility with specific apps** (none has been tried yet; web-based clients are
the first target, and reports of what works and what does not are welcome), and **performance on
large libraries**. Album lists and search work on the in-memory index and read tags from files
the first time they are needed, which should be fine for personal libraries but has not been
measured at scale.
