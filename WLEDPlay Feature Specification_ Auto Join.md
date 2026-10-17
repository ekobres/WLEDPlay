# WLEDPlay Feature Specification: Auto Join

## 1. Overview

Add an **Auto Join** feature to WLEDPlay that allows WLEDPlay to automatically join the active AirPlay 2 playback group of a selected AirPlay 2 speaker when that speaker begins playback.

The feature is configured through a dedicated **Auto Join** section in the existing WLEDPlay web UI.

Auto Join must be entirely **runtime-configurable**. Enabling, disabling, or changing the selected target must not require a Docker stack restart, container restart, or WLEDPlay process restart.

Configuration changes must also be **non-disruptive** to an active AirPlay session. Any Auto Join configuration change applies only once WLEDPlay is idle.

---

## 2. Goals

The feature should allow a user to:

- Enable or disable Auto Join from the WLEDPlay web UI.
- View currently detected AirPlay 2 devices.
- Select one AirPlay 2 speaker as the Auto Join target.
- Change the target at runtime.
- Disable Auto Join at runtime.
- Have WLEDPlay automatically join the selected speaker's AirPlay group when that speaker starts playing.
- Allow the current AirPlay session to continue undisturbed if Auto Join settings are changed while WLEDPlay is active.
- Have configuration changes take effect automatically the next time WLEDPlay is idle and a qualifying playback event occurs.

---

## 3. Non-Goals

This feature does not initially need to:

- Support multiple simultaneous Auto Join target speakers.
- Automatically switch away from an active AirPlay session when another target begins playing.
- Interrupt or replace an existing WLEDPlay AirPlay session.
- Join a different AirPlay group while WLEDPlay is already receiving audio.
- Remove WLEDPlay from an active group merely because Auto Join is disabled.
- Reconfigure Shairport Sync by restarting the container or stack.
- Attempt to arbitrate between multiple simultaneously active AirPlay groups.
- Automatically leave a group solely because the selected Auto Join target changes.

These behaviors may be considered separately in future enhancements.

---

# 4. User Interface

## 4.1 New Auto Join Section

Add a dedicated **Auto Join** section to the existing WLEDPlay web UI.

The section should contain:

1. **Enable Auto Join**
   - Toggle control.
   - Default: disabled unless an existing configuration explicitly enables it.

2. **Auto Join Target**
   - Selector containing detected AirPlay 2 speakers.
   - Disabled when Auto Join is off.
   - Only one target may be selected.

3. **Discovery Status**
   - Indicate whether AirPlay discovery is active.
   - Optionally display the number of detected compatible devices.

4. **Target Status**
   - Show the selected target's current state when known:
     - Available
     - Playing
     - Idle
     - Offline / not currently discovered

5. **Auto Join Runtime Status**
   - Suggested statuses:
     - Disabled
     - Waiting for `<speaker>`
     - Target playing
     - Joining group
     - Joined
     - WLEDPlay busy
     - Waiting until idle
     - Target unavailable
     - Join failed

The UI should make the distinction between **configured target** and **current session state** obvious.

---

## 4.2 Device Selector

The target selector should be populated dynamically from AirPlay discovery.

Display a human-friendly device name such as:

```text
Living Room HomePod
Kitchen
Basement Apple TV
Office Speaker
```

Internally, selection must use a stable identifier rather than the display name.

Preferred identifiers, in order:

1. AirPlay device identifier / Device ID
2. AirPlay unique identifier exposed through discovery
3. Persistent receiver UUID
4. MAC-style AirPlay identifier if that is the only stable identifier available

Do not persist selection using only the advertised display name.

The UI should tolerate duplicate device names.

Example:

```text
Kitchen
Kitchen
```

should internally remain distinguishable and may optionally display additional identifying information:

```text
Kitchen — HomePod
Kitchen — Apple TV
```

or:

```text
Kitchen — A1:B2:C3
Kitchen — D4:E5:F6
```

