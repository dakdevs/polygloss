# ADR-0029: Product motion

- **Status:** Accepted
- **Date:** 2026-10-05
- **Design:** [§11.16 Motion](../design.md#1116-motion-adr-0029)

## Context

Polygloss had no motion of its own; everything that moved came from gpui-kit (the tab row's spring, dialog and toast entrances, tooltips, scrollbars). The redesign asked for motion following two design-engineering skills (find-animation-opportunities, emil-design-engineering) written for the web. Their gate asks four questions of each candidate: how often it is seen, what it is for, whether it is fast enough, and whether it gets in the way.

GPUI can fade (`opacity`) and offset (`top`/`left` on a relative element); it cannot scale or transform a `div`. `with_animation` and gpui-base's motion helpers render the end state at once when `App::reduce_motion()` is set. gpui-base reads macOS Reduce Motion once at startup and does not follow later changes.

## Decision

At most three motions, all on occasional, pointer-initiated surfaces, ease-out, no exit motion:

| Motion                                                                 | Spec                                                                                            | Why                                                |
| ---------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------- | -------------------------------------------------- |
| Threads panel content, opened by its toolbar button                    | Opacity 0→1 and 12 px from the right, 180 ms, ease-out quint; the panel's width snaps           | Shows what took the space; the canvas reflows once |
| Toolbar dropdown menus (display options, iteration), opened by pointer | Opacity 0→1 and 8 px down from the trigger, 150 ms, ease-out cubic (gpui-kit's dropdown values) | The menu comes from its button                     |
| Banner notices                                                         | Opacity 0→1 and 4 px down, 160 ms, ease-out cubic; replays only when a kind appears             | Peripheral "something new" without moving content  |

- Opening the same surface by keyboard, restoring it from view state or opening it automatically shows the end state at once (`Window::last_input_was_keyboard`).
- The app re-reads Reduce Motion on every window activation (`apply_system_reduce_motion`); under it every motion is instant. Screenshot tests always run with reduced motion.
- New chrome avoids gpui-component pieces with built-in motion: `TabBar::segmented`, `Accordion`, `Collapsible`, `Sidebar` and `Checkbox`.
- Theme colors are never animated.

**Rejected** (and why, by the gate): the line cursor, scrolling and every jump (frequency, and the 8.3 ms scroll budget); switching reviews, ⌘1–⌘9 and ⌃Tab (frequency); Viewed toggles (frequency, keyboard); tree and accordion expansion, the Files | Reviews switch, split | unified and display toggles (frequency); category sections in the diff and the header card's commit list (height animation fights the scroll anchor and virtualization); sidebar and threads-panel width (relayout every frame, may flip split/unified); hover fades (frequency); a pulsing Live dot (decoration, keeps frames running); landing highlights after a jump (the cursor already marks it); counters (frequency); the composer (keyboard, 50 ms budget).

## Consequences

- gpui-kit motion we inherit and cannot switch off per surface stays: dialogs (250 ms, including ⌘K, ⌘P and ⌘O), toasts (400 ms in), tooltips and scrollbars. Whether to own the palette and finder overlays to make them instant is an open question (design OQ-49).
- A wrapped gpui-component `PopupMenu` paints its own shadow, which shows through a half-transparent menu for a few frames. If a frame-by-frame check shows it, the menu motion is dropped and recorded here.
- `watcher_banner_ms` still counts the first frame that shows the banner.

## Alternatives rejected

| Option                                | Why not                                             |
| ------------------------------------- | --------------------------------------------------- |
| Five to seven motions                 | Only three candidates passed the gate               |
| Spring or bounce curves               | Product chrome; ease-out reads as responsive        |
| Animate panel width or section height | Layout work on the virtualized viewport every frame |
| Follow Reduce Motion only at launch   | A change made while the app runs would be ignored   |
