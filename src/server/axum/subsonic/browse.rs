//! Browsing: music folders, the folder-based index and directories, and the tag-based
//! artists, albums, songs and genres.

use std::path::{Path, PathBuf};

use crate::app::{
	index::{Album, File},
	state::ObjectKind,
	Error,
};

use super::ctx::{internal, Ctx};
use super::model::{self, AlbumData, Mapper, Wanted};
use super::params::Params;
use super::response::{Element, SubsonicError};

/// Articles that are ignored when sorting artists into the index.
pub const IGNORED_ARTICLES: &str = "The El La Los Las Le Les";

pub fn browse_error(error: Error) -> SubsonicError {
	match error {
		Error::DirectoryNotFound(_)
		| Error::ArtistNotFound
		| Error::AlbumNotFound
		| Error::SongNotFound
		| Error::GenreNotFound => SubsonicError::not_found(),
		other => internal(other),
	}
}

pub fn last_component(path: &Path) -> String {
	path.file_name()
		.map(|n| n.to_string_lossy().into_owned())
		.unwrap_or_else(|| path.to_string_lossy().into_owned())
}

// Music folders

/// The music folders (the mount directories), with their numeric IDs. The ID comes from the
/// folder name, so it survives changes to the other folders.
pub async fn music_folders(ctx: &Ctx) -> Vec<(i64, String)> {
	let mut names: Vec<String> = ctx
		.config
		.get_mounts()
		.await
		.into_iter()
		.map(|m| m.name)
		.collect();
	names.sort();

	let mut used = std::collections::HashSet::new();
	let mut folders = Vec::new();
	for name in names {
		let mut id = (crc32fast::hash(name.as_bytes()) & 0x7fff_ffff) as i64;
		while !used.insert(id) {
			id = (id + 1) & 0x7fff_ffff;
		}
		folders.push((id, name));
	}
	folders
}

/// The folder selected by an optional `musicFolderId` parameter, if any. An unknown folder is
/// an error.
pub async fn selected_folder(
	ctx: &Ctx,
	params: &Params,
) -> Result<Option<(i64, String)>, SubsonicError> {
	let Some(requested) = params.get("musicFolderId").filter(|v| !v.is_empty()) else {
		return Ok(None);
	};
	let requested: i64 = requested
		.parse()
		.map_err(|_| SubsonicError::invalid_parameter("musicFolderId"))?;
	music_folders(ctx)
		.await
		.into_iter()
		.find(|(id, _)| *id == requested)
		.map(Some)
		.ok_or_else(SubsonicError::not_found)
}

pub async fn get_music_folders(ctx: &Ctx, _params: &Params) -> Result<Element, SubsonicError> {
	let folders = music_folders(ctx)
		.await
		.into_iter()
		.map(|(id, name)| Element::new().attr("id", id).attr("name", name))
		.collect();
	Ok(Element::new().object("musicFolders", Element::new().list("musicFolder", folders)))
}

// The folder based index

/// The index heading an artist name goes under: its first letter, ignoring a leading article.
pub fn index_letter(name: &str) -> String {
	let stripped = IGNORED_ARTICLES
		.split(' ')
		.find_map(|article| {
			let rest = name.strip_prefix(article)?;
			rest.strip_prefix(' ')
		})
		.unwrap_or(name);
	match stripped.chars().next() {
		Some(c) if c.is_alphabetic() => c.to_uppercase().collect(),
		_ => "#".to_owned(),
	}
}

fn sort_key(name: &str) -> String {
	name.to_lowercase()
}

/// Groups names into index headings, sorted, with `#` first.
pub fn group_by_letter<T>(mut entries: Vec<(String, T)>) -> Vec<(String, Vec<(String, T)>)> {
	entries.sort_by_key(|(name, _)| sort_key(name));
	let mut groups: Vec<(String, Vec<(String, T)>)> = Vec::new();
	for (name, value) in entries {
		let letter = index_letter(&name);
		match groups.iter_mut().find(|(l, _)| *l == letter) {
			Some((_, members)) => members.push((name, value)),
			None => groups.push((letter, vec![(name, value)])),
		}
	}
	groups.sort_by(|(a, _), (b, _)| match (a.as_str(), b.as_str()) {
		("#", "#") => std::cmp::Ordering::Equal,
		("#", _) => std::cmp::Ordering::Less,
		(_, "#") => std::cmp::Ordering::Greater,
		_ => a.to_lowercase().cmp(&b.to_lowercase()),
	});
	groups
}

