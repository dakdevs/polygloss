//! Cursor, range, selection and the gutter "+" (T3.8, design §11.6), on a
//! split Pierre Light viewport over the code-change fixture: a line range on
//! the new side with the pointer on a line's numbers, and text selected on
//! the old side.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    AnyWindowHandle, AppContext as _, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PlatformInput, point, px,
};
use polygloss_app::CoreDiffProvider;
use polygloss_diff::Side;
use polygloss_highlight::Appearance;
use polygloss_viewport::{
    CursorPos, DiffProvider, DiffViewport, FrameStats, LayoutMode, ViewportDebug, ViewportEvent,
    ViewportOptions, ViewportTheme,
};

use crate::support::harness::Test;
use crate::support::screenshot::{self, assert_screenshot};
use crate::support::{Sandbox, code_change_repo, open_compare};

pub const TESTS: &[Test] = &crate::tests![e2e_cursor_range_and_plus, e2e_cursor_text_selection];

/// Frames drawn at most while waiting for loads and highlights.
const MAX_FRAMES: usize = 20;

fn dispatch(cx: &mut gpui_kit::HeadlessAppContext, window: AnyWindowHandle, input: PlatformInput) {
    cx.update_window(window, |_, window, cx| {
        window.dispatch_event(input, cx);
    })
    .expect("window is open");
    screenshot::draw(cx, window);
}

fn mouse_move(x: f32, y: f32, pressed: Option<MouseButton>) -> PlatformInput {
    PlatformInput::MouseMove(MouseMoveEvent {
        position: point(px(x), px(y)),
        pressed_button: pressed,
        modifiers: Modifiers::default(),
    })
}

/// Top and height of the visible row printed as `text`.
fn row(debug: &ViewportDebug, text: &str) -> (f32, f32) {
    let i = debug
        .visible_rows
        .iter()
        .position(|r| r.contains(text))
        .unwrap_or_else(|| panic!("no row {text:?} in {:#?}", debug.visible_rows));
    debug.row_bounds[i]
}

/// Left edge of the painted text `text`.
fn text_x(debug: &ViewportDebug, text: &str) -> f32 {
    debug
        .painted_text
        .iter()
        .find(|(_, _, t)| t == text)
        .map(|(x, _, _)| *x)
        .unwrap_or_else(|| panic!("{text:?} was not painted"))
}

/// A settled split Pierre Light viewport over the code-change fixture.
/// Fields drop in order: the viewport's handle before the app.
struct Setup {
    viewport: gpui_kit::Entity<DiffViewport>,
    window: AnyWindowHandle,
    cx: gpui_kit::HeadlessAppContext,
    _repo: polygloss_core::testing::FixtureRepo,
    _sb: Sandbox,
}

impl Setup {
    fn new() -> Setup {
        let sb = Sandbox::isolate();
        let repo = code_change_repo();
        let opened = open_compare(repo.path());
        let provider: Arc<dyn DiffProvider> = Arc::new(
            CoreDiffProvider::open(&opened, &default_categories()).expect("open the provider"),
        );
        let opts = ViewportOptions {
            layout: LayoutMode::Split,
            theme: Arc::new(ViewportTheme::pierre(Appearance::Light)),
            ..ViewportOptions::default()
        };
        // The app's asset source: the headers' Lucide icons (T6.7).
        let mut cx = screenshot::headless_app_with_assets(Arc::new(gpui_kit::assets::Assets));
        let window = screenshot::open_window(&mut cx, |window, cx| {
            cx.new(|cx| DiffViewport::new(provider, opts, window, cx))
        });
        let handle: AnyWindowHandle = *window;
        let viewport = window.root(&mut cx).expect("window has a root view");
        let frames: Rc<RefCell<Vec<FrameStats>>> = Rc::default();
        let sink = frames.clone();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&viewport, move |_, event: &ViewportEvent, _| {
                if let ViewportEvent::FrameStats(stats) = event {
                    sink.borrow_mut().push(*stats);
                }
            })
        });
        let settled = |frames: &[FrameStats]| {
            frames.last().is_some_and(|s| {
                s.visible_rows > 0 && s.loading_rows == 0 && s.unhighlighted_rows == 0
            })
        };
        for _ in 0..MAX_FRAMES {
            screenshot::draw(&mut cx, handle);
            if settled(&frames.borrow()) {
                break;
            }
        }
        screenshot::draw(&mut cx, handle);
        assert!(settled(&frames.borrow()), "the frame never settled");
        Setup {
            viewport,
            window: handle,
            cx,
            _repo: repo,
            _sb: sb,
        }
    }

    fn debug(&mut self) -> ViewportDebug {
        self.viewport.read_with(&self.cx, |v, _| v.debug())
    }

    fn input(&mut self, input: PlatformInput) {
        dispatch(&mut self.cx, self.window, input);
    }
}

