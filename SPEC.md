# PaneBot rewrite specification

Status: implemented through the approved Dashboard and Details/playlist
sections. Deferred distribution and layout sections remain explicitly marked.

## 1. Product contract

PaneBot is the canonical Marlovious terminal-native VJ/playback client for
`mpv-targets`. It is deliberately ruthless, clean, and lightweight, and uses
the same public service and Rust client library available to every other
product.

```text
PaneBot UI -> public mpv_targets library -> mpv-targetsd -> named targets -> mpv
```

The public `mpv-targets` contract is the authority for service state and
operations. Production PaneBot is the literal compatibility baseline for the
visible UI and interaction behavior.

The implementation is a clean internal rewrite. Production PaneBot's source
architecture and daemon are not copied; its visible output and input behavior
are reproduced exactly unless a specific change is approved by the user.

## 2. Operator-experience boundary

The production interaction contract includes:

- horizontal `<>-<>-<>` navigation;
- command mode and clear modal precedence;
- Vim keys paired with arrow keys;
- concise keyboard-driven operation and immediate feedback.

The following product semantics do not survive automatically and require
section review:

- the node picker and connection model;
- layouts, lifecycle/setup/management workflows, and orchestration semantics;
- playlist actions and remote filesystem assumptions;
- daemon ownership, shutdown, logging, and configuration behavior.

Each section begins with an exact inventory of production rendering and input,
then maps its data and effects to public service capabilities. Existing keys,
screens, prompts, text, spacing, and colors are mandatory compatibility facts.
Useful service concepts may add functionality only after the user approves how
it extends the existing UI.

## 3. Service contract

`mpv-targets` owns all target execution behavior. PaneBot imports and uses its
public `mpv_targets` crate. It does not:

- open a WebSocket itself;
- implement TLS or certificate acceptance;
- open mpv Unix sockets;
- launch or monitor individual mpv processes;
- maintain a competing authoritative playback model;
- write remote target configuration or media files;
- invent daemon operations for PaneBot.

The service snapshot is authoritative. PaneBot may derive display labels such
as Playing, Paused, Idle, Offline, Stopped, or Disabled, but does not create a
second lifecycle state.

Raw public native mpv operations remain acceptable for genuinely native
behavior such as keypress passthrough. Raw transport or private protocol is
never acceptable.

`SERVICE_CROSSWALK.md` is the source-audited map from production PaneBot to
the current public library. PaneBot must prefer its existing typed methods.
The application adapter may use the public native escape hatch only where the
shared library has no typed convenience, and screen/widget code may never
construct `Operation::Mpv`, command strings, or response JSON.

## 4. Application ownership

The narrow PaneBot client owns:

- terminal rendering and input;
- service selection and connection presentation/policy;
- client-side actions approved for the current operator section;
- UI-only preferences only if later operator use proves they are useful.

Layouts, playlist editing, local daemon ownership/packaging, setup, target
management, and migration are deferred product sections, not assumed scope.

PaneBot configuration must remain UI/application configuration. It must not
duplicate `disabled`, selected channel, live playback state, or other service
authority.

## 5. State model

The rewrite maintains four explicit state groups.

### 5.1 Service state

The latest authoritative `mpv_targets::Snapshot`, updated only by the shared
client. For display, PaneBot places enabled targets before disabled targets,
preserving service order within each group. It applies this ordering on initial
connection and snapshot updates and retains selection by target identity.

### 5.2 Connection state

Selected node, connecting/connected/reconnecting status, whether the local
daemon is owned by this application process, and the latest public connection
error.

### 5.3 Persistent PaneBot state

Known node addresses, layouts, optional display aliases, and local application
setup. This state never becomes a second target configuration model.

### 5.4 Transient UI state

Active screen, selected row, command mode, passthrough mode, picker selection,
playlist selection and marks, input buffers, prompt mode, search query, and
status message.

Transient UI state survives ordinary redraws. On reconnect, node-specific
playlist details close and authoritative service state is replaced, matching
production behavior.

## 6. Action and effect model

