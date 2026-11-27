//! Protection of password sign-ins: a limit on failed attempts, and a short memory of
//! successful ones.
//!
//! Only passwords need the limit: API keys are long random secrets that cannot be guessed, and
//! are not affected when a user's password sign-in is locked.

use std::{
	collections::HashMap,
	sync::Mutex,
	time::{Duration, Instant},
};

use pbkdf2::sha2::{Digest, Sha256};

/// Failed attempts allowed per user within [`WINDOW`].
const MAX_FAILURES: u32 = 10;
/// Failures are counted from the first one for this long, and sign-in stays locked until then.
const WINDOW: Duration = Duration::from_secs(10 * 60);
/// How long a successful password check is remembered.
const VERIFIED_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_TRACKED_USERS: usize = 10_000;
const MAX_VERIFIED: usize = 1_000;

#[derive(Default)]
pub struct Guard {
	failures: Mutex<HashMap<String, (u32, Instant)>>,
	verified: Mutex<HashMap<[u8; 32], Instant>>,
}

impl Guard {
	/// `Err(seconds)` if the user has failed too often and must wait that long.
	pub fn check(&self, username: &str) -> Result<(), u64> {
		self.check_at(username, Instant::now())
	}

	pub fn failed(&self, username: &str) {
		self.failed_at(username, Instant::now());
	}

	/// A successful sign-in forgets earlier failures.
	pub fn succeeded(&self, username: &str) {
		self.failures.lock().unwrap().remove(username);
	}

	fn check_at(&self, username: &str, now: Instant) -> Result<(), u64> {
		let mut failures = self.failures.lock().unwrap();
		let Some((count, since)) = failures.get(username).copied() else {
			return Ok(());
		};
		let elapsed = now.saturating_duration_since(since);
		if elapsed >= WINDOW {
			failures.remove(username);
			return Ok(());
		}
		if count >= MAX_FAILURES {
			Err((WINDOW - elapsed).as_secs() + 1)
		} else {
			Ok(())
		}
	}

	fn failed_at(&self, username: &str, now: Instant) {
		let mut failures = self.failures.lock().unwrap();
		if failures.len() >= MAX_TRACKED_USERS && !failures.contains_key(username) {
			// Forget the expired entries, and if that is not enough, all of them
			failures.retain(|_, (_, since)| now.saturating_duration_since(*since) < WINDOW);
			if failures.len() >= MAX_TRACKED_USERS {
				failures.clear();
			}
		}
		let entry = failures.entry(username.to_owned()).or_insert((0, now));
		if now.saturating_duration_since(entry.1) >= WINDOW {
			*entry = (0, now);
		}
		entry.0 += 1;
	}

	/// What identifies a password check: the user, the password and the stored hash, so a
	/// changed password is never taken for a verified one. Only a digest is kept.
	pub fn verification_key(username: &str, password: &str, stored_hash: &str) -> [u8; 32] {
		let mut hasher = Sha256::new();
		for part in [username, password, stored_hash] {
			hasher.update((part.len() as u64).to_le_bytes());
			hasher.update(part.as_bytes());
		}
		hasher.finalize().into()
	}

	pub fn is_verified(&self, key: &[u8; 32]) -> bool {
		self.is_verified_at(key, Instant::now())
	}

	pub fn remember_verified(&self, key: [u8; 32]) {
		self.remember_verified_at(key, Instant::now());
	}

	fn is_verified_at(&self, key: &[u8; 32], now: Instant) -> bool {
		self.verified
			.lock()
			.unwrap()
			.get(key)
			.is_some_and(|at| now.saturating_duration_since(*at) < VERIFIED_TTL)
	}

	fn remember_verified_at(&self, key: [u8; 32], now: Instant) {
		let mut verified = self.verified.lock().unwrap();
		if verified.len() >= MAX_VERIFIED {
			verified.retain(|_, at| now.saturating_duration_since(*at) < VERIFIED_TTL);
			if verified.len() >= MAX_VERIFIED {
				verified.clear();
			}
		}
		verified.insert(key, now);
	}
}

