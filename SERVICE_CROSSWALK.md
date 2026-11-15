# PaneBot to mpv-targets service crosswalk

Status: source-audited against `mpv-targets` commit `090cdcc` and production
PaneBot commit `49d6190`.

This is a capability audit, not the PaneBot roadmap. A production behavior's
presence in this document does not authorize rebuilding it. The new client
starts from the public `mpv-targets` contract and adds behavior only through
section-by-section operator review.

This document prevents the rewrite from rebuilding service behavior inside
PaneBot. It records what exists now, what is merely allowed on the wire, what
small shared-library work remains, and what properly belongs to the client.

## 1. Hard boundary

All target identity, persistent enabled/channel configuration, mpv process
ownership, IPC, recovery, playback state, correlated requests, WSS transport,
and certificate acceptance come from `mpv-targets`.

PaneBot may own presentation, connection/reconnection policy, deterministic
multi-target composition, local application-process ownership, layouts,
playlist editing interaction, and UI configuration. It may not create a
second daemon, state authority, protocol, mpv socket path, or remote filesystem
API.

Use this preference order:

1. Existing typed `mpv_targets` method.
2. A small generally useful typed method added to `mpv_targets` over an
   existing public operation.
3. The documented public `Operation::Mpv` escape hatch, isolated inside the
   PaneBot service adapter.
4. If none applies, stop and identify a genuine public-contract gap.

Screens and widgets never parse `Response` JSON or construct mpv command
arrays.

## 2. Current public client, exactly

`RemoteClient` currently provides:

- `connect(ClientOptions::new(url))` with configurable deadlines;
- a cloned cached `snapshot()`;
- authoritative `subscribe_snapshots()` latest-state watching;
- raw `subscribe()` events;
- `target(name)`;
- polled and awaitable/watchable disconnect state;
- `close()`;
- raw target `request(target, Operation)`;
- typed `list_channels()`.

It owns TLS, the WebSocket reader/writer task, request IDs and correlation,
configurable conservative timeouts, snapshot mutation, revision-gap rejection,
typed stable results, canonical derived status, and explicit close behavior.
The completed foundation backlog was removed at commit `090cdcc`.

## 3. Existing typed target operations

These are available directly on `TargetClient` now.

### Lifecycle and persistent configuration

- add, including `from` and initial enable;
- remove;
- start, intentional stop, and restart;
- enable and disable;
- rename;
- set or clear the selected channel, with explicit restart choice.

### Playback and playlist

- load with replace, append, or append-play;
- load an M3U with the same three modes;
- deterministic play and pause;
- stop playback;
- toggle play;
- next and previous;
- relative seek;
- play a playlist index;
- query the current playlist;
- clear, remove, move, shuffle, and unshuffle the playlist.

### Audio, tracks, and display

- set absolute volume;
- deterministic mute/unmute and mute toggle;
- fullscreen toggle;
- file loop and playlist repeat;
- audio/subtitle selection using auto, disabled, or numeric ID;
- audio/subtitle cycling;
- subtitle-visibility toggle or deterministic setting;
- transient OSD text.

Every method above uses the same public operation as `targets`. PaneBot should
call it rather than spelling the native command itself.

## 4. Production behavior capability map

This table proves how an old behavior could use the new service. It does not
require every behavior to survive the section review. A row may be retained,
removed, or conceptually rewritten; anything retained must use the listed
public boundary.

