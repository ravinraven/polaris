//! The OpenSubsonic API, served at `/rest`.
//!
//! See `docs/OPENSUBSONIC.md`. Every endpoint is reachable as `/rest/<name>` or
//! `/rest/<name>.view`, with GET or (form-encoded) POST. Unless the server setting
//! `opensubsonic_enabled` is on, the whole API answers 404.

use std::{sync::Arc, time::Duration};

use axum::{
	body::to_bytes,
	extract::{DefaultBodyLimit, Path, Request, State},
	http::{header, HeaderMap, Method, StatusCode},
	response::{IntoResponse, Response},
	routing::get,
	Extension, Router,
};
use tower_http::cors::{Any, CorsLayer};

use crate::app::App;

mod admin;
mod annotation;
mod auth;
mod browse;
mod ctx;
mod guard;
mod lists;
mod lyrics;
mod media;
mod model;
mod params;
mod playlists;
mod queue;
mod response;
mod system;
mod tags;

use ctx::Ctx;
use guard::Guard;
use params::Params;
use response::{is_valid_jsonp_callback, Element, Format, Reply, SubsonicError};

/// Requests are small: this is only a limit on what a form POST may carry.
const MAX_BODY_BYTES: usize = 64 * 1024;

pub fn router() -> Router<App> {
	// Third-party web clients live on other origins. Requests carry their own credentials
	// (never cookies), so any origin may call the API.
	let cors = CorsLayer::new()
		.allow_origin(Any)
		.allow_methods([Method::GET, Method::POST])
		.allow_headers([header::CONTENT_TYPE])
		.max_age(Duration::from_secs(24 * 60 * 60));

	Router::new()
		.route("/{method}", get(handle).post(handle))
		.layer(Extension(Arc::new(Guard::default())))
		.layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
		.layer(cors)
}

/// Picks the response format from `f`. A JSONP request without a usable callback is answered
/// in JSON, with an error.
fn select_format(params: &Params) -> (Format, Option<SubsonicError>) {
	match params.get("f") {
		Some("json") => (Format::Json, None),
		Some("jsonp") => match params.get("callback") {
			None => (
				Format::Json,
				Some(SubsonicError::missing_parameter("callback")),
			),
			Some(callback) if is_valid_jsonp_callback(callback) => {
				(Format::Jsonp(callback.to_owned()), None)
			}
			Some(_) => (
				Format::Json,
				Some(SubsonicError::generic("Invalid callback parameter")),
			),
		},
		// XML is the default, also for values we do not know.
		_ => (Format::Xml, None),
	}
}

fn is_form_content_type(request: &Request) -> bool {
	request
		.headers()
		.get(header::CONTENT_TYPE)
		.and_then(|v| v.to_str().ok())
		.is_some_and(|v| {
			v.trim()
				.to_ascii_lowercase()
				.starts_with("application/x-www-form-urlencoded")
		})
}

/// Every endpoint Polaris implements.
pub(crate) const SUPPORTED: &[&str] = &[
	// System
	"ping",
	"getLicense",
	"getOpenSubsonicExtensions",
	"tokenInfo",
	// Browsing
	"getMusicFolders",
	"getIndexes",
	"getMusicDirectory",
	"getGenres",
	"getArtists",
	"getArtist",
	"getAlbum",
	"getSong",
	"getArtistInfo",
	"getArtistInfo2",
	"getAlbumInfo",
	"getAlbumInfo2",
	"getSimilarSongs",
	"getSimilarSongs2",
	// Lists and search
	"getAlbumList",
	"getAlbumList2",
	"getRandomSongs",
	"getSongsByGenre",
	"search2",
	"search3",
	// Media
	"stream",
	"download",
	"getCoverArt",
	"getAvatar",
	// Playlists
	"getPlaylists",
	"getPlaylist",
	"createPlaylist",
	"updatePlaylist",
	"deletePlaylist",
	// Users and scanning
	"getUser",
	"getUsers",
	"getScanStatus",
	"startScan",
	// What users do with music
	"star",
	"unstar",
	"setRating",
	"scrobble",
	"getNowPlaying",
	"reportPlayback",
	"getStarred",
	"getStarred2",
	"getTopSongs",
	// Bookmarks and the play queue
	"getBookmarks",
	"createBookmark",
	"deleteBookmark",
	"getPlayQueue",
	"savePlayQueue",
	"getPlayQueueByIndex",
	"savePlayQueueByIndex",
	// Lyrics
	"getLyricsBySongId",
	"getLyrics",
];

