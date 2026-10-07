//! The banner strip (design §11.7, ADR-0009): a strip of fixed height under
//! the toolbar, on the canvas. Banners are rounded inline notices that never
//! insert rows into the viewport, and the strip never changes height, so
//! nothing below it moves when one comes or goes. With no banner the strip
//! shows its context line, which is empty on a review's latest state
//! (OQ-39).
//!
//! Motion (M12, ADR-0030), whoever caused the change: a notice enters once
//! per appearance of its kind, from 4 pt above and fading in over QUICK on
//! OUT, never again when its text changes. When the strip is left empty
//! (the last notice dismissed, a Refresh applied, a jump) the notices it
//! showed leave the way they came: an inert copy ([`Exit`]: no listeners,
//! no hitbox) over the empty strip rises 4 pt and fades over MICRO. The
//! strip's frame decides it, from what its last frame drew and what it shows
//! now, so the order of its callers' `clear` and `set_context` never
//! matters. A notice replaced by another notice or by the context line
//! leaves on the commit frame (a swap in place), and a notice that comes
//! back while its copy leaves enters from where the copy was. Under Reduce
//! Motion both only fade. A strip that was off screen (its tab hidden)
//! plays nothing that happened meanwhile.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex};
use gpui_kit::{
    Action, AnyElement, App, Bounds, Context, Div, FocusHandle, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, Point, Render, SharedString, Size, Styled as _,
    Window, canvas, div, point, px, relative,
};

use crate::motion::exit::{Exit, ExitHitbox, ExitSpec, Placement};
use crate::motion::settle::OnScreen;
use crate::motion::{
    self, Initiator, Motion, Recorded, Reduced, ReducedPlay, Settle, Track, quantize, tokens, wrap,
};
use crate::review_tab::toolbar::text;
use crate::space::{TextStyleExt as _, edge, gap, height, pad, radius, size, text as type_style};

/// A notice entering and leaving (ADR-0030 M12): QUICK in and MICRO out on
/// OUT, for every initiator (the strip is system-initiated); under Reduce
/// Motion opacity only.
pub const NOTICE: Motion = Motion {
    enter: tokens::QUICK,
    exit: tokens::MICRO,
    easing: tokens::out,
    animates: &[
        Initiator::Pointer,
        Initiator::Keyboard,
        Initiator::Programmatic,
    ],
    reduced: Reduced {
        enter: ReducedPlay::Fade,
        exit: ReducedPlay::Fade,
    },
};

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
    /// Its entrance, from its first frame (`None` until drawn).
    enter: Option<Track>,
}

/// A notice as drawn: what an exiting notice's copy shows.
#[derive(Debug, Clone, PartialEq)]
struct NoticeCopy {
    kind: BannerKind,
    text: SharedString,
    label: &'static str,
}

/// The notices a leaving copy shows (an [`Exit`]'s snapshot).
struct Leaving(Vec<NoticeCopy>);

/// The review tab's banner strip.
pub struct BannerStrip {
    /// Shown when no banner is: which iteration the tab shows, off its
    /// latest state; empty on it.
    context: SharedString,
    banners: Vec<Banner>,
    /// Where banner actions are dispatched (the review tab); `None`: from
    /// the focused element.
    target: Option<FocusHandle>,
    /// The notices the last frame drew: what leaves when the strip empties.
    drawn: Vec<NoticeCopy>,
    /// The strip's size as last laid out: the leaving copy's bounds.
    size: Size<Pixels>,
    /// Whether the strip was drawn on the window's last frame.
    on_screen: OnScreen,
    /// The leaving copy and whose notices it shows.
    exit: Option<(Vec<BannerKind>, Exit<Leaving>)>,
    /// Whether the last mouse down froze `exit`.
    exit_frozen: bool,
}

