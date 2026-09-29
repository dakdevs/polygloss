//! Language detection over lumis's compiled-in grammars (the curated set in the
//! workspace `lumis` features, design §11.11).

use lumis::languages::Language as LumisLanguage;

/// How much of a blob's head [`guess_language`] looks at: the shebang and
/// Emacs mode line are on the first two lines, the doctype sniff at the start.
const HEAD_LIMIT: usize = 1024;

/// A lumis grammar that is compiled in. Never plain text: "no grammar" is
/// `None` wherever a `Language` is optional.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Language(LumisLanguage);

impl Language {
    /// Looks up a language by id, alias, file name or extension (`rust`, `rs`,
    /// `Makefile`), e.g. a Markdown code fence's info string.
    pub fn from_name(name: &str) -> Option<Language> {
        name.parse::<LumisLanguage>()
            .ok()
            .and_then(Language::highlightable)
    }

    /// lumis's id: `rust`, `typescript`, `tsx`, …
    pub fn id(&self) -> &'static str {
        self.0.id_name()
    }

    /// Display name: `Rust`, `TypeScript`, …
    pub fn name(&self) -> &'static str {
        self.0.name()
    }

    /// The compiled-in language whose lumis id is exactly `id`.
    fn from_id(id: &str) -> Option<Language> {
        let lang = id.parse::<LumisLanguage>().ok()?;
        (lang.id_name() == id)
            .then_some(lang)
            .and_then(Language::highlightable)
    }

    pub(crate) fn lumis(self) -> LumisLanguage {
        self.0
    }

    fn highlightable(l: LumisLanguage) -> Option<Language> {
        (l != LumisLanguage::PlainText).then_some(Language(l))
    }
}

/// Picks a grammar for the file at `path` (repo-relative, `/`-separated) whose
/// content starts with `head`: by file name or extension first, then by the
/// first line (shebang, Emacs mode line, doctype). `None` when nothing matches
/// or the match is plain text. Directory names never count.
pub fn guess_language(path: &str, head: &[u8]) -> Option<Language> {
    let file_name = path.rsplit('/').next().unwrap_or(path);
    // lumis's file-name index (globs such as `*.rs`, `Makefile`), not
    // `Language::from_name`: that treats a dotless name as an extension, so a
    // file called `tool` would be Bash (`*.tool`).
    if let Some(lang) =
        lumis::languages::language_id_for_filename(file_name).and_then(Language::from_id)
    {
        return Some(lang);
    }
    let head = &head[..head.len().min(HEAD_LIMIT)];
    let text = match std::str::from_utf8(head) {
        Ok(text) => text,
        // A cut multi-byte char or binary data: keep the valid prefix.
        Err(e) => std::str::from_utf8(&head[..e.valid_up_to()]).unwrap_or_default(),
    };
    Language::highlightable(LumisLanguage::guess(None, text))
}
