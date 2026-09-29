//! Shared plain types: object ids and formats, sides, file statuses and kinds,
//! modes, git paths and `FileChange` (one `diff-tree` entry).

use std::fmt;

use serde::{Deserialize, Serialize};

/// A repository's object format (`git rev-parse --show-object-format`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ObjectFormat {
    Sha1,
    Sha256,
}

impl ObjectFormat {
    /// `"sha1"` or `"sha256"`, as git prints it and as `diff_id` hashes it.
    pub fn as_str(&self) -> &'static str {
        match self {
            ObjectFormat::Sha1 => "sha1",
            ObjectFormat::Sha256 => "sha256",
        }
    }

    /// Parses git's name for the format; `None` for anything else.
    pub fn from_name(name: &str) -> Option<ObjectFormat> {
        match name {
            "sha1" => Some(ObjectFormat::Sha1),
            "sha256" => Some(ObjectFormat::Sha256),
            _ => None,
        }
    }

    /// Length of an object id in hex characters: 40 or 64.
    pub fn hex_len(&self) -> usize {
        match self {
            ObjectFormat::Sha1 => 40,
            ObjectFormat::Sha256 => 64,
        }
    }

    /// The empty tree's id in this format (design §3).
    pub fn empty_tree(&self) -> Oid {
        Oid(match self {
            ObjectFormat::Sha1 => "4b825dc642cb6eb9a060e54bf8d69288fbee4904",
            ObjectFormat::Sha256 => {
                "6ef19b41225c5369f1c104d45d8d85efa9b057b53b14b4b9b939dd74decc5321"
            }
        }
        .to_owned())
    }

    fn from_hex_len(len: usize) -> Option<ObjectFormat> {
        match len {
            40 => Some(ObjectFormat::Sha1),
            64 => Some(ObjectFormat::Sha256),
            _ => None,
        }
    }
}

/// Why a string is not an [`Oid`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OidError {
    #[error("object id {oid:?} has {actual} chars, expected {expected}")]
    WrongLength {
        oid: String,
        expected: usize,
        actual: usize,
    },
    #[error("object id {0:?} is not lowercase hex")]
    NotLowercaseHex(String),
}

/// A git object id: lowercase hex of its format's length (40 or 64 chars).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Oid(String);

impl Oid {
    /// Validates `s` as an id of format `fmt`. Upper case is rejected, not folded:
    /// every id we store comes from git, which prints lower case.
    pub fn parse(s: &str, fmt: ObjectFormat) -> Result<Oid, OidError> {
        if !s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(OidError::NotLowercaseHex(s.to_owned()));
        }
        if s.len() != fmt.hex_len() {
            return Err(OidError::WrongLength {
                oid: s.to_owned(),
                expected: fmt.hex_len(),
                actual: s.len(),
            });
        }
        Ok(Oid(s.to_owned()))
    }

    /// The all-zero id git uses for "no object" (added/deleted sides).
    pub fn zero(fmt: ObjectFormat) -> Oid {
        Oid("0".repeat(fmt.hex_len()))
    }

    pub fn is_zero(&self) -> bool {
        self.0.bytes().all(|b| b == b'0')
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The first 7 hex chars, for display.
    pub fn short(&self) -> &str {
        &self.0[..7]
    }

    /// The format implied by the id's length.
    pub fn object_format(&self) -> ObjectFormat {
        if self.0.len() == ObjectFormat::Sha1.hex_len() {
            ObjectFormat::Sha1
        } else {
            ObjectFormat::Sha256
        }
    }
}

impl fmt::Display for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for Oid {
    type Error = OidError;

    /// Accepts either format, chosen by length.
    fn try_from(s: String) -> Result<Oid, OidError> {
        match ObjectFormat::from_hex_len(s.len()) {
            Some(fmt) => Oid::parse(&s, fmt),
            None => Err(OidError::WrongLength {
                expected: ObjectFormat::Sha1.hex_len(),
                actual: s.len(),
                oid: s,
            }),
        }
    }
}

impl From<Oid> for String {
    fn from(oid: Oid) -> String {
        oid.0
    }
}

/// The old (base) or new (head) side of a diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Old,
    New,
}

/// A `diff-tree --raw` status. Copies never occur because `-C` is off (design §6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    TypeChanged,
}

impl FileStatus {
    /// Maps git's raw status letter (`A`, `M`, `D`, `R`, `T`); `None` for others.
    pub fn from_raw(letter: u8) -> Option<FileStatus> {
        match letter {
            b'A' => Some(FileStatus::Added),
            b'M' => Some(FileStatus::Modified),
            b'D' => Some(FileStatus::Deleted),
            b'R' => Some(FileStatus::Renamed),
            b'T' => Some(FileStatus::TypeChanged),
            _ => None,
        }
    }

