//! The banner strip (design §11.7, ADR-0009): a strip of fixed height under
//! the toolbar. Banners never insert rows into the viewport and the strip
//! never changes height, so nothing below it moves when a banner comes or
//! goes. With no banner it shows what the tab compares (GitHub's "wants to
//! merge … into …" line).

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Action, Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Window, div, px,
};

/// The strip's height, reserved whether or not a banner shows.
pub const BANNER_STRIP_HEIGHT: f32 = 32.0;

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
}

/// The review tab's banner strip.
pub struct BannerStrip {
    /// Shown when no banner is (the base and head of the review).
    context: SharedString,
    banners: Vec<Banner>,
}

impl BannerStrip {
    pub fn new(context: SharedString) -> BannerStrip {
        BannerStrip {
            context,
            banners: Vec::new(),
        }
    }

    /// Shows (or replaces) the banner of `kind`; clicking it or its button
    /// dispatches `action`.
    pub fn set(
        &mut self,
        kind: BannerKind,
        text: SharedString,
        action: Box<dyn Action>,
        cx: &mut Context<Self>,
    ) {
        let banner = Banner { kind, text, action };
        match self.banners.iter_mut().find(|b| b.kind == kind) {
            Some(slot) => *slot = banner,
            None => {
                self.banners.push(banner);
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let strip = h_flex()
            .id("banner-strip")
            .debug_selector(|| "banner-strip".into())
            .flex_none()
            .h(px(BANNER_STRIP_HEIGHT))
            .w_full()
            .px_3()
            .gap_3()
            .overflow_hidden()
            .border_b_1()
            .border_color(theme.border)
            .text_sm();
        if self.banners.is_empty() {
            return strip.bg(theme.background).child(
                div()
                    .text_color(theme.muted_foreground)
                    .truncate()
                    .child(self.context.clone()),
            );
        }
        let (bg, fg) = (theme.info.opacity(0.12), theme.foreground);
        strip
            .bg(bg)
            .children(self.banners.iter().enumerate().map(|(i, b)| {
                let action = b.action.boxed_clone();
                h_flex()
                    .gap_2()
                    .when(i > 0, |d| d.pl_3().border_l_1().border_color(theme.border))
                    .child(div().size(px(6.)).rounded_full().bg(theme.info))
                    .child(div().text_color(fg).truncate().child(b.text.clone()))
                    .child(
                        Button::new(("banner", i))
                            .label(button_label(b.kind))
                            .xsmall()
                            .ghost()
                            .on_click(move |_, window, cx| {
                                window.dispatch_action(action.boxed_clone(), cx)
                            }),
                    )
            }))
    }
}
