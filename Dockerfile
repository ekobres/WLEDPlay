# syntax=docker/dockerfile:1
# AirPlay 2 stack for Linux hosts (network_mode: host).
# Official Shairport AP2 cannot run on macOS — NQPTP needs UDP 319/320.

ARG SHAIRPORT_VERSION=5.5.1
ARG NQPTP_VERSION=1.2.8

FROM rust:1.89-bookworm AS wledplay-build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

FROM debian:bookworm-slim AS shairport-build
ARG SHAIRPORT_VERSION
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    build-essential git autoconf automake libtool pkg-config \
    libssl-dev libavahi-client-dev libasound2-dev libsoxr-dev \
    libplist-dev libplist-utils libgcrypt20-dev uuid-dev libpopt-dev libconfig-dev \
    libsodium-dev libavutil-dev libavcodec-dev libavformat-dev xxd \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /build
RUN git clone --depth 1 --branch "${SHAIRPORT_VERSION}" \
      https://github.com/mikebrady/shairport-sync.git && \
    cd shairport-sync && \
    autoreconf -fi && \
    ./configure --with-stdout --with-airplay-2 --with-avahi --with-ssl=openssl --with-soxr && \
    make -j"$(nproc)" && \
    make install

FROM debian:bookworm-slim AS nqptp-build
ARG NQPTP_VERSION
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    build-essential git autoconf automake libtool \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /build
RUN git clone --depth 1 --branch "${NQPTP_VERSION}" \
      https://github.com/mikebrady/nqptp.git && \
    cd nqptp && \
    autoreconf -fi && \
    ./configure && \
    make -j"$(nproc)" && \
    make install

FROM debian:bookworm-slim
# Runtime package names are Debian 12 (bookworm), not Trixie.
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates avahi-daemon dbus dbus-bin \
    libssl3 libsoxr0 libconfig9 libgcrypt20 \
    libplist3 libuuid1 libpopt0 libsodium23 \
    libavahi-client3 libavahi-common3 \
    libavcodec59 libavformat59 libavutil57 libswresample4 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=shairport-build /usr/local/bin/shairport-sync /usr/local/bin/
COPY --from=nqptp-build /usr/local/bin/nqptp /usr/local/bin/
COPY --from=wledplay-build /src/target/release/wledplay /wledplay

COPY config/config.yaml /etc/wledplay/config.yaml
COPY docker/shairport-sync.conf /etc/shairport-sync.conf
COPY docker/avahi-daemon.conf /etc/avahi/avahi-daemon.conf
COPY docker/entrypoint.sh /entrypoint.sh
COPY docker/NOTICE /NOTICE
COPY LICENSE /LICENSE

RUN chmod +x /entrypoint.sh

ENV WLEDPLAY_AIRPLAY_NAME=WLEDPlay \
    WLEDPLAY_MULTICAST_ADDRESS=239.0.0.1 \
    WLEDPLAY_PORT=11988 \
    WLEDPLAY_LOG_LEVEL=info \
    WLEDPLAY_PCM_RATE=48000 \
    WLEDPLAY_PCM_FORMAT=s16le \
    WLEDPLAY_INTERFACE= \
    WLEDPLAY_SHAIRPORT_LATENCY_OFFSET= \
    ENABLE_AIRPLAY2=1 \
    ENABLE_AVAHI=1

EXPOSE 8080
ENTRYPOINT ["/entrypoint.sh"]
