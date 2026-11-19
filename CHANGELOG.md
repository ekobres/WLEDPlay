# Changelog

## Unreleased

- Docker and native upstream builds now default to unpatched Shairport Sync and NQPTP; local patches remain opt-in with build args.
- Docker Compose now detects host Avahi inside the container and reuses it when present; `WLEDPLAY_AVAHI_MODE` can force host or container mode.
- Auto Join releases Avahi resolver signal matches and resolver objects after discovery completes or a service is removed, preventing DBus match-rule accumulation.
- The Visualizer switch in the card header has no extra label.
- The Statistics card shows CPU, memory, send lateness, jitter, and the protocol counters, each with a description. Stream duration is on the Session card. Sync offset and WLED destinations stay on Control and Configuration.
- The visualizer Decay switch stays off after a page refresh.
- Auto Join keeps listening after its first browse. Speakers that show up later are added to the target list, and this receiver is resolved again when Shairport advertises it, so a playing group can be joined without Rescan for Targets.
- A track change publishes the new title and artist together, and the cover art is replaced instead of keeping the previous picture. The once-per-second debug line reports UDP sends, audio frames, and whether a peak occurred during that second.
- Startup waits about five seconds for the Auto Join sidecar port, keeps watching for the NQPTP clock file, and grants Shairport `SYS_NICE` so it can set realtime scheduling. An `rtprio` limit is not set, because the runtime rejects it and will not start the container. Auto Join waits while this receiver is not advertised yet.
- WLEDPlay log timestamps use local time with a numeric offset. Debug and trace record RMS, peak, frequency, FFT bands, packet counters, delay-queue drops, and the timing offset about once a second while audio is playing.
- The Logs card shows WLEDPlay, Shairport Sync, NQPTP, and the container console. It follows the end, scrolls back, searches, downloads, and colors each line by level. The WLEDPlay level applies immediately and is saved. Each log keeps its last 2 MB after it passes 8 MB.
- AirPlay player records are resolved on the AirPlay NIC. A sighting on the unicast NIC no longer takes the only resolve slot, which was dropping the session player list.
- The Control card lists WLED controllers found by Avahi and the saved unicast rows. A discovered controller is not sent audio until Unicast is enabled and the card is saved. Each row can shift its send time by a millisecond offset. Controllers that share a send time share one delay-queue entry, so a longer unicast list does not discard packets that are about to be sent. Avahi listens on the AirPlay NIC and, when one is set, the unicast NIC. Shairport still advertises only on the AirPlay NIC.
- Measure delays fills each controller delay from its LED count, frame rate, and a short network sample, sending slower controllers earlier. The table is stored on Control Save.
- Startup reads the unicast NIC from the `network` section. A controller row's `unicast: false` flag is no longer taken as the interface name, which was exiting the process on boot.
- Delay fields in the controller table keep the cursor. A status refresh no longer rebuilds a row while that field is being edited.
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