pub async fn get_indexes(ctx: &Ctx, params: &Params) -> Result<Element, SubsonicError> {
	let selected = selected_folder(ctx, params).await?;
	let last_modified = ctx
		.scanner
		.get_status()
		.await
		.last_end_time
		.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
		.map(|d| d.as_millis() as i64)
		.unwrap_or(0);

	let indexes = Element::new()
		.attr("ignoredArticles", IGNORED_ARTICLES)
		.attr("lastModified", last_modified);

	// Nothing to send if the client already has the current index
	if params
		.int("ifModifiedSince")?
		.is_some_and(|since| last_modified <= since)
	{
		return Ok(Element::new().object("indexes", indexes));
	}

	let folders: Vec<String> = match selected {
		Some((_, name)) => vec![name],
		None => music_folders(ctx)
			.await
			.into_iter()
			.map(|(_, name)| name)
			.collect(),
	};

	let mut directories = Vec::new();
	let mut song_paths = Vec::new();
	for folder in folders {
		for file in ctx
			.index
			.browse(PathBuf::from(folder))
			.await
			.map_err(browse_error)?
		{
			match file {
				File::Directory(path) => directories.push(path),
				File::Song(path) => song_paths.push(path),
			}
		}
	}
	let songs: Vec<_> = ctx
		.index
		.get_songs(song_paths)
		.await
		.into_iter()
		.filter_map(|s| s.ok())
		.collect();

	let mut wanted = Wanted::new();
	for directory in &directories {
		wanted.dir(directory);
	}
	wanted.songs(&songs);
	let mapper = Mapper::load(ctx, wanted).await?;

	let entries = directories
		.iter()
		.map(|path| (last_component(path), path))
		.collect();
	let index = group_by_letter(entries)
		.into_iter()
		.map(|(letter, members)| {
			let artists = members
				.into_iter()
				.map(|(name, path)| {
					Element::new()
						.attr(
							"id",
							mapper.id(ObjectKind::Directory, &model::path_key(path)),
						)
						.attr("name", name)
				})
				.collect();
			Element::new().attr("name", letter).list("artist", artists)
		})
		.collect();

	let indexes = indexes
		.list("index", index)
		.list("child", mapper.songs(&songs).await);
	Ok(Element::new().object("indexes", indexes))
}

// Directories

pub async fn get_music_directory(ctx: &Ctx, params: &Params) -> Result<Element, SubsonicError> {
	let id = params.required("id")?;
	let (kind, key) = ctx.resolve(id).await?;

	let directory = match kind {
		ObjectKind::Directory => {
			let path = PathBuf::from(&key);
			let mut directories = Vec::new();
			let mut song_paths = Vec::new();
			for file in ctx.index.browse(path.clone()).await.map_err(browse_error)? {
				match file {
					File::Directory(p) => directories.push(p),
					File::Song(p) => song_paths.push(p),
				}
			}
			let songs: Vec<_> = ctx
				.index
				.get_songs(song_paths)
				.await
				.into_iter()
				.filter_map(|s| s.ok())
				.collect();

			let mut wanted = Wanted::new();
			wanted.dir(&path);
			for directory in &directories {
				wanted.dir(directory);
			}
			wanted.songs(&songs);
			let mapper = Mapper::load(ctx, wanted).await?;

			let parent = path
				.parent()
				.filter(|p| !p.as_os_str().is_empty())
				.map(|p| mapper.id(ObjectKind::Directory, &model::path_key(p)));
			let mut children: Vec<Element> = directories
				.iter()
				.map(|d| {
					let child_id = mapper.id(ObjectKind::Directory, &model::path_key(d));
					mapper.directory_child(&child_id, Some(id.to_owned()), &last_component(d))
				})
				.collect();
			children.extend(mapper.songs(&songs).await);

			let element = Element::new()
				.attr("id", id)
				.attr_opt("parent", parent)
				.attr("name", last_component(&path));
			mapper_state(&mapper, element, id).list("child", children)
		}
		ObjectKind::Album => {
			let album = load_album(ctx, &key).await?;
			let data = AlbumData::from(&album);
			let mut wanted = Wanted::new();
			wanted.album(&data);
			wanted.songs(&album.songs);
			let mapper = Mapper::load(ctx, wanted).await?;
			let parent = data
				.artists
				.first()
				.map(|a| mapper.id(ObjectKind::Artist, a));
			let element = Element::new()
				.attr("id", id)
				.attr_opt("parent", parent)
				.attr("name", album.header.name.clone());
			mapper_state(&mapper, element, id).list("child", mapper.songs(&album.songs).await)
		}
		ObjectKind::Artist => {
			let artist = ctx
				.index
				.get_artist(key.clone())
				.await
				.map_err(browse_error)?;
			let albums: Vec<AlbumData> = artist.albums.iter().map(AlbumData::from).collect();
			let mut wanted = Wanted::new();
			wanted.artist(&key);
			for album in &albums {
				wanted.album(album);
			}
			let mapper = Mapper::load(ctx, wanted).await?;
			let element = Element::new().attr("id", id).attr("name", key.clone());
			mapper_state(&mapper, element, id).list(
				"child",
				albums.iter().map(|a| mapper.album_child(a)).collect(),
			)
		}
		_ => return Err(SubsonicError::not_found()),
	};
	Ok(Element::new().object("directory", directory))
}

