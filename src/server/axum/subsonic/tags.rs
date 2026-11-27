//! Tags that are read from the music files when they are asked for, instead of being kept in
//! the index: BPM, MusicBrainz IDs, ReplayGain and lyrics. Reading a file's tags is cheap, and
//! doing it on demand (with a cache) keeps the index format unchanged.

use std::{
	collections::HashMap,
	path::{Path, PathBuf},
	sync::{Arc, LazyLock, Mutex},
	time::SystemTime,
};

use symphonia::core::{
	formats::{probe::Hint, FormatOptions},
	io::{MediaSourceStream, MediaSourceStreamOptions},
	meta::{MetadataOptions, StandardTag},
};
use tokio::task::spawn_blocking;

/// How many files' tags are remembered before the cache starts over.
const MAX_CACHED_FILES: usize = 100_000;
/// Lyrics files larger than this are not read.
const MAX_LYRICS_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReplayGain {
	pub track_gain: Option<f64>,
	pub album_gain: Option<f64>,
	pub track_peak: Option<f64>,
	pub album_peak: Option<f64>,
}

impl ReplayGain {
	pub fn is_empty(&self) -> bool {
		*self == Self::default()
	}
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Extras {
	pub bpm: Option<i64>,
	pub musicbrainz_track_id: Option<String>,
	pub musicbrainz_album_id: Option<String>,
	pub replay_gain: ReplayGain,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LyricLine {
	/// Milliseconds from the start of the song, for synced lyrics.
	pub start: Option<i64>,
	pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Lyrics {
	pub synced: bool,
	pub lines: Vec<LyricLine>,
	pub artist: Option<String>,
	pub title: Option<String>,
	/// Milliseconds. Positive means the lyrics appear sooner.
	pub offset: i64,
}

impl Lyrics {
	pub fn text(&self) -> String {
		self.lines
			.iter()
			.map(|l| l.text.as_str())
			.collect::<Vec<_>>()
			.join("\n")
	}
}

struct FileTags {
	extras: Extras,
	lyrics: Option<String>,
}

/// A ReplayGain gain: a number of decibels, usually written with a trailing "dB".
fn parse_gain(text: &str) -> Option<f64> {
	let text = text.trim();
	let text = text
		.strip_suffix("dB")
		.or_else(|| text.strip_suffix("db"))
		.or_else(|| text.strip_suffix("DB"))
		.unwrap_or(text);
	text.trim().parse::<f64>().ok().filter(|g| g.is_finite())
}

fn parse_peak(text: &str) -> Option<f64> {
	text.trim()
		.parse::<f64>()
		.ok()
		.filter(|p| p.is_finite() && *p >= 0.0)
}

fn non_empty(text: &str) -> Option<String> {
	let text = text.trim();
	(!text.is_empty()).then(|| text.to_owned())
}

fn read_file_tags(path: &Path) -> Option<FileTags> {
	let file = std::fs::File::open(path).ok()?;
	let stream = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());
	let mut hint = Hint::new();
	if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
		hint.with_extension(extension);
	}
	let mut format = symphonia::default::get_probe()
		.probe(
			&hint,
			stream,
			FormatOptions::default(),
			MetadataOptions::default(),
		)
		.ok()?;

	let mut tags = FileTags {
		extras: Extras::default(),
		lyrics: None,
	};
	let metadata = format.metadata();
	let Some(revision) = metadata.current() else {
		return Some(tags);
	};
	let mut recording_id = None;
	for tag in &revision.media.tags {
		match &tag.std {
			Some(StandardTag::Bpm(bpm)) => tags.extras.bpm = i64::try_from(*bpm).ok(),
			Some(StandardTag::MusicBrainzRecordingId(id)) => recording_id = non_empty(id),
			Some(StandardTag::MusicBrainzTrackId(id)) => {
				tags.extras.musicbrainz_track_id = non_empty(id)
			}
			Some(StandardTag::MusicBrainzAlbumId(id)) => {
				tags.extras.musicbrainz_album_id = non_empty(id)
			}
			Some(StandardTag::ReplayGainTrackGain(g)) => {
				tags.extras.replay_gain.track_gain = parse_gain(g)
			}
			Some(StandardTag::ReplayGainAlbumGain(g)) => {
				tags.extras.replay_gain.album_gain = parse_gain(g)
			}
			Some(StandardTag::ReplayGainTrackPeak(p)) => {
				tags.extras.replay_gain.track_peak = parse_peak(p)
			}
			Some(StandardTag::ReplayGainAlbumPeak(p)) => {
				tags.extras.replay_gain.album_peak = parse_peak(p)
			}
			Some(StandardTag::Lyrics(text)) if tags.lyrics.is_none() => {
				tags.lyrics = non_empty(text)
			}
			_ => (),
		}
	}
	// The recording is the more precise identifier when a file has both
	if recording_id.is_some() {
		tags.extras.musicbrainz_track_id = recording_id;
	}
	Some(tags)
}

/// Tags by file, with the modification time they were read at.
type TagCache = HashMap<PathBuf, (SystemTime, Arc<Extras>)>;

static CACHE: LazyLock<Mutex<TagCache>> = LazyLock::new(Default::default);

/// The extra tags of a file. Files that cannot be read have none.
pub async fn extras(path: &Path) -> Arc<Extras> {
	let Some(modified) = tokio::fs::metadata(path)
		.await
		.ok()
		.and_then(|m| m.modified().ok())
	else {
		return Arc::default();
	};
	if let Some((cached_at, extras)) = CACHE.lock().unwrap().get(path) {
		if *cached_at == modified {
			return extras.clone();
		}
	}

	let owned = path.to_owned();
	let extras = Arc::new(
		spawn_blocking(move || read_file_tags(&owned).map(|t| t.extras))
			.await
			.ok()
			.flatten()
			.unwrap_or_default(),
	);
	let mut cache = CACHE.lock().unwrap();
	if cache.len() >= MAX_CACHED_FILES {
		cache.clear();
	}
	cache.insert(path.to_owned(), (modified, extras.clone()));
	extras
}

// Lyrics

/// `mm:ss`, `mm:ss.x`, `mm:ss.xx` or `mm:ss.xxx`, in milliseconds.
fn parse_timestamp(text: &str) -> Option<i64> {
	let (minutes, rest) = text.split_once(':')?;
	let (seconds, fraction) = match rest.split_once(['.', ':']) {
		Some((s, f)) => (s, f),
		None => (rest, ""),
	};
	let minutes: i64 = minutes.trim().parse().ok().filter(|m| *m >= 0)?;
	let seconds: i64 = seconds
		.trim()
		.parse()
		.ok()
		.filter(|s| (0..60).contains(s))?;
	if !fraction.chars().all(|c| c.is_ascii_digit()) || fraction.len() > 3 {
		return None;
	}
	let millis = match fraction.len() {
		0 => 0,
		1 => fraction.parse::<i64>().ok()? * 100,
		2 => fraction.parse::<i64>().ok()? * 10,
		_ => fraction.parse::<i64>().ok()?,
	};
	Some((minutes * 60 + seconds) * 1000 + millis)
}

/// Removes word timings (`<00:01.00>`) of enhanced LRC from a line of text.
fn strip_word_timings(text: &str) -> String {
	let mut out = String::with_capacity(text.len());
	let mut rest = text;
	while let Some(start) = rest.find('<') {
		match rest[start..].find('>') {
			Some(end) if parse_timestamp(&rest[start + 1..start + end]).is_some() => {
				out.push_str(&rest[..start]);
				rest = &rest[start + end + 1..];
			}
			_ => {
				out.push_str(&rest[..start + 1]);
				rest = &rest[start + 1..];
			}
		}
	}
	out.push_str(rest);
	out.trim().to_owned()
}

/// Parses text in the LRC format. Returns `None` if it has no timestamped lines.
pub fn parse_lrc(text: &str) -> Option<Lyrics> {
	let mut lines = Vec::new();
	let (mut artist, mut title, mut offset) = (None, None, 0);

	for raw in text.trim_start_matches('\u{feff}').lines() {
		let mut rest = raw.trim();
		let mut starts = Vec::new();
		let mut metadata = None;
		while let Some(inner) = rest.strip_prefix('[') {
			let Some(end) = inner.find(']') else { break };
			let tag = &inner[..end];
			match parse_timestamp(tag) {
				Some(start) => starts.push(start),
				None => {
					metadata = tag.split_once(':');
					rest = &inner[end + 1..];
					break;
				}
			}
			rest = &inner[end + 1..];
		}

		if let (true, Some((key, value))) = (starts.is_empty(), metadata) {
			match key.trim().to_ascii_lowercase().as_str() {
				"ar" => artist = non_empty(value),
				"ti" => title = non_empty(value),
				"offset" => offset = value.trim().parse().unwrap_or(0),
				_ => (),
			}
			continue;
		}
		let text = strip_word_timings(rest);
		for start in starts {
			lines.push(LyricLine {
				start: Some(start),
				text: text.clone(),
			});
		}
	}
	if lines.is_empty() {
		return None;
	}
	lines.sort_by_key(|l| l.start);
	Some(Lyrics {
		synced: true,
		lines,
		artist,
		title,
		offset,
	})
}

fn plain_lyrics(text: &str) -> Lyrics {
	let mut lines: Vec<LyricLine> = text
		.trim_start_matches('\u{feff}')
		.lines()
		.map(|l| LyricLine {
			start: None,
			text: l.trim_end().to_owned(),
		})
		.collect();
	while lines.last().is_some_and(|l| l.text.is_empty()) {
		lines.pop();
	}
	Lyrics {
		synced: false,
		lines,
		artist: None,
		title: None,
		offset: 0,
	}
}

/// Lyrics from text that is either LRC (synced) or plain.
pub fn parse_lyrics(text: &str) -> Lyrics {
	parse_lrc(text).unwrap_or_else(|| plain_lyrics(text))
}

fn read_lyrics_file(path: &Path) -> Option<String> {
	for extension in ["lrc", "LRC", "Lrc"] {
		let candidate = path.with_extension(extension);
		let Ok(metadata) = std::fs::metadata(&candidate) else {
			continue;
		};
		if !metadata.is_file() || metadata.len() > MAX_LYRICS_FILE_BYTES {
			continue;
		}
		if let Ok(bytes) = std::fs::read(&candidate) {
			return non_empty(&String::from_utf8_lossy(&bytes));
		}
	}
	None
}

/// The lyrics of a song: those of an `.lrc` file next to it, then those embedded in its tags
/// (unless they are the same).
pub async fn lyrics(path: &Path) -> Vec<Lyrics> {
	let owned = path.to_owned();
	spawn_blocking(move || {
		let mut found: Vec<Lyrics> = Vec::new();
		if let Some(text) = read_lyrics_file(&owned) {
			found.push(parse_lyrics(&text));
		}
		if let Some(text) = read_file_tags(&owned).and_then(|t| t.lyrics) {
			let embedded = parse_lyrics(&text);
			if !found.iter().any(|l| l.lines == embedded.lines) {
				found.push(embedded);
			}
		}
		found.retain(|l| !l.lines.is_empty());
		found
	})
	.await
	.unwrap_or_default()
}

#[cfg(test)]
mod test {
	use super::*;

