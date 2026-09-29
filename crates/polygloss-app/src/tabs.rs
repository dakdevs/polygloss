//! The main window's tabs (design §11.1, ADR-0023): Home first, then one tab
//! per review. [`Tabs`] is the plain model the window renders and its tab
//! actions drive.

use gpui_kit::{App, Entity, KeyBinding, SharedString};

use crate::home::HomeView;
use crate::review_tab::ReviewTab;

gpui_kit::actions!(
    window,
    [
        /// ⌘W: close the active review tab; on Home, close the window.
        CloseTab,
        /// ⌘⇧] (⌘}) or ⌃Tab: the next tab, wrapping around.
        NextTab,
        /// ⌘⇧[ (⌘{) or ⌃⇧Tab: the previous tab, wrapping around.
        PrevTab,
    ]
);

/// The tab key bindings (design §11.9 provisional macOS additions). T3.2's
/// keymap file can rebind them.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-w", CloseTab, None),
        KeyBinding::new("cmd-}", NextTab, None),
        KeyBinding::new("cmd-{", PrevTab, None),
        KeyBinding::new("ctrl-tab", NextTab, None),
        KeyBinding::new("ctrl-shift-tab", PrevTab, None),
    ]);
}

/// One tab of the main window.
#[derive(Clone)]
pub enum TabItem {
    Home(Entity<HomeView>),
    Review(Entity<ReviewTab>),
}

impl TabItem {
    pub fn review(&self) -> Option<&Entity<ReviewTab>> {
        match self {
            TabItem::Review(tab) => Some(tab),
            TabItem::Home(_) => None,
        }
    }

    /// The label on the tab.
    pub fn title(&self, cx: &App) -> SharedString {
        match self {
            TabItem::Home(_) => SharedString::new_static("Home"),
            TabItem::Review(tab) => tab.read(cx).title(),
        }
    }
}

/// The tabs in order and the active one. Home is always tab 0 and cannot be
/// closed.
pub struct Tabs {
    items: Vec<TabItem>,
    active: usize,
}

impl Tabs {
    pub fn new(home: Entity<HomeView>) -> Tabs {
        Tabs {
            items: vec![TabItem::Home(home)],
            active: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn active(&self) -> usize {
        self.active
    }

    pub fn active_item(&self) -> &TabItem {
        &self.items[self.active]
    }

    pub fn get(&self, ix: usize) -> Option<&TabItem> {
        self.items.get(ix)
    }

    pub fn items(&self) -> &[TabItem] {
        &self.items
    }

    /// Whether tab `ix` has a close button (every tab but Home).
    pub fn closable(&self, ix: usize) -> bool {
        matches!(self.items.get(ix), Some(TabItem::Review(_)))
    }

    /// The tab showing review `review_id`.
    pub fn find_review(&self, review_id: &str, cx: &App) -> Option<usize> {
        self.items.iter().position(|item| {
            item.review()
                .is_some_and(|tab| tab.read(cx).review_id == review_id)
        })
    }

    /// Adds a review tab after the others and activates it.
    pub fn push_review(&mut self, tab: Entity<ReviewTab>) -> usize {
        self.items.push(TabItem::Review(tab));
        self.active = self.items.len() - 1;
        self.active
    }

    /// Activates tab `ix` (ignored when out of range).
    pub fn activate(&mut self, ix: usize) {
        if ix < self.items.len() {
            self.active = ix;
        }
    }

    pub fn next(&mut self) {
        self.active = (self.active + 1) % self.items.len();
    }

    pub fn prev(&mut self) {
        self.active = (self.active + self.items.len() - 1) % self.items.len();
    }

    /// Closes tab `ix` if it can be closed; the tab to its left becomes
    /// active when it was the active one (or the active one moves left
    /// with it). Returns the closed tab.
    pub fn close(&mut self, ix: usize) -> Option<TabItem> {
        if !self.closable(ix) {
            return None;
        }
        let item = self.items.remove(ix);
        if self.active >= ix {
            self.active = self.active.saturating_sub(1).min(self.items.len() - 1);
        }
        Some(item)
    }
}
