# PaneBot service-native decision matrix

Status: governing mandate locked; every section and proposed deviation remains
subject to review before implementation.

## Governing ruling

PaneBot is a ruthless, clean, lightweight TUI written specifically for the
public `mpv-targets` model. Production PaneBot is the functional operator
baseline.

The public `mpv_targets` library, authoritative snapshots, canonical state,
typed operations, correlated results, and public errors define the internals.
The rewrite does not port the old daemon, protocol, state authority, or source
architecture.

The operator must be able to use the new PaneBot the same way and obtain the
same results unless the user explicitly approves a specific difference during
review of that section. Preserve operator jobs, keys, modes, navigation,
focus, modal precedence, prompts, feedback, and outcomes by default.

Required interaction behavior includes:

- horizontal `<>-<>-<>` navigation;
- command mode;
- Vim keys paired with arrow keys;
- fast keyboard operation, clear modes, and immediate feedback.

Internal structure is open to ruthless simplification: use Targets-native
state and operations, small reusable TUI primitives, and the least code that
faithfully implements the approved behavior. Visual and interaction
improvements are welcome, including substantial ones, but none are implicit.
Each proposed change is reviewed and approved on its own while its production
section is in front of the user.

Work literally section by section through production PaneBot. For each
section:

1. Read the complete production renderer and key handling for that section.
2. Inventory everything the operator sees and can do.
3. Map every state and operation to the public `mpv_targets` contract.
4. Separate unchanged operator behavior, Targets-native internal
   simplification, and proposed product improvements.
5. Present every visible or behavioral difference and obtain a ruling.
6. Implement only the approved section.
7. Compare rendering and interaction directly with production and verify the
   service behavior through the public client.

A screen, mode, action, key, prompt, or transition is not removed merely
because it has not been implemented yet or because its old backend semantics
cannot be carried forward unchanged. Record it as pending or unresolved. An
unfinished milestone never becomes a product-scope ruling, and approval of one
change never authorizes adjacent changes.

Shared `mpv_targets` improvements are implemented only when the active client
section proves the need. Nothing in this roadmap authorizes an upfront library
refactor.

## First section: service connection and authoritative target view

| operator job | service source/effect | proposed client direction | current public support | ruling |
| --- | --- | --- | --- | --- |
| Choose a service | Resolve the configured local endpoint through public `DaemonConfig`; read the documented `mpv-targets/nodes.toml` client address book; accept an optional direct URL | Always show a plain picker: local first, address-book nodes in file order, then a clearly named direct URL when supplied. `j/k` and arrows select without wrapping; Enter connects; initial `q` quits; `C` returns with the previous row selected | Available without a new service capability; PaneBot owns only the picker and documented address-book reading | **Approved** |
| Connect | `RemoteClient::connect` owns WSS/TLS and returns only after the authoritative initial snapshot | Show a compact connecting state. Do not probe nodes in the background and do not spawn or own a daemon | Available now | **Approved** |
| Recover from disconnect | A fresh `RemoteClient` and snapshot restore authority; revision gaps require reconnect | Use one visible indefinite retry flow for local and remote endpoints. Discard stale state immediately; `q` cancels connection/retry back to the picker; reconnect the selected endpoint until cancelled | Available now; reconnection policy is client-owned | **Approved** |
| View node health | `Snapshot.node_id` and `Snapshot.health` | Use service facts and terminology; no old PaneBot active-count calculation | None for the initial facts | **Approved** |
| View targets | Authoritative `Snapshot.targets` and updates applied by the shared client | Header shows PaneBot, authoritative node id, selected endpoint URL, and `Targets: online/configured`; omit Layout. Rows retain the compact quoted-name, status, volume, and title rhythm | `snapshot()` plus `subscribe_snapshots()` are available | **Approved** |
| Understand target state | `disabled`, `stopped`, `online`, and observed playback fields | Present Disabled, Stopped, Offline, Idle, Paused, and Playing with service-native precedence | Shared `target_status` helper is implemented and consumed | **Approved** |
| Inspect unavailable targets | Service clears observed playback state when a process is absent | Keep disabled, stopped, and offline targets visible; show unknown playback facts rather than stale volume/title; do not allow playback command mode | Shared derived status as above | **Approved** |
| Select targets efficiently | Client-only transient selection over snapshot targets | Preserve non-wrapping `j/k` plus arrows, orange `::` cursor, and full-row teal selection; retain selection by canonical target identity across snapshot replacement | None | **Approved** |
| Operate a live target | Typed `TargetClient` methods and correlated results | Command mode is a candidate core interaction. Disabled, stopped, and offline targets cannot enter playback command mode; Idle is online and controllable | Add individual general typed conveniences only when their command-mode action is reviewed | **Direction approved; command section deferred** |
| Navigate operator surfaces | Client-only UI state | Evaluate horizontal `<>-<>-<>` navigation against the service-native screens that actually earn a place | None | **Interaction language approved; destinations pending** |
| Exit | `RemoteClient::close` | Close the client connection only. The narrow controller does not own or stop a daemon or targets | Awaitable close/disconnect semantics may be refined with the connection section | **Approved** |