impl BannerStrip {
    pub fn new(context: SharedString) -> BannerStrip {
        BannerStrip {
            context,
            banners: Vec::new(),
            target: None,
            drawn: Vec::new(),
            size: Size::default(),
            on_screen: OnScreen::default(),
            exit: None,
            exit_frozen: false,
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
                self.banners.push(Banner {
                    kind,
                    text,
                    action,
                    enter: None,
                });
                self.banners.sort_by_key(|b| b.kind);
            }
        }
        cx.notify();
    }

    /// Hides the banner of `kind`. If the strip's next frame is empty, the
    /// notices it showed leave as they came (M12).
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

    /// Starts this frame's motion: a notice drawn for the first time enters
    /// (from where a leaving copy of it was), and when the strip is empty
    /// now, the notices its last frame drew leave; a leaving copy goes at
    /// once when the strip shows anything again. After frames off screen
    /// (its tab hidden) nothing that happened meanwhile plays: what left is
    /// gone and what was entering is in.
    fn begin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = cx.background_executor().now();
        let policy = motion::policy(cx);
        if !self.on_screen.was_drawn(window, cx) {
            self.drawn.clear();
            self.exit = None;
            for track in self.banners.iter_mut().filter_map(|b| b.enter.as_mut()) {
                track.settle();
            }
        }
        self.on_screen.mark(window, cx);
        let empty = self.banners.is_empty() && self.context.is_empty();
        // Back mid-exit: the copy's sampled opacity, and whose copy it was.
        let mut resumed: Option<(Vec<BannerKind>, f32)> = None;
        if empty && !self.drawn.is_empty() {
            let copies = std::mem::take(&mut self.drawn);
            let spec = ExitSpec {
                placement: Placement::Overlay(Bounds::new(Point::default(), self.size)),
                hitbox: ExitHitbox::None,
                motion: &NOTICE,
                travel: point(px(0.), px(-tokens::NUDGE)),
                height: None,
            };
            let kinds = copies.iter().map(|c| c.kind).collect();
            let start = Exit::start(
                Leaving(copies),
                copy,
                spec,
                Initiator::Programmatic,
                window,
                cx,
            );
            self.exit = start.map(|exit| (kinds, exit));
            self.exit_frozen = false;
        } else if !empty && let Some((kinds, mut exit)) = self.exit.take() {
            resumed = exit.frame(window, cx).map(|f| (kinds, f.opacity));
        }
        for b in &mut self.banners {
            if b.enter.is_none() {
                let from = resumed
                    .as_ref()
                    .filter(|(kinds, _)| kinds.contains(&b.kind))
                    .map_or(0.0, |&(_, opacity)| opacity);
                // Full: its offset and opacity continue from the copy's;
                // Reduced: its fade does.
                let mut track = Track::visible_at(from, 1.0, from);
                track.retarget(1.0, &NOTICE, Initiator::Programmatic, policy, now);
                b.enter = Some(track);
            }
        }
    }

    /// Whether an entrance or the exit is under way.
    fn moving(&self) -> bool {
        self.exit.is_some()
            || self
                .banners
                .iter()
                .any(|b| b.enter.as_ref().is_some_and(|t| !t.is_settled()))
    }

    /// While something moves: its freeze and settle with `motion::settle`.
    fn register(&self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.moving() {
            return;
        }
        let (freeze, settle) = (cx.weak_entity(), cx.weak_entity());
        motion::settle::register(
            None,
            move |now, cx| {
                freeze
                    .update(cx, |s, cx| {
                        for track in s.banners.iter_mut().filter_map(|b| b.enter.as_mut()) {
                            track.freeze(now);
                        }
                        if let Some((_, exit)) = &mut s.exit {
                            exit.freeze(now);
                            s.exit_frozen = true;
                        }
                        cx.notify();
                    })
                    .ok();
            },
            move |which, cx| {
                settle
                    .update(cx, |s, cx| {
                        for track in s.banners.iter_mut().filter_map(|b| b.enter.as_mut()) {
                            if which == Settle::All || track.is_frozen() {
                                track.settle();
                            }
                        }
                        if which == Settle::All || s.exit_frozen {
                            s.exit = None;
                        }
                        cx.notify();
                    })
                    .ok();
            },
            window,
            cx,
        );
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

/// A notice: the info tint, a dot, its text (shrinking and truncating, so
/// its button stays in view) and `button`, each with debug selector
/// `<prefix>-notice-{i}`.
fn notice(prefix: &'static str, i: usize, text: SharedString, button: AnyElement, cx: &App) -> Div {
    let theme = cx.theme();
    let (fg, info) = (theme.foreground, theme.info);
    h_flex()
        .debug_selector(move || format!("{prefix}-notice-{i}"))
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
        .child(div().min_w_0().text_color(fg).truncate().child(text))
        .child(button)
}

/// The strip's row of notices: on the cards' edge, `CONTROLS` apart.
fn row() -> Div {
    h_flex()
        .px(px(edge::CANVAS))
        .gap(px(gap::CONTROLS))
        .text_style(type_style::SMALL)
}

/// The leaving copy of `copies` ([`Exit`]'s render function): the strip's
/// row of notices with lookalikes of their buttons, and no listeners, focus
/// handles, tab stops or tooltips.
fn copy(Leaving(copies): &Leaving, _window: &mut Window, cx: &mut App) -> AnyElement {
    let notices: Vec<Div> = copies
        .iter()
        .enumerate()
        .map(|(i, c)| {
            notice(
                "banner-exit",
                i,
                c.text.clone(),
                inert_button(i, c.label, cx),
                cx,
            )
        })
        .collect();
    row().size_full().children(notices).into_any_element()
}

/// A notice's button drawn as gpui-kit draws an `.xsmall().ghost()`
/// `Button` with a label (gpui-component `button.rs`: `h_5`, `px_1`, its
/// `line_height(1.25)`, the label row's `text_xs` and `gap_1`), rounded as
/// the live one, with nothing interactive.
fn inert_button(i: usize, label: &'static str, cx: &App) -> AnyElement {
    div()
        .debug_selector(move || format!("banner-exit-button-{i}"))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .h_5()
        .px_1()
        .rounded(px(radius::XS))
        .text_color(cx.theme().secondary_foreground)
        .line_height(relative(1.25))
        .child(
            h_flex()
                .size_full()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .items_center()
                .justify_center()
                .text_xs()
                .gap_1()
                .child(
                    div()
                        .min_w_0()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(label),
                ),
        )
        .into_any_element()
}

impl Render for BannerStrip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.begin(window, cx);
        self.register(window, cx);
        let muted = cx.theme().muted_foreground;
        let canvas_color = crate::theme::viewport_theme(cx).canvas;
        let this = cx.weak_entity();
        let strip = row()
            .id("banner-strip")
            .debug_selector(|| "banner-strip".into())
            .relative()
            .flex_none()
            // Reserved whether or not a banner shows (OQ-39); its content on
            // the cards' edge.
            .h(px(height::BANNER_STRIP))
            .w_full()
            .overflow_hidden()
            .bg(canvas_color)
            // The strip's size, for a leaving copy laid over it.
            .child(
                canvas(
                    move |bounds, _, cx| {
                        this.update(cx, |s, _| s.size = bounds.size).ok();
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            );
        if self.banners.is_empty() {
            self.drawn.clear();
            let context = (!self.context.is_empty()).then(|| {
                div()
                    .debug_selector(|| "banner-context".into())
                    .min_w_0()
                    .text_color(muted)
                    .child(text("banner-context", self.context.clone()).truncate())
            });
            let leaving = self.exit.as_mut().and_then(|(_, exit)| {
                let (frame, element) = exit.draw(window, cx)?;
                report_exit(frame.offset, frame.opacity, cx);
                Some(element)
            });
            if leaving.is_none() {
                self.exit = None;
            }
            return strip.children(context).children(leaving);
        }
        let scale = window.scale_factor();
        let mut notices: Vec<AnyElement> = Vec::with_capacity(self.banners.len());
        let mut drawn = Vec::with_capacity(self.banners.len());
        for (i, b) in self.banners.iter_mut().enumerate() {
            let action = b.action.boxed_clone();
            let target = self.target.clone();
            let label = button_label(b.kind);
            drawn.push(NoticeCopy {
                kind: b.kind,
                text: b.text.clone(),
                label,
            });
            let button = Button::new(("banner", i))
                .flex_none()
                .label(label)
                .xsmall()
                .rounded(px(radius::XS))
                .ghost()
                .debug_selector(move || format!("banner-button-{i}"))
                .on_click(move |_, window, cx| match &target {
                    Some(target) => target.dispatch_action(&*action, window, cx),
                    None => window.dispatch_action(action.boxed_clone(), cx),
                })
                .into_any_element();
            let sample = b
                .enter
                .as_mut()
                .map(|track| motion::sample(track, window, cx))
                .map_or((1.0, 1.0), |s| (s.value, s.value * s.opacity));
            let (shown, opacity) = sample;
            let offset = point(px(0.), px(quantize(-tokens::NUDGE * (1.0 - shown), scale)));
            motion::report(
                || Recorded {
                    id: ("banner", b.kind as usize).into(),
                    offset,
                    opacity,
                    rotation: 0.0,
                },
                cx,
            );
            // Always wrapped (rule 13): the notice keeps one element tree.
            notices.push(wrap::slide(
                offset,
                notice("banner", i, b.text.clone(), button, cx).opacity(opacity),
            ));
        }
        self.drawn = drawn;
        strip.children(notices)
    }
}

/// Records what the leaving copy drew (tests: `motion::record`).
fn report_exit(offset: Point<Pixels>, opacity: f32, cx: &App) {
    motion::report(
        || Recorded {
            id: "banner-exit".into(),
            offset,
            opacity,
            rotation: 0.0,
        },
        cx,
    );
}
