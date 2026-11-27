//! The OpenSubsonic API, served at `/rest`.
//!
//! See `docs/OPENSUBSONIC.md`. Every endpoint is reachable as `/rest/<name>` or
//! `/rest/<name>.view`, with GET or (form-encoded) POST. Unless the server setting
//! `opensubsonic_enabled` is on, the whole API answers 404.

use std::time::Duration;

use axum::{
	body::to_bytes,
	extract::{DefaultBodyLimit, Path, Request, State},
	http::{header, HeaderMap, Method, StatusCode},
	response::{IntoResponse, Response},
	routing::get,
	Router,
};
use tower_http::cors::{Any, CorsLayer};

use crate::app::App;

mod auth;
mod browse;
mod ctx;
mod lists;
mod model;
mod params;
mod response;
mod system;

use auth::Mechanism;
use ctx::Ctx;
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
const SUPPORTED: &[&str] = &[
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
];

async fn handle(State(app): State<App>, Path(method): Path<String>, request: Request) -> Response {
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
	dispatch(&app, method, &params, &headers, format).await
}

async fn dispatch(
	app: &App,
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

	let user = match auth::authenticate(params, &app.config_manager, &app.state_manager).await {
		Ok(user) => user,
		Err(error) => return Reply::error(format, error).into_response(),
	};
	let ctx = Ctx::new(app, &user);

	let _ = headers;
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

		_ => Err(SubsonicError::generic(format!(
			"Endpoint not supported: {method}"
		))),
	}
}
