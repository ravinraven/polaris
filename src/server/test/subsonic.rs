//! Contract tests for the OpenSubsonic API (`/rest`).
//!
//! JSON responses are validated against the OpenAPI schemas published by the OpenSubsonic
//! project, which are vendored in `docs/opensubsonic/openapi`.

use std::path::{Path, PathBuf};

use http::{header, Method, Request, StatusCode};
use serde_json::Value;

use crate::server::dto;
use crate::server::test::{protocol, ServiceType, TestService};
use crate::test_name;

mod library;
mod lists;
mod media;
mod playlists;
mod server;

const SPEC_DIR: &str = "docs/opensubsonic/openapi";
const VERSION: &str = "1.16.1";

// Schema validation

/// Loads a spec file with every `$ref` to another file replaced by that file's content, which
/// gives a self-contained JSON schema.
fn load_schema(relative_path: &str) -> Value {
	fn inline(value: Value, dir: &Path, depth: usize) -> Value {
		assert!(depth < 40, "schema references are nested too deeply");
		match value {
			Value::Object(map) => {
				if let Some(Value::String(reference)) = map.get("$ref") {
					assert!(
						!reference.starts_with('#'),
						"unsupported reference {reference}"
					);
					return load(&dir.join(reference), depth + 1);
				}
				Value::Object(
					map.into_iter()
						.map(|(k, v)| (k, inline(v, dir, depth)))
						.collect(),
				)
			}
			Value::Array(items) => {
				Value::Array(items.into_iter().map(|v| inline(v, dir, depth)).collect())
			}
			other => other,
		}
	}

	fn load(path: &Path, depth: usize) -> Value {
		let path = std::fs::canonicalize(path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
		let content = std::fs::read_to_string(&path).unwrap();
		let value: Value = serde_json::from_str(&content).unwrap();
		inline(value, path.parent().unwrap(), depth)
	}

	load(&PathBuf::from(SPEC_DIR).join(relative_path), 0)
}

fn is_valid(schema_path: &str, response: &Value) -> bool {
	let schema = load_schema(schema_path);
	jsonschema::validator_for(&schema)
		.unwrap()
		.is_valid(response)
}

fn assert_matches_schema(schema_path: &str, response: &Value) {
	let schema = load_schema(schema_path);
	let validator = jsonschema::validator_for(&schema).unwrap();
	let errors: Vec<String> = validator
		.iter_errors(response)
		.map(|e| format!("{e} (at {})", e.instance_path()))
		.collect();
	assert!(
		errors.is_empty(),
		"{response}\n\ndoes not match {schema_path}:\n{}",
		errors.join("\n")
	);
	assert!(
		response.get("subsonic-response").is_some(),
		"missing subsonic-response wrapper"
	);
}

#[test]
fn the_schema_check_rejects_invalid_responses() {
	let valid = serde_json::json!({"subsonic-response": {
		"status": "ok", "version": "1.16.1", "type": "polaris",
		"serverVersion": "1", "openSubsonic": true,
	}});
	assert!(is_valid("schemas/SubsonicResponse.json", &valid));

	let mut cases = Vec::new();
	for missing in ["version", "type", "serverVersion", "openSubsonic", "status"] {
		let mut case = valid.clone();
		case["subsonic-response"]
			.as_object_mut()
			.unwrap()
			.remove(missing);
		cases.push((format!("missing {missing}"), case));
	}
	let mut wrong_type = valid.clone();
	wrong_type["subsonic-response"]["openSubsonic"] = "true".into();
	cases.push(("openSubsonic as a string".to_owned(), wrong_type));
	let mut wrong_status = valid.clone();
	wrong_status["subsonic-response"]["status"] = "fine".into();
	cases.push(("unknown status".to_owned(), wrong_status));
	let mut failed_without_error = valid.clone();
	failed_without_error["subsonic-response"]["status"] = "failed".into();
	cases.push(("failed without error".to_owned(), failed_without_error));
	let mut bad_code = valid.clone();
	bad_code["subsonic-response"]["status"] = "failed".into();
	bad_code["subsonic-response"]["error"] = serde_json::json!({"code": 999});
	cases.push(("error code outside the spec".to_owned(), bad_code));

	for (name, case) in cases {
		assert!(
			!is_valid("schemas/SubsonicResponse.json", &case),
			"accepted: {name}"
		);
	}

	// Endpoint-specific payloads are checked too.
	let mut no_extensions = valid.clone();
	no_extensions["subsonic-response"]["openSubsonicExtensions"] =
		serde_json::json!([{"name": "x"}]);
	assert!(!is_valid(
		"endpoints/getOpenSubsonicExtensions/GetOpenSubsonicExtensionsResponse.json",
		&no_extensions
	));
	let mut license = valid.clone();
	license["subsonic-response"]["license"] = serde_json::json!({});
	assert!(!is_valid(
		"endpoints/getLicense/GetLicenseResponse.json",
		&license
	));
}

// Test helpers

fn get(uri: &str) -> Request<()> {
	Request::builder()
		.method(Method::GET)
		.uri(uri)
		.body(())
		.unwrap()
}

/// Sets up a server with the OpenSubsonic API enabled, and returns an API key of a regular user.
async fn enabled_service(test_name: &str) -> (ServiceType, String) {
	let mut service = ServiceType::new(test_name).await;
	service.complete_initial_setup().await;
	service.login_admin().await;
	let response = service
		.fetch(&protocol::put_settings(dto::NewSettings {
			opensubsonic_enabled: Some(true),
			..Default::default()
		}))
		.await;
	assert_eq!(response.status(), StatusCode::OK);

	service.login().await;
	let created = service
		.fetch_json::<_, dto::CreatedApiKey>(&protocol::create_api_key(dto::NewApiKey {
			label: "test".to_owned(),
		}))
		.await
		.into_body();
	service.logout().await;
	(service, created.key)
}

/// Like [`enabled_service`], with the test collection indexed.
async fn library_service(test_name: &str) -> (ServiceType, String) {
	let mut service = ServiceType::new(test_name).await;
	service.complete_initial_setup().await;
	service.login_admin().await;
	let response = service
		.fetch(&protocol::put_settings(dto::NewSettings {
			opensubsonic_enabled: Some(true),
			..Default::default()
		}))
		.await;
	assert_eq!(response.status(), StatusCode::OK);
	service.index().await;

	service.login().await;
	let created = service
		.fetch_json::<_, dto::CreatedApiKey>(&protocol::create_api_key(dto::NewApiKey {
			label: "test".to_owned(),
		}))
		.await
		.into_body();
	service.logout().await;
	(service, created.key)
}

/// The schema describing the response of an endpoint, in the vendored specification.
fn response_schema(endpoint: &str) -> String {
	let mut name = endpoint.to_owned();
	name[..1].make_ascii_uppercase();
	let specific = format!("endpoints/{endpoint}/{name}Response.json");
	// A few endpoints share a response with another one and have no schema of their own.
	if PathBuf::from(SPEC_DIR).join(&specific).exists() {
		specific
	} else {
		"schemas/SubsonicResponse.json".to_owned()
	}
}

/// Calls an endpoint as JSON with the test user's key, and checks the response against the
/// endpoint's schema. Returns the `subsonic-response` object.
async fn call(service: &mut ServiceType, key: &str, endpoint: &str, extra: &str) -> Value {
	let uri = format!("/rest/{endpoint}?{}&{extra}", auth_query(key));
	let reply = fetch(service, &uri).await;
	assert_eq!(reply.status, StatusCode::OK, "{endpoint}");
	let json = reply.json();
	assert_matches_schema(&response_schema(endpoint), &json);
	json["subsonic-response"].clone()
}

async fn call_ok(service: &mut ServiceType, key: &str, endpoint: &str, extra: &str) -> Value {
	let response = call(service, key, endpoint, extra).await;
	assert_eq!(response["status"], "ok", "{endpoint} {extra}: {response}");
	response
}

async fn call_failing(service: &mut ServiceType, key: &str, endpoint: &str, extra: &str) -> u64 {
	let response = call(service, key, endpoint, extra).await;
	assert_eq!(
		response["status"], "failed",
		"{endpoint} {extra}: {response}"
	);
	response["error"]["code"].as_u64().unwrap()
}

struct Reply {
	status: StatusCode,
	content_type: String,
	body: Vec<u8>,
	headers: http::HeaderMap,
}

impl Reply {
	fn text(&self) -> String {
		String::from_utf8(self.body.clone()).unwrap()
	}

	fn json(&self) -> Value {
		assert!(
			self.content_type.starts_with("application/json"),
			"content type was {}",
			self.content_type
		);
		serde_json::from_slice(&self.body).unwrap()
	}
}

async fn request(service: &mut ServiceType, request: Request<()>) -> Reply {
	let (builder, body) = service.send_json(&request).await;
	let response = builder.body(()).unwrap();
	Reply {
		status: response.status(),
		content_type: response
			.headers()
			.get(header::CONTENT_TYPE)
			.map(|v| v.to_str().unwrap().to_owned())
			.unwrap_or_default(),
		headers: response.headers().clone(),
		body: body.map(|b| b.to_vec()).unwrap_or_default(),
	}
}

async fn fetch(service: &mut ServiceType, uri: &str) -> Reply {
	request(service, get(uri)).await
}

fn error_code(response: &Value) -> u64 {
	assert_eq!(
		response["subsonic-response"]["status"], "failed",
		"{response}"
	);
	response["subsonic-response"]["error"]["code"]
		.as_u64()
		.unwrap()
}

fn auth_query(key: &str) -> String {
	format!("apiKey={key}&v={VERSION}&c=polaris-tests&f=json")
}

// Availability

#[tokio::test]
async fn is_disabled_by_default() {
	let mut service = ServiceType::new(&test_name!()).await;
	service.complete_initial_setup().await;
	for uri in [
		"/rest/ping",
		"/rest/ping.view",
		"/rest/getOpenSubsonicExtensions",
		"/rest/getLicense?f=json",
	] {
		let reply = fetch(&mut service, uri).await;
		assert_eq!(reply.status, StatusCode::NOT_FOUND, "{uri}");
	}
}

#[tokio::test]
async fn can_be_disabled_again() {
	let (mut service, key) = enabled_service(&test_name!()).await;
	let uri = format!("/rest/ping?{}", auth_query(&key));
	assert_eq!(fetch(&mut service, &uri).await.status, StatusCode::OK);

	service.login_admin().await;
	let response = service
		.fetch(&protocol::put_settings(dto::NewSettings {
			opensubsonic_enabled: Some(false),
			..Default::default()
		}))
		.await;
	assert_eq!(response.status(), StatusCode::OK);
	assert_eq!(
		fetch(&mut service, &uri).await.status,
		StatusCode::NOT_FOUND
	);
}

// Envelope and formats

#[tokio::test]
async fn ping_returns_a_valid_json_envelope() {
	let (mut service, key) = enabled_service(&test_name!()).await;
	let reply = fetch(&mut service, &format!("/rest/ping?{}", auth_query(&key))).await;
	assert_eq!(reply.status, StatusCode::OK);

	let json = reply.json();
	assert_matches_schema("schemas/SubsonicResponse.json", &json);
	let response = &json["subsonic-response"];
	assert_eq!(response["status"], "ok");
	assert_eq!(response["version"], VERSION);
	assert_eq!(response["type"], "polaris");
	assert_eq!(response["openSubsonic"], true);
	assert!(response["serverVersion"].is_string());
}

#[tokio::test]
async fn xml_is_the_default_format() {
	let (mut service, key) = enabled_service(&test_name!()).await;
	let reply = fetch(
		&mut service,
		&format!("/rest/ping?apiKey={key}&v={VERSION}&c=polaris-tests"),
	)
	.await;
	assert_eq!(reply.status, StatusCode::OK);
	assert!(reply.content_type.starts_with("application/xml"));
	let body = reply.text();
	let doc = roxmltree::Document::parse(&body).unwrap();
	let root = doc.root_element();
	assert_eq!(root.tag_name().name(), "subsonic-response");
	assert_eq!(root.attribute("status"), Some("ok"));
	assert_eq!(root.attribute("version"), Some(VERSION));
	assert_eq!(root.attribute("type"), Some("polaris"));
	assert_eq!(root.attribute("openSubsonic"), Some("true"));
}

#[tokio::test]
async fn xml_is_used_for_f_xml_and_unknown_formats() {
	let (mut service, key) = enabled_service(&test_name!()).await;
	for f in ["xml", "nonsense", ""] {
		let uri = format!("/rest/ping?apiKey={key}&v={VERSION}&c=t&f={f}");
		let reply = fetch(&mut service, &uri).await;
		assert!(reply.content_type.starts_with("application/xml"), "f={f}");
	}
}

#[tokio::test]
async fn view_suffix_is_accepted() {
	let (mut service, key) = enabled_service(&test_name!()).await;
	let reply = fetch(
		&mut service,
		&format!("/rest/ping.view?{}", auth_query(&key)),
	)
	.await;
	assert_eq!(reply.status, StatusCode::OK);
	assert_eq!(reply.json()["subsonic-response"]["status"], "ok");
}

#[tokio::test]
async fn jsonp_wraps_the_response_in_the_callback() {
	let (mut service, key) = enabled_service(&test_name!()).await;
	let uri = format!("/rest/ping?apiKey={key}&v={VERSION}&c=t&f=jsonp&callback=handle_ping");
	let reply = fetch(&mut service, &uri).await;
	assert!(reply.content_type.starts_with("application/javascript"));
	let body = reply.text();
	assert!(body.starts_with("handle_ping({"), "{body}");
	assert!(body.ends_with("});"), "{body}");
}

#[tokio::test]
async fn jsonp_needs_a_safe_callback() {
	let (mut service, key) = enabled_service(&test_name!()).await;
	let base = format!("/rest/ping?apiKey={key}&v={VERSION}&c=t&f=jsonp");

	let reply = fetch(&mut service, &base).await;
	assert_eq!(error_code(&reply.json()), 10);

	for callback in ["alert(1)", "a%20b", "%3Cscript%3E", "1abc", "a;b"] {
		let reply = fetch(&mut service, &format!("{base}&callback={callback}")).await;
		assert!(
			reply.content_type.starts_with("application/json"),
			"{callback}"
		);
		assert_eq!(error_code(&reply.json()), 0, "{callback}");
		assert!(!reply.text().contains("alert("), "{callback}");
	}
}

#[tokio::test]
async fn responses_are_never_cached_or_sniffed() {
	let (mut service, key) = enabled_service(&test_name!()).await;
	let reply = fetch(&mut service, &format!("/rest/ping?{}", auth_query(&key))).await;
	assert_eq!(reply.headers[header::CACHE_CONTROL], "no-store");
	assert_eq!(reply.headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
}

// Endpoints

#[tokio::test]
async fn get_open_subsonic_extensions_needs_no_credentials() {
	let (mut service, _) = enabled_service(&test_name!()).await;
	let reply = fetch(&mut service, "/rest/getOpenSubsonicExtensions?f=json").await;
	assert_eq!(reply.status, StatusCode::OK);
	let json = reply.json();
	assert_matches_schema(
		"endpoints/getOpenSubsonicExtensions/GetOpenSubsonicExtensionsResponse.json",
		&json,
	);

	let extensions = json["subsonic-response"]["openSubsonicExtensions"]
		.as_array()
		.unwrap();
	let names: Vec<&str> = extensions
		.iter()
		.map(|e| e["name"].as_str().unwrap())
		.collect();
	assert!(names.contains(&"apiKeyAuthentication"));
	assert!(names.contains(&"formPost"));
	for extension in extensions {
		assert_eq!(extension["versions"], serde_json::json!([1]));
	}
}

#[tokio::test]
async fn get_open_subsonic_extensions_in_xml() {
	let (mut service, _) = enabled_service(&test_name!()).await;
	let reply = fetch(&mut service, "/rest/getOpenSubsonicExtensions").await;
	let body = reply.text();
	let doc = roxmltree::Document::parse(&body).unwrap();
	let extension = doc
		.root_element()
		.children()
		.find(|n| {
			n.has_tag_name("openSubsonicExtensions") && n.attribute("name") == Some("formPost")
		})
		.expect("formPost extension");
	let versions: Vec<_> = extension
		.children()
		.filter(|n| n.has_tag_name("versions"))
		.map(|n| n.text().unwrap())
		.collect();
	assert_eq!(versions, vec!["1"]);
}

#[tokio::test]
async fn get_license_is_valid() {
	let (mut service, key) = enabled_service(&test_name!()).await;
	let reply = fetch(
		&mut service,
		&format!("/rest/getLicense?{}", auth_query(&key)),
	)
	.await;
	let json = reply.json();
	assert_matches_schema("endpoints/getLicense/GetLicenseResponse.json", &json);
	assert_eq!(json["subsonic-response"]["license"]["valid"], true);
}

#[tokio::test]
async fn token_info_returns_the_owner_of_the_key() {
	let (mut service, key) = enabled_service(&test_name!()).await;
	let reply = fetch(
		&mut service,
		&format!("/rest/tokenInfo?{}", auth_query(&key)),
	)
	.await;
	let json = reply.json();
	assert_matches_schema("endpoints/tokenInfo/GetTokenInfoResponse.json", &json);
	assert_eq!(
		json["subsonic-response"]["tokenInfo"]["username"],
		crate::server::test::constants::TEST_USERNAME
	);
}

#[tokio::test]
async fn token_info_is_only_for_api_keys() {
	let (mut service, _) = enabled_service(&test_name!()).await;
	let uri = format!("/rest/tokenInfo?u=test_user&p=test_password&v={VERSION}&c=t&f=json");
	let json = fetch(&mut service, &uri).await.json();
	assert_matches_schema("schemas/SubsonicResponse.json", &json);
	assert_eq!(error_code(&json), 42);
}

#[tokio::test]
async fn unsupported_endpoints_report_an_error() {
	let (mut service, key) = enabled_service(&test_name!()).await;
	for endpoint in ["getVideos", "doesNotExist", "jukeboxControl"] {
		let reply = fetch(
			&mut service,
			&format!("/rest/{endpoint}?{}", auth_query(&key)),
		)
		.await;
		assert_eq!(reply.status, StatusCode::OK);
		let json = reply.json();
		assert_matches_schema("schemas/SubsonicResponse.json", &json);
		assert_eq!(error_code(&json), 0, "{endpoint}");
	}
}

// Authentication

#[tokio::test]
async fn api_key_errors() {
	let (mut service, key) = enabled_service(&test_name!()).await;
	let base = format!("v={VERSION}&c=t&f=json");

	// Unknown key
	let json = fetch(&mut service, &format!("/rest/ping?apiKey=nope&{base}"))
		.await
		.json();
	assert_matches_schema("schemas/SubsonicResponse.json", &json);
	assert_eq!(error_code(&json), 44);
	let json = fetch(&mut service, &format!("/rest/ping?apiKey=&{base}"))
		.await
		.json();
	assert_eq!(error_code(&json), 44);

	// A user's password is not a valid API key
	let json = fetch(
		&mut service,
		&format!("/rest/ping?apiKey=test_password&{base}"),
	)
	.await
	.json();
	assert_eq!(error_code(&json), 44);

	// An API key cannot be combined with other credentials
	for extra in [
		"u=test_user",
		"p=x",
		"t=abc",
		"s=abc",
		"u=test_user&p=test_password",
	] {
		let uri = format!("/rest/ping?apiKey={key}&{extra}&{base}");
		let json = fetch(&mut service, &uri).await.json();
		assert_eq!(error_code(&json), 43, "{extra}");
	}

	// The same credential twice is ambiguous
	let uri = format!("/rest/ping?apiKey={key}&apiKey={key}&{base}");
	assert_eq!(error_code(&fetch(&mut service, &uri).await.json()), 43);
}

#[tokio::test]
async fn revoked_keys_stop_working_immediately() {
	let (mut service, key) = enabled_service(&test_name!()).await;
	let uri = format!("/rest/ping?{}", auth_query(&key));
	assert_eq!(
		fetch(&mut service, &uri).await.json()["subsonic-response"]["status"],
		"ok"
	);

	service.login().await;
	let keys = service
		.fetch_json::<_, Vec<dto::ApiKey>>(&protocol::list_api_keys())
		.await
		.into_body();
	let response = service.fetch(&protocol::delete_api_key(&keys[0].id)).await;
	assert_eq!(response.status(), StatusCode::OK);
	service.logout().await;

	assert_eq!(error_code(&fetch(&mut service, &uri).await.json()), 44);
}

#[tokio::test]
async fn keys_of_deleted_users_stop_working() {
	let (mut service, key) = enabled_service(&test_name!()).await;
	let uri = format!("/rest/ping?{}", auth_query(&key));
	assert_eq!(
		fetch(&mut service, &uri).await.json()["subsonic-response"]["status"],
		"ok"
	);

	service.login_admin().await;
	let response = service
		.fetch(&protocol::delete_user(
			crate::server::test::constants::TEST_USERNAME,
		))
		.await;
	assert_eq!(response.status(), StatusCode::OK);
	service.logout().await;

	assert_eq!(error_code(&fetch(&mut service, &uri).await.json()), 44);
}

#[tokio::test]
async fn password_authentication() {
	let (mut service, _) = enabled_service(&test_name!()).await;
	let base = format!("v={VERSION}&c=t&f=json");

	let json = fetch(
		&mut service,
		&format!("/rest/ping?u=test_user&p=test_password&{base}"),
	)
	.await
	.json();
	assert_matches_schema("schemas/SubsonicResponse.json", &json);
	assert_eq!(json["subsonic-response"]["status"], "ok");

	// Hex encoded, as clients may send it ("test_password")
	let hex: String = "test_password"
		.bytes()
		.map(|b| format!("{b:02x}"))
		.collect();
	let json = fetch(
		&mut service,
		&format!("/rest/ping?u=test_user&p=enc:{hex}&{base}"),
	)
	.await
	.json();
	assert_eq!(json["subsonic-response"]["status"], "ok");

	for (query, code) in [
		("u=test_user&p=wrong", 40),
		("u=nobody&p=test_password", 40),
		("u=test_user&p=enc:zz", 40),
		("u=test_user", 10),
		("p=test_password", 10),
		("", 10),
	] {
		let json = fetch(&mut service, &format!("/rest/ping?{query}&{base}"))
			.await
			.json();
		assert_eq!(error_code(&json), code, "{query}");
	}
}

#[tokio::test]
async fn token_authentication_is_not_supported() {
	let (mut service, _) = enabled_service(&test_name!()).await;
	let base = format!("v={VERSION}&c=t&f=json");
	let json = fetch(
		&mut service,
		&format!("/rest/ping?u=test_user&t=abc&s=def&{base}"),
	)
	.await
	.json();
	assert_matches_schema("schemas/SubsonicResponse.json", &json);
	assert_eq!(error_code(&json), 41);

	// Mixed with a password it is ambiguous instead
	let json = fetch(
		&mut service,
		&format!("/rest/ping?u=test_user&p=x&t=abc&s=def&{base}"),
	)
	.await
	.json();
	assert_eq!(error_code(&json), 43);
}

#[tokio::test]
async fn version_and_client_are_required_and_checked() {
	let (mut service, key) = enabled_service(&test_name!()).await;

	for (query, code) in [
		(format!("apiKey={key}&c=t&f=json"), 10),
		(format!("apiKey={key}&v={VERSION}&f=json"), 10),
		(format!("apiKey={key}&v=&c=t&f=json"), 10),
		(format!("apiKey={key}&v={VERSION}&c=&f=json"), 10),
		(format!("apiKey={key}&v=0.9.0&c=t&f=json"), 20),
		(format!("apiKey={key}&v=2.0.0&c=t&f=json"), 30),
		(format!("apiKey={key}&v=1.17.0&c=t&f=json"), 30),
		(format!("apiKey={key}&v=banana&c=t&f=json"), 0),
	] {
		let json = fetch(&mut service, &format!("/rest/ping?{query}"))
			.await
			.json();
		assert_matches_schema("schemas/SubsonicResponse.json", &json);
		assert_eq!(error_code(&json), code, "{query}");
	}

	for v in ["1.16.1", "1.16.0", "1.15.0", "1.1.0"] {
		let json = fetch(
			&mut service,
			&format!("/rest/ping?apiKey={key}&v={v}&c=t&f=json"),
		)
		.await
		.json();
		assert_eq!(json["subsonic-response"]["status"], "ok", "v={v}");
	}
}

#[tokio::test]
async fn errors_follow_the_requested_format() {
	let (mut service, _) = enabled_service(&test_name!()).await;
	let reply = fetch(
		&mut service,
		&format!("/rest/ping?apiKey=nope&v={VERSION}&c=t"),
	)
	.await;
	assert_eq!(reply.status, StatusCode::OK);
	assert!(reply.content_type.starts_with("application/xml"));
	let body = reply.text();
	let doc = roxmltree::Document::parse(&body).unwrap();
	assert_eq!(doc.root_element().attribute("status"), Some("failed"));
	let error = doc
		.root_element()
		.children()
		.find(|n| n.has_tag_name("error"))
		.unwrap();
	assert_eq!(error.attribute("code"), Some("44"));
}

// POST (the formPost extension)

async fn post_form(service: &mut ServiceType, uri: &str, content_type: &str, body: &str) -> Reply {
	let request = Request::builder()
		.method(Method::POST)
		.uri(uri)
		.header(header::CONTENT_TYPE, content_type)
		.body(body.as_bytes().to_vec())
		.unwrap();
	let (builder, body) = service.send_binary(&request).await;
	let response = builder.body(()).unwrap();
	Reply {
		status: response.status(),
		content_type: response
			.headers()
			.get(header::CONTENT_TYPE)
			.map(|v| v.to_str().unwrap().to_owned())
			.unwrap_or_default(),
		headers: response.headers().clone(),
		body: body.map(|b| b.to_vec()).unwrap_or_default(),
	}
}

#[tokio::test]
async fn form_post_is_supported() {
	let (mut service, key) = enabled_service(&test_name!()).await;
	let form = "application/x-www-form-urlencoded";

	let reply = post_form(&mut service, "/rest/ping", form, &auth_query(&key)).await;
	assert_eq!(reply.status, StatusCode::OK);
	assert_eq!(reply.json()["subsonic-response"]["status"], "ok");

	// Parameters may be split between the query string and the body
	let reply = post_form(
		&mut service,
		&format!("/rest/ping.view?apiKey={key}"),
		"application/x-www-form-urlencoded; charset=UTF-8",
		&format!("v={VERSION}&c=t&f=json"),
	)
	.await;
	assert_eq!(reply.json()["subsonic-response"]["status"], "ok");

	// Credentials in the body are checked like any others
	let reply = post_form(
		&mut service,
		"/rest/ping",
		form,
		&format!("apiKey=bad&v={VERSION}&c=t&f=json"),
	)
	.await;
	assert_eq!(error_code(&reply.json()), 44);
}

#[tokio::test]
async fn form_post_rejects_other_content_types_and_huge_bodies() {
	let (mut service, key) = enabled_service(&test_name!()).await;

	let reply = post_form(
		&mut service,
		"/rest/ping",
		"application/json",
		&auth_query(&key),
	)
	.await;
	assert_eq!(reply.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
	let reply = post_form(&mut service, "/rest/ping", "text/plain", &auth_query(&key)).await;
	assert_eq!(reply.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);

	let huge = format!("{}&junk={}", auth_query(&key), "x".repeat(70 * 1024));
	let reply = post_form(
		&mut service,
		"/rest/ping",
		"application/x-www-form-urlencoded",
		&huge,
	)
	.await;
	assert_eq!(reply.status, StatusCode::PAYLOAD_TOO_LARGE);
}

// Browser access

#[tokio::test]
async fn can_be_called_from_other_origins() {
	let (mut service, key) = enabled_service(&test_name!()).await;

	let reply = request(
		&mut service,
		Request::builder()
			.method(Method::GET)
			.uri(format!("/rest/ping?{}", auth_query(&key)))
			.header(header::ORIGIN, "https://music.example.com")
			.body(())
			.unwrap(),
	)
	.await;
	assert_eq!(reply.headers[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");

	let preflight = request(
		&mut service,
		Request::builder()
			.method(Method::OPTIONS)
			.uri("/rest/ping")
			.header(header::ORIGIN, "https://music.example.com")
			.header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
			.header(header::ACCESS_CONTROL_REQUEST_HEADERS, "content-type")
			.body(())
			.unwrap(),
	)
	.await;
	assert!(preflight.status.is_success(), "{}", preflight.status);
	assert_eq!(preflight.headers[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
	let methods = preflight.headers[header::ACCESS_CONTROL_ALLOW_METHODS]
		.to_str()
		.unwrap()
		.to_ascii_uppercase();
	assert!(
		methods.contains("POST") && methods.contains("GET"),
		"{methods}"
	);
}

#[tokio::test]
async fn the_regular_api_is_not_affected() {
	let (mut service, _) = enabled_service(&test_name!()).await;
	service.login().await;
	let response = service.fetch(&protocol::list_api_keys()).await;
	assert_eq!(response.status(), StatusCode::OK);
	// API keys do not work as credentials for the regular API.
	service.logout().await;
	let response = service.fetch(&protocol::list_api_keys()).await;
	assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
