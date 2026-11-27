//! Tags read from files (BPM, MusicBrainz IDs, ReplayGain) and lyrics.

use super::*;

/// The song with this title, from the tagged files.
async fn find_song(service: &mut ServiceType, key: &str, title: &str) -> Value {
	let query = title.replace(' ', "%20");
	let response = call_ok(
		service,
		key,
		"search3",
		&format!("query={query}&songCount=50"),
	)
	.await;
	response["searchResult3"]["song"]
		.as_array()
		.unwrap()
		.iter()
		.find(|s| s["title"] == title)
		.unwrap_or_else(|| panic!("no song {title}"))
		.clone()
}

#[tokio::test]
async fn songs_report_the_tags_of_their_files() {
	let (mut service, key) = tagged_service(&test_name!()).await;

	let flac = find_song(&mut service, &key, "Tagged Song").await;
	assert_eq!(flac["bpm"], 128);
	assert_eq!(
		flac["musicBrainzId"],
		"11111111-1111-4111-8111-111111111111"
	);
	let gain = &flac["replayGain"];
	assert_eq!(gain["trackGain"], -6.5);
	assert_eq!(gain["albumGain"], -5.1);
	assert_eq!(gain["trackPeak"], 0.987654);
	assert_eq!(gain["albumPeak"], 1.0);
	assert_eq!(flac["contentType"], "audio/flac");
	assert_eq!(flac["suffix"], "flac");

	let mp3 = find_song(&mut service, &key, "Embedded Lyrics").await;
	assert_eq!(mp3["bpm"], 90);
	assert!(mp3.get("replayGain").is_none());
	assert!(mp3.get("musicBrainzId").is_none());

	let plain = find_song(&mut service, &key, "Plain Song").await;
	for field in ["bpm", "replayGain", "musicBrainzId"] {
		assert!(plain.get(field).is_none(), "{field}");
	}
}

#[tokio::test]
async fn tags_show_up_in_every_listing_of_songs() {
	let (mut service, key) = tagged_service(&test_name!()).await;
	let flac = find_song(&mut service, &key, "Tagged Song").await;
	let id = flac["id"].as_str().unwrap();

	let by_id = call_ok(&mut service, &key, "getSong", &format!("id={id}")).await;
	assert_eq!(by_id["song"]["replayGain"], flac["replayGain"]);
	let random = call_ok(&mut service, &key, "getRandomSongs", "size=500").await;
	let from_random = random["randomSongs"]["song"]
		.as_array()
		.unwrap()
		.iter()
		.find(|s| s["id"] == id)
		.unwrap();
	assert_eq!(from_random["bpm"], 128);
}

#[tokio::test]
async fn albums_report_the_musicbrainz_id_of_their_songs() {
	let (mut service, key) = tagged_service(&test_name!()).await;
	let flac = find_song(&mut service, &key, "Tagged Song").await;
	let album = flac["albumId"].as_str().unwrap();
	let response = call_ok(&mut service, &key, "getAlbum", &format!("id={album}")).await;
	assert_eq!(response["album"]["songCount"], 4);
	assert_eq!(
		response["album"]["musicBrainzId"],
		"22222222-2222-4222-8222-222222222222"
	);

	// An album whose files have none reports none
	let hunted = call_ok(
		&mut service,
		&key,
		"getAlbumList2",
		"type=alphabeticalByName&size=500",
	)
	.await;
	let hunted = hunted["albumList2"]["album"]
		.as_array()
		.unwrap()
		.iter()
		.find(|a| a["name"] == "Hunted")
		.unwrap()["id"]
		.as_str()
		.unwrap()
		.to_owned();
	let response = call_ok(&mut service, &key, "getAlbum", &format!("id={hunted}")).await;
	assert!(response["album"].get("musicBrainzId").is_none());
}