async fn handle(
	State(app): State<App>,
	Extension(guard): Extension<Arc<Guard>>,
	Path(method): Path<String>,
	request: Request,
) -> Response {
	if !app.config_manager.get_opensubsonic_enabled().await {
		return StatusCode::NOT_FOUND.into_response();
	}

	let query = request.uri().query().map(str::to_owned);
	let is_post = request.method() == Method::POST;
	let headers = request.headers().clone();
	let body = if is_post {
		if !is_form_content_type(&request) {
			return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
		}
		match to_bytes(request.into_body(), MAX_BODY_BYTES).await {
			Ok(body) => Some(body),
			Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
		}
	} else {
		None
	};

	let params = Params::parse(query.as_deref(), body.as_deref());
	let (format, format_error) = select_format(&params);
	if let Some(error) = format_error {
		return Reply::error(format, error).into_response();
	}

	let method = method.strip_suffix(".view").unwrap_or(&method);
	dispatch(&app, &guard, method, &params, &headers, format).await
}

async fn dispatch(
	app: &App,
	guard: &Guard,
	method: &str,
	params: &Params,
	headers: &HeaderMap,
	format: Format,
) -> Response {
	// Anyone may ask what the server supports, without credentials.
	if method == "getOpenSubsonicExtensions" {
		return Reply::ok(format, system::get_open_subsonic_extensions()).into_response();
	}

	if !SUPPORTED.contains(&method) {
		return Reply::error(
			format,
			SubsonicError::generic(format!("Endpoint not supported: {method}")),
		)
		.into_response();
	}

	for required in ["v", "c"] {
		if params.get(required).is_none_or(str::is_empty) {
			return Reply::error(format, SubsonicError::missing_parameter(required))
				.into_response();
		}
	}
	if let Err(error) = auth::check_client_version(params.get("v").unwrap_or_default()) {
		return Reply::error(format, error).into_response();
	}

	let user =
		match auth::authenticate(params, &app.config_manager, &app.state_manager, guard).await {
			Ok(user) => user,
			Err(error) => return Reply::error(format, error).into_response(),
		};
	let ctx = Ctx::new(app, &user);

	// Endpoints that answer with a file instead of a description
	let file = match method {
		"stream" => Some(media::serve_song(&ctx, params, headers, false).await),
		"download" => Some(media::serve_song(&ctx, params, headers, true).await),
		"getCoverArt" => Some(media::get_cover_art(&ctx, params, headers).await),
		// Polaris has no user avatars
		"getAvatar" => Some(Err(SubsonicError::not_found())),
		_ => None,
	};
	if let Some(file) = file {
		return match file {
			Ok(response) => response,
			Err(error) => Reply::error(format, error).into_response(),
		};
	}

	match call(method, &ctx, params).await {
		Ok(element) => Reply::ok(format, element).into_response(),
		Err(error) => Reply::error(format, error).into_response(),
	}
}

