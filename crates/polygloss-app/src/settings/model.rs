//! The `settings.json` model (design §18; names and defaults provisional,
//! OQ-19). Every section and key is optional: a missing key keeps its
//! default and unknown keys are ignored, so an older or newer file still
//! loads. A value of the wrong type (or an unknown enum spelling) makes the
//! whole file invalid, and the loader keeps the previous settings.

use polygloss_diff::options::{Algorithm, DiffOptions};
use polygloss_diff::word::Granularity;
use polygloss_viewport::{DiffStyle, Indicators, LayoutMode, ViewportOptions};
use serde::{Deserialize, Serialize};

/// Every preference of design §18, with its provisional default.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeSettings,
    pub buffer_font: FontSettings,
    pub diff: DiffSettings,
    pub editor: EditorSettings,
    pub agent_notes: AgentNotesSettings,
    pub notifications: NotificationSettings,
    pub storage: StorageSettings,
    pub updates: UpdateSettings,
}

/// `theme.mode` (`"system"` follows the macOS appearance).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeSettings {
    pub mode: ThemeMode,
    /// Theme used in light mode, by name.
    pub light: String,
    /// Theme used in dark mode, by name.
    pub dark: String,
}

impl Default for ThemeSettings {
    fn default() -> ThemeSettings {
        ThemeSettings {
            mode: ThemeMode::System,
            light: "Pierre Light".to_owned(),
            dark: "Pierre Dark".to_owned(),
        }
    }
}

/// `buffer_font`: the code font.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FontSettings {
    pub family: String,
    pub size: f32,
}

impl Default for FontSettings {
    fn default() -> FontSettings {
        FontSettings {
            family: "Lilex".to_owned(),
            size: 13.0,
        }
    }
}

/// `diff.layout`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LayoutSetting {
    #[default]
    Auto,
    Split,
    Unified,
}

/// `diff.word_diff`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WordDiffSetting {
    #[default]
    Word,
    Char,
    Off,
}

/// `diff.algorithm`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffAlgorithm {
    #[default]
    Myers,
    Histogram,
}

/// `diff.style.indicators`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum IndicatorStyle {
    #[default]
    #[serde(rename = "+-")]
    PlusMinus,
    #[serde(rename = "bars")]
    Bars,
    #[serde(rename = "none")]
    None,
}

/// `diff.style`: Pierre's diff-style options.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DiffStyleSettings {
    pub backgrounds: bool,
    pub indicators: IndicatorStyle,
    pub wrap: bool,
}

