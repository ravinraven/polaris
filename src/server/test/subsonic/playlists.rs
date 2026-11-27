//! Playlists.

use super::*;
use crate::server::test::constants::{TEST_PASSWORD_ADMIN, TEST_USERNAME_ADMIN};

/// The IDs of every song, in a fixed order (the order of their paths).
async fn song_ids(service: &mut ServiceType, key: &str) -> Vec<String> {
	let response = call_ok(service, key, "getRandomSongs", "size=500").await;
	let mut songs: Vec<(String, String)> = response["randomSongs"]["song"]
		.as_array()
		.unwrap()
		.iter()
		.map(|s| {
			(
				s["path"].as_str().unwrap().to_owned(),
				s["id"].as_str().unwrap().to_owned(),
			)
		})
		.collect();
	songs.sort();
	songs.into_iter().map(|(_, id)| id).collect()
}

fn song_params(ids: &[&String]) -> String {
	ids.iter()
		.map(|id| format!("songId={id}"))
		.collect::<Vec<_>>()
		.join("&")
}

fn add_params(ids: &[&String]) -> String {
	ids.iter()
		.map(|id| format!("songIdToAdd={id}"))
		.collect::<Vec<_>>()
		.join("&")
}

fn entry_ids(playlist: &Value) -> Vec<String> {
	playlist["entry"]
		.as_array()
		.map(|e| {
			e.iter()
				.map(|s| s["id"].as_str().unwrap().to_owned())
				.collect()
		})
		.unwrap_or_default()
}

async fn read(service: &mut ServiceType, key: String, id: String) -> Vec<String> {
	let response = call_ok(service, &key, "getPlaylist", &format!("id={id}")).await;
	entry_ids(&response["playlist"])
}

async fn create(service: &mut ServiceType, key: &str, name: &str, ids: &[&String]) -> Value {
	let response = call_ok(
		service,
		key,
		"createPlaylist",
		&format!("name={name}&{}", song_params(ids)),
	)
	.await;
	response["playlist"].clone()
}

/// A key for another user, who is made (as a regular user) if needed.
async fn key_for(service: &mut ServiceType, username: &str, password: &str, admin: bool) -> String {
	service.login_admin().await;
	let _ = service
		.fetch(&protocol::create_user(dto::NewUser {
			name: username.to_owned(),
			password: password.to_owned(),
			admin,
		}))
		.await;
	let response = service
		.fetch_json::<_, dto::Authorization>(&protocol::login(username, password))
		.await;
	assert_eq!(response.status(), StatusCode::OK);
	service.set_authorization(Some(response.into_body()));
	let key = service
		.fetch_json::<_, dto::CreatedApiKey>(&protocol::create_api_key(dto::NewApiKey {
			label: "other".to_owned(),
		}))
		.await
		.into_body()
		.key;
	service.logout().await;
	key
}

#[tokio::test]
async fn a_new_account_has_no_playlists() {
	let (mut service, key) = library_service(&test_name!()).await;
	let response = call_ok(&mut service, &key, "getPlaylists", "").await;
	assert!(response["playlists"].get("playlist").is_none());
}

#[tokio::test]
async fn playlists_can_be_created_read_and_listed() {
	let (mut service, key) = library_service(&test_name!()).await;
	let ids = song_ids(&mut service, &key).await;

	let created = create(
		&mut service,
		&key,
		"Road%20trip",
		&[&ids[3], &ids[0], &ids[7]],
	)
	.await;
	assert_eq!(created["name"], "Road trip");
	assert_eq!(created["owner"], "test_user");
	assert_eq!(created["public"], false);
	assert_eq!(created["songCount"], 3);
	assert!(created["id"].as_str().unwrap().starts_with("pl_"));
	assert!(created["created"].as_str().unwrap().ends_with('Z'));
	assert!(created["changed"].as_str().unwrap().ends_with('Z'));
	assert_eq!(
		entry_ids(&created),
		vec![ids[3].clone(), ids[0].clone(), ids[7].clone()],
		"in the order given"
	);

	let id = created["id"].as_str().unwrap();
	let read = call_ok(&mut service, &key, "getPlaylist", &format!("id={id}")).await;
	assert_eq!(read["playlist"], created);

	let listed = call_ok(&mut service, &key, "getPlaylists", "").await;
	let listed = listed["playlists"]["playlist"].as_array().unwrap();
	assert_eq!(listed.len(), 1);
	assert_eq!(listed[0]["id"], id);
	assert_eq!(listed[0]["songCount"], 3);
	assert!(listed[0].get("entry").is_none(), "a listing has no entries");
}

