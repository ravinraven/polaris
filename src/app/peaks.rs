use std::{
	hash::{DefaultHasher, Hash, Hasher},
	path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use symphonia::core::{
	audio::{sample::Sample, GenericAudioBufferRef},
	codecs::audio::{well_known::CODEC_ID_OPUS, AudioDecoderOptions},
	formats::{probe::Hint, FormatOptions, TrackType},
	io::{MediaSourceStream, MediaSourceStreamOptions},
	meta::MetadataOptions,
};
use tokio::{io::AsyncWriteExt, task::spawn_blocking};

use crate::app::Error;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Peaks {
	pub interleaved: Vec<u8>,
}

#[derive(Clone)]
pub struct Manager {
	peaks_dir_path: PathBuf,
}

impl Manager {
	pub fn new(peaks_dir_path: PathBuf) -> Self {
		Self { peaks_dir_path }
	}

	pub async fn get_peaks(&self, audio_path: &Path) -> Result<Peaks, Error> {
		match self.read_from_cache(audio_path).await {
			Ok(Some(peaks)) => Ok(peaks),
			_ => self.read_from_source(audio_path).await,
		}
	}

	fn get_peaks_path(&self, audio_path: &Path) -> PathBuf {
		let hash = Manager::hash(audio_path);
		let mut peaks_path = self.peaks_dir_path.clone();
		peaks_path.push(format!("{}.peaks", hash));
		peaks_path
	}

	async fn read_from_cache(&self, audio_path: &Path) -> Result<Option<Peaks>, Error> {
		let peaks_path = self.get_peaks_path(audio_path);
		if peaks_path.exists() {
			let serialized = tokio::fs::read(&peaks_path)
				.await
				.map_err(|e| Error::Io(peaks_path.clone(), e))?;
			let peaks =
				bitcode::deserialize::<Peaks>(&serialized).map_err(Error::PeaksDeserialization)?;
			Ok(Some(peaks))
		} else {
			Ok(None)
		}
	}

	async fn read_from_source(&self, audio_path: &Path) -> Result<Peaks, Error> {
		let peaks = spawn_blocking({
			let audio_path = audio_path.to_owned();
			move || compute_peaks(&audio_path)
		})
		.await??;

		let serialized = bitcode::serialize(&peaks).map_err(Error::PeaksSerialization)?;

		tokio::fs::create_dir_all(&self.peaks_dir_path)
			.await
			.map_err(|e| Error::Io(self.peaks_dir_path.clone(), e))?;

		let path = self.get_peaks_path(audio_path);
		let mut out_file = tokio::fs::File::create(&path)
			.await
			.map_err(|e| Error::Io(path.clone(), e))?;

		out_file
			.write_all(&serialized)
			.await
			.map_err(|e| Error::Io(path.clone(), e))?;

		Ok(peaks)
	}

	fn hash(path: &Path) -> u64 {
		let mut hasher = DefaultHasher::new();
		path.hash(&mut hasher);
		hasher.finish()
	}
}

fn compute_peaks(audio_path: &Path) -> Result<Peaks, Error> {
	let peaks_per_minute = 4000;

	let file = std::fs::File::open(audio_path).map_err(|e| Error::Io(audio_path.to_owned(), e))?;
	let media_source = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());

	let mut format = symphonia::default::get_probe()
		.probe(
			&Hint::new(),
			media_source,
			FormatOptions::default(),
			MetadataOptions::default(),
		)
		.map_err(Error::MediaProbe)?;

	let track = format
		.default_track(TrackType::Audio)
		.ok_or_else(|| Error::MediaEmpty(audio_path.to_owned()))?;
	let track_id = track.id;
	let audio_params = track
		.codec_params
		.as_ref()
		.and_then(|p| p.audio())
		.ok_or_else(|| Error::MediaEmpty(audio_path.to_owned()))?
		.clone();

	let mut accumulator = Accumulator::default();

	// symphonia does not ship an Opus decoder, so Opus streams (in Ogg or
	// Matroska/WebM containers) are decoded with a pure-Rust Opus decoder instead.
	if audio_params.codec == CODEC_ID_OPUS {
		let num_channels = audio_params
			.channels
			.as_ref()
			.map(|c| c.count())
			.unwrap_or(2);
		if !(1..=2).contains(&num_channels) {
			return Err(Error::OpusChannelCount(num_channels));
		}
		const OPUS_SAMPLE_RATE: u32 = 48_000;
		const MAX_FRAME_SAMPLES: usize = 5760; // 120 ms at 48 kHz, per channel
		let mut decoder = opus_decoder::OpusDecoder::new(OPUS_SAMPLE_RATE, num_channels)
			.map_err(Error::OpusDecoder)?;
		let mut pcm = vec![0i16; MAX_FRAME_SAMPLES * num_channels];
		let mut samples = Vec::with_capacity(pcm.len());

		while let Some(packet) = next_packet(format.as_mut())? {
			if packet.track_id != track_id {
				continue;
			}
			let Ok(num_decoded) = decoder.decode(&packet.data, &mut pcm, false) else {
				continue;
			};
			samples.clear();
			// Same conversion as symphonia's u8 samples: keep the top 8 bits, unsigned.
			samples.extend(
				pcm[..num_decoded * num_channels]
					.iter()
					.map(|s| ((*s >> 8) + 128) as u8),
			);
			accumulator.ingest(&samples, num_channels, OPUS_SAMPLE_RATE, peaks_per_minute);
		}

		return Ok(accumulator.peaks);
	}

	let mut decoder = symphonia::default::get_codecs()
		.make_audio_decoder(&audio_params, &AudioDecoderOptions::default())
		.map_err(Error::MediaDecoder)?;

	let mut samples: Vec<u8> = Vec::new();
	while let Some(packet) = next_packet(format.as_mut())? {
		if packet.track_id != track_id {
			continue;
		}

		let decoded: GenericAudioBufferRef = match decoder.decode(&packet) {
			Ok(d) => d,
			Err(_) => continue,
		};

		let num_channels = decoded.spec().channels().count();
		let sample_rate = decoded.spec().rate();
		samples.resize(decoded.samples_interleaved(), u8::MID);
		decoded.copy_to_slice_interleaved(&mut samples);
		accumulator.ingest(&samples, num_channels, sample_rate, peaks_per_minute);
	}

	Ok(accumulator.peaks)
}