### Implemented connection slice

The fresh client starts with the shared `mpv-targets` address-book picker,
connects through `RemoteClient`, retries without owning a daemon, replaces
state only from authoritative snapshots, and returns to the picker with `C`.
State/render tests and a live local-service check cover this slice.

### Approved authoritative Dashboard slice

Every new-product reference uses **target**, never pane. The Dashboard uses
canonical target names directly; it does not recreate production's separate
display aliases. It retains the proven compact row and selection rhythm while
showing the service-native Disabled, Stopped, Offline, Idle, Paused, and
Playing states. The Dashboard retains the production-shaped footer region but
does not fill it with persistent command hints. Confirmations, progress,
errors, statuses, and active-mode treatments use that stable bottom surface.

## TUI baseline roadmap

The following production operator capabilities and interaction relationships
remain the functional baseline unless the user explicitly approves a specific
difference during that section's review:

- startup and service-selection presentation;
- Dashboard target list, selection, aliases, and ordering;
- horizontal `Log <> Panes <> Details` navigation rhythm;
- Dashboard selected-target lifecycle actions, Solo, and node-wide Play/Pause
  Toggle actions;
- shared command mode and mpv passthrough mode;
- Details/current-playlist screen;
- playlist search, jump, confirmation, add, marking, delete, move, and crop
  modes;
- prompts, status messages, modal precedence, return behavior, and contextual
  help when needed.

Backend semantics must use the public `mpv_targets` contract. If an old UI
interaction has no valid public-service mapping, preserve it as an unresolved
product decision rather than deleting, faking, or redesigning it silently.

The production Log screen remains part of the navigation and visual-flow
inventory, but its old local-file data source is invalid for a remote public
client. Its honest data source and retained behavior require a specific user
ruling before implementation.

Playlist Save likewise remains recorded as production evidence. Remote
filesystem writing is outside the public service contract, so Save requires a
specific semantic ruling and may not be implemented through a private
shortcut.

## Out of scope for the current TUI slice

These exclusions concern this slice only; they are not blanket product
rejections:

- deployment, installation, and packaging;
- layouts, the layout picker, and geometry application;
- spawning, stopping, bundling, supervising, or owning `mpv-targetsd`;
- binding PaneBot process lifetime to service lifetime;
- local-runtime bootstrap, migration, and other post-targets additions;
- speculative scaffolding for any of the above.

The TUI connects to an already-running service through the public
`mpv_targets` client. Layout-specific controls and state are omitted from this
slice. Their absence must not leave misleading labels or dead controls in the
new interface.

## Proposed improvement ledger

No proposal below is approved merely by appearing here. Before implementation,
review the complete production section and record its operator behavior, the
exact proposed difference, its reason, affected text/keys/transitions, service
mapping, verification, and the user's explicit ruling.

