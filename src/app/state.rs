//! Persistent state backing the OpenSubsonic API: API keys, generated object IDs,
//! per-user stars/ratings/play counts, bookmarks and play queues.
//!
//! Everything here lives in the native_db database (`polaris.ndb`). That is deliberately
//! separate from `collection.index`, which is a cache that can be rebuilt at any time:
//! this data is the only copy, so it should be backed up.

// Consumed by the OpenSubsonic endpoints, which are not implemented yet.
#![allow(dead_code)]
// `app::Error` is large; the other managers hit this lint too.
#![allow(clippy::result_large_err)]

use std::time::{SystemTime, UNIX_EPOCH};

use native_db::transaction::{RTransaction, RwTransaction};
use native_db::*;
use native_model::{native_model, Model};
use pbkdf2::sha2::{Digest, Sha256};
use rand::{rngs::SysRng, TryRng};
use serde::{Deserialize, Serialize};
use tokio::task::spawn_blocking;

use crate::app::{ndb, Error};

/// How often (at most) the `last_used_at` of an API key is written back.
const API_KEY_LAST_USED_GRANULARITY_SECS: i64 = 60 * 60;
/// Length of the short identifier used to refer to an API key without revealing it.
const API_KEY_ID_LENGTH: usize = 16;
const MAX_RATING: u8 = 5;

/// The URL-safe alphabet used by NanoID. It has exactly 64 symbols, so each one can be picked
/// from 6 bits of a random byte without any modulo bias.
const ID_ALPHABET: &[u8; 64] = b"useandom-26T198340PX75pxJACKVERYMINDBUSHWOLF_GQZbfghjklqvwyzrict";
/// 21 symbols of 6 bits each: 126 bits of randomness, the NanoID default.
const ID_LENGTH: usize = 21;

pub mod v1 {
	use super::*;

	/// An API key. Only a SHA-256 hash of the key is stored, never the key itself.
	#[derive(Clone, Debug, Default, Serialize, Deserialize)]
	#[native_model(id = 2, version = 1)]
	#[native_db]
	pub struct ApiKeyModel {
		#[primary_key]
		pub key_hash: String,
		#[secondary_key]
		pub owner: String,
		pub label: String,
		pub created_at: i64,
		pub last_used_at: Option<i64>,
	}

	/// Maps a generated, opaque object ID to the natural key of the object it refers to
	/// (eg. a virtual path for a song). The mapping is stable across library rescans.
	#[derive(Clone, Debug, Default, Serialize, Deserialize)]
	#[native_model(id = 3, version = 1)]
	#[native_db]
	pub struct ObjectIdModel {
		#[primary_key]
		pub id: String,
		/// `<kind prefix>\0<natural key>`
		#[secondary_key(unique)]
		pub lookup: String,
	}

	/// Stars, rating and play statistics of one user for one object.
	#[derive(Clone, Debug, Default, Serialize, Deserialize)]
	#[native_model(id = 4, version = 1)]
	#[native_db(primary_key(custom_id -> (&str, &str)))]
	pub struct ItemStateModel {
		#[secondary_key]
		pub owner: String,
		pub item_id: String,
		pub starred_at: Option<i64>,
		pub rating: Option<u8>,
		pub play_count: u32,
		pub last_played: Option<i64>,
	}

	impl ItemStateModel {
		fn custom_id(&self) -> (&str, &str) {
			(&self.owner, &self.item_id)
		}
	}

	#[derive(Clone, Debug, Default, Serialize, Deserialize)]
	#[native_model(id = 5, version = 1)]
	#[native_db(primary_key(custom_id -> (&str, &str)))]
	pub struct BookmarkModel {
		#[secondary_key]
		pub owner: String,
		pub song_id: String,
		pub position_ms: u64,
		pub comment: Option<String>,
		pub created_at: i64,
		pub changed_at: i64,
	}

	impl BookmarkModel {
		fn custom_id(&self) -> (&str, &str) {
			(&self.owner, &self.song_id)
		}
	}

	/// The saved play queue of a user (one per user).
	#[derive(Clone, Debug, Default, Serialize, Deserialize)]
	#[native_model(id = 6, version = 1)]
	#[native_db]
	pub struct PlayQueueModel {
		#[primary_key]
		pub owner: String,
		pub entries: Vec<String>,
		pub current_index: Option<u32>,
		pub position_ms: u64,
		pub changed_at: i64,
		pub changed_by: String,
	}
}

type ApiKeyModel = v1::ApiKeyModel;
type ApiKeyModelKey = v1::ApiKeyModelKey;
type ObjectIdModel = v1::ObjectIdModel;
type ObjectIdModelKey = v1::ObjectIdModelKey;
type ItemStateModel = v1::ItemStateModel;
type ItemStateModelKey = v1::ItemStateModelKey;
type BookmarkModel = v1::BookmarkModel;
type BookmarkModelKey = v1::BookmarkModelKey;
type PlayQueueModel = v1::PlayQueueModel;

pub use v1::{BookmarkModel as Bookmark, PlayQueueModel as PlayQueue};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiKeyInfo {
	/// Short identifier that can be shown and used to revoke the key.
	pub id: String,
	pub label: String,
	pub created_at: i64,
	pub last_used_at: Option<i64>,
}

