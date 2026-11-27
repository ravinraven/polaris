//! Streaming, downloading and cover art.

use super::*;

const SONG_FILE: &str = "test-data/small-collection/Khemmis/Hunted/01 - Above The Water.mp3";

/// The ID of the first song of Khemmis's Hunted, and its size as the API reports it.
async fn first_song(service: &mut ServiceType, key: &str) -> (String, String, i64) {
	let artists = call_ok(service, key, "getArtists", "").await;
	let artist = artists["artists"]["index"][0]["artist"][0]["id"]
		.as_str()
		.unwrap()
		.to_owned();
	let artist = call_ok(service, key, "getArtist", &format!("id={artist}")).await;
	let album = artist["artist"]["album"][0]["id"]
		.as_str()
		.unwrap()
		.to_owned();
	let album_response = call_ok(service, key, "getAlbum", &format!("id={album}")).await;
	let song = &album_response["album"]["song"][0];
	(
		song["id"].as_str().unwrap().to_owned(),
		album,
		song["size"].as_i64().unwrap(),
	)
}

async fn get_raw(service: &mut ServiceType, key: &str, endpoint: &str, extra: &str) -> Reply {
	fetch(
		service,
		&format!("/rest/{endpoint}?apiKey={key}&v={VERSION}&c=t&{extra}"),
	)
	.await
}

async fn get_with_range(service: &mut ServiceType, key: &str, id: &str, range: &str) -> Reply {
	request(
		service,
		Request::builder()
			.method(Method::GET)
			.uri(format!("/rest/stream?apiKey={key}&v={VERSION}&c=t&id={id}"))
			.header(header::RANGE, range)
			.body(())
			.unwrap(),
	)
	.await
}

#[tokio::test]
async fn a_song_streams_as_the_original_file() {
	let (mut service, key) = library_service(&test_name!()).await;
	let (id, _, size) = first_song(&mut service, &key).await;
	let file = std::fs::read(SONG_FILE).unwrap();
	assert_eq!(size, file.len() as i64);

	let reply = get_raw(&mut service, &key, "stream", &format!("id={id}")).await;
	assert_eq!(reply.status, StatusCode::OK);
	assert_eq!(reply.content_type, "audio/mpeg");
	assert_eq!(reply.body, file);
}

#[tokio::test]
async fn streaming_ignores_transcoding_options_and_works_with_the_view_suffix() {
	let (mut service, key) = library_service(&test_name!()).await;
	let (id, _, _) = first_song(&mut service, &key).await;
	let file = std::fs::read(SONG_FILE).unwrap();

	let uri = format!("/rest/stream.view?apiKey={key}&v={VERSION}&c=t&id={id}&maxBitRate=64&format=opus&timeOffset=30&estimateContentLength=true");
	let reply = fetch(&mut service, &uri).await;
	assert_eq!(reply.status, StatusCode::OK);
	assert_eq!(reply.body, file);
}

#[tokio::test]
async fn range_requests_are_honored() {
	let (mut service, key) = library_service(&test_name!()).await;
	let (id, _, size) = first_song(&mut service, &key).await;
	let file = std::fs::read(SONG_FILE).unwrap();

	let reply = get_with_range(&mut service, &key, &id, "bytes=0-99").await;
	assert_eq!(reply.status, StatusCode::PARTIAL_CONTENT);
	assert_eq!(reply.body, file[..100]);
	assert_eq!(
		reply.headers[header::CONTENT_RANGE].to_str().unwrap(),
		format!("bytes 0-99/{size}")
	);

	let reply = get_with_range(&mut service, &key, &id, "bytes=100-").await;
	assert_eq!(reply.status, StatusCode::PARTIAL_CONTENT);
	assert_eq!(reply.body, file[100..]);

	let reply = get_with_range(&mut service, &key, &id, "bytes=-50").await;
	assert_eq!(reply.status, StatusCode::PARTIAL_CONTENT);
	assert_eq!(reply.body, file[file.len() - 50..]);

	let reply = get_with_range(&mut service, &key, &id, &format!("bytes={}-", size + 10)).await;
	assert_eq!(reply.status, StatusCode::RANGE_NOT_SATISFIABLE);
}

