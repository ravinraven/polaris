//! What users do with music: stars, ratings, plays, what is playing now, and the lists built
//! from that.

use std::{collections::HashMap, path::PathBuf};

use crate::app::{
	index::{AlbumSummary, Song},
	state::{NowPlaying, ObjectKind},
};

use super::browse::{browse_error, selected_folder};
use super::ctx::{internal, Ctx};
use super::model::{self, now_secs, AlbumData, Mapper, Wanted};
use super::params::Params;
use super::response::{Element, SubsonicError};

const DEFAULT_TOP_SONGS: usize = 50;
const MAX_TOP_SONGS: usize = 500;
const MAX_RATING: i64 = 5;

/// Whether an object of this kind can be starred and rated.
fn annotatable(kind: ObjectKind) -> bool {
	matches!(
		kind,
		ObjectKind::Song | ObjectKind::Album | ObjectKind::Artist | ObjectKind::Directory
	)
}

/// Resolves IDs, all of which must exist and be something that can be annotated.
async fn annotatable_ids(ctx: &Ctx, ids: &[&str]) -> Result<(), SubsonicError> {
	for id in ids {
		let (kind, _) = ctx.resolve(id).await?;
		if !annotatable(kind) {
			return Err(SubsonicError::not_found());
		}
	}
	Ok(())
}

async fn set_star(ctx: &Ctx, params: &Params, starred: bool) -> Result<Element, SubsonicError> {
	let ids: Vec<&str> = ["id", "albumId", "artistId"]
		.into_iter()
		.flat_map(|name| params.get_all(name))
		.filter(|id| !id.is_empty())
		.collect();
	if ids.is_empty() {
		return Err(SubsonicError::missing_parameter("id"));
	}
	// Nothing changes unless every ID is valid
	annotatable_ids(ctx, &ids).await?;

	for id in ids {
		let current = ctx
			.state
			.get_item_state(&ctx.user, id)
			.await
			.map_err(internal)?;
		let is_starred = current.is_some_and(|s| s.starred_at.is_some());
		match (starred, is_starred) {
			(true, false) => ctx.state.star(&ctx.user, id).await.map_err(internal)?,
			(false, true) => ctx.state.unstar(&ctx.user, id).await.map_err(internal)?,
			_ => (),
		}
	}
	Ok(Element::new())
}

pub async fn star(ctx: &Ctx, params: &Params) -> Result<Element, SubsonicError> {
	set_star(ctx, params, true).await
}

pub async fn unstar(ctx: &Ctx, params: &Params) -> Result<Element, SubsonicError> {
	set_star(ctx, params, false).await
}

pub async fn set_rating(ctx: &Ctx, params: &Params) -> Result<Element, SubsonicError> {
	let id = params.required("id")?;
	let rating = params.required_int("rating")?;
	if !(0..=MAX_RATING).contains(&rating) {
		return Err(SubsonicError::invalid_parameter("rating"));
	}
	annotatable_ids(ctx, &[id]).await?;
	ctx.state
		.set_rating(&ctx.user, id, Some(rating as u8))
		.await
		.map_err(internal)?;
	Ok(Element::new())
}

// Plays

fn player_id(client: &str) -> i64 {
	(crc32fast::hash(client.as_bytes()) & 0x7fff) as i64
}

fn client_name(params: &Params) -> String {
	params.get("c").unwrap_or_default().to_owned()
}

async fn song_of(ctx: &Ctx, id: &str) -> Result<Song, SubsonicError> {
	let path = ctx.resolve_as(id, ObjectKind::Song).await?;
	ctx.index
		.get_songs(vec![PathBuf::from(path)])
		.await
		.into_iter()
		.next()
		.ok_or_else(SubsonicError::not_found)?
		.map_err(browse_error)
}

