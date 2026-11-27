//! Album and song lists, and search.

use super::*;

fn names(list: &Value, key: &str) -> Vec<String> {
	list.get(key)
		.and_then(|v| v.as_array())
		.map(|items| {
			items
				.iter()
				.map(|i| {
					i["name"]
						.as_str()
						.or(i["title"].as_str())
						.unwrap()
						.to_owned()
				})
				.collect()
		})
		.unwrap_or_default()
}

#[tokio::test]
async fn album_lists_need_a_valid_type() {
	let (mut service, key) = library_service(&test_name!()).await;
	assert_eq!(
		call_failing(&mut service, &key, "getAlbumList2", "").await,
		10
	);
	assert_eq!(
		call_failing(&mut service, &key, "getAlbumList2", "type=").await,
		10
	);
	assert_eq!(
		call_failing(&mut service, &key, "getAlbumList2", "type=bogus").await,
		0
	);
	assert_eq!(
		call_failing(&mut service, &key, "getAlbumList2", "type=byYear").await,
		10
	);
	assert_eq!(
		call_failing(
			&mut service,
			&key,
			"getAlbumList2",
			"type=byYear&fromYear=2000"
		)
		.await,
		10
	);
	assert_eq!(
		call_failing(&mut service, &key, "getAlbumList2", "type=byGenre").await,
		10
	);
	for bad in ["size=-1", "size=x", "offset=-5", "offset=x"] {
		let extra = format!("type=newest&{bad}");
		assert_eq!(
			call_failing(&mut service, &key, "getAlbumList2", &extra).await,
			0,
			"{bad}"
		);
	}
}

#[tokio::test]
async fn albums_can_be_listed_by_name_and_by_artist() {
	let (mut service, key) = library_service(&test_name!()).await;
	let response = call_ok(
		&mut service,
		&key,
		"getAlbumList2",
		"type=alphabeticalByName&size=500",
	)
	.await;
	assert_eq!(
		names(&response["albumList2"], "album"),
		vec!["Hunted", "Picnic", "Picnic (Remixes)"]
	);

	let response = call_ok(
		&mut service,
		&key,
		"getAlbumList2",
		"type=alphabeticalByArtist",
	)
	.await;
	let artists: Vec<String> = response["albumList2"]["album"]
		.as_array()
		.unwrap()
		.iter()
		.map(|a| a["artist"].as_str().unwrap().to_owned())
		.collect();
	assert_eq!(artists, vec!["Khemmis", "Tobokegao", "Tobokegao"]);
}

#[tokio::test]
async fn album_lists_are_paged() {
	let (mut service, key) = library_service(&test_name!()).await;
	let all = call_ok(
		&mut service,
		&key,
		"getAlbumList2",
		"type=alphabeticalByName",
	)
	.await;
	let all = names(&all["albumList2"], "album");

	let page = call_ok(
		&mut service,
		&key,
		"getAlbumList2",
		"type=alphabeticalByName&size=2",
	)
	.await;
	assert_eq!(names(&page["albumList2"], "album"), all[..2].to_vec());
	let page = call_ok(
		&mut service,
		&key,
		"getAlbumList2",
		"type=alphabeticalByName&size=2&offset=2",
	)
	.await;
	assert_eq!(names(&page["albumList2"], "album"), all[2..].to_vec());
	let page = call_ok(
		&mut service,
		&key,
		"getAlbumList2",
		"type=alphabeticalByName&offset=99",
	)
	.await;
	assert!(names(&page["albumList2"], "album").is_empty());
	let page = call_ok(
		&mut service,
		&key,
		"getAlbumList2",
		"type=alphabeticalByName&size=0",
	)
	.await;
	assert!(names(&page["albumList2"], "album").is_empty());
	// The default page holds ten albums, and the size is capped
	let page = call_ok(
		&mut service,
		&key,
		"getAlbumList2",
		"type=newest&size=100000",
	)
	.await;
	assert_eq!(names(&page["albumList2"], "album").len(), 3);
}

#[tokio::test]
async fn random_and_newest_lists_return_every_album_once() {
	let (mut service, key) = library_service(&test_name!()).await;
	for list_type in ["random", "newest"] {
		let response = call_ok(
			&mut service,
			&key,
			"getAlbumList2",
			&format!("type={list_type}&size=10"),
		)
		.await;
		let mut found = names(&response["albumList2"], "album");
		found.sort();
		assert_eq!(
			found,
			vec!["Hunted", "Picnic", "Picnic (Remixes)"],
			"{list_type}"
		);
	}
}