    pub fn as_letter(&self) -> char {
        match self {
            FileStatus::Added => 'A',
            FileStatus::Modified => 'M',
            FileStatus::Deleted => 'D',
            FileStatus::Renamed => 'R',
            FileStatus::TypeChanged => 'T',
        }
    }
}

/// How a file's content is shown (design §6.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileKind {
    Text,
    Binary,
    Symlink,
    Submodule,
}

/// A git file mode. `Display` prints the 6-digit octal form, e.g. `100644`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Mode(pub u32);

impl Mode {
    pub const SYMLINK: Mode = Mode(0o120000);
    pub const SUBMODULE: Mode = Mode(0o160000);

    /// Parses git's octal mode text (`100644`).
    pub fn parse_octal(s: &str) -> Option<Mode> {
        if s.is_empty() || !s.bytes().all(|b| matches!(b, b'0'..=b'7')) {
            return None;
        }
        u32::from_str_radix(s, 8).ok().map(Mode)
    }

    pub fn is_symlink(&self) -> bool {
        *self == Mode::SYMLINK
    }

    pub fn is_submodule(&self) -> bool {
        *self == Mode::SUBMODULE
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:06o}", self.0)
    }
}

/// A path as git reports it. UTF-8 paths are kept as is. Other byte strings are
/// stored in git's C-style quoted form (as `core.quotePath=true` prints them,
/// surrounding quotes included) with `escaped = true` (design OQ-25).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GitPath {
    pub text: String,
    pub escaped: bool,
}

impl GitPath {
    pub fn from_bytes(bytes: &[u8]) -> GitPath {
        match std::str::from_utf8(bytes) {
            Ok(text) => GitPath {
                text: text.to_owned(),
                escaped: false,
            },
            Err(_) => GitPath {
                text: quote_c_style(bytes),
                escaped: true,
            },
        }
    }

    /// The raw path bytes, for handing the path back to git.
    pub fn to_bytes(&self) -> Vec<u8> {
        if self.escaped {
            unquote_c_style(&self.text)
        } else {
            self.text.as_bytes().to_vec()
        }
    }
}

/// git's `quote_c_style` with `core.quotePath=true`: control bytes, `"`, `\`,
/// DEL and every byte >= 0x80 are escaped; the result is wrapped in quotes.
fn quote_c_style(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() + 2);
    out.push('"');
    for &b in bytes {
        match b {
            0x07 => out.push_str("\\a"),
            0x08 => out.push_str("\\b"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            0x0b => out.push_str("\\v"),
            0x0c => out.push_str("\\f"),
            b'\r' => out.push_str("\\r"),
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\{b:03o}")),
        }
    }
    out.push('"');
    out
}

/// Inverse of [`quote_c_style`] for the strings it produces.
fn unquote_c_style(text: &str) -> Vec<u8> {
    let inner = text
        .strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .unwrap_or(text)
        .as_bytes();
    let mut out = Vec::with_capacity(inner.len());
    let mut i = 0;
    while i < inner.len() {
        let b = inner[i];
        i += 1;
        if b != b'\\' || i >= inner.len() {
            out.push(b);
            continue;
        }
        let e = inner[i];
        i += 1;
        out.push(match e {
            b'a' => 0x07,
            b'b' => 0x08,
            b't' => b'\t',
            b'n' => b'\n',
            b'v' => 0x0b,
            b'f' => 0x0c,
            b'r' => b'\r',
            b'0'..=b'3' if is_octal(inner.get(i)) && is_octal(inner.get(i + 1)) => {
                let v = (e - b'0') * 64 + (inner[i] - b'0') * 8 + (inner[i + 1] - b'0');
                i += 2;
                v
            }
            other => other,
        });
    }
    out
}

fn is_octal(b: Option<&u8>) -> bool {
    b.is_some_and(|d| (b'0'..=b'7').contains(d))
}

/// One entry of `diff-tree -r -z --raw -M50%`: a file's status, paths, modes,
/// blobs, rename similarity and classification. `idx` is its position in git's
/// output order.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileChange {
    pub idx: u32,
    pub status: FileStatus,
    pub old_path: Option<GitPath>,
    pub new_path: Option<GitPath>,
    pub old_mode: Option<Mode>,
    pub new_mode: Option<Mode>,
    pub old_blob: Oid,
    pub new_blob: Oid,
    pub similarity: Option<u8>,
    pub kind: FileKind,
    pub generated: bool,
}

impl FileChange {
    /// The new path, or the old one for deletions.
    pub fn display_path(&self) -> &str {
        self.new_path
            .as_ref()
            .or(self.old_path.as_ref())
            .map_or("", |p| p.text.as_str())
    }

