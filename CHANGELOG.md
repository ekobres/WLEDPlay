# Changelog

## Unreleased

- Added [`docs/ROADMAP.md`](docs/ROADMAP.md).
- The container exits when Shairport Sync, NQPTP, or the Auto Join sidecar exits, so Compose can restart the stack.
- A multicast address or UDP port change waits for restart. Live unicast keeps the previous port until then.
- Configuration info popups say to change those settings only if you know what you are doing.
- Documented that Docker always enables the metadata FIFO, that `pcm.rate` in the example is the file-input default, and that `GET /airplay/devices` exists.

## 0.3.0

Current development snapshot.

## 0.2.1

Previous snapshot. Earlier history is in the git log.
