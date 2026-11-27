//! Users and the library scan.

use super::*;

async fn admin_key(service: &mut ServiceType) -> String {
	service.login_admin().await;
	let key = service
		.fetch_json::<_, dto::CreatedApiKey>(&protocol::create_api_key(dto::NewApiKey {
			label: "admin".to_owned(),
		}))
		.await
		.into_body()
		.key;
	service.logout().await;
	key
}

#[tokio::test]
async fn a_user_can_look_at_themselves() {
	let (mut service, key) = library_service(&test_name!()).await;
	let response = call_ok(&mut service, &key, "getUser", "username=test_user").await;
	let user = &response["user"];
	assert_eq!(user["username"], "test_user");
	assert_eq!(user["adminRole"], false);
	assert_eq!(user["settingsRole"], false);
	assert_eq!(user["streamRole"], true);
	assert_eq!(user["downloadRole"], true);
	assert_eq!(user["playlistRole"], true);
	assert_eq!(user["uploadRole"], false);
	assert_eq!(user["jukeboxRole"], false);
	assert_eq!(user["scrobblingEnabled"], true);

	// The folders they can use are the music folders
	let folders = call_ok(&mut service, &key, "getMusicFolders", "").await;
	let expected = folders["musicFolders"]["musicFolder"][0]["id"].clone();
	assert_eq!(user["folder"], serde_json::json!([expected]));
}

#[tokio::test]
async fn looking_at_other_users_is_for_administrators() {
	let (mut service, key) = library_service(&test_name!()).await;
	assert_eq!(
		call_failing(&mut service, &key, "getUser", "username=test_admin").await,
		50
	);
	assert_eq!(
		call_failing(&mut service, &key, "getUser", "username=nobody").await,
		50
	);
	assert_eq!(call_failing(&mut service, &key, "getUser", "").await, 10);

	let admin = admin_key(&mut service).await;
	let response = call_ok(&mut service, &admin, "getUser", "username=test_admin").await;
	assert_eq!(response["user"]["adminRole"], true);
	assert_eq!(response["user"]["settingsRole"], true);
	let response = call_ok(&mut service, &admin, "getUser", "username=test_user").await;
	assert_eq!(response["user"]["adminRole"], false);
	assert_eq!(
		call_failing(&mut service, &admin, "getUser", "username=nobody").await,
		70
	);
}

#[tokio::test]
async fn all_users_are_listed_for_administrators_only() {
	let (mut service, key) = library_service(&test_name!()).await;
	assert_eq!(call_failing(&mut service, &key, "getUsers", "").await, 50);

	let admin = admin_key(&mut service).await;
	let response = call_ok(&mut service, &admin, "getUsers", "").await;
	let names: Vec<&str> = response["users"]["user"]
		.as_array()
		.unwrap()
		.iter()
		.map(|u| u["username"].as_str().unwrap())
		.collect();
	assert_eq!(names, vec!["test_admin", "test_user"]);
}

#[tokio::test]
async fn the_scan_status_reports_the_indexed_songs() {
	let (mut service, key) = library_service(&test_name!()).await;
	let response = call_ok(&mut service, &key, "getScanStatus", "").await;
	assert_eq!(response["scanStatus"]["scanning"], false);
	assert_eq!(response["scanStatus"]["count"], 13);
}

#[tokio::test]
async fn only_administrators_can_start_a_scan() {
	let (mut service, key) = library_service(&test_name!()).await;
	assert_eq!(call_failing(&mut service, &key, "startScan", "").await, 50);

	let admin = admin_key(&mut service).await;
	let response = call_ok(&mut service, &admin, "startScan", "").await;
	assert!(response["scanStatus"]["scanning"].is_boolean());

	// The scan finishes, and the library is still there afterwards
	let mut done = false;
	for _ in 0..100 {
		let status = call_ok(&mut service, &key, "getScanStatus", "").await;
		if status["scanStatus"]["scanning"] == false && status["scanStatus"]["count"] == 13 {
			done = true;
			break;
		}
		tokio::time::sleep(std::time::Duration::from_millis(100)).await;
	}
	assert!(done, "the scan did not finish");
	let albums = call_ok(&mut service, &key, "getAlbumList2", "type=newest").await;
	assert_eq!(albums["albumList2"]["album"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn user_and_scan_endpoints_work_with_a_password_too() {
	let (mut service, _) = library_service(&test_name!()).await;
	let uri = format!(
		"/rest/getUser?u=test_user&p=test_password&v={VERSION}&c=t&f=json&username=test_user"
	);
	let reply = fetch(&mut service, &uri).await;
	assert_eq!(
		reply.json()["subsonic-response"]["user"]["username"],
		"test_user"
	);
}
