//! Browsing the collection: folders, the folder-based index, directories, artists, albums,
//! songs and genres.

use super::*;

async fn artist_id(service: &mut ServiceType, key: &str, name: &str) -> String {
	let response = call_ok(service, key, "getArtists", "").await;
	response["artists"]["index"]
		.as_array()
		.unwrap()
		.iter()
		.flat_map(|i| i["artist"].as_array().unwrap())
		.find(|a| a["name"] == name)
		.unwrap_or_else(|| panic!("no artist {name}"))["id"]
		.as_str()
		.unwrap()
		.to_owned()
}

async fn album_id(service: &mut ServiceType, key: &str, artist: &str, album: &str) -> String {
	let id = artist_id(service, key, artist).await;
	let response = call_ok(service, key, "getArtist", &format!("id={id}")).await;
	response["artist"]["album"]
		.as_array()
		.unwrap()
		.iter()
		.find(|a| a["name"] == album)
		.unwrap_or_else(|| panic!("no album {album}"))["id"]
		.as_str()
		.unwrap()
		.to_owned()
}

#[tokio::test]
async fn music_folders() {
	let (mut service, key) = library_service(&test_name!()).await;
	let response = call_ok(&mut service, &key, "getMusicFolders", "").await;
	let folders = response["musicFolders"]["musicFolder"].as_array().unwrap();
	assert_eq!(folders.len(), 1);
	assert_eq!(folders[0]["name"], "collection");
	assert!(folders[0]["id"].is_i64());
}

#[tokio::test]
async fn folder_ids_are_stable_integers() {
	let (mut service, key) = library_service(&test_name!()).await;
	let first = call_ok(&mut service, &key, "getMusicFolders", "").await;
	let second = call_ok(&mut service, &key, "getMusicFolders", "").await;
	assert_eq!(first["musicFolders"], second["musicFolders"]);
}

#[tokio::test]
async fn indexes_list_the_top_level_folders_of_each_music_folder() {
	let (mut service, key) = library_service(&test_name!()).await;
	let response = call_ok(&mut service, &key, "getIndexes", "").await;
	let indexes = &response["indexes"];
	assert_eq!(indexes["ignoredArticles"], "The El La Los Las Le Les");
	assert!(indexes["lastModified"].as_i64().unwrap() > 0);

	let groups = indexes["index"].as_array().unwrap();
	let letters: Vec<&str> = groups.iter().map(|g| g["name"].as_str().unwrap()).collect();
	assert_eq!(letters, vec!["K", "T"]);
	assert_eq!(groups[0]["artist"][0]["name"], "Khemmis");
	assert_eq!(groups[1]["artist"][0]["name"], "Tobokegao");
}

#[tokio::test]
async fn indexes_can_be_limited_to_a_music_folder_and_skipped_when_unchanged() {
	let (mut service, key) = library_service(&test_name!()).await;
	let folders = call_ok(&mut service, &key, "getMusicFolders", "").await;
	let folder_id = folders["musicFolders"]["musicFolder"][0]["id"]
		.as_i64()
		.unwrap();

	let response = call_ok(
		&mut service,
		&key,
		"getIndexes",
		&format!("musicFolderId={folder_id}"),
	)
	.await;
	assert_eq!(response["indexes"]["index"].as_array().unwrap().len(), 2);

	let code = call_failing(&mut service, &key, "getIndexes", "musicFolderId=12345").await;
	assert_eq!(code, 70);
	let code = call_failing(&mut service, &key, "getIndexes", "musicFolderId=abc").await;
	assert_eq!(code, 0);

	// A client that already has the index gets no entries
	let last_modified = call_ok(&mut service, &key, "getIndexes", "").await["indexes"]
		["lastModified"]
		.as_i64()
		.unwrap();
	let response = call_ok(
		&mut service,
		&key,
		"getIndexes",
		&format!("ifModifiedSince={}", last_modified + 1),
	)
	.await;
	assert!(response["indexes"].get("index").is_none());
	assert_eq!(response["indexes"]["lastModified"], last_modified);
	let response = call_ok(&mut service, &key, "getIndexes", "ifModifiedSince=0").await;
	assert!(response["indexes"]["index"].is_array());
}

