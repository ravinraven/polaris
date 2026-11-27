//! Media: streaming and downloading songs, and cover art.

use std::path::PathBuf;

use axum::{
	http::{header, HeaderMap, HeaderValue},
	response::{IntoResponse, Response},
};
use axum_extra::headers::{Header, Range};
use axum_range::{KnownSize, Ranged};

use crate::app::{state::ObjectKind, thumbnail};

use super::browse::{browse_error, load_album};
use super::ctx::Ctx;
use super::model::audio_type;
use super::params::Params;
use super::response::SubsonicError;

/// Cover art is never larger than this, whatever a client asks for.
const MAX_COVER_ART_SIZE: i64 = 2048;

fn range_header(headers: &HeaderMap) -> Option<Range> {
	Range::decode(&mut headers.get_all(header::RANGE).iter()).ok()
}

/// A file name that is safe to put in a `Content-Disposition` header.
fn download_name(path: &std::path::Path) -> String {
	let name = path
		.file_name()
		.map(|n| n.to_string_lossy().into_owned())
		.unwrap_or_else(|| "download".to_owned());
	name.chars()
		.map(|c| match c {
			'"' | '\\' | '/' | '\r' | '\n' => '_',
			c if c.is_control() || !c.is_ascii() => '_',
			c => c,
		})
		.collect()
}

async fn song_file(ctx: &Ctx, id: &str) -> Result<(PathBuf, PathBuf), SubsonicError> {
	let virtual_path = PathBuf::from(ctx.resolve_as(id, ObjectKind::Song).await?);
	let real_path = ctx
		.config
		.resolve_virtual_path(&virtual_path)
		.await
		.map_err(|_| SubsonicError::not_found())?;
	Ok((virtual_path, real_path))
}

async fn serve_file(
	path: &std::path::Path,
	content_type: &str,
	headers: &HeaderMap,
) -> Result<Response, SubsonicError> {
	let file = tokio::fs::File::open(path)
		.await
		.map_err(|_| SubsonicError::not_found())?;
	let body = KnownSize::file(file)
		.await
		.map_err(|_| SubsonicError::generic("The file could not be read"))?;
	let mut response = Ranged::new(range_header(headers), body).into_response();
	response.headers_mut().insert(
		header::CONTENT_TYPE,
		HeaderValue::from_str(content_type)
			.unwrap_or(HeaderValue::from_static("application/octet-stream")),
	);
	Ok(response)
}

/// `stream` and `download`: the original file, with support for range requests. Polaris does
/// not transcode, so the format and bit rate a client asks for are ignored.
pub async fn serve_song(
	ctx: &Ctx,
	params: &Params,
	headers: &HeaderMap,
	attachment: bool,
) -> Result<Response, SubsonicError> {
	let id = params.required("id")?;
	let (virtual_path, real_path) = song_file(ctx, id).await?;
	let (content_type, _) = audio_type(&virtual_path);
	let mut response = serve_file(&real_path, content_type, headers).await?;
	if attachment {
		let disposition = format!("attachment; filename=\"{}\"", download_name(&virtual_path));
		if let Ok(value) = HeaderValue::from_str(&disposition) {
			response
				.headers_mut()
				.insert(header::CONTENT_DISPOSITION, value);
		}
	}
	Ok(response)
}

/// The image (a file, or a song with embedded art) that stands for an object.
async fn artwork_of(ctx: &Ctx, id: &str) -> Result<PathBuf, SubsonicError> {
	let (kind, key) = ctx.resolve(id).await?;
	let artwork = match kind {
		ObjectKind::Song => ctx
			.index
			.get_songs(vec![PathBuf::from(&key)])
			.await
			.into_iter()
			.next()
			.and_then(|s| s.ok())
			.and_then(|s| s.artwork),
		ObjectKind::Album => load_album(ctx, &key).await?.header.artwork,
		ObjectKind::Artist => ctx
			.index
			.get_artist(key)
			.await
			.map_err(browse_error)?
			.albums
			.into_iter()
			.find_map(|a| a.header.artwork),
		_ => None,
	};
	artwork.ok_or_else(SubsonicError::not_found)
}

pub async fn get_cover_art(
	ctx: &Ctx,
	params: &Params,
	headers: &HeaderMap,
) -> Result<Response, SubsonicError> {
	let id = params.required("id")?;
	let size = params.int("size")?;
	if size.is_some_and(|s| s <= 0) {
		return Err(SubsonicError::invalid_parameter("size"));
	}

	let virtual_path = artwork_of(ctx, id).await?;
	let image_path = ctx
		.config
		.resolve_virtual_path(&virtual_path)
		.await
		.map_err(|_| SubsonicError::not_found())?;

	let options = thumbnail::Options {
		max_dimension: size.map(|s| s.min(MAX_COVER_ART_SIZE) as u32),
		resize_if_almost_square: true,
		pad_to_square: false,
	};
	let thumbnail_path = ctx
		.thumbnails
		.get_thumbnail(&image_path, &options)
		.await
		.map_err(|_| SubsonicError::not_found())?;

	let mut response = serve_file(&thumbnail_path, "image/jpeg", headers).await?;
	response.headers_mut().insert(
		header::CACHE_CONTROL,
		HeaderValue::from_static("private, max-age=3600"),
	);
	Ok(response)
}

#[cfg(test)]
mod test {
	use super::*;
	use std::path::Path;

	#[test]
	fn download_names_cannot_break_out_of_the_header() {
		assert_eq!(download_name(Path::new("a/b/song.mp3")), "song.mp3");
		assert_eq!(download_name(Path::new("a/\"x\\y\".mp3")), "_x_y_.mp3");
		assert_eq!(download_name(Path::new("a/ピクニック.mp3")), "_____.mp3");
		assert_eq!(download_name(Path::new("")), "download");
	}

	#[test]
	fn the_range_header_is_parsed() {
		let mut headers = HeaderMap::new();
		assert!(range_header(&headers).is_none());
		headers.insert(header::RANGE, HeaderValue::from_static("bytes=0-99"));
		assert!(range_header(&headers).is_some());
		headers.insert(header::RANGE, HeaderValue::from_static("garbage"));
		assert!(range_header(&headers).is_none());
	}
}