/// Counts a play of a song, and of the album it is on.
async fn record_play(ctx: &Ctx, song: &Song, song_id: &str, at: i64) -> Result<(), SubsonicError> {
	ctx.state
		.record_play(&ctx.user, song_id, at)
		.await
		.map_err(internal)?;
	if let Some(album_key) = model::song_album_key(song) {
		let album_id = ctx
			.state
			.get_or_create_id(ObjectKind::Album, &album_key)
			.await
			.map_err(internal)?;
		ctx.state
			.record_play(&ctx.user, &album_id, at)
			.await
			.map_err(internal)?;
	}
	Ok(())
}

fn now_playing_entry(ctx: &Ctx, params: &Params, song_id: &str) -> NowPlaying {
	let now = now_secs();
	NowPlaying {
		user: ctx.user.clone(),
		song_id: song_id.to_owned(),
		client: client_name(params),
		started_at: now,
		updated_at: now,
		position_ms: None,
		state: None,
		playback_rate: None,
	}
}

pub async fn scrobble(ctx: &Ctx, params: &Params) -> Result<Element, SubsonicError> {
	let ids = params.get_all("id");
	if ids.is_empty() {
		return Err(SubsonicError::missing_parameter("id"));
	}
	let times = params.ints("time")?;
	if times.iter().any(|t| *t < 0) {
		return Err(SubsonicError::invalid_parameter("time"));
	}
	let submission = params.bool("submission")?.unwrap_or(true);

	// Check everything before counting anything
	let mut songs = Vec::with_capacity(ids.len());
	for id in &ids {
		songs.push(song_of(ctx, id).await?);
	}

	let now = now_secs();
	for (index, (id, song)) in ids.iter().zip(&songs).enumerate() {
		if submission {
			// Times are milliseconds, matched to the songs in order
			let at = times.get(index).map(|ms| ms / 1000).unwrap_or(now).min(now);
			record_play(ctx, song, id, at).await?;
		} else {
			ctx.state
				.set_now_playing(now_playing_entry(ctx, params, id));
		}
	}
	if submission {
		// What was playing has been played
		ctx.state.clear_now_playing(&ctx.user);
	}
	Ok(Element::new())
}

pub async fn report_playback(ctx: &Ctx, params: &Params) -> Result<Element, SubsonicError> {
	let id = params.required("mediaId")?;
	if params.required("mediaType")? != "song" {
		return Err(SubsonicError::invalid_parameter("mediaType"));
	}
	let position = params.required_int("positionMs")?;
	if position < 0 {
		return Err(SubsonicError::invalid_parameter("positionMs"));
	}
	let state = params.required("state")?;
	if !matches!(state, "starting" | "playing" | "paused" | "stopped") {
		return Err(SubsonicError::invalid_parameter("state"));
	}
	let rate = match params.get("playbackRate").filter(|r| !r.is_empty()) {
		None => None,
		Some(r) => Some(
			r.parse::<f64>()
				.ok()
				.filter(|r| r.is_finite() && *r > 0.0)
				.ok_or_else(|| SubsonicError::invalid_parameter("playbackRate"))?,
		),
	};
	let ignore_scrobble = params.bool("ignoreScrobble")?.unwrap_or(false);
	let song = song_of(ctx, id).await?;

	if state == "stopped" {
		ctx.state.clear_now_playing(&ctx.user);
		return Ok(Element::new());
	}

	// A play is counted once, when the player starts the song
	let was_playing_this = ctx
		.state
		.now_playing_of(&ctx.user)
		.is_some_and(|n| n.song_id == id);
	if state == "starting" && !ignore_scrobble && !was_playing_this {
		record_play(ctx, &song, id, now_secs()).await?;
	}

	let mut entry = now_playing_entry(ctx, params, id);
	entry.position_ms = Some(position as u64);
	entry.state = Some(state.to_owned());
	entry.playback_rate = rate;
	ctx.state.set_now_playing(entry);
	Ok(Element::new())
}