impl Default for DiffStyleSettings {
    fn default() -> DiffStyleSettings {
        DiffStyleSettings {
            backgrounds: true,
            indicators: IndicatorStyle::PlusMinus,
            wrap: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DiffSettings {
    pub layout: LayoutSetting,
    /// Auto layout: split at this many code columns or more.
    pub split_min_columns: u32,
    pub word_diff: WordDiffSetting,
    pub algorithm: DiffAlgorithm,
    pub hide_whitespace: bool,
    pub style: DiffStyleSettings,
    /// Files with more changed lines show "Load diff" (design §12.3).
    pub large_file_changed_lines: u32,
    /// Extends the built-in generated-file list.
    pub generated_patterns: Vec<String>,
    pub renames: bool,
    /// Rename similarity threshold in percent (`-M50%`).
    pub rename_threshold: u8,
}

impl Default for DiffSettings {
    fn default() -> DiffSettings {
        DiffSettings {
            layout: LayoutSetting::Auto,
            split_min_columns: 160,
            word_diff: WordDiffSetting::Word,
            algorithm: DiffAlgorithm::Myers,
            hide_whitespace: false,
            style: DiffStyleSettings::default(),
            large_file_changed_lines: 20_000,
            generated_patterns: Vec::new(),
            renames: true,
            rename_threshold: 50,
        }
    }
}

/// `editor.command`: `None` auto-detects (T3.16), e.g. `"zed {path}:{line}"`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorSettings {
    pub command: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentNotesSettings {
    pub hidden: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationSettings {
    pub enabled: bool,
}

impl Default for NotificationSettings {
    fn default() -> NotificationSettings {
        NotificationSettings { enabled: true }
    }
}

/// `storage.prune_reviews_after_days`: `None` is off (OQ-34).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageSettings {
    pub prune_reviews_after_days: Option<u32>,
}

/// `updates.automatic_checks`: set by Sparkle's first-launch prompt.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateSettings {
    pub automatic_checks: Option<bool>,
}

impl Settings {
    /// Parses a `settings.json` text. Like Zed's settings files, `//` and
    /// `/* */` comments and trailing commas are allowed; blank text is the
    /// defaults.
    pub fn parse(text: &str) -> Result<Settings, serde_json::Error> {
        let json = strip_jsonc(text);
        if json.trim().is_empty() {
            return Ok(Settings::default());
        }
        serde_json::from_str(&json)
    }

    /// The viewport options these settings ask for (the theme is the
    /// default; the review tab sets it).
    pub fn viewport_options(&self) -> ViewportOptions {
        let d = &self.diff;
        ViewportOptions {
            layout: match d.layout {
                LayoutSetting::Auto => LayoutMode::Auto,
                LayoutSetting::Split => LayoutMode::Split,
                LayoutSetting::Unified => LayoutMode::Unified,
            },
            split_min_columns: d.split_min_columns,
            word_diff: match d.word_diff {
                WordDiffSetting::Word => Some(Granularity::Word),
                WordDiffSetting::Char => Some(Granularity::Char),
                WordDiffSetting::Off => None,
            },
            diff: DiffOptions {
                algorithm: match d.algorithm {
                    DiffAlgorithm::Myers => Algorithm::Myers,
                    DiffAlgorithm::Histogram => Algorithm::Histogram,
                },
                ignore_whitespace: d.hide_whitespace,
                ..DiffOptions::default()
            },
            style: DiffStyle {
                backgrounds: d.style.backgrounds,
                indicators: match d.style.indicators {
                    IndicatorStyle::PlusMinus => Indicators::PlusMinus,
                    IndicatorStyle::Bars => Indicators::Bars,
                    IndicatorStyle::None => Indicators::None,
                },
                wrap: d.style.wrap,
            },
            code_font: self.buffer_font.family.clone().into(),
            code_font_size: self.buffer_font.size,
            large_file_changed_lines: d.large_file_changed_lines,
            ..ViewportOptions::default()
        }
    }
}

/// `text` without JSONC comments and trailing commas (strings are kept
/// byte for byte), so `serde_json` can read it. Positions of the rest are
/// kept (comments become spaces), so errors still point at the right line.
pub fn strip_jsonc(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                // A string, escapes included.
                out.push(b'"');
                i += 1;
                while i < bytes.len() {
                    let b = bytes[i];
                    out.push(b);
                    i += 1;
                    if b == b'\\' && i < bytes.len() {
                        out.push(bytes[i]);
                        i += 1;
                    } else if b == b'"' {
                        break;
                    }
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    out.push(b' ');
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                out.extend_from_slice(b"  ");
                i += 2;
                while i < bytes.len() && !(bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/')) {
                    out.push(if bytes[i] == b'\n' { b'\n' } else { b' ' });
                    i += 1;
                }
                if i < bytes.len() {
                    out.extend_from_slice(b"  ");
                    i += 2;
                }
            }
            b',' => {
                // Trailing when the next significant byte closes the value.
                let mut j = i + 1;
                loop {
                    while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                        j += 1;
                    }
                    if bytes.get(j) == Some(&b'/') && bytes.get(j + 1) == Some(&b'/') {
                        while j < bytes.len() && bytes[j] != b'\n' {
                            j += 1;
                        }
                    } else if bytes.get(j) == Some(&b'/') && bytes.get(j + 1) == Some(&b'*') {
                        j += 2;
                        while j < bytes.len()
                            && !(bytes[j] == b'*' && bytes.get(j + 1) == Some(&b'/'))
                        {
                            j += 1;
                        }
                        j += 2;
                    } else {
                        break;
                    }
                }
                out.push(if matches!(bytes.get(j), Some(b'}' | b']')) {
                    b' '
                } else {
                    b','
                });
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    // Only ASCII was replaced, and only outside strings.
    String::from_utf8(out).unwrap_or_default()
}