#[tokio::test]
async fn albums_can_be_filtered_by_year_and_genre() {
	let (mut service, key) = library_service(&test_name!()).await;
	let all = call_ok(
		&mut service,
		&key,
		"getAlbumList2",
		"type=alphabeticalByName",
	)
	.await;
	let years: Vec<i64> = all["albumList2"]["album"]
		.as_array()
		.unwrap()
		.iter()
		.filter_map(|a| a["year"].as_i64())
		.collect();

	let nothing = call_ok(
		&mut service,
		&key,
		"getAlbumList2",
		"type=byYear&fromYear=1&toYear=2",
	)
	.await;
	assert!(names(&nothing["albumList2"], "album").is_empty());
	if let Some(year) = years.first() {
		let found = call_ok(
			&mut service,
			&key,
			"getAlbumList2",
			&format!("type=byYear&fromYear={year}&toYear={year}"),
		)
		.await;
		let found = found["albumList2"]["album"].as_array().unwrap();
		assert!(!found.is_empty());
		assert!(found.iter().all(|a| a["year"] == *year));
		// A backwards range means newest first, over the same albums
		let backwards = call_ok(
			&mut service,
			&key,
			"getAlbumList2",
			&format!("type=byYear&fromYear={}&toYear=1", year + 1),
		)
		.await;
		assert!(!backwards["albumList2"]["album"]
			.as_array()
			.unwrap()
			.is_empty());
	}

	let none = call_ok(
		&mut service,
		&key,
		"getAlbumList2",
		"type=byGenre&genre=Nonexistent",
	)
	.await;
	assert!(names(&none["albumList2"], "album").is_empty());
}

#[tokio::test]
async fn state_based_lists_are_empty_without_state() {
	let (mut service, key) = library_service(&test_name!()).await;
	for list_type in ["starred", "highest", "frequent", "recent"] {
		let response = call_ok(
			&mut service,
			&key,
			"getAlbumList2",
			&format!("type={list_type}"),
		)
		.await;
		assert!(
			names(&response["albumList2"], "album").is_empty(),
			"{list_type}"
		);
	}
}

#[tokio::test]
async fn the_legacy_album_list_describes_albums_as_directories() {
	let (mut service, key) = library_service(&test_name!()).await;
	let response = call_ok(
		&mut service,
		&key,
		"getAlbumList",
		"type=alphabeticalByName",
	)
	.await;
	let albums = response["albumList"]["album"].as_array().unwrap();
	assert_eq!(albums.len(), 3);
	assert!(albums.iter().all(|a| a["isDir"] == true));
	assert_eq!(albums[0]["title"], "Hunted");

	// Their IDs open as directories of songs
	let id = albums[0]["id"].as_str().unwrap();
	let directory = call_ok(&mut service, &key, "getMusicDirectory", &format!("id={id}")).await;
	assert_eq!(directory["directory"]["child"].as_array().unwrap().len(), 5);
}

#[tokio::test]
async fn random_songs() {
	let (mut service, key) = library_service(&test_name!()).await;
	let response = call_ok(&mut service, &key, "getRandomSongs", "size=4").await;
	let songs = response["randomSongs"]["song"].as_array().unwrap();
	assert_eq!(songs.len(), 4);
	assert!(songs.iter().all(|s| s["isDir"] == false));

	let response = call_ok(&mut service, &key, "getRandomSongs", "size=500").await;
	let ids: std::collections::HashSet<&str> = response["randomSongs"]["song"]
		.as_array()
		.unwrap()
		.iter()
		.map(|s| s["id"].as_str().unwrap())
		.collect();
	assert_eq!(ids.len(), 13, "every song once");

	let response = call_ok(&mut service, &key, "getRandomSongs", "genre=Nonexistent").await;
	assert!(response["randomSongs"].get("song").is_none());
	let response = call_ok(&mut service, &key, "getRandomSongs", "fromYear=1&toYear=2").await;
	assert!(response["randomSongs"].get("song").is_none());
	assert_eq!(
		call_failing(&mut service, &key, "getRandomSongs", "size=-1").await,
		0
	);
}