/// A freshly created API key. This is the only time the key itself is available.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreatedApiKey {
	pub key: String,
	pub info: ApiKeyInfo,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ObjectKind {
	Song,
	Album,
	Artist,
	Playlist,
	Directory,
}

impl ObjectKind {
	fn prefix(self) -> &'static str {
		match self {
			ObjectKind::Song => "so",
			ObjectKind::Album => "al",
			ObjectKind::Artist => "ar",
			ObjectKind::Playlist => "pl",
			ObjectKind::Directory => "di",
		}
	}

	fn from_prefix(prefix: &str) -> Option<Self> {
		[
			ObjectKind::Song,
			ObjectKind::Album,
			ObjectKind::Artist,
			ObjectKind::Playlist,
			ObjectKind::Directory,
		]
		.into_iter()
		.find(|k| k.prefix() == prefix)
	}
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemState {
	pub item_id: String,
	pub starred_at: Option<i64>,
	pub rating: Option<u8>,
	pub play_count: u32,
	pub last_played: Option<i64>,
}

impl From<ItemStateModel> for ItemState {
	fn from(m: ItemStateModel) -> Self {
		Self {
			item_id: m.item_id,
			starred_at: m.starred_at,
			rating: m.rating,
			play_count: m.play_count,
			last_played: m.last_played,
		}
	}
}

impl ItemState {
	fn is_empty(&self) -> bool {
		self.starred_at.is_none()
			&& self.rating.is_none()
			&& self.play_count == 0
			&& self.last_played.is_none()
	}
}

#[derive(Clone)]
pub struct Manager {
	db: ndb::Manager,
}

fn db_err(e: native_db::db_type::Error) -> Error {
	Error::NativeDatabase(Box::new(e))
}

fn now() -> i64 {
	SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.map(|d| d.as_secs() as i64)
		.unwrap_or_default()
}

