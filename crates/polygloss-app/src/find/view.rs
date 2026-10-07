//! How find looks: while it is open it takes the left pane in place of the
//! file tree, like Xcode's Find navigator, so it never covers the diff. A
//! "FIND" header with the count, previous / next and close; the field with
//! the match-case and regex toggles; then the result list (each file's path
//! and count, then its matches: line number and the line with the match
//! highlighted), virtualized, the current match selected and kept in view.
//!
//! Spacing (ADR-0031 S1–S4, dense): the header is a pane bar
//! (`height::BAR`, its divider included) whose leading icon box and
//! trailing button icon sit on the sidebar's icon columns; the field has
//! the filter field's geometry (on the sidebar's edges, `RIM` below the
//! bar, `height::MD`), the toggles inside it; results are `height::SM` row
//! highlights on the sidebar's edge, their text `pad::TEXT` in, the
//! current match marked by a `CURSOR_BAR` rail. Rows show press ink.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Selectable as _, Sizable as _,
    StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, Div, Font, FontWeight, HighlightStyle, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, StyledText, Window, div, px, uniform_list,
};
use polygloss_diff::Side;

use super::{CONTEXT, FindBar, ListRow, ToggleCaseSensitive, ToggleRegex, file_label};
use crate::space::{TextStyleExt as _, edge, gap, height, layout, pad, radius, size, stroke, text};

impl FindBar {
    /// A result-list cell (line number or preview) in `font`: its family and
    /// its features, so previews draw `->` as typed when the diff does
    /// (ligatures off).
    pub fn preview_cell(font: &Font) -> Div {
        div()
            .font_family(font.family.clone())
            .font_features(font.features.clone())
    }
}

