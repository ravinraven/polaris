//! Lists of albums and songs, and search.

use std::{cmp::Reverse, collections::HashMap, path::PathBuf};

use rand::seq::SliceRandom;

use crate::app::{
	index::{AlbumSummary, Song},
	state::{ItemState, ObjectKind},
};

use super::browse::{browse_error, selected_folder};
use super::ctx::{internal, Ctx};
use super::model::{self, AlbumData, Mapper, Wanted};
use super::params::Params;
use super::response::{Element, SubsonicError};

const DEFAULT_PAGE_SIZE: i64 = 10;
const MAX_PAGE_SIZE: i64 = 500;
const DEFAULT_SEARCH_COUNT: i64 = 20;

/// A page size or offset parameter: a non-negative number, at most `max`.
fn count_param(
	params: &Params,
	name: &str,
	default: i64,
	max: i64,
) -> Result<usize, SubsonicError> {
	let value = params.int(name)?.unwrap_or(default);
	if value < 0 {
		return Err(SubsonicError::invalid_parameter(name));
	}
	Ok(value.min(max) as usize)
}

/// The states of albums the user has starred, rated or played, by album key.
async fn album_states(ctx: &Ctx) -> Result<HashMap<String, ItemState>, SubsonicError> {
	let mut by_key = HashMap::new();
	for state in ctx
		.state
		.list_item_states(&ctx.user)
		.await
		.map_err(internal)?
	{
		if !state.item_id.starts_with("al_") {
			continue;
		}
		if let Some((ObjectKind::Album, key)) = ctx
			.state
			.resolve_id(&state.item_id)
			.await
			.map_err(internal)?
		{
			by_key.insert(key, state);
		}
	}
	Ok(by_key)
}

fn summary_key(album: &AlbumSummary) -> String {
	model::album_key(&album.header.artists, &album.header.name)
}

/// The albums of a list type, in order, before paging.
async fn albums_of_type(
	ctx: &Ctx,
	params: &Params,
	list_type: &str,
) -> Result<Vec<AlbumSummary>, SubsonicError> {
	let mut albums = ctx.index.get_album_summaries().await;

	match list_type {
		"random" => albums.shuffle(&mut rand::rng()),
		"newest" => albums.sort_by_key(|a| Reverse(a.header.date_added)),
		"alphabeticalByName" => (),
		"alphabeticalByArtist" => albums.sort_by_cached_key(|a| {
			(
				a.header.artists.join(", ").to_lowercase(),
				a.header.name.to_lowercase(),
			)
		}),
		"byYear" => {
			let from = params
				.int("fromYear")?
				.ok_or_else(|| SubsonicError::missing_parameter("fromYear"))?;
			let to = params
				.int("toYear")?
				.ok_or_else(|| SubsonicError::missing_parameter("toYear"))?;
			let (low, high) = (from.min(to), from.max(to));
			albums.retain(|a| a.header.year.is_some_and(|y| (low..=high).contains(&y)));
			albums.sort_by_key(|a| a.header.year);
			// A range given backwards means newest first
			if from > to {
				albums.reverse();
			}
		}
		"byGenre" => {
			let genre = params.required("genre")?.to_lowercase();
			albums.retain(|a| a.genres.iter().any(|g| g.to_lowercase() == genre));
		}
		"starred" | "highest" | "frequent" | "recent" => {
			let states = album_states(ctx).await?;
			albums.retain(|a| {
				states
					.get(&summary_key(a))
					.is_some_and(|s| match list_type {
						"starred" => s.starred_at.is_some(),
						"highest" => s.rating.is_some(),
						"frequent" => s.play_count > 0,
						_ => s.last_played.is_some(),
					})
			});
			let sort_value = |a: &AlbumSummary| -> i64 {
				states
					.get(&summary_key(a))
					.map(|s| match list_type {
						"starred" => s.starred_at.unwrap_or_default(),
						"highest" => s.rating.unwrap_or_default() as i64,
						"frequent" => s.play_count as i64,
						_ => s.last_played.unwrap_or_default(),
					})
					.unwrap_or_default()
			};
			albums.sort_by_cached_key(|a| Reverse(sort_value(a)));
		}
		_ => return Err(SubsonicError::invalid_parameter("type")),
	}
	Ok(albums)
}

