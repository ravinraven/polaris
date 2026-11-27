//! Authentication of Subsonic requests.
//!
//! Supported: an API key (`apiKey`, the `apiKeyAuthentication` extension) and a username with a
//! password (`u` + `p`, plain or hex-encoded with an `enc:` prefix). Salted-token authentication
//! (`t` + `s`) needs the plaintext password, which Polaris does not keep, so it is not supported.

use crate::app::{auth as app_auth, config, state, Error};
use headers::authorization::{Bearer, Credentials};
use http::HeaderMap;

use super::guard::Guard;
use super::params::Params;
use super::response::{SubsonicError, API_VERSION_PARTS};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mechanism {
	ApiKey,
	Password,
	/// A Polaris session token, as the bundled web client uses.
	Session,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Authenticated {
	pub username: String,
	pub mechanism: Mechanism,
}

/// Checks the protocol version the client says it speaks (`v`). Compatible means the same
/// major version and a client minor version that is not newer than ours.
pub fn check_client_version(version: &str) -> Result<(), SubsonicError> {
	let numbers: Result<Vec<u32>, _> = version.split('.').map(str::parse::<u32>).collect();
	let client = match numbers.as_deref() {
		Ok([major, minor]) => (*major, *minor, 0),
		Ok([major, minor, patch]) => (*major, *minor, *patch),
		_ => {
			return Err(SubsonicError::generic(
				"Invalid protocol version (parameter v)",
			))
		}
	};
	let (server_major, server_minor, server_patch) = API_VERSION_PARTS;
	if client.0 < server_major {
		Err(SubsonicError::client_must_upgrade())
	} else if client.0 > server_major || (client.1, client.2) > (server_minor, server_patch) {
		Err(SubsonicError::server_must_upgrade())
	} else {
		Ok(())
	}
}

fn decode_password(raw: &str) -> Option<String> {
	let Some(hex) = raw.strip_prefix("enc:") else {
		return Some(raw.to_owned());
	};
	if hex.len() % 2 != 0 || !hex.is_ascii() {
		return None;
	}
	let bytes: Option<Vec<u8>> = (0..hex.len())
		.step_by(2)
		.map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
		.collect();
	String::from_utf8(bytes?).ok()
}

const OTHER_CREDENTIALS: [&str; 5] = ["apiKey", "u", "p", "t", "s"];

/// The Polaris session token of a request, from `auth_token` or an `Authorization: Bearer`
/// header, if there is one. A header never competes with credentials in the parameters.
pub fn session_token(params: &Params, headers: &HeaderMap) -> Option<String> {
	params.get("auth_token").map(str::to_owned).or_else(|| {
		if OTHER_CREDENTIALS.iter().any(|n| params.contains(n)) {
			return None;
		}
		headers
			.get(http::header::AUTHORIZATION)
			.and_then(Bearer::decode)
			.map(|b| b.token().to_owned())
	})
}

/// `enabled` says whether the OpenSubsonic API is switched on for third-party apps. Session
/// tokens are the web client's own, so they work either way.
pub async fn authenticate(
	params: &Params,
	headers: &HeaderMap,
	enabled: bool,
	config_manager: &config::Manager,
	state_manager: &state::Manager,
	guard: &Guard,
) -> Result<Authenticated, SubsonicError> {
	// A repeated credential parameter is ambiguous, so it is never guessed at.
	let credentials = ["apiKey", "u", "p", "t", "s", "auth_token"];
	if credentials.iter().any(|name| params.count(name) > 1) {
		return Err(SubsonicError::conflicting_mechanisms());
	}

	if let Some(token) = session_token(params, headers) {
		if OTHER_CREDENTIALS.iter().any(|n| params.contains(n)) {
			return Err(SubsonicError::conflicting_mechanisms());
		}
		let authorization = config_manager
			.authenticate(&app_auth::Token(token), app_auth::Scope::PolarisAuth)
			.await
			.map_err(|_| SubsonicError::wrong_credentials())?;
		return Ok(Authenticated {
			username: authorization.username,
			mechanism: Mechanism::Session,
		});
	}
	if !enabled {
		return Err(SubsonicError::wrong_credentials());
	}

	let has = |name: &str| params.contains(name);
	let has_token = has("t") || has("s");

	if has("apiKey") {
		if has("u") || has("p") || has_token {
			return Err(SubsonicError::conflicting_mechanisms());
		}
		let key = params.get("apiKey").unwrap_or_default();
		let owner = state_manager
			.authenticate_api_key(key)
			.await
			.map_err(|_| SubsonicError::generic("Internal error"))?;
		let Some(owner) = owner else {
			return Err(SubsonicError::invalid_api_key());
		};
		// A key does not outlive the account it belongs to.
		if config_manager.get_user(&owner).await.is_err() {
			return Err(SubsonicError::invalid_api_key());
		}
		return Ok(Authenticated {
			username: owner,
			mechanism: Mechanism::ApiKey,
		});
	}

	if has_token {
		if has("p") {
			return Err(SubsonicError::conflicting_mechanisms());
		}
		return Err(SubsonicError::token_auth_unsupported());
	}

	match (params.get("u"), params.get("p")) {
		(Some(username), Some(password)) => {
			guard.check(username).map_err(SubsonicError::rate_limited)?;
			let Some(password) = decode_password(password) else {
				guard.failed(username);
				return Err(SubsonicError::wrong_credentials());
			};

			// A password that was just checked is not checked again
			let stored = config_manager.get_user(username).await.ok();
			let key = stored
				.as_ref()
				.map(|u| Guard::verification_key(username, &password, &u.hashed_password));
			let signed_in = Authenticated {
				username: username.to_owned(),
				mechanism: Mechanism::Password,
			};
			if key.as_ref().is_some_and(|k| guard.is_verified(k)) {
				return Ok(signed_in);
			}

			match config_manager.login(username, &password).await {
				Ok(_) => {
					guard.succeeded(username);
					if let Some(key) = key {
						guard.remember_verified(key);
					}
					Ok(signed_in)
				}
				Err(Error::IncorrectUsername) | Err(Error::IncorrectPassword) => {
					guard.failed(username);
					Err(SubsonicError::wrong_credentials())
				}
				Err(_) => Err(SubsonicError::generic("Internal error")),
			}
		}
		(Some(_), None) => Err(SubsonicError::missing_parameter("p")),
		(None, Some(_)) => Err(SubsonicError::missing_parameter("u")),
		(None, None) => Err(SubsonicError::missing_parameter("apiKey")),
	}
}

#[cfg(test)]
mod test {
	use super::*;

	#[test]
	fn accepts_compatible_versions() {
		for v in ["1.16.1", "1.16.0", "1.16", "1.15.0", "1.1.0", "1.0.0"] {
			assert_eq!(check_client_version(v), Ok(()), "{v}");
		}
	}

	#[test]
	fn rejects_versions_the_server_cannot_serve() {
		assert_eq!(check_client_version("0.9.0").unwrap_err().code, 20);
		assert_eq!(check_client_version("2.0.0").unwrap_err().code, 30);
		assert_eq!(check_client_version("1.17.0").unwrap_err().code, 30);
		assert_eq!(check_client_version("1.16.2").unwrap_err().code, 30);
	}

	#[test]
	fn rejects_malformed_versions() {
		for v in ["", "1", "a.b.c", "1.2.3.4", "1..2", "-1.0.0", "1.16.1 "] {
			assert_eq!(check_client_version(v).unwrap_err().code, 0, "{v:?}");
		}
	}

	#[test]
	fn decodes_plain_and_hex_passwords() {
		assert_eq!(decode_password("sesame"), Some("sesame".to_owned()));
		assert_eq!(
			decode_password("enc:736573616d65"),
			Some("sesame".to_owned())
		);
		assert_eq!(decode_password("enc:"), Some(String::new()));
		assert_eq!(decode_password("enc:7"), None);
		assert_eq!(decode_password("enc:zz"), None);
		assert_eq!(decode_password("enc:ff"), None);
		assert_eq!(decode_password("enc:é"), None);
	}
}
