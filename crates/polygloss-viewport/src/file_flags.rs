//! Review state shown in file headers (design §11.6, §9): the Viewed
//! checkbox, "changed since viewed", open threads and agent threads.
//!
//! The host owns it. The viewport renders it and reports clicks on the
//! checkbox ([`crate::ViewportEvent::ViewedToggled`]); the host updates its
//! store and pushes the new flags back with [`DiffViewport::set_file_flags`].

use std::borrow::Cow;

use gpui_kit::Context;

use crate::view::DiffViewport;

/// One file's review state, as the header shows it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct FileFlags {
    /// The Viewed checkbox is checked.
    pub viewed: bool,
    /// Viewed in an earlier state of the file, changed since (design §9).
    pub changed_since_viewed: bool,
    /// Unresolved threads on the file.
    pub open_threads: u32,
    /// Some of the file's threads are an agent's.
    pub agent_threads: bool,
}

/// A review-state pill of a file header.
pub(crate) struct FlagBadge {
    pub text: Cow<'static, str>,
    /// Drawn in the accent color.
    pub accent: bool,
    /// Its icon (`icons/<name>.svg`), left of the text.
    pub icon: Option<&'static str>,
}

impl FileFlags {
    /// The header's review-state pills for this state, left to right.
    pub(crate) fn badges(&self) -> Vec<FlagBadge> {
        let mut badges = Vec::new();
        let mut push = |text, accent, icon| badges.push(FlagBadge { text, accent, icon });
        if self.changed_since_viewed {
            push(Cow::Borrowed("changed since viewed"), true, None);
        }
        let threads = Some("icons/message-square.svg");
        match self.open_threads {
            0 => {}
            1 => push(Cow::Borrowed("1 open thread"), false, threads),
            n => push(Cow::Owned(format!("{n} open threads")), false, threads),
        }
        if self.agent_threads {
            push(Cow::Borrowed("agent"), true, Some("icons/bot.svg"));
        }
        badges
    }
}

impl DiffViewport {
    /// Replaces every file's review state (one entry per file, in file
    /// order; missing entries are all-false, extra ones ignored), and with it
    /// the section bands' indicators.
    pub fn set_file_flags(&mut self, mut flags: Vec<FileFlags>, cx: &mut Context<Self>) {
        flags.resize(self.files.len(), FileFlags::default());
        if flags != self.flags {
            self.flags = flags;
            self.refresh_band_flags();
            cx.notify();
        }
    }

    /// Every file's review state, as last set.
    pub fn file_flags(&self) -> &[FileFlags] {
        &self.flags
    }
}