	const DIR: &str = "test-data/subsonic-tags";

	#[test]
	fn gains_and_peaks_are_parsed_leniently() {
		assert_eq!(parse_gain("-6.50 dB"), Some(-6.5));
		assert_eq!(parse_gain("+3.2dB"), Some(3.2));
		assert_eq!(parse_gain(" 0 DB "), Some(0.0));
		assert_eq!(parse_gain("-1"), Some(-1.0));
		assert_eq!(parse_gain("loud"), None);
		assert_eq!(parse_gain("NaN"), None);
		assert_eq!(parse_gain(""), None);
		assert_eq!(parse_peak("0.987654"), Some(0.987654));
		assert_eq!(parse_peak("-0.5"), None);
		assert_eq!(parse_peak("inf"), None);
	}

	#[test]
	fn timestamps() {
		assert_eq!(parse_timestamp("00:01.50"), Some(1500));
		assert_eq!(parse_timestamp("01:02.345"), Some(62345));
		assert_eq!(parse_timestamp("12:34"), Some(754000));
		assert_eq!(parse_timestamp("00:01.5"), Some(1500));
		assert_eq!(parse_timestamp("00:01:50"), Some(1500));
		assert_eq!(parse_timestamp("00:60.00"), None);
		assert_eq!(parse_timestamp("ar:Someone"), None);
		assert_eq!(parse_timestamp("00:01.5000"), None);
		assert_eq!(parse_timestamp("-1:00.00"), None);
		assert_eq!(parse_timestamp(""), None);
	}