pub async fn get_now_playing(ctx: &Ctx, _params: &Params) -> Result<Element, SubsonicError> {
	// Everyone's plays are for administrators; others see their own
	let is_admin = ctx.is_admin().await;
	let entries: Vec<NowPlaying> = ctx
		.state
		.now_playing()
		.into_iter()
		.filter(|e| is_admin || e.user == ctx.user)
		.collect();

	let mut songs = Vec::with_capacity(entries.len());
	let mut playing = Vec::with_capacity(entries.len());
	for entry in entries {
		// Songs that no longer exist are skipped
		if let Ok(song) = song_of(ctx, &entry.song_id).await {
			songs.push(song);
			playing.push(entry);
		}
	}

	let mut wanted = Wanted::new();
	wanted.songs(&songs);
	let mapper = Mapper::load(ctx, wanted).await?;
	let now = now_secs();
	let elements = mapper
		.songs(&songs)
		.await
		.into_iter()
		.zip(&playing)
		.map(|(song, entry)| {
			song.attr("username", entry.user.clone())
				.attr("minutesAgo", (now - entry.started_at).max(0) / 60)
				.attr("playerId", player_id(&entry.client))
				.attr_opt(
					"playerName",
					(!entry.client.is_empty()).then(|| entry.client.clone()),
				)
				.attr_opt("state", entry.state.clone())
				.attr_opt("positionMs", entry.position_ms.map(|p| p as i64))
				.attr_opt("playbackRate", entry.playback_rate)
		})
		.collect();
	Ok(Element::new().object(
		"nowPlaying",
		Element::new().required_list("entry", elements),
	))
}

// Starred items

struct Starred {
	artists: Vec<String>,
	albums: Vec<AlbumSummary>,
	songs: Vec<Song>,
}

/// The user's starred items, most recently starred first.
async fn starred(ctx: &Ctx) -> Result<Starred, SubsonicError> {
	let mut states: Vec<_> = ctx
		.state
		.list_item_states(&ctx.user)
		.await
		.map_err(internal)?
		.into_iter()
		.filter(|s| s.starred_at.is_some())
		.collect();
	states.sort_by_key(|s| std::cmp::Reverse(s.starred_at));

	let mut artist_names = Vec::new();
	let mut album_keys = Vec::new();
	let mut song_paths = Vec::new();
	for state in &states {
		match ctx
			.state
			.resolve_id(&state.item_id)
			.await
			.map_err(internal)?
		{
			Some((ObjectKind::Artist, name)) => artist_names.push(name),
			Some((ObjectKind::Album, key)) => album_keys.push(key),
			Some((ObjectKind::Song, path)) => song_paths.push(PathBuf::from(path)),
			_ => (),
		}
	}

	let known_artists: std::collections::HashSet<String> = ctx
		.index
		.get_artists()
		.await
		.into_iter()
		.map(|a| a.name.to_string())
		.collect();
	artist_names.retain(|a| known_artists.contains(a));

	let mut summaries: HashMap<String, AlbumSummary> = ctx
		.index
		.get_album_summaries()
		.await
		.into_iter()
		.map(|a| (model::album_key(&a.header.artists, &a.header.name), a))
		.collect();
	let albums = album_keys
		.iter()
		.filter_map(|key| summaries.remove(key))
		.collect();

	let songs = ctx
		.index
		.get_songs(song_paths)
		.await
		.into_iter()
		.filter_map(|s| s.ok())
		.collect();

	Ok(Starred {
		artists: artist_names,
		albums,
		songs,
	})
}