Input maps to named actions before screen logic:

```text
KeyEvent -> Action -> active screen update -> Vec<Effect>
```

Actions describe user intent, for example:

```text
NavigateUp
NavigateDown
ScreenLeft
ScreenRight
ToggleCommandMode
EnterPassthrough
RestartSelected
SoloSelected
OpenPlaylist
PlaylistMark
PlaylistDelete
PromptSubmit
Quit
```

Effects are narrow application work:

```text
ConnectNode
TargetOperation
RefreshPlaylist
ApplyLayout
PersistUiConfig
StartOwnedDaemon
StopOwnedSession
```

Do not build a generic event framework. Enums and direct dispatch are enough.
UI widgets render data and emit actions; they do not perform network or file
operations.

## 7. Reusable UI components

Extract components only where the new client demonstrates repetition:

- `Theme`: approved new-client colors and shared styles;
- `ScreenFrame`: the approved terminal composition;
- `Separator`: dim and orange ` :: ` spans;
- `KeyHintBar`: ordered key/label hints with existing spacing;
- `PlaybackBadge` and `VolumeBadge`;
- `SelectableList` viewport and selection background;
- `TargetRow`;
- `PlaylistRow` with current/marked/search-match markers;
- `PickerOverlay`;
- `InputPrompt` and confirmation prompt;
- startup/connection status line.

Prefer stateless rendering functions over trait objects or widget registries.
Screen modules own screen-specific state and compose these pieces.

## 8. Production visual contract

Production uses the following required colors:

```text
orange:  RGB(224, 128, 48)
cyan:    RGB(50, 140, 140)
dim:     RGB(100, 120, 120)
hint:    RGB(140, 160, 160)
divider: RGB(40, 58, 58)
red:     RGB(200, 60, 60)
green:   RGB(60, 180, 100)
white:   RGB(220, 220, 220)
mpv:     RGB(80, 120, 180)

selected row:       RGB(30, 58, 58)
passthrough row:    RGB(20, 30, 55)
command footer bg:  RGB(80, 40, 10)
passthrough footer: RGB(15, 20, 45)
```

Production uses five vertical regions: header height 1, divider height 1, body
fill, divider height 1, footer height 1. Retained screens preserve that
composition unless the user explicitly approves a specific replacement.

Every PaneBot screen retains that footer region and the production PaneBot
footer styling, spacing, separators, and contextual replacement behavior.
Details uses it for its approved shortcut hints, prompts, errors, and status
messages. Dashboard does not show a persistent shortcut list: its footer is a
clean reserved bottom line when idle and shows only contextual confirmations,
progress, errors, and `:: Status` messages. Dashboard lifecycle menus, command
mode, passthrough, and prompts replace that same footer line rather than
changing the screen geometry.

## 9. Review sections and construction order

Each section follows this gate:

1. Inspect the relevant public `mpv_targets` state and operations.
2. Define the operator job and record proposed states and inputs in
   `BEHAVIOR_MATRIX.md`.
3. Inspect the complete relevant production renderer and key handler.
4. Identify exact public-service mappings and unresolved decisions. Propose a
   library addition only if the current public client demonstrably cannot
   support the approved behavior.
5. Present that analysis to the user and obtain a ruling.
6. Amend this specification with the approved behavior.
7. Implement only that section and the shared components it proves necessary.
8. Run state, rendering, and live integration checks.
9. Show comparison evidence and obtain approval before continuing.

Pixel and interaction parity are required for retained UI. Any difference must
be identified and approved before implementation.

### 9.1 Application shell and theme

Implement terminal entry/exit safety, alternate screen, raw mode, and the
approved minimal composition. A panic or error must restore the terminal.
Validate at fixed terminal sizes before adding live service behavior.

The initial shell uses Ratatui with Crossterm's event stream. Every terminal
resize clears the terminal and immediately redraws from current application
state; layout dimensions are never cached. The shell has explicit Normal,
Command, and Passthrough input modes, with Vim/arrow selection equivalence.

### 9.2 Service selection and connection