/// Reads the next packet, treating the end of the stream as `None`.
fn next_packet(
	format: &mut dyn symphonia::core::formats::FormatReader,
) -> Result<Option<symphonia::core::packet::Packet>, Error> {
	match format.next_packet() {
		Ok(packet) => Ok(packet),
		Err(symphonia::core::errors::Error::IoError(e))
			if e.kind() == std::io::ErrorKind::UnexpectedEof =>
		{
			Ok(None)
		}
		Err(e) => Err(Error::MediaPacket(e)),
	}
}

struct Accumulator {
	peaks: Peaks,
	min: u8,
	max: u8,
	num_ingested: usize,
}

impl Default for Accumulator {
	fn default() -> Self {
		Self {
			peaks: Peaks::default(),
			min: u8::MAX,
			max: u8::MIN,
			num_ingested: 0,
		}
	}
}

impl Accumulator {
	/// Folds interleaved unsigned 8-bit samples into min/max peak pairs.
	fn ingest(
		&mut self,
		interleaved: &[u8],
		num_channels: usize,
		sample_rate: u32,
		peaks_per_minute: usize,
	) {
		let num_samples_per_peak =
			((sample_rate as f32) * 60.0 / (peaks_per_minute as f32)).round() as usize;

		for samples in interleaved.chunks_exact(num_channels) {
			// Merge channels into mono signal
			let mut mono: u32 = 0;
			for sample in samples {
				mono += *sample as u32;
			}
			mono /= samples.len() as u32;

			self.min = u8::min(self.min, mono as u8);
			self.max = u8::max(self.max, mono as u8);
			self.num_ingested += 1;

			if self.num_ingested >= num_samples_per_peak {
				self.peaks.interleaved.push(self.min);
				self.peaks.interleaved.push(self.max);
				(self.min, self.max) = (u8::MAX, u8::MIN);
				self.num_ingested = 0;
			}
		}
	}
}

#[cfg(test)]
mod test {
	use super::*;

	#[test]
	fn computes_peaks_for_opus_streams() {
		// sample.opus is silent, so only check that decoding succeeds.
		let silent = compute_peaks(Path::new("test-data/formats/sample.opus")).unwrap();
		assert!(!silent.interleaved.is_empty());

		// These contain a sine wave, so peaks must not be flat.
		for path in [
			"test-data/formats/sample.webm",
			"test-data/formats/sample-tone.mka",
		] {
			let peaks = compute_peaks(Path::new(path)).unwrap();
			assert!(!peaks.interleaved.is_empty(), "no peaks for {path}");
			assert!(
				peaks.interleaved.iter().min() != peaks.interleaved.iter().max(),
				"peaks for {path} are flat, audio was probably not decoded"
			);
		}
	}

	#[test]
	fn computes_peaks_for_matroska_flac() {
		let peaks = compute_peaks(Path::new("test-data/formats/sample.mka")).unwrap();
		assert!(!peaks.interleaved.is_empty());
	}
}
