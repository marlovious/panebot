# PaneBot rewrite handoff

## Objective

Build the canonical Marlovious-published VJ/playback experience as a ruthless,
clean, lightweight client of `mpv-targets`.

The user-defined invariant is literal:

> We are writing an `mpv-targets` client.

Production PaneBot is the literal UI compatibility contract. Preserve its
visible text, capitalization, colors, spacing, dividers, row formats,
selection treatment, headers, footers, prompts, modes, key bindings,
navigation, focus, and interaction rhythm exactly. The rewrite replaces its
internals and adjudicates domain semantics against `mpv-targets`; it does not
silently redesign the UI. Any visible or interaction difference requires the
user's explicit approval for that exact difference.

Work proceeds one service-native operator section at a time. For each section:

- start from the public `mpv_targets` snapshot, state, terminology, errors, and
  typed operations;
- identify the VJ/playback operator job;
- inventory the exact production renderer and key handling for that section;
- include useful `mpv-targets` functionality even when old PaneBot lacked it;
- obtain the user's ruling before implementation.

Shared-library improvements remain on the roadmap and are implemented
piece-by-piece only when the active client section proves a general need.

## Repositories and authority

### New work

```text
~/gitz/panebot-rewrite
```

This directory begins clean. The new implementation belongs here.

### Production behavior reference

```text
~/gitz/panebot
```

At handoff time, production `main` is commit `49d6190` (`Resolve mpv app
bundle on macOS`). The production repository is frozen, deployed software.
Treat it as the read-only UI specification. Its source architecture is not a
template.

Required production references:

```text
AGENTS.md
docs/ecosystem-primer.md
README.md
assets/dashboard.png
tui/panebot-tui/src/main.rs
tui/panebot-lib/src/lib.rs
tui/panebot-daemon/src/main.rs
```

The old TUI is approximately 1,361 lines in one file; the old daemon is
approximately 1,421 lines. Their concentration is a reason for the rewrite,
not a module plan.

### Service authority

```text
~/gitz/mpv-targets-v1
```

At the September 24, 2026 handoff, the client-foundation commit is:

```text
090cdcc Complete public client foundation
```

Before using it, read its `AGENTS.md`, `SPEC.md`, public protocol types, and
`mpv_targets` client API. That repository is authoritative for:

- target identity and configuration;
- enabled, disabled, stopped, online, and recovery behavior;
- mpv process ownership and IPC;
- snapshots and incremental state;
- channel selection and the live mpv playlist;
- native-command allowlisting;
- correlated results and errors;
- WSS transport and native certificate behavior.

PaneBot must not reproduce any of those concerns.

`SERVICE_CROSSWALK.md` records the client-foundation surface at commit
`090cdcc`.
It is more precise than this narrative summary and must be consulted before
each integration section.

## Product boundary

PaneBot is a terminal application for controlling named mpv targets. The
narrow controller owns:

- its terminal UI and approved interaction model;
- service selection and connection presentation/policy;
- client-side composition only where a reviewed operator feature requires it.

Layouts, playlist editing, setup, target-management presentation, local daemon
packaging/ownership, and old orchestration commands are deferred questions.
They are not part of the client merely because production PaneBot had them.

PaneBot does not own:

- an alternate daemon or protocol;
- target supervision or mpv IPC;
- TLS implementation;
- media discovery, metadata, downloading, libraries, or accounts;
- remote filesystem editing;
- a private service capability unavailable to other clients.

The ecosystem rule remains: **Just the streams.**

## Production UI compatibility inventory

The checked-out production TUI has these visible states. Any state retained in
the new product must preserve its exact visible and interaction behavior:

1. Startup/connection status.
2. Node picker, always shown, with local first and configured remote nodes.
3. Dashboard target list.
4. Dashboard command mode.
5. mpv passthrough mode.
6. Log screen.
7. Details/current-playlist screen.
8. Playlist search, jump, play/queue confirmation, add, save, marking, delete,
   move, and crop modes.
9. Local-only layout picker.

Required interaction patterns include:

- horizontal `<>-<>-<>` navigation;
- command mode and clear modal precedence;
- Vim keys paired with arrow keys;
- concise contextual key hints;
- fast target selection and immediate feedback.

Production colors, spacing, labels, frames, geometry, badges, and screenshots
are preservation requirements for retained UI. Do not substitute a cleaner or
more service-native rendering without an explicit user ruling.

## Known integration mapping

