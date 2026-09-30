//! The composer (design §8.7): a gpui-kit `Textarea` (auto-grow, IME,
//! undo/redo) with a Write/Preview toggle, a header naming what it comments
//! on, and Cancel / Save buttons. `⌘⏎` saves (`composer::SaveDraft`), `Esc`
//! cancels (`composer::Cancel`); both are key context `Composer`.
//!
//! The view only edits and reports ([`ComposerEvent`]); the review tab
//! saves, cancels and autosaves ([`super`]). gpui-kit's text field binds
//! `⌘⏎` itself (its `Enter { secondary }`, which would insert a newline
//! first), so the composer takes that action in the capture phase and saves
//! instead.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{
    Enter, Escape, IndentInline, InputEvent, OutdentInline, Textarea, TextareaState,
};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
};

use super::draft_store::ComposerKey;
use crate::keymap::actions::composer as actions;
use crate::keymap::actions::tab as tab_actions;

/// Rows the text field shows at least and grows to at most.
const MIN_ROWS: usize = 3;
const MAX_ROWS: usize = 14;

/// What a composer asks of its tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposerEvent {
    /// `⌘⏎` or the Save button.
    Save,
    /// `Esc` or the Cancel button.
    Cancel,
    /// The text changed (autosave).
    Changed,
}