fn random_hex(num_bytes: usize) -> Result<String, Error> {
	let mut bytes = vec![0u8; num_bytes];
	SysRng
		.try_fill_bytes(&mut bytes)
		.map_err(|_| Error::EntropyUnavailable)?;
	Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Generates the random part of an object ID (NanoID style) from the operating system's
/// cryptographically secure random number generator. Fails rather than falling back to a
/// weaker source if the OS cannot provide randomness.
fn random_id() -> Result<String, Error> {
	let mut bytes = [0u8; ID_LENGTH];
	SysRng
		.try_fill_bytes(&mut bytes)
		.map_err(|_| Error::EntropyUnavailable)?;
	Ok(bytes
		.iter()
		.map(|b| ID_ALPHABET[(b & 0x3f) as usize] as char)
		.collect())
}

fn api_key_id(key_hash: &str) -> String {
	key_hash.chars().take(API_KEY_ID_LENGTH).collect()
}

fn hash_api_key(key: &str) -> String {
	// API keys are long random secrets, so a fast hash is sufficient (unlike passwords).
	Sha256::digest(key.as_bytes())
		.iter()
		.map(|b| format!("{b:02x}"))
		.collect()
}

fn lookup_key(kind: ObjectKind, natural_key: &str) -> String {
	format!("{}\0{}", kind.prefix(), natural_key)
}

impl Manager {
	pub fn new(db: ndb::Manager) -> Self {
		Self { db }
	}

	async fn read<T, F>(&self, f: F) -> Result<T, Error>
	where
		T: Send + 'static,
		F: FnOnce(&RTransaction) -> Result<T, Error> + Send + 'static,
	{
		let db = self.db.clone();
		spawn_blocking(move || {
			let transaction = db.r_transaction().map_err(db_err)?;
			f(&transaction)
		})
		.await?
	}

	async fn write<T, F>(&self, f: F) -> Result<T, Error>
	where
		T: Send + 'static,
		F: FnOnce(&RwTransaction) -> Result<T, Error> + Send + 'static,
	{
		let db = self.db.clone();
		spawn_blocking(move || {
			let transaction = db.rw_transaction().map_err(db_err)?;
			let output = f(&transaction)?;
			transaction.commit().map_err(db_err)?;
			Ok(output)
		})
		.await?
	}

	// API keys

	/// Creates an API key for a user. The returned key is the only copy: only its hash is
	/// stored.
	pub async fn create_api_key(&self, owner: &str, label: &str) -> Result<CreatedApiKey, Error> {
		let key = random_hex(32)?;
		let model = ApiKeyModel {
			key_hash: hash_api_key(&key),
			owner: owner.to_owned(),
			label: label.to_owned(),
			created_at: now(),
			last_used_at: None,
		};
		let info = ApiKeyInfo {
			id: api_key_id(&model.key_hash),
			label: model.label.clone(),
			created_at: model.created_at,
			last_used_at: None,
		};
		self.write(move |t| t.insert(model).map_err(db_err)).await?;
		Ok(CreatedApiKey { key, info })
	}

	/// Returns the owner of an API key, or `None` if the key is not valid.
	pub async fn authenticate_api_key(&self, key: &str) -> Result<Option<String>, Error> {
		let hash = hash_api_key(key);
		let found = self
			.read({
				let hash = hash.clone();
				move |t| {
					t.get()
						.primary::<ApiKeyModel>(hash.as_str())
						.map_err(db_err)
				}
			})
			.await?;
		let Some(model) = found else {
			return Ok(None);
		};

		let now = now();
		let owner = model.owner.clone();
		if model
			.last_used_at
			.is_none_or(|t| now - t >= API_KEY_LAST_USED_GRANULARITY_SECS)
		{
			self.write(move |t| {
				// The key may have been revoked since it was read.
				if let Some(current) = t
					.get()
					.primary::<ApiKeyModel>(hash.as_str())
					.map_err(db_err)?
				{
					let mut updated = current.clone();
					updated.last_used_at = Some(now);
					t.update(current, updated).map_err(db_err)?;
				}
				Ok(())
			})
			.await?;
		}
		Ok(Some(owner))
	}

	pub async fn list_api_keys(&self, owner: &str) -> Result<Vec<ApiKeyInfo>, Error> {
		let owner = owner.to_owned();
		self.read(move |t| {
			let mut keys = t
				.scan()
				.secondary::<ApiKeyModel>(ApiKeyModelKey::owner)
				.map_err(db_err)?
				.range(owner.as_str()..=owner.as_str())
				.map_err(db_err)?
				.filter_map(|k| k.ok())
				.map(|k| ApiKeyInfo {
					id: api_key_id(&k.key_hash),
					label: k.label,
					created_at: k.created_at,
					last_used_at: k.last_used_at,
				})
				.collect::<Vec<_>>();
			keys.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
			Ok(keys)
		})
		.await
	}

	/// Revokes one of a user's keys, identified by its exact id. Returns `false` if the user
	/// has no such key.
	pub async fn revoke_api_key(&self, owner: &str, id: &str) -> Result<bool, Error> {
		let owner = owner.to_owned();
		let id = id.to_owned();
		self.write(move |t| {
			// Only an exact id revokes a key, never a prefix of one.
			if id.chars().count() != API_KEY_ID_LENGTH {
				return Ok(false);
			}
			let found = t
				.scan()
				.secondary::<ApiKeyModel>(ApiKeyModelKey::owner)
				.map_err(db_err)?
				.range(owner.as_str()..=owner.as_str())
				.map_err(db_err)?
				.filter_map(|k| k.ok())
				.find(|k| k.key_hash.starts_with(&id));
			match found {
				Some(key) => {
					t.remove(key).map_err(db_err)?;
					Ok(true)
				}
				None => Ok(false),
			}
		})
		.await
	}

	// Object IDs

	pub async fn get_or_create_id(
		&self,
		kind: ObjectKind,
		natural_key: &str,
	) -> Result<String, Error> {
		let mut ids = self
			.get_or_create_ids(kind, &[natural_key.to_owned()])
			.await?;
		Ok(ids.remove(0))
	}

	/// Returns the ID of each natural key, in the same order, creating IDs that don't exist
	/// yet. Only writes when something is missing.
	pub async fn get_or_create_ids(
		&self,
		kind: ObjectKind,
		natural_keys: &[String],
	) -> Result<Vec<String>, Error> {
		let lookups: Vec<String> = natural_keys.iter().map(|k| lookup_key(kind, k)).collect();

		let existing = self
			.read({
				let lookups = lookups.clone();
				move |t| {
					Self::find_ids(
						|l| {
							t.get()
								.secondary::<ObjectIdModel>(ObjectIdModelKey::lookup, l)
						},
						&lookups,
					)
				}
			})
			.await?;
		if existing.iter().all(|id| id.is_some()) {
			return Ok(existing.into_iter().flatten().collect());
		}

		self.write(move |t| {
			let mut ids = Self::find_ids(
				|l| {
					t.get()
						.secondary::<ObjectIdModel>(ObjectIdModelKey::lookup, l)
				},
				&lookups,
			)?;
			// The same new key can appear more than once in a batch: create its ID only once.
			let mut created = std::collections::HashMap::<&String, String>::new();
			for (id, lookup) in ids.iter_mut().zip(&lookups) {
				if id.is_some() {
					continue;
				}
				if let Some(new_id) = created.get(lookup) {
					*id = Some(new_id.clone());
					continue;
				}
				let new_id = format!("{}_{}", kind.prefix(), random_id()?);
				t.insert(ObjectIdModel {
					id: new_id.clone(),
					lookup: lookup.clone(),
				})
				.map_err(db_err)?;
				created.insert(lookup, new_id.clone());
				*id = Some(new_id);
			}
			Ok(ids.into_iter().flatten().collect())
		})
		.await
	}

	fn find_ids<F>(find: F, lookups: &[String]) -> Result<Vec<Option<String>>, Error>
	where
		F: Fn(&str) -> Result<Option<ObjectIdModel>, native_db::db_type::Error>,
	{
		lookups
			.iter()
			.map(|lookup| find(lookup).map(|m| m.map(|m| m.id)).map_err(db_err))
			.collect()
	}

	/// Returns the kind and natural key an ID refers to, or `None` for an unknown ID.
	pub async fn resolve_id(&self, id: &str) -> Result<Option<(ObjectKind, String)>, Error> {
		let id = id.to_owned();
		self.read(move |t| {
			let Some(model) = t
				.get()
				.primary::<ObjectIdModel>(id.as_str())
				.map_err(db_err)?
			else {
				return Ok(None);
			};
			let parsed = model.lookup.split_once('\0').and_then(|(prefix, natural)| {
				ObjectKind::from_prefix(prefix).map(|kind| (kind, natural.to_owned()))
			});
			Ok(parsed)
		})
		.await
	}

	/// Points an existing ID at a new natural key (eg. a file that was moved), so that
	/// clients and stored state keep working. Returns `false` (and changes nothing) if the
	/// old key has no ID or the new key already has one.
	pub async fn rekey_id(
		&self,
		kind: ObjectKind,
		old_natural_key: &str,
		new_natural_key: &str,
	) -> Result<bool, Error> {
		let old_lookup = lookup_key(kind, old_natural_key);
		let new_lookup = lookup_key(kind, new_natural_key);
		self.write(move |t| {
			let Some(old) = t
				.get()
				.secondary::<ObjectIdModel>(ObjectIdModelKey::lookup, old_lookup.as_str())
				.map_err(db_err)?
			else {
				return Ok(false);
			};
			let taken = t
				.get()
				.secondary::<ObjectIdModel>(ObjectIdModelKey::lookup, new_lookup.as_str())
				.map_err(db_err)?
				.is_some();
			if taken {
				return Ok(false);
			}
			let mut updated = old.clone();
			updated.lookup = new_lookup;
			t.update(old, updated).map_err(db_err)?;
			Ok(true)
		})
		.await
	}

	// Stars, ratings and play statistics

	pub async fn get_item_state(
		&self,
		owner: &str,
		item_id: &str,
	) -> Result<Option<ItemState>, Error> {
		let owner = owner.to_owned();
		let item_id = item_id.to_owned();
		self.read(move |t| {
			Ok(t.get()
				.primary::<ItemStateModel>((owner.as_str(), item_id.as_str()))
				.map_err(db_err)?
				.map(ItemState::from))
		})
		.await
	}

	/// All objects a user has any state for.
	pub async fn list_item_states(&self, owner: &str) -> Result<Vec<ItemState>, Error> {
		let owner = owner.to_owned();
		self.read(move |t| {
			let mut states = t
				.scan()
				.secondary::<ItemStateModel>(ItemStateModelKey::owner)
				.map_err(db_err)?
				.range(owner.as_str()..=owner.as_str())
				.map_err(db_err)?
				.filter_map(|s| s.ok())
				.map(ItemState::from)
				.collect::<Vec<_>>();
			states.sort_by(|a, b| a.item_id.cmp(&b.item_id));
			Ok(states)
		})
		.await
	}

	pub async fn star(&self, owner: &str, item_id: &str) -> Result<(), Error> {
		let at = now();
		self.update_item_state(owner, item_id, move |s| s.starred_at = Some(at))
			.await
	}

	pub async fn unstar(&self, owner: &str, item_id: &str) -> Result<(), Error> {
		self.update_item_state(owner, item_id, |s| s.starred_at = None)
			.await
	}

	/// Sets a rating from 1 to 5. A rating of 0 (or `None`) clears it.
	pub async fn set_rating(
		&self,
		owner: &str,
		item_id: &str,
		rating: Option<u8>,
	) -> Result<(), Error> {
		if rating.is_some_and(|r| r > MAX_RATING) {
			return Err(Error::InvalidRating);
		}
		let rating = rating.filter(|r| *r > 0);
		self.update_item_state(owner, item_id, move |s| s.rating = rating)
			.await
	}

	/// Records that a user played an object, at a time given in seconds since the epoch.
	pub async fn record_play(&self, owner: &str, item_id: &str, at: i64) -> Result<(), Error> {
		self.update_item_state(owner, item_id, move |s| {
			s.play_count = s.play_count.saturating_add(1);
			s.last_played = Some(s.last_played.map_or(at, |previous| previous.max(at)));
		})
		.await
	}

	async fn update_item_state<F>(&self, owner: &str, item_id: &str, change: F) -> Result<(), Error>
	where
		F: FnOnce(&mut ItemState) + Send + 'static,
	{
		let owner = owner.to_owned();
		let item_id = item_id.to_owned();
		self.write(move |t| {
			let existing = t
				.get()
				.primary::<ItemStateModel>((owner.as_str(), item_id.as_str()))
				.map_err(db_err)?;
			let mut state = existing
				.clone()
				.map(ItemState::from)
				.unwrap_or_else(|| ItemState {
					item_id: item_id.clone(),
					starred_at: None,
					rating: None,
					play_count: 0,
					last_played: None,
				});
			change(&mut state);

			// Don't keep rows that carry no information.
			match (existing, state.is_empty()) {
				(Some(old), true) => {
					t.remove(old).map_err(db_err)?;
				}
				(None, true) => (),
				(old, false) => {
					let new = ItemStateModel {
						owner,
						item_id,
						starred_at: state.starred_at,
						rating: state.rating,
						play_count: state.play_count,
						last_played: state.last_played,
					};
					match old {
						Some(old) => t.update(old, new).map_err(db_err)?,
						None => t.insert(new).map_err(db_err)?,
					};
				}
			}
			Ok(())
		})
		.await
	}

	// Bookmarks

	/// Creates or updates a bookmark. The creation time of an existing bookmark is kept.
	pub async fn save_bookmark(
		&self,
		owner: &str,
		song_id: &str,
		position_ms: u64,
		comment: Option<String>,
	) -> Result<(), Error> {
		let owner = owner.to_owned();
		let song_id = song_id.to_owned();
		self.write(move |t| {
			let existing = t
				.get()
				.primary::<BookmarkModel>((owner.as_str(), song_id.as_str()))
				.map_err(db_err)?;
			let now = now();
			let new = BookmarkModel {
				owner,
				song_id,
				position_ms,
				comment,
				created_at: existing.as_ref().map_or(now, |b| b.created_at),
				changed_at: now,
			};
			match existing {
				Some(old) => t.update(old, new).map_err(db_err),
				None => t.insert(new).map_err(db_err),
			}
		})
		.await
	}

	/// Returns `false` if there was no such bookmark.
	pub async fn delete_bookmark(&self, owner: &str, song_id: &str) -> Result<bool, Error> {
		let owner = owner.to_owned();
		let song_id = song_id.to_owned();
		self.write(move |t| {
			match t
				.get()
				.primary::<BookmarkModel>((owner.as_str(), song_id.as_str()))
				.map_err(db_err)?
			{
				Some(bookmark) => {
					t.remove(bookmark).map_err(db_err)?;
					Ok(true)
				}
				None => Ok(false),
			}
		})
		.await
	}

	pub async fn list_bookmarks(&self, owner: &str) -> Result<Vec<Bookmark>, Error> {
		let owner = owner.to_owned();
		self.read(move |t| {
			let mut bookmarks = t
				.scan()
				.secondary::<BookmarkModel>(BookmarkModelKey::owner)
				.map_err(db_err)?
				.range(owner.as_str()..=owner.as_str())
				.map_err(db_err)?
				.filter_map(|b| b.ok())
				.collect::<Vec<_>>();
			bookmarks.sort_by(|a, b| a.song_id.cmp(&b.song_id));
			Ok(bookmarks)
		})
		.await
	}

	// Play queue

	/// Replaces the saved play queue of a user.
	pub async fn save_play_queue(
		&self,
		owner: &str,
		entries: Vec<String>,
		current_index: Option<u32>,
		position_ms: u64,
		changed_by: &str,
	) -> Result<(), Error> {
		let model = PlayQueueModel {
			owner: owner.to_owned(),
			entries,
			current_index,
			position_ms,
			changed_at: now(),
			changed_by: changed_by.to_owned(),
		};
		self.write(move |t| {
			t.upsert(model).map_err(db_err)?;
			Ok(())
		})
		.await
	}

	pub async fn get_play_queue(&self, owner: &str) -> Result<Option<PlayQueue>, Error> {
		let owner = owner.to_owned();
		self.read(move |t| {
			t.get()
				.primary::<PlayQueueModel>(owner.as_str())
				.map_err(db_err)
		})
		.await
	}

	pub async fn delete_play_queue(&self, owner: &str) -> Result<bool, Error> {
		let owner = owner.to_owned();
		self.write(move |t| {
			match t
				.get()
				.primary::<PlayQueueModel>(owner.as_str())
				.map_err(db_err)?
			{
				Some(queue) => {
					t.remove(queue).map_err(db_err)?;
					Ok(true)
				}
				None => Ok(false),
			}
		})
		.await
	}
}

#[cfg(test)]
mod test {
	use super::*;
	use crate::test::*;
	use crate::test_name;

	fn manager(test_name: String) -> (Manager, std::path::PathBuf) {
		let directory = prepare_test_directory(test_name);
		let db = ndb::Manager::new(&directory).unwrap();
		(Manager::new(db), directory)
	}

	#[tokio::test]
	async fn api_keys_authenticate_list_and_revoke() {
		let (state, _) = manager(test_name!());

		let key = state.create_api_key("alice", "phone").await.unwrap().key;
		let other = state.create_api_key("bob", "laptop").await.unwrap().key;
		assert_ne!(key, other);
		assert_eq!(key.len(), 64);

		assert_eq!(
			state.authenticate_api_key(&key).await.unwrap(),
			Some("alice".to_owned())
		);
		assert_eq!(
			state.authenticate_api_key(&other).await.unwrap(),
			Some("bob".to_owned())
		);
		assert_eq!(state.authenticate_api_key("not a key").await.unwrap(), None);
		assert_eq!(state.authenticate_api_key("").await.unwrap(), None);

		let keys = state.list_api_keys("alice").await.unwrap();
		assert_eq!(keys.len(), 1);
		assert_eq!(keys[0].label, "phone");
		assert!(keys[0].last_used_at.is_some());

		// Users cannot revoke each other's keys, and unknown or empty ids are rejected.
		assert!(!state.revoke_api_key("bob", &keys[0].id).await.unwrap());
		assert!(!state.revoke_api_key("alice", "").await.unwrap());
		assert!(!state.revoke_api_key("alice", "zzzz").await.unwrap());
		assert!(state.revoke_api_key("alice", &keys[0].id).await.unwrap());
		assert_eq!(state.authenticate_api_key(&key).await.unwrap(), None);
		assert!(state.list_api_keys("alice").await.unwrap().is_empty());
		assert_eq!(
			state.authenticate_api_key(&other).await.unwrap(),
			Some("bob".to_owned())
		);
	}

	#[tokio::test]
	async fn revoking_needs_the_exact_id_not_a_prefix() {
		let (state, _) = manager(test_name!());
		let created = state.create_api_key("alice", "phone").await.unwrap();
		let id = created.info.id.clone();
		assert_eq!(id.len(), API_KEY_ID_LENGTH);

		for prefix_length in [1, 4, API_KEY_ID_LENGTH - 1] {
			assert!(!state
				.revoke_api_key("alice", &id[..prefix_length])
				.await
				.unwrap());
		}
		assert!(!state
			.revoke_api_key("alice", &format!("{id}0"))
			.await
			.unwrap());
		assert_eq!(
			state.authenticate_api_key(&created.key).await.unwrap(),
			Some("alice".to_owned())
		);

		assert!(state.revoke_api_key("alice", &id).await.unwrap());
		assert_eq!(
			state.authenticate_api_key(&created.key).await.unwrap(),
			None
		);
	}

	#[tokio::test]
	async fn created_key_info_matches_the_listing() {
		let (state, _) = manager(test_name!());
		let created = state.create_api_key("alice", "phone").await.unwrap();
		let listed = state.list_api_keys("alice").await.unwrap();
		assert_eq!(listed, vec![created.info.clone()]);
		assert_eq!(created.info.label, "phone");
		assert_eq!(created.info.last_used_at, None);
	}

	#[tokio::test]
	async fn api_keys_are_stored_hashed() {
		let (state, _) = manager(test_name!());
		let key = state.create_api_key("alice", "phone").await.unwrap().key;

		let stored = state.list_api_keys("alice").await.unwrap();
		assert!(!key.starts_with(&stored[0].id));

		// The stored hash is not itself a valid key.
		let hash = hash_api_key(&key);
		assert_ne!(hash, key);
		assert_eq!(state.authenticate_api_key(&hash).await.unwrap(), None);
	}

	#[tokio::test]
	async fn ids_are_stable_unique_and_resolvable() {
		let (state, _) = manager(test_name!());

		let song = state
			.get_or_create_id(ObjectKind::Song, "music/a/b.mp3")
			.await
			.unwrap();
		assert!(song.starts_with("so_"));
		assert_eq!(
			state
				.get_or_create_id(ObjectKind::Song, "music/a/b.mp3")
				.await
				.unwrap(),
			song
		);

		// Same natural key, different kind: different object.
		let album = state
			.get_or_create_id(ObjectKind::Album, "music/a/b.mp3")
			.await
			.unwrap();
		assert_ne!(album, song);
		assert!(album.starts_with("al_"));

		assert_eq!(
			state.resolve_id(&song).await.unwrap(),
			Some((ObjectKind::Song, "music/a/b.mp3".to_owned()))
		);
		assert_eq!(
			state.resolve_id(&album).await.unwrap(),
			Some((ObjectKind::Album, "music/a/b.mp3".to_owned()))
		);
		assert_eq!(state.resolve_id("so_unknown").await.unwrap(), None);
		assert_eq!(state.resolve_id("").await.unwrap(), None);
	}

	#[tokio::test]
	async fn ids_support_batches_and_odd_keys() {
		let (state, _) = manager(test_name!());

		let keys = vec![
			"one".to_owned(),
			"two".to_owned(),
			"one".to_owned(),
			"ünï cödé/\"quoted\"".to_owned(),
			"".to_owned(),
		];
		let ids = state
			.get_or_create_ids(ObjectKind::Artist, &keys)
			.await
			.unwrap();
		assert_eq!(ids.len(), keys.len());
		assert_eq!(ids[0], ids[2]);
		assert_ne!(ids[0], ids[1]);

		let again = state
			.get_or_create_ids(ObjectKind::Artist, &keys)
			.await
			.unwrap();
		assert_eq!(ids, again);
		for (id, key) in ids.iter().zip(&keys) {
			assert_eq!(
				state.resolve_id(id).await.unwrap(),
				Some((ObjectKind::Artist, key.clone()))
			);
		}
	}

	#[tokio::test]
	async fn rekey_keeps_the_id_when_a_file_moves() {
		let (state, _) = manager(test_name!());
		let id = state
			.get_or_create_id(ObjectKind::Song, "old/path.mp3")
			.await
			.unwrap();
		let occupied = state
			.get_or_create_id(ObjectKind::Song, "taken.mp3")
			.await
			.unwrap();

		assert!(state
			.rekey_id(ObjectKind::Song, "old/path.mp3", "new/path.mp3")
			.await
			.unwrap());
		assert_eq!(
			state
				.get_or_create_id(ObjectKind::Song, "new/path.mp3")
				.await
				.unwrap(),
			id
		);
		assert_eq!(
			state.resolve_id(&id).await.unwrap(),
			Some((ObjectKind::Song, "new/path.mp3".to_owned()))
		);
		// The old key is free again and gets a different ID.
		assert_ne!(
			state
				.get_or_create_id(ObjectKind::Song, "old/path.mp3")
				.await
				.unwrap(),
			id
		);

		// Unknown old key, or new key already in use: nothing changes.
		assert!(!state
			.rekey_id(ObjectKind::Song, "never/seen.mp3", "x.mp3")
			.await
			.unwrap());
		assert!(!state
			.rekey_id(ObjectKind::Song, "new/path.mp3", "taken.mp3")
			.await
			.unwrap());
		assert_eq!(
			state.resolve_id(&occupied).await.unwrap(),
			Some((ObjectKind::Song, "taken.mp3".to_owned()))
		);
		assert_eq!(
			state.resolve_id(&id).await.unwrap(),
			Some((ObjectKind::Song, "new/path.mp3".to_owned()))
		);
	}

	#[tokio::test]
	async fn stars_ratings_and_plays() {
		let (state, _) = manager(test_name!());

		assert_eq!(state.get_item_state("alice", "so-1").await.unwrap(), None);

		state.star("alice", "so-1").await.unwrap();
		let s = state
			.get_item_state("alice", "so-1")
			.await
			.unwrap()
			.unwrap();
		assert!(s.starred_at.is_some());
		assert_eq!(s.play_count, 0);

		state.set_rating("alice", "so-1", Some(4)).await.unwrap();
		state.record_play("alice", "so-1", 100).await.unwrap();
		state.record_play("alice", "so-1", 50).await.unwrap();
		let s = state
			.get_item_state("alice", "so-1")
			.await
			.unwrap()
			.unwrap();
		assert_eq!(s.rating, Some(4));
		assert_eq!(s.play_count, 2);
		assert_eq!(s.last_played, Some(100));

		// State is per user.
		assert_eq!(state.get_item_state("bob", "so-1").await.unwrap(), None);
		state.record_play("bob", "so-2", 7).await.unwrap();
		assert_eq!(state.list_item_states("alice").await.unwrap().len(), 1);
		assert_eq!(state.list_item_states("bob").await.unwrap().len(), 1);

		assert!(matches!(
			state.set_rating("alice", "so-1", Some(6)).await,
			Err(Error::InvalidRating)
		));
		state.set_rating("alice", "so-1", Some(0)).await.unwrap();
		assert_eq!(
			state
				.get_item_state("alice", "so-1")
				.await
				.unwrap()
				.unwrap()
				.rating,
			None
		);
	}

	#[tokio::test]
	async fn empty_item_state_rows_are_removed() {
		let (state, _) = manager(test_name!());

		state.star("alice", "so-1").await.unwrap();
		state.unstar("alice", "so-1").await.unwrap();
		assert_eq!(state.get_item_state("alice", "so-1").await.unwrap(), None);
		assert!(state.list_item_states("alice").await.unwrap().is_empty());

		// Unstarring something that was never starred is a no-op.
		state.unstar("alice", "so-never").await.unwrap();
		assert!(state.list_item_states("alice").await.unwrap().is_empty());

		// A played item keeps its row even when it is unstarred.
		state.star("alice", "so-2").await.unwrap();
		state.record_play("alice", "so-2", 1).await.unwrap();
		state.unstar("alice", "so-2").await.unwrap();
		assert_eq!(state.list_item_states("alice").await.unwrap().len(), 1);
	}

	#[tokio::test]
	async fn bookmarks_keep_their_creation_time() {
		let (state, _) = manager(test_name!());

		state
			.save_bookmark("alice", "so-1", 1000, Some("note".to_owned()))
			.await
			.unwrap();
		let first = state.list_bookmarks("alice").await.unwrap();
		assert_eq!(first.len(), 1);
		assert_eq!(first[0].position_ms, 1000);

		state
			.save_bookmark("alice", "so-1", 2500, None)
			.await
			.unwrap();
		let second = state.list_bookmarks("alice").await.unwrap();
		assert_eq!(second.len(), 1);
		assert_eq!(second[0].position_ms, 2500);
		assert_eq!(second[0].comment, None);
		assert_eq!(second[0].created_at, first[0].created_at);

		assert!(state.list_bookmarks("bob").await.unwrap().is_empty());
		assert!(!state.delete_bookmark("alice", "so-other").await.unwrap());
		assert!(state.delete_bookmark("alice", "so-1").await.unwrap());
		assert!(state.list_bookmarks("alice").await.unwrap().is_empty());
	}

	#[tokio::test]
	async fn play_queue_is_replaced_per_user() {
		let (state, _) = manager(test_name!());

		assert!(state.get_play_queue("alice").await.unwrap().is_none());
		state
			.save_play_queue(
				"alice",
				vec!["so-1".into(), "so-2".into(), "so-1".into()],
				Some(2),
				1234,
				"phone",
			)
			.await
			.unwrap();
		let queue = state.get_play_queue("alice").await.unwrap().unwrap();
		assert_eq!(queue.entries, vec!["so-1", "so-2", "so-1"]);
		assert_eq!(queue.current_index, Some(2));
		assert_eq!(queue.position_ms, 1234);
		assert_eq!(queue.changed_by, "phone");

		state
			.save_play_queue("alice", vec![], None, 0, "laptop")
			.await
			.unwrap();
		let queue = state.get_play_queue("alice").await.unwrap().unwrap();
		assert!(queue.entries.is_empty());
		assert_eq!(queue.changed_by, "laptop");
		assert!(state.get_play_queue("bob").await.unwrap().is_none());

		assert!(state.delete_play_queue("alice").await.unwrap());
		assert!(!state.delete_play_queue("alice").await.unwrap());
	}

	#[tokio::test]
	async fn everything_survives_a_restart() {
		let directory = prepare_test_directory(test_name!());

		let (key, id) = {
			let state = Manager::new(ndb::Manager::new(&directory).unwrap());
			let key = state.create_api_key("alice", "phone").await.unwrap().key;
			let id = state
				.get_or_create_id(ObjectKind::Song, "music/song.mp3")
				.await
				.unwrap();
			state.star("alice", &id).await.unwrap();
			state.record_play("alice", &id, 42).await.unwrap();
			state.save_bookmark("alice", &id, 99, None).await.unwrap();
			state
				.save_play_queue("alice", vec![id.clone()], Some(0), 5, "phone")
				.await
				.unwrap();
			(key, id)
		};

		// Reopen the database from disk.
		let state = Manager::new(ndb::Manager::new(&directory).unwrap());
		assert_eq!(
			state.authenticate_api_key(&key).await.unwrap(),
			Some("alice".to_owned())
		);
		assert_eq!(
			state
				.get_or_create_id(ObjectKind::Song, "music/song.mp3")
				.await
				.unwrap(),
			id
		);
		let item = state.get_item_state("alice", &id).await.unwrap().unwrap();
		assert!(item.starred_at.is_some());
		assert_eq!(item.play_count, 1);
		assert_eq!(state.list_bookmarks("alice").await.unwrap().len(), 1);
		assert_eq!(
			state
				.get_play_queue("alice")
				.await
				.unwrap()
				.unwrap()
				.entries,
			vec![id]
		);
	}

	#[test]
	fn id_alphabet_is_the_64_symbol_url_safe_nanoid_alphabet() {
		assert_eq!(ID_ALPHABET.len(), 64);
		let unique: std::collections::HashSet<_> = ID_ALPHABET.iter().collect();
		assert_eq!(unique.len(), 64);
		assert!(ID_ALPHABET
			.iter()
			.all(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'-'));
	}

	#[test]
	fn random_ids_have_the_expected_shape_and_never_repeat() {
		let mut seen = std::collections::HashSet::new();
		for _ in 0..20_000 {
			let id = random_id().unwrap();
			assert_eq!(id.len(), ID_LENGTH);
			assert!(id.bytes().all(|c| ID_ALPHABET.contains(&c)));
			assert!(seen.insert(id), "random_id produced a duplicate");
		}
	}

	#[test]
	fn random_ids_use_every_symbol_uniformly() {
		// Chi-squared goodness of fit against a uniform distribution over the 64 symbols.
		// With 63 degrees of freedom the 0.001 critical value is about 103, so the generous
		// limit below makes a false failure vanishingly unlikely while still catching a
		// biased or constant generator (which scores in the tens of thousands).
		let mut counts = [0u64; 64];
		let samples = 20_000;
		for _ in 0..samples {
			for c in random_id().unwrap().bytes() {
				let index = ID_ALPHABET.iter().position(|a| *a == c).unwrap();
				counts[index] += 1;
			}
		}
		let total = (samples * ID_LENGTH) as f64;
		let expected = total / 64.0;
		let chi_squared: f64 = counts
			.iter()
			.map(|c| {
				let d = *c as f64 - expected;
				d * d / expected
			})
			.sum();
		assert!(counts.iter().all(|c| *c > 0), "a symbol was never produced");
		assert!(chi_squared < 140.0, "chi-squared too high: {chi_squared}");
	}

	#[test]
	fn random_ids_are_uniform_at_every_position() {
		// A flaw such as a weak first character would hide in the aggregate test above.
		let samples = 30_000;
		let mut counts = vec![[0u32; 64]; ID_LENGTH];
		for _ in 0..samples {
			for (position, c) in random_id().unwrap().bytes().enumerate() {
				let index = ID_ALPHABET.iter().position(|a| *a == c).unwrap();
				counts[position][index] += 1;
			}
		}
		let expected = samples as f64 / 64.0;
		for (position, row) in counts.iter().enumerate() {
			let chi_squared: f64 = row
				.iter()
				.map(|c| {
					let d = *c as f64 - expected;
					d * d / expected
				})
				.sum();
			assert!(
				chi_squared < 140.0,
				"position {position} is not uniform: {chi_squared}"
			);
		}
	}

	#[tokio::test]
	async fn object_ids_use_prefix_underscore_and_nanoid_part() {
		let (state, _) = manager(test_name!());
		for (kind, prefix) in [
			(ObjectKind::Song, "so"),
			(ObjectKind::Album, "al"),
			(ObjectKind::Artist, "ar"),
			(ObjectKind::Playlist, "pl"),
			(ObjectKind::Directory, "di"),
		] {
			let id = state.get_or_create_id(kind, "key").await.unwrap();
			assert_eq!(id.len(), 3 + ID_LENGTH);
			assert_eq!(&id[..3], format!("{prefix}_"));
			assert!(id[3..].bytes().all(|c| ID_ALPHABET.contains(&c)));
			// IDs are case sensitive: changing the case must not resolve to the same object.
			let flipped: String = id
				.chars()
				.map(|c| {
					if c.is_ascii_lowercase() {
						c.to_ascii_uppercase()
					} else {
						c.to_ascii_lowercase()
					}
				})
				.collect();
			if flipped != id {
				assert_eq!(state.resolve_id(&flipped).await.unwrap(), None);
			}
			assert!(state.resolve_id(&id).await.unwrap().is_some());
		}
	}
}
