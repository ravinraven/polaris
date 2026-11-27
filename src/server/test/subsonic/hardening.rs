//! Limits on failed sign-ins, and the memory of successful ones.

use super::*;

fn password_uri(user: &str, password: &str) -> String {
	format!("/rest/ping?u={user}&p={password}&v={VERSION}&c=t&f=json")
}

async fn fail_times(service: &mut ServiceType, user: &str, times: usize) {
	for attempt in 0..times {
		let reply = fetch(service, &password_uri(user, "wrong")).await;
		assert_eq!(reply.status, StatusCode::OK, "attempt {attempt}");
		assert_eq!(error_code(&reply.json()), 40, "attempt {attempt}");
	}
}

#[tokio::test]
async fn too_many_wrong_passwords_lock_the_user_out_for_a_while() {
	let (mut service, key) = library_service(&test_name!()).await;
	fail_times(&mut service, "test_user", 10).await;

	let reply = fetch(&mut service, &password_uri("test_user", "wrong")).await;
	assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS);
	let retry: u64 = reply.headers[header::RETRY_AFTER]
		.to_str()
		.unwrap()
		.parse()
		.unwrap();
	assert!(retry > 0 && retry <= 10 * 60 + 1, "{retry}");
	let json = reply.json();
	assert_matches_schema("schemas/SubsonicResponse.json", &json);
	assert_eq!(error_code(&json), 0);
	assert!(json["subsonic-response"]["error"]["message"]
		.as_str()
		.unwrap()
		.contains("Try again"));

	// Not even the right password gets in while locked
	let reply = fetch(&mut service, &password_uri("test_user", "test_password")).await;
	assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS);

	// An API key is not affected, so apps using one keep working
	let reply = fetch(&mut service, &format!("/rest/ping?{}", auth_query(&key))).await;
	assert_eq!(reply.status, StatusCode::OK);
	assert_eq!(reply.json()["subsonic-response"]["status"], "ok");

	// Other users are not affected either
	let reply = fetch(
		&mut service,
		&password_uri("test_admin", "test_password_admin"),
	)
	.await;
	assert_eq!(reply.json()["subsonic-response"]["status"], "ok");
}

#[tokio::test]
async fn the_lockout_is_reported_in_the_requested_format() {
	let (mut service, _) = library_service(&test_name!()).await;
	fail_times(&mut service, "test_user", 10).await;
	let uri = format!("/rest/ping?u=test_user&p=wrong&v={VERSION}&c=t");
	let reply = fetch(&mut service, &uri).await;
	assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS);
	assert!(reply.content_type.starts_with("application/xml"));
	assert!(reply.text().contains("status=\"failed\""));
}

#[tokio::test]
async fn a_correct_password_clears_earlier_failures() {
	let (mut service, _) = library_service(&test_name!()).await;
	fail_times(&mut service, "test_user", 9).await;
	let reply = fetch(&mut service, &password_uri("test_user", "test_password")).await;
	assert_eq!(reply.json()["subsonic-response"]["status"], "ok");
	fail_times(&mut service, "test_user", 9).await;
	let reply = fetch(&mut service, &password_uri("test_user", "test_password")).await;
	assert_eq!(reply.status, StatusCode::OK, "still not locked");
}

#[tokio::test]
async fn unknown_users_are_limited_like_known_ones() {
	let (mut service, _) = library_service(&test_name!()).await;
	fail_times(&mut service, "ghost", 10).await;
	let reply = fetch(&mut service, &password_uri("ghost", "wrong")).await;
	assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS);
	// The lockout of one name says nothing about another
	let reply = fetch(&mut service, &password_uri("ghost2", "wrong")).await;
	assert_eq!(reply.status, StatusCode::OK);
}

#[tokio::test]
async fn malformed_passwords_count_as_failures() {
	let (mut service, _) = library_service(&test_name!()).await;
	for _ in 0..10 {
		let reply = fetch(&mut service, &password_uri("test_user", "enc:zz")).await;
		assert_eq!(error_code(&reply.json()), 40);
	}
	let reply = fetch(&mut service, &password_uri("test_user", "wrong")).await;
	assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn bad_api_keys_are_never_rate_limited() {
	let (mut service, key) = library_service(&test_name!()).await;
	for attempt in 0..30 {
		let reply = fetch(
			&mut service,
			&format!("/rest/ping?apiKey=nope{attempt}&v={VERSION}&c=t&f=json"),
		)
		.await;
		assert_eq!(reply.status, StatusCode::OK);
		assert_eq!(error_code(&reply.json()), 44);
	}
	let reply = fetch(&mut service, &format!("/rest/ping?{}", auth_query(&key))).await;
	assert_eq!(reply.json()["subsonic-response"]["status"], "ok");
}

#[tokio::test]
async fn limits_do_not_apply_to_servers_that_are_not_asked() {
	// Each server keeps its own count
	let (mut first, _) = library_service(&test_name!()).await;
	fail_times(&mut first, "test_user", 10).await;
	let (mut second, _) = library_service(&test_name!()).await;
	let reply = fetch(&mut second, &password_uri("test_user", "test_password")).await;
	assert_eq!(reply.status, StatusCode::OK);
}

#[tokio::test]
async fn a_changed_password_takes_effect_at_once_even_after_a_successful_sign_in() {
	let (mut service, _) = library_service(&test_name!()).await;
	// Signing in remembers the check...
	for _ in 0..3 {
		let reply = fetch(&mut service, &password_uri("test_user", "test_password")).await;
		assert_eq!(reply.json()["subsonic-response"]["status"], "ok");
	}

	service.login_admin().await;
	let response = service
		.fetch(&protocol::update_user(
			"test_user",
			dto::UserUpdate {
				new_password: Some("brand_new_password".to_owned()),
				..Default::default()
			},
		))
		.await;
	assert_eq!(response.status(), StatusCode::OK);
	service.logout().await;

	// ...which must not outlive the password
	let reply = fetch(&mut service, &password_uri("test_user", "test_password")).await;
	assert_eq!(error_code(&reply.json()), 40);
	let reply = fetch(
		&mut service,
		&password_uri("test_user", "brand_new_password"),
	)
	.await;
	assert_eq!(reply.json()["subsonic-response"]["status"], "ok");
}

#[tokio::test]
async fn a_deleted_user_cannot_sign_in_with_a_remembered_password() {
	let (mut service, _) = library_service(&test_name!()).await;
	let reply = fetch(&mut service, &password_uri("test_user", "test_password")).await;
	assert_eq!(reply.json()["subsonic-response"]["status"], "ok");

	service.login_admin().await;
	let response = service.fetch(&protocol::delete_user("test_user")).await;
	assert_eq!(response.status(), StatusCode::OK);
	service.logout().await;

	let reply = fetch(&mut service, &password_uri("test_user", "test_password")).await;
	assert_eq!(error_code(&reply.json()), 40);
}

#[tokio::test]
async fn the_password_of_one_user_never_works_for_another() {
	let (mut service, _) = library_service(&test_name!()).await;
	let reply = fetch(&mut service, &password_uri("test_user", "test_password")).await;
	assert_eq!(reply.json()["subsonic-response"]["status"], "ok");
	let reply = fetch(&mut service, &password_uri("test_admin", "test_password")).await;
	assert_eq!(error_code(&reply.json()), 40);
	let reply = fetch(
		&mut service,
		&password_uri("test_user", "test_password_admin"),
	)
	.await;
	assert_eq!(error_code(&reply.json()), 40);
}