---

## 4.3 Live Discovery

The device list should update without requiring a page reload where practical.

Acceptable mechanisms include:

- WebSocket
- Server-Sent Events
- Existing WLEDPlay realtime UI transport
- Periodic API refresh

AirPlay discovery should continue independently of whether Auto Join is enabled so that the selector can immediately display devices when the user enables the feature.

If maintaining continuous discovery has a meaningful cost, discovery may instead begin when the Auto Join section is displayed or when Auto Join is enabled, provided UI responsiveness remains reasonable.

---

# 5. Configuration

Auto Join configuration should be stored independently from static container/environment configuration.

Suggested persistent structure:

```yaml
auto_join:
  enabled: true
  target_id: "AA:BB:CC:DD:EE:FF"
  target_name: "Living Room"
```

`target_name` is informational and should not be treated as the authoritative identifier.

The configuration must be writable at runtime.

Saving the configuration must not require:

```text
docker compose restart
docker compose up -d
container restart
process restart
```

The running WLEDPlay application must reload or directly apply changes.

---

# 6. Core Runtime Rule

The fundamental rule is:

> Auto Join configuration must never interrupt an existing playback session.

Configuration changes become effective only when WLEDPlay is idle.

This applies to:

- enabling Auto Join
- disabling Auto Join
- changing the target
- target disappearing
- target reappearing
- any other Auto Join configuration change

---

# 7. Definition of Idle

WLEDPlay should have a single authoritative runtime concept of **idle**.

For Auto Join purposes, WLEDPlay is idle only when it is not currently participating in an active AirPlay audio session.

At minimum:

```text
idle =
    no active Shairport Sync playback session
    AND no active inbound AirPlay audio stream
```

If the existing application already has a reliable playback/session state, reuse that rather than creating a parallel definition.

A short debounce may be used to prevent transient state changes from being interpreted as true idle.

Suggested default:

```text
idle debounce: 2–5 seconds
```

The exact value should be implementation-specific unless WLEDPlay already exposes an equivalent concept.

---

# 8. Deferred Configuration Semantics

If a configuration change occurs while WLEDPlay is active, persist the configuration immediately but do not alter the active AirPlay session.

Example:

```text
Current target: Living Room
WLEDPlay currently joined to Living Room playback

User changes target to Kitchen
```

Required behavior:

```text
Configuration:
    target = Kitchen

Current session:
    continue unchanged

When current session ends:
    WLEDPlay becomes idle

Future behavior:
    monitor Kitchen
```

Likewise:

```text
Auto Join enabled
WLEDPlay currently participating in playback

User disables Auto Join
```

Required behavior:

```text
Configuration:
    enabled = false

Current session:
    continue unchanged

When session ends:
    remain idle
    do not Auto Join future sessions
```

The UI may display:

```text
Auto Join disabled
Current playback will continue until it ends
```

or:

```text
Change pending until idle
```

but no user action should be required.

---

# 9. AirPlay Discovery

WLEDPlay must discover AirPlay 2 receivers on the local network.

Discovery should use Bonjour/mDNS service advertisements appropriate for AirPlay, including as needed:

```text
_airplay._tcp.local
_raop._tcp.local
```

The implementation should derive:

- display name
- stable device identifier
- IP address / hostname
- AirPlay 2 capability
- current availability
- playback/session state where available
- AirPlay group information where available
- group leader information where available

Only devices believed to be compatible with the Auto Join mechanism should appear as selectable targets where feasible.

If compatibility cannot be established until a join is attempted, the device may still be shown, but failures must be surfaced clearly.

---

# 10. Target Monitoring

When Auto Join is enabled and WLEDPlay is idle:

1. Monitor the selected target.
2. Detect when that target enters active AirPlay playback.
3. Determine the target's current AirPlay 2 playback group.
4. Determine the controlling/group-leader device required to modify group membership.
5. Attempt to add WLEDPlay's AirPlay receiver to that group.
6. Once the AirPlay sender begins streaming to WLEDPlay, normal WLEDPlay audio processing continues.

