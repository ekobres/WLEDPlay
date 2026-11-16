# Changelog

## Unreleased

- The Control card lists WLED controllers found by Avahi and the saved unicast rows. A discovered controller is not sent audio until Unicast is enabled and the card is saved. Each row can shift its send time by a millisecond offset. Avahi listens on the AirPlay NIC and, when one is set, the unicast NIC. Shairport still advertises only on the AirPlay NIC.
- Auto Join watches speakers with a standing Avahi browser. The target list is a snapshot from startup, replaced by Rescan for Targets. pyatv runs only when a join is sent. A previously saved target may need to be selected again. Each speaker is one row, and a join tries the receiver id in the form the group leader matches.
- Each WLED packet renders the spectrum for its own departure slot. A PCM burst no longer repeats one set of bands across later packets.
- Outgoing WLED packets stay on one departure grid. A wake with two ready windows no longer sends the second in the same instant as the next wake's packet. A peak whose hold has already elapsed is dropped when overdue slots are skipped.
- Added [`docs/ROADMAP.md`](docs/ROADMAP.md).
- The container exits when Shairport Sync, NQPTP, or the Auto Join sidecar exits, so Compose can restart the stack.
- A multicast address or UDP port change waits for restart. Live unicast keeps the previous port until then.
- Configuration info popups say to change those settings only if you know what you are doing.
- The README now leads with why WLEDPlay exists and the install path. The PCM-file recipe is in CONTRIBUTING.
- Documented that Docker always enables the metadata FIFO, that `pcm.rate` in the example is the file-input default, and that `GET /airplay/devices` exists.

## 0.3.0

Current development snapshot.

## 0.2.1

Previous snapshot. Earlier history is in the git log.
