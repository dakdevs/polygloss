//! The Submit review dialog (design §8.3, ADR-0011): an optional markdown
//! summary, the verdict (Comment, Approve or Request changes, GitHub's
//! order), how many drafts it publishes, and whether the agent is waiting
//! for it. The summary and verdict are autosaved (`Core::save_submit_draft`)
//! [`AUTOSAVE_DEBOUNCE`] after the last change and when the dialog closes
//! (Esc, Cancel, ×), and restored the next time it opens. Submitting (the button or
//! `⌘⏎`) runs `Core::submit_review` on the background executor, pinning a
//! live tab's displayed state first.

use std::time::Duration;

use futures::FutureExt as _;
use futures::future::Shared;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{
    Enter, IndentInline, InputEvent, OutdentInline, Textarea, TextareaState,
};
use gpui_kit::component::radio::Radio;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable as _, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Task, Window, div, px,
};
use polygloss_core::git::{LiveState, ResolvedSide};
use polygloss_core::review::{Submission, SubmitDraft, Verdict};

use crate::app_state::AppState;
use crate::space::{TextStyleExt as _, edge, gap, height, pad, radius, size, text};

/// The dialog's key context.
pub const CONTEXT: &str = "SubmitDialog";

gpui_kit::actions!(
    submit,
    [
        /// `⌘⏎` in the Submit review dialog: submit.
        ConfirmSubmit,
        /// `⌘1`: the verdict Comment (T5.6).
        VerdictComment,
        /// `⌘2`: the verdict Approve (T5.6).
        VerdictApprove,
        /// `⌘3`: the verdict Request changes (T5.6).
        VerdictRequestChanges,
    ]
);

/// A verdict's key in the dialog (`⌘1`…`⌘3`, the order the rows show).
pub fn verdict_key(v: Verdict) -> &'static str {
    match v {
        Verdict::Comment => "cmd-1",
        Verdict::Approve => "cmd-2",
        Verdict::RequestChanges => "cmd-3",
    }
}

/// How long the dialog waits after the last change before autosaving.
pub const AUTOSAVE_DEBOUNCE: Duration = Duration::from_millis(400);

/// The verdicts in the order the dialog lists them (GitHub's).
pub const VERDICTS: [Verdict; 3] = [Verdict::Comment, Verdict::Approve, Verdict::RequestChanges];

/// A verdict's label and explanation.
pub fn verdict_text(v: Verdict) -> (&'static str, &'static str) {
    match v {
        Verdict::Comment => (
            "Comment",
            "Submit general feedback without explicit approval.",
        ),
        Verdict::Approve => ("Approve", "The changes are good to go."),
        Verdict::RequestChanges => (
            "Request changes",
            "Feedback that must be addressed before this is done.",
        ),
    }
}

/// A verdict as the debug selectors name it.
pub fn verdict_slug(v: Verdict) -> &'static str {
    match v {
        Verdict::Comment => "comment",
        Verdict::Approve => "approve",
        Verdict::RequestChanges => "request-changes",
    }
}

/// Whether the agent will see the submission right away.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaiterState {
    /// The assigned session has a live waiter (`polygloss wait` or
    /// `wait_for_review`).
    Listening { agent: String },
    /// Assigned, but nothing is waiting.
    NotListening { agent: String },
    /// No agent session is assigned to the review.
    Unassigned,
}

impl WaiterState {
    /// The dialog's waiter line (design §8.3, T3.10).
    pub fn text(&self) -> String {
        match self {
            WaiterState::Listening { agent } => format!("{agent} is listening"),
            WaiterState::NotListening { agent } => {
                format!("{agent} isn't listening; it will see this on its next turn")
            }
            WaiterState::Unassigned => "No agent session is assigned to this review".to_owned(),
        }
    }
}

/// What the dialog opens with.
#[derive(Debug, Clone)]
pub struct DialogInit {
    pub review_id: String,
    /// Draft comments the submission publishes.
    pub drafts: u32,
    pub saved: Option<SubmitDraft>,
    pub waiter: WaiterState,
    /// The displayed base and live state of a live tab.
    pub live: Option<(ResolvedSide, LiveState)>,
}

/// What the dialog asks of its tab.
#[derive(Debug, Clone)]
pub enum SubmitEvent {
    /// Submitted: close the dialog, show the result.
    Submitted(Box<Submission>),
}