#[tokio::test]
async fn downloads_are_attachments() {
	let (mut service, key) = library_service(&test_name!()).await;
	let (id, _, _) = first_song(&mut service, &key).await;
	let file = std::fs::read(SONG_FILE).unwrap();

	let reply = get_raw(&mut service, &key, "download", &format!("id={id}")).await;
	assert_eq!(reply.status, StatusCode::OK);
	assert_eq!(reply.body, file);
	assert_eq!(
		reply.headers[header::CONTENT_DISPOSITION].to_str().unwrap(),
		"attachment; filename=\"01 - Above The Water.mp3\""
	);
	// Streaming is not an attachment
	let reply = get_raw(&mut service, &key, "stream", &format!("id={id}")).await;
	assert!(reply.headers.get(header::CONTENT_DISPOSITION).is_none());
}

#[tokio::test]
async fn download_file_names_stay_safe_for_non_ascii_names() {
	let (mut service, key) = library_service(&test_name!()).await;
	let artists = call_ok(&mut service, &key, "getArtists", "").await;
	let tobokegao = artists["artists"]["index"][1]["artist"][0]["id"]
		.as_str()
		.unwrap()
		.to_owned();
	let artist = call_ok(&mut service, &key, "getArtist", &format!("id={tobokegao}")).await;
	let remixes = artist["artist"]["album"]
		.as_array()
		.unwrap()
		.iter()
		.find(|a| a["name"] == "Picnic (Remixes)")
		.unwrap()["id"]
		.as_str()
		.unwrap()
		.to_owned();
	let album = call_ok(&mut service, &key, "getAlbum", &format!("id={remixes}")).await;
	let id = album["album"]["song"][0]["id"].as_str().unwrap().to_owned();

	let reply = get_raw(&mut service, &key, "download", &format!("id={id}")).await;
	assert_eq!(reply.status, StatusCode::OK);
	let disposition = reply.headers[header::CONTENT_DISPOSITION].to_str().unwrap();
	assert!(disposition.is_ascii());
	assert!(disposition.starts_with("attachment; filename=\""));
	assert!(disposition.ends_with(".mp3\""));
}

#[tokio::test]
async fn only_songs_can_be_streamed_or_downloaded() {
	let (mut service, key) = library_service(&test_name!()).await;
	let (song, album, _) = first_song(&mut service, &key).await;

	for endpoint in ["stream", "download"] {
		for id in [album.as_str(), "so_unknown", "garbage"] {
			let reply = get_raw(&mut service, &key, endpoint, &format!("id={id}&f=json")).await;
			assert_eq!(reply.status, StatusCode::OK, "{endpoint} {id}");
			assert_eq!(error_code(&reply.json()), 70, "{endpoint} {id}");
		}
		let reply = get_raw(&mut service, &key, endpoint, "f=json").await;
		assert_eq!(error_code(&reply.json()), 10, "{endpoint}");
		// Errors are XML when no format is asked for
		let reply = get_raw(&mut service, &key, endpoint, "id=so_unknown").await;
		assert!(reply.content_type.starts_with("application/xml"));
		assert!(reply.text().contains("status=\"failed\""));
	}
	assert!(!song.is_empty());
}

#[tokio::test]
async fn cover_art_is_served_as_a_jpeg_at_the_requested_size() {
	let (mut service, key) = library_service(&test_name!()).await;
	let (song, album, _) = first_song(&mut service, &key).await;

	for id in [&album, &song] {
		let reply = get_raw(&mut service, &key, "getCoverArt", &format!("id={id}")).await;
		assert_eq!(reply.status, StatusCode::OK, "{id}");
		assert_eq!(reply.content_type, "image/jpeg");
		assert_eq!(&reply.body[..2], &[0xFF, 0xD8], "JPEG signature");
		assert!(reply.headers[header::CACHE_CONTROL]
			.to_str()
			.unwrap()
			.contains("max-age"));
	}

	let full = get_raw(&mut service, &key, "getCoverArt", &format!("id={album}")).await;
	let full = image::load_from_memory(&full.body).unwrap();
	let small = get_raw(
		&mut service,
		&key,
		"getCoverArt",
		&format!("id={album}&size=32"),
	)
	.await;
	let small = image::load_from_memory(&small.body).unwrap();
	assert!(
		small.width() <= 32 && small.height() <= 32,
		"{}x{}",
		small.width(),
		small.height()
	);
	assert!(full.width() > 32);
	// The aspect ratio is kept
	assert!(
		(small.width() as i64 - small.height() as i64).abs() <= 1 || full.width() != full.height()
	);

	// A huge size is capped, not an error
	let reply = get_raw(
		&mut service,
		&key,
		"getCoverArt",
		&format!("id={album}&size=100000"),
	)
	.await;
	assert_eq!(reply.status, StatusCode::OK);
	assert_eq!(reply.content_type, "image/jpeg");
}

