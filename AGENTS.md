# PaneBot rewrite guidance

Read `HANDOFF.md`, `SERVICE_CROSSWALK.md`, and all of `SPEC.md` before making any design,
implementation, dependency, UI, configuration, or packaging decision.

## Absolute product rules

- PaneBot is a ruthless, clean, lightweight `mpv-targets` client internally.
  The public service contract is the authority for data and operations.
- Production PaneBot is the literal UI compatibility contract. Preserve its
  visible text, capitalization, colors, spacing, dividers, row formats,
  selection treatment, headers, footers, prompts, modes, key bindings,
  navigation, focus, and interaction rhythm exactly unless the user explicitly
  approves a specific difference.
- Never infer UI behavior from summaries or memory when the production source
  is available. Read the relevant production renderer and key handler before
  writing or reviewing each UI section.
- The rewrite boundary is internal architecture and service semantics: replace
  the old daemon/protocol with the public `mpv_targets` client without silently
  redesigning what the operator sees or how the operator uses it.
- Work section-by-section. Begin from the public `mpv_targets` snapshot,
  canonical state, terminology, errors, and typed operations; identify the
  operator job; inventory the exact production UI; identify only the semantic
  changes forced or enabled by `mpv-targets`; obtain the user's ruling on each
  visible or behavioral difference; and only then implement that section.
- When `mpv-targets` exposes useful concepts or capabilities that production
  PaneBot lacked, the client may and should present them after section review.
- Shared `mpv_targets` improvements are made piece-by-piece when the active
  client section proves a general need, not as an upfront foundation sweep.
- `mpv-targets` is the sole authority for target lifecycle, mpv IPC, state,
  protocol, TLS transport, and target configuration. Read its `AGENTS.md` and
  `SPEC.md` completely before integration work.
- Use the public `mpv_targets` library exactly as an outside client would. No
  private WebSocket implementation, private endpoint, Unix-socket access,
  shared runtime state, remote filesystem shortcut, or old PaneBot protocol.
- Treat `SERVICE_CROSSWALK.md` as the integration checklist. Use an existing
  typed library method wherever one exists. When only the documented public
  native escape hatch exists, keep it inside one adapter. When a generally
  useful convenience is missing, add it to `mpv_targets` first instead of
  rebuilding it in PaneBot.
- The narrow client owns only its TUI, connection presentation/policy, and
  client-side composition approved section-by-section. Layouts, playlist
  editing, setup, and an optional packaged local-daemon wrapper are deferred
  product questions, not assumed PaneBot responsibilities. PaneBot never
  regains daemon responsibilities.
- Use canonical `mpv-targets` terminology internally. Existing visible PaneBot
  wording remains unchanged unless the user explicitly approves that exact
  label change.

## Rewrite discipline

- The production source at `../panebot` is the read-only UI specification. Do
  not modify it, copy its monolithic architecture, or port its old daemon.
- Recreate the exact UI behavior through explicit tests and small modules.
  Copying literal keys, labels, colors, spacing, and rendering constants is
  required UI compatibility work, not an architectural port.
- Product/domain workflows may change only after explicit discussion. A
  workflow change does not authorize unrelated visual or interaction changes.
- Do not change `mpv-targets` to satisfy PaneBot privately. If a genuine
  general client-library gap is found, report it and address it in the
  `mpv-targets` repository through its public contract first.
- Keep the implementation light. No plugin framework, generic application
  framework, dependency injection system, message bus, database, background
  polling layer, or speculative abstraction.

## UI component rule

Reusable UI elements are pure, small Ratatui rendering/input components based
on repetition demonstrated by the new client: status badges, target rows,
footers/key hints, selectable lists, prompts, and any later playlist/channel
rows. Screens compose them. Do not create a universal widget hierarchy or
trait system before concrete reuse exists.

## Verification rule

Every completed section needs:

1. an exact production UI inventory plus service mapping in the behavior matrix;
2. pure state/action tests;
3. fixed-size Ratatui buffer or snapshot tests for rendering;
4. direct visual and interaction comparison with production PaneBot;
5. live validation through the public `mpv_targets` client where applicable.

Do not claim UI parity from compilation, memory, summaries, or unit tests
alone.
