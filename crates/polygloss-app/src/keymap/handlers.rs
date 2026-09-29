//! Action handlers scoped to a view: a feature registers a handler for view
//! type `V` once (in its `init`), and every `V` attaches the registered
//! handlers to its root element when it renders ([`apply`]). The review tab
//! and the main window do, so a handler registered for [`ReviewTab`] runs
//! only when focus is inside that tab (the keyboard, the palette and menus
//! dispatching on its focus), with the tab at hand.
//!
//! [`ReviewTab`]: crate::review_tab::ReviewTab

use std::rc::Rc;

use gpui_kit::{Action, App, Context, Div, Global, InteractiveElement as _, Window};

type Registrar<V> = Rc<dyn Fn(Div, &mut Context<V>) -> Div>;

/// The handlers registered for view type `V`.
struct Handlers<V: 'static>(Vec<Registrar<V>>);

impl<V: 'static> Default for Handlers<V> {
    fn default() -> Self {
        Handlers(Vec::new())
    }
}

impl<V: 'static> Global for Handlers<V> {}

/// Registers `handler` for action `A` on every view of type `V`.
pub fn on_action<V: 'static, A: Action>(
    cx: &mut App,
    handler: impl Fn(&mut V, &A, &mut Window, &mut Context<V>) + 'static,
) {
    let handler = Rc::new(handler);
    cx.default_global::<Handlers<V>>()
        .0
        .push(Rc::new(move |div: Div, cx: &mut Context<V>| {
            let handler = handler.clone();
            div.on_action(cx.listener(move |view: &mut V, action: &A, window, cx| {
                handler(view, action, window, cx)
            }))
        }));
}

/// `div` with every handler registered for `V` attached (call on the root
/// element of `V`'s render).
pub fn apply<V: 'static>(div: Div, cx: &mut Context<V>) -> Div {
    let Some(handlers) = cx.try_global::<Handlers<V>>() else {
        return div;
    };
    let registrars = handlers.0.clone();
    registrars
        .iter()
        .fold(div, |div, register| register(div, cx))
}
