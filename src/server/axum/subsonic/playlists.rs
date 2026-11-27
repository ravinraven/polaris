//! Playlists. Polaris playlists are private and identified by name, so a playlist's owner is
//! always the user who made it, `public` is always false, and a name can only be used once per
//! user.

use std::path::PathBuf;

use crate::app::{playlist::Playlist, state::ObjectKind, Error};

use super::ctx::{internal, Ctx};
use super::model::{self, Mapper, Wanted};
use super::params::Params;
use super::response::{Element, SubsonicError};

const MAX_NAME_LENGTH: usize = 200;

fn playlist_error(error: Error) -> SubsonicError {
	match error {
		Error::PlaylistNotFound => SubsonicError::not_found(),
		other => internal(other),
	}
}

/// The (owner, name) of a playlist ID, if the current user may see it.
async fn visible_playlist(ctx: &Ctx, id: &str) -> Result<(String, String), SubsonicError> {
	let key = ctx.resolve_as(id, ObjectKind::Playlist).await?;
	let (owner, name) = model::parse_playlist_key(&key).ok_or_else(SubsonicError::not_found)?;
	if owner != ctx.user && !ctx.is_admin().await {
		return Err(SubsonicError::not_found());
	}
	Ok((owner.to_owned(), name.to_owned()))
}

/// Like [`visible_playlist`], but only the owner may change a playlist.
async fn own_playlist(ctx: &Ctx, id: &str) -> Result<String, SubsonicError> {
	let (owner, name) = visible_playlist(ctx, id).await?;
	if owner != ctx.user {
		return Err(SubsonicError::not_authorized());
	}
	Ok(name)
}

fn validated_name(name: &str) -> Result<String, SubsonicError> {
	let name = name.trim();
	if name.is_empty()
		|| name.chars().count() > MAX_NAME_LENGTH
		|| name.chars().any(char::is_control)
	{
		return Err(SubsonicError::invalid_parameter("name"));
	}
	Ok(name.to_owned())
}

/// The virtual paths of songs given by ID. Every ID must be a song.
async fn song_paths(ctx: &Ctx, ids: Vec<&str>) -> Result<Vec<PathBuf>, SubsonicError> {
	let mut paths = Vec::with_capacity(ids.len());
	for id in ids {
		paths.push(PathBuf::from(ctx.resolve_as(id, ObjectKind::Song).await?));
	}
	Ok(paths)
}

async fn describe(
	ctx: &Ctx,
	mapper: &Mapper,
	owner: &str,
	name: &str,
	playlist: &Playlist,
) -> Result<Element, SubsonicError> {
	let meta = ctx
		.state
		.playlist_meta(owner, name)
		.await
		.map_err(internal)?;
	let id = mapper.id(ObjectKind::Playlist, &model::playlist_key(owner, name));
	Ok(Element::new()
		.attr("id", id)
		.attr("name", name)
		.attr_opt("comment", meta.comment)
		.attr("owner", owner)
		.attr("public", false)
		.attr("songCount", playlist.songs.len() as u32)
		.attr("duration", playlist.header.duration.as_secs() as i64)
		.attr("created", model::iso8601(meta.created_at))
		.attr("changed", model::iso8601(meta.changed_at)))
}

async fn full_playlist(ctx: &Ctx, owner: &str, name: &str) -> Result<Element, SubsonicError> {
	let playlist = ctx
		.playlists
		.read_playlist(name, owner)
		.await
		.map_err(playlist_error)?;
	let songs: Vec<_> = ctx
		.index
		.get_songs(playlist.songs.clone())
		.await
		.into_iter()
		.filter_map(|s| s.ok())
		.collect();

	let mut wanted = Wanted::new();
	wanted.playlist(owner, name);
	wanted.songs(&songs);
	let mapper = Mapper::load(ctx, wanted).await?;

	let entries = mapper.songs(&songs).await;
	Ok(describe(ctx, &mapper, owner, name, &playlist)
		.await?
		.list("entry", entries))
}

pub async fn get_playlists(ctx: &Ctx, params: &Params) -> Result<Element, SubsonicError> {
	let owner = match params.get("username").filter(|u| !u.is_empty()) {
		Some(other) if other != ctx.user => {
			if !ctx.is_admin().await {
				return Err(SubsonicError::not_authorized());
			}
			other.to_owned()
		}
		_ => ctx.user.clone(),
	};

	let headers = ctx
		.playlists
		.list_playlists(&owner)
		.await
		.map_err(playlist_error)?;
	let mut playlists = Vec::with_capacity(headers.len());
	for header in &headers {
		playlists.push(
			ctx.playlists
				.read_playlist(&header.name, &owner)
				.await
				.map_err(playlist_error)?,
		);
	}

	let mut wanted = Wanted::new();
	for header in &headers {
		wanted.playlist(&owner, &header.name);
	}
	let mapper = Mapper::load(ctx, wanted).await?;

	let mut elements = Vec::with_capacity(playlists.len());
	for (header, playlist) in headers.iter().zip(&playlists) {
		elements.push(describe(ctx, &mapper, &owner, &header.name, playlist).await?);
	}
	Ok(Element::new().object("playlists", Element::new().list("playlist", elements)))
}

pub async fn get_playlist(ctx: &Ctx, params: &Params) -> Result<Element, SubsonicError> {
	let id = params.required("id")?;
	let (owner, name) = visible_playlist(ctx, id).await?;
	Ok(Element::new().object("playlist", full_playlist(ctx, &owner, &name).await?))
}