/// Adds the user's star, rating and play count of an object to its element.
fn mapper_state(mapper: &Mapper, element: Element, id: &str) -> Element {
	match mapper.state(id) {
		None => element,
		Some(state) => {
			let mut element = element
				.attr_opt("starred", state.starred_at.map(model::iso8601))
				.attr_opt("userRating", state.rating.map(|r| r as i32));
			if state.play_count > 0 {
				element = element.attr("playCount", state.play_count);
			}
			element
		}
	}
}

// Tag based browsing

pub async fn load_album(ctx: &Ctx, key: &str) -> Result<Album, SubsonicError> {
	let (name, artists) = model::parse_album_key(key).ok_or_else(SubsonicError::not_found)?;
	ctx.index
		.get_album(artists, name)
		.await
		.map_err(browse_error)
}

pub async fn get_genres(ctx: &Ctx, _params: &Params) -> Result<Element, SubsonicError> {
	let genres = ctx
		.index
		.get_genre_summaries()
		.await
		.into_iter()
		.map(|g| {
			Element::new()
				.attr("value", g.name)
				.attr("songCount", g.num_songs)
				.attr("albumCount", g.num_albums)
		})
		.collect();
	Ok(Element::new().object("genres", Element::new().list("genre", genres)))
}

pub async fn get_artists(ctx: &Ctx, params: &Params) -> Result<Element, SubsonicError> {
	selected_folder(ctx, params).await?;
	let artists = ctx.index.get_artists().await;

	let mut wanted = Wanted::new();
	for artist in &artists {
		wanted.artist(&artist.name);
	}
	let mapper = Mapper::load(ctx, wanted).await?;

	let entries = artists
		.iter()
		.map(|a| (a.name.to_string(), a.num_albums_as_performer))
		.collect();
	let index = group_by_letter(entries)
		.into_iter()
		.map(|(letter, members)| {
			let artists = members
				.into_iter()
				.map(|(name, num_albums)| mapper.artist(&name, num_albums))
				.collect();
			Element::new().attr("name", letter).list("artist", artists)
		})
		.collect();
	Ok(Element::new().object(
		"artists",
		Element::new()
			.attr("ignoredArticles", IGNORED_ARTICLES)
			.list("index", index),
	))
}

pub async fn get_artist(ctx: &Ctx, params: &Params) -> Result<Element, SubsonicError> {
	let id = params.required("id")?;
	let name = ctx.resolve_as(id, ObjectKind::Artist).await?;
	let artist = ctx
		.index
		.get_artist(name.clone())
		.await
		.map_err(browse_error)?;

	let albums: Vec<AlbumData> = artist.albums.iter().map(AlbumData::from).collect();
	let mut wanted = Wanted::new();
	wanted.artist(&name);
	for album in &albums {
		wanted.album(album);
	}
	let mapper = Mapper::load(ctx, wanted).await?;

	let element = mapper
		.artist(&name, albums.len() as u32)
		.attr_opt(
			"coverArt",
			albums.iter().any(|a| a.has_artwork).then(|| id.to_owned()),
		)
		.list(
			"album",
			albums.iter().map(|a| mapper.album(a, None)).collect(),
		);
	Ok(Element::new().object("artist", element))
}

