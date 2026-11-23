# Running Polaris in a container

The [Containerfile](../Containerfile) at the root of this repository builds an OCI container image of Polaris based on [Alpine Linux](https://alpinelinux.org/). The image contains the Polaris server compiled from the source in this repository, along with the latest [web interface](https://github.com/agersant/polaris-web).

The commands below use [Podman](https://podman.io/). Podman's CLI is compatible with Docker's, so you can replace `podman` with `docker` if you prefer.

On macOS and Windows, Podman runs containers in a virtual machine. Set it up once with `podman machine init` and `podman machine start`.

## Getting the image

### Build it locally

From the root of a clone of this repository:

```sh
podman build -t polaris .
```

The first build compiles Polaris and takes a few minutes. Later builds reuse cached dependencies.

By default the build bundles the latest release of the web interface. To use a specific version, pass its `web.zip` URL:

```sh
podman build -t polaris \
  --build-arg POLARIS_WEB_URL=https://github.com/agersant/polaris-web/releases/download/<tag>/web.zip .
```

### Pull a published image

Each published release of this repository is also built and pushed to the GitHub Container Registry as `ghcr.io/<owner>/polaris`, tagged with the release version (e.g. `0.16.1` and `0.16`) and, for stable releases, `latest`. Use that image name in place of `polaris` in the commands below.

## Run

```sh
podman run -d --name=polaris -p 5050:5050 polaris
```

This starts Polaris and publishes its port to the host. Go to http://localhost:5050 to see it running. Use `podman logs polaris` to see its output.

⚠️ This basic run keeps all data inside the container. It is destroyed when the container is removed (`podman rm`).

## Volumes

The image declares three volumes:

| Path | Purpose |
| --- | --- |
| `/music` | Your music collection |
| `/var/lib/polaris` | Data directory: [configuration file](CONFIGURATION.md), playlists, collection index and auth secrets |
| `/var/cache/polaris` | Cache directory |

Polaris runs as UID/GID 100 inside the container. It needs to be able to write to the data and cache directories, or it will not start.

### Using named volumes (recommended)

Podman creates named volumes with the right ownership, so there is nothing to set up:

```sh
podman run -d \
  --name=polaris \
  -p 5050:5050 \
  -v /my/music/directory:/music:ro \
  -v polaris-cache:/var/cache/polaris \
  -v polaris-data:/var/lib/polaris \
  polaris
```

### Using host directories

To keep the data and cache in directories of your choosing, create them and give UID/GID 100 (inside the container) ownership of them. Without this, Polaris fails to start with `Permission denied` errors.

On **Linux with rootless Podman** (the default for regular users), your own user is root inside the container, so use `podman unshare` to change ownership from the container's point of view:

```sh
mkdir -p /my/polaris/cache /my/polaris/data
podman unshare chown -R 100:100 /my/polaris/cache /my/polaris/data
```

On **macOS and Windows**, `podman unshare` is not available because Podman talks to a virtual machine. Use the named volumes above for the data and cache directories. Only your music directory needs to be a host directory, and it is mounted read-only.

With **rootful Podman** (or Docker) on Linux, change ownership directly:

```sh
mkdir -p /my/polaris/cache /my/polaris/data
sudo chown -R 100:100 /my/polaris/cache /my/polaris/data
```

Then mount them:

```sh
podman run -d \
  --name=polaris \
  -p 5050:5050 \
  -v /my/music/directory:/music:ro \
  -v /my/polaris/cache:/var/cache/polaris \
  -v /my/polaris/data:/var/lib/polaris \
  polaris
```

On Linux systems using SELinux (Fedora, RHEL, etc.), add `:Z` to the data and cache mounts, and `:z` or `:ro,z` to the music mount if it is shared with other containers or applications, so the container is allowed to access them. Without this, Polaris will fail with permission errors even though file ownership is correct.

Once Polaris is running, add `/music` as a mount directory in the settings of the web interface (or in the [configuration file](CONFIGURATION.md#format)) to have it indexed.

### Compose

The same setup works with `podman compose` (which requires a compose provider such as `podman-compose` or `docker-compose`):

```yaml
services:
  polaris:
    build: .
    ports:
      - "5050:5050"
    volumes:
      - /my/music/directory:/music:ro
      - polaris-cache:/var/cache/polaris
      - polaris-data:/var/lib/polaris
    restart: unless-stopped

volumes:
  polaris-cache:
  polaris-data:
```

## Environment variables

The container entrypoint (`res/container/run-polaris`) turns these environment variables into command line arguments for Polaris. Set them with `-e`, e.g. `podman run -e POLARIS_LOG_LEVEL=debug ...`.

| Variable | Used for | Default value |
| --- | --- | --- |
| `POLARIS_PORT` | Listen port | `5050` |
| `POLARIS_CONFIG_FILE` | Location of the [configuration file](CONFIGURATION.md) | `/var/lib/polaris/polaris.toml` |
| `POLARIS_DATA_DIR` | Data directory | `/var/lib/polaris` |
| `POLARIS_CACHE_DIR` | Cache directory | `/var/cache/polaris` |
| `POLARIS_WEB_DIR` | Directory served as the web interface | `/usr/share/polaris/web` |
| `POLARIS_LOG_LEVEL` | Log level (`off`, `error`, `warn`, `info`, `debug` or `trace`) | Polaris default (`info`) |

Any arguments placed after the image name are passed on to Polaris, for example `podman run polaris --show-paths`.

If you change `POLARIS_PORT`, publish that port instead of `5050` (e.g. `-e POLARIS_PORT=8080 -p 8080:8080`).

Polaris runs in the foreground in the container and writes its logs to standard output, not to a log file.

## Running at startup

To start Polaris automatically at boot (or login, for rootless Podman), use [Quadlet](https://docs.podman.io/en/latest/markdown/podman-systemd.unit.5.html) or generate a systemd unit from the container. Alternatively, add `--restart=unless-stopped` to the `podman run` command and enable the `podman-restart.service` systemd unit.

## Migrating from `agersant/polaris`

Earlier Docker images of Polaris were built from the separate [polaris-docker](https://github.com/agersant/polaris-docker) repository and published on Docker Hub as `agersant/polaris`. The image built from this repository uses the same volumes, user and environment variables, so existing data directories can be mounted as-is.