async fn starred_list(ctx: &Ctx, params: &Params, id3: bool) -> Result<Element, SubsonicError> {
	selected_folder(ctx, params).await?;
	let starred = starred(ctx).await?;
	let albums: Vec<AlbumData> = starred.albums.iter().map(AlbumData::from).collect();

	let mut wanted = Wanted::new();
	for artist in &starred.artists {
		wanted.artist(artist);
	}
	for album in &albums {
		wanted.album(album);
	}
	wanted.songs(&starred.songs);
	let mapper = Mapper::load(ctx, wanted).await?;

	let album_counts: HashMap<String, u32> = if id3 {
		ctx.index
			.get_artists()
			.await
			.into_iter()
			.map(|a| (a.name.to_string(), a.num_albums_as_performer))
			.collect()
	} else {
		HashMap::new()
	};
	let artists = starred
		.artists
		.iter()
		.map(|name| {
			if id3 {
				mapper.artist(name, album_counts.get(name).copied().unwrap_or_default())
			} else {
				mapper.artist_ref(name)
			}
		})
		.collect();
	let album_elements = albums
		.iter()
		.map(|a| {
			if id3 {
				mapper.album(a, None)
			} else {
				mapper.album_child(a)
			}
		})
		.collect();

	let result = Element::new()
		.list("artist", artists)
		.list("album", album_elements)
		.list("song", mapper.songs(&starred.songs).await);
	Ok(Element::new().object(if id3 { "starred2" } else { "starred" }, result))
}

pub async fn get_starred2(ctx: &Ctx, params: &Params) -> Result<Element, SubsonicError> {
	starred_list(ctx, params, true).await
}

pub async fn get_starred(ctx: &Ctx, params: &Params) -> Result<Element, SubsonicError> {
	starred_list(ctx, params, false).await
}

// Top songs

/// The songs of an artist the user has played most. Polaris has no outside source of
/// popularity, so a user's own listening is used, and songs never played are not listed.
pub async fn get_top_songs(ctx: &Ctx, params: &Params) -> Result<Element, SubsonicError> {
	let count = match params.int("count")? {
		None => DEFAULT_TOP_SONGS,
		Some(c) if c < 0 => return Err(SubsonicError::invalid_parameter("count")),
		Some(c) => (c as usize).min(MAX_TOP_SONGS),
	};

	let name = match (
		params.get("id").filter(|i| !i.is_empty()),
		params.get("artist"),
	) {
		(Some(id), _) => ctx.resolve_as(id, ObjectKind::Artist).await?,
		(None, Some(name)) if !name.is_empty() => {
			let lowered = name.to_lowercase();
			match ctx
				.index
				.get_artists()
				.await
				.into_iter()
				.find(|a| a.name.to_lowercase() == lowered)
			{
				Some(artist) => artist.name.to_string(),
				None => return Ok(Element::new().object("topSongs", Element::new())),
			}
		}
		_ => return Err(SubsonicError::missing_parameter("artist")),
	};

	let artist = ctx
		.index
		.get_artist(name.clone())
		.await
		.map_err(browse_error)?;
	let songs: Vec<Song> = artist
		.albums
		.into_iter()
		.flat_map(|a| a.songs)
		.filter(|s| s.artists.contains(&name) || s.album_artists.contains(&name))
		.collect();

	let mut wanted = Wanted::new();
	wanted.songs(&songs);
	let mapper = Mapper::load(ctx, wanted).await?;
	let play_count = |song: &Song| {
		mapper
			.state(&mapper.id(ObjectKind::Song, &model::song_key(song)))
			.map(|s| s.play_count)
			.unwrap_or_default()
	};
	let mut ranked: Vec<(u32, Song)> = songs
		.into_iter()
		.map(|s| (play_count(&s), s))
		.filter(|(plays, _)| *plays > 0)
		.collect();
	ranked.sort_by(|(a, song_a), (b, song_b)| {
		b.cmp(a)
			.then_with(|| song_a.virtual_path.cmp(&song_b.virtual_path))
	});
	let songs: Vec<Song> = ranked.into_iter().take(count).map(|(_, s)| s).collect();

	Ok(Element::new().object(
		"topSongs",
		Element::new().list("song", mapper.songs(&songs).await),
	))
}