| Production PaneBot behavior | Rewrite authority |
| --- | --- |
| Initial node/pane snapshot | `RemoteClient` authoritative snapshot |
| Property-change updates | `RemoteClient` cached snapshot plus event subscription |
| Online/offline | target records from `mpv_targets` |
| Restart selected/all | typed target restart, composed client-side |
| Play/pause/mute/seek/fullscreen | existing typed `TargetClient` methods |
| Relative volume | typed public relative-volume helper |
| mpv passthrough | typed public `keypress` helper |
| Current playlist | typed public playlist query |
| Play/remove/move/shuffle playlist items | public target playlist methods |
| Add URL/path | public load/append methods |
| Add/remove/rename targets | public target service operations |
| Solo/Mute Others/Pause All | PaneBot client-side orchestration over public operations |
| Channel selection | public channel catalog and set-channel operations |
| TLS | entirely inside `mpv_targets`; PaneBot must not rebuild it |
| Target process lifecycle | entirely inside `mpv-targetsd` |
| Node address book | PaneBot client configuration or the shared documented node-book format |
| Layout and geometry | PaneBot client concern, applied only through supported public operations |

The UI must never construct WebSocket JSON or mpv command arrays inside screen
or widget code. A narrow application service adapter calls the public Rust
library and returns application-level results.

The recorded client-foundation commit provides typed service results, typed
playlist/channel results, authoritative snapshot watching, awaitable
disconnect notification, configurable timeouts, canonical derived target
status, keypress, and relative-volume helpers. PaneBot uses those public APIs;
do not compensate with a second cache, response parser, or mpv command
vocabulary.

## Current rewrite state and next action

The workspace contains the approved service selector, authoritative Dashboard,
target lifecycle workflows, command mode, mpv passthrough, and Details/live-
playlist workspace. It renders from authoritative snapshots, clears stale state
on disconnect, awaits correlated public-client results, and has pure state and
fixed-buffer rendering tests.

The next milestone is distribution validation: build release artifacts from a
clean checkout, exercise the client against local and remote `mpv-targetsd`
instances, record the results, and compare retained rendering and interactions
directly with production before declaring the replacement release final.

## Important unresolved evidence

### Visible terminology

Canonical `mpv-targets` terminology is required in code and architecture.
Production visible PaneBot wording remains the UI contract unless the user
explicitly approves an exact label change.

### Playlist Save boundary

`mpv-targets` now exposes a typed `save_channel` operation. It writes the
target's complete live queue in current order to a named M3U in the node's
managed channel directory, requires explicit overwrite authorization, and
never assigns the saved channel back to the target. PaneBot's visible Save
interaction remains subject to playlist-section review.

### Setup and target-management screens

The user recalls these screens as already made, but they are not present in
the checked-out production `panebot-tui/src/main.rs` at commit `49d6190`.
Before specifying or implementing them, locate the actual source/branch or ask
the user to identify it. Do not invent replacement screens from memory.

### Layout application through the new service

Production restarts mpv and passes `--geometry` on macOS. The rewrite may not
launch mpv itself. During the layout section, prove whether live mpv geometry
through the allowed public operation preserves behavior. If not, report the
general service/library gap before changing either project. Do not use direct
mpv sockets or target config rewrites as a shortcut.

### Log screen data

Production reads a local `panebot-daemon.log`, even while the selected node may
be remote. The service correctly does not expose remote logs. The old Log
destination is therefore not part of this client. A future activity view would
require a separately reviewed public-service capability.

## Deferred local packaging and ownership

The narrow controller is endpoint-agnostic. It connects through
`RemoteClient`, does not distinguish local from remote service behavior, does
not spawn or terminate a daemon, and closes only its client connection on
exit. Distribution, canned local target roots, setup, and an optional wrapper
that owns a bundled `mpv-targetsd` are deferred to their own later section.
Do not add speculative local-runtime stubs while building the controller.

## Lightweight architecture

The implementation remains one small binary crate. Reusable rendering
primitives live in `src/ui` and screen-specific state remains in the application
modules. Do not split crates or introduce a framework without demonstrated
reuse.

Recommended application flow:

```text
terminal event
    -> named UI action
    -> active screen updates pure UI state
    -> optional application effect
    -> mpv_targets adapter / local runtime / layout store
    -> authoritative snapshot or correlated result
    -> render
```

Keep these state categories distinct:

- authoritative service snapshot;
- connection/session state;
- persistent PaneBot UI configuration;
- transient screen state such as selection, marks, inputs, and prompts.

## Next action

Prepare and exercise release artifacts on clean macOS and Linux machines. Keep
the deployed implementation available for direct interaction comparison and
rollback until the local and remote cutover checklist passes.
