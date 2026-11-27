# OpenSubsonic API reference

This folder holds a copy of the OpenSubsonic OpenAPI specification. It is a reference for
implementing the OpenSubsonic API in Polaris (see [OPENSUBSONIC.md](../OPENSUBSONIC.md)), and
the contract tests check Polaris's JSON responses against these schemas.

- **Source:** <https://github.com/opensubsonic/opensubsonic.github.io>, folder `openapi/`
- **Pinned commit:** `62ece71da1` (2026-11-12, "Move hosting from Netlify to GitHub Pages")
- **Spec version in `openapi.json`:** `1.16.1`
- **License:** Apache-2.0, see [LICENSE](LICENSE). The files are unmodified copies.

The specification is published at <https://opensubsonic.github.io/docs/>. Note that the OpenAPI
files describe JSON responses only: the XML form follows the original Subsonic
`subsonic-rest-api.xsd` schema, which is not included here.

## Updating

1. Download the repository archive at the new commit and replace the `openapi/` folder with its
   `openapi/` folder (and `LICENSE` if it changed).
2. Update the commit and version in this file.
3. Run `cargo test`. Schema changes that Polaris's responses no longer satisfy show up as failures
   in the `server::test::subsonic` tests.