| area | production baseline | proposed difference | reason | ruling |
| --- | --- | --- | --- | --- |
| Overall presentation | Five-region screen with persistent header, dividers, and footer | Every screen retains the five-region PaneBot frame; Dashboard shows its approved idle shortcuts | Preserve stable PaneBot geometry and discoverability across screens | **Approved** |
| Footer/help | Persistent contextual footer | Details shows approved shortcuts and context; Dashboard shows `[Enter] Target Actions :: [Tab] Send Command :: [S] Solo :: [P] Play/Pause All :: [N] New Target :: [l/→] Details :: [C] Nodes :: [q] Quit`; confirmations, progress, errors, `:: Status` messages, prompts, and active modes replace it temporarily | Preserve PaneBot footer presentation and expose the Dashboard controls | **Approved** |
| Service selection | Always-present production node picker | Back it with the configured local endpoint, shared address book, and optional direct URL; retain the lean keyboard flow and omit discovery/probing | Respect the new service boundary without losing the operator workflow | **Approved** |
| Connection feedback | Separate local-start and timed-remote retry paths | One compact indefinite retry state with `q` returning to the picker | Remove daemon ownership and redundant retry machinery while preserving the operator outcome | **Approved** |
| Target status | Production Offline/Stopped/Paused/Playing vocabulary | Add service-native Disabled and intentional Stopped states without disturbing row rhythm | The public service distinguishes states production did not | **Approved** |
| Dashboard actions | Production restart-all, selected restart, Solo, Mute Others, and Pause All | Remove Restart All and Mute Others; Enter opens state-appropriate selected-target lifecycle actions; `S` makes the selected target live now; `P` performs the targets-native all-target play/pause toggle | Match the public target lifecycle and retain only useful operator composites | **Approved** |
| Dashboard command mode | `Tab` adds `[CMD]` to the selected row and an orange/brown temporary command footer | Retain it only on Dashboard for online enabled targets; await every correlated result and surface failures temporarily | Preserve the VJ control rhythm without mixing playback command mode into Details | **Approved** |
| mpv passthrough | Production used `v` for the transition and therefore hid mpv's `v` binding | Use `\` to enter and leave passthrough so plain `v` reaches mpv's subtitle-visibility binding | Preserve direct VJ control without sacrificing a stock mpv binding | **Approved** |
| Log destination | Local `panebot-daemon.log` view | Remove the destination; do not present local files as remote service activity | Remote log-file access is not public | **Approved** |
| Playlist Save | Remote daemon writes an arbitrary M3U path | Save the live queue through the typed managed-channel operation and never assign it automatically | Keeps persistence inside the public service contract | **Approved** |

Add proposals only when the active section proves a concrete operator benefit.
Approval of one row applies only to that row's exact stated difference. The
implementation should seek the smallest code that delivers the approved
operator behavior; fidelity to old internal structure is not a goal, while
functional continuity is.

### Approved Dashboard action semantics

Identify addition (November 12): the user approved uppercase `I` on normal
Dashboard to identify all online targets on the current node. Production
Dashboard has no Identify binding; this specifically approved addition matches
the public `targets identify` command: uppercase names, ASS font size 50,
4,000 ms, typed `show_text`, snapshot order, first error stops. It uses the
existing busy/result footer and adds `[I] Identify` to the idle hints. No new
service operation or persistent target change. State coverage checks modal
precedence and duplicate suppression; the existing Dashboard buffer verifies
the footer hint. The adapter also passed an isolated live daemon/headless-mpv
exercise through the public client. Visible OSD appearance remains for direct
operator review; the formatting matches the `targets` implementation exactly.

`Enter` opens a contextual action picker for the selected target. Disabled
targets offer Enable and Enable & Start; intentionally stopped targets offer
Start, Disable, Rename, and Remove; enabled offline and online targets offer
Restart, Stop, and Disable. Every selected target then offers Set Channel and
Duplicate. Set Channel uses the numbered public channel catalog picker;
an online running target asks `Restart Now? [y/N]`, while Disabled, Stopped,
and Offline targets assign without a restart question. Target Actions is one
horizontal line: `h`/`l` or Left/Right selects without wrapping, Enter performs
the action, and only Esc closes it without changing Dashboard selection.
The previous `j`/`k` and Up/Down selection keys remain aliases. The line scrolls
to reveal the selected action when it exceeds the terminal width. Opening the
picker clears stale action feedback so its contextual footer remains the
bottom line.

Rename is available only when the authoritative target state is intentionally
stopped, matching the public service precondition. It collects a replacement
name, validates it with the public target-name rules, and asks
`Rename "OLD" to "NEW"? [y/N]` before calling the typed rename method. `y`
confirms; `n`, Enter, or Esc cancels. A service error returns to the input with
the entered name intact. Success returns to the Dashboard with the replacement
identity selected after the authoritative topology snapshot arrives.

Remove is offered only for an intentionally stopped target and requires
`Remove "TARGET" and its target files? [y/N]` confirmation before the typed
remove operation. Duplicate is available for every selected target,
collects and validates a new name, and uses that selected target as the public
`add(from)` source. It then offers Create: Disabled, Create: Enabled/Stopped,
and Create: Enabled/Start; the last option enables through `add` and awaits
typed `start` afterward. The new authoritative target identity is selected on
success.