async fn call(method: &str, ctx: &Ctx, params: &Params) -> Result<Element, SubsonicError> {
	match method {
		"ping" => Ok(system::ping()),
		"getLicense" => Ok(system::get_license()),
		"tokenInfo" => system::token_info(ctx),

		"getMusicFolders" => browse::get_music_folders(ctx, params).await,
		"getIndexes" => browse::get_indexes(ctx, params).await,
		"getMusicDirectory" => browse::get_music_directory(ctx, params).await,
		"getGenres" => browse::get_genres(ctx, params).await,
		"getArtists" => browse::get_artists(ctx, params).await,
		"getArtist" => browse::get_artist(ctx, params).await,
		"getAlbum" => browse::get_album(ctx, params).await,
		"getSong" => browse::get_song(ctx, params).await,
		"getArtistInfo" => browse::get_artist_info("artistInfo", ctx, params).await,
		"getArtistInfo2" => browse::get_artist_info("artistInfo2", ctx, params).await,
		"getAlbumInfo" | "getAlbumInfo2" => browse::get_album_info(ctx, params).await,
		"getSimilarSongs" => browse::get_similar_songs("similarSongs", ctx, params).await,
		"getSimilarSongs2" => browse::get_similar_songs("similarSongs2", ctx, params).await,

		"getAlbumList" => lists::get_album_list(ctx, params).await,
		"getAlbumList2" => lists::get_album_list2(ctx, params).await,
		"getRandomSongs" => lists::get_random_songs(ctx, params).await,
		"getSongsByGenre" => lists::get_songs_by_genre(ctx, params).await,
		"search2" => lists::search2(ctx, params).await,
		"search3" => lists::search3(ctx, params).await,

		"getPlaylists" => playlists::get_playlists(ctx, params).await,
		"getPlaylist" => playlists::get_playlist(ctx, params).await,
		"createPlaylist" => playlists::create_playlist(ctx, params).await,
		"updatePlaylist" => playlists::update_playlist(ctx, params).await,
		"deletePlaylist" => playlists::delete_playlist(ctx, params).await,

		"getUser" => admin::get_user(ctx, params).await,
		"getUsers" => admin::get_users(ctx, params).await,
		"getScanStatus" => admin::get_scan_status(ctx, params).await,
		"startScan" => admin::start_scan(ctx, params).await,

		"star" => annotation::star(ctx, params).await,
		"unstar" => annotation::unstar(ctx, params).await,
		"setRating" => annotation::set_rating(ctx, params).await,
		"scrobble" => annotation::scrobble(ctx, params).await,
		"getNowPlaying" => annotation::get_now_playing(ctx, params).await,
		"reportPlayback" => annotation::report_playback(ctx, params).await,
		"getStarred" => annotation::get_starred(ctx, params).await,
		"getStarred2" => annotation::get_starred2(ctx, params).await,
		"getTopSongs" => annotation::get_top_songs(ctx, params).await,

		"getBookmarks" => queue::get_bookmarks(ctx, params).await,
		"createBookmark" => queue::create_bookmark(ctx, params).await,
		"deleteBookmark" => queue::delete_bookmark(ctx, params).await,
		"getPlayQueue" => queue::get_play_queue(ctx, params).await,
		"savePlayQueue" => queue::save_play_queue(ctx, params).await,
		"getPlayQueueByIndex" => queue::get_play_queue_by_index(ctx, params).await,
		"savePlayQueueByIndex" => queue::save_play_queue_by_index(ctx, params).await,

		"getLyricsBySongId" => lyrics::get_lyrics_by_song_id(ctx, params).await,
		"getLyrics" => lyrics::get_lyrics(ctx, params).await,

		_ => Err(SubsonicError::generic(format!(
			"Endpoint not supported: {method}"
		))),
	}
}

#[cfg(test)]
mod test {
	use super::*;

	/// The endpoints of the vendored specification (one file each).
	fn specified_endpoints() -> std::collections::HashSet<String> {
		std::fs::read_dir("docs/opensubsonic/openapi/endpoints")
			.unwrap()
			.filter_map(|e| e.ok())
			.filter_map(|e| {
				let path = e.path();
				(path.extension()? == "json")
					.then(|| path.file_stem()?.to_str().map(str::to_owned))?
			})
			.collect()
	}

	#[test]
	fn every_supported_endpoint_is_in_the_specification_and_listed_once() {
		let specified = specified_endpoints();
		assert!(specified.len() > 80, "the specification could not be read");
		let mut seen = std::collections::HashSet::new();
		for endpoint in SUPPORTED {
			assert!(
				specified.contains(*endpoint),
				"{endpoint} is not in the specification"
			);
			assert!(seen.insert(*endpoint), "{endpoint} is listed twice");
		}
	}
}
