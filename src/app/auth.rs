use std::time::{SystemTime, UNIX_EPOCH};

use pbkdf2::phc::PasswordHash;
use pbkdf2::{Algorithm, Params, PasswordHasher, PasswordVerifier, Pbkdf2};
use serde::{Deserialize, Serialize};

use crate::app::Error;

#[derive(Clone, Default)]
pub struct Secret(pub [u8; 32]);

impl AsRef<[u8]> for Secret {
	fn as_ref(&self) -> &[u8] {
		&self.0
	}
}

impl AsMut<[u8]> for Secret {
	fn as_mut(&mut self) -> &mut [u8] {
		&mut self.0
	}
}

#[derive(Debug)]
pub struct Token(pub String);

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum Scope {
	PolarisAuth,
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Authorization {
	pub username: String,
	pub scope: Scope,
	pub generation: u32,
}

pub fn checksum(input: &str) -> u32 {
	crc32fast::hash(input.as_bytes())
}

// Parameters used by every version of polaris so far (the pbkdf2 crate's old defaults).
// Newer versions of the crate recommend 600,000 rounds, which makes logins much slower
// on small servers, so raising this is a deliberate decision rather than a side effect
// of a dependency upgrade. Verification reads parameters from each stored hash, so
// changing these never invalidates existing passwords.
const PASSWORD_HASH_ROUNDS: u32 = 10_000;
const PASSWORD_HASH_LENGTH: usize = 32;

fn password_hasher() -> Result<Pbkdf2, Error> {
	let params = Params::new_with_output_len(PASSWORD_HASH_ROUNDS, PASSWORD_HASH_LENGTH)
		.map_err(|_| Error::PasswordHashing)?;
	Ok(Pbkdf2::new(Algorithm::Pbkdf2Sha256, params))
}

pub fn hash_password(password: &str) -> Result<String, Error> {
	if password.is_empty() {
		return Err(Error::EmptyPassword);
	}
	match password_hasher()?.hash_password(password.as_bytes()) {
		Ok(h) => Ok(h.to_string()),
		Err(_) => Err(Error::PasswordHashing),
	}
}

pub fn verify_password(password_hash: &str, attempted_password: &str) -> bool {
	match PasswordHash::new(password_hash) {
		Ok(h) => Pbkdf2::default()
			.verify_password(attempted_password.as_bytes(), &h)
			.is_ok(),
		Err(_) => false,
	}
}

pub fn generate_auth_token(
	authorization: &Authorization,
	auth_secret: &Secret,
) -> Result<Token, Error> {
	let serialized_authorization =
		serde_json::to_string(&authorization).or(Err(Error::AuthorizationTokenEncoding))?;
	branca::encode(
		serialized_authorization.as_bytes(),
		auth_secret.as_ref(),
		SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs() as u32,
	)
	.or(Err(Error::BrancaTokenEncoding))
	.map(Token)
}

pub fn decode_auth_token(
	auth_token: &Token,
	scope: Scope,
	auth_secret: &Secret,
) -> Result<Authorization, Error> {
	let Token(data) = auth_token;
	let ttl = match scope {
		Scope::PolarisAuth => 0, // permanent
	};
	let authorization =
		branca::decode(data, auth_secret.as_ref(), ttl).map_err(|_| Error::InvalidAuthToken)?;
	let authorization: Authorization =
		serde_json::from_slice(&authorization[..]).map_err(|_| Error::InvalidAuthToken)?;
	if authorization.scope != scope {
		return Err(Error::IncorrectAuthorizationScope);
	}
	Ok(authorization)
}

#[cfg(test)]
mod test {
	use super::*;

	#[test]
	fn hashes_and_verifies_passwords() {
		let hash = hash_password("correct horse").unwrap();
		assert!(hash.starts_with("$pbkdf2-sha256$i=10000,l=32$"));
		assert!(verify_password(&hash, "correct horse"));
		assert!(!verify_password(&hash, "wrong horse"));
	}

	#[test]
	fn salts_are_unique() {
		assert_ne!(
			hash_password("same password").unwrap(),
			hash_password("same password").unwrap()
		);
	}

	#[test]
	fn rejects_empty_password() {
		assert!(hash_password("").is_err());
	}

	#[test]
	fn verifies_hashes_stored_by_older_versions() {
		// Generated independently (Python hashlib.pbkdf2_hmac) in the PHC format that
		// previous versions of polaris wrote to disk: sha256, 10000 rounds, 32 byte output.
		let legacy = "$pbkdf2-sha256$i=10000,l=32$MDEyMzQ1Njc4OWFiY2RlZg$nqeTxoWzYobMFj7Cw/CfZWZ/iURKmeX+/GSX6Q2oYYE";
		assert!(verify_password(legacy, "legacy-password"));
		assert!(!verify_password(legacy, "other-password"));
	}

	#[test]
	fn rejects_malformed_hashes() {
		assert!(!verify_password("not a hash", "anything"));
		assert!(!verify_password("", "anything"));
	}
}
