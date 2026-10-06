//! The banner strip (design §11.7, ADR-0009): a strip of fixed height under
//! the toolbar, on the canvas. Banners are rounded inline notices that never
//! insert rows into the viewport, and the strip never changes height, so
//! nothing below it moves when one comes or goes. A notice enters (design
//! §11.16: 4 pt down and fading in) once per appearance of its kind, never
//! again when its text changes. With no banner the strip shows its context
//! line, which is empty on a review's latest state (OQ-39).

use std::time::Instant;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex};
use gpui_kit::{
    Action, AnyElement, Context, FocusHandle, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Window, div, point, px,
};

use crate::motion;
use crate::review_tab::toolbar::text;
use crate::space::{TextStyleExt as _, edge, gap, height, pad, radius, size, text as type_style};

/// How far above its place a notice starts entering.
const ENTER_FROM_ABOVE: f32 = 4.0;

/// The banners of design §11.7, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BannerKind {
    /// "N files changed · Refresh (R)" (the live watcher, T3.11).
    LiveChanges,
    /// "New iteration available · Refresh (R)" (compare refs moved, T3.11).
    NewIteration,
    /// "claude-code replied to N threads" (T3.13).
    AgentReplies,
    /// "Re-review requested" with the agent's summary (T3.13).
    Rereview,
}

struct Banner {
    kind: BannerKind,
    text: SharedString,
    action: Box<dyn Action>,
    /// Its appearance: the entrance's epoch.
    generation: u64,
    /// When its entrance began on the executor clock (its first frame).
    entered: Option<Instant>,
}

/// The review tab's banner strip.
pub struct BannerStrip {
    /// Shown when no banner is: which iteration the tab shows, off its
    /// latest state; empty on it.
    context: SharedString,
    banners: Vec<Banner>,
    /// Appearances so far, of any kind.
    appeared: u64,
    /// Where banner actions are dispatched (the review tab); `None`: from
    /// the focused element.
    target: Option<FocusHandle>,
}

impl BannerStrip {
    pub fn new(context: SharedString) -> BannerStrip {
        BannerStrip {
            context,
            banners: Vec::new(),
            appeared: 0,
            target: None,
        }
    }

    /// Dispatches banner actions on the element that tracks `target` (the
    /// review tab), so a click reaches the tab's handlers wherever focus
    /// is.
    pub fn with_target(mut self, target: FocusHandle) -> BannerStrip {
        self.target = Some(target);
        self
    }

    /// Shows (or updates) the banner of `kind`; clicking its button
    /// dispatches `action` (on the tab, see [`Self::with_target`]). A kind
    /// not shown yet enters; an update keeps its place and plays nothing.
    pub fn set(
        &mut self,
        kind: BannerKind,
        text: SharedString,
        action: Box<dyn Action>,
        cx: &mut Context<Self>,
    ) {
        match self.banners.iter_mut().find(|b| b.kind == kind) {
            Some(shown) => {
                shown.text = text;
                shown.action = action;
            }
            None => {
                self.appeared += 1;
                self.banners.push(Banner {
                    kind,
                    text,
                    action,
                    generation: self.appeared,
                    entered: None,
                });
                self.banners.sort_by_key(|b| b.kind);
            }
        }
        cx.notify();
    }

    /// Hides the banner of `kind`.
    pub fn clear(&mut self, kind: BannerKind, cx: &mut Context<Self>) {
        let before = self.banners.len();
        self.banners.retain(|b| b.kind != kind);
        if self.banners.len() != before {
            cx.notify();
        }
    }

    /// The banners shown, in order.
    pub fn banners(&self) -> Vec<(BannerKind, SharedString)> {
        self.banners
            .iter()
            .map(|b| (b.kind, b.text.clone()))
            .collect()
    }

    /// The action the button of the banner of `kind` dispatches, if it
    /// shows.
    pub fn action(&self, kind: BannerKind) -> Option<&dyn Action> {
        self.banners
            .iter()
            .find(|b| b.kind == kind)
            .map(|b| &*b.action)
    }

    /// The text shown without banners.
    pub fn context(&self) -> &SharedString {
        &self.context
    }

    pub fn set_context(&mut self, context: SharedString, cx: &mut Context<Self>) {
        self.context = context;
        cx.notify();
    }
}

/// The button label of a banner.
fn button_label(kind: BannerKind) -> &'static str {
    match kind {
        BannerKind::LiveChanges | BannerKind::NewIteration => "Refresh (R)",
        BannerKind::AgentReplies => "Show",
        BannerKind::Rereview => "View changes",
    }
}

impl Render for BannerStrip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = cx.background_executor().now();
        let theme = cx.theme();
        let (muted, fg, info) = (theme.muted_foreground, theme.foreground, theme.info);
        let canvas = crate::theme::viewport_theme(cx).canvas;
        let strip = h_flex()
            .id("banner-strip")
            .debug_selector(|| "banner-strip".into())
            .flex_none()
            // Reserved whether or not a banner shows (OQ-39); its content on
            // the cards' edge.
            .h(px(height::BANNER_STRIP))
            .w_full()
            .px(px(edge::CANVAS))
            .gap(px(gap::CONTROLS))
            .overflow_hidden()
            .bg(canvas)
            .text_style(type_style::SMALL);
        if self.banners.is_empty() {
            let context = (!self.context.is_empty()).then(|| {
                div()
                    .debug_selector(|| "banner-context".into())
                    .min_w_0()
                    .text_color(muted)
                    .child(text("banner-context", self.context.clone()).truncate())
            });
            return strip.children(context);
        }
        let mut notices: Vec<AnyElement> = Vec::with_capacity(self.banners.len());
        for (i, b) in self.banners.iter_mut().enumerate() {
            let action = b.action.boxed_clone();
            let target = self.target.clone();
            // Long texts shrink and truncate, so every button stays in view.
            let notice = h_flex()
                .debug_selector(move || format!("banner-notice-{i}"))
                .min_w_0()
                .h(px(height::SM))
                .pl(px(pad::TEXT))
                .gap(px(gap::CONTROLS))
                .rounded(px(radius::for_height(height::SM)))
                .border_1()
                .border_color(info.opacity(0.3))
                .bg(info.opacity(0.1))
                .child(
                    div()
                        .flex_none()
                        .size(px(size::DOT))
                        .rounded_full()
                        .bg(info),
                )
                .child(
                    div()
                        .min_w_0()
                        .text_color(fg)
                        .truncate()
                        .child(b.text.clone()),
                )
                .child(
                    Button::new(("banner", i))
                        .flex_none()
                        .label(button_label(b.kind))
                        .xsmall()
                        .rounded(px(radius::XS))
                        .ghost()
                        .debug_selector(move || format!("banner-button-{i}"))
                        .on_click(move |_, window, cx| match &target {
                            Some(target) => target.dispatch_action(&*action, window, cx),
                            None => window.dispatch_action(action.boxed_clone(), cx),
                        }),
                );
            // Wrapped only while it enters: `enter_from` would play again on
            // a frame after one without it (another review shown).
            let began = *b.entered.get_or_insert(now);
            notices.push(if now < began + motion::ENTER_NOTICE {
                motion::enter_from(
                    ("banner", b.kind as usize),
                    b.generation,
                    point(px(0.), px(-ENTER_FROM_ABOVE)),
                    motion::ENTER_NOTICE,
                    motion::ease_out_cubic,
                    notice,
                    window,
                    cx,
                )
            } else {
                notice.into_any_element()
            });
        }
        strip.children(notices)
    }
}
