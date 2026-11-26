//! The OpenSubsonic API, served at `/rest`.
//!
//! See `docs/OPENSUBSONIC.md`. Every endpoint is reachable as `/rest/<name>` or
//! `/rest/<name>.view`, with GET or (form-encoded) POST. Unless the server setting
//! `opensubsonic_enabled` is on, the whole API answers 404.

use std::time::Duration;

use axum::{
	body::to_bytes,
	extract::{DefaultBodyLimit, Path, Request, State},
	http::{header, Method, StatusCode},
	response::{IntoResponse, Response},
	routing::get,
	Router,
};
use tower_http::cors::{Any, CorsLayer};

use crate::app::{config, state, App};

mod auth;
mod params;
mod response;
mod system;

use auth::Mechanism;
use params::Params;
use response::{is_valid_jsonp_callback, Format, Reply, SubsonicError};

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

async fn handle(
	State(config_manager): State<config::Manager>,
	State(state_manager): State<state::Manager>,
	Path(method): Path<String>,
	request: Request,
) -> Response {
	if !config_manager.get_opensubsonic_enabled().await {
		return StatusCode::NOT_FOUND.into_response();
	}

	let query = request.uri().query().map(str::to_owned);
	let is_post = request.method() == Method::POST;
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
	dispatch(method, &params, format, &config_manager, &state_manager)
		.await
		.into_response()
}

async fn dispatch(
	method: &str,
	params: &Params,
	format: Format,
	config_manager: &config::Manager,
	state_manager: &state::Manager,
) -> Reply {
	// Anyone may ask what the server supports, without credentials.
	if method == "getOpenSubsonicExtensions" {
		return Reply::ok(format, system::get_open_subsonic_extensions());
	}

	if !matches!(method, "ping" | "getLicense" | "tokenInfo") {
		return Reply::error(
			format,
			SubsonicError::generic(format!("Endpoint not supported: {method}")),
		);
	}

	for required in ["v", "c"] {
		if params.get(required).is_none_or(str::is_empty) {
			return Reply::error(format, SubsonicError::missing_parameter(required));
		}
	}
	if let Err(error) = auth::check_client_version(params.get("v").unwrap_or_default()) {
		return Reply::error(format, error);
	}

	let user = match auth::authenticate(params, config_manager, state_manager).await {
		Ok(user) => user,
		Err(error) => return Reply::error(format, error),
	};

	match method {
		"ping" => Reply::ok(format, system::ping()),
		"getLicense" => Reply::ok(format, system::get_license()),
		"tokenInfo" => {
			if user.mechanism == Mechanism::ApiKey {
				Reply::ok(format, system::token_info(&user.username))
			} else {
				Reply::error(
					format,
					SubsonicError::mechanism_unsupported(
						"tokenInfo requires API key authentication",
					),
				)
			}
		}
		_ => unreachable!("method was checked above"),
	}
}
