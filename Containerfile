# Builds a container image of Polaris from the sources in this repository.
#
#   podman build -t polaris .
#
# See docs/CONTAINER.md for how to run the resulting image.

ARG ALPINE_VERSION=3.24

# The web interface lives in a separate repository (agersant/polaris-web) and
# is bundled into releases as web.zip. Override this to pin a specific version,
# e.g. https://github.com/agersant/polaris-web/releases/download/<tag>/web.zip
ARG POLARIS_WEB_URL=https://github.com/agersant/polaris-web/releases/latest/download/web.zip


# Stage 1: compile the Polaris server
FROM docker.io/rust:1-alpine${ALPINE_VERSION} AS server

RUN apk add --no-cache build-base

WORKDIR /src
COPY . .

# Use the Rust toolchain provided by the base image instead of letting rustup
# fetch the one requested in rust-toolchain.toml (which also pulls in
# editor-only components).
RUN rm -f rust-toolchain.toml

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked && \
    install -D -m0755 target/release/polaris /out/polaris


# Stage 2: fetch the web interface
FROM docker.io/alpine:${ALPINE_VERSION} AS web

ARG POLARIS_WEB_URL
ADD ${POLARIS_WEB_URL} /tmp/web.zip
RUN unzip -q /tmp/web.zip -d /out && test -f /out/web/index.html


# Stage 3: runtime image
FROM docker.io/alpine:${ALPINE_VERSION}

# Polaris runs as UID/GID 100 (the `users` group). Bind-mounted data and cache
# directories on the host must be writable by this user.
RUN adduser -S -D -H -u 100 -G users -h /var/lib/polaris -s /sbin/nologin polaris && \
    install -d -m0755 -o100 -g100 /var/cache/polaris /var/lib/polaris && \
    install -d -m0755 /music

COPY --from=server /out/polaris /usr/local/bin/polaris
COPY --from=web /out/web /usr/share/polaris/web
COPY --chmod=0755 res/container/run-polaris /usr/local/bin/run-polaris

EXPOSE 5050
VOLUME [ "/music", "/var/cache/polaris", "/var/lib/polaris" ]

USER polaris
ENTRYPOINT [ "/usr/local/bin/run-polaris" ]
