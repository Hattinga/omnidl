# syntax=docker/dockerfile:1
# omnidl web interface in a container.
#
#   docker build -t omnidl .
#   docker run -d -p 8080:8080 -e OMNIDL_PASSWORD=geheim \
#     -v omnidl-data:/data -v "$PWD/downloads:/downloads" omnidl
#
# /data holds the tools omnidl loads on first start (yt-dlp, ffmpeg, ... ~500 MB),
# settings and the queue; /downloads the finished files. The program runs as
# user 10001; a bind-mounted downloads folder must be writable for it
# (sudo chown 10001:10001 downloads).
# Release images (ghcr.io/hattinga/omnidl) use the "prebuilt" stage with the
# programs built by .github/workflows/release.yml.

ARG DEBIAN=bookworm

FROM rust:1-${DEBIAN} AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked --no-default-features --features cli --bin omnidl-cli \
    && cp target/release/omnidl-cli /usr/local/bin/omnidl-cli

FROM debian:${DEBIAN}-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 omnidl \
    && useradd --system --uid 10001 --gid omnidl --home-dir /data --no-create-home \
        --shell /usr/sbin/nologin omnidl \
    && mkdir -p /data /downloads \
    && chown omnidl:omnidl /data /downloads
ENV OMNIDL_HOME=/data
VOLUME ["/data", "/downloads"]
WORKDIR /data
EXPOSE 8080
USER omnidl
CMD ["omnidl-cli", "serve", "--listen", "0.0.0.0:8080", "--dir", "/downloads"]

# Programs from the release workflow: dist/docker/<amd64|arm64>/omnidl-cli.
FROM runtime AS prebuilt
ARG TARGETARCH
COPY --chmod=755 dist/docker/${TARGETARCH}/omnidl-cli /usr/local/bin/omnidl-cli

# Default: built from source.
FROM runtime
COPY --from=build /usr/local/bin/omnidl-cli /usr/local/bin/omnidl-cli