`N` opens New Target from the normal Dashboard. It collects and validates the
new target name, asks `Set Startup Channel? [y/N]` and uses the public channel
catalog picker when selected, then asks the independent base-configuration
questions `Play On Start? [y/N]` and `Mute On Start? [Y/n]`. It finishes with
the same three Create choices and `Create "TARGET" ? - [y/N]` confirmation.
The public typed base-add operation writes those two startup values through the
daemon's existing canonical `base_mpv_conf` generator; PaneBot never writes or
accepts arbitrary mpv configuration.

`S` is Solo: unmute, play, and set fullscreen on the selected target, then mute
every other online enabled target in authoritative snapshot order. `P` selects
all online enabled targets and plays them only when every applicable target is
paused; otherwise it pauses them. Both composites await each correlated result
in order, stop on the first error, and do not roll back completed work.

Enable & Start likewise awaits Enable before Start and stops on the first
error. Operations use the Dashboard's stable footer region for progress and
results; completed Dashboard feedback clears on the next normal interaction so
the approved idle shortcut footer returns. Restart All,
Mute Others, and the old `r`, `R`, and `M` bindings do not exist. `Tab`,
Right/`l`, and Left/`h` outside the action picker remain reserved for later
reviewed sections.

### Production command-mode inventory and proposed mapping

Production enters shared command mode with `Tab`, appends an orange `[CMD]` to
the selected row, retains the normal teal row selection, adds the
orange/brown command footer, and consumes normal navigation/action keys until
`Tab` exits. Its controls map directly to public typed target methods:

| key | production effect | public client mapping |
| --- | --- | --- |
| Space | toggle pause | `toggle_play()` |
| `m` | toggle mute | `toggle_mute()` |
| Enter | send mpv `ENTER` | `keypress("ENTER")` |
| `f` | toggle fullscreen | `fullscreen()` |
| `h`/Left, `l`/Right | seek -5/+5 seconds | `seek(-5.0)` / `seek(5.0)` |
| `j`/Down, `k`/Up | seek -60/+60 seconds | `seek(-60.0)` / `seek(60.0)` |
| `9`, `0` | volume -5/+5 | `adjust_volume(-5.0)` / `adjust_volume(5.0)` |
| `v` | enter passthrough | client-only mode transition |
| Tab | close command mode | client-only mode transition |

The proposed rewrite keeps those keys, labels, colors, row badge, mode
precedence, and interaction rhythm. Because authoritative targets distinguish
availability, `Tab` enters only when the selected target is online and enabled
(Idle remains controllable); Disabled, Stopped, and Offline targets remain on
the Dashboard but do not enter playback command mode. Every operation awaits
its correlated result. A failure temporarily replaces the command footer with
an error line; the next recognized command restores the footer, and `Tab`
still exits immediately. Command mode is Dashboard-only.