    /// The Viewed key `(path, old_blob, new_blob)` (design §9, ADR-0022). Added and
    /// deleted files already carry git's all-zero id on the missing side.
    pub fn viewed_key(&self) -> (String, Oid, Oid) {
        (
            self.display_path().to_owned(),
            self.old_blob.clone(),
            self.new_blob.clone(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA1_EMPTY: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
    const SHA256_EMPTY: &str = "6ef19b41225c5369f1c104d45d8d85efa9b057b53b14b4b9b939dd74decc5321";

    #[test]
    fn oid_parse_rejects_uppercase_and_wrong_length() {
        let upper = SHA1_EMPTY.to_uppercase();
        assert!(matches!(
            Oid::parse(&upper, ObjectFormat::Sha1),
            Err(OidError::NotLowercaseHex(_))
        ));
        assert!(matches!(
            Oid::parse(&SHA1_EMPTY[..39], ObjectFormat::Sha1),
            Err(OidError::WrongLength {
                expected: 40,
                actual: 39,
                ..
            })
        ));
        // A valid sha256 id is the wrong length for a sha1 repo and vice versa.
        assert!(Oid::parse(SHA256_EMPTY, ObjectFormat::Sha1).is_err());
        assert!(Oid::parse(SHA1_EMPTY, ObjectFormat::Sha256).is_err());
        assert!(Oid::parse("", ObjectFormat::Sha1).is_err());
        let not_hex = format!("{}g", &SHA1_EMPTY[..39]);
        assert!(matches!(
            Oid::parse(&not_hex, ObjectFormat::Sha1),
            Err(OidError::NotLowercaseHex(_))
        ));
        // Multi-byte chars must not be counted as hex digits or panic on slicing.
        let multibyte = format!("{}é", &SHA1_EMPTY[..38]);
        assert!(Oid::parse(&multibyte, ObjectFormat::Sha1).is_err());

        let ok = Oid::parse(SHA1_EMPTY, ObjectFormat::Sha1).unwrap();
        assert_eq!(ok.as_str(), SHA1_EMPTY);
        assert_eq!(ok.short(), "4b825dc");
        assert_eq!(ok.object_format(), ObjectFormat::Sha1);
        assert_eq!(ok.to_string(), SHA1_EMPTY);
    }

    #[test]
    fn oid_zero_roundtrip() {
        for fmt in [ObjectFormat::Sha1, ObjectFormat::Sha256] {
            let zero = Oid::zero(fmt);
            assert!(zero.is_zero());
            assert_eq!(zero.as_str().len(), fmt.hex_len());
            assert!(zero.as_str().bytes().all(|b| b == b'0'));
            let reparsed = Oid::parse(zero.as_str(), fmt).unwrap();
            assert_eq!(reparsed, zero);
            assert!(reparsed.is_zero());
            assert!(!fmt.empty_tree().is_zero());
        }
    }

    #[test]
    fn empty_tree_constants_match_design() {
        // design §3 "Empty tree", both checked with git 2.54.
        assert_eq!(ObjectFormat::Sha1.empty_tree().as_str(), SHA1_EMPTY);
        assert_eq!(ObjectFormat::Sha256.empty_tree().as_str(), SHA256_EMPTY);
        assert_eq!(ObjectFormat::Sha1.as_str(), "sha1");
        assert_eq!(ObjectFormat::Sha256.as_str(), "sha256");
        assert_eq!(ObjectFormat::Sha1.hex_len(), 40);
        assert_eq!(ObjectFormat::Sha256.hex_len(), 64);
        assert_eq!(ObjectFormat::from_name("sha1"), Some(ObjectFormat::Sha1));
        assert_eq!(
            ObjectFormat::from_name("sha256"),
            Some(ObjectFormat::Sha256)
        );
        assert_eq!(ObjectFormat::from_name("SHA1"), None);
    }

    #[test]
    fn git_path_non_utf8_is_escaped() {
        let plain = GitPath::from_bytes("src/main.rs".as_bytes());
        assert_eq!(plain.text, "src/main.rs");
        assert!(!plain.escaped);

        // Valid UTF-8 stays as is, including characters git would quote.
        let unicode = GitPath::from_bytes("dir with space/naïve \"q\"\t🙂".as_bytes());
        assert_eq!(unicode.text, "dir with space/naïve \"q\"\t🙂");
        assert!(!unicode.escaped);

        // Non-UTF-8: git's C-style quoting with core.quotePath=true (OQ-25).
        let raw = b"caf\xe9 \"x\"\\\t\n\x01\x7f\xc3\xa9.txt";
        let escaped = GitPath::from_bytes(raw);
        assert!(escaped.escaped);
        assert_eq!(escaped.text, r#""caf\351 \"x\"\\\t\n\001\177\303\251.txt""#);
        assert_eq!(escaped.to_bytes(), raw.to_vec());
        assert_eq!(plain.to_bytes(), b"src/main.rs".to_vec());
        assert_eq!(
            unicode.to_bytes(),
            "dir with space/naïve \"q\"\t🙂".as_bytes()
        );
    }

    #[test]
    fn file_status_letters_roundtrip() {
        for (letter, status) in [
            (b'A', FileStatus::Added),
            (b'M', FileStatus::Modified),
            (b'D', FileStatus::Deleted),
            (b'R', FileStatus::Renamed),
            (b'T', FileStatus::TypeChanged),
        ] {
            assert_eq!(FileStatus::from_raw(letter), Some(status));
            assert_eq!(status.as_letter(), letter as char);
        }
        assert_eq!(FileStatus::from_raw(b'C'), None);
        assert_eq!(FileStatus::from_raw(b'U'), None);
    }

    #[test]
    fn mode_displays_octal() {
        assert_eq!(Mode(0o100644).to_string(), "100644");
        assert_eq!(Mode(0o120000).to_string(), "120000");
        assert_eq!(Mode(0o040000).to_string(), "040000");
        assert_eq!(Mode::parse_octal("160000"), Some(Mode(0o160000)));
        assert_eq!(Mode::parse_octal("100755"), Some(Mode(0o100755)));
        assert_eq!(Mode::parse_octal("10064x"), None);
        assert_eq!(Mode::parse_octal(""), None);
        assert!(Mode(0o120000).is_symlink());
        assert!(Mode(0o160000).is_submodule());
        assert!(!Mode(0o100644).is_symlink());
    }

    fn change(status: FileStatus, old: Option<&str>, new: Option<&str>) -> FileChange {
        let fmt = ObjectFormat::Sha1;
        let blob = Oid::parse("ce013625030ba8dba906f756967f9e9ca394464a", fmt).unwrap();
        FileChange {
            idx: 0,
            status,
            old_path: old.map(|p| GitPath::from_bytes(p.as_bytes())),
            new_path: new.map(|p| GitPath::from_bytes(p.as_bytes())),
            old_mode: old.map(|_| Mode(0o100644)),
            new_mode: new.map(|_| Mode(0o100644)),
            old_blob: if old.is_some() {
                blob.clone()
            } else {
                Oid::zero(fmt)
            },
            new_blob: if new.is_some() { blob } else { Oid::zero(fmt) },
            similarity: None,
            kind: FileKind::Text,
            generated: false,
        }
    }

    #[test]
    fn file_change_display_path_and_viewed_key() {
        let renamed = change(FileStatus::Renamed, Some("old/a.rs"), Some("new/a.rs"));
        assert_eq!(renamed.display_path(), "new/a.rs");
        let deleted = change(FileStatus::Deleted, Some("gone.rs"), None);
        assert_eq!(deleted.display_path(), "gone.rs");

        // design §9: added files use an all-zero old blob, deleted an all-zero new blob.
        let added = change(FileStatus::Added, None, Some("new.rs"));
        let (path, old, new) = added.viewed_key();
        assert_eq!(path, "new.rs");
        assert!(old.is_zero());
        assert_eq!(new, added.new_blob);
        let (path, old, new) = deleted.viewed_key();
        assert_eq!(path, "gone.rs");
        assert_eq!(old, deleted.old_blob);
        assert!(new.is_zero());
    }

    #[test]
    fn serde_forms_are_stable_and_validated() {
        let oid = ObjectFormat::Sha1.empty_tree();
        let json = serde_json::to_string(&oid).unwrap();
        assert_eq!(json, format!("\"{SHA1_EMPTY}\""));
        assert_eq!(serde_json::from_str::<Oid>(&json).unwrap(), oid);
        assert!(serde_json::from_str::<Oid>("\"ABC\"").is_err());
        assert!(
            serde_json::from_str::<Oid>(&format!("\"{}\"", SHA1_EMPTY.to_uppercase())).is_err()
        );

        assert_eq!(
            serde_json::to_string(&ObjectFormat::Sha256).unwrap(),
            "\"sha256\""
        );
        assert_eq!(serde_json::to_string(&Side::Old).unwrap(), "\"old\"");
        assert_eq!(
            serde_json::to_string(&FileStatus::TypeChanged).unwrap(),
            "\"type_changed\""
        );
        assert_eq!(
            serde_json::to_string(&FileKind::Submodule).unwrap(),
            "\"submodule\""
        );

        let fc = change(FileStatus::Modified, Some("a b.rs"), Some("a b.rs"));
        let back: FileChange = serde_json::from_str(&serde_json::to_string(&fc).unwrap()).unwrap();
        assert_eq!(back, fc);
    }
}
