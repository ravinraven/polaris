//! Bookmarks and the saved play queue.

use super::*;

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

fn ids(songs: &[&String]) -> String {
	songs
		.iter()
		.map(|id| format!("id={id}"))
		.collect::<Vec<_>>()
		.join("&")
}

fn queue_ids(queue: &Value) -> Vec<String> {
	queue["entry"]
		.as_array()
		.map(|e| {
			e.iter()
				.map(|s| s["id"].as_str().unwrap().to_owned())
				.collect()
		})
		.unwrap_or_default()
}

async fn second_user_key(service: &mut ServiceType) -> String {
	service.login_admin().await;
	let _ = service
		.fetch(&protocol::create_user(dto::NewUser {
			name: "other_user".to_owned(),
			password: "password".to_owned(),
			admin: false,
		}))
		.await;
	let response = service
		.fetch_json::<_, dto::Authorization>(&protocol::login("other_user", "password"))
		.await;
	service.set_authorization(Some(response.into_body()));
	let key = service
		.fetch_json::<_, dto::CreatedApiKey>(&protocol::create_api_key(dto::NewApiKey {
			label: "x".to_owned(),
		}))
		.await
		.into_body()
		.key;
	service.logout().await;
	key
}

// Bookmarks

#[tokio::test]
async fn bookmarks_can_be_created_updated_listed_and_deleted() {
	let (mut service, key) = library_service(&test_name!()).await;
	let songs = song_ids(&mut service, &key).await;
	let empty = call_ok(&mut service, &key, "getBookmarks", "").await;
	assert!(empty["bookmarks"].get("bookmark").is_none());

	call_ok(
		&mut service,
		&key,
		"createBookmark",
		&format!("id={}&position=12345&comment=chapter%203", songs[2]),
	)
	.await;
	call_ok(
		&mut service,
		&key,
		"createBookmark",
		&format!("id={}&position=0", songs[5]),
	)
	.await;
	let response = call_ok(&mut service, &key, "getBookmarks", "").await;
	let bookmarks = response["bookmarks"]["bookmark"].as_array().unwrap();
	assert_eq!(bookmarks.len(), 2);
	let first = bookmarks
		.iter()
		.find(|b| b["entry"]["id"] == songs[2].as_str())
		.unwrap();
	assert_eq!(first["position"], 12345);
	assert_eq!(first["comment"], "chapter 3");
	assert_eq!(first["username"], "test_user");
	assert!(first["created"].as_str().unwrap().ends_with('Z'));
	assert!(first["changed"].as_str().unwrap().ends_with('Z'));
	assert_eq!(
		first["entry"]["isDir"], false,
		"the entry is the whole song"
	);
	assert!(first["entry"]["title"].is_string());
	let second = bookmarks
		.iter()
		.find(|b| b["entry"]["id"] == songs[5].as_str())
		.unwrap();
	assert!(second.get("comment").is_none());

	// A song has one bookmark: updating replaces the position and comment
	call_ok(
		&mut service,
		&key,
		"createBookmark",
		&format!("id={}&position=999", songs[2]),
	)
	.await;
	let response = call_ok(&mut service, &key, "getBookmarks", "").await;
	let bookmarks = response["bookmarks"]["bookmark"].as_array().unwrap();
	assert_eq!(bookmarks.len(), 2);
	let updated = bookmarks
		.iter()
		.find(|b| b["entry"]["id"] == songs[2].as_str())
		.unwrap();
	assert_eq!(updated["position"], 999);
	assert!(updated.get("comment").is_none());
	assert_eq!(updated["created"], first["created"]);

	call_ok(
		&mut service,
		&key,
		"deleteBookmark",
		&format!("id={}", songs[2]),
	)
	.await;
	let response = call_ok(&mut service, &key, "getBookmarks", "").await;
	assert_eq!(
		response["bookmarks"]["bookmark"].as_array().unwrap().len(),
		1
	);
	assert_eq!(
		call_failing(
			&mut service,
			&key,
			"deleteBookmark",
			&format!("id={}", songs[2])
		)
		.await,
		70
	);
}

