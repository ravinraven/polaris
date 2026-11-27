//! Bookmarks and the saved play queue.

use std::path::PathBuf;

use crate::app::{index::Song, state::ObjectKind};

use super::ctx::{internal, Ctx};
use super::model::{self, Mapper, Wanted};
use super::params::Params;
use super::response::{Element, SubsonicError};

const MAX_COMMENT_LENGTH: usize = 4000;

/// The songs behind song IDs, in order. IDs of songs that no longer exist are `None`.
async fn songs_by_id(ctx: &Ctx, ids: &[String]) -> Result<Vec<Option<Song>>, SubsonicError> {
	let mut paths = Vec::new();
	let mut slots = Vec::with_capacity(ids.len());
	for id in ids {
		match ctx.state.resolve_id(id).await.map_err(internal)? {
			Some((ObjectKind::Song, path)) => {
				slots.push(Some(paths.len()));
				paths.push(PathBuf::from(path));
			}
			_ => slots.push(None),
		}
	}
	let mut found: Vec<Option<Song>> = ctx
		.index
		.get_songs(paths)
		.await
		.into_iter()
		.map(|s| s.ok())
		.collect();
	Ok(slots
		.into_iter()
		.map(|slot| slot.and_then(|i| found[i].take()))
		.collect())
}

/// Song IDs from a request, every one of which must be a song.
async fn require_songs(ctx: &Ctx, ids: &[&str]) -> Result<(), SubsonicError> {
	for id in ids {
		ctx.resolve_as(id, ObjectKind::Song).await?;
	}
	Ok(())
}

// Bookmarks

pub async fn get_bookmarks(ctx: &Ctx, _params: &Params) -> Result<Element, SubsonicError> {
	let bookmarks = ctx
		.state
		.list_bookmarks(&ctx.user)
		.await
		.map_err(internal)?;
	let ids: Vec<String> = bookmarks.iter().map(|b| b.song_id.clone()).collect();
	let songs = songs_by_id(ctx, &ids).await?;

	// Bookmarks of songs that are gone are left out
	let present: Vec<_> = bookmarks
		.iter()
		.zip(songs)
		.filter_map(|(b, s)| s.map(|s| (b, s)))
		.collect();
	let mut wanted = Wanted::new();
	for (_, song) in &present {
		wanted.song(song);
	}
	let mapper = Mapper::load(ctx, wanted).await?;
	let song_list: Vec<Song> = present.iter().map(|(_, s)| s.clone()).collect();
	let entries = mapper.songs(&song_list).await;

	let elements = present
		.iter()
		.zip(entries)
		.map(|((bookmark, _), entry)| {
			Element::new()
				.attr("position", bookmark.position_ms as i64)
				.attr("username", ctx.user.clone())
				.attr_opt("comment", bookmark.comment.clone())
				.attr("created", model::iso8601(bookmark.created_at))
				.attr("changed", model::iso8601(bookmark.changed_at))
				.object("entry", entry)
		})
		.collect();
	Ok(Element::new().object("bookmarks", Element::new().list("bookmark", elements)))
}

pub async fn create_bookmark(ctx: &Ctx, params: &Params) -> Result<Element, SubsonicError> {
	let id = params.required("id")?;
	let position = params.required_int("position")?;
	if position < 0 {
		return Err(SubsonicError::invalid_parameter("position"));
	}
	let comment = params
		.get("comment")
		.filter(|c| !c.is_empty())
		.map(str::to_owned);
	if comment
		.as_ref()
		.is_some_and(|c| c.chars().count() > MAX_COMMENT_LENGTH)
	{
		return Err(SubsonicError::invalid_parameter("comment"));
	}
	require_songs(ctx, &[id]).await?;
	ctx.state
		.save_bookmark(&ctx.user, id, position as u64, comment)
		.await
		.map_err(internal)?;
	Ok(Element::new())
}

pub async fn delete_bookmark(ctx: &Ctx, params: &Params) -> Result<Element, SubsonicError> {
	let id = params.required("id")?;
	if ctx
		.state
		.delete_bookmark(&ctx.user, id)
		.await
		.map_err(internal)?
	{
		Ok(Element::new())
	} else {
		Err(SubsonicError::not_found())
	}
}