Do not port the production local-first startup picker or its retry loops.
Resolve the service using the same concepts as `targets`: the configured
default endpoint, an explicit shared-node-book selection, or a direct URL.
The controller is indifferent to whether that endpoint is local or remote.

Design connecting, failure, retry, disconnect, and optional in-application
service switching as one coherent public-client workflow. Horizontal
navigation and Vim/arrow-key equivalence may inform the interaction; the old
picker screen does not constrain it.

Use `RemoteClient`; do not recreate TLS or WebSocket retry code. Consume the
public authoritative snapshot watch and awaitable disconnect notification.
PaneBot owns only the retry/user-feedback policy around connection attempts.

The initial executable accepts one explicit service endpoint:
`panebot --url wss://host:port`. This is a narrow, endpoint-agnostic starting
interaction, not a decision to retain a command-line-only selector or to add a
local-first picker. Connection success, snapshot updates, and disconnect
notifications come directly from `RemoteClient`; a disconnect clears the
rendered snapshot before presenting its public error.

### 9.3 Authoritative target view

Design the primary view directly from the authoritative snapshot. Use
canonical target identity, health, lifecycle, playback state, and terminology.
The initial shared status vocabulary is Disabled, Stopped, Offline, Idle,
Paused, and Playing; add the general derived-status helper to `mpv_targets`
only when this section is implemented, then use it from both clients.

Preserve production's selection, horizontal navigation, command-mode entry,
Vim/arrow-key equivalence, contextual hints, header fields, count semantics,
layout label, row format, aliases, ordering, colors, and retained destination
screens. Stop for a ruling wherever `mpv-targets` cannot supply an old field or
where product semantics are intentionally changing.
Disabled, stopped, and offline targets remain visible authoritative records
but do not accept playback command mode. Idle targets are online and may be
controlled.

### 9.4 Dashboard orchestration

Approved Identify shortcut: uppercase `I` in normal Dashboard mode identifies
every online target on the connected node. Match `targets identify`: uppercase
target names, ASS font size 50, 4,000 ms, through typed `show_text`. Await results
in snapshot order and stop on the first error. Use the existing progress/result
footer and expose `[I] Identify` in the idle hints. It is unavailable inside
prompts, target actions, command mode, passthrough, and Details.

Production assigns these shortcuts and semantics. Preserve their visible keys
and feedback; review any service-semantic change before implementation:

- `r`: restart selected target;
- `R`: restart applicable targets in display order;
- `S`: selected target unmute + play + fullscreen; mute all others;
- `M`: mute all targets except selected;
- `P`: if any online non-idle target is playing, pause all; otherwise production
  mutes all and plays all. Confirm this unusual unpause behavior before coding;
- no server-side bulk operation or rollback;
- operations occur client-side in deterministic display order.

Every public request result must be handled. Do not fire-and-forget failures as
production did; show failure through the existing status-message vocabulary
without redesigning the screen.

### 9.5 Command mode and mpv passthrough

Command mode and mpv passthrough exist on the Dashboard only. Details never
enters either mode; the operator returns to Dashboard for playback control.
Dashboard command mode provides this required visual treatment and controls:

- Space toggles pause;
- `m` toggles mute;
- Enter sends mpv `ENTER`;
- `f` toggles fullscreen;
- `h/l` or Left/Right seek -/+5 seconds;
- `j/k` or Down/Up seek -/+60 seconds exactly as production maps them;
- `9/0` change volume -/+5;
- `\` enters passthrough;
- Tab exits command mode.

Passthrough forwards approved key names through the public allowed `keypress`
operation. It preserves the blue production treatment, forwards plain `v` so
mpv can toggle subtitle visibility, and uses plain `\` to return to command
mode. Stock mpv has no default `\` binding.

Use the existing general `TargetClient::keypress` and relative-volume
conveniences. They compile to already allowed native operations and add no
protocol or daemon feature.

### 9.6 Details/current-playlist screen

Details preserves PaneBot's visual identity and approved interaction behavior
while sourcing its state from the authoritative target record.

Production presents the following playlist UI behavior for review:

- selected target header and badges;
- current title or item count fallback;
- centered scrolling behavior;
- production's orange `>>` selection cursor is removed; the existing row
  background highlight alone represents keyboard selection and recovers three
  title columns;
- green `*` current item;
- orange `•` marked item;
- search-match coloring;
- zero-based playlist positions matching production PaneBot, mpv, and the
  `targets playlist` operator command;
- `k`/Up and `j`/Down move one item; uppercase `K`/physical Page Up and
  uppercase `J`/physical Page Down move one visible playlist-body page;
  Home/End select the first/final item; production's `[`/`]` ten-item jumps
  are removed;
- Space mark-and-advance;
- Escape clears marks;
- Left/`h` returns to Dashboard.

Details is exclusively a live-playlist workspace. It has no Dashboard command
mode, passthrough mode, or transport-control bindings; its approved Play Now
playlist-item action remains part of the workspace.

Live playlist data comes through the public typed playlist API. Screen code
must not parse protocol JSON.

The Details header adds fixed-width `[Loop:On ]`/`[Loop:Off]` and
`[Repeat:On ]`/`[Repeat:Off]` badges from authoritative `loop_file` and
`loop_playlist` state. Uppercase `L` deterministically toggles current-item
loop and uppercase `P` deterministically toggles whole-queue repeat through
typed library methods, with no dialog. Completion status is exactly
`Item Loop On`, `Item Loop Off`, `Queue Repeat On`, or `Queue Repeat Off`;
correlated errors are shown instead of optimistic state.

The full Details header order is target identity, playback badge, volume badge,
Loop badge, Repeat badge, optional timing badge, then title. Known timing is
rendered `[ELAPSED / -REMAINING]` (for example `[01:23 / -03:33]`), adding
hours only when needed. If position or duration is unavailable or
indeterminate, the timing badge is omitted. The title uses the remaining width
and is only visually clipped.

This version adds neither Jump to Current nor Follow Current. The green `*`
and selection background remain independent and may appear on different rows.
It adds no separate Next marker: ordinary next is the following queue row, and
loop/repeat state does not create a fabricated noncontiguous pointer.

Every playlist refresh retains the highlighted item by stable mpv playlist ID
when that item still exists. If it no longer exists, selection clamps the old
position to the refreshed queue; an empty queue has no selection. Individual
actions do not duplicate this retention logic.

Marks are short-lived queue-editing intent, not durable selection state.
Navigation and search preserve them so the operator can reach a destination.
Executing any action clears all marks, whether or not that action consumes
them. Canceling a prompt executes nothing and preserves them.

### 9.7 Playlist actions and prompts

Playlist control will materially change. Review it later across the service's
separate concepts of persistent selected channels, the live mpv playlist,
loading/appending sources, playlist mutation, and upstream playlist creation
or publication. Do not promote the following old actions into roadmap items:

Production provides these prompt modes and actions. Preserve their visible
interaction; review service-semantic changes individually:

- Enter opens the exact single-item confirmation
  `Play Now [Enter]  ::  Move To Next [n]  ::  Cancel [Esc]`;
- the Enter confirmation is exclusively a single-item workflow for the
  highlighted item; existing marks neither change its target nor participate
  in either action;
- Enter in confirmation plays the highlighted item and unpauses;
- `Play Now` on the already-playing item deliberately restarts it from the
  beginning and unpauses, matching production;
- `n` in confirmation moves only the highlighted item immediately after the
  playing item;
- choosing `Move To Next` for the playing item sends no command, closes the
  confirmation, preserves the queue, and shows `Currently playing`;
- choosing it for the item already immediately after the playing item sends no
  command, closes the confirmation, preserves the queue, and shows
  `Already next`;
- when nothing is playing, `Move To Next` moves the highlighted item to
  position `0`, does not start playback, closes the confirmation, and shows
  `Moved to front`;
- Escape is the only cancellation key in this confirmation; production's
  Left/`h` cancellation is removed;
- completing either confirmation action clears all marks, including the
  `Currently playing`, `Already next`, and `Moved to front` outcomes; Escape
  cancels without clearing marks;
- `/` opens one self-contained incremental Search mode and remembers the
  previously highlighted item;
- the highlight jumps to the first match as the query changes, and `]`/`[`
  move through next/previous matches only while Search is open;
- Enter chooses the highlighted match, clears the query and match treatment,
  and returns to normal Details with that item highlighted;
- Escape cancels Search, clears the query and match treatment, and restores
  the item highlighted before `/` by stable ID;
- Search does not support marking or bulk actions while open;
- Search is case-insensitive across both optional title and full filename/URL;
  unquoted whitespace-separated terms are ANDed, quoted text is one exact
  phrase, and punctuation is normalized as a separator for matching;
- Search has no regular-expression or fuzzy-matching mode;
- with no matches, Search preserves the pre-search highlight, shows
  `No matches`, leaves `]`/`[` inert, and continues accepting query edits;
  Enter or Escape exits with the pre-search item restored;
- the Search footer is exactly
  `/QUERY  ::  Match X/Y  ::  [[/]] Navigate  [Enter] Select  [Esc] Cancel`;
  with no results it is
  `/QUERY  ::  No matches  ::  [Enter/Esc] Cancel`;
- matching rows retain production's white match coloring and Search never
  filters rows from the visible queue;
- uppercase `G` opens the client-only zero-based prompt `Go to index: `;
  digits and Backspace edit it, a valid Enter highlights that item and exits,
  an invalid Enter remains open with `Index out of range`, empty Enter cancels,
  and Escape cancels and restores the prior highlight by stable ID;
- Goto never clamps and never sends a service request;
- uppercase `A` opens the production `Add: ` source-entry flow; after a valid
  source is entered it opens the exact mode prompt
  `Append to Queue [a]  ::  Replace Current Queue [r]  ::  Cancel [Esc]`;
- Add stores the complete source without an arbitrary URL-length cap, enables
  bracketed paste, accepts a pasted source atomically, and never truncates the
  stored value to terminal width;
- the one-line editor scrolls horizontally to keep the cursor/end visible and
  Backspace removes one complete character rather than one byte;
- submission strips only surrounding whitespace; one trailing pasted newline
  is discarded, while embedded newlines are rejected with
  `Source must be one line`;
- Escape cancels and clears the complete source; choosing a mode passes the
  complete stored source unchanged to the corresponding typed load operation;
- Add never expands a leading `~` using either client or service-user state;
  such input is rejected with `Use an absolute target path or URL`, keeping
  local and remote behavior identical;
- Add has no Start Now or Insert mode: Append leaves current playback alone;
  Replace replaces the complete live queue and begins loading the submitted
  source through the typed mpv-targets operation;
- choosing `Replace Current Queue [r]` opens
  `Replace current queue? [y/N]`; only `y`/`Y` executes the replacement, while
  Enter or `n`/`N` never changes the queue;
- `n`/`N`, Enter, or Escape at the Replace confirmation discards the pending
  source and exits Add completely; it does not return to the mode chooser;
- production `D`/`Del` is replaced by uppercase `R`, visibly labeled
  `Remove from Queue`; it removes the highlighted item, or the marked items
  when marks exist, from only the live mpv queue and never modifies the source
  channel unless the operator later invokes Save State to Channel;
- if the playing item is among the targets of `R`, PaneBot leaves that item in
  the queue and removes every other targeted item; the playing item does not
  abort the rest of the removal;
- partial removal reports `Removed N; kept playing item`, using the actual
  removed count; if the playing item is the only target, no command is sent
  and the status is `Cannot remove playing item`;
- `R` acts immediately and never opens a confirmation dialog;
- after `R` completes, all marks clear, including a mark on a retained playing
  item;
- Space retains production's mark/unmark-and-advance behavior and exact marked
  row colors/treatment;
- `M` retains production's mark, navigate, move interaction: marked items are
  treated as one ordered block, preserve their queue order, and are inserted
  immediately before the highlighted destination item;
- after `M`, marks clear and the destination item remains highlighted;
- `M` with no marks retains the production message
  `Mark items with Space first`;
- if the highlighted destination is itself marked, `M` is a silent no-op and
  preserves all marks so the operator can choose another destination;
- there are no special Move to Top, Move to End, or Move After Current
  commands; ordinary navigation plus `M` provides those placements;
- the currently playing item cannot be moved; if it belongs to the marked
  block, `M` makes no change, preserves the queue, and shows
  `Cannot move playing item`;
- uppercase `C` opens the exact prompt
  `Crop Before Current [b]  ::  Crop After Current [a]  ::  Cancel [Esc]`;
- `b` removes every live-queue item before the playing item; `a` removes every
  live-queue item after it; the playing item is always retained;
- Crop ignores highlight and marks, never modifies the source channel, and
  does nothing until `b` or `a` is chosen; Escape cancels without change;
- with nothing playing, Crop makes no change and shows
  `Nothing playing to crop around`;
- after Crop succeeds, all marks clear, the playing item remains highlighted
  by stable ID, the queue refreshes, and status reports the removed count;
- uppercase `Z` immediately invokes the typed standard mpv playlist shuffle,
  clears marks, refreshes the queue, and shows `Queue shuffled`; there is no
  upcoming-only shuffle, Unshuffle action, or fabricated shuffle-state badge;
- queue-changing action completion refreshes the live playlist; Loop, Repeat,
  and Save use their correlated results and authoritative state without an
  additional playlist query;
- Details allows one refresh query at a time, folds changes arriving during a
  query into one follow-up, and defers automatic queries during queue actions;
  a read reply never releases an action still running, and queue-action feedback
  completes after its post-action refresh;
- first failure stops the action and produces a visible correlated error;
- no rollback transaction is introduced.

Details uses existing authoritative `last_end_reason`/`last_end_error` only to
show a current playback failure as
`Playback Error :: <mpv error text>` in production red/bold footer styling.
Prompts temporarily cover it and it returns only while the service still
reports it; ordinary EOF, Stop, and Redirect outcomes are hidden. This version
adds no failed-item ID, retained failure state, failed-row coloring, or Remove
Failed action.

Uppercase `S` opens `Save Queue as Channel: ` using the shared one-line prompt.
The operator enters one safe channel filename stem with no path separators and
PaneBot appends `.m3u`. A new name saves immediately through the typed managed
channel operation. An existing name opens
`Overwrite "name.m3u"? [y/N]`; `n`/`N`, Enter, or Escape cancels and exits
Save. Success reports `Saved N items to name.m3u`. Saving never assigns the
new or overwritten channel to the target.
If the entered name has no extension PaneBot appends `.m3u`; an existing
`.m3u` or `.m3u8` extension is preserved with case-insensitive recognition;
any other extension is rejected with
`Channel must use .m3u or .m3u8`.
If the live queue is empty, `S` opens no prompt, sends no request, and shows
`Queue is empty`; PaneBot cannot create or overwrite an empty channel.

### 9.8 Layout picker and macOS geometry

Deferred. Layouts are not part of the narrow target controller. Reopen this
section only after the core service-native client is useful.

### 9.9 Log/activity screen

The production Log screen is rejected and is not a navigation destination.
A useful combined multi-target mpv message view would require a separately
reviewed, generally useful public `mpv-targets` capability. PaneBot must not
scrape local or remote log files or invent a private stream. Structured public
events and client request/results may later justify a distinct activity view,
but none is assumed.

### 9.10 Setup and target management

Approved inline presentation: Target Actions replaces only the selected target's
row with its identity and a horizontal action line; all other target rows stay
visible. Follow-up channel pickers and prompts retain their full-body views.
`h`/`l` and Left/Right select without wrapping; `j`/`k` and Up/Down remain
selection aliases. Enter performs the selected action; only Esc closes the
menu. Scroll the line horizontally as needed to reveal the selected option.
The visible duplicate action and prompt title are `Duplicate`.

Channel pickers remain vertical with `j`/`k`, Up/Down, Enter, and Esc, without
search. Shared vertical lists (targets, nodes, channels, creation-mode choices)
use the playlist's centered selection-following viewport, clamped at the ends.
Creation-mode menus also use Esc alone for Back; h/Left do not close them.

Locate and inspect the user's referenced implementation before specifying this
section. Required capabilities already available from the service include add,
`--from` semantics, enable, disable, start, stop, restart, rename, remove,
channel list, and channel assignment. The UI must compose those public
operations and confirmations; it must not edit remote target directories or
send arbitrary `mpv.conf` contents.

The operations exist today, but their stable success payloads are still
returned as raw `Response` values. Add shared typed lifecycle, topology,
channel-mutation, and channel-catalog results before wiring these screens.

### 9.11 Local macOS runtime and packaging

Deferred. The narrow controller does not spawn, own, stop, package, or
distinguish a local daemon. It connects to an endpoint through `RemoteClient`
and closes only that client connection on exit. Reopen distribution, canned
local target roots, and an optional wrapper-owned bundled daemon after the
controller is established; do not add speculative runtime stubs now.

## 10. Configuration and migration

Do not make the old `pb.panes.conf` authoritative in the rewrite. Migration
may read it once to recover:

- target order;
- display aliases;
- active layout;
- external playlist/channel hints requiring user confirmation.

Target existence, disabled state, selected channel, and live state come from
`mpv-targets`. Known hosts may be migrated from `pb.daemon.conf` to one small
PaneBot client configuration or the documented shared node-book structure.

Migration must be explicit, previewable, and non-destructive. It never deletes
production PaneBot configuration during development or initial validation.

## 11. Error handling

- Every mutating request consumes its correlated result.
- Media failure after command acceptance remains observable target state, not
  a retroactive request error.
- Connection loss changes the UI to its established reconnect flow; it does
  not guess target state.
- Revision gaps require a fresh connection/snapshot as the library specifies.
- Multi-target client actions use the approved client-side selection and
  first-error rules for that section.
- Errors use current status/prompt surfaces. Do not add a notification center
  or monitoring subsystem.

## 12. Verification

### 12.1 Behavior matrix

`BEHAVIOR_MATRIX.md` is created during the rewrite. Every row records:

```text
operator state | input/event | service source/effect | proposed client behavior | verification | ruling
```

### 12.2 Rendering tests

Use Ratatui's test backend at representative fixed sizes. Assert the full
buffer for headers, target rows, footers, pickers, prompts, playlist states,
command mode, passthrough, offline state, and long-title clipping. Golden tests
must remain readable and intentionally updated.

### 12.3 State/action tests

Pure tests cover navigation bounds, modal precedence, reconnect reset,
selection retention, playlist marking/moving/cropping indices, orchestration
selection, and action-to-effect mapping.

### 12.4 Service integration

Test each approved section through the public `mpv_targets` library against a
controlled daemon. The eventual complete client coverage includes:

- initial snapshot and updates;
- local and remote connection;
- play/pause/mute/seek/volume/fullscreen;
- restart and recovery observation;
- Solo, Mute Others, and Pause All composition;
- playlist query/play/append/remove/move/crop;
- target add/from/enable/start/disable/rename/remove;
- channels;
- disconnect/reconnect;
- no duplicate target processes.

### 12.5 Visual approval

At the end of each section, present fixed-size rendering and live
service-backed behavior directly compared with production. The section is
incomplete until parity is demonstrated and the user approves any explicit
difference.

## 13. Explicit non-goals

- No new PaneBot daemon.
- No private service protocol.
- No redesign hidden inside componentization.
- No media library, discovery, downloader, metadata system, scheduler, NVR,
  accounts, authentication, or fleet control.
- No generic dashboard/plugin framework during this rewrite.
- No server-side bulk operation.
- No remote filesystem editor.
- No modification of production PaneBot before cutover.
- No cutover until the complete behavior matrix and real local/remote checks
  pass.