fn e2e_cursor_range_and_plus() {
    let mut s = Setup::new();
    // A range over new lines 9–11 (`parse`'s first lines), cursor on 11.
    let range = CursorPos {
        file_idx: 0,
        side: Side::New,
        line: 10,
        range_start: Some(8),
    };
    let viewport = s.viewport.clone();
    s.cx.update(|cx| viewport.update(cx, |v, cx| v.set_cursor(Some(range), cx)));
    screenshot::draw(&mut s.cx, s.window);
    // The pointer on the new side's numbers of `fn get`.
    let debug = s.debug();
    let (y, h) = row(&debug, "pub fn get(&self, key: &str) -> Option<&str> {");
    s.input(mouse_move(648.0, y + h / 2.0, None));
    let debug = s.debug();
    let plus = debug.plus_button.expect("the + is shown");
    assert_eq!(plus.side, Side::New);
    assert_eq!(
        s.viewport.read_with(&s.cx, |v, _| v.cursor()),
        Some(range),
        "hovering leaves the cursor alone"
    );
    assert_screenshot(&screenshot::capture(&mut s.cx, s.window));
}

fn e2e_cursor_text_selection() {
    let mut s = Setup::new();
    // Drag across the old side's code from "std::" on line 1 to the end of
    // "/// A parsed" on line 3.
    let debug = s.debug();
    let code_x = text_x(&debug, "use std::collections::HashMap;");
    assert_eq!(
        text_x(&debug, "/// A parsed `key = value` config file."),
        code_x,
        "code starts at one column"
    );
    let advance = 0.6 * 13.0;
    let (use_y, use_h) = row(&debug, "use std::collections::HashMap;");
    let (doc_y, doc_h) = row(&debug, "/// A parsed `key = value` config file.");
    let from = (code_x + 4.0 * advance + 1.0, use_y + use_h / 2.0);
    let to = (code_x + 12.0 * advance + 1.0, doc_y + doc_h / 2.0);
    s.input(mouse_move(from.0, from.1, None));
    s.input(PlatformInput::MouseDown(MouseDownEvent {
        button: MouseButton::Left,
        position: point(px(from.0), px(from.1)),
        modifiers: Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    }));
    s.input(mouse_move(to.0, to.1, Some(MouseButton::Left)));
    s.input(PlatformInput::MouseUp(MouseUpEvent {
        button: MouseButton::Left,
        position: point(px(to.0), px(to.1)),
        modifiers: Modifiers::default(),
        click_count: 1,
    }));
    let selected = s.viewport.read_with(&s.cx, |v, _| v.selected_text());
    assert_eq!(
        selected.as_deref(),
        Some("std::collections::HashMap;\n\n/// A parsed")
    );
    screenshot::park_pointer(&mut s.cx, s.window);
    screenshot::draw(&mut s.cx, s.window);
    assert_eq!(s.debug().plus_button, None);
    assert_screenshot(&screenshot::capture(&mut s.cx, s.window));
}

/// The categorizer of the default settings (design §11.15): what a review
/// tab builds its provider with.
fn default_categories() -> polygloss_core::categories::Categorizer {
    polygloss_core::categories::Categorizer::new(&Default::default(), &[])
        .expect("the defaults compile")
}