Production passthrough is nested inside command mode. It changes the selected
row to a dark-blue background with blue `[MPV]`, shows `[MPV] passthrough — all
keys forward to mpv  [v] exit`, and uses this exact supported key-name set:
Space, Enter, Escape, Backspace, Delete, Tab, arrows, Page Up/Down, Home, End,
F1–F10, lowercase `a`–`z`, and digits `0`–`9`. Unsupported keys do nothing.
The approved rewrite forwards that set through typed `keypress`, including
plain `v`, and reserves plain `\` to return to command mode. This deliberate
difference preserves mpv's default subtitle-visibility key; stock mpv has no
default `\` binding.

### Approved compact menus and scrolling (November 10, 2026)

Inline presentation (November 12): after direct use, the user approved keeping
actions within the selected target row. Keep the rest of the Dashboard list
visible and keep existing full-body
channel pickers/follow-up prompts. No input or service behavior changes. The
selected-row orange cursor, target name and teal background are retained;
horizontal option scrolling uses the remaining row width. Existing action
input tests and fixed buffers cover inline actions, narrow views, unchanged other
rows, and restoration of the Dashboard when Esc closes the actions.
The user completed direct operator review and accepted this presentation.
Temporary trial backups and rollback artifacts were removed on request.

The user approved horizontal Target Actions, Esc-only menu cancellation,
shortening the visible label `Duplicate as New` to `Duplicate`, and vertical
selection-following scrolling. Channel selection stays vertical and has no
search. The same small list renderer centers selection where possible for
Dashboard targets, node selection, channel pickers, and creation-mode choices.
There is no new service operation or background work.

Production inventory: `main` at `49d6190` renders Dashboard and picker rows as
vertical Ratatui lists with teal selected backgrounds and Vim/arrow selection;
Details explicitly centers the playlist viewport. The current rewrite's
vertical Target Actions was a previously approved addition, not an old
production screen. Its horizontal presentation is the specific new ruling.
Retain frame geometry, colors, option order, confirmations, and typed effects.

| operator state | input/event | service source/effect | client behavior | verification | ruling |
| --- | --- | --- | --- | --- | --- |
| Target Actions | h/l, Left/Right, Enter, Esc | Existing typed lifecycle/topology/channel operations, unchanged | One horizontally scrolling line; selected option retains white/bold text with teal background; only Esc cancels | Updated pure input test, full disabled-menu buffer, and 20-column selected-option coverage | Approved |
| Duplicate workflow | Open Duplicate | Existing typed add(from) | Shorter label in action menu and prompt title; workflow unchanged | Existing duplicate workflow/render tests updated | Approved |
| Vertical lists | Navigate or resize | Snapshot targets/endpoints or typed channel catalog | Center selection where possible, clamp viewport at list ends, retain original row numbering and styling | Existing buffers plus both channel-picker variants at 40x10 and 40x8 | Approved |

State/render checks passed. Direct live terminal review is pending the user's
check of the release binary; these checks do not claim full production parity.

### Approved Details/current-playlist behavior

| operator job | service source/effect | approved client behavior | mapping | ruling |
| --- | --- | --- | --- | --- |
| Open/leave Details | Selected authoritative target; typed playlist query | Right/`l` opens; Left/`h` returns. Details has no command mode, passthrough, or `q` binding | typed-existing + client-owned | **Approved** |
| Read the live queue | Typed playlist items with stable mpv IDs; target snapshot | PaneBot five-region frame, centered zero-based rows, green current `*`, orange marked `•`, background-only selection, live header badges and timing | typed-existing | **Approved** |
| Navigate/search | Client-only playlist state | `j/k`, arrows, `J/K`, Page Down/Up, Home/End, zero-based `G`, and incremental `/` search with `]`/`[` next/previous matches | client-owned | **Approved** |
| Play or queue one item | Typed playlist play/move plus deterministic play | Enter opens `Play Now` / `Move To Next`; highlighted item only, marks ignored | typed-existing + client-composition | **Approved** |
| Edit the queue | Typed remove/move/shuffle operations | Space marks; `M` moves one ordered block; `R` removes from live queue; `C` crops before/after current; `Z` shuffles | typed-existing + client-composition | **Approved** |
| Add a source | Typed literal load with Append or Replace | Robust one-line `A` prompt; append or confirmed complete replacement; no path expansion | typed-existing | **Approved** |
| Loop/repeat | Authoritative loop state and typed setters | `L` toggles current-item loop; `P` toggles queue repeat; both remain visible in the header | typed-existing | **Approved** |
| Save the queue | Typed managed-channel save | `S` saves exact live order, confirms overwrite, and never assigns the channel | typed-existing | **Approved** |
| Refresh and errors | Snapshot watch, typed playlist query, correlated results | Refresh on entry, mutation, and playlist position/count changes; retain selection by stable ID; use the footer for results and current playback errors | typed-existing + client-owned | **Approved** |

### Client efficiency pass (November 10, 2026)

Additional approved Dashboard ordering: disabled targets move below all enabled
targets, retaining service order within each group. Apply on connection and
every snapshot; keep keyboard selection on the same target when it moves.
Offline/stopped enabled targets stay in the enabled group. One combined
state/buffer regression covers ordering, row rendering, and selection through
disable/re-enable. This is client-only presentation of authoritative records;
production row text, colors, and input bindings remain unchanged. Live and
direct interactive comparison for this ordering change remains unperformed.

Production reference: `main` at `49d6190`, Details renderer and key handler in
`tui/panebot-tui/src/main.rs`. Retain the five-region frame, row selection and
current/marked/search treatments, contextual prompts, keys, and ordered queue
actions, with the previously approved Details differences above. This pass
changes internal work, not those labels or controls.

| input/event | service source/effect | tightened implementation | verification |
| --- | --- | --- | --- |
| Move To Next | Typed playlist read and move | Reuse one pre-move read, then refresh after completion | Isolated public-client exercise: two playlist reads total, correct resulting order |
| Loop / Repeat / Save | Typed setters/save result and authoritative snapshots | Omit the unconditional full-playlist read | Isolated public-client exercise: zero client playlist reads for each; Save preserves order and does not assign a channel |
| Count/position changes during reads or edits | Snapshot watch and typed playlist read | One read in flight; remember one follow-up; defer reads during edits; read completion cannot unlock the edit | Three focused state tests covering read/edit ordering, coalescing, failures and receipt-only completion |
| Details render during Search | Client search over typed playlist items | Share one match calculation between body and footer | Six before/after fixed buffers, including styles, match at 80x10 and 120x24 across Normal, Search, and item confirmation |
| Unhandled terminal event / obsolete connection-generation update | Client event dispatch | Skip the redraw for these known no-ops | Source review; ordinary key handling and resize redraw retained |

The public-client exercise used an isolated localhost daemon and headless mpv
with temporary files, not deployed targets. Direct interactive comparison with
the old production client and a real remote host was not performed in this pass;
buffer equality is not a claim of complete production UI parity.

### Approved channel Search (November 14, 2026)

The user approved reusing playlist Search in both channel pickers. Production
reference: `main:.../panebot-tui/src/main.rs` Details search renderer and handler
(lines 620–665 and 917–944): `/`, incremental matching, white matches, Enter
keeps selection, Esc cancels. The current approved Details adds AND/quoted
terms, bracket navigation, and restoring selection on cancel. Channel pickers
reuse that matcher and exact contextual footer; existing numbering, vertical
viewport, row geometry, and channel workflows remain. Search is client-owned
over the existing typed channel catalog; no new service calls or operations.
This supersedes the earlier “no search” ruling above.

The existing two-picker state/buffer regression now exercises query digits,
match navigation, narrow scrolling, cancellation, Enter without assignment,
Backspace, and no matches. Existing playlist search tests cover the shared
matcher. Direct terminal review and live channel assignment for this addition
remain unperformed; automated checks do not establish visual parity.

### Approved command-mode track cycling (November 14, 2026)

The user approved `a` for audio and `s` for subtitle cycling in Dashboard
command mode. Production renderer `footer_dashboard_cmd` and Dashboard command
handler (`main` at `49d6190`) have neither control; preserve the existing seek,
volume, fullscreen, and mode keys. Add `[a] Audio [s] Subs` to the existing
command footer with its existing colors and spacing conventions. Passthrough
continues forwarding literal keys to mpv; normal Dashboard is unaffected.

Both actions use existing typed `cycle_audio` / `cycle_subtitle` methods in the
pinned public client, retaining correlated error/result handling. No service or
dependency change. Existing input tests cover both bindings and mode isolation;
the fixed-size command buffer checks the new hints and preserved colors.
Direct terminal review and live multi-track playback validation remain pending.

Approved compact Set Channel picker: remove the redundant `Select Channel`
question row while Picking, giving its row to the scrolling list. Keep the
Set Channel/target heading, loading text, restart question, and footer. Existing
two-picker buffer coverage verifies first-row placement and resized scrolling.
Direct terminal review of this spacing adjustment remains pending.

### Approved shared playback command mode (November 14, 2026)

The user requested Tab command mode throughout the connected UI, with lifecycle
actions left in their existing menu. This supersedes Dashboard-only command
mode. Production Details command handling and `footer_details_cmd` already
provide playback controls; reuse the rewrite's existing command interpreter,
service adapter, and mode footer instead of another handler. The underlying
Details, channel/action menu, or prompt stays intact; command input takes
precedence until Tab closes it. Passthrough remains available via backslash.
No target exists in the connection/node picker, so those views are unchanged.

Details uses existing command/passthrough row colors and footer, including
correlated command errors. Entry is blocked while operations or Details loading/
edits are pending. Paste cannot alter a suspended Details prompt. Existing
Details and both-channel-picker state/buffer checks now verify Tab entry/exit,
footer/row colors, typed audio action, passthrough, preserved selections, and
lifecycle-key isolation. Live terminal comparison remains pending.