impl Render for FindBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let has_matches = !self.matches.is_empty();
        let status = self.status_label();
        let body = if self.rows.is_empty() {
            self.render_empty(cx)
        } else {
            self.render_list(cx)
        };
        let icon_button = |id: &'static str| {
            Button::new(id)
                .debug_selector(move || id.into())
                .ghost()
                .xsmall()
                .rounded(px(radius::XS))
        };
        // The close button's icon ends `ICON_LEAD` in from the sidebar's
        // edge, as leading icons start.
        let trailing = edge::SIDEBAR + pad::ICON_LEAD - (height::XS - size::ICON_XS) / 2.0;
        v_flex()
            .id("find-pane")
            .debug_selector(|| "find-pane".into())
            .key_context(CONTEXT)
            .on_action(cx.listener(Self::on_escape))
            .on_action(cx.listener(|bar, _: &ToggleCaseSensitive, _, cx| {
                bar.set_case_sensitive(!bar.options.case_sensitive, cx)
            }))
            .on_action(
                cx.listener(|bar, _: &ToggleRegex, _, cx| bar.set_regex(!bar.options.regex, cx)),
            )
            .size_full()
            .bg(theme.sidebar)
            .child(
                h_flex()
                    .debug_selector(|| "find-header".into())
                    .flex_none()
                    .h(px(height::BAR))
                    .pl(px(edge::SIDEBAR + pad::ICON_LEAD))
                    .pr(px(trailing))
                    .gap(px(gap::CONTROLS))
                    .border_b_1()
                    .border_color(theme.border)
                    .text_style(text::SMALL)
                    .font_semibold()
                    .text_color(theme.muted_foreground)
                    .child(
                        h_flex()
                            .flex_none()
                            .gap(px(gap::ICON_LABEL))
                            .child(
                                div()
                                    .debug_selector(|| "find-header-icon".into())
                                    .flex()
                                    .child(
                                        Icon::new(IconName::Search).with_size(px(size::ICON_SM)),
                                    ),
                            )
                            .child("FIND"),
                    )
                    .child(
                        div()
                            .id("find-status")
                            .debug_selector(|| "find-status".into())
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_normal()
                            .font_features(crate::chrome::tabular_figures())
                            .text_color(if self.error.is_some() {
                                theme.danger
                            } else {
                                theme.muted_foreground
                            })
                            .when_some(self.error.clone(), |el, e| {
                                el.tooltip(move |window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(e.clone())
                                        .build(window, cx)
                                })
                            })
                            .child(status),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .gap(px(gap::INLINE))
                            .child(
                                icon_button("find-prev")
                                    .icon(Icon::new(IconName::ChevronUp))
                                    .disabled(!has_matches)
                                    .tooltip("Previous match (⇧⏎)")
                                    .on_click(cx.listener(|bar, _, _, cx| bar.prev(cx))),
                            )
                            .child(
                                icon_button("find-next")
                                    .icon(Icon::new(IconName::ChevronDown))
                                    .disabled(!has_matches)
                                    .tooltip("Next match (⏎)")
                                    .on_click(cx.listener(|bar, _, _, cx| bar.next(cx))),
                            )
                            .child(
                                icon_button("find-close")
                                    .icon(Icon::new(IconName::Close))
                                    .tooltip("Close (Esc)")
                                    .on_click(
                                        cx.listener(|bar, _, window, cx| bar.close(window, cx)),
                                    ),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .px(px(edge::SIDEBAR))
                    .pt(px(pad::RIM))
                    .pb(px(gap::CONTROLS))
                    .child(
                        div().debug_selector(|| "find-input".into()).child(
                            Input::new(&self.input)
                                .small()
                                .prefix(
                                    Icon::new(IconName::Search)
                                        .with_size(px(size::ICON_SM))
                                        .text_color(theme.muted_foreground),
                                )
                                .suffix(
                                    h_flex()
                                        .gap(px(gap::INLINE))
                                        .child(
                                            icon_button("find-case-toggle")
                                                .icon(Icon::new(IconName::CaseSensitive))
                                                .selected(self.options.case_sensitive)
                                                .toggled(self.options.case_sensitive)
                                                .tooltip("Match case (⌥⌘C)")
                                                .on_click(cx.listener(|bar, _, _, cx| {
                                                    bar.set_case_sensitive(
                                                        !bar.options.case_sensitive,
                                                        cx,
                                                    )
                                                })),
                                        )
                                        .child(
                                            // No regex glyph in the bundled icon set: `.*`, as
                                            // editors label it.
                                            icon_button("find-regex-toggle")
                                                .label(".*")
                                                .selected(self.options.regex)
                                                .toggled(self.options.regex)
                                                .accessibility_label("Regular expression")
                                                .tooltip("Regular expression (⌥⌘R)")
                                                .on_click(cx.listener(|bar, _, _, cx| {
                                                    bar.set_regex(!bar.options.regex, cx)
                                                })),
                                        ),
                                )
                                .h(px(height::MD))
                                // The kit's field draws its border inside: the icon box
                                // stays `ICON_LEAD` from the field's edge.
                                .pl(px(pad::ICON_LEAD - stroke::BORDER))
                                .gap(px(gap::ICON_LABEL))
                                .rounded(px(radius::for_height(height::MD))),
                        ),
                    ),
            )
            .child(body)
    }
}

impl FindBar {
    /// What the list area says without results.
    fn render_empty(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (title, detail) = if let Some(e) = &self.error {
            ("Invalid pattern", Some(e.clone()))
        } else if self.query.is_empty() {
            (
                "Find in all files",
                Some("Searches both sides of every file, loaded or not.".to_owned()),
            )
        } else if self.searching {
            ("Searching…", None)
        } else {
            ("No matches", None)
        };
        v_flex()
            .flex_1()
            .min_h_0()
            .px(px(edge::SIDEBAR))
            .items_center()
            .justify_center()
            .text_center()
            .text_style(text::UI)
            .text_color(theme.muted_foreground)
            .child(title)
            .when_some(detail, |el, d| {
                el.child(div().text_style(text::SMALL).child(d))
            })
            .into_any_element()
    }