#[tokio::test]
async fn directories_can_be_walked_from_the_index_down_to_the_songs() {
	let (mut service, key) = library_service(&test_name!()).await;
	let indexes = call_ok(&mut service, &key, "getIndexes", "").await;
	let artist = indexes["indexes"]["index"][0]["artist"][0].clone();
	assert_eq!(artist["name"], "Khemmis");
	let artist_dir = artist["id"].as_str().unwrap();

	let response = call_ok(
		&mut service,
		&key,
		"getMusicDirectory",
		&format!("id={artist_dir}"),
	)
	.await;
	let directory = &response["directory"];
	assert_eq!(directory["id"], artist_dir);
	assert_eq!(directory["name"], "Khemmis");
	let children = directory["child"].as_array().unwrap();
	assert_eq!(children.len(), 1);
	assert_eq!(children[0]["isDir"], true);
	assert_eq!(children[0]["title"], "Hunted");
	assert_eq!(children[0]["parent"], artist_dir);
	let album_dir = children[0]["id"].as_str().unwrap();

	let response = call_ok(
		&mut service,
		&key,
		"getMusicDirectory",
		&format!("id={album_dir}"),
	)
	.await;
	let directory = &response["directory"];
	assert_eq!(directory["parent"], artist_dir);
	let songs = directory["child"].as_array().unwrap();
	assert_eq!(songs.len(), 5);
	for song in songs {
		assert_eq!(song["isDir"], false);
		assert_eq!(song["parent"], album_dir);
		assert_eq!(song["contentType"], "audio/mpeg");
	}
}

#[tokio::test]
async fn directories_also_open_albums_and_artists() {
	let (mut service, key) = library_service(&test_name!()).await;
	let album = album_id(&mut service, &key, "Khemmis", "Hunted").await;
	let response = call_ok(
		&mut service,
		&key,
		"getMusicDirectory",
		&format!("id={album}"),
	)
	.await;
	assert_eq!(response["directory"]["name"], "Hunted");
	assert_eq!(response["directory"]["child"].as_array().unwrap().len(), 5);

	let artist = artist_id(&mut service, &key, "Tobokegao").await;
	let response = call_ok(
		&mut service,
		&key,
		"getMusicDirectory",
		&format!("id={artist}"),
	)
	.await;
	let albums = response["directory"]["child"].as_array().unwrap();
	assert_eq!(albums.len(), 2);
	assert!(albums.iter().all(|a| a["isDir"] == true));
}

#[tokio::test]
async fn artists_are_grouped_into_an_index() {
	let (mut service, key) = library_service(&test_name!()).await;
	let response = call_ok(&mut service, &key, "getArtists", "").await;
	let artists = &response["artists"];
	assert_eq!(artists["ignoredArticles"], "The El La Los Las Le Les");
	let all: Vec<(&str, i64)> = artists["index"]
		.as_array()
		.unwrap()
		.iter()
		.flat_map(|i| i["artist"].as_array().unwrap())
		.map(|a| {
			(
				a["name"].as_str().unwrap(),
				a["albumCount"].as_i64().unwrap(),
			)
		})
		.collect();
	assert!(all.contains(&("Khemmis", 1)));
	assert!(all.contains(&("Tobokegao", 2)));
}

#[tokio::test]
async fn an_artist_lists_its_albums() {
	let (mut service, key) = library_service(&test_name!()).await;
	let id = artist_id(&mut service, &key, "Tobokegao").await;
	let response = call_ok(&mut service, &key, "getArtist", &format!("id={id}")).await;
	let artist = &response["artist"];
	assert_eq!(artist["id"], id);
	assert_eq!(artist["name"], "Tobokegao");
	assert_eq!(artist["albumCount"], 2);
	assert!(artist["coverArt"].is_string());

	let albums = artist["album"].as_array().unwrap();
	let picnic = albums.iter().find(|a| a["name"] == "Picnic").unwrap();
	assert_eq!(picnic["songCount"], 7);
	assert_eq!(picnic["artist"], "Tobokegao");
	assert_eq!(picnic["artistId"], id);
	assert!(picnic["duration"].as_i64().unwrap() >= 0);
	assert!(picnic["created"].as_str().unwrap().ends_with('Z'));
	let remixes = albums
		.iter()
		.find(|a| a["name"] == "Picnic (Remixes)")
		.unwrap();
	assert_eq!(remixes["songCount"], 1);
}

