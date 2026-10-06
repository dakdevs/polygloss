# ADR-0029: Product motion

- **Status:** Superseded by [ADR-0030](0030-motion-system.md). Its two motions are re-gated and kept there (retuned, with exits); its limits, its Reduce Motion rule and its rejections of panel, section and tree motion are replaced.
- **Date:** 2026-10-05
- **Design:** [§11.16 Motion](../design.md#1116-motion-adr-0030)

## Context

Polygloss had no motion of its own; everything that moved came from gpui-kit (the tab row's spring, dialog and toast entrances, tooltips, scrollbars). The redesign asked for motion following two design-engineering skills (find-animation-opportunities, emil-design-engineering) written for the web. Their gate asks four questions of each candidate: how often it is seen, what it is for, whether it is fast enough, and whether it gets in the way.

GPUI can fade (`opacity`) and offset (`top`/`left` on a relative element); it cannot scale or transform a `div`, and it has no group opacity: a fading element's drop shadow shows through it. Two clocks drive motion: gpui's `with_animation` reads the wall clock (`Instant::now()`), while gpui-base's `animate_keyframes` and `transition` read `background_executor().now()`, which the test scheduler advances only on `advance_clock`. Both render the end state at once when `App::reduce_motion()` is set. gpui-base reads macOS Reduce Motion once at startup and does not follow later changes.

## Decision

Two motions, both on occasional surfaces, ease-out, no exit motion:

| Motion                                              | Spec                                                                                  | Why                                                |
| --------------------------------------------------- | ------------------------------------------------------------------------------------- | -------------------------------------------------- |
| Threads panel content, opened by its toolbar button | Opacity 0→1 and 12 px from the right, 180 ms, ease-out quint; the panel's width snaps | Shows what took the space; the canvas reflows once |
| Banner notices                                      | Opacity 0→1 and 4 px down, 160 ms, ease-out cubic; replays only when a kind appears   | Peripheral "something new" without moving content  |

- The threads panel opened by keyboard (`Window::last_input_was_keyboard`), restored from view state or opened automatically (composer, a URL, `focus`) shows its end state at once.
- Banner notices are the exception to that rule: they always appear on their own. They animate because they are small (4 px), peripheral, take no focus and move no content.
- Both run on gpui-base's executor clock (`animate_keyframes`), never `with_animation`, so tests step them with `advance_clock`.
- The app re-reads Reduce Motion on every window activation through a replaceable reader (default `apply_system_reduce_motion`, which does nothing under the test scheduler; tests install their own); under it nothing moves. Screenshot tests always run with reduced motion.
- New chrome avoids gpui-component pieces with built-in motion: `TabBar::segmented`, `Accordion`, `Collapsible`, `Sidebar` and `Checkbox`.
- Theme colors are never animated.

**Rejected** (and why, by the gate): toolbar dropdown menus (gpui-component's `DropdownMenu` gives no access to its `PopupMenu` surface, which paints its own shadow; fading a wrapper shows that shadow through the menu as a dark slab, and owning a menu component is out of proportion for one motion); the line cursor, scrolling and every jump (frequency, and the 8.3 ms scroll budget); switching reviews, ⌘1–⌘9 and ⌃Tab (frequency); Viewed toggles (frequency, keyboard); tree and accordion expansion, the Files | Reviews switch, split | unified and display toggles (frequency); category sections in the diff and the header card's commit list (height animation fights the scroll anchor and virtualization); sidebar and threads-panel width (relayout every frame, may flip split/unified); hover fades (frequency); a pulsing Live dot (decoration, keeps frames running); landing highlights after a jump (the cursor already marks it); counters (frequency); the composer (keyboard, 50 ms budget).

## Consequences

- gpui-kit motion we inherit and cannot switch off per surface stays: dialogs (250 ms, including ⌘K, ⌘P and ⌘O), toasts (400 ms in), tooltips and scrollbars. Whether to own the palette and finder overlays to make them instant is an open question (design OQ-49).
- Motion tests are deterministic: the first frame shows the full travel, a frame after half the duration lies strictly between, a frame after the duration is settled. Under keyboard opening or Reduce Motion the first frame is already settled.
- `watcher_banner_ms` still counts the first frame that shows the banner.

## Alternatives rejected

| Option                                  | Why not                                                                      |
| --------------------------------------- | ---------------------------------------------------------------------------- |
| Five to seven motions                   | Only two candidates passed the gate                                          |
| Toolbar menus entering from the trigger | The kit's menu shadow shows through while it fades; no hook to ramp it       |
| `with_animation` (wall clock)           | Tests could only sleep and sample, which flakes under load or proves nothing |
| Spring or bounce curves                 | Product chrome; ease-out reads as responsive                                 |
| Animate panel width or section height   | Layout work on the virtualized viewport every frame                          |
| Follow Reduce Motion only at launch     | A change made while the app runs would be ignored                            |
