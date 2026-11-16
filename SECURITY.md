# Security

WLEDPlay is a trusted-LAN appliance. The operator UI does not authenticate callers.

## Control plane

The HTTP server listens on `0.0.0.0:8080` unless you bind it to one interface. These routes change or stop the process:

- `PATCH /config` writes live settings, including WLED destinations and Auto Join.
- `POST /restart` restarts the stack. An AirPlay session can drop.
- `POST /airplay/rescan` starts another AirPlay speaker lookup on the AirPlay network.

Anyone who can open that port can do both. `GET /status`, `GET /levels`, and `GET /artwork` also expose now-playing metadata and cover art.

Bind the UI to a management NIC:

- `network.http` in `config/config.yaml`, or
- `WLEDPLAY_HTTP_INTERFACE`

Or firewall TCP 8080 so only your management hosts can reach it. Do not publish port 8080 on a router.

Docker Compose uses `network_mode: host`, so the container shares the host's addresses. Besides the UI, that exposes mDNS (UDP 5353) and AirPlay 2 timing (UDP 319 and 320) on the NICs Shairport and Avahi use.

This release does not add a token or change the default bind. Treat the host as inside a network you control.

## Reporting

Report a vulnerability with a [private security advisory](https://github.com/ekobres/WLEDPlay/security/advisories/new) on `ekobres/WLEDPlay`. Do not open a public issue for an unfixed vulnerability.