#[tokio::test]
async fn an_album_lists_its_songs_in_order() {
	let (mut service, key) = library_service(&test_name!()).await;
	let id = album_id(&mut service, &key, "Khemmis", "Hunted").await;
	let response = call_ok(&mut service, &key, "getAlbum", &format!("id={id}")).await;
	let album = &response["album"];
	assert_eq!(album["id"], id);
	assert_eq!(album["name"], "Hunted");
	assert_eq!(album["songCount"], 5);
	assert_eq!(album["artist"], "Khemmis");
	assert_eq!(album["coverArt"], id, "the album has a Folder.jpg");

	let songs = album["song"].as_array().unwrap();
	assert_eq!(songs.len(), 5);
	let titles: Vec<&str> = songs.iter().map(|s| s["title"].as_str().unwrap()).collect();
	assert_eq!(
		titles,
		vec![
			"Above The Water",
			"Candlelight",
			"Three Gates",
			"Beyond The Door",
			"Hunted"
		]
	);
	let tracks: Vec<i64> = songs.iter().map(|s| s["track"].as_i64().unwrap()).collect();
	assert_eq!(tracks, vec![1, 2, 3, 4, 5]);

	let song = &songs[0];
	assert_eq!(song["album"], "Hunted");
	assert_eq!(song["albumId"], id);
	assert_eq!(song["artist"], "Khemmis");
	assert_eq!(song["contentType"], "audio/mpeg");
	assert_eq!(song["suffix"], "mp3");
	assert_eq!(song["type"], "music");
	assert_eq!(song["isVideo"], false);
	assert!(song["size"].as_i64().unwrap() > 0);
	assert!(song["path"]
		.as_str()
		.unwrap()
		.starts_with("collection/Khemmis/Hunted/"));
	assert_eq!(song["artists"][0]["name"], "Khemmis");
	assert_eq!(song["artistId"], song["artists"][0]["id"]);
}

#[tokio::test]
async fn a_song_can_be_fetched_on_its_own() {
	let (mut service, key) = library_service(&test_name!()).await;
	let album = album_id(&mut service, &key, "Khemmis", "Hunted").await;
	let album_response = call_ok(&mut service, &key, "getAlbum", &format!("id={album}")).await;
	let expected = &album_response["album"]["song"][2];

	let id = expected["id"].as_str().unwrap();
	let response = call_ok(&mut service, &key, "getSong", &format!("id={id}")).await;
	assert_eq!(&response["song"], expected);
	assert_eq!(response["song"]["title"], "Three Gates");
}

#[tokio::test]
async fn songs_lead_back_to_their_directory_and_album() {
	let (mut service, key) = library_service(&test_name!()).await;
	let album = album_id(&mut service, &key, "Khemmis", "Hunted").await;
	let response = call_ok(&mut service, &key, "getAlbum", &format!("id={album}")).await;
	let parent = response["album"]["song"][0]["parent"]
		.as_str()
		.unwrap()
		.to_owned();

	let directory = call_ok(
		&mut service,
		&key,
		"getMusicDirectory",
		&format!("id={parent}"),
	)
	.await;
	assert_eq!(directory["directory"]["name"], "Hunted");
	assert_eq!(directory["directory"]["child"].as_array().unwrap().len(), 5);
}

#[tokio::test]
async fn genres_are_listed_with_counts() {
	let (mut service, key) = library_service(&test_name!()).await;
	let response = call_ok(&mut service, &key, "getGenres", "").await;
	assert!(response["genres"].is_object());
	for genre in response["genres"]
		.get("genre")
		.and_then(|g| g.as_array())
		.cloned()
		.unwrap_or_default()
	{
		assert!(genre["songCount"].as_i64().unwrap() >= 1);
		assert!(genre["albumCount"].as_i64().unwrap() >= 1);
	}
}

#[tokio::test]
async fn ids_are_stable_across_requests_and_rescans() {
	let (mut service, key) = library_service(&test_name!()).await;
	let before = album_id(&mut service, &key, "Khemmis", "Hunted").await;
	let artist_before = artist_id(&mut service, &key, "Khemmis").await;
	let songs_before = call_ok(&mut service, &key, "getAlbum", &format!("id={before}")).await
		["album"]["song"]
		.clone();

	service.login_admin().await;
	service.index().await;
	service.logout().await;

	assert_eq!(
		album_id(&mut service, &key, "Khemmis", "Hunted").await,
		before
	);
	assert_eq!(
		artist_id(&mut service, &key, "Khemmis").await,
		artist_before
	);
	let songs_after = call_ok(&mut service, &key, "getAlbum", &format!("id={before}")).await
		["album"]["song"]
		.clone();
	assert_eq!(songs_after, songs_before);
}

#[tokio::test]
async fn ids_are_distinct_per_kind_and_object() {
	let (mut service, key) = library_service(&test_name!()).await;
	let album = album_id(&mut service, &key, "Khemmis", "Hunted").await;
	let artist = artist_id(&mut service, &key, "Khemmis").await;
	assert_ne!(album, artist);
	assert!(album.starts_with("al_"));
	assert!(artist.starts_with("ar_"));
	let response = call_ok(&mut service, &key, "getAlbum", &format!("id={album}")).await;
	let ids: std::collections::HashSet<&str> = response["album"]["song"]
		.as_array()
		.unwrap()
		.iter()
		.map(|s| s["id"].as_str().unwrap())
		.collect();
	assert_eq!(ids.len(), 5);
	assert!(ids.iter().all(|id| id.starts_with("so_")));
}

