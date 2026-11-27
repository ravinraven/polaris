//! Stars, ratings, plays, what is playing now, and the lists built from them.

use super::*;

struct Hunted {
	artist: String,
	album: String,
	songs: Vec<String>,
}

async fn hunted(service: &mut ServiceType, key: &str) -> Hunted {
	let artists = call_ok(service, key, "getArtists", "").await;
	let artist = artists["artists"]["index"][0]["artist"][0]["id"]
		.as_str()
		.unwrap()
		.to_owned();
	let artist_response = call_ok(service, key, "getArtist", &format!("id={artist}")).await;
	let album = artist_response["artist"]["album"][0]["id"]
		.as_str()
		.unwrap()
		.to_owned();
	let album_response = call_ok(service, key, "getAlbum", &format!("id={album}")).await;
	let songs = album_response["album"]["song"]
		.as_array()
		.unwrap()
		.iter()
		.map(|s| s["id"].as_str().unwrap().to_owned())
		.collect();
	Hunted {
		artist,
		album,
		songs,
	}
}

async fn song(service: &mut ServiceType, key: &str, id: &str) -> Value {
	call_ok(service, key, "getSong", &format!("id={id}")).await["song"].clone()
}

async fn other_key(service: &mut ServiceType, name: &str, admin: bool) -> String {
	service.login_admin().await;
	let _ = service
		.fetch(&protocol::create_user(dto::NewUser {
			name: name.to_owned(),
			password: "password".to_owned(),
			admin,
		}))
		.await;
	let response = service
		.fetch_json::<_, dto::Authorization>(&protocol::login(name, "password"))
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

// Stars

#[tokio::test]
async fn songs_albums_and_artists_can_be_starred_and_unstarred() {
	let (mut service, key) = library_service(&test_name!()).await;
	let h = hunted(&mut service, &key).await;
	assert!(song(&mut service, &key, &h.songs[0])
		.await
		.get("starred")
		.is_none());

	call_ok(
		&mut service,
		&key,
		"star",
		&format!(
			"id={}&albumId={}&artistId={}",
			h.songs[0], h.album, h.artist
		),
	)
	.await;
	let starred = song(&mut service, &key, &h.songs[0]).await;
	assert!(starred["starred"].as_str().unwrap().ends_with('Z'));

	let response = call_ok(&mut service, &key, "getStarred2", "").await;
	let result = &response["starred2"];
	assert_eq!(result["song"][0]["id"], h.songs[0].as_str());
	assert_eq!(result["album"][0]["id"], h.album.as_str());
	assert_eq!(result["artist"][0]["id"], h.artist.as_str());
	assert!(result["album"][0]["starred"].is_string());
	assert_eq!(result["album"][0]["songCount"], 5);
	assert_eq!(result["artist"][0]["albumCount"], 1);

	// Starred albums have a list of their own
	let list = call_ok(&mut service, &key, "getAlbumList2", "type=starred").await;
	assert_eq!(list["albumList2"]["album"].as_array().unwrap().len(), 1);

	call_ok(
		&mut service,
		&key,
		"unstar",
		&format!(
			"id={}&albumId={}&artistId={}",
			h.songs[0], h.album, h.artist
		),
	)
	.await;
	assert!(song(&mut service, &key, &h.songs[0])
		.await
		.get("starred")
		.is_none());
	let response = call_ok(&mut service, &key, "getStarred2", "").await;
	assert!(response["starred2"].as_object().unwrap().is_empty());
}

#[tokio::test]
async fn starring_twice_keeps_the_first_time_and_unstarring_twice_is_fine() {
	let (mut service, key) = library_service(&test_name!()).await;
	let h = hunted(&mut service, &key).await;
	call_ok(&mut service, &key, "star", &format!("id={}", h.songs[1])).await;
	let first = song(&mut service, &key, &h.songs[1]).await["starred"].clone();
	tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
	call_ok(&mut service, &key, "star", &format!("id={}", h.songs[1])).await;
	assert_eq!(
		song(&mut service, &key, &h.songs[1]).await["starred"],
		first
	);

	call_ok(&mut service, &key, "unstar", &format!("id={}", h.songs[1])).await;
	call_ok(&mut service, &key, "unstar", &format!("id={}", h.songs[1])).await;
	call_ok(&mut service, &key, "unstar", &format!("id={}", h.songs[2])).await;
}

#[tokio::test]
async fn several_items_can_be_starred_at_once_but_only_if_all_are_valid() {
	let (mut service, key) = library_service(&test_name!()).await;
	let h = hunted(&mut service, &key).await;
	call_ok(
		&mut service,
		&key,
		"star",
		&format!("id={}&id={}&id={}", h.songs[0], h.songs[1], h.songs[2]),
	)
	.await;
	let response = call_ok(&mut service, &key, "getStarred2", "").await;
	assert_eq!(response["starred2"]["song"].as_array().unwrap().len(), 3);

	// One bad ID spoils the whole request
	assert_eq!(
		call_failing(
			&mut service,
			&key,
			"star",
			&format!("id={}&id=so_unknown", h.songs[4])
		)
		.await,
		70
	);
	assert!(song(&mut service, &key, &h.songs[4])
		.await
		.get("starred")
		.is_none());
	assert_eq!(call_failing(&mut service, &key, "star", "").await, 10);
	assert_eq!(call_failing(&mut service, &key, "unstar", "id=").await, 10);
	assert_eq!(
		call_failing(&mut service, &key, "star", "albumId=al_unknown").await,
		70
	);
	assert_eq!(
		call_failing(&mut service, &key, "star", "artistId=ar_unknown").await,
		70
	);
}

#[tokio::test]
async fn playlists_cannot_be_starred() {
	let (mut service, key) = library_service(&test_name!()).await;
	let created = call_ok(&mut service, &key, "createPlaylist", "name=Mix").await;
	let id = created["playlist"]["id"].as_str().unwrap();
	assert_eq!(
		call_failing(&mut service, &key, "star", &format!("id={id}")).await,
		70
	);
	assert_eq!(
		call_failing(
			&mut service,
			&key,
			"setRating",
			&format!("id={id}&rating=3")
		)
		.await,
		70
	);
}

#[tokio::test]
async fn stars_belong_to_a_user() {
	let (mut service, key) = library_service(&test_name!()).await;
	let h = hunted(&mut service, &key).await;
	let other = other_key(&mut service, "other_user", false).await;
	call_ok(&mut service, &key, "star", &format!("id={}", h.songs[0])).await;
	call_ok(
		&mut service,
		&key,
		"setRating",
		&format!("id={}&rating=4", h.songs[0]),
	)
	.await;

	assert!(song(&mut service, &other, &h.songs[0])
		.await
		.get("starred")
		.is_none());
	assert!(song(&mut service, &other, &h.songs[0])
		.await
		.get("userRating")
		.is_none());
	let response = call_ok(&mut service, &other, "getStarred2", "").await;
	assert!(response["starred2"].as_object().unwrap().is_empty());
}

#[tokio::test]
async fn the_legacy_starred_list_describes_albums_as_directories() {
	let (mut service, key) = library_service(&test_name!()).await;
	let h = hunted(&mut service, &key).await;
	call_ok(
		&mut service,
		&key,
		"star",
		&format!(
			"id={}&albumId={}&artistId={}",
			h.songs[0], h.album, h.artist
		),
	)
	.await;
	let response = call_ok(&mut service, &key, "getStarred", "").await;
	let result = &response["starred"];
	assert_eq!(result["album"][0]["isDir"], true);
	assert_eq!(result["artist"][0]["name"], "Khemmis");
	assert_eq!(result["song"][0]["id"], h.songs[0].as_str());
}

// Ratings

#[tokio::test]
async fn items_can_be_rated_and_the_rating_cleared() {
	let (mut service, key) = library_service(&test_name!()).await;
	let h = hunted(&mut service, &key).await;

	call_ok(
		&mut service,
		&key,
		"setRating",
		&format!("id={}&rating=5", h.songs[0]),
	)
	.await;
	assert_eq!(song(&mut service, &key, &h.songs[0]).await["userRating"], 5);
	call_ok(
		&mut service,
		&key,
		"setRating",
		&format!("id={}&rating=2", h.songs[0]),
	)
	.await;
	assert_eq!(song(&mut service, &key, &h.songs[0]).await["userRating"], 2);

	call_ok(
		&mut service,
		&key,
		"setRating",
		&format!("id={}&rating=0", h.songs[0]),
	)
	.await;
	assert!(song(&mut service, &key, &h.songs[0])
		.await
		.get("userRating")
		.is_none());

	call_ok(
		&mut service,
		&key,
		"setRating",
		&format!("id={}&rating=3", h.album),
	)
	.await;
	let list = call_ok(&mut service, &key, "getAlbumList2", "type=highest").await;
	assert_eq!(list["albumList2"]["album"][0]["userRating"], 3);
	assert_eq!(list["albumList2"]["album"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn ratings_are_validated() {
	let (mut service, key) = library_service(&test_name!()).await;
	let h = hunted(&mut service, &key).await;
	for bad in ["6", "-1", "x", "2.5", "100"] {
		assert_eq!(
			call_failing(
				&mut service,
				&key,
				"setRating",
				&format!("id={}&rating={bad}", h.songs[0])
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
			"setRating",
			&format!("id={}", h.songs[0])
		)
		.await,
		10
	);
	assert_eq!(
		call_failing(&mut service, &key, "setRating", "rating=3").await,
		10
	);
	assert_eq!(
		call_failing(&mut service, &key, "setRating", "id=so_unknown&rating=3").await,
		70
	);
	assert!(song(&mut service, &key, &h.songs[0])
		.await
		.get("userRating")
		.is_none());
}

// Plays

#[tokio::test]
async fn scrobbling_counts_plays_of_songs_and_their_albums() {
	let (mut service, key) = library_service(&test_name!()).await;
	let h = hunted(&mut service, &key).await;
	assert!(song(&mut service, &key, &h.songs[0])
		.await
		.get("playCount")
		.is_none());

	call_ok(
		&mut service,
		&key,
		"scrobble",
		&format!("id={}", h.songs[0]),
	)
	.await;
	call_ok(
		&mut service,
		&key,
		"scrobble",
		&format!("id={}", h.songs[0]),
	)
	.await;
	call_ok(
		&mut service,
		&key,
		"scrobble",
		&format!("id={}", h.songs[1]),
	)
	.await;

	let first = song(&mut service, &key, &h.songs[0]).await;
	assert_eq!(first["playCount"], 2);
	assert!(first["played"].as_str().unwrap().ends_with('Z'));
	assert_eq!(song(&mut service, &key, &h.songs[1]).await["playCount"], 1);
	assert!(song(&mut service, &key, &h.songs[2])
		.await
		.get("playCount")
		.is_none());

	let album = call_ok(&mut service, &key, "getAlbum", &format!("id={}", h.album)).await;
	assert_eq!(album["album"]["playCount"], 3);

	let frequent = call_ok(&mut service, &key, "getAlbumList2", "type=frequent").await;
	assert_eq!(frequent["albumList2"]["album"][0]["id"], h.album.as_str());
	let recent = call_ok(&mut service, &key, "getAlbumList2", "type=recent").await;
	assert_eq!(recent["albumList2"]["album"][0]["id"], h.album.as_str());
}

#[tokio::test]
async fn scrobbles_can_be_dated_and_batched() {
	let (mut service, key) = library_service(&test_name!()).await;
	let h = hunted(&mut service, &key).await;
	// Times are milliseconds, matched with the songs in order
	call_ok(
		&mut service,
		&key,
		"scrobble",
		&format!(
			"id={}&time=1700000000000&id={}&time=1700000100000",
			h.songs[0], h.songs[1]
		),
	)
	.await;
	assert_eq!(
		song(&mut service, &key, &h.songs[0]).await["played"],
		"2023-11-14T22:13:20.000Z"
	);
	assert_eq!(
		song(&mut service, &key, &h.songs[1]).await["played"],
		"2023-11-14T22:15:00.000Z"
	);

	// The last played time never moves backwards
	call_ok(
		&mut service,
		&key,
		"scrobble",
		&format!("id={}&time=1600000000000", h.songs[0]),
	)
	.await;
	let again = song(&mut service, &key, &h.songs[0]).await;
	assert_eq!(again["played"], "2023-11-14T22:13:20.000Z");
	assert_eq!(again["playCount"], 2);

	// A time in the future is taken as now
	call_ok(
		&mut service,
		&key,
		"scrobble",
		&format!("id={}&time=99999999999999", h.songs[2]),
	)
	.await;
	let played = song(&mut service, &key, &h.songs[2]).await["played"]
		.as_str()
		.unwrap()
		.to_owned();
	assert!(played.as_str() < "2100-01-01", "{played}");
}

#[tokio::test]
async fn a_bad_scrobble_counts_nothing() {
	let (mut service, key) = library_service(&test_name!()).await;
	let h = hunted(&mut service, &key).await;
	assert_eq!(
		call_failing(
			&mut service,
			&key,
			"scrobble",
			&format!("id={}&id=so_unknown", h.songs[0])
		)
		.await,
		70
	);
	assert_eq!(
		call_failing(&mut service, &key, "scrobble", &format!("id={}", h.album)).await,
		70
	);
	assert_eq!(
		call_failing(
			&mut service,
			&key,
			"scrobble",
			&format!("id={}&time=-5", h.songs[0])
		)
		.await,
		0
	);
	assert_eq!(
		call_failing(
			&mut service,
			&key,
			"scrobble",
			&format!("id={}&time=abc", h.songs[0])
		)
		.await,
		0
	);
	assert_eq!(
		call_failing(
			&mut service,
			&key,
			"scrobble",
			&format!("id={}&submission=maybe", h.songs[0])
		)
		.await,
		0
	);
	assert_eq!(call_failing(&mut service, &key, "scrobble", "").await, 10);
	assert!(song(&mut service, &key, &h.songs[0])
		.await
		.get("playCount")
		.is_none());
}

// What is playing now

#[tokio::test]
async fn a_now_playing_notification_shows_up_but_is_not_a_play() {
	let (mut service, key) = library_service(&test_name!()).await;
	let h = hunted(&mut service, &key).await;
	let empty = call_ok(&mut service, &key, "getNowPlaying", "").await;
	assert_eq!(empty["nowPlaying"]["entry"], serde_json::json!([]));

	call_ok(
		&mut service,
		&key,
		"scrobble",
		&format!("id={}&submission=false", h.songs[2]),
	)
	.await;
	let response = call_ok(&mut service, &key, "getNowPlaying", "").await;
	let entry = &response["nowPlaying"]["entry"][0];
	assert_eq!(entry["id"], h.songs[2].as_str());
	assert_eq!(entry["title"], "Three Gates");
	assert_eq!(entry["username"], "test_user");
	assert_eq!(entry["minutesAgo"], 0);
	assert_eq!(entry["playerName"], "polaris-tests");
	assert!(entry["playerId"].is_i64());
	assert!(song(&mut service, &key, &h.songs[2])
		.await
		.get("playCount")
		.is_none());

	// Playing something else replaces it; finishing a song clears it
	call_ok(
		&mut service,
		&key,
		"scrobble",
		&format!("id={}&submission=false", h.songs[3]),
	)
	.await;
	let response = call_ok(&mut service, &key, "getNowPlaying", "").await;
	assert_eq!(response["nowPlaying"]["entry"].as_array().unwrap().len(), 1);
	assert_eq!(
		response["nowPlaying"]["entry"][0]["id"],
		h.songs[3].as_str()
	);
	call_ok(
		&mut service,
		&key,
		"scrobble",
		&format!("id={}", h.songs[3]),
	)
	.await;
	let response = call_ok(&mut service, &key, "getNowPlaying", "").await;
	assert_eq!(response["nowPlaying"]["entry"], serde_json::json!([]));
}

#[tokio::test]
async fn everyone_is_visible_to_administrators_and_only_themselves_to_others() {
	let (mut service, key) = library_service(&test_name!()).await;
	let h = hunted(&mut service, &key).await;
	let other = other_key(&mut service, "other_user", false).await;
	let admin = other_key(&mut service, "second_admin", true).await;

	call_ok(
		&mut service,
		&key,
		"scrobble",
		&format!("id={}&submission=false", h.songs[0]),
	)
	.await;
	call_ok(
		&mut service,
		&other,
		"scrobble",
		&format!("id={}&submission=false", h.songs[1]),
	)
	.await;

	let users = |response: &Value| -> Vec<String> {
		response["nowPlaying"]["entry"]
			.as_array()
			.map(|e| {
				e.iter()
					.map(|x| x["username"].as_str().unwrap().to_owned())
					.collect()
			})
			.unwrap_or_default()
	};
	assert_eq!(
		users(&call_ok(&mut service, &key, "getNowPlaying", "").await),
		vec!["test_user"]
	);
	assert_eq!(
		users(&call_ok(&mut service, &other, "getNowPlaying", "").await),
		vec!["other_user"]
	);
	assert_eq!(
		users(&call_ok(&mut service, &admin, "getNowPlaying", "").await),
		vec!["other_user", "test_user"]
	);
}

// Playback reports

fn report(id: &str, state: &str, position: i64, extra: &str) -> String {
	format!("mediaId={id}&mediaType=song&positionMs={position}&state={state}{extra}")
}

#[tokio::test]
async fn reporting_playback_updates_what_is_playing_and_counts_one_play() {
	let (mut service, key) = library_service(&test_name!()).await;
	let h = hunted(&mut service, &key).await;
	let id = &h.songs[0];

	call_ok(
		&mut service,
		&key,
		"reportPlayback",
		&report(id, "starting", 0, ""),
	)
	.await;
	call_ok(
		&mut service,
		&key,
		"reportPlayback",
		&report(id, "playing", 15000, "&playbackRate=1.5"),
	)
	.await;
	let response = call_ok(&mut service, &key, "getNowPlaying", "").await;
	let entry = &response["nowPlaying"]["entry"][0];
	assert_eq!(entry["state"], "playing");
	assert_eq!(entry["positionMs"], 15000);
	assert_eq!(entry["playbackRate"], 1.5);

	call_ok(
		&mut service,
		&key,
		"reportPlayback",
		&report(id, "paused", 20000, ""),
	)
	.await;
	call_ok(
		&mut service,
		&key,
		"reportPlayback",
		&report(id, "starting", 20000, ""),
	)
	.await;
	assert_eq!(
		song(&mut service, &key, id).await["playCount"],
		1,
		"resuming is not another play"
	);

	call_ok(
		&mut service,
		&key,
		"reportPlayback",
		&report(id, "stopped", 30000, ""),
	)
	.await;
	let response = call_ok(&mut service, &key, "getNowPlaying", "").await;
	assert_eq!(response["nowPlaying"]["entry"], serde_json::json!([]));

	// Starting the song again later is a new play
	call_ok(
		&mut service,
		&key,
		"reportPlayback",
		&report(id, "starting", 0, ""),
	)
	.await;
	assert_eq!(song(&mut service, &key, id).await["playCount"], 2);
}

#[tokio::test]
async fn reports_can_leave_the_play_counts_alone() {
	let (mut service, key) = library_service(&test_name!()).await;
	let h = hunted(&mut service, &key).await;
	call_ok(
		&mut service,
		&key,
		"reportPlayback",
		&report(&h.songs[0], "starting", 0, "&ignoreScrobble=true"),
	)
	.await;
	let response = call_ok(&mut service, &key, "getNowPlaying", "").await;
	assert_eq!(
		response["nowPlaying"]["entry"][0]["id"],
		h.songs[0].as_str()
	);
	assert!(song(&mut service, &key, &h.songs[0])
		.await
		.get("playCount")
		.is_none());
}

#[tokio::test]
async fn playback_reports_are_validated() {
	let (mut service, key) = library_service(&test_name!()).await;
	let h = hunted(&mut service, &key).await;
	let id = &h.songs[0];
	for (query, code) in [
		(report(id, "starting", 0, "").replace("song", "podcast"), 0),
		(report(id, "dancing", 0, ""), 0),
		(report(id, "playing", -1, ""), 0),
		(report(id, "playing", 0, "&playbackRate=0"), 0),
		(report(id, "playing", 0, "&playbackRate=fast"), 0),
		(report(id, "playing", 0, "&playbackRate=-1"), 0),
		(report(id, "playing", 0, "&ignoreScrobble=maybe"), 0),
		(report("so_unknown", "playing", 0, ""), 70),
		(report(&h.album, "playing", 0, ""), 70),
		("mediaType=song&positionMs=0&state=playing".to_owned(), 10),
		(format!("mediaId={id}&positionMs=0&state=playing"), 10),
		(format!("mediaId={id}&mediaType=song&state=playing"), 10),
		(format!("mediaId={id}&mediaType=song&positionMs=0"), 10),
	] {
		assert_eq!(
			call_failing(&mut service, &key, "reportPlayback", &query).await,
			code,
			"{query}"
		);
	}
	assert!(song(&mut service, &key, id)
		.await
		.get("playCount")
		.is_none());
}

// Top songs

#[tokio::test]
async fn top_songs_are_the_most_played_songs_of_an_artist() {
	let (mut service, key) = library_service(&test_name!()).await;
	let h = hunted(&mut service, &key).await;
	let plays = [(0, 1), (1, 4), (2, 2), (3, 3)];
	for (index, count) in plays {
		for _ in 0..count {
			call_ok(
				&mut service,
				&key,
				"scrobble",
				&format!("id={}", h.songs[index]),
			)
			.await;
		}
	}

	let by_name = call_ok(&mut service, &key, "getTopSongs", "artist=Khemmis").await;
	let titles: Vec<&str> = by_name["topSongs"]["song"]
		.as_array()
		.unwrap()
		.iter()
		.map(|s| s["title"].as_str().unwrap())
		.collect();
	assert_eq!(
		titles,
		vec![
			"Candlelight",
			"Beyond The Door",
			"Three Gates",
			"Above The Water"
		],
		"most played first, unplayed omitted"
	);

	let by_id = call_ok(
		&mut service,
		&key,
		"getTopSongs",
		&format!("id={}", h.artist),
	)
	.await;
	assert_eq!(by_id["topSongs"], by_name["topSongs"]);
	let limited = call_ok(
		&mut service,
		&key,
		"getTopSongs",
		&format!("id={}&count=2", h.artist),
	)
	.await;
	assert_eq!(limited["topSongs"]["song"].as_array().unwrap().len(), 2);
	let lowercase = call_ok(&mut service, &key, "getTopSongs", "artist=khemmis").await;
	assert_eq!(lowercase["topSongs"], by_name["topSongs"]);
	let none = call_ok(
		&mut service,
		&key,
		"getTopSongs",
		&format!("id={}&count=0", h.artist),
	)
	.await;
	assert!(none["topSongs"].get("song").is_none());
}

#[tokio::test]
async fn top_songs_edge_cases() {
	let (mut service, key) = library_service(&test_name!()).await;
	let h = hunted(&mut service, &key).await;
	// Nothing played yet, and an unknown name, are not errors
	assert!(
		call_ok(&mut service, &key, "getTopSongs", "artist=Khemmis").await["topSongs"]
			.get("song")
			.is_none()
	);
	assert!(
		call_ok(&mut service, &key, "getTopSongs", "artist=Nobody").await["topSongs"]
			.get("song")
			.is_none()
	);
	assert_eq!(
		call_failing(&mut service, &key, "getTopSongs", "").await,
		10
	);
	assert_eq!(
		call_failing(&mut service, &key, "getTopSongs", "id=ar_unknown").await,
		70
	);
	assert_eq!(
		call_failing(
			&mut service,
			&key,
			"getTopSongs",
			&format!("id={}", h.album)
		)
		.await,
		70
	);
	assert_eq!(
		call_failing(&mut service, &key, "getTopSongs", "artist=Khemmis&count=-1").await,
		0
	);
	// The ID wins when both are given
	call_ok(
		&mut service,
		&key,
		"scrobble",
		&format!("id={}", h.songs[0]),
	)
	.await;
	let both = call_ok(
		&mut service,
		&key,
		"getTopSongs",
		&format!("artist=Tobokegao&id={}", h.artist),
	)
	.await;
	assert_eq!(both["topSongs"]["song"][0]["id"], h.songs[0].as_str());
}

#[tokio::test]
async fn the_new_extensions_are_advertised() {
	let (mut service, _) = library_service(&test_name!()).await;
	let reply = fetch(&mut service, "/rest/getOpenSubsonicExtensions?f=json").await;
	let json = reply.json();
	let names: Vec<&str> = json["subsonic-response"]["openSubsonicExtensions"]
		.as_array()
		.unwrap()
		.iter()
		.map(|e| e["name"].as_str().unwrap())
		.collect();
	for expected in [
		"apiKeyAuthentication",
		"formPost",
		"playbackReport",
		"topSongsByArtistId",
	] {
		assert!(names.contains(&expected), "{expected} in {names:?}");
	}
}
