# OpenSubsonic API

Polaris can speak the [OpenSubsonic](https://opensubsonic.github.io/docs/) API, so that apps
written for Subsonic servers can browse and play your collection. This is work in progress: the
framework and the system endpoints are done, the endpoints that serve your music are not yet.

## Enabling it

The API is **off by default**. An administrator turns it on with the `opensubsonic_enabled`
setting, either in the configuration file:

```toml
opensubsonic_enabled = true
```

or with `PUT /api/settings` and `{ "opensubsonic_enabled": true }`. While it is off, every URL
under `/rest` answers `404`.

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
that is not form-encoded) and `413` (a `POST` body over 64 KiB).

Responses are sent with `Cache-Control: no-store` and `X-Content-Type-Options: nosniff`. Any web
page may call the API (`Access-Control-Allow-Origin: *`): requests carry their own credentials and
never rely on cookies, so this does not let a page act as a signed-in user.

## Supported

| Endpoint | Notes |
| --- | --- |
| `ping` | |
| `getLicense` | Always valid: there is nothing to license. |
| `getOpenSubsonicExtensions` | The only endpoint that needs no credentials. |
| `tokenInfo` | Only with `apiKey` authentication. |

Extensions advertised: `apiKeyAuthentication` (1), `formPost` (1).

Every other endpoint currently answers with error `0` ("Endpoint not supported"). Browsing,
searching, streaming, cover art and playlists come next, followed by stars, ratings, play counts
and bookmarks. Video, podcasts, internet radio, chat, jukebox, sharing and transcoding are not
planned.

## Decisions worth knowing about

These are places where the specification is silent or where Polaris deliberately differs.

- **XML namespace.** Subsonic's XML uses the namespace `http://subsonic.org/restapi`, and Polaris
  declares it on the root element. The OpenSubsonic documentation does not show one in its
  examples (it omits the namespace and the XML declaration), so this follows the original
  Subsonic schema.
- **Errors are HTTP 200.** The specification does not state a status code for failed responses;
  this follows Subsonic.
- **Passwords are supported, hashes unchanged.** Checking a password costs one PBKDF2 verification
  per request, the same as logging in to the regular API.
- **Several credentials, or the same one twice, are an error (`43`)** rather than a guess.
- **Object IDs** are opaque strings (`so_...`, `al_...`, `ar_...`, `pl_...`, `di_...`), generated
  once and stored, so they survive rescans. They are case sensitive.

## Reference

`docs/opensubsonic/` contains an unmodified copy of the OpenSubsonic OpenAPI files (Apache-2.0),
pinned to a commit; see its [README](opensubsonic/README.md). The tests validate Polaris's JSON
responses against those schemas.

The specification describes JSON only. XML responses follow the original Subsonic
`subsonic-rest-api.xsd`, which is not vendored, so XML is covered by hand-written assertions.

## Trying it

```sh
KEY=...   # an API key from Settings, API Keys
curl "http://localhost:5050/rest/getOpenSubsonicExtensions?f=json"
curl "http://localhost:5050/rest/ping?apiKey=$KEY&v=1.16.1&c=curl&f=json"
```

Compatibility with specific apps has not been verified yet. Web-based clients are the first
target; reports of what works and what does not are welcome.