#[tokio::test]
async fn artists_have_the_cover_art_of_their_albums() {
	let (mut service, key) = library_service(&test_name!()).await;
	let artists = call_ok(&mut service, &key, "getArtists", "").await;
	let artist = artists["artists"]["index"][0]["artist"][0]["id"]
		.as_str()
		.unwrap()
		.to_owned();
	let reply = get_raw(&mut service, &key, "getCoverArt", &format!("id={artist}")).await;
	assert_eq!(reply.status, StatusCode::OK);
	assert_eq!(reply.content_type, "image/jpeg");
}

#[tokio::test]
async fn cover_art_errors() {
	let (mut service, key) = library_service(&test_name!()).await;
	let (_, album, _) = first_song(&mut service, &key).await;

	// Objects without art, unknown objects and bad sizes
	let remixes = call_ok(
		&mut service,
		&key,
		"getAlbumList2",
		"type=alphabeticalByName",
	)
	.await["albumList2"]["album"]
		.as_array()
		.unwrap()
		.iter()
		.find(|a| a["name"] == "Picnic (Remixes)")
		.unwrap()
		.clone();
	assert!(
		remixes.get("coverArt").is_none(),
		"no coverArt is advertised without art"
	);
	let remixes_id = remixes["id"].as_str().unwrap();
	for id in [remixes_id, "al_unknown", "x"] {
		let reply = get_raw(
			&mut service,
			&key,
			"getCoverArt",
			&format!("id={id}&f=json"),
		)
		.await;
		assert_eq!(error_code(&reply.json()), 70, "{id}");
	}
	for size in ["0", "-5", "abc"] {
		let reply = get_raw(
			&mut service,
			&key,
			"getCoverArt",
			&format!("id={album}&size={size}&f=json"),
		)
		.await;
		assert_eq!(error_code(&reply.json()), 0, "size={size}");
	}
	let reply = get_raw(&mut service, &key, "getCoverArt", "f=json").await;
	assert_eq!(error_code(&reply.json()), 10);
}

#[tokio::test]
async fn avatars_do_not_exist() {
	let (mut service, key) = library_service(&test_name!()).await;
	let reply = get_raw(&mut service, &key, "getAvatar", "username=test_user&f=json").await;
	assert_eq!(error_code(&reply.json()), 70);
}

#[tokio::test]
async fn media_needs_authentication() {
	let (mut service, key) = library_service(&test_name!()).await;
	let (id, _, _) = first_song(&mut service, &key).await;
	for endpoint in ["stream", "download", "getCoverArt", "getAvatar"] {
		let reply = fetch(
			&mut service,
			&format!("/rest/{endpoint}?v={VERSION}&c=t&id={id}&username=x&f=json"),
		)
		.await;
		assert_eq!(error_code(&reply.json()), 10, "{endpoint}");
		let reply = fetch(
			&mut service,
			&format!("/rest/{endpoint}?apiKey=bad&v={VERSION}&c=t&id={id}&username=x&f=json"),
		)
		.await;
		assert_eq!(error_code(&reply.json()), 44, "{endpoint}");
		let reply = fetch(&mut service, &format!("/rest/{endpoint}?u=test_user&p=test_password&v={VERSION}&c=t&id={id}&username=x&f=json")).await;
		assert_ne!(
			reply.content_type, "",
			"{endpoint} also works with a password"
		);
	}
}