	#[test]
	fn lrc_files_are_parsed_with_metadata_repeats_and_offsets() {
		let lyrics = parse_lrc(
			"\u{feff}[ar:An Artist]\n[ti:A Title]\n[offset:-250]\n[length:3:00]\n\
			 [00:04.25][00:09.00]Repeated\n[00:01.50]First\n[00:03.00]<00:03.00>Word <00:03.50>timed\nuntimed line\n\n",
		)
		.unwrap();
		assert!(lyrics.synced);
		assert_eq!(lyrics.artist.as_deref(), Some("An Artist"));
		assert_eq!(lyrics.title.as_deref(), Some("A Title"));
		assert_eq!(lyrics.offset, -250);
		let lines: Vec<(Option<i64>, &str)> = lyrics
			.lines
			.iter()
			.map(|l| (l.start, l.text.as_str()))
			.collect();
		assert_eq!(
			lines,
			vec![
				(Some(1500), "First"),
				(Some(3000), "Word timed"),
				(Some(4250), "Repeated"),
				(Some(9000), "Repeated"),
			],
			"sorted by time, repeats expanded, word timings and untimed lines dropped"
		);
	}

	#[test]
	fn text_without_timestamps_is_not_lrc() {
		assert!(parse_lrc("just some words\nand more").is_none());
		assert!(parse_lrc("[ar:Someone]\n[ti:Song]").is_none());
		assert!(parse_lrc("").is_none());
		assert!(parse_lrc("[not a time]text").is_none());
		assert!(parse_lrc("[00:01.00").is_none());
	}