#[tokio::test]
async fn bookmarks_are_validated() {
	let (mut service, key) = library_service(&test_name!()).await;
	let songs = song_ids(&mut service, &key).await;
	assert_eq!(
		call_failing(&mut service, &key, "createBookmark", "").await,
		10
	);
	assert_eq!(
		call_failing(
			&mut service,
			&key,
			"createBookmark",
			&format!("id={}", songs[0])
		)
		.await,
		10
	);
	assert_eq!(
		call_failing(&mut service, &key, "createBookmark", "position=5").await,
		10
	);
	assert_eq!(
		call_failing(
			&mut service,
			&key,
			"createBookmark",
			"id=so_unknown&position=5"
		)
		.await,
		70
	);
	for bad in ["-1", "x", "1.5"] {
		assert_eq!(
			call_failing(
				&mut service,
				&key,
				"createBookmark",
				&format!("id={}&position={bad}", songs[0])
			)
			.await,
			0,
			"{bad}"
		);
	}
	let long = "x".repeat(4001);
	assert_eq!(
		call_failing(
			&mut service,
			&key,
			"createBookmark",
			&format!("id={}&position=5&comment={long}", songs[0])
		)
		.await,
		0
	);
	assert_eq!(
		call_failing(&mut service, &key, "deleteBookmark", "").await,
		10
	);
	assert_eq!(
		call_failing(&mut service, &key, "deleteBookmark", "id=so_unknown").await,
		70
	);
	let response = call_ok(&mut service, &key, "getBookmarks", "").await;
	assert!(response["bookmarks"].get("bookmark").is_none());
}

#[tokio::test]
async fn bookmarks_are_personal() {
	let (mut service, key) = library_service(&test_name!()).await;
	let songs = song_ids(&mut service, &key).await;
	let other = second_user_key(&mut service).await;
	call_ok(
		&mut service,
		&key,
		"createBookmark",
		&format!("id={}&position=5", songs[0]),
	)
	.await;
	let response = call_ok(&mut service, &other, "getBookmarks", "").await;
	assert!(response["bookmarks"].get("bookmark").is_none());
	assert_eq!(
		call_failing(
			&mut service,
			&other,
			"deleteBookmark",
			&format!("id={}", songs[0])
		)
		.await,
		70
	);
	let response = call_ok(&mut service, &key, "getBookmarks", "").await;
	assert_eq!(
		response["bookmarks"]["bookmark"].as_array().unwrap().len(),
		1
	);
}

// The play queue

#[tokio::test]
async fn without_a_saved_queue_the_queue_is_empty() {
	let (mut service, key) = library_service(&test_name!()).await;
	let response = call_ok(&mut service, &key, "getPlayQueue", "").await;
	assert_eq!(response["playQueue"]["username"], "test_user");
	assert!(response["playQueue"].get("entry").is_none());
	assert!(response["playQueue"].get("current").is_none());
	let response = call_ok(&mut service, &key, "getPlayQueueByIndex", "").await;
	assert!(response["playQueueByIndex"].get("entry").is_none());
	assert!(response["playQueueByIndex"].get("currentIndex").is_none());
}

#[tokio::test]
async fn a_queue_can_be_saved_and_restored_by_id() {
	let (mut service, key) = library_service(&test_name!()).await;
	let songs = song_ids(&mut service, &key).await;
	let queue = [&songs[4], &songs[1], &songs[7]];
	call_ok(
		&mut service,
		&key,
		"savePlayQueue",
		&format!("{}&current={}&position=45000", ids(&queue), songs[1]),
	)
	.await;

	let response = call_ok(&mut service, &key, "getPlayQueue", "").await;
	let restored = &response["playQueue"];
	assert_eq!(
		queue_ids(restored),
		queue.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
		"order kept"
	);
	assert_eq!(restored["current"], songs[1].as_str());
	assert_eq!(restored["position"], 45000);
	assert_eq!(restored["username"], "test_user");
	assert_eq!(restored["changedBy"], "polaris-tests");
	assert!(restored["changed"].as_str().unwrap().ends_with('Z'));
	assert!(
		restored["entry"][0]["title"].is_string(),
		"entries are whole songs"
	);
}