async fn album_list(ctx: &Ctx, params: &Params, id3: bool) -> Result<Element, SubsonicError> {
	let list_type = params.required("type")?;
	selected_folder(ctx, params).await?;
	let size = count_param(params, "size", DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE)?;
	let offset = count_param(params, "offset", 0, i64::MAX)?;

	let albums: Vec<AlbumData> = albums_of_type(ctx, params, list_type)
		.await?
		.iter()
		.skip(offset)
		.take(size)
		.map(AlbumData::from)
		.collect();

	let mut wanted = Wanted::new();
	for album in &albums {
		wanted.album(album);
	}
	let mapper = Mapper::load(ctx, wanted).await?;
	let elements = albums
		.iter()
		.map(|a| {
			if id3 {
				mapper.album(a, None)
			} else {
				mapper.album_child(a)
			}
		})
		.collect();

	let name = if id3 { "albumList2" } else { "albumList" };
	Ok(Element::new().object(name, Element::new().list("album", elements)))
}

pub async fn get_album_list2(ctx: &Ctx, params: &Params) -> Result<Element, SubsonicError> {
	album_list(ctx, params, true).await
}

pub async fn get_album_list(ctx: &Ctx, params: &Params) -> Result<Element, SubsonicError> {
	album_list(ctx, params, false).await
}

// Songs

async fn song_elements(ctx: &Ctx, songs: &[Song]) -> Result<Vec<Element>, SubsonicError> {
	let mut wanted = Wanted::new();
	wanted.songs(songs);
	let mapper = Mapper::load(ctx, wanted).await?;
	Ok(mapper.songs(songs).await)
}

async fn all_songs(ctx: &Ctx) -> Result<Vec<Song>, SubsonicError> {
	let paths = ctx
		.index
		.flatten(PathBuf::new())
		.await
		.map_err(browse_error)?;
	Ok(ctx
		.index
		.get_songs(paths)
		.await
		.into_iter()
		.filter_map(|s| s.ok())
		.collect())
}

async fn songs_of_genre(ctx: &Ctx, genre: &str) -> Result<Vec<Song>, SubsonicError> {
	let genres = ctx.index.get_genre_summaries().await;
	// Genre names are matched without regard to case, as clients may change it
	let Some(found) = genres
		.into_iter()
		.find(|g| g.name.to_lowercase() == genre.to_lowercase())
	else {
		return Ok(Vec::new());
	};
	Ok(ctx
		.index
		.get_genre(found.name)
		.await
		.map_err(browse_error)?
		.songs)
}

pub async fn get_random_songs(ctx: &Ctx, params: &Params) -> Result<Element, SubsonicError> {
	selected_folder(ctx, params).await?;
	let size = count_param(params, "size", DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE)?;
	let genre = params.get("genre").filter(|g| !g.is_empty());
	let from_year = params.int("fromYear")?;
	let to_year = params.int("toYear")?;

	let mut songs = match genre {
		Some(genre) => songs_of_genre(ctx, genre).await?,
		None => all_songs(ctx).await?,
	};
	songs.retain(|s| match s.year {
		Some(year) => from_year.is_none_or(|f| year >= f) && to_year.is_none_or(|t| year <= t),
		None => from_year.is_none() && to_year.is_none(),
	});
	songs.shuffle(&mut rand::rng());
	songs.truncate(size);

	let elements = song_elements(ctx, &songs).await?;
	Ok(Element::new().object("randomSongs", Element::new().list("song", elements)))
}

pub async fn get_songs_by_genre(ctx: &Ctx, params: &Params) -> Result<Element, SubsonicError> {
	selected_folder(ctx, params).await?;
	let genre = params.required("genre")?;
	let count = count_param(params, "count", DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE)?;
	let offset = count_param(params, "offset", 0, i64::MAX)?;

	let songs: Vec<Song> = songs_of_genre(ctx, genre)
		.await?
		.into_iter()
		.skip(offset)
		.take(count)
		.collect();
	let elements = song_elements(ctx, &songs).await?;
	Ok(Element::new().object("songsByGenre", Element::new().list("song", elements)))
}

// Search

/// A Polaris search query that looks for `text` anywhere. Quotes inside the text would end the
/// quoted string early, so they are dropped.
pub fn text_query(text: &str) -> String {
	format!("\"{}\"", text.replace('"', " "))
}

fn contains_ignoring_case(haystack: &str, needle_lowercase: &str) -> bool {
	haystack.to_lowercase().contains(needle_lowercase)
}