	#[test]
	fn plain_lyrics_keep_their_lines_and_blank_separators() {
		let lyrics = parse_lyrics("one\r\ntwo\n\nthree  \n\n\n");
		assert!(!lyrics.synced);
		let lines: Vec<&str> = lyrics.lines.iter().map(|l| l.text.as_str()).collect();
		assert_eq!(lines, vec!["one", "two", "", "three"]);
		assert!(lyrics.lines.iter().all(|l| l.start.is_none()));
		assert_eq!(lyrics.text(), "one\ntwo\n\nthree");
		assert!(parse_lyrics("").lines.is_empty());
	}

	#[tokio::test]
	async fn tags_are_read_from_a_flac_file() {
		let extras = extras(&Path::new(DIR).join("tagged.flac")).await;
		assert_eq!(extras.bpm, Some(128));
		assert_eq!(extras.replay_gain.track_gain, Some(-6.5));
		assert_eq!(extras.replay_gain.album_gain, Some(-5.1));
		assert_eq!(extras.replay_gain.track_peak, Some(0.987654));
		assert_eq!(extras.replay_gain.album_peak, Some(1.0));
		assert_eq!(
			extras.musicbrainz_track_id.as_deref(),
			Some("11111111-1111-4111-8111-111111111111")
		);
		assert_eq!(
			extras.musicbrainz_album_id.as_deref(),
			Some("22222222-2222-4222-8222-222222222222")
		);
	}