Conceptual flow:

```text
Auto Join enabled
       |
       v
WLEDPlay idle
       |
       v
Monitor selected target
       |
       v
Target begins AirPlay playback
       |
       v
Discover active AirPlay group
       |
       v
Identify controllable group leader
       |
       v
Add WLEDPlay receiver to group
       |
       v
WLEDPlay begins receiving synchronized audio
       |
       v
Normal WLEDPlay processing
```

---

# 11. Busy Behavior

If WLEDPlay is already receiving AirPlay audio, Auto Join must not perform any join operation.

Example:

```text
Selected Auto Join target: Kitchen

WLEDPlay currently receiving audio from:
Living Room group

Kitchen begins playing
```

Required behavior:

```text
Do nothing.
```

Do not:

- switch sessions
- stop existing playback
- join Kitchen
- modify Kitchen's group
- queue a mandatory join

Once WLEDPlay becomes idle, normal monitoring resumes.

If Kitchen is still actively playing at that point, the implementation may attempt to join it.

This is preferred behavior:

```text
WLEDPlay becomes idle
    |
    v
Is configured target currently playing?
    |
    +-- no --> wait for next playback start
    |
    +-- yes --> attempt Auto Join
```

This avoids requiring a new playback transition after WLEDPlay becomes idle.

---

# 12. Enabling Auto Join

When Auto Join changes:

```text
disabled -> enabled
```

## If WLEDPlay is idle

Immediately begin monitoring the selected target.

If the selected target is already actively playing, attempt to join its current group.

## If WLEDPlay is active

Do not alter the current session.

Persist the setting and defer Auto Join behavior until WLEDPlay becomes idle.

---

# 13. Disabling Auto Join

When Auto Join changes:

```text
enabled -> disabled
```

## If WLEDPlay is idle

Immediately stop Auto Join monitoring/actions.

AirPlay discovery may continue for UI purposes.

## If WLEDPlay is active

Do not:

- stop Shairport Sync
- leave the current AirPlay group
- terminate playback
- modify the active group

Persist the setting.

Once the current session ends, no further automatic joins should occur.

---

# 14. Changing the Target

When the selected target changes:

```text
Living Room -> Kitchen
```

## If WLEDPlay is idle

Immediately stop monitoring Living Room for Auto Join purposes and begin monitoring Kitchen.

If Kitchen is already playing, WLEDPlay may immediately attempt to join Kitchen's group.

## If WLEDPlay is active

Persist the new target but do not modify the current AirPlay session.

Once idle, begin monitoring Kitchen.

---

# 15. Target Becomes Unavailable

If the selected target disappears from discovery:

- retain the configured target identifier
- do not automatically clear the user's selection
- indicate that the target is currently unavailable
- continue discovery
- automatically resume monitoring when the same target returns

Example UI:

```text
Target: Living Room
Status: Offline
```

The selector should ideally retain the configured device even when it is temporarily absent:

```text
Living Room — currently unavailable
```

This prevents normal speaker/network restarts from destroying configuration.

---

# 16. Join Operation

The Auto Join service should perform group modification using the existing AirPlay control implementation or an appropriate AirPlay control library such as pyatv if already included or accepted by the project architecture.

The implementation should conceptually support:

```text
discover group
find leader
get current outputs
add WLEDPlay output
```

Before attempting to add WLEDPlay:

- verify WLEDPlay is idle
- verify Auto Join remains enabled
- verify the target still matches the active configuration
- verify the target is still participating in playback
- verify WLEDPlay is not already in the group

The final state must be revalidated immediately before the group-modification request to avoid races.

---

# 17. Concurrency and Race Handling

Auto Join will inherently encounter asynchronous state changes.

The implementation must handle cases such as:

```text
target starts playing
target stops playing
target changes groups
WLEDPlay receives a direct AirPlay connection
user changes Auto Join target
user disables Auto Join
device disappears
device reappears
join operation still in progress
```

Use a serialized or otherwise concurrency-safe Auto Join state machine.

A join attempt should be cancellable or logically invalidated if conditions change before the actual group modification is issued.

Recommended pattern:

```text
configuration generation / revision number
```

Each configuration change increments a revision.

Before performing a delayed join operation:

```text
if captured_revision != current_revision:
    abort
```

Likewise, always re-check WLEDPlay's idle state immediately before issuing a join request.

---

# 18. Auto Join State Machine

Suggested states:

```text
DISABLED

IDLE_WAITING
    Auto Join enabled
    WLEDPlay idle
    waiting for target

TARGET_ACTIVE
    target appears to be playing
    validating group

JOINING
    attempting to add WLEDPlay

JOINED
    WLEDPlay now receiving audio

BUSY
    WLEDPlay is active independently of Auto Join

TARGET_UNAVAILABLE

ERROR
```

Typical transition:

```text
DISABLED
    |
    | enable
    v
IDLE_WAITING
    |
    | target begins playing
    v
TARGET_ACTIVE
    |
    | valid group discovered
    v
JOINING
    |
    | AirPlay session starts
    v
JOINED
    |
    | playback ends
    v
IDLE_WAITING
```

If WLEDPlay receives a direct AirPlay connection while waiting:

```text
IDLE_WAITING
    |
    | inbound AirPlay session
    v
BUSY
    |
    | session ends
    v
IDLE_WAITING
```

---

# 19. Join Retry Behavior

Auto Join should tolerate transient discovery and AirPlay timing issues.

A target may advertise active playback before its full group topology is available.

Use bounded retries.

Suggested behavior:

```text
initial attempt
500 ms
1 sec
2 sec
4 sec
```

Stop retrying when:

- join succeeds
- target stops playing
- WLEDPlay becomes busy
- Auto Join is disabled
- target changes
- maximum retry interval/count is reached

Do not retry indefinitely at high frequency.

After failure, remain capable of responding to future discovery/state changes.

---

# 20. Detection of Successful Join

Do not consider Auto Join successful merely because the group modification request returned successfully.

The preferred success signal is:

```text
WLEDPlay begins receiving the expected AirPlay session/audio stream
```

A group-control operation may therefore transition:

```text
JOINING
```

to:

```text
JOINED
```

only when Shairport Sync or the existing WLEDPlay playback-state mechanism reports active playback.

Use a reasonable timeout.

If the timeout expires:

```text
JOINING -> ERROR / IDLE_WAITING
```

depending on whether the target is still active.

---

# 21. Interaction With Existing WLEDPlay Audio Pipeline

Auto Join must not create a separate audio-processing path.

After Auto Join succeeds, audio must pass through the same existing pipeline used for normal AirPlay playback:

```text
AirPlay 2
    |
    v
Shairport Sync
    |
    v
PCM
    |
    v
WLEDPlay analysis
    |
    v
WLED Audio Sync / configured outputs
```

Auto Join is strictly a session orchestration feature.

It should not duplicate:

- PCM capture
- FFT processing
- WLED Audio Sync generation
- metadata processing
- multicast transmission
- output management

---

# 22. Runtime API

Expose Auto Join state and configuration through the existing WLEDPlay backend API style.

Exact endpoint naming should follow existing project conventions.

Conceptually:

```text
GET  /api/auto-join
PUT  /api/auto-join
GET  /api/airplay/devices
```

Example state response:

```json
{
  "enabled": true,
  "target": {
    "id": "AA:BB:CC:DD:EE:FF",
    "name": "Living Room"
  },
  "runtime": {
    "state": "waiting",
    "targetAvailable": true,
    "targetPlaying": false,
    "wledplayBusy": false,
    "pendingConfigChange": false
  }
}
```

Example update:

```json
{
  "enabled": true,
  "targetId": "AA:BB:CC:DD:EE:FF"
}
```

The backend should validate that:

- `enabled` is boolean
- `targetId` is valid or null
- Auto Join cannot be functionally active without a configured target

The UI may allow enabling Auto Join before selecting a target, but runtime state should then be:

```text
Enabled — no target selected
```

and no join operations should occur.

---

# 23. Persistence

Auto Join settings must survive:

- browser reload
- WLEDPlay process restart
- container restart
- Docker stack restart
- host reboot

Persist at least:

```text
enabled
target_id
target_name
```

Do not persist transient runtime state such as:

```text
JOINING
BUSY
JOINED
```

Runtime state should always be reconstructed after startup.

---

# 24. Startup Behavior

On WLEDPlay startup:

1. Load persisted Auto Join configuration.
2. Start AirPlay discovery.
3. Determine whether WLEDPlay is idle.
4. Resolve the configured target if currently available.
5. If Auto Join is enabled and WLEDPlay is idle:
   - monitor the selected target
   - if the target is already actively playing, it may attempt to join

Startup must not require a target to be online.

---

# 25. Logging

Generate useful structured log messages.

Examples:

```text
Auto Join enabled
Auto Join disabled
Auto Join target changed: Living Room -> Kitchen
Auto Join target discovered: Kitchen
Auto Join target unavailable: Kitchen
Auto Join target started playback: Kitchen
Auto Join group leader identified: Living Room HomePod
Auto Join attempting to add WLEDPlay to AirPlay group
Auto Join successful
Auto Join join request succeeded; waiting for inbound AirPlay session
Auto Join failed: unable to identify group leader
Auto Join failed: group modification not supported
Auto Join deferred: WLEDPlay currently busy
Auto Join configuration changed while active; change deferred until idle
```

Avoid excessive repeated logging from mDNS advertisements or polling events.

---

# 26. UI Behavior Examples

## Example A — Normal Auto Join

```text
Auto Join: ON
Target: Living Room
WLEDPlay: idle

Living Room starts playing.
```

Expected:

```text
WLEDPlay automatically joins Living Room's AirPlay group.
WLEDPlay begins processing audio.
```

---

## Example B — Disable During Playback

```text
Auto Join: ON
Target: Living Room
WLEDPlay currently receiving Living Room playback.

User turns Auto Join OFF.
```

Expected:

```text
Current playback continues unchanged.
No AirPlay group change occurs.
Setting is immediately persisted.
After playback ends, WLEDPlay remains idle.
No future automatic join occurs.
```

---

## Example C — Target Change During Playback

```text
Auto Join target: Living Room
WLEDPlay currently receiving audio.

User changes target to Kitchen.
```

Expected:

```text
Current playback continues.
No switch occurs.
Kitchen becomes the configured target immediately.
After current playback ends:
    WLEDPlay monitors Kitchen.
```

---

## Example D — Target Change While Idle

```text
WLEDPlay idle
Target changed:
Living Room -> Kitchen

Kitchen is already playing.
```

Expected:

```text
WLEDPlay detects that Kitchen is active.
WLEDPlay attempts to join Kitchen's current AirPlay group.
```

---

## Example E — Direct Playback Takes Priority

```text
Auto Join target: Kitchen
WLEDPlay idle

A user explicitly AirPlays directly to WLEDPlay.
```

Expected:

```text
Direct playback begins normally.
Auto Join becomes inactive while WLEDPlay is busy.
Kitchen beginning playback must not interrupt the direct session.
After direct playback ends, Auto Join resumes monitoring Kitchen.
```

---

# 27. Priority Rules

Priority should be explicit:

```text
1. Existing active WLEDPlay AirPlay session
2. Direct user-initiated AirPlay connection
3. Auto Join
```

Auto Join must never displace priority 1 or 2.

Auto Join is opportunistic only when WLEDPlay is idle.

---

# 28. Security