async fn search(ctx: &Ctx, params: &Params, id3: bool) -> Result<Element, SubsonicError> {
	let query = params
		.get("query")
		.ok_or_else(|| SubsonicError::missing_parameter("query"))?;
	selected_folder(ctx, params).await?;
	let artist_count = count_param(params, "artistCount", DEFAULT_SEARCH_COUNT, MAX_PAGE_SIZE)?;
	let artist_offset = count_param(params, "artistOffset", 0, i64::MAX)?;
	let album_count = count_param(params, "albumCount", DEFAULT_SEARCH_COUNT, MAX_PAGE_SIZE)?;
	let album_offset = count_param(params, "albumOffset", 0, i64::MAX)?;
	let song_count = count_param(params, "songCount", DEFAULT_SEARCH_COUNT, MAX_PAGE_SIZE)?;
	let song_offset = count_param(params, "songOffset", 0, i64::MAX)?;

	// Many clients ask for everything with an empty query, or `*`, to sync a whole library
	let text = query.trim().trim_matches('"');
	let everything = text.is_empty() || text == "*";
	let needle = text.to_lowercase();

	let mut artists = ctx.index.get_artists().await;
	if !everything {
		artists.retain(|a| contains_ignoring_case(&a.name, &needle));
	}
	let artists: Vec<_> = artists
		.into_iter()
		.skip(artist_offset)
		.take(artist_count)
		.collect();

	let mut albums = ctx.index.get_album_summaries().await;
	if !everything {
		albums.retain(|a| {
			contains_ignoring_case(&a.header.name, &needle)
				|| a.header
					.artists
					.iter()
					.any(|artist| contains_ignoring_case(artist, &needle))
		});
	}
	let albums: Vec<AlbumData> = albums
		.iter()
		.skip(album_offset)
		.take(album_count)
		.map(AlbumData::from)
		.collect();

	let paths = if everything {
		ctx.index
			.flatten(PathBuf::new())
			.await
			.map_err(browse_error)?
	} else {
		ctx.index
			.search(text_query(text))
			.await
			.map_err(|_| SubsonicError::generic("Could not search for that text"))?
	};
	let songs: Vec<Song> = ctx
		.index
		.get_songs(
			paths
				.into_iter()
				.skip(song_offset)
				.take(song_count)
				.collect(),
		)
		.await
		.into_iter()
		.filter_map(|s| s.ok())
		.collect();

	let mut wanted = Wanted::new();
	for artist in &artists {
		wanted.artist(&artist.name);
	}
	for album in &albums {
		wanted.album(album);
	}
	wanted.songs(&songs);
	let mapper = Mapper::load(ctx, wanted).await?;

	let artist_elements = artists
		.iter()
		.map(|a| {
			if id3 {
				mapper.artist(&a.name, a.num_albums_as_performer)
			} else {
				mapper.artist_ref(&a.name)
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
		.list("artist", artist_elements)
		.list("album", album_elements)
		.list("song", mapper.songs(&songs).await);
	let name = if id3 {
		"searchResult3"
	} else {
		"searchResult2"
	};
	Ok(Element::new().object(name, result))
}

pub async fn search3(ctx: &Ctx, params: &Params) -> Result<Element, SubsonicError> {
	search(ctx, params, true).await
}

pub async fn search2(ctx: &Ctx, params: &Params) -> Result<Element, SubsonicError> {
	search(ctx, params, false).await
}

#[cfg(test)]
mod test {
	use super::*;

	#[test]
	fn search_text_is_quoted_without_breaking_out_of_the_quotes() {
		assert_eq!(text_query("rhapsody"), "\"rhapsody\"");
		assert_eq!(text_query("a \"b\" c"), "\"a  b  c\"");
		assert_eq!(text_query("a || b"), "\"a || b\"");
	}

	#[test]
	fn counts_must_be_non_negative_and_are_capped() {
		let params = Params::parse(Some("a=5&b=-1&c=100000&d=x"), None);
		assert_eq!(count_param(&params, "a", 10, 500).unwrap(), 5);
		assert_eq!(count_param(&params, "missing", 10, 500).unwrap(), 10);
		assert_eq!(count_param(&params, "c", 10, 500).unwrap(), 500);
		assert!(count_param(&params, "b", 10, 500).is_err());
		assert!(count_param(&params, "d", 10, 500).is_err());
	}
}
