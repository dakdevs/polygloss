//! Reads of the user's `settings.json` outside the app (design §18): the
//! file is the app's (`polygloss-app`'s `settings::model`), but the MCP server
//! must honor the global `notifications.enabled` before it launches the app to
//! post a notification (design §17, T5.9 #12).
//!
//! [`strip_jsonc`] is the JSONC reader both use: `//` and `/* */` comments and
//! trailing commas are allowed, like Zed's settings files.

use crate::paths::DataPaths;

/// `settings.json` in the config dir.
pub const SETTINGS_FILE: &str = "settings.json";

/// Whether notifications are on in `<config_dir>/settings.json`
/// (`notifications.enabled`, default `true`). A missing, unreadable or
/// invalid file, or a value that is not a boolean, counts as the default, as
/// it does for an app that starts with it.
pub fn notifications_enabled(paths: &DataPaths) -> bool {
    let Ok(text) = std::fs::read_to_string(paths.config_dir.join(SETTINGS_FILE)) else {
        return true;
    };
    notifications_enabled_in(&text)
}

/// [`notifications_enabled`] of a settings text.
pub fn notifications_enabled_in(text: &str) -> bool {
    let json = strip_jsonc(text);
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) else {
        return true;
    };
    value
        .get("notifications")
        .and_then(|n| n.get("enabled"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notifications_default_on_and_follow_the_setting() {
        assert!(notifications_enabled_in(""));
        assert!(notifications_enabled_in("{}"));
        assert!(notifications_enabled_in("not json"));
        assert!(notifications_enabled_in(
            r#"{"notifications": {"enabled": "no"}}"#
        ));
        assert!(notifications_enabled_in(
            r#"{"notifications": {"enabled": true}}"#
        ));
        assert!(!notifications_enabled_in(
            "{\n  // quiet, please\n  \"notifications\": { \"enabled\": false, },\n}\n"
        ));
    }
}
