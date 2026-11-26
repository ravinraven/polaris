use http::StatusCode;

use crate::server::dto::{self, Settings};
use crate::server::test::{protocol, ServiceType, TestService};
use crate::test_name;

#[tokio::test]
async fn get_settings_requires_auth() {
	let mut service = ServiceType::new(&test_name!()).await;
	service.complete_initial_setup().await;

	let request = protocol::get_settings();
	let response = service.fetch(&request).await;
	assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn get_settings_requires_admin() {
	let mut service = ServiceType::new(&test_name!()).await;
	service.complete_initial_setup().await;
	service.login().await;

	let request = protocol::get_settings();
	let response = service.fetch(&request).await;
	assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn get_settings_golden_path() {
	let mut service = ServiceType::new(&test_name!()).await;
	service.complete_initial_setup().await;
	service.login_admin().await;

	let request = protocol::get_settings();
	let response = service.fetch_json::<_, dto::Settings>(&request).await;
	assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn put_settings_requires_auth() {
	let mut service = ServiceType::new(&test_name!()).await;
	service.complete_initial_setup().await;
	let request = protocol::put_settings(dto::NewSettings::default());
	let response = service.fetch(&request).await;
	assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn put_settings_requires_admin() {
	let mut service = ServiceType::new(&test_name!()).await;
	service.complete_initial_setup().await;
	service.login().await;
	let request = protocol::put_settings(dto::NewSettings::default());
	let response = service.fetch(&request).await;
	assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn put_settings_golden_path() {
	let mut service = ServiceType::new(&test_name!()).await;
	service.complete_initial_setup().await;
	service.login_admin().await;

	let request = protocol::put_settings(dto::NewSettings {
		album_art_pattern: Some("test_pattern".to_owned()),
		ddns_update_url: Some("http://example.com/".to_owned()),
		opensubsonic_enabled: None,
	});
	let response = service.fetch(&request).await;
	assert_eq!(response.status(), StatusCode::OK);

	let request = protocol::get_settings();
	let response = service.fetch_json::<_, dto::Settings>(&request).await;
	let settings = response.body();
	assert_eq!(
		settings,
		&Settings {
			album_art_pattern: "test_pattern".to_owned(),
			ddns_update_url: "http://example.com/".to_owned(),
			opensubsonic_enabled: false,
		},
	);
}

#[tokio::test]
async fn opensubsonic_is_disabled_by_default_and_can_be_toggled() {
	let mut service = ServiceType::new(&test_name!()).await;
	service.complete_initial_setup().await;
	service.login_admin().await;

	let response = service
		.fetch_json::<_, dto::Settings>(&protocol::get_settings())
		.await;
	assert!(!response.body().opensubsonic_enabled);

	let enable = |enabled| {
		protocol::put_settings(dto::NewSettings {
			opensubsonic_enabled: Some(enabled),
			..Default::default()
		})
	};
	let response = service.fetch(&enable(true)).await;
	assert_eq!(response.status(), StatusCode::OK);
	let response = service
		.fetch_json::<_, dto::Settings>(&protocol::get_settings())
		.await;
	assert!(response.body().opensubsonic_enabled);

	// Other settings are left alone when the field is absent, and vice versa.
	let response = service
		.fetch(&protocol::put_settings(dto::NewSettings {
			album_art_pattern: Some("other".to_owned()),
			..Default::default()
		}))
		.await;
	assert_eq!(response.status(), StatusCode::OK);
	let response = service
		.fetch_json::<_, dto::Settings>(&protocol::get_settings())
		.await;
	assert!(response.body().opensubsonic_enabled);
	assert_eq!(response.body().album_art_pattern, "other");

	let response = service.fetch(&enable(false)).await;
	assert_eq!(response.status(), StatusCode::OK);
	let response = service
		.fetch_json::<_, dto::Settings>(&protocol::get_settings())
		.await;
	assert!(!response.body().opensubsonic_enabled);
}