#[tokio::test]
async fn playlist_ids_and_times_are_stable() {
	let (mut service, key) = library_service(&test_name!()).await;
	let ids = song_ids(&mut service, &key).await;
	let created = create(&mut service, &key, "Stable", &[&ids[0]]).await;
	let first = call_ok(&mut service, &key, "getPlaylists", "").await;
	let second = call_ok(&mut service, &key, "getPlaylists", "").await;
	assert_eq!(first, second);
	assert_eq!(
		first["playlists"]["playlist"][0]["created"],
		created["created"]
	);
}

#[tokio::test]
async fn creating_a_playlist_is_validated() {
	let (mut service, key) = library_service(&test_name!()).await;
	let ids = song_ids(&mut service, &key).await;
	create(&mut service, &key, "Taken", &[&ids[0]]).await;

	assert_eq!(
		call_failing(&mut service, &key, "createPlaylist", "").await,
		10
	);
	assert_eq!(
		call_failing(&mut service, &key, "createPlaylist", "name=").await,
		10
	);
	for name in ["%20%20", "a%0Ab", &"x".repeat(201)] {
		assert_eq!(
			call_failing(
				&mut service,
				&key,
				"createPlaylist",
				&format!("name={name}")
			)
			.await,
			0,
			"{name:?}"
		);
	}
	// Names are unique per user, whatever the surrounding spaces
	assert_eq!(
		call_failing(&mut service, &key, "createPlaylist", "name=Taken").await,
		0
	);
	assert_eq!(
		call_failing(&mut service, &key, "createPlaylist", "name=%20Taken%20").await,
		0
	);
	// Songs must exist, and be songs
	assert_eq!(
		call_failing(
			&mut service,
			&key,
			"createPlaylist",
			"name=New&songId=so_unknown"
		)
		.await,
		70
	);
	let artists = call_ok(&mut service, &key, "getArtists", "").await;
	let artist = artists["artists"]["index"][0]["artist"][0]["id"]
		.as_str()
		.unwrap()
		.to_owned();
	assert_eq!(
		call_failing(
			&mut service,
			&key,
			"createPlaylist",
			&format!("name=New&songId={artist}")
		)
		.await,
		70
	);

	// Nothing was created by the failures
	let listed = call_ok(&mut service, &key, "getPlaylists", "").await;
	assert_eq!(listed["playlists"]["playlist"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn a_playlist_can_be_empty_and_hold_the_same_song_twice() {
	let (mut service, key) = library_service(&test_name!()).await;
	let ids = song_ids(&mut service, &key).await;
	let empty = create(&mut service, &key, "Empty", &[]).await;
	assert_eq!(empty["songCount"], 0);
	assert!(empty.get("entry").is_none());

	let twice = create(&mut service, &key, "Twice", &[&ids[1], &ids[1]]).await;
	assert_eq!(entry_ids(&twice), vec![ids[1].clone(), ids[1].clone()]);
}

#[tokio::test]
async fn creating_with_an_id_replaces_the_songs_and_can_rename() {
	let (mut service, key) = library_service(&test_name!()).await;
	let ids = song_ids(&mut service, &key).await;
	let created = create(&mut service, &key, "Old", &[&ids[0], &ids[1]]).await;
	let id = created["id"].as_str().unwrap().to_owned();

	let response = call_ok(
		&mut service,
		&key,
		"createPlaylist",
		&format!("playlistId={id}&{}", song_params(&[&ids[5]])),
	)
	.await;
	assert_eq!(entry_ids(&response["playlist"]), vec![ids[5].clone()]);
	assert_eq!(response["playlist"]["id"], id.as_str());
	assert_eq!(response["playlist"]["name"], "Old");

	let response = call_ok(
		&mut service,
		&key,
		"createPlaylist",
		&format!(
			"playlistId={id}&name=New%20name&{}",
			song_params(&[&ids[6]])
		),
	)
	.await;
	assert_eq!(response["playlist"]["name"], "New name");
	assert_eq!(
		response["playlist"]["id"],
		id.as_str(),
		"renaming keeps the ID"
	);
	assert_eq!(response["playlist"]["created"], created["created"]);

	let listed = call_ok(&mut service, &key, "getPlaylists", "").await;
	let names: Vec<&str> = listed["playlists"]["playlist"]
		.as_array()
		.unwrap()
		.iter()
		.map(|p| p["name"].as_str().unwrap())
		.collect();
	assert_eq!(names, vec!["New name"]);
}

#[tokio::test]
async fn songs_can_be_added_and_removed() {
	let (mut service, key) = library_service(&test_name!()).await;
	let ids = song_ids(&mut service, &key).await;
	let created = create(
		&mut service,
		&key,
		"Edit",
		&[&ids[0], &ids[1], &ids[2], &ids[3]],
	)
	.await;
	let id = created["id"].as_str().unwrap().to_owned();

	call_ok(
		&mut service,
		&key,
		"updatePlaylist",
		&format!("playlistId={id}&{}", add_params(&[&ids[8], &ids[9]])),
	)
	.await;
	assert_eq!(
		read(&mut service, key.clone(), id.clone()).await,
		vec![
			ids[0].clone(),
			ids[1].clone(),
			ids[2].clone(),
			ids[3].clone(),
			ids[8].clone(),
			ids[9].clone()
		]
	);

	// Indices refer to the playlist before the request, and the removal comes before the addition
	call_ok(
		&mut service,
		&key,
		"updatePlaylist",
		&format!(
			"playlistId={id}&songIndexToRemove=0&songIndexToRemove=3&songIndexToRemove=5&{}",
			add_params(&[&ids[10]])
		),
	)
	.await;
	assert_eq!(
		read(&mut service, key.clone(), id.clone()).await,
		vec![
			ids[1].clone(),
			ids[2].clone(),
			ids[8].clone(),
			ids[10].clone()
		]
	);

	for bad in [
		"songIndexToRemove=-1",
		"songIndexToRemove=99",
		"songIndexToRemove=x",
	] {
		assert_eq!(
			call_failing(
				&mut service,
				&key,
				"updatePlaylist",
				&format!("playlistId={id}&{bad}")
			)
			.await,
			0,
			"{bad}"
		);
	}
	assert_eq!(
		read(&mut service, key.clone(), id.clone()).await.len(),
		4,
		"failed updates change nothing"
	);
	assert_eq!(
		call_failing(
			&mut service,
			&key,
			"updatePlaylist",
			&format!("playlistId={id}&songIdToAdd=so_unknown")
		)
		.await,
		70
	);
	assert_eq!(read(&mut service, key.clone(), id.clone()).await.len(), 4);
}

#[tokio::test]
async fn playlists_can_be_renamed_and_commented() {
	let (mut service, key) = library_service(&test_name!()).await;
	let ids = song_ids(&mut service, &key).await;
	create(&mut service, &key, "Other", &[&ids[0]]).await;
	let created = create(&mut service, &key, "Mine", &[&ids[1]]).await;
	let id = created["id"].as_str().unwrap().to_owned();
	assert!(created.get("comment").is_none());

	call_ok(
		&mut service,
		&key,
		"updatePlaylist",
		&format!("playlistId={id}&comment=for%20the%20road&public=true"),
	)
	.await;
	let read = call_ok(&mut service, &key, "getPlaylist", &format!("id={id}")).await;
	assert_eq!(read["playlist"]["comment"], "for the road");
	assert_eq!(
		read["playlist"]["public"], false,
		"Polaris playlists are always private"
	);
	assert_eq!(read["playlist"]["created"], created["created"]);

	call_ok(
		&mut service,
		&key,
		"updatePlaylist",
		&format!("playlistId={id}&comment="),
	)
	.await;
	let read = call_ok(&mut service, &key, "getPlaylist", &format!("id={id}")).await;
	assert!(read["playlist"].get("comment").is_none());

	call_ok(
		&mut service,
		&key,
		"updatePlaylist",
		&format!("playlistId={id}&name=Renamed"),
	)
	.await;
	let read = call_ok(&mut service, &key, "getPlaylist", &format!("id={id}")).await;
	assert_eq!(read["playlist"]["name"], "Renamed");
	assert_eq!(entry_ids(&read["playlist"]), vec![ids[1].clone()]);

	assert_eq!(
		call_failing(
			&mut service,
			&key,
			"updatePlaylist",
			&format!("playlistId={id}&name=Other")
		)
		.await,
		0
	);
	assert_eq!(
		call_failing(
			&mut service,
			&key,
			"updatePlaylist",
			&format!("playlistId={id}&name=%20%20")
		)
		.await,
		0
	);
	assert_eq!(
		call_failing(
			&mut service,
			&key,
			"updatePlaylist",
			&format!("playlistId={id}&public=maybe")
		)
		.await,
		0
	);
}

#[tokio::test]
async fn updating_needs_a_known_playlist() {
	let (mut service, key) = library_service(&test_name!()).await;
	assert_eq!(
		call_failing(&mut service, &key, "updatePlaylist", "").await,
		10
	);
	assert_eq!(
		call_failing(
			&mut service,
			&key,
			"updatePlaylist",
			"playlistId=pl_unknown"
		)
		.await,
		70
	);
	assert_eq!(
		call_failing(&mut service, &key, "getPlaylist", "id=pl_unknown").await,
		70
	);
	assert_eq!(
		call_failing(&mut service, &key, "getPlaylist", "").await,
		10
	);
	assert_eq!(
		call_failing(&mut service, &key, "deletePlaylist", "id=pl_unknown").await,
		70
	);
	assert_eq!(
		call_failing(&mut service, &key, "deletePlaylist", "").await,
		10
	);
}

#[tokio::test]
async fn playlists_can_be_deleted() {
	let (mut service, key) = library_service(&test_name!()).await;
	let ids = song_ids(&mut service, &key).await;
	let created = create(&mut service, &key, "Doomed", &[&ids[0]]).await;
	let id = created["id"].as_str().unwrap().to_owned();

	call_ok(&mut service, &key, "deletePlaylist", &format!("id={id}")).await;
	assert_eq!(
		call_failing(&mut service, &key, "getPlaylist", &format!("id={id}")).await,
		70
	);
	assert_eq!(
		call_failing(&mut service, &key, "deletePlaylist", &format!("id={id}")).await,
		70
	);
	let listed = call_ok(&mut service, &key, "getPlaylists", "").await;
	assert!(listed["playlists"].get("playlist").is_none());

	// The name can be used again, with a fresh creation time
	let again = create(&mut service, &key, "Doomed", &[]).await;
	assert_eq!(again["songCount"], 0);
}

#[tokio::test]
async fn users_cannot_see_or_change_each_others_playlists() {
	let (mut service, key) = library_service(&test_name!()).await;
	let ids = song_ids(&mut service, &key).await;
	let created = create(&mut service, &key, "Private", &[&ids[0]]).await;
	let id = created["id"].as_str().unwrap().to_owned();
	let other = key_for(&mut service, "other_user", "other_password", false).await;

	assert_eq!(
		call_failing(&mut service, &other, "getPlaylist", &format!("id={id}")).await,
		70
	);
	assert_eq!(
		call_failing(
			&mut service,
			&other,
			"updatePlaylist",
			&format!("playlistId={id}&name=Mine")
		)
		.await,
		70
	);
	assert_eq!(
		call_failing(&mut service, &other, "deletePlaylist", &format!("id={id}")).await,
		70
	);
	assert_eq!(
		call_failing(
			&mut service,
			&other,
			"createPlaylist",
			&format!("playlistId={id}")
		)
		.await,
		70
	);
	let listed = call_ok(&mut service, &other, "getPlaylists", "").await;
	assert!(listed["playlists"].get("playlist").is_none());
	// Another user can use the same name
	let theirs = create(&mut service, &other, "Private", &[&ids[1]]).await;
	assert_ne!(theirs["id"], created["id"]);
	// ...and a regular user may not look at someone else's list
	assert_eq!(
		call_failing(&mut service, &other, "getPlaylists", "username=test_user").await,
		50
	);

	// The owner's playlist is untouched
	let read = call_ok(&mut service, &key, "getPlaylist", &format!("id={id}")).await;
	assert_eq!(entry_ids(&read["playlist"]), vec![ids[0].clone()]);
}

#[tokio::test]
async fn administrators_can_look_but_not_change() {
	let (mut service, key) = library_service(&test_name!()).await;
	let ids = song_ids(&mut service, &key).await;
	let created = create(&mut service, &key, "Watched", &[&ids[0]]).await;
	let id = created["id"].as_str().unwrap().to_owned();

	service.login_admin().await;
	let admin_key = service
		.fetch_json::<_, dto::CreatedApiKey>(&protocol::create_api_key(dto::NewApiKey {
			label: "admin".to_owned(),
		}))
		.await
		.into_body()
		.key;
	service.logout().await;
	assert!(!TEST_USERNAME_ADMIN.is_empty() && !TEST_PASSWORD_ADMIN.is_empty());

	let read = call_ok(&mut service, &admin_key, "getPlaylist", &format!("id={id}")).await;
	assert_eq!(read["playlist"]["owner"], "test_user");
	let listed = call_ok(
		&mut service,
		&admin_key,
		"getPlaylists",
		"username=test_user",
	)
	.await;
	assert_eq!(listed["playlists"]["playlist"].as_array().unwrap().len(), 1);
	assert_eq!(
		call_failing(
			&mut service,
			&admin_key,
			"updatePlaylist",
			&format!("playlistId={id}&name=Hijacked")
		)
		.await,
		50
	);
	assert_eq!(
		call_failing(
			&mut service,
			&admin_key,
			"deletePlaylist",
			&format!("id={id}")
		)
		.await,
		50
	);
}

#[tokio::test]
async fn playlists_made_in_polaris_show_up() {
	let (mut service, key) = library_service(&test_name!()).await;
	service.login().await;
	let response = service
		.fetch(&protocol::save_playlist(
			"From Polaris",
			dto::SavePlaylistInput {
				tracks: vec![std::path::PathBuf::from(
					"collection/Khemmis/Hunted/01 - Above The Water.mp3",
				)],
			},
		))
		.await;
	assert_eq!(response.status(), StatusCode::OK);
	service.logout().await;

	let listed = call_ok(&mut service, &key, "getPlaylists", "").await;
	let playlist = &listed["playlists"]["playlist"][0];
	assert_eq!(playlist["name"], "From Polaris");
	assert_eq!(playlist["songCount"], 1);
	let id = playlist["id"].as_str().unwrap();
	let read = call_ok(&mut service, &key, "getPlaylist", &format!("id={id}")).await;
	assert_eq!(read["playlist"]["entry"][0]["title"], "Above The Water");
	let again = call_ok(&mut service, &key, "getPlaylists", "").await;
	assert_eq!(
		again["playlists"]["playlist"][0]["created"],
		playlist["created"]
	);
}

#[tokio::test]
async fn playlists_are_described_in_xml_too() {
	let (mut service, key) = library_service(&test_name!()).await;
	let ids = song_ids(&mut service, &key).await;
	create(&mut service, &key, "In%20XML", &[&ids[0], &ids[1]]).await;
	let reply = fetch(
		&mut service,
		&format!("/rest/getPlaylists?apiKey={key}&v={VERSION}&c=t"),
	)
	.await;
	let body = reply.text();
	let doc = roxmltree::Document::parse(&body).unwrap();
	let playlist = doc
		.root_element()
		.descendants()
		.find(|n| n.has_tag_name("playlist"))
		.unwrap();
	assert_eq!(playlist.attribute("name"), Some("In XML"));
	assert_eq!(playlist.attribute("songCount"), Some("2"));
	assert_eq!(playlist.attribute("public"), Some("false"));
}