#[tokio::test]
async fn lyrics_in_an_lrc_file_are_synced() {
	let (mut service, key) = tagged_service(&test_name!()).await;
	let song = find_song(&mut service, &key, "Synced Lyrics").await;
	let id = song["id"].as_str().unwrap();

	let response = call_ok(&mut service, &key, "getLyricsBySongId", &format!("id={id}")).await;
	let lists = response["lyricsList"]["structuredLyrics"]
		.as_array()
		.unwrap();
	assert_eq!(lists.len(), 1);
	let lyrics = &lists[0];
	assert_eq!(lyrics["synced"], true);
	assert_eq!(lyrics["lang"], "xxx");
	assert_eq!(lyrics["displayArtist"], "Tag Artist");
	assert_eq!(lyrics["displayTitle"], "Synced Lyrics");
	assert!(lyrics.get("offset").is_none(), "no offset means zero");
	let lines: Vec<(i64, &str)> = lyrics["line"]
		.as_array()
		.unwrap()
		.iter()
		.map(|l| (l["start"].as_i64().unwrap(), l["value"].as_str().unwrap()))
		.collect();
	assert_eq!(
		lines,
		vec![
			(1500, "First synced line"),
			(3000, "Second synced line"),
			(4250, "Repeated line"),
			(9000, "Repeated line"),
		]
	);
}

#[tokio::test]
async fn lyrics_in_tags_are_not_synced() {
	let (mut service, key) = tagged_service(&test_name!()).await;
	for (title, expected) in [
		("Embedded Lyrics", vec!["Embedded one", "Embedded two"]),
		("Tagged Song", vec!["First line", "Second line"]),
	] {
		let song = find_song(&mut service, &key, title).await;
		let id = song["id"].as_str().unwrap();
		let response = call_ok(&mut service, &key, "getLyricsBySongId", &format!("id={id}")).await;
		let lists = response["lyricsList"]["structuredLyrics"]
			.as_array()
			.unwrap();
		assert_eq!(lists.len(), 1, "{title}");
		assert_eq!(lists[0]["synced"], false);
		let lines = lists[0]["line"].as_array().unwrap();
		assert_eq!(
			lines
				.iter()
				.map(|l| l["value"].as_str().unwrap())
				.collect::<Vec<_>>(),
			expected
		);
		assert!(
			lines.iter().all(|l| l.get("start").is_none()),
			"unsynced lines have no start"
		);
	}
}

#[tokio::test]
async fn songs_without_lyrics_have_an_empty_list() {
	let (mut service, key) = tagged_service(&test_name!()).await;
	let song = find_song(&mut service, &key, "Plain Song").await;
	let id = song["id"].as_str().unwrap();
	let response = call_ok(&mut service, &key, "getLyricsBySongId", &format!("id={id}")).await;
	assert!(response["lyricsList"].get("structuredLyrics").is_none());

	// ...and so do the songs of a collection with no lyrics at all
	let hunted = call_ok(&mut service, &key, "getRandomSongs", "size=500").await;
	let id = hunted["randomSongs"]["song"]
		.as_array()
		.unwrap()
		.iter()
		.find(|s| s["album"] == "Hunted")
		.unwrap()["id"]
		.as_str()
		.unwrap()
		.to_owned();
	let response = call_ok(&mut service, &key, "getLyricsBySongId", &format!("id={id}")).await;
	assert!(response["lyricsList"].get("structuredLyrics").is_none());
}

#[tokio::test]
async fn lyrics_requests_are_validated() {
	let (mut service, key) = tagged_service(&test_name!()).await;
	let song = find_song(&mut service, &key, "Synced Lyrics").await;
	let id = song["id"].as_str().unwrap();
	assert_eq!(
		call_failing(&mut service, &key, "getLyricsBySongId", "").await,
		10
	);
	assert_eq!(
		call_failing(&mut service, &key, "getLyricsBySongId", "id=so_unknown").await,
		70
	);
	assert_eq!(
		call_failing(
			&mut service,
			&key,
			"getLyricsBySongId",
			&format!("id={}", song["albumId"].as_str().unwrap())
		)
		.await,
		70
	);
	assert_eq!(
		call_failing(
			&mut service,
			&key,
			"getLyricsBySongId",
			&format!("id={id}&enhanced=maybe")
		)
		.await,
		0
	);
	// The enhanced form is accepted and answered in the basic form
	let response = call_ok(
		&mut service,
		&key,
		"getLyricsBySongId",
		&format!("id={id}&enhanced=true"),
	)
	.await;
	assert_eq!(
		response["lyricsList"]["structuredLyrics"]
			.as_array()
			.unwrap()
			.len(),
		1
	);
	assert!(response["lyricsList"]["structuredLyrics"][0]
		.get("cueLine")
		.is_none());
}