Auto Join must not expose new unauthenticated control interfaces beyond whatever authentication model the existing WLEDPlay web UI/API already uses.

Use the existing application security model.

Do not persist AirPlay credentials or pairing secrets in browser-local storage.

If AirPlay control requires pairing credentials, store them using the same protected server-side persistence strategy used for other application secrets.

---

# 29. Failure Handling

A failed Auto Join attempt must not affect normal WLEDPlay operation.

Examples of expected failures:

- unsupported third-party AirPlay receiver
- group leader cannot be determined
- target disappears during join
- target playback ends during join
- network timeout
- pyatv/control failure
- authentication/pairing failure
- WLEDPlay becomes busy before join completes

In all cases:

```text
normal direct AirPlay operation must continue to work
```

Auto Join failures should be visible in logs and, where useful, in the web UI.

---

# 30. Acceptance Criteria

The feature is complete when all of the following are true:

1. The WLEDPlay UI contains a dedicated **Auto Join** configuration section.

2. Auto Join has an enable/disable toggle.

3. The user can select one detected AirPlay 2 receiver from a dynamically populated device list.

4. The selected target is persisted using a stable identifier.

5. Auto Join settings survive WLEDPlay/container/host restart.

6. Auto Join can be enabled without restarting WLEDPlay or the Docker stack.

7. Auto Join can be disabled without restarting WLEDPlay or the Docker stack.

8. The selected target can be changed without restarting WLEDPlay or the Docker stack.

9. Changing Auto Join configuration while WLEDPlay is actively playing does not interrupt or alter the active AirPlay session.

10. Disabling Auto Join during active playback does not remove WLEDPlay from the current AirPlay group.

11. Changing the target during active playback does not cause WLEDPlay to switch groups.

12. Once WLEDPlay becomes idle, the latest saved Auto Join configuration becomes effective.

13. When idle and the selected speaker starts playing, WLEDPlay attempts to join that speaker's active AirPlay 2 group.

14. If the selected target is already playing when WLEDPlay becomes idle, WLEDPlay may immediately attempt to join it.

15. If WLEDPlay is already busy, Auto Join never interrupts or replaces the current session.

16. A temporarily unavailable target remains configured and is automatically recognized again when rediscovered.

17. A failed Auto Join attempt does not impair normal direct AirPlay operation.

18. Successful Auto Join results in audio entering the existing WLEDPlay PCM/audio-analysis pipeline without introducing a second processing path.

19. Auto Join state and useful errors are visible through logs.

20. The UI clearly indicates the configured target, availability, and current Auto Join runtime state.

---

# 31. Recommended Internal Architecture

Prefer implementing Auto Join as a distinct service/module rather than embedding the logic directly into Shairport Sync control or UI handlers.

Conceptually:

```text
AirPlayDiscoveryService
        |
        v
AutoJoinService
        |
        +---- ConfigurationStore
        |
        +---- PlaybackStateService
        |
        +---- AirPlayGroupController
        |
        +---- UI/API state
```

Responsibilities:

### AirPlayDiscoveryService

- mDNS discovery
- device identity
- availability
- AirPlay capability
- playback/group observations

### AutoJoinService

- state machine
- target monitoring
- deferred configuration semantics
- join decision logic
- retries
- concurrency protection

### AirPlayGroupController

- determine current group
- locate group leader
- enumerate outputs
- add WLEDPlay output

### PlaybackStateService

- authoritative WLEDPlay idle/busy state
- Shairport Sync session state

### ConfigurationStore

- persistent Auto Join configuration
- runtime update notifications

This separation is important because Auto Join policy should remain independent of the mechanism used to perform AirPlay control.

---

# 32. Design Principle

The central behavioral invariant for this feature is:

> **Auto Join may initiate playback only while WLEDPlay is idle. It must never modify, replace, terminate, or redirect an existing WLEDPlay playback session.**

All implementation decisions should preserve this invariant.