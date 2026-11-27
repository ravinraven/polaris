//! Turns Polaris songs, albums and artists into the objects of the Subsonic API.
//!
//! Objects are identified by generated IDs (see `app::state`). A [`Wanted`] collects the
//! objects a response will mention so that all their IDs, and the user's stars and ratings,
//! are looked up once, before the response is built.

use std::{
	collections::{HashMap, HashSet},
	path::Path,
};

use chrono::{DateTime, SecondsFormat};

use crate::app::{
	index::{Album, AlbumSummary, Song},
	state::{ItemState, ObjectKind},
};

use super::ctx::{internal, Ctx};
use super::response::{Element, SubsonicError};
use super::tags::{self, Extras};

// Dates and file types

/// The current time, in seconds since the epoch.
pub fn now_secs() -> i64 {
	std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.map(|d| d.as_secs() as i64)
		.unwrap_or_default()
}

pub fn iso8601(seconds_since_epoch: i64) -> String {
	DateTime::from_timestamp(seconds_since_epoch, 0)
		.unwrap_or_default()
		.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// The MIME type and (lowercase) suffix of an audio file, from its name.
pub fn audio_type(path: &Path) -> (&'static str, String) {
	let suffix = path
		.extension()
		.and_then(|e| e.to_str())
		.unwrap_or_default()
		.to_ascii_lowercase();
	let mime = match suffix.as_str() {
		"mp3" => "audio/mpeg",
		"flac" => "audio/flac",
		"ogg" => "audio/ogg",
		"opus" => "audio/opus",
		"m4a" | "m4b" | "mp4" => "audio/mp4",
		"wav" => "audio/x-wav",
		"aif" | "aiff" => "audio/aiff",
		"ape" => "audio/x-ape",
		"mpc" => "audio/x-musepack",
		"webm" => "audio/webm",
		"mka" => "audio/x-matroska",
		_ => "application/octet-stream",
	};
	(mime, suffix)
}

// Natural keys: what an object ID stands for

pub fn song_key(song: &Song) -> String {
	path_key(&song.virtual_path)
}

pub fn path_key(path: &Path) -> String {
	path.to_string_lossy().into_owned()
}

pub fn album_key(artists: &[String], name: &str) -> String {
	serde_json::to_string(&(name, artists)).expect("strings always serialize")
}

pub fn parse_album_key(key: &str) -> Option<(String, Vec<String>)> {
	serde_json::from_str::<(String, Vec<String>)>(key).ok()
}

/// The key of the album a song belongs to (the same rule the index uses).
pub fn song_album_key(song: &Song) -> Option<String> {
	let name = song.album.as_ref()?;
	let artists = if song.album_artists.is_empty() {
		&song.artists
	} else {
		&song.album_artists
	};
	if artists.is_empty() {
		None
	} else {
		Some(album_key(artists, name))
	}
}

pub fn playlist_key(owner: &str, name: &str) -> String {
	format!("{owner}\0{name}")
}

pub fn parse_playlist_key(key: &str) -> Option<(&str, &str)> {
	key.split_once('\0')
}

pub fn join_names(names: &[String]) -> String {
	names.join(", ")
}

pub fn file_title(path: &Path) -> String {
	path.file_stem()
		.map(|s| s.to_string_lossy().into_owned())
		.unwrap_or_default()
}

// What an album is, whether it came from a summary or from a full album

pub struct AlbumData {
	pub name: String,
	pub artists: Vec<String>,
	pub year: Option<i64>,
	pub date_added: i64,
	pub has_artwork: bool,
	pub num_songs: u32,
	pub duration: i64,
	pub genres: Vec<String>,
}

impl From<&AlbumSummary> for AlbumData {
	fn from(a: &AlbumSummary) -> Self {
		Self {
			name: a.header.name.clone(),
			artists: a.header.artists.clone(),
			year: a.header.year,
			date_added: a.header.date_added,
			has_artwork: a.header.artwork.is_some(),
			num_songs: a.num_songs,
			duration: a.duration,
			genres: a.genres.clone(),
		}
	}
}

impl From<&Album> for AlbumData {
	fn from(a: &Album) -> Self {
		let mut genres: Vec<String> = a.songs.iter().flat_map(|s| s.genres.clone()).collect();
		genres.sort();
		genres.dedup();
		Self {
			name: a.header.name.clone(),
			artists: a.header.artists.clone(),
			year: a.header.year,
			date_added: a.header.date_added,
			has_artwork: a.header.artwork.is_some(),
			num_songs: a.songs.len() as u32,
			duration: a.songs.iter().filter_map(|s| s.duration).sum(),
			genres,
		}
	}
}

impl AlbumData {
	pub fn key(&self) -> String {
		album_key(&self.artists, &self.name)
	}
}

// Collecting what a response needs

#[derive(Default)]
pub struct Wanted {
	objects: HashSet<(ObjectKind, String)>,
}

impl Wanted {
	pub fn new() -> Self {
		Self::default()
	}

	fn add(&mut self, kind: ObjectKind, key: String) {
		self.objects.insert((kind, key));
	}

	pub fn song(&mut self, song: &Song) {
		self.add(ObjectKind::Song, song_key(song));
		if let Some(parent) = song.virtual_path.parent() {
			self.add(ObjectKind::Directory, path_key(parent));
		}
		if let Some(key) = song_album_key(song) {
			self.add(ObjectKind::Album, key);
		}
		for name in song
			.artists
			.iter()
			.chain(&song.album_artists)
			.chain(&song.composers)
			.chain(&song.lyricists)
		{
			self.add(ObjectKind::Artist, name.clone());
		}
	}

	pub fn songs(&mut self, songs: &[Song]) {
		for song in songs {
			self.song(song);
		}
	}

	pub fn album(&mut self, album: &AlbumData) {
		self.add(ObjectKind::Album, album.key());
		for artist in &album.artists {
			self.add(ObjectKind::Artist, artist.clone());
		}
	}

	pub fn artist(&mut self, name: &str) {
		self.add(ObjectKind::Artist, name.to_owned());
	}

	pub fn dir(&mut self, path: &Path) {
		self.add(ObjectKind::Directory, path_key(path));
		if let Some(parent) = path.parent() {
			self.add(ObjectKind::Directory, path_key(parent));
		}
	}

	pub fn playlist(&mut self, owner: &str, name: &str) {
		self.add(ObjectKind::Playlist, playlist_key(owner, name));
	}
}

/// The IDs and the user state needed to describe objects.
pub struct Mapper {
	ids: HashMap<(ObjectKind, String), String>,
	states: HashMap<String, ItemState>,
}

impl Mapper {
	pub async fn load(ctx: &Ctx, wanted: Wanted) -> Result<Self, SubsonicError> {
		let mut by_kind: HashMap<ObjectKind, Vec<String>> = HashMap::new();
		for (kind, key) in wanted.objects {
			by_kind.entry(kind).or_default().push(key);
		}

		let mut ids = HashMap::new();
		for (kind, keys) in by_kind {
			let generated = ctx
				.state
				.get_or_create_ids(kind, &keys)
				.await
				.map_err(internal)?;
			for (key, id) in keys.into_iter().zip(generated) {
				ids.insert((kind, key), id);
			}
		}

		let states = ctx
			.state
			.list_item_states(&ctx.user)
			.await
			.map_err(internal)?
			.into_iter()
			.map(|s| (s.item_id.clone(), s))
			.collect();

		Ok(Self { ids, states })
	}

	pub fn id(&self, kind: ObjectKind, key: &str) -> String {
		self.ids
			.get(&(kind, key.to_owned()))
			.cloned()
			.unwrap_or_default()
	}

	pub fn state(&self, id: &str) -> Option<&ItemState> {
		self.states.get(id)
	}

	/// Adds the user's star, rating and play statistics for an object.
	fn with_state(&self, element: Element, id: &str) -> Element {
		let Some(state) = self.state(id) else {
			return element;
		};
		let mut element = element
			.attr_opt("starred", state.starred_at.map(iso8601))
			.attr_opt("userRating", state.rating.map(|r| r as i32));
		if state.play_count > 0 {
			element = element.attr("playCount", state.play_count);
		}
		element.attr_opt("played", state.last_played.map(iso8601))
	}

	/// A minimal artist (`ArtistID3`): enough to refer to one.
	pub fn artist_ref(&self, name: &str) -> Element {
		let id = self.id(ObjectKind::Artist, name);
		let element = Element::new().attr("id", id.clone()).attr("name", name);
		self.with_state(element, &id)
	}

	pub fn artist_refs(&self, names: &[String]) -> Vec<Element> {
		names.iter().map(|n| self.artist_ref(n)).collect()
	}

	/// An `ArtistID3`, with the number of albums.
	pub fn artist(&self, name: &str, num_albums: u32) -> Element {
		let id = self.id(ObjectKind::Artist, name);
		let element = Element::new()
			.attr("id", id.clone())
			.attr("name", name)
			.attr("albumCount", num_albums);
		self.with_state(element, &id)
	}

	/// An `AlbumID3`. Pass the songs to get an `AlbumID3WithSongs`.
	pub fn album(&self, album: &AlbumData, songs: Option<Vec<Element>>) -> Element {
		let key = album.key();
		let id = self.id(ObjectKind::Album, &key);
		let first_artist = album.artists.first();
		let mut element = Element::new()
			.attr("id", id.clone())
			.attr("name", album.name.clone())
			.attr_opt(
				"artist",
				(!album.artists.is_empty()).then(|| join_names(&album.artists)),
			)
			.attr_opt(
				"artistId",
				first_artist.map(|a| self.id(ObjectKind::Artist, a)),
			)
			.attr_opt("coverArt", album.has_artwork.then(|| id.clone()))
			.attr("songCount", album.num_songs)
			.attr("duration", album.duration)
			.attr("created", iso8601(album.date_added))
			.attr_opt("year", album.year)
			.attr_opt("genre", album.genres.first().cloned())
			.attr_opt(
				"displayArtist",
				(!album.artists.is_empty()).then(|| join_names(&album.artists)),
			);
		element = self.with_state(element, &id);
		element = element
			.list(
				"genres",
				album
					.genres
					.iter()
					.map(|g| Element::new().attr("name", g.clone()))
					.collect(),
			)
			.list("artists", self.artist_refs(&album.artists));
		if let Some(songs) = songs {
			element = element.list("song", songs);
		}
		element
	}

	/// A directory as a `Child` (`isDir` is true): a folder, or an album in the legacy API.
	pub fn directory_child(&self, id: &str, parent: Option<String>, title: &str) -> Element {
		let element = Element::new()
			.attr("id", id)
			.attr_opt("parent", parent)
			.attr("isDir", true)
			.attr("title", title);
		self.with_state(element, id)
	}

	/// An album as a `Child` directory (what the non-ID3 browsing endpoints return).
	pub fn album_child(&self, album: &AlbumData) -> Element {
		let id = self.id(ObjectKind::Album, &album.key());
		let first_artist = album.artists.first();
		let element = Element::new()
			.attr("id", id.clone())
			.attr_opt(
				"parent",
				first_artist.map(|a| self.id(ObjectKind::Artist, a)),
			)
			.attr("isDir", true)
			.attr("title", album.name.clone())
			.attr("album", album.name.clone())
			.attr_opt(
				"artist",
				(!album.artists.is_empty()).then(|| join_names(&album.artists)),
			)
			.attr_opt("coverArt", album.has_artwork.then(|| id.clone()))
			.attr("created", iso8601(album.date_added))
			.attr_opt("year", album.year)
			.attr_opt("genre", album.genres.first().cloned());
		self.with_state(element, &id)
	}

	/// A song (`Child`).
	pub fn song(&self, song: &Song, size: Option<u64>, extras: Option<&Extras>) -> Element {
		let id = self.id(ObjectKind::Song, &song_key(song));
		let (content_type, suffix) = audio_type(&song.virtual_path);
		let album_id = song_album_key(song).map(|k| self.id(ObjectKind::Album, &k));
		let parent = song
			.virtual_path
			.parent()
			.map(|p| self.id(ObjectKind::Directory, &path_key(p)));
		let bit_rate = match (size, song.duration) {
			(Some(size), Some(duration)) if duration > 0 => {
				Some((size as i64 * 8 / 1000 / duration) as i32)
			}
			_ => None,
		};
		let has_artists = !song.artists.is_empty();
		let has_album_artists = !song.album_artists.is_empty();

		let element = Element::new()
			.attr("id", id.clone())
			.attr_opt("parent", parent)
			.attr("isDir", false)
			.attr(
				"title",
				song.title
					.clone()
					.unwrap_or_else(|| file_title(&song.virtual_path)),
			)
			.attr_opt("album", song.album.clone())
			.attr_opt("artist", has_artists.then(|| join_names(&song.artists)))
			.attr_opt("track", song.track_number)
			.attr_opt("year", song.year)
			.attr_opt("genre", song.genres.first().cloned())
			.attr_opt("coverArt", song.artwork.is_some().then(|| id.clone()))
			.attr_opt("size", size.map(|s| s as i64))
			.attr("contentType", content_type)
			.attr("suffix", suffix)
			.attr_opt("duration", song.duration)
			.attr_opt("bitRate", bit_rate)
			.attr("path", path_key(&song.virtual_path))
			.attr("isVideo", false)
			.attr_opt("discNumber", song.disc_number)
			.attr("created", iso8601(song.date_added))
			.attr_opt("albumId", album_id)
			.attr_opt(
				"artistId",
				song.artists.first().map(|a| self.id(ObjectKind::Artist, a)),
			)
			.attr("type", "music")
			.attr("mediaType", "song")
			.attr_opt(
				"displayArtist",
				has_artists.then(|| join_names(&song.artists)),
			)
			.attr_opt(
				"displayAlbumArtist",
				has_album_artists.then(|| join_names(&song.album_artists)),
			)
			.attr_opt(
				"displayComposer",
				(!song.composers.is_empty()).then(|| join_names(&song.composers)),
			);
		let mut element = self.with_state(element, &id);
		if let Some(extras) = extras {
			element = element
				.attr_opt("bpm", extras.bpm)
				.attr_opt("musicBrainzId", extras.musicbrainz_track_id.clone());
			let gain = &extras.replay_gain;
			if !gain.is_empty() {
				element = element.object(
					"replayGain",
					Element::new()
						.attr_opt("trackGain", gain.track_gain)
						.attr_opt("albumGain", gain.album_gain)
						.attr_opt("trackPeak", gain.track_peak)
						.attr_opt("albumPeak", gain.album_peak),
				);
			}
		}

		let contributors = song
			.composers
			.iter()
			.map(|name| ("composer", name))
			.chain(song.lyricists.iter().map(|name| ("lyricist", name)))
			.map(|(role, name)| {
				Element::new()
					.attr("role", role)
					.object("artist", self.artist_ref(name))
			})
			.collect();

		element
			.list(
				"genres",
				song.genres
					.iter()
					.map(|g| Element::new().attr("name", g.clone()))
					.collect(),
			)
			.list("artists", self.artist_refs(&song.artists))
			.list("albumArtists", self.artist_refs(&song.album_artists))
			.list("contributors", contributors)
	}

	/// Songs (`Child` objects), with their file sizes and tags.
	pub async fn songs(&self, songs: &[Song]) -> Vec<Element> {
		let mut elements = Vec::with_capacity(songs.len());
		for song in songs {
			let size = tokio::fs::metadata(&song.real_path)
				.await
				.ok()
				.map(|m| m.len());
			let extras = tags::extras(&song.real_path).await;
			elements.push(self.song(song, size, Some(&extras)));
		}
		elements
	}
}

#[cfg(test)]
mod test {
	use super::*;
	use std::path::PathBuf;

	fn song() -> Song {
		Song {
			virtual_path: PathBuf::from("music/Artist/Album/01 - Song.mp3"),
			album: Some("Album".to_owned()),
			artists: vec!["Artist".to_owned()],
			..Default::default()
		}
	}

	#[test]
	fn album_keys_round_trip_whatever_the_names_contain() {
		let artists = vec!["A \"quoted\" one".to_owned(), "B\u{0}\n, & ü".to_owned()];
		let key = album_key(&artists, "Name: with [odd] \\ chars");
		assert_eq!(
			parse_album_key(&key),
			Some(("Name: with [odd] \\ chars".to_owned(), artists))
		);
		assert_eq!(parse_album_key("not json"), None);
	}

	#[test]
	fn albums_of_songs_follow_the_index_rule() {
		let mut s = song();
		assert_eq!(
			song_album_key(&s),
			Some(album_key(&["Artist".to_owned()], "Album"))
		);

		// Album artists take precedence over song artists
		s.album_artists = vec!["Band".to_owned()];
		assert_eq!(
			song_album_key(&s),
			Some(album_key(&["Band".to_owned()], "Album"))
		);

		// No album, or nobody to attribute it to, means no album
		s.album = None;
		assert_eq!(song_album_key(&s), None);
		let mut s = song();
		s.artists.clear();
		assert_eq!(song_album_key(&s), None);
	}

	#[test]
	fn playlist_keys_keep_owner_and_name_apart() {
		let key = playlist_key("alice", "my: list");
		assert_eq!(parse_playlist_key(&key), Some(("alice", "my: list")));
		assert_ne!(playlist_key("a", "b c"), playlist_key("a b", "c"));
	}

	#[test]
	fn file_types() {
		assert_eq!(
			audio_type(Path::new("a/b.MP3")),
			("audio/mpeg", "mp3".to_owned())
		);
		assert_eq!(audio_type(Path::new("a.webm")).0, "audio/webm");
		assert_eq!(audio_type(Path::new("a.mka")).0, "audio/x-matroska");
		assert_eq!(
			audio_type(Path::new("noextension")).0,
			"application/octet-stream"
		);
	}

	#[test]
	fn dates_are_iso_8601_in_utc() {
		assert_eq!(iso8601(0), "1970-01-01T00:00:00.000Z");
		assert_eq!(iso8601(1_767_225_600), "2026-01-01T00:00:00.000Z");
	}

	#[test]
	fn titles_fall_back_to_the_file_name() {
		assert_eq!(file_title(Path::new("a/b/01 - Song.mp3")), "01 - Song");
	}
}