pub async fn create_playlist(ctx: &Ctx, params: &Params) -> Result<Element, SubsonicError> {
	let song_ids = params.get_all("songId");
	let paths = song_paths(ctx, song_ids).await?;

	let name = match params.get("playlistId").filter(|id| !id.is_empty()) {
		// With an ID, the songs of an existing playlist are replaced
		Some(id) => {
			let current = own_playlist(ctx, id).await?;
			ctx.playlists
				.save_playlist(&current, &ctx.user, paths)
				.await
				.map_err(playlist_error)?;
			match params.get("name").filter(|n| !n.is_empty()) {
				Some(new_name) => rename(ctx, &current, &validated_name(new_name)?).await?,
				None => {
					ctx.state
						.touch_playlist(&ctx.user, &current, None)
						.await
						.map_err(internal)?;
					current
				}
			}
		}
		None => {
			let name = validated_name(params.required("name")?)?;
			if ctx.playlists.read_playlist(&name, &ctx.user).await.is_ok() {
				return Err(SubsonicError::generic(format!(
					"A playlist named \"{name}\" already exists"
				)));
			}
			ctx.playlists
				.save_playlist(&name, &ctx.user, paths)
				.await
				.map_err(playlist_error)?;
			ctx.state
				.touch_playlist(&ctx.user, &name, None)
				.await
				.map_err(internal)?;
			name
		}
	};
	Ok(Element::new().object("playlist", full_playlist(ctx, &ctx.user, &name).await?))
}

/// Renames a playlist, refusing to replace another one. Returns the new name.
async fn rename(ctx: &Ctx, current: &str, new_name: &str) -> Result<String, SubsonicError> {
	if new_name == current {
		return Ok(current.to_owned());
	}
	if ctx
		.playlists
		.read_playlist(new_name, &ctx.user)
		.await
		.is_ok()
	{
		return Err(SubsonicError::generic(format!(
			"A playlist named \"{new_name}\" already exists"
		)));
	}
	let playlist = ctx
		.playlists
		.read_playlist(current, &ctx.user)
		.await
		.map_err(playlist_error)?;
	ctx.playlists
		.save_playlist(new_name, &ctx.user, playlist.songs)
		.await
		.map_err(playlist_error)?;
	ctx.playlists
		.delete_playlist(current, &ctx.user)
		.await
		.map_err(playlist_error)?;
	ctx.state
		.rename_playlist(&ctx.user, current, new_name)
		.await
		.map_err(internal)?;
	Ok(new_name.to_owned())
}

pub async fn update_playlist(ctx: &Ctx, params: &Params) -> Result<Element, SubsonicError> {
	let id = params.required("playlistId")?;
	let name = own_playlist(ctx, id).await?;

	let to_add = song_paths(ctx, params.get_all("songIdToAdd")).await?;
	let to_remove = params.ints("songIndexToRemove")?;
	let new_name = match params.get("name").filter(|n| !n.is_empty()) {
		Some(n) => Some(validated_name(n)?),
		None => None,
	};
	// Not stored: Polaris playlists are always private
	params.bool("public")?;

	let songs = ctx
		.playlists
		.read_playlist(&name, &ctx.user)
		.await
		.map_err(playlist_error)?
		.songs;
	if !to_remove.is_empty() || !to_add.is_empty() {
		// Indices refer to the playlist as it was before this request
		let mut keep = vec![true; songs.len()];
		for index in &to_remove {
			let slot = usize::try_from(*index)
				.ok()
				.and_then(|i| keep.get_mut(i))
				.ok_or_else(|| SubsonicError::invalid_parameter("songIndexToRemove"))?;
			*slot = false;
		}
		let mut kept: Vec<PathBuf> = songs
			.into_iter()
			.zip(keep)
			.filter_map(|(song, keep)| keep.then_some(song))
			.collect();
		kept.extend(to_add);
		ctx.playlists
			.save_playlist(&name, &ctx.user, kept)
			.await
			.map_err(playlist_error)?;
	}

	let comment = params.get("comment").map(|c| match c.trim() {
		"" => None,
		text => Some(text.to_owned()),
	});
	ctx.state
		.touch_playlist(&ctx.user, &name, comment)
		.await
		.map_err(internal)?;
	if let Some(new_name) = new_name {
		rename(ctx, &name, &new_name).await?;
	}
	Ok(Element::new())
}

pub async fn delete_playlist(ctx: &Ctx, params: &Params) -> Result<Element, SubsonicError> {
	let id = params.required("id")?;
	let name = own_playlist(ctx, id).await?;
	ctx.playlists
		.delete_playlist(&name, &ctx.user)
		.await
		.map_err(playlist_error)?;
	ctx.state
		.forget_playlist(&ctx.user, &name)
		.await
		.map_err(internal)?;
	Ok(Element::new())
}

#[cfg(test)]
mod test {
	use super::*;

	#[test]
	fn names_are_trimmed_and_limited() {
		assert_eq!(validated_name("  mix  ").unwrap(), "mix");
		assert_eq!(validated_name("ünï 🎧 / slash").unwrap(), "ünï 🎧 / slash");
		assert!(validated_name("").is_err());
		assert!(validated_name("   ").is_err());
		assert!(validated_name("a\nb").is_err());
		assert!(validated_name(&"x".repeat(201)).is_err());
		assert!(validated_name(&"x".repeat(200)).is_ok());
	}
}
