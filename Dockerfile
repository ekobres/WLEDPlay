# syntax=docker/dockerfile:1
# AirPlay 2 stack for Linux hosts (network_mode: host).
# Official Shairport AP2 cannot run on macOS — NQPTP needs UDP 319/320.

ARG SHAIRPORT_VERSION=5.6.3
ARG NQPTP_VERSION=1.2.9

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
COPY docker/shairport-mixed-stack.patch /tmp/shairport-mixed-stack.patch
# Upstream 5.6.3. The patch sends the timing-peer list and B at group setup,
# limits advertised addresses to the AirPlay interface, and lets stdout play
# before a PTP lock exists.
RUN git clone --depth 1 --branch "${SHAIRPORT_VERSION}" \
      https://github.com/mikebrady/shairport-sync.git && \
    cd shairport-sync && \
    git apply /tmp/shairport-mixed-stack.patch && \
    autoreconf -fi && \
    ./configure --with-stdout --with-airplay-2 --with-avahi --with-ssl=openssl --with-soxr --with-metadata && \
    make -j"$(nproc)" && \
    make install

FROM debian:bookworm-slim AS nqptp-build
ARG NQPTP_VERSION
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    build-essential git autoconf automake libtool \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY docker/nqptp-timing-peers.patch /tmp/nqptp-timing-peers.patch
# Upstream 1.2.9. A T command lists every address the client offered.
# Stock keeps the first and drops the rest.
RUN git clone --depth 1 --branch "${NQPTP_VERSION}" \
      https://github.com/mikebrady/nqptp.git && \
    cd nqptp && \
    git apply /tmp/nqptp-timing-peers.patch && \
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
    python3 python3-pip iproute2 \
    && rm -rf /var/lib/apt/lists/* \
    && pip3 install --no-cache-dir --break-system-packages 'pyatv==0.16.1' 'srptools==1.0.1'

COPY --from=shairport-build /usr/local/bin/shairport-sync /usr/local/bin/
COPY --from=nqptp-build /usr/local/bin/nqptp /usr/local/bin/
COPY --from=wledplay-build /src/target/release/wledplay /wledplay

COPY config/config-example.yaml /etc/wledplay/config.yaml
COPY config/shairport-sync-example.conf /etc/shairport-sync.conf
COPY docker/avahi-daemon.conf /etc/avahi/avahi-daemon.conf
COPY docker/entrypoint.sh /entrypoint.sh
COPY docker/autojoin_sidecar.py /usr/local/bin/autojoin_sidecar.py
COPY docker/NOTICE /NOTICE
COPY LICENSE /LICENSE

RUN chmod +x /entrypoint.sh

ENV WLEDPLAY_AIRPLAY_NAME=WLEDPlay \
    WLEDPLAY_MULTICAST_ADDRESS=239.0.0.1 \
    WLEDPLAY_PORT=11988 \
    WLEDPLAY_LOG_LEVEL=info \
    WLEDPLAY_PCM_RATE=48000 \
    WLEDPLAY_PCM_FORMAT=s16le \
    WLEDPLAY_HTTP_INTERFACE= \
    WLEDPLAY_AIRPLAY_INTERFACE= \
    WLEDPLAY_UNICAST_INTERFACE= \
    WLEDPLAY_MULTICAST_INTERFACE= \
    WLEDPLAY_WEB_LISTEN=0.0.0.0 \
    WLEDPLAY_WEB_PORT=8080 \
    WLEDPLAY_SHAIRPORT_LATENCY_OFFSET= \
    WLEDPLAY_METADATA_PIPE=/run/wledplay.meta \
    WLEDPLAY_AUTOJOIN_URL=http://127.0.0.1:8091 \
    PYATV_STORAGE=/host-config/pyatv \
    ENABLE_AVAHI=1

EXPOSE 8080
ENTRYPOINT ["/entrypoint.sh"]