#[tokio::test]
async fn unknown_missing_and_mismatched_ids_are_errors() {
	let (mut service, key) = library_service(&test_name!()).await;
	let album = album_id(&mut service, &key, "Khemmis", "Hunted").await;
	let artist = artist_id(&mut service, &key, "Khemmis").await;
	let song = call_ok(&mut service, &key, "getAlbum", &format!("id={album}")).await["album"]
		["song"][0]["id"]
		.as_str()
		.unwrap()
		.to_owned();

	for endpoint in ["getAlbum", "getArtist", "getSong", "getMusicDirectory"] {
		assert_eq!(
			call_failing(&mut service, &key, endpoint, "id=al_unknown").await,
			70,
			"{endpoint}"
		);
		assert_eq!(
			call_failing(&mut service, &key, endpoint, "id=").await,
			10,
			"{endpoint}"
		);
		assert_eq!(
			call_failing(&mut service, &key, endpoint, "").await,
			10,
			"{endpoint}"
		);
	}
	// The right kind of ID is required
	assert_eq!(
		call_failing(&mut service, &key, "getAlbum", &format!("id={artist}")).await,
		70
	);
	assert_eq!(
		call_failing(&mut service, &key, "getAlbum", &format!("id={song}")).await,
		70
	);
	assert_eq!(
		call_failing(&mut service, &key, "getArtist", &format!("id={album}")).await,
		70
	);
	assert_eq!(
		call_failing(&mut service, &key, "getSong", &format!("id={album}")).await,
		70
	);
	// IDs are case sensitive
	let shouted = album.to_uppercase();
	if shouted != album {
		assert_eq!(
			call_failing(&mut service, &key, "getAlbum", &format!("id={shouted}")).await,
			70
		);
	}
}

#[tokio::test]
async fn information_endpoints_answer_with_empty_descriptions() {
	let (mut service, key) = library_service(&test_name!()).await;
	let album = album_id(&mut service, &key, "Khemmis", "Hunted").await;
	let artist = artist_id(&mut service, &key, "Khemmis").await;

	let response = call_ok(
		&mut service,
		&key,
		"getArtistInfo2",
		&format!("id={artist}"),
	)
	.await;
	assert!(response["artistInfo2"].is_object());
	let response = call_ok(&mut service, &key, "getArtistInfo", &format!("id={artist}")).await;
	assert!(response["artistInfo"].is_object());
	let response = call_ok(&mut service, &key, "getAlbumInfo2", &format!("id={album}")).await;
	assert!(response["albumInfo"].is_object());
	let response = call_ok(
		&mut service,
		&key,
		"getSimilarSongs2",
		&format!("id={artist}"),
	)
	.await;
	assert!(response["similarSongs2"].get("song").is_none());
	assert_eq!(
		call_failing(&mut service, &key, "getArtistInfo2", "id=ar_unknown").await,
		70
	);
}

#[tokio::test]
async fn browsing_needs_authentication() {
	let (mut service, _) = library_service(&test_name!()).await;
	for endpoint in [
		"getArtists",
		"getAlbum",
		"getMusicFolders",
		"getIndexes",
		"getGenres",
	] {
		let reply = fetch(
			&mut service,
			&format!("/rest/{endpoint}?v={VERSION}&c=t&f=json&id=x"),
		)
		.await;
		assert_eq!(error_code(&reply.json()), 10, "{endpoint}");
		let reply = fetch(
			&mut service,
			&format!("/rest/{endpoint}?apiKey=wrong&v={VERSION}&c=t&f=json&id=x"),
		)
		.await;
		assert_eq!(error_code(&reply.json()), 44, "{endpoint}");
	}
}

#[tokio::test]
async fn albums_are_described_in_xml_too() {
	let (mut service, key) = library_service(&test_name!()).await;
	let id = album_id(&mut service, &key, "Khemmis", "Hunted").await;
	let uri = format!("/rest/getAlbum?apiKey={key}&v={VERSION}&c=t&id={id}");
	let reply = fetch(&mut service, &uri).await;
	assert!(reply.content_type.starts_with("application/xml"));
	let body = reply.text();
	let doc = roxmltree::Document::parse(&body).unwrap();
	let album = doc
		.root_element()
		.children()
		.find(|n| n.has_tag_name("album"))
		.unwrap();
	assert_eq!(album.attribute("name"), Some("Hunted"));
	assert_eq!(album.attribute("songCount"), Some("5"));
	let songs: Vec<_> = album
		.children()
		.filter(|n| n.has_tag_name("song"))
		.collect();
	assert_eq!(songs.len(), 5);
	assert_eq!(songs[0].attribute("title"), Some("Above The Water"));
	assert_eq!(songs[0].attribute("isDir"), Some("false"));
	assert!(songs[0].children().any(|n| n.has_tag_name("artists")));
}
