//! Lyrics, from `.lrc` files next to songs and from tags (`songLyrics` extension, version 1).

use std::path::PathBuf;

use crate::app::{index::Song, state::ObjectKind};

use super::browse::browse_error;
use super::ctx::Ctx;
use super::lists::text_query;
use super::params::Params;
use super::response::{Element, SubsonicError};
use super::tags::{self, Lyrics};

/// What to report as the language of lyrics: Polaris does not know it.
const UNKNOWN_LANGUAGE: &str = "xxx";

async fn song_by_id(ctx: &Ctx, id: &str) -> Result<Song, SubsonicError> {
	let path = ctx.resolve_as(id, ObjectKind::Song).await?;
	ctx.index
		.get_songs(vec![PathBuf::from(path)])
		.await
		.into_iter()
		.next()
		.ok_or_else(SubsonicError::not_found)?
		.map_err(browse_error)
}

fn structured(lyrics: &Lyrics, song: &Song) -> Element {
	let lines = lyrics
		.lines
		.iter()
		.map(|line| {
			Element::new()
				.attr_opt("start", line.start)
				.text(line.text.clone())
		})
		.collect();
	let artist = lyrics
		.artist
		.clone()
		.or_else(|| (!song.artists.is_empty()).then(|| song.artists.join(", ")));
	let title = lyrics.title.clone().or_else(|| song.title.clone());
	Element::new()
		.attr("lang", UNKNOWN_LANGUAGE)
		.attr("synced", lyrics.synced)
		.attr_opt("displayArtist", artist)
		.attr_opt("displayTitle", title)
		.attr_opt("offset", (lyrics.offset != 0).then_some(lyrics.offset))
		.required_list("line", lines)
}

/// `getLyricsBySongId`. The `enhanced` parameter (version 2 of the extension) is accepted and
/// ignored: Polaris answers with the version 1 form.
pub async fn get_lyrics_by_song_id(ctx: &Ctx, params: &Params) -> Result<Element, SubsonicError> {
	let id = params.required("id")?;
	params.bool("enhanced")?;
	let song = song_by_id(ctx, id).await?;
	let found = tags::lyrics(&song.real_path).await;
	let lists = found.iter().map(|l| structured(l, &song)).collect();
	Ok(Element::new().object("lyricsList", Element::new().list("structuredLyrics", lists)))
}

/// `getLyrics`: the plain text of the lyrics of the song with this artist and title.
pub async fn get_lyrics(ctx: &Ctx, params: &Params) -> Result<Element, SubsonicError> {
	let artist = params
		.get("artist")
		.unwrap_or_default()
		.trim()
		.to_lowercase();
	let title = params
		.get("title")
		.unwrap_or_default()
		.trim()
		.to_lowercase();

	let mut text = String::new();
	let query = if title.is_empty() { &artist } else { &title };
	if !query.is_empty() {
		let paths = ctx
			.index
			.search(text_query(query))
			.await
			.map_err(|_| SubsonicError::generic("Could not search for that song"))?;
		let songs = ctx.index.get_songs(paths).await;
		for song in songs.into_iter().filter_map(|s| s.ok()) {
			let title_matches = title.is_empty()
				|| song
					.title
					.as_deref()
					.is_some_and(|t| t.to_lowercase() == title);
			let artist_matches =
				artist.is_empty() || song.artists.iter().any(|a| a.to_lowercase() == artist);
			if !(title_matches && artist_matches) {
				continue;
			}
			if let Some(lyrics) = tags::lyrics(&song.real_path).await.first() {
				text = lyrics.text();
				break;
			}
		}
	}

	let element = Element::new()
		.attr_opt("artist", params.get("artist").filter(|a| !a.is_empty()))
		.attr_opt("title", params.get("title").filter(|t| !t.is_empty()))
		.text(text);
	Ok(Element::new().object("lyrics", element))
}