#[tokio::test]
async fn songs_by_genre() {
	let (mut service, key) = library_service(&test_name!()).await;
	assert_eq!(
		call_failing(&mut service, &key, "getSongsByGenre", "").await,
		10
	);
	let response = call_ok(&mut service, &key, "getSongsByGenre", "genre=Nonexistent").await;
	assert!(response["songsByGenre"].get("song").is_none());

	// Any genre the library has lists its songs, paged, whatever the case it is asked in
	let genres = call_ok(&mut service, &key, "getGenres", "").await;
	if let Some(genre) = genres["genres"]["genre"].as_array().and_then(|g| g.first()) {
		let name = genre["value"].as_str().unwrap().to_lowercase();
		let response = call_ok(
			&mut service,
			&key,
			"getSongsByGenre",
			&format!("genre={name}&count=500"),
		)
		.await;
		assert_eq!(
			response["songsByGenre"]["song"].as_array().unwrap().len() as i64,
			genre["songCount"].as_i64().unwrap()
		);
	}
}

#[tokio::test]
async fn search_finds_artists_albums_and_songs() {
	let (mut service, key) = library_service(&test_name!()).await;

	let response = call_ok(&mut service, &key, "search3", "query=khem").await;
	let result = &response["searchResult3"];
	assert_eq!(names(result, "artist"), vec!["Khemmis"]);
	assert_eq!(names(result, "album"), vec!["Hunted"]);
	assert!(result["artist"][0]["id"]
		.as_str()
		.unwrap()
		.starts_with("ar_"));

	let response = call_ok(&mut service, &key, "search3", "query=candlelight").await;
	let result = &response["searchResult3"];
	assert_eq!(names(result, "song"), vec!["Candlelight"]);
	assert!(result.get("artist").is_none());

	let response = call_ok(&mut service, &key, "search3", "query=PICNIC").await;
	assert_eq!(
		names(&response["searchResult3"], "album"),
		vec!["Picnic", "Picnic (Remixes)"]
	);

	let response = call_ok(&mut service, &key, "search3", "query=zzzzzz").await;
	assert!(response["searchResult3"].as_object().unwrap().is_empty());
}

#[tokio::test]
async fn an_empty_query_lists_everything_page_by_page() {
	let (mut service, key) = library_service(&test_name!()).await;
	for query in ["query=", "query=*", "query=%22%22"] {
		let response = call_ok(
			&mut service,
			&key,
			"search3",
			&format!("{query}&songCount=500"),
		)
		.await;
		let result = &response["searchResult3"];
		assert_eq!(result["song"].as_array().unwrap().len(), 13, "{query}");
		assert_eq!(result["album"].as_array().unwrap().len(), 3, "{query}");
		assert_eq!(result["artist"].as_array().unwrap().len(), 2, "{query}");
	}
	let page = call_ok(
		&mut service,
		&key,
		"search3",
		"query=&songCount=5&songOffset=10&albumCount=1&albumOffset=2&artistCount=0",
	)
	.await;
	let result = &page["searchResult3"];
	assert_eq!(result["song"].as_array().unwrap().len(), 3);
	assert_eq!(result["album"].as_array().unwrap().len(), 1);
	assert!(result.get("artist").is_none());

	assert_eq!(call_failing(&mut service, &key, "search3", "").await, 10);
	assert_eq!(
		call_failing(&mut service, &key, "search3", "query=a&songCount=-1").await,
		0
	);
}

#[tokio::test]
async fn search_text_cannot_be_used_to_inject_search_syntax() {
	let (mut service, key) = library_service(&test_name!()).await;
	// Quotes, operators and field names are all just text
	for query in [
		"a%22%20%7C%7C%20album%20%25%20%22",
		"title%20%25%20candlelight",
		"%28%28%28",
		"%22",
		"%26%26",
	] {
		let response = call(&mut service, &key, "search3", &format!("query={query}")).await;
		assert_eq!(response["status"], "ok", "{query}: {response}");
	}
}

#[tokio::test]
async fn the_legacy_search_describes_albums_as_directories() {
	let (mut service, key) = library_service(&test_name!()).await;
	let response = call_ok(&mut service, &key, "search2", "query=hunted").await;
	let result = &response["searchResult2"];
	assert!(result["album"]
		.as_array()
		.unwrap()
		.iter()
		.all(|a| a["isDir"] == true));
	assert!(result["song"].as_array().unwrap().len() >= 1);
}
