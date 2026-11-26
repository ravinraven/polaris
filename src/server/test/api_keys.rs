use http::StatusCode;

use crate::server::dto;
use crate::server::test::{protocol, ServiceType, TestService};
use crate::test_name;

fn new_key(label: &str) -> dto::NewApiKey {
	dto::NewApiKey {
		label: label.to_owned(),
	}
}

#[tokio::test]
async fn api_key_endpoints_require_auth() {
	let mut service = ServiceType::new(&test_name!()).await;
	service.complete_initial_setup().await;

	let response = service.fetch(&protocol::list_api_keys()).await;
	assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
	let response = service
		.fetch(&protocol::create_api_key(new_key("phone")))
		.await;
	assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
	let response = service.fetch(&protocol::delete_api_key("abcdef")).await;
	assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn create_list_and_revoke_golden_path() {
	let mut service = ServiceType::new(&test_name!()).await;
	service.complete_initial_setup().await;
	service.login().await;

	let response = service
		.fetch_json::<_, Vec<dto::ApiKey>>(&protocol::list_api_keys())
		.await;
	assert_eq!(response.status(), StatusCode::OK);
	assert!(response.body().is_empty());

	let response = service
		.fetch_json::<_, dto::CreatedApiKey>(&protocol::create_api_key(new_key("  My phone  ")))
		.await;
	assert_eq!(response.status(), StatusCode::OK);
	let created = response.into_body();
	assert_eq!(created.key.len(), 64);
	assert_eq!(created.api_key.label, "My phone");
	assert_eq!(created.api_key.last_used_at, None);

	let response = service
		.fetch_json::<_, Vec<dto::ApiKey>>(&protocol::list_api_keys())
		.await;
	assert_eq!(response.body(), &vec![created.api_key.clone()]);

	let response = service
		.fetch(&protocol::delete_api_key(&created.api_key.id))
		.await;
	assert_eq!(response.status(), StatusCode::OK);

	let response = service
		.fetch_json::<_, Vec<dto::ApiKey>>(&protocol::list_api_keys())
		.await;
	assert!(response.body().is_empty());
}

#[tokio::test]
async fn listing_never_reveals_the_key() {
	let mut service = ServiceType::new(&test_name!()).await;
	service.complete_initial_setup().await;
	service.login().await;

	let created = service
		.fetch_json::<_, dto::CreatedApiKey>(&protocol::create_api_key(new_key("phone")))
		.await
		.into_body();

	let response = service.fetch_bytes(&protocol::list_api_keys()).await;
	assert_eq!(response.status(), StatusCode::OK);
	let body = String::from_utf8(response.into_body()).unwrap();
	assert!(!body.contains(&created.key));
	assert!(!body.contains("\"key\""));
}

#[tokio::test]
async fn keys_belong_to_one_user() {
	let mut service = ServiceType::new(&test_name!()).await;
	service.complete_initial_setup().await;

	service.login().await;
	let created = service
		.fetch_json::<_, dto::CreatedApiKey>(&protocol::create_api_key(new_key("phone")))
		.await
		.into_body();

	service.login_admin().await;
	let response = service
		.fetch_json::<_, Vec<dto::ApiKey>>(&protocol::list_api_keys())
		.await;
	assert!(response.body().is_empty());

	// Another user (even an administrator) cannot revoke it.
	let response = service
		.fetch(&protocol::delete_api_key(&created.api_key.id))
		.await;
	assert_eq!(response.status(), StatusCode::NOT_FOUND);

	service.login().await;
	let response = service
		.fetch_json::<_, Vec<dto::ApiKey>>(&protocol::list_api_keys())
		.await;
	assert_eq!(response.body().len(), 1);
}

#[tokio::test]
async fn revoking_unknown_or_partial_ids_is_not_found() {
	let mut service = ServiceType::new(&test_name!()).await;
	service.complete_initial_setup().await;
	service.login().await;

	let created = service
		.fetch_json::<_, dto::CreatedApiKey>(&protocol::create_api_key(new_key("phone")))
		.await
		.into_body();

	for id in ["unknown", "a", &created.api_key.id[..4]] {
		let response = service.fetch(&protocol::delete_api_key(id)).await;
		assert_eq!(response.status(), StatusCode::NOT_FOUND, "id: {id}");
	}

	let response = service
		.fetch_json::<_, Vec<dto::ApiKey>>(&protocol::list_api_keys())
		.await;
	assert_eq!(response.body().len(), 1);
}

#[tokio::test]
async fn labels_are_validated() {
	let mut service = ServiceType::new(&test_name!()).await;
	service.complete_initial_setup().await;
	service.login().await;

	for label in ["", "   ", &"x".repeat(101)] {
		let response = service
			.fetch(&protocol::create_api_key(new_key(label)))
			.await;
		assert_eq!(response.status(), StatusCode::BAD_REQUEST);
	}

	let response = service
		.fetch(&protocol::create_api_key(new_key(&"x".repeat(100))))
		.await;
	assert_eq!(response.status(), StatusCode::OK);
	let response = service
		.fetch(&protocol::create_api_key(new_key("ünï cödé 🎧")))
		.await;
	assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn number_of_keys_per_user_is_limited() {
	let mut service = ServiceType::new(&test_name!()).await;
	service.complete_initial_setup().await;
	service.login().await;

	for i in 0..100 {
		let response = service
			.fetch(&protocol::create_api_key(new_key(&format!("key {i}"))))
			.await;
		assert_eq!(response.status(), StatusCode::OK, "creating key {i}");
	}
	let response = service
		.fetch(&protocol::create_api_key(new_key("one too many")))
		.await;
	assert_eq!(response.status(), StatusCode::BAD_REQUEST);

	// Revoking one makes room again.
	let listed = service
		.fetch_json::<_, Vec<dto::ApiKey>>(&protocol::list_api_keys())
		.await
		.into_body();
	let response = service
		.fetch(&protocol::delete_api_key(&listed[0].id))
		.await;
	assert_eq!(response.status(), StatusCode::OK);
	let response = service
		.fetch(&protocol::create_api_key(new_key("fits now")))
		.await;
	assert_eq!(response.status(), StatusCode::OK);
}