| Production PaneBot behavior | Correct rewrite path | Current status |
| --- | --- | --- |
| Connect local or remote node | `RemoteClient::connect` | Available |
| Initial target set and state | cached `Snapshot` | Available |
| Incremental state | authoritative snapshot watch | Available |
| Online, disabled, stopped, idle, paused, playing | shared `target_status` helper | Available |
| Title, mute, volume, position, duration, tracks, loop/repeat | `TargetState` | Available |
| Later EOF/media failure | `last_end_reason` and `last_end_error` | Available |
| Restart selected target | `TargetClient::restart` | Available |
| Restart all | select targets from snapshot, call typed restart in display order | Client composition by design |
| Solo selected | typed unmute, play, fullscreen; typed mute for others | Available; client composition |
| Identify all online targets | typed `show_text`, matching `targets identify` formatting and duration | Available; client composition |
| Mute Others | typed mute in display order | Available; client composition |
| Pause/Play All | select from snapshot, typed pause/play | Available; client composition |
| Toggle pause/mute | typed toggle methods | Available |
| Command-mode audio/subtitle cycle | `TargetClient::cycle_audio` / `cycle_subtitle` | Available; `a` / `s` |
| Seek | typed relative seek | Available |
| Fullscreen | typed fullscreen toggle | Available |
| Volume +/-5 | typed relative-volume helper | Available |
| Enter and passthrough keys | typed keypress helper | Available |
| Query live playlist | typed playlist result | Available |
| Play/remove/move/shuffle/crop live playlist | typed playlist methods; crop is ordered client composition | Available |
| Append a URL/path | typed load with `Append` | Available |
| Save live queue as managed channel | typed `save_channel` with explicit overwrite | Available; never assigns the saved channel |
| List shared channels | typed `RemoteClient::list_channels` | Available |
| Assign/clear channel | typed `set_channel` result | Available |
| Add/from/start/enable/disable/rename/remove target | typed service methods and results | Available |
| macOS geometry | public native `set_property geometry` through adapter | Wire available; verify live behavior before choosing final helper |
| Node picker/address book | PaneBot configuration and connection policy | Client-owned |
| Display order and optional aliases | PaneBot UI configuration | Client-owned |
| Layout definitions/picker | PaneBot | Client-owned |
| Activity/log presentation | public events plus client request/results | Client-owned; no remote log access |
| Reconnection timing and messages | policy around `RemoteClient` | Client-owned; transport stays in library |
| Local daemon child ownership | PaneBot local runtime | Client-owned; target processes remain daemon-owned |

## 5. General library foundation status

Typed playlist/channel/lifecycle/topology results, keypress, relative volume,
snapshot watching, awaitable disconnect, configurable timing, and canonical
derived status are complete at `090cdcc`. Keep the raw request API isolated to
uncommon native mpv behavior. Any future convenience must still be generally
useful and must not add PaneBot vocabulary to the protocol.

## 6. Deliberate client-owned behavior

The following should not be pushed into the daemon merely to reduce PaneBot
code:

- Solo, Mute Others, Pause All, or any server-side bulk transaction;
- target display order or aliases;
- node selection and retry presentation;
- terminal modes, prompts, marks, search, crop planning, or selection;
- layouts and macOS window arrangement;
- local app packaging and ownership of its daemon child;
- log/history UI assembled from public activity.

These remain thin client composition over authoritative service state and
typed operations.

## 7. Local macOS bootstrap gap

The library currently exports `AnswerFile`, `DaemonConfig`, validation,
`base_mpv_conf`, and TLS loading. The Linux deployer privately owns filesystem
materialization, self-signed certificate generation, and systemd/profile
installation.

PaneBot's local macOS setup must not invoke Linux systemd and must not copy the
entire deployer. Before implementing setup, choose the smallest explicit
boundary:

- extract reusable platform-neutral config/target/TLS materialization into a
  public `mpv-targets` API, leaving systemd/profile work in `deploy-targets`;
  or
- approve a small PaneBot-owned macOS bootstrap that constructs the exported
  public config types and files directly.

After bootstrap, there is no special local control path: PaneBot starts its
bundled foreground `mpv-targetsd`, connects through `RemoteClient`, and uses
the same target API as every remote node.

## 8. Integration gate

Before implementing a PaneBot section, mark every service-facing behavior in
its behavior matrix with one of:

- `typed-existing`;
- `typed-library-prerequisite`;
- `public-native-adapter`;
- `client-composition`;
- `client-owned`;
- `unresolved-contract`.

Anything else is a boundary error and must be discussed before code is added.
