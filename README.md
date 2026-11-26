<div align="center">
  <h1><img src="res/readme/logo.png?raw=true"/></h1>

  [![Actions Status](https://github.com/agersant/polaris/workflows/Build/badge.svg)](https://github.com/agersant/polaris/actions)
  [![codecov](https://codecov.io/github/agersant/polaris/graph/badge.svg?token=EQqCmBEf2T)](https://codecov.io/github/agersant/polaris)
  [![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE-MIT)

  ![Polaris Web UI](res/readme/web_ui.png?raw=true "Polaris Web UI")
</div>

# About

Polaris is a self-hosted music streaming server, to enjoy your music collection from any computer or mobile device. It is free and open-source software, without any kind of premium version.

The goals of this project are:
- 🔥 Exceptional performance and responsiveness
- 📚️ First-class support for large music collections (100,000+ songs)
- 📦️ Ease of installation, deployment and maintenance
- ✨ Beautiful user interface

# Features

- 🖥️ Runs on Windows, Linux, BSD, or in a container (Podman, Docker)
- 🔊 Support for `flac`, `mp3`, `mp4`, `mpc`, `ogg`, `opus`, `ape`, `wav`, `webm`, `mka` and `aiff` files ([details](#supported-audio-formats))
- 🌈 Dark mode variants and customizable color palette
- 💿️ Browse your music by album, artist or genre
- 📂 Browse your music as a file tree
- 🌊 Song audio-waveform visualization
- 🏷️ Support for multi-value fields in song metadata (eg. multiple artists per song)
- 🔍️ Powerful search functionality with per-field queries
- 🔌 OpenSubsonic API for third-party apps (work in progress, see [docs/OPENSUBSONIC.md](docs/OPENSUBSONIC.md))
- ⚙️ Plain-text configuration also editable with built-in UI
- 👥 Setup multiple users, each with their own playlists
- 📱 Listen to your music on the go:
  - Polaris Android ([Google Play Store](https://play.google.com/store/apps/details?id=agersant.polaris) · [F-Droid](https://f-droid.org/packages/agersant.polaris/) · [Repository](https://github.com/agersant/polaris-android))
  - Polarios ([App Store](https://apps.apple.com/app/polarios/id1662366309) · [Repository](https://gitlab.com/elise/Polarios)) [third-party]
  - Polarity ([Demo](https://www.hackster.io/hardcoder/polarity-a-music-player-for-polaris-8cd4eb) · [Repository](https://github.com/p-dial8891/Polarity)) [third-party]

# Supported audio formats

| Extension | Container / codec | Notes |
| --- | --- | --- |
| `flac` | FLAC | |
| `mp3` | MP3 | |
| `m4a`, `m4b` | MPEG-4 | |
| `mpc` | Musepack | |
| `ogg` | Ogg Vorbis | |
| `opus` | Ogg Opus | Waveforms use a pure-Rust Opus decoder (added in this fork). |
| `ape` | Monkey's Audio | |
| `wav` | WAVE | |
| `aif`, `aiff` | AIFF | |
| `webm` | WebM (typically Opus or Vorbis) | Added in this fork. |
| `mka` | Matroska audio | Added in this fork. |

Notes on `webm` and `mka`:
- Tags are read from standard Matroska tags. A title stored in the Matroska segment info (which is where ffmpeg writes it) is not read.
- Waveforms are generated with a pure-Rust Opus decoder for Opus, and with [symphonia](https://github.com/pdeljanov/Symphonia) for other codecs such as FLAC. Opus streams with more than 2 channels have no waveform.
- Embedded artwork is not extracted for these formats.
- Files are streamed as-is, without transcoding, so playback depends on what your browser or client supports.

# Installation

[Installation documentation](docs/SETUP.md)

[Running Polaris in a container](docs/CONTAINER.md)

[Streaming from remote devices](docs/DDNS.md)

[![Packaging status](https://repology.org/badge/vertical-allrepos/polaris-streaming.svg?columns=3)](https://repology.org/project/polaris-streaming/versions)

# Documentation

- 📒 [Changelog](CHANGELOG.md)
- 🔧 [Configuration](docs/CONFIGURATION.md)
- 👷 [Contribute to Polaris](CONTRIBUTING.md)
- 🛟 [Maintenance Runbooks](docs/MAINTENANCE.md)

The Polaris server API is documented via OpenAPI, every installation of Polaris features this interactive documentation. To access it, open http://localhost:5050/api-docs/ in your browser on the machine running Polaris.