	#[tokio::test]
	async fn tags_are_read_from_an_mp3_file() {
		let extras = extras(&Path::new(DIR).join("embedded.mp3")).await;
		assert_eq!(extras.bpm, Some(90));
		assert!(extras.replay_gain.is_empty());
		assert_eq!(extras.musicbrainz_track_id, None);
	}

	#[tokio::test]
	async fn unreadable_and_missing_files_have_no_extras() {
		assert_eq!(
			*extras(Path::new("test-data/nothing-here.mp3")).await,
			Extras::default()
		);
		assert_eq!(
			*extras(Path::new("test-data/subsonic-tags/synced.lrc")).await,
			Extras::default()
		);
		assert_eq!(
			*extras(&Path::new(DIR).join("plain.mp3")).await,
			Extras::default()
		);
	}

	#[tokio::test]
	async fn tags_are_cached_until_the_file_changes() {
		let directory = crate::test::prepare_test_directory(crate::test_name!());
		let path = directory.join("song.flac");
		std::fs::copy(Path::new(DIR).join("tagged.flac"), &path).unwrap();
		assert_eq!(extras(&path).await.bpm, Some(128));

		// Same modification time: the cached tags are used, even if the content is replaced
		let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
		std::fs::copy(Path::new(DIR).join("plain.mp3"), &path).unwrap();
		std::fs::File::options()
			.write(true)
			.open(&path)
			.unwrap()
			.set_modified(modified)
			.unwrap();
		assert_eq!(extras(&path).await.bpm, Some(128));

		// A new modification time reads the file again
		std::fs::File::options()
			.write(true)
			.open(&path)
			.unwrap()
			.set_modified(modified + std::time::Duration::from_secs(60))
			.unwrap();
		assert_eq!(extras(&path).await.bpm, None);
	}

	#[tokio::test]
	async fn lyrics_come_from_a_sidecar_file_or_from_the_tags() {
		let synced = lyrics(&Path::new(DIR).join("synced.mp3")).await;
		assert_eq!(synced.len(), 1);
		assert!(synced[0].synced);
		assert_eq!(synced[0].lines[0].text, "First synced line");
		assert_eq!(synced[0].lines[0].start, Some(1500));
		assert_eq!(synced[0].lines.len(), 4);
		assert_eq!(synced[0].artist.as_deref(), Some("Tag Artist"));

		let embedded = lyrics(&Path::new(DIR).join("embedded.mp3")).await;
		assert_eq!(embedded.len(), 1);
		assert!(!embedded[0].synced);
		assert_eq!(embedded[0].text(), "Embedded one\nEmbedded two");

		let flac = lyrics(&Path::new(DIR).join("tagged.flac")).await;
		assert_eq!(flac[0].text(), "First line\nSecond line");

		assert!(lyrics(&Path::new(DIR).join("plain.mp3")).await.is_empty());
		assert!(lyrics(Path::new("test-data/nothing-here.mp3"))
			.await
			.is_empty());
	}

	#[tokio::test]
	async fn embedded_lyrics_that_repeat_the_sidecar_are_not_listed_twice() {
		let directory = crate::test::prepare_test_directory(crate::test_name!());
		let path = directory.join("song.flac");
		std::fs::copy(Path::new(DIR).join("tagged.flac"), &path).unwrap();
		// A sidecar with the same text as the tag, and then one that differs
		std::fs::write(path.with_extension("lrc"), "First line\nSecond line").unwrap();
		assert_eq!(lyrics(&path).await.len(), 1);
		std::fs::write(path.with_extension("lrc"), "[00:01.00]Something else").unwrap();
		let both = lyrics(&path).await;
		assert_eq!(both.len(), 2);
		assert!(both[0].synced && !both[1].synced);
	}
}