#[tokio::test]
async fn saving_a_queue_replaces_it_and_no_songs_clears_it() {
	let (mut service, key) = library_service(&test_name!()).await;
	let songs = song_ids(&mut service, &key).await;
	call_ok(
		&mut service,
		&key,
		"savePlayQueue",
		&format!("{}&current={}", ids(&[&songs[0], &songs[1]]), songs[0]),
	)
	.await;
	call_ok(&mut service, &key, "savePlayQueue", &ids(&[&songs[2]])).await;
	let response = call_ok(&mut service, &key, "getPlayQueue", "").await;
	assert_eq!(queue_ids(&response["playQueue"]), vec![songs[2].clone()]);
	assert!(
		response["playQueue"].get("current").is_none(),
		"no current song was given"
	);
	assert_eq!(response["playQueue"]["position"], 0);

	call_ok(&mut service, &key, "savePlayQueue", "").await;
	let response = call_ok(&mut service, &key, "getPlayQueue", "").await;
	assert!(response["playQueue"].get("entry").is_none());
	// Clearing an empty queue is fine
	call_ok(&mut service, &key, "savePlayQueue", "").await;
}

#[tokio::test]
async fn saving_a_queue_is_validated() {
	let (mut service, key) = library_service(&test_name!()).await;
	let songs = song_ids(&mut service, &key).await;
	call_ok(&mut service, &key, "savePlayQueue", &ids(&[&songs[0]])).await;

	assert_eq!(
		call_failing(
			&mut service,
			&key,
			"savePlayQueue",
			&format!("{}&id=so_unknown", ids(&[&songs[1]]))
		)
		.await,
		70
	);
	assert_eq!(
		call_failing(
			&mut service,
			&key,
			"savePlayQueue",
			&format!("{}&current={}", ids(&[&songs[1]]), songs[2])
		)
		.await,
		0
	);
	assert_eq!(
		call_failing(
			&mut service,
			&key,
			"savePlayQueue",
			&format!("{}&position=-1", ids(&[&songs[1]]))
		)
		.await,
		0
	);
	assert_eq!(
		call_failing(
			&mut service,
			&key,
			"savePlayQueue",
			&format!("{}&position=x", ids(&[&songs[1]]))
		)
		.await,
		0
	);
	// Failures leave the saved queue alone
	let response = call_ok(&mut service, &key, "getPlayQueue", "").await;
	assert_eq!(queue_ids(&response["playQueue"]), vec![songs[0].clone()]);
}

#[tokio::test]
async fn the_same_song_can_be_queued_twice() {
	let (mut service, key) = library_service(&test_name!()).await;
	let songs = song_ids(&mut service, &key).await;
	let queue = [&songs[3], &songs[5], &songs[3]];
	// Saved by ID, the first occurrence is the current song
	call_ok(
		&mut service,
		&key,
		"savePlayQueue",
		&format!("{}&current={}", ids(&queue), songs[3]),
	)
	.await;
	let response = call_ok(&mut service, &key, "getPlayQueueByIndex", "").await;
	assert_eq!(response["playQueueByIndex"]["currentIndex"], 0);
	assert_eq!(queue_ids(&response["playQueueByIndex"]).len(), 3);

	// By index, the position of the song in the queue is exact
	call_ok(
		&mut service,
		&key,
		"savePlayQueueByIndex",
		&format!("{}&currentIndex=2&position=7", ids(&queue)),
	)
	.await;
	let response = call_ok(&mut service, &key, "getPlayQueueByIndex", "").await;
	assert_eq!(response["playQueueByIndex"]["currentIndex"], 2);
	assert_eq!(response["playQueueByIndex"]["position"], 7);
	assert_eq!(
		queue_ids(&response["playQueueByIndex"]),
		queue.iter().map(|s| s.to_string()).collect::<Vec<_>>()
	);
}

#[tokio::test]
async fn the_queue_is_shared_between_the_two_styles() {
	let (mut service, key) = library_service(&test_name!()).await;
	let songs = song_ids(&mut service, &key).await;
	let queue = [&songs[0], &songs[1], &songs[2]];
	call_ok(
		&mut service,
		&key,
		"savePlayQueueByIndex",
		&format!("{}&currentIndex=1&position=100", ids(&queue)),
	)
	.await;
	let response = call_ok(&mut service, &key, "getPlayQueue", "").await;
	assert_eq!(response["playQueue"]["current"], songs[1].as_str());
	assert_eq!(response["playQueue"]["position"], 100);
	assert_eq!(queue_ids(&response["playQueue"]).len(), 3);

	call_ok(
		&mut service,
		&key,
		"savePlayQueue",
		&format!("{}&current={}", ids(&queue), songs[2]),
	)
	.await;
	let response = call_ok(&mut service, &key, "getPlayQueueByIndex", "").await;
	assert_eq!(response["playQueueByIndex"]["currentIndex"], 2);

	call_ok(&mut service, &key, "savePlayQueueByIndex", "").await;
	let response = call_ok(&mut service, &key, "getPlayQueue", "").await;
	assert!(response["playQueue"].get("entry").is_none());
}