// The play queue

fn client_name(params: &Params) -> String {
	params.get("c").unwrap_or_default().to_owned()
}

async fn save_queue(
	ctx: &Ctx,
	params: &Params,
	current_index: impl FnOnce(&[&str]) -> Result<Option<u32>, SubsonicError>,
) -> Result<Element, SubsonicError> {
	let ids = params.get_all("id");
	// No songs at all clears the queue
	if ids.is_empty() {
		ctx.state
			.delete_play_queue(&ctx.user)
			.await
			.map_err(internal)?;
		return Ok(Element::new());
	}
	require_songs(ctx, &ids).await?;
	let position = params.int("position")?.unwrap_or(0);
	if position < 0 {
		return Err(SubsonicError::invalid_parameter("position"));
	}
	let current = current_index(&ids)?;
	ctx.state
		.save_play_queue(
			&ctx.user,
			ids.iter().map(|id| (*id).to_owned()).collect(),
			current,
			position as u64,
			&client_name(params),
		)
		.await
		.map_err(internal)?;
	Ok(Element::new())
}

pub async fn save_play_queue(ctx: &Ctx, params: &Params) -> Result<Element, SubsonicError> {
	save_queue(ctx, params, |ids| {
		match params.get("current").filter(|c| !c.is_empty()) {
			None => Ok(None),
			// With duplicates, the first occurrence is the current one
			Some(current) => ids
				.iter()
				.position(|id| *id == current)
				.map(|i| Some(i as u32))
				.ok_or_else(|| SubsonicError::invalid_parameter("current")),
		}
	})
	.await
}

pub async fn save_play_queue_by_index(
	ctx: &Ctx,
	params: &Params,
) -> Result<Element, SubsonicError> {
	save_queue(ctx, params, |ids| match params.int("currentIndex")? {
		None => Ok(None),
		Some(i) if (0..ids.len() as i64).contains(&i) => Ok(Some(i as u32)),
		Some(_) => Err(SubsonicError::invalid_parameter("currentIndex")),
	})
	.await
}

async fn get_queue(ctx: &Ctx, by_index: bool) -> Result<Element, SubsonicError> {
	let name = if by_index {
		"playQueueByIndex"
	} else {
		"playQueue"
	};
	let Some(queue) = ctx
		.state
		.get_play_queue(&ctx.user)
		.await
		.map_err(internal)?
	else {
		// The specification always has a queue in the response: this one is empty.
		return Ok(Element::new().object(
			name,
			Element::new()
				.attr("username", ctx.user.clone())
				.attr("changed", model::iso8601(0))
				.attr("changedBy", ""),
		));
	};

	let songs = songs_by_id(ctx, &queue.entries).await?;
	// Songs that are gone are left out, which moves the ones after them up
	let mut kept = Vec::new();
	let mut current_song_id = None;
	let mut new_index = None;
	for (index, (id, song)) in queue.entries.iter().zip(songs).enumerate() {
		if Some(index as u32) == queue.current_index {
			current_song_id = Some(id.clone());
			new_index = song.as_ref().map(|_| kept.len() as i64);
		}
		if let Some(song) = song {
			kept.push(song);
		}
	}

	let mut wanted = Wanted::new();
	wanted.songs(&kept);
	let mapper = Mapper::load(ctx, wanted).await?;

	let mut element = Element::new()
		.attr("position", queue.position_ms as i64)
		.attr("username", ctx.user.clone())
		.attr("changed", model::iso8601(queue.changed_at))
		.attr("changedBy", queue.changed_by.clone());
	if by_index {
		element = element.attr_opt("currentIndex", new_index);
	} else {
		let current = current_song_id.filter(|_| new_index.is_some());
		element = element.attr_opt("current", current);
	}
	element = element.list("entry", mapper.songs(&kept).await);
	Ok(Element::new().object(name, element))
}

pub async fn get_play_queue(ctx: &Ctx, _params: &Params) -> Result<Element, SubsonicError> {
	get_queue(ctx, false).await
}

pub async fn get_play_queue_by_index(
	ctx: &Ctx,
	_params: &Params,
) -> Result<Element, SubsonicError> {
	get_queue(ctx, true).await
}