#[tokio::test]
async fn the_legacy_lyrics_endpoint_finds_a_song_by_artist_and_title() {
	let (mut service, key) = tagged_service(&test_name!()).await;
	let response = call_ok(
		&mut service,
		&key,
		"getLyrics",
		"artist=Tag%20Artist&title=Synced%20Lyrics",
	)
	.await;
	assert_eq!(
		response["lyrics"]["value"],
		"First synced line\nSecond synced line\nRepeated line\nRepeated line"
	);
	assert_eq!(response["lyrics"]["artist"], "Tag Artist");
	assert_eq!(response["lyrics"]["title"], "Synced Lyrics");

	// Case does not matter, and the artist is optional
	let response = call_ok(&mut service, &key, "getLyrics", "title=EMBEDDED%20lyrics").await;
	assert_eq!(response["lyrics"]["value"], "Embedded one\nEmbedded two");
	assert!(response["lyrics"].get("artist").is_none());

	// No such song, no lyrics, the wrong artist, and no question at all: an empty answer
	for query in [
		"artist=Nobody&title=Nothing",
		"artist=Tag%20Artist&title=Plain%20Song",
		"artist=Wrong%20Artist&title=Synced%20Lyrics",
		"",
	] {
		let response = call_ok(&mut service, &key, "getLyrics", query).await;
		assert_eq!(response["lyrics"]["value"], "", "{query}");
	}
}

#[tokio::test]
async fn lyrics_are_described_in_xml_too() {
	let (mut service, key) = tagged_service(&test_name!()).await;
	let song = find_song(&mut service, &key, "Synced Lyrics").await;
	let id = song["id"].as_str().unwrap();

	let reply = fetch(
		&mut service,
		&format!("/rest/getLyricsBySongId?apiKey={key}&v={VERSION}&c=t&id={id}"),
	)
	.await;
	let body = reply.text();
	let doc = roxmltree::Document::parse(&body).unwrap();
	let lyrics = doc
		.descendants()
		.find(|n| n.has_tag_name("structuredLyrics"))
		.unwrap();
	assert_eq!(lyrics.attribute("synced"), Some("true"));
	let lines: Vec<_> = lyrics
		.children()
		.filter(|n| n.has_tag_name("line"))
		.collect();
	assert_eq!(lines.len(), 4);
	assert_eq!(lines[0].attribute("start"), Some("1500"));
	assert_eq!(lines[0].text(), Some("First synced line"));

	let reply = fetch(
		&mut service,
		&format!("/rest/getLyrics?apiKey={key}&v={VERSION}&c=t&title=Embedded%20Lyrics"),
	)
	.await;
	let body = reply.text();
	let doc = roxmltree::Document::parse(&body).unwrap();
	let lyrics = doc
		.descendants()
		.find(|n| n.has_tag_name("lyrics"))
		.unwrap();
	assert_eq!(lyrics.text(), Some("Embedded one\nEmbedded two"));
}

#[tokio::test]
async fn the_lyrics_extension_is_advertised() {
	let (mut service, _) = library_service(&test_name!()).await;
	let json = fetch(&mut service, "/rest/getOpenSubsonicExtensions?f=json")
		.await
		.json();
	let extension = json["subsonic-response"]["openSubsonicExtensions"]
		.as_array()
		.unwrap()
		.iter()
		.find(|e| e["name"] == "songLyrics")
		.expect("songLyrics");
	assert_eq!(extension["versions"], serde_json::json!([1]));
}