#[tokio::test]
async fn saving_a_queue_by_index_is_validated() {
	let (mut service, key) = library_service(&test_name!()).await;
	let songs = song_ids(&mut service, &key).await;
	let queue = ids(&[&songs[0], &songs[1]]);
	for bad in ["-1", "2", "99", "x"] {
		assert_eq!(
			call_failing(
				&mut service,
				&key,
				"savePlayQueueByIndex",
				&format!("{queue}&currentIndex={bad}")
			)
			.await,
			0,
			"{bad}"
		);
	}
	assert_eq!(
		call_failing(
			&mut service,
			&key,
			"savePlayQueueByIndex",
			&format!("{queue}&position=-3")
		)
		.await,
		0
	);
	assert_eq!(
		call_failing(&mut service, &key, "savePlayQueueByIndex", "id=so_unknown").await,
		70
	);
	let response = call_ok(&mut service, &key, "getPlayQueueByIndex", "").await;
	assert!(
		response["playQueueByIndex"].get("entry").is_none(),
		"nothing was saved"
	);
	// A queue with no current song is fine
	call_ok(&mut service, &key, "savePlayQueueByIndex", &queue).await;
	let response = call_ok(&mut service, &key, "getPlayQueueByIndex", "").await;
	assert!(response["playQueueByIndex"].get("currentIndex").is_none());
}

#[tokio::test]
async fn queues_are_personal() {
	let (mut service, key) = library_service(&test_name!()).await;
	let songs = song_ids(&mut service, &key).await;
	let other = second_user_key(&mut service).await;
	call_ok(&mut service, &key, "savePlayQueue", &ids(&[&songs[0]])).await;
	call_ok(
		&mut service,
		&other,
		"savePlayQueue",
		&ids(&[&songs[1], &songs[2]]),
	)
	.await;

	let mine = call_ok(&mut service, &key, "getPlayQueue", "").await;
	assert_eq!(queue_ids(&mine["playQueue"]), vec![songs[0].clone()]);
	let theirs = call_ok(&mut service, &other, "getPlayQueue", "").await;
	assert_eq!(
		queue_ids(&theirs["playQueue"]),
		vec![songs[1].clone(), songs[2].clone()]
	);
	assert_eq!(theirs["playQueue"]["username"], "other_user");
}

#[tokio::test]
async fn queue_and_bookmark_endpoints_need_authentication() {
	let (mut service, _) = library_service(&test_name!()).await;
	for endpoint in [
		"getBookmarks",
		"createBookmark",
		"deleteBookmark",
		"getPlayQueue",
		"savePlayQueue",
		"getPlayQueueByIndex",
		"savePlayQueueByIndex",
	] {
		let reply = fetch(
			&mut service,
			&format!("/rest/{endpoint}?apiKey=bad&v={VERSION}&c=t&f=json"),
		)
		.await;
		assert_eq!(error_code(&reply.json()), 44, "{endpoint}");
	}
}

#[tokio::test]
async fn the_queue_is_described_in_xml_too() {
	let (mut service, key) = library_service(&test_name!()).await;
	let songs = song_ids(&mut service, &key).await;
	call_ok(
		&mut service,
		&key,
		"savePlayQueue",
		&format!(
			"{}&current={}&position=5",
			ids(&[&songs[0], &songs[1]]),
			songs[1]
		),
	)
	.await;
	let reply = fetch(
		&mut service,
		&format!("/rest/getPlayQueue?apiKey={key}&v={VERSION}&c=t"),
	)
	.await;
	let body = reply.text();
	let doc = roxmltree::Document::parse(&body).unwrap();
	let queue = doc
		.root_element()
		.children()
		.find(|n| n.has_tag_name("playQueue"))
		.unwrap();
	assert_eq!(queue.attribute("current"), Some(songs[1].as_str()));
	assert_eq!(queue.attribute("position"), Some("5"));
	assert_eq!(
		queue.children().filter(|n| n.has_tag_name("entry")).count(),
		2
	);
}
