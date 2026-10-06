//! The app's spacing tokens (T7.1, ADR-0031, design §11.17):
//! `polygloss_app::space` re-exports the viewport's groups and adds
//! `layout`, the app-only sizes, and the text helper.

use gpui_kit::{
    Context, InteractiveElement as _, IntoElement, ParentElement as _, Render, TestAppContext,
    Window, div, px,
};
use polygloss_app::space::{TextStyleExt as _, gap, height, layout, text};

#[test]
fn traffic_light_inset_clears_the_zoom_button() {
    // The macOS 26 probe: in a unified toolbar the three traffic lights are
    // 14 pt wide at x = 19, 42 and 65, so 9 pt apart.
    let (first_x, light, light_gap) = (19.0, 14.0, 9.0);
    let zoom_end = first_x + 3.0 * light + 2.0 * light_gap;
    assert_eq!(zoom_end, 79.0);
    assert_eq!(layout::TRAFFIC_LIGHTS_END, zoom_end);
    // One light gap after the zoom button: the show-sidebar button's inset
    // while the sidebar is hidden, and the first item one button and one
    // control gap later.
    assert_eq!(layout::TOOLBAR_INSET_HIDDEN, zoom_end + light_gap);
    assert_eq!(layout::TOOLBAR_INSET_HIDDEN, 88.0);
    assert_eq!(
        layout::TOOLBAR_INSET_HIDDEN + height::SM + gap::CONTROLS,
        120.0
    );
}

/// One line of text styled with `text_style`.
struct Line(text::Style);

impl Render for Line {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child(
            div()
                .debug_selector(|| "line".into())
                .text_style(self.0)
                .child("Total: 12 files"),
        )
    }
}

#[gpui_kit::test]
fn text_style_sets_the_line_height(cx: &mut TestAppContext) {
    // A single line of text is as tall as the line height the style names,
    // whatever its font's own metrics.
    for style in [text::SMALL, text::BODY, text::HEADING] {
        let (_, cx) = cx.add_window_view(|_, _| Line(style));
        cx.run_until_parked();
        let bounds = cx.debug_bounds("line").expect("the line is painted");
        assert_eq!(bounds.size.height, px(style.1), "{style:?}");
    }
}