    fn render_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        div()
            .debug_selector(|| "find-results".into())
            .flex_1()
            .min_h_0()
            .border_t_1()
            .border_color(theme.border)
            .child(
                uniform_list(
                    "find-results",
                    self.rows.len(),
                    cx.processor(|bar: &mut FindBar, range: std::ops::Range<usize>, _, cx| {
                        range.map(|i| bar.render_row(i, cx)).collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.list_scroll)
                .px(px(edge::SIDEBAR))
                .py(px(gap::INLINE))
                .size_full(),
            )
            .into_any_element()
    }

    fn render_row(&self, i: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(&row) = self.rows.get(i) else {
            return div().into_any_element();
        };
        let radius = radius::for_height(height::SM);
        let base = h_flex()
            .id(("find-row", i))
            .relative()
            .w_full()
            .h(px(height::SM))
            .px(px(pad::TEXT))
            .gap(px(gap::ICON_LABEL))
            .rounded(px(radius))
            .text_style(text::SMALL)
            .cursor_pointer()
            .child(crate::chrome::ink_layer(radius, cx));
        match row {
            ListRow::File(f) => {
                let (path, generated) = file_label(self.viewport.read(cx), f);
                let (dir, name) = match path.rsplit_once('/') {
                    Some((dir, name)) => (dir.to_owned(), name.to_owned()),
                    None => (String::new(), path.to_string()),
                };
                let count = self.file_counts.get(&f).copied().unwrap_or(0);
                let first = self.rows.get(i + 1).and_then(|r| match r {
                    ListRow::Match(ix) => Some(*ix),
                    ListRow::File(_) => None,
                });
                base.debug_selector(move || format!("find-file-{f}"))
                    .child(
                        div()
                            .flex_none()
                            .font_semibold()
                            .text_color(theme.foreground)
                            .child(SharedString::from(name)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis_start()
                            .text_color(theme.muted_foreground)
                            .child(SharedString::from(dir)),
                    )
                    .when(generated, |el| {
                        el.child(
                            div()
                                .flex_none()
                                .px(px(pad::BADGE_X))
                                .rounded_full()
                                .bg(theme.secondary)
                                .text_color(theme.secondary_foreground)
                                .child("generated"),
                        )
                    })
                    .child(
                        div()
                            .flex_none()
                            .px(px(pad::BADGE_X))
                            .rounded_full()
                            .bg(theme.muted)
                            .font_features(crate::chrome::tabular_figures())
                            .text_color(theme.muted_foreground)
                            .child(super::thousands(count)),
                    )
                    .when_some(first, |el, ix| {
                        el.on_click(cx.listener(move |bar, _, _, cx| bar.go_to(ix, cx)))
                    })
                    .into_any_element()
            }
            ListRow::Match(ix) => {
                let Some(m) = self.matches.get(ix) else {
                    return div().into_any_element();
                };
                let selected = self.current == Some(ix);
                let code_font = self.preview_font(cx);
                let (number, number_color) = match m.side {
                    Side::Old => (format!("−{}", m.line + 1), theme.red),
                    Side::New => (format!("{}", m.line + 1), theme.muted_foreground),
                };
                let highlight = HighlightStyle {
                    background_color: Some(theme.yellow.opacity(0.35)),
                    font_weight: Some(FontWeight::SEMIBOLD),
                    ..HighlightStyle::default()
                };
                let text = StyledText::new(m.preview.clone())
                    .with_highlights([(m.preview_match.clone(), highlight)]);
                base.debug_selector(move || format!("find-match-{ix}"))
                    // The selection: the list's active color and an accent
                    // rail, as the diff marks the cursor line; it takes no
                    // room, so the text never moves.
                    .when(selected, |el| {
                        el.bg(theme.list_active).child(
                            div()
                                .absolute()
                                .left_0()
                                .top_0()
                                .bottom_0()
                                .w(px(stroke::CURSOR_BAR))
                                .bg(theme.primary),
                        )
                    })
                    .on_click(cx.listener(move |bar, _, _, cx| bar.go_to(ix, cx)))
                    .child(
                        FindBar::preview_cell(&code_font)
                            .flex_none()
                            .w(px(layout::FIND_NUMBER_COL))
                            .text_right()
                            .text_color(number_color)
                            .child(SharedString::from(number)),
                    )
                    .child(
                        FindBar::preview_cell(&code_font)
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(theme.foreground)
                            .child(text),
                    )
                    .into_any_element()
            }
        }
    }
}
