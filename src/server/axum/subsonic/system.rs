//! System endpoints: ping, getLicense, getOpenSubsonicExtensions and tokenInfo.

use super::auth::Mechanism;
use super::ctx::Ctx;
use super::response::{Element, SubsonicError};

/// The OpenSubsonic extensions this server supports, with their versions.
const EXTENSIONS: &[(&str, &[i32])] = &[
	("apiKeyAuthentication", &[1]),
	("formPost", &[1]),
	("playbackReport", &[1]),
	("indexBasedQueue", &[1]),
	("topSongsByArtistId", &[1]),
];

pub fn ping() -> Element {
	Element::new()
}

pub fn get_license() -> Element {
	// Polaris is free software: there is nothing to license or to expire.
	Element::new().object("license", Element::new().attr("valid", true))
}

pub fn get_open_subsonic_extensions() -> Element {
	let extensions = EXTENSIONS
		.iter()
		.map(|(name, versions)| {
			Element::new()
				.attr("name", *name)
				.scalars("versions", versions.to_vec())
		})
		.collect();
	Element::new().list("openSubsonicExtensions", extensions)
}

pub fn token_info(ctx: &Ctx) -> Result<Element, SubsonicError> {
	if ctx.mechanism != Mechanism::ApiKey {
		return Err(SubsonicError::mechanism_unsupported(
			"tokenInfo requires API key authentication",
		));
	}
	Ok(Element::new().object(
		"tokenInfo",
		Element::new().attr("username", ctx.user.clone()),
	))
}
