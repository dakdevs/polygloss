//! Review state shown in file headers (design §11.6, §9): the Viewed
//! checkbox, "changed since viewed", open threads and agent threads.
//!
//! The host owns it. The viewport renders it and reports clicks on the
//! checkbox ([`crate::ViewportEvent::ViewedToggled`]); the host updates its
//! store and pushes the new flags back with [`DiffViewport::set_file_flags`].

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

impl FileFlags {
    /// The header badges for this state, left to right, each with whether it
    /// is drawn in the accent color.
    pub(crate) fn badges(&self) -> Vec<(String, bool)> {
        let mut badges = Vec::new();
        if self.changed_since_viewed {
            badges.push(("changed since viewed".to_owned(), true));
        }
        match self.open_threads {
            0 => {}
            1 => badges.push(("1 open thread".to_owned(), false)),
            n => badges.push((format!("{n} open threads"), false)),
        }
        if self.agent_threads {
            badges.push(("agent".to_owned(), true));
        }
        badges
    }
}

impl DiffViewport {
    /// Replaces every file's review state (one entry per file, in file
    /// order; missing entries are all-false, extra ones ignored).
    pub fn set_file_flags(&mut self, mut flags: Vec<FileFlags>, cx: &mut Context<Self>) {
        flags.resize(self.files.len(), FileFlags::default());
        if flags != self.flags {
            self.flags = flags;
            cx.notify();
        }
    }

    /// Every file's review state, as last set.
    pub fn file_flags(&self) -> &[FileFlags] {
        &self.flags
    }
}