#[cfg(test)]
mod test {
	use super::*;

	#[test]
	fn users_are_locked_after_too_many_failures_and_released_when_the_window_ends() {
		let guard = Guard::default();
		let start = Instant::now();
		for _ in 0..MAX_FAILURES - 1 {
			assert_eq!(guard.check_at("alice", start), Ok(()));
			guard.failed_at("alice", start);
		}
		assert_eq!(guard.check_at("alice", start), Ok(()), "one attempt left");
		guard.failed_at("alice", start);

		let wait = guard
			.check_at("alice", start + Duration::from_secs(1))
			.unwrap_err();
		assert!(
			wait > WINDOW.as_secs() - 5 && wait <= WINDOW.as_secs(),
			"{wait}"
		);
		assert_eq!(
			guard.check_at("bob", start),
			Ok(()),
			"other users are not affected"
		);

		let almost = start + WINDOW - Duration::from_secs(1);
		assert!(guard.check_at("alice", almost).is_err());
		assert_eq!(guard.check_at("alice", start + WINDOW), Ok(()));
		// After the window the count starts again
		guard.failed_at("alice", start + WINDOW);
		assert_eq!(guard.check_at("alice", start + WINDOW), Ok(()));
	}

	#[test]
	fn a_successful_sign_in_forgets_the_failures() {
		let guard = Guard::default();
		let now = Instant::now();
		for _ in 0..MAX_FAILURES - 1 {
			guard.failed_at("alice", now);
		}
		guard.succeeded("alice");
		for _ in 0..MAX_FAILURES - 1 {
			guard.failed_at("alice", now);
		}
		assert_eq!(guard.check_at("alice", now), Ok(()));
	}

	#[test]
	fn the_limit_is_per_exact_user_name() {
		let guard = Guard::default();
		let now = Instant::now();
		for _ in 0..MAX_FAILURES {
			guard.failed_at("alice", now);
		}
		assert!(guard.check_at("alice", now).is_err());
		assert_eq!(guard.check_at("Alice", now), Ok(()));
		assert_eq!(guard.check_at("alice ", now), Ok(()));
	}

	#[test]
	fn the_failure_table_cannot_grow_without_bound() {
		let guard = Guard::default();
		let now = Instant::now();
		for i in 0..MAX_TRACKED_USERS + 50 {
			guard.failed_at(&format!("user{i}"), now);
		}
		assert!(guard.failures.lock().unwrap().len() <= MAX_TRACKED_USERS);
	}

	#[test]
	fn verified_passwords_are_remembered_for_a_while() {
		let guard = Guard::default();
		let now = Instant::now();
		let key = Guard::verification_key("alice", "secret", "hash1");
		assert!(!guard.is_verified_at(&key, now));
		guard.remember_verified_at(key, now);
		assert!(guard.is_verified_at(&key, now + Duration::from_secs(60)));
		assert!(!guard.is_verified_at(&key, now + VERIFIED_TTL));
	}

	#[test]
	fn a_different_password_user_or_stored_hash_is_never_taken_for_a_verified_one() {
		let base = Guard::verification_key("alice", "secret", "hash1");
		for other in [
			Guard::verification_key("alice", "Secret", "hash1"),
			Guard::verification_key("alicE", "secret", "hash1"),
			Guard::verification_key("alice", "secret", "hash2"),
			Guard::verification_key("alice", "secre", "thash1"),
			Guard::verification_key("alices", "ecret", "hash1"),
		] {
			assert_ne!(base, other);
		}
		assert_eq!(base, Guard::verification_key("alice", "secret", "hash1"));
	}

	#[test]
	fn the_verified_table_cannot_grow_without_bound() {
		let guard = Guard::default();
		let now = Instant::now();
		for i in 0..MAX_VERIFIED + 50 {
			guard.remember_verified_at(Guard::verification_key("u", &i.to_string(), "h"), now);
		}
		assert!(guard.verified.lock().unwrap().len() <= MAX_VERIFIED);
	}
}