pub async fn get_album(ctx: &Ctx, params: &Params) -> Result<Element, SubsonicError> {
	let id = params.required("id")?;
	let key = ctx.resolve_as(id, ObjectKind::Album).await?;
	let album = load_album(ctx, &key).await?;
	let data = AlbumData::from(&album);

	let mut wanted = Wanted::new();
	wanted.album(&data);
	wanted.songs(&album.songs);
	let mapper = Mapper::load(ctx, wanted).await?;

	let songs = mapper.songs(&album.songs).await;
	Ok(Element::new().object("album", mapper.album(&data, Some(songs))))
}

pub async fn get_song(ctx: &Ctx, params: &Params) -> Result<Element, SubsonicError> {
	let id = params.required("id")?;
	let path = ctx.resolve_as(id, ObjectKind::Song).await?;
	let song = ctx
		.index
		.get_songs(vec![PathBuf::from(path)])
		.await
		.into_iter()
		.next()
		.ok_or_else(SubsonicError::not_found)?
		.map_err(browse_error)?;

	let mut wanted = Wanted::new();
	wanted.song(&song);
	let mapper = Mapper::load(ctx, wanted).await?;
	let element = mapper.songs(&[song]).await.remove(0);
	Ok(Element::new().object("song", element))
}

// Information from outside the library. Polaris has none, so these answer with an empty but
// valid description of an object that exists.

pub async fn get_artist_info(
	name: &'static str,
	ctx: &Ctx,
	params: &Params,
) -> Result<Element, SubsonicError> {
	let id = params.required("id")?;
	ctx.resolve_as(id, ObjectKind::Artist).await?;
	Ok(Element::new().object(name, Element::new()))
}

pub async fn get_album_info(ctx: &Ctx, params: &Params) -> Result<Element, SubsonicError> {
	let id = params.required("id")?;
	let (kind, _) = ctx.resolve(id).await?;
	if !matches!(kind, ObjectKind::Album | ObjectKind::Directory) {
		return Err(SubsonicError::not_found());
	}
	Ok(Element::new().object("albumInfo", Element::new()))
}

pub async fn get_similar_songs(
	name: &'static str,
	ctx: &Ctx,
	params: &Params,
) -> Result<Element, SubsonicError> {
	let id = params.required("id")?;
	ctx.resolve(id).await?;
	Ok(Element::new().object(name, Element::new().list("song", vec![])))
}

#[cfg(test)]
mod test {
	use super::*;

	#[test]
	fn index_letters_ignore_articles_and_group_symbols() {
		assert_eq!(index_letter("Khemmis"), "K");
		assert_eq!(index_letter("the cure"), "T");
		assert_eq!(index_letter("The Cure"), "C");
		assert_eq!(index_letter("Les Paul"), "P");
		assert_eq!(index_letter("Theory of a Deadman"), "T");
		assert_eq!(index_letter("The"), "T");
		assert_eq!(index_letter("3 Doors Down"), "#");
		assert_eq!(index_letter("!!!"), "#");
		assert_eq!(index_letter(""), "#");
		assert_eq!(index_letter("épée"), "É");
		assert_eq!(index_letter("ピクニック"), "ピ");
	}

	#[test]
	fn names_are_grouped_and_sorted() {
		let groups = group_by_letter(vec![
			("Zebra".to_owned(), 1),
			("apple".to_owned(), 2),
			("The Beatles".to_owned(), 3),
			("Aardvark".to_owned(), 4),
			("1975".to_owned(), 5),
		]);
		let letters: Vec<&str> = groups.iter().map(|(l, _)| l.as_str()).collect();
		assert_eq!(letters, vec!["#", "A", "B", "Z"]);
		let a: Vec<&str> = groups[1].1.iter().map(|(n, _)| n.as_str()).collect();
		assert_eq!(a, vec!["Aardvark", "apple"]);
	}
}