/// One open composer.
pub struct Composer {
    key: ComposerKey,
    input: Entity<TextareaState>,
    preview: bool,
    saving: bool,
    error: Option<SharedString>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ComposerEvent> for Composer {}

impl Composer {
    /// A composer for `key` holding `text` (restored or the comment being
    /// edited), the caret at its end.
    pub fn new(key: ComposerKey, text: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let placeholder = match key {
            ComposerKey::Reply { .. } => "Reply…",
            _ => "Leave a comment",
        };
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(placeholder)
                .auto_grow(MIN_ROWS, MAX_ROWS)
                .soft_wrap(true)
        });
        if !text.is_empty() {
            input.update(cx, |state, cx| {
                state.set_value(text, window, cx);
                state.set_selected_range(text.len()..text.len(), cx);
            });
        }
        let subscriptions =
            vec![
                cx.subscribe(&input, |this: &mut Composer, _, event: &InputEvent, cx| {
                    if let InputEvent::Change = event {
                        this.error = None;
                        cx.emit(ComposerEvent::Changed);
                        cx.notify();
                    }
                }),
            ];
        Composer {
            key,
            input,
            preview: false,
            saving: false,
            error: None,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    pub fn key(&self) -> &ComposerKey {
        &self.key
    }

    /// The text as typed.
    pub fn text(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    /// The text field (tests type into it).
    pub fn input(&self) -> &Entity<TextareaState> {
        &self.input
    }

    pub fn is_preview(&self) -> bool {
        self.preview
    }

    pub fn is_saving(&self) -> bool {
        self.saving
    }

    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    /// Where the keyboard goes: the text field (or, in Preview, the
    /// composer).
    pub fn focus_target(&self, cx: &App) -> FocusHandle {
        if self.preview {
            self.focus.clone()
        } else {
            self.input.focus_handle(cx)
        }
    }

    /// Whether the keyboard is in this composer.
    pub fn contains_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus.contains_focused(window, cx) || self.input.focus_handle(cx).is_focused(window)
    }

    /// Switches between Write and Preview.
    pub fn toggle_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.preview = !self.preview;
        let handle = self.focus_target(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    /// Marks a save in flight (the buttons are disabled meanwhile).
    pub fn set_saving(&mut self, saving: bool, cx: &mut Context<Self>) {
        self.saving = saving;
        cx.notify();
    }

    /// Shows why the last save failed (cleared by the next edit).
    pub fn set_error(&mut self, error: Option<String>, cx: &mut Context<Self>) {
        self.error = error.map(Into::into);
        self.saving = false;
        cx.notify();
    }

    fn tab_button(
        &self,
        id: &'static str,
        label: &'static str,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .id(id)
            .debug_selector(move || id.to_owned())
            .px_2p5()
            .h(px(24.))
            .flex()
            .items_center()
            .rounded(px(5.))
            .text_xs()
            .cursor_pointer()
            .when(selected, |el| {
                el.bg(theme.background)
                    .border_1()
                    .border_color(theme.border)
                    .text_color(theme.foreground)
                    .font_medium()
            })
            .when(!selected, |el| {
                el.text_color(theme.muted_foreground)
                    .hover(|s| s.text_color(theme.foreground))
            })
            .child(label)
            .on_click(cx.listener(move |this, _, window, cx| {
                if this.preview == selected {
                    this.toggle_preview(window, cx);
                }
            }))
    }
}

impl Focusable for Composer {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        if self.preview {
            self.focus.clone()
        } else {
            self.input.focus_handle(cx)
        }
    }
}

impl Render for Composer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let key = self.key.to_string();
        let empty = self.input.read(cx).value().trim().is_empty();
        let body = if self.preview {
            let text = self.text(cx);
            let content = if text.trim().is_empty() {
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Nothing to preview")
                    .into_any_element()
            } else {
                crate::markdown::render_markdown(
                    SharedString::from(format!("composer-preview-{key}")),
                    &text,
                    cx,
                )
                .into_any_element()
            };
            div()
                .debug_selector({
                    let key = key.clone();
                    move || format!("composer-preview-{key}")
                })
                .min_h(px(64.))
                .px_3()
                .py_2()
                .text_sm()
                .child(content)
                .into_any_element()
        } else {
            div()
                .px_1()
                .py_1()
                .child(Textarea::new(&self.input).bordered(false).w_full())
                .into_any_element()
        };
        // The focus ring shows while the keyboard is in use (focus-visible).
        let focused = window.last_input_was_keyboard() && self.contains_focus(window, cx);
        let (write, preview) = (!self.preview, self.preview);
        let tabs = h_flex()
            .gap_1()
            .child(self.tab_button("composer-write", "Write", write, cx))
            .child(self.tab_button("composer-preview", "Preview", preview, cx));
        let theme = cx.theme();
        let header = h_flex()
            .w_full()
            .h(px(34.))
            .px_1p5()
            .gap_2()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.secondary)
            .child(tabs)
            .child(div().flex_1())
            .child(
                div()
                    .pr_1p5()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(self.key.title()),
            );
        let hint = h_flex()
            .gap_1()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child("Markdown · ⌘⏎ to save · Esc to cancel · ⇥ next pane");
        let error = self.error.clone();
        let footer = h_flex()
            .w_full()
            .px_2p5()
            .py_2()
            .gap_2()
            .border_t_1()
            .border_color(theme.border)
            .child(match error {
                Some(err) => div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(theme.danger)
                    .child(err)
                    .into_any_element(),
                None => div().flex_1().min_w_0().child(hint).into_any_element(),
            })
            .child(
                Button::new("composer-cancel")
                    .debug_selector({
                        let key = key.clone();
                        move || format!("composer-cancel-{key}")
                    })
                    .small()
                    .ghost()
                    .label("Cancel")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ComposerEvent::Cancel))),
            )
            .child(
                Button::new("composer-save")
                    .debug_selector({
                        let key = key.clone();
                        move || format!("composer-save-{key}")
                    })
                    .small()
                    .primary()
                    .label(self.key.save_label())
                    .loading(self.saving)
                    .disabled(self.saving || empty)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ComposerEvent::Save))),
            );
        v_flex()
            .id(SharedString::from(format!("composer-{key}")))
            .debug_selector(move || format!("composer-{key}"))
            .key_context("Composer")
            .track_focus(&self.focus)
            // gpui-kit binds ⌘⏎ in its text field (it would insert a
            // newline): save instead, before the field sees it.
            .capture_action(cx.listener(|_, action: &Enter, _, cx| {
                if action.secondary {
                    cx.stop_propagation();
                    cx.emit(ComposerEvent::Save);
                }
            }))
            .on_action(cx.listener(|_, _: &actions::SaveDraft, _, cx| cx.emit(ComposerEvent::Save)))
            .on_action(cx.listener(|_, _: &actions::Cancel, _, cx| cx.emit(ComposerEvent::Cancel)))
            // The text field's own Esc, which it passes on.
            .on_action(cx.listener(|_, _: &Escape, _, cx| cx.emit(ComposerEvent::Cancel)))
            .on_action(cx.listener(|this, _: &actions::TogglePreview, window, cx| {
                this.toggle_preview(window, cx)
            }))
            // ⇥ / ⇧⇥ go to the next or previous pane (T5.6) instead of
            // indenting (⌘] / ⌘[ still indent).
            .capture_action(cx.listener(|_, _: &IndentInline, window, cx| {
                cx.stop_propagation();
                window.dispatch_action(Box::new(tab_actions::FocusNextPane), cx);
            }))
            .capture_action(cx.listener(|_, _: &OutdentInline, window, cx| {
                cx.stop_propagation();
                window.dispatch_action(Box::new(tab_actions::FocusPrevPane), cx);
            }))
            .relative()
            .w_full()
            .rounded(px(6.))
            .border_1()
            .border_color(theme.ring.opacity(0.6))
            .bg(theme.background)
            .shadow_xs()
            .overflow_hidden()
            .child(header)
            .child(body)
            .child(footer)
            // The focus ring: a second pixel of ring while the keyboard is
            // here.
            .when(focused, |el| {
                let key = self.key.clone();
                el.child(
                    div()
                        .debug_selector(move || format!("composer-focus-ring-{key}"))
                        .absolute()
                        .inset_0()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme.ring),
                )
            })
    }
}