/// The dialog's content.
pub struct SubmitDialog {
    review_id: String,
    drafts: u32,
    waiter: WaiterState,
    live: Option<(ResolvedSide, LiveState)>,
    summary: Entity<TextareaState>,
    verdict: Verdict,
    submitting: bool,
    error: Option<SharedString>,
    /// The debounce of the next autosave.
    autosave: Option<Task<()>>,
    /// The summary or verdict changed since the last write.
    dirty: bool,
    /// The last background write (an autosave or the submission); the next
    /// one waits for it, so they land in order and an autosave never writes
    /// the dialog back after the submission consumed it.
    last_write: Option<Shared<Task<bool>>>,
    /// Autosaves written (tests).
    saves: u32,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl gpui_kit::EventEmitter<SubmitEvent> for SubmitDialog {}

impl SubmitDialog {
    pub fn new(init: DialogInit, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let text = init
            .saved
            .as_ref()
            .map(|s| s.summary_md.clone())
            .unwrap_or_default();
        let summary = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Leave a comment (optional)")
                .auto_grow(4, 12)
                .soft_wrap(true)
        });
        if !text.is_empty() {
            summary.update(cx, |s, cx| {
                s.set_value(text.as_str(), window, cx);
                s.set_selected_range(text.len()..text.len(), cx);
            });
        }
        // Closing the dialog (Esc, Cancel, ×) drops it, and with it the
        // debounce: write what it had not saved yet, and let a write in
        // flight finish.
        cx.on_release(|this: &mut SubmitDialog, cx: &mut App| {
            this.autosave = None;
            this.write_draft(cx);
            if let Some(write) = this.last_write.take() {
                cx.background_spawn(write).detach();
            }
        })
        .detach();
        let subscriptions = vec![cx.subscribe(
            &summary,
            |this: &mut SubmitDialog, _, event: &InputEvent, cx| {
                // The field is read-only while submitting; an edit then
                // would be written after the submission consumed the draft.
                if let InputEvent::Change = event
                    && !this.submitting
                {
                    this.error = None;
                    this.schedule_autosave(cx);
                    cx.notify();
                }
            },
        )];
        SubmitDialog {
            review_id: init.review_id,
            drafts: init.drafts,
            waiter: init.waiter,
            live: init.live,
            summary,
            verdict: init
                .saved
                .and_then(|s| s.verdict)
                .unwrap_or(Verdict::Comment),
            submitting: false,
            error: None,
            autosave: None,
            dirty: false,
            last_write: None,
            saves: 0,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    pub fn verdict(&self) -> Verdict {
        self.verdict
    }

    pub fn drafts(&self) -> u32 {
        self.drafts
    }

    pub fn waiter(&self) -> &WaiterState {
        &self.waiter
    }

    pub fn summary(&self, cx: &App) -> String {
        self.summary.read(cx).value().to_string()
    }

    /// The summary field (tests type into it).
    pub fn summary_input(&self) -> &Entity<TextareaState> {
        &self.summary
    }

    pub fn is_submitting(&self) -> bool {
        self.submitting
    }

    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    /// Autosaves written so far.
    pub fn autosaves(&self) -> u32 {
        self.saves
    }

    /// A background write (an autosave or the submission) has started and
    /// not finished.
    pub fn write_in_flight(&self) -> bool {
        self.last_write.as_ref().is_some_and(|w| w.peek().is_none())
    }

    /// The summary's focus handle (the dialog opens with the keyboard
    /// there).
    pub fn summary_focus(&self, cx: &App) -> FocusHandle {
        self.summary.focus_handle(cx)
    }

    /// Picks the verdict (ignored while the submission is in flight).
    pub fn set_verdict(&mut self, verdict: Verdict, cx: &mut Context<Self>) {
        if self.verdict != verdict && !self.submitting {
            self.verdict = verdict;
            self.schedule_autosave(cx);
            cx.notify();
        }
    }

    /// Saves the summary and verdict [`AUTOSAVE_DEBOUNCE`] after the last
    /// change.
    fn schedule_autosave(&mut self, cx: &mut Context<Self>) {
        self.dirty = true;
        self.autosave = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(AUTOSAVE_DEBOUNCE).await;
            let Ok(Some(write)) = this.update(cx, |this, cx| this.write_draft(cx)) else {
                return;
            };
            if write.await {
                this.update(cx, |this, _| this.saves += 1).ok();
            }
        }));
    }

    /// Writes the summary and verdict on the background executor if they
    /// changed since the last write, after the write before it.
    fn write_draft(&mut self, cx: &mut App) -> Option<Shared<Task<bool>>> {
        if !std::mem::take(&mut self.dirty) {
            return None;
        }
        let core = AppState::global(cx).core.clone();
        let (review, summary, verdict) = (self.review_id.clone(), self.summary(cx), self.verdict);
        let previous = self.last_write.take();
        let write = cx
            .background_spawn(async move {
                if let Some(previous) = previous {
                    previous.await;
                }
                match core.save_submit_draft(&review, &summary, Some(verdict)) {
                    Ok(()) => true,
                    Err(e) => {
                        tracing::warn!("autosaving the submit dialog: {e}");
                        false
                    }
                }
            })
            .shared();
        self.last_write = Some(write.clone());
        Some(write)
    }

    /// Submits the review (Submit review, `⌘⏎`).
    pub fn submit(&mut self, cx: &mut Context<Self>) {
        if self.submitting {
            return;
        }
        self.submitting = true;
        self.error = None;
        // The submission consumes the autosaved draft: a pending autosave
        // must not write it back, and one in flight lands first.
        self.autosave = None;
        let was_dirty = std::mem::take(&mut self.dirty);
        let previous = self.last_write.take();
        cx.notify();
        let core = AppState::global(cx).core.clone();
        let (review, summary, verdict, live) = (
            self.review_id.clone(),
            self.summary(cx),
            self.verdict,
            self.live.clone(),
        );
        let work = cx.background_spawn(async move {
            if let Some(previous) = previous {
                previous.await;
            }
            core.submit_review(
                &review,
                verdict,
                summary.trim(),
                live.as_ref().map(|(base, state)| (base, state)),
            )
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                this.submitting = false;
                match result {
                    Ok(sub) => cx.emit(SubmitEvent::Submitted(Box::new(sub))),
                    Err(e) => {
                        // Still open and unsaved: closing it writes it.
                        this.dirty |= was_dirty;
                        tracing::warn!("submitting review {}: {e}", this.review_id);
                        this.error = Some(format!("Could not submit: {e}").into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn verdict_row(&self, v: Verdict, cx: &mut Context<Self>) -> impl IntoElement {
        let (label, detail) = verdict_text(v);
        let slug = verdict_slug(v);
        let checked = self.verdict == v;
        let theme = cx.theme();
        // A two-line row (`ROW2`): the radio centred on the title's line,
        // `GROUP` before the label; the key cap at the right.
        h_flex()
            .id(SharedString::from(format!("submit-verdict-{slug}")))
            .debug_selector(move || format!("submit-verdict-{slug}"))
            .w_full()
            .min_h(px(height::ROW2))
            .px(px(pad::TEXT))
            .gap(px(gap::GROUP))
            .rounded(px(radius::for_height(height::ROW2)))
            .when(!self.submitting, |row| {
                row.cursor_pointer()
                    .hover(|s| s.bg(theme.secondary))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_verdict(v, cx)))
            })
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .items_start()
                    .gap(px(gap::GROUP))
                    .child(
                        div()
                            .debug_selector(move || format!("submit-verdict-radio-{slug}"))
                            .flex_none()
                            .h(px(text::UI.1))
                            .flex()
                            .items_center()
                            .child(
                                Radio::new(SharedString::from(format!("submit-radio-{slug}")))
                                    .checked(checked)
                                    .disabled(self.submitting)
                                    .on_click(cx.listener(move |this, _: &bool, _, cx| {
                                        this.set_verdict(v, cx)
                                    })),
                            ),
                    )
                    .child(
                        v_flex()
                            .debug_selector(move || format!("submit-verdict-label-{slug}"))
                            .min_w_0()
                            .child(
                                div()
                                    .text_style(text::UI)
                                    .font_medium()
                                    .text_color(if v == Verdict::RequestChanges && checked {
                                        theme.danger
                                    } else {
                                        theme.foreground
                                    })
                                    .child(label),
                            )
                            .child(
                                div()
                                    .text_style(text::SMALL)
                                    .text_color(theme.muted_foreground)
                                    .child(detail),
                            ),
                    ),
            )
            // Its key (T5.6).
            .children(
                gpui_kit::Keystroke::parse(verdict_key(v))
                    .ok()
                    .map(|k| crate::palette::key_cap::key_cap(&k, cx)),
            )
    }
}

impl Render for SubmitDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let verdicts: Vec<_> = VERDICTS
            .iter()
            .map(|v| self.verdict_row(*v, cx).into_any_element())
            .collect();
        let theme = cx.theme();
        let drafts = match self.drafts {
            0 => "No draft comments. You can still submit a verdict.".to_owned(),
            1 => "1 draft comment will be published.".to_owned(),
            n => format!("{n} draft comments will be published."),
        };
        let listening = matches!(self.waiter, WaiterState::Listening { .. });
        let waiter = h_flex()
            .debug_selector(|| "submit-waiter".into())
            .gap(px(gap::ICON_LABEL))
            .min_w_0()
            .text_style(text::SMALL)
            .text_color(theme.muted_foreground)
            .child(
                div()
                    .debug_selector(|| "submit-waiter-dot".into())
                    .flex_none()
                    .size(px(size::DOT))
                    .rounded_full()
                    .bg(if listening {
                        theme.success
                    } else {
                        theme.muted_foreground.opacity(0.5)
                    }),
            )
            .child(div().min_w_0().child(self.waiter.text()));
        v_flex()
            .debug_selector(|| "submit-dialog".into())
            .key_context(CONTEXT)
            .on_action(cx.listener(|this, _: &ConfirmSubmit, _, cx| this.submit(cx)))
            .on_action(
                cx.listener(|this, _: &VerdictComment, _, cx| {
                    this.set_verdict(Verdict::Comment, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &VerdictApprove, _, cx| {
                    this.set_verdict(Verdict::Approve, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &VerdictRequestChanges, _, cx| {
                this.set_verdict(Verdict::RequestChanges, cx)
            }))
            .track_focus(&self.focus)
            // ⌘⏎ in the summary submits (the field would insert a newline).
            .capture_action(cx.listener(|this, action: &Enter, _, cx| {
                if action.secondary {
                    cx.stop_propagation();
                    this.submit(cx);
                }
            }))
            // ⇥ / ⇧⇥ leave the summary for the verdicts and buttons (T5.6)
            // instead of indenting it (⌘] / ⌘[ still indent).
            .capture_action(cx.listener(|_, _: &IndentInline, window, cx| {
                cx.stop_propagation();
                crate::keyboard::focus_step(true, window, cx);
            }))
            .capture_action(cx.listener(|_, _: &OutdentInline, window, cx| {
                cx.stop_propagation();
                crate::keyboard::focus_step(false, window, cx);
            }))
            .w_full()
            .gap(px(gap::GROUP))
            .child(
                // The field's frame; the text area keeps its own inset.
                div()
                    .debug_selector(|| "submit-summary".into())
                    .rounded(px(radius::MD))
                    .border_1()
                    .border_color(theme.border)
                    .child(
                        Textarea::new(&self.summary)
                            .bordered(false)
                            .readonly(self.submitting)
                            .w_full(),
                    ),
            )
            // The verdicts touch, as a list's rows.
            .child(v_flex().children(verdicts))
            .child(
                // A compact card: the drafts and the waiter.
                v_flex()
                    .debug_selector(|| "submit-status".into())
                    .w_full()
                    .px(px(edge::COMPACT_X))
                    .py(px(edge::COMPACT_Y))
                    .rounded(px(radius::MD))
                    .bg(theme.secondary)
                    .child(
                        div()
                            .debug_selector(|| "submit-drafts".into())
                            .text_style(text::SMALL)
                            .text_color(theme.foreground)
                            .child(drafts),
                    )
                    .child(waiter),
            )
            .when_some(self.error.clone(), |el, err| {
                el.child(
                    div()
                        .debug_selector(|| "submit-error".into())
                        .text_style(text::SMALL)
                        .text_color(theme.danger)
                        .child(err),
                )
            })
            .child(
                h_flex()
                    .w_full()
                    .gap(px(gap::CONTROLS))
                    .child(div().flex_1())
                    .child(
                        Button::new("submit-cancel")
                            .debug_selector(|| "submit-cancel".into())
                            .small()
                            .ghost()
                            .label("Cancel")
                            .on_click(|_, window, cx| {
                                gpui_kit::component::WindowExt::close_dialog(window, cx)
                            }),
                    )
                    .child(
                        Button::new("submit-confirm")
                            .debug_selector(|| "submit-confirm".into())
                            .small()
                            .primary()
                            .label("Submit review")
                            .tooltip("⌘⏎")
                            .loading(self.submitting)
                            .disabled(self.submitting)
                            .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                    ),
            )
    }
}
