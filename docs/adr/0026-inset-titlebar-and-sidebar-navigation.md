# ADR-0026: Inset titlebar and sidebar navigation

- **Status:** Accepted. Supersedes the tab row of ADR-0023; ADR-0023's decision (one window, one tab per review, reopen focuses) stands.
- **Date:** 2026-10-05
- **Design:** [§11.1 Window and navigation](../design.md#111-window-and-navigation-adr-0023-adr-0026), [§11.2 Home](../design.md#112-home-and-the-reviews-list), [§11.4 Toolbar](../design.md#114-toolbar)

## Context

The redesign (plan M6) follows a reference layout: no titlebar and no tab row, the traffic lights inside a full-height sidebar, and the main column's top row acting as the titlebar. ADR-0023's tabs (one per review, Home first, ⌘W, ⌃Tab) are used by URLs, IPC, notifications, the composer and the feed through the `Tabs` model, so the model must survive while its presentation changes.

GPUI (gpui-pre 0.3.7) already gives us what this needs: `TitlebarOptions { appears_transparent, traffic_light_position }`, `app_owns_titlebar_drag`, `Window::start_window_move` and `titlebar_double_click`. It offers window-wide blur only (`WindowBackgroundAppearance::Blurred`, the Selection material), with no per-region vibrancy and no Reduce Transparency read, and headless screenshots cannot render it.

## Decision

- **Window:** transparent titlebar, traffic lights at (19, 19) pt centered in a 52 pt top row, `app_owns_titlebar_drag`, `tabbing_identifier: None` (no native tabs), opaque background.
- **Two top rows, one height:** the sidebar's top row (traffic lights, a `[Files | Reviews]` segmented control, a sidebar toggle) and the main column's toolbar row. Both are drag regions with double-click zoom, labels and the gaps between controls included; each control claims its mouse-down, so a press on a button or pill never moves the window.
- **No tab row.** The `Tabs` model and its actions stay unchanged (Home is index 0; ⌘W, ⌘{ ⌘}, ⌃Tab, close-to-the-left, reopen focuses). Open reviews are listed in the sidebar's **Reviews** segment with close buttons and an Open… (⌘O) button; **Files** shows the review's file tree. New: ⌘0 shows Home, ⌘1–⌘8 select the Nth open review and ⌘9 the last (context `Window`, so dialogs keep their own ⌘1–⌘3).
- **Every page renders the whole shell** (sidebar + main column) through one helper, so the tree stays inside the review tab's key context. The sidebar's width lives in one window-level resizable state; the viewport | threads split stays per tab.
- **Widths:** the sidebar gives way first. It never takes more than the window minus the main column's minimum: 320 pt, or 480 pt while the threads panel shows (viewport 260 + panel 220). The 720 pt minimum window therefore always fits a 220 pt sidebar. The threads panel gives way next (to 220 pt) so the viewport keeps 260 pt; stored widths return when the window widens.
- **Opaque sidebar.** Translucency is deferred.
- Menu items that named tabs now name reviews ("Close Review", "Show Next Review"); action names are unchanged, so `keymap.json` files keep working.

## Consequences

- Traffic lights and their alignment cannot appear in headless screenshots; bounds tests pin the rows and one manual check in a real window pins the lights. In fullscreen the lights return to AppKit and the sidebar's inset collapses.
- With the sidebar hidden, the toolbar takes the traffic-light inset and a "show sidebar" button.
- Pane cycling skips the tree while the sidebar is hidden or shows Reviews; ⌘F, `/`, ⌘P and cycling into the tree switch it to Files first.
- The gpui-kit `TabBar` spring indicator and color fade disappear with the tab row.

## Alternatives rejected

| Option                                   | Why not                                                                                                          |
| ---------------------------------------- | ---------------------------------------------------------------------------------------------------------------- |
| Keep the tab row under an inset titlebar | Not the reference layout; costs a row of height                                                                  |
| Native macOS window tabs                 | Each tab is its own `NSWindow`, which breaks ADR-0023's one-window model and the shared sidebar                  |
| Sidebar rendered by `MainWindow`         | The focused tree would leave the review tab's key context: `⇥`, ⌘F, `R`, ⌘⇧⏎ and `i` would silently stop working |
| Translucent sidebar via `Blurred`        | Whole-window blur, Selection material, compositing cost on every frame, no Reduce Transparency read, untestable  |
| gpui-component `Sidebar`                 | A menu sidebar with a 200 ms width animation; it does not hold an accordion of trees                             |
