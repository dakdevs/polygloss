//! Press and hover ink (T7.2, ADR-0030 Feedback without motion): a custom
//! control shows the foreground at α 0.06 under the pointer and α 0.12
//! while pressed, at once, drawn by Metal. Expected colors are blended in
//! the test from the theme's foreground and the canvas.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{
    AnyWindowHandle, Context, HeadlessAppContext, Hsla, InteractiveElement as _, IntoElement,
    Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _,
    PlatformInput, Point, Render, Styled as _, Window, div, point, px, white,
};
use image::RgbaImage;
use polygloss_app::motion::ink::PressInk as _;

use crate::support::Sandbox;
use crate::support::harness::Test;
use crate::support::screenshot;

pub const TESTS: &[Test] = &crate::tests![e2e_press_ink_shows_at_once_on_hover_and_press];

/// A white canvas with one 80 × 40 control at 100, 100.
struct Control;

impl Render for Control {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().bg(white()).child(
            div()
                .id("control")
                .absolute()
                .left(px(100.))
                .top(px(100.))
                .w(px(80.))
                .h(px(40.))
                .press_ink(cx),
        )
    }
}

fn dispatch(cx: &mut HeadlessAppContext, window: AnyWindowHandle, input: PlatformInput) {
    cx.update_window(window, |_, window, cx| window.dispatch_event(input, cx))
        .expect("window is open");
}

/// The control's center pixel in a fresh capture.
fn center(cx: &mut HeadlessAppContext, window: AnyWindowHandle) -> [u8; 3] {
    screenshot::draw(cx, window);
    let image: RgbaImage = screenshot::capture(cx, window);
    let p = image
        .get_pixel(140 * screenshot::SCALE, 120 * screenshot::SCALE)
        .0;
    [p[0], p[1], p[2]]
}

/// `color` at `alpha` over white, as 8-bit sRGB.
fn over_white(color: Hsla, alpha: f32) -> [u8; 3] {
    let c = color.to_rgb();
    let blend = |channel: f32| ((channel * alpha + (1.0 - alpha)) * 255.0).round() as u8;
    [blend(c.r), blend(c.g), blend(c.b)]
}

fn assert_close(actual: [u8; 3], expected: [u8; 3], what: &str) {
    let off = actual
        .iter()
        .zip(expected)
        .any(|(a, e)| a.abs_diff(e) > screenshot::CHANNEL_TOLERANCE);
    assert!(!off, "{what}: {actual:?}, expected {expected:?}");
}

fn e2e_press_ink_shows_at_once_on_hover_and_press() {
    let _sb = Sandbox::isolate();
    let mut cx = screenshot::headless_app();
    cx.update(gpui_kit::init);
    let window = *screenshot::open_window(&mut cx, |_, cx| {
        use gpui_kit::AppContext as _;
        cx.new(|_| Control)
    });
    let foreground = cx.update(|cx| cx.theme().foreground);
    let at: Point<_> = point(px(140.), px(120.));

    assert_close(center(&mut cx, window), [255, 255, 255], "at rest");
    dispatch(
        &mut cx,
        window,
        PlatformInput::MouseMove(MouseMoveEvent {
            position: at,
            pressed_button: None,
            modifiers: Modifiers::default(),
        }),
    );
    let hover = center(&mut cx, window);
    assert_close(hover, over_white(foreground, 0.06), "hovered");
    dispatch(
        &mut cx,
        window,
        PlatformInput::MouseDown(MouseDownEvent {
            position: at,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        }),
    );
    let pressed = center(&mut cx, window);
    assert_close(pressed, over_white(foreground, 0.12), "pressed");
    assert_ne!(pressed, hover, "pressed is darker than hovered");
    dispatch(
        &mut cx,
        window,
        PlatformInput::MouseUp(MouseUpEvent {
            position: at,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
            click_count: 1,
        }),
    );
    assert_close(
        center(&mut cx, window),
        over_white(foreground, 0.06),
        "released, still hovered",
    );
    screenshot::park_pointer(&mut cx, window);
    assert_close(center(&mut cx, window), [255, 255, 255], "the pointer left");
}
